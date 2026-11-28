// GPL-3.0 reference; data/REALM-SRC-LICENSE: realm-src@1855c017eab870d13f0b61b1cf0b9e66a01922ff
// wServer/logic/db/BehaviorDb.{SkullShrine,CubeGod,Pentaract,Sphinx}.cs; NR-Core and Skilly. Ownership replaces nearest-object damage transfer.
use super::*;
use crate::content::game::SiteRole;

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) struct Orbit {
    pub(super) speed: f32,
    pub(super) radius: f32,
    pub(super) direction: f32,
}

pub(super) enum EventAdmission {
    Miss,
    Waiting,
    Spawned(i32),
}

#[derive(Clone)]
pub(super) struct ReservedEvent {
    pub(super) game: Arc<crate::content::game::GameData>,
    pub(super) root: Entity,
    pub(super) children: Vec<Entity>,
    pub(super) site: realm_setpieces::EventSite,
}

impl World {
    pub(super) fn reserve_realm_events(&mut self) -> Result<()> {
        ensure!(
            self.now == 0 && self.players.is_empty(),
            "event reserves require an unentered Realm"
        );
        let landmarks: Vec<_> = self
            .entities
            .values()
            .filter(|e| matches!(e.kind, 0x678..=0x67c | 0xd50..=0xd55))
            .map(|e| e.id)
            .collect();
        for id in landmarks {
            self.landmark_bind(id)?;
        }
        let roots: Vec<_> = self
            .entities
            .values()
            .filter(|e| self.xml_game().site_role(e.kind) == Some(SiteRole::Event))
            .map(|e| e.id)
            .collect();
        for id in roots {
            let children: Vec<_> = self
                .entities
                .values()
                .filter(|e| e.quest == Some(id) && e.id != id)
                .map(|e| e.id)
                .collect();
            for entity in std::iter::once(&id).chain(&children) {
                let kind = self.entities[entity].kind;
                self.xml_declared_admission(kind, "Realm event roster")?;
            }
            let children = children
                .into_iter()
                .map(|id| self.remove_entity(id))
                .collect();
            let root = self.remove_entity(id);
            let index = self
                .event_sites
                .iter()
                .position(|site| site.area.distance(root.pos) == 0.)
                .context("reserved event has no original site snapshot")?;
            let original = self.event_sites.remove(index);
            let site = self.swap_event_site(&original)?;
            ensure!(
                root.quest == Some(id) && root.hp > 0,
                "event reserve lacks a living owned root"
            );
            tracing::info!(
                entity = id,
                kind = root.kind,
                "Realm event reserved outside active indexes"
            );
            self.event_reserves.push(ReservedEvent {
                game: self
                    .content
                    .game
                    .as_ref()
                    .expect("validated GameData")
                    .clone(),
                root,
                children,
                site,
            });
        }
        ensure!(
            self.event_sites.is_empty(),
            "unbound original event site snapshot"
        );
        Ok(())
    }
    pub(super) fn earn_realm_event(&mut self, source: i32) -> Result<EventAdmission> {
        let mut identities = BTreeSet::new();
        for event in &self.event_reserves {
            ensure!(
                Arc::ptr_eq(
                    &event.game,
                    self.content
                        .game
                        .as_ref()
                        .context("reserved event GameData absent")?
                ),
                "reserved event owning catalog changed: root={} kind={:#x}",
                event.root.id,
                event.root.kind
            );
            ensure!(
                event.root.quest == Some(event.root.id),
                "reserved event root lost actual encounter ownership"
            );
            for entity in std::iter::once(&event.root).chain(&event.children) {
                ensure!(
                    identities.insert(entity.id) && !self.entities.contains_key(&entity.id),
                    "reserved event identity reused before selection/site mutation"
                );
                ensure!(
                    entity.quest == Some(event.root.id),
                    "reserved event child lost actual encounter ownership"
                );
                self.xml_preflight_admission(entity.kind, entity.pos)?;
            }
            self.preflight_event_scenery(&event.site)?;
        }
        // Reserve space, not visible arenas: SpawnEvent paints the site with its boss: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/Oryx.cs#L797-L813
        if let Some(waiting) = self.event_waiting {
            // Holding other defeats behind a blocked site froze closure credit for hours in production.
            if waiting != source {
                return Ok(EventAdmission::Miss);
            }
        } else {
            if self.event_reserves.is_empty() || !self.event_rng.gen_bool(0.25) {
                return Ok(EventAdmission::Miss);
            }
            self.event_waiting = Some(source);
            tracing::info!(source, "quest defeat earned a pending Realm event");
        }
        if self.event_choice.is_none() {
            let kinds: BTreeSet<_> = self.event_reserves.iter().map(|e| e.root.kind).collect();
            let kinds: Vec<_> = kinds.into_iter().collect();
            self.event_choice = Some(
                *kinds
                    .choose(&mut self.event_rng)
                    .context("pending event has no eligible type")?,
            );
        }
        let entry_children: BTreeMap<u16, usize> = self
            .event_reserves
            .iter()
            .map(|e| (e.root.kind, self.xml_entry_children(e.root.kind)))
            .collect();
        let clear: Vec<_> = self
            .event_reserves
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                let root_index =
                    event.root.pos.y as usize * self.map.width as usize + event.root.pos.x as usize;
                let native_root = matches!(event.root.kind, 0xd59 | 0xd50 | 0xd5f)
                    || event.root.kind == 0xd56
                        && !event
                            .site
                            .cells
                            .iter()
                            .any(|(index, _)| *index == root_index);
                // Use the same budget as XML births; the old summon-only cap counted ordinary fauna too.
                Some(event.root.kind) == self.event_choice
                    && self.xml_live_count() + 1 + event.children.len() + entry_children[&event.root.kind]
                        <= xml::ACTOR_LIMIT
                    && self.players.values().all(|p| {
                        p.pos.distance(event.root.pos) >= 32.
                            && event.site.area.distance(p.pos) >= 4.
                    })
                    && self.event_site_clear(&event.site)
                    // An eroded birth cell is outside the painted-site swap.
                    && (!native_root
                        || self.walkable(event.root.pos)
                            && self.setpiece_scenery_clear(root_index))
            })
            .map(|(index, _)| index)
            .collect();
        let Some(index) = clear.first().copied() else {
            return Ok(EventAdmission::Waiting);
        };
        let event = self.event_reserves.remove(index);
        let previous_site = match self.swap_event_site(&event.site) {
            Ok(previous) => previous,
            Err(error) => {
                self.event_reserves.insert(index, event);
                return Err(error.context("event site refused; original reserve retained"));
            }
        };
        let (id, kind, pos) = (event.root.id, event.root.kind, event.root.pos);
        for entity in std::iter::once(event.root).chain(event.children) {
            let id = entity.id;
            assert!(
                self.entities.insert(id, entity).is_none(),
                "reserved event identity reused"
            );
            self.index_actor(id);
            self.xml_admit(id)
                .expect("preflighted reserved event admission");
        }
        let tiles: Vec<_> = event
            .site
            .cells
            .iter()
            .zip(&previous_site.cells)
            .filter(|((_, next), (_, previous))| next.ground != previous.ground)
            .map(|((index, cell), _)| {
                (
                    (*index % self.map.width as usize) as i16,
                    (*index / self.map.width as usize) as i16,
                    cell.ground,
                )
            })
            .collect();
        for player in self.players.values_mut() {
            let known = tiles
                .iter()
                .filter(|(x, y, _)| player.tiles.contains(&(*x, *y)))
                .copied()
                .collect::<Vec<_>>();
            player.tile_updates.extend(known);
        }
        self.event_waiting = None;
        self.event_choice = None;
        tracing::info!(
            source,
            entity = id,
            kind,
            x = pos.x,
            y = pos.y,
            remaining = self.event_reserves.len(),
            "quest defeat summoned a Realm event"
        );
        Ok(EventAdmission::Spawned(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "optimized loaded-Realm CPU comparison"]
    fn hermit_god_npc_phase() -> Result<()> {
        use crate::performance::Measure;
        const PLAYERS: usize = 47;
        const TICKS: usize = 200;

        let mut directory =
            Directory::prepared(Arc::new(crate::world::test_content()), 85, 1370, 1)?;
        let world = &mut directory.worlds[&-3];
        let center = world
            .event_reserves
            .iter()
            .find(|e| e.root.kind == 0xd61)
            .context("Hermit event absent")?
            .root
            .pos;
        let candidates: Vec<_> = world.actor_ids.iter().copied().collect();
        let mut players = Vec::new();
        for i in 0..PLAYERS {
            let position = candidates
                .iter()
                .cycle()
                .skip(i * candidates.len() / PLAYERS)
                .find_map(|id| {
                    let e = &world.entities[id];
                    (e.pos.distance(center) > 60. && world.walkable(e.pos)).then_some(e.pos)
                })
                .context("no inhabitable spot away from the event")?;
            let mut ch = Character::fresh(world.content.get(crate::content::WIZARD)?)?;
            ch.id = i as i32 + 1;
            ch.account = ch.id;
            let id = world.enter(ch, None)?;
            world.players.get_mut(&id).unwrap().pos = position;
            players.push(id);
        }
        println!(
            "layout={:?} {}x{} actors={} entities={} players={PLAYERS} event={center:?}",
            world.realm_layout,
            world.map.width,
            world.map.height,
            world.actor_ids.len(),
            world.entities.len()
        );
        let run = |world: &mut World, label| -> Result<()> {
            let mut times = [0.; TICKS];
            for ms in &mut times {
                world.now += 50;
                let start = Measure::start();
                world.xml_step()?;
                world.native_expiry();
                *ms = start
                    .finish()
                    .cpu
                    .context("thread CPU clock unavailable")?
                    .as_secs_f64()
                    * 1000.;
                // Keep projectile storage bounded without adding combat or networking to the NPC sample.
                world
                    .bullets
                    .retain(|b| ((world.now - b.born) as f32) < b.spec.lifetime);
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{label}: mean={:.3} p50={:.3} p95={:.3} max={:.3} ms",
                times.iter().sum::<f64>() / TICKS as f64,
                times[TICKS / 2],
                times[TICKS * 95 / 100 - 1],
                times[TICKS - 1]
            );
            Ok(())
        };
        // Choose a pending event deterministically; the production site checks and publication still run.
        world.event_waiting = Some(1);
        world.event_choice = Some(0xd61);
        run(world, "baseline")?;
        let EventAdmission::Spawned(root) = world.earn_realm_event(1)? else {
            bail!("available Hermit event did not summon");
        };
        for (&id, dx) in players.iter().take(2).zip([-8., 8.]) {
            world.players.get_mut(&id).unwrap().pos = Pos {
                x: center.x + dx,
                y: center.y + 7.,
            };
        }
        run(world, "Hermit")?;
        assert_eq!(world.entities[&root].kind, 0xd61);
        for kind in [0xd62, 0xd64, 0xd65, 0xd66] {
            assert!(
                world.entities.values().any(|e| {
                    e.kind == kind
                        && std::iter::successors(*e.parent, |id| *world.entities[id].parent)
                            .any(|id| id == root)
                }),
                "Hermit child {kind:#x} absent after PlaceMap and encounter startup"
            );
        }
        Ok(())
    }

    #[test]
    fn pending_event_does_not_withhold_other_quest_credit() {
        let content = Arc::new(crate::world::test_content());
        let map = Map {
            elevation: None,
            width: 8,
            height: 8,
            cells: vec![
                crate::content::Cell {
                    ground: 0x18,
                    object: None,
                    config: String::new(),
                    terrain: 0,
                    region: 0,
                };
                64
            ],
            spawns: vec![(4.5, 4.5)],
        };
        let mut world = World::new(content, Arc::new(map)).unwrap();
        world.event_waiting = Some(1);
        assert!(matches!(
            world.earn_realm_event(2).unwrap(),
            EventAdmission::Miss
        ));
        assert_eq!(world.event_waiting, Some(1));
    }

    #[test]
    fn event_sites_accept_inland_biomes_and_reject_lowlands_and_shore() {
        let content = Arc::new(crate::world::test_content());
        let mut map = Map {
            elevation: Some(vec![200; 256 * 256].into()),
            width: 256,
            height: 256,
            cells: vec![
                crate::content::Cell {
                    ground: 0x18,
                    object: None,
                    config: String::new(),
                    terrain: 0,
                    region: 0,
                };
                256 * 256
            ],
            spawns: vec![(8.5, 8.5)],
        };
        for terrain in 0..=12 {
            for cell in &mut map.cells {
                cell.terrain = terrain;
            }
            for (kinds, copies) in [
                (&[0xd56, 0xd59, 0xd5f][..], 2),
                (&[0xd54][..], 1),
                (&[0xd50][..], 1),
            ] {
                let mut world =
                    World::in_domain(content.clone(), Arc::new(map.clone()), WorldDomain::Realm)
                        .unwrap();
                world.rng = rand::rngs::StdRng::seed_from_u64(1370);
                let result = (|| -> Result<()> {
                    for _ in 0..copies {
                        for &kind in kinds {
                            let rule = content.game.as_ref().unwrap().named_site(kind).unwrap();
                            world.allocate_site(rule)?;
                        }
                    }
                    Ok(())
                })();
                if !(1..=7).contains(&terrain) {
                    let error = result.unwrap_err();
                    assert!(
                        format!("{error:#}").contains("selected habitat"),
                        "{error:#}"
                    );
                    assert!(world.event_sites.is_empty());
                    continue;
                }
                result.unwrap_or_else(|error| {
                    panic!("event kinds {kinds:x?} refused inland terrain {terrain}: {error:#}")
                });
                for kind in kinds {
                    let roots: Vec<_> = world
                        .entities
                        .values()
                        .filter(|e| e.kind == *kind)
                        .collect();
                    assert_eq!(roots.len(), copies);
                    for root in roots {
                        assert_eq!(
                            world.map.cell(root.pos.x, root.pos.y).unwrap().terrain,
                            terrain
                        );
                        assert!(root.hp > 0);
                    }
                }
                world.reserve_realm_events().unwrap();
                assert_eq!(world.event_reserves.len(), kinds.len() * copies);
                assert!(world.event_sites.is_empty());
                assert!(!world.entities.values().any(|e| kinds.contains(&e.kind)));
                for event in &world.event_reserves {
                    assert_eq!(event.root.quest, Some(event.root.id));
                    assert!(event.root.hp > 0);
                }
                println!(
                    "terrain={terrain} kinds={kinds:x?}: prepared and reserved {} living events",
                    kinds.len() * copies
                );
            }
        }
    }
}
