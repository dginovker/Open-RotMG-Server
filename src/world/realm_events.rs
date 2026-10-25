// GPL-3.0 reference; data/REALM-SRC-LICENSE: realm-src@1855c017eab870d13f0b61b1cf0b9e66a01922ff
// wServer/logic/db/BehaviorDb.{SkullShrine,CubeGod,Pentaract,Sphinx}.cs; NR-Core and Skilly. Ownership replaces nearest-object damage transfer.
use super::*;
use crate::{
    condition,
    encounter::{Anchor, Motion},
    quest,
};

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) struct Orbit {
    pub(super) speed: f32,
    pub(super) radius: f32,
    pub(super) direction: f32,
}

// The shared mover divides by 1.5; preserve the reference Utils.GetSpeed velocity.
fn movement_speed(value: f32) -> f32 {
    (5.55 * value + 0.74) * 1.5
}

pub(super) enum EventAdmission {
    Miss,
    Waiting,
    Spawned(i32),
}

#[derive(Clone)]
pub(super) struct ReservedEvent {
    pub(super) root: Entity,
    pub(super) children: Vec<Entity>,
    pub(super) site: realm_setpieces::EventSite,
}

#[cfg(test)]
#[tokio::test]
async fn reserved_event_sites_remain_original_and_ordinary_quests_exist_at_creation() {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    let mut directory = Directory::test_with_seed(data, content.clone(), 85, 12351).unwrap();
    let original = directory.realms[0].open(&content).unwrap();
    let world = directory.worlds.get_mut(&-3).unwrap();
    assert!(world.players.is_empty());
    for kind in quest::ROOTS {
        let roots: Vec<_> = world.entities.values().filter(|e| e.kind == kind).collect();
        assert!(!roots.is_empty() || matches!(kind, 0x675 | 0x678));
        assert!(roots.iter().all(|e| e.hp > 0 && e.published));
    }
    assert!(!world.event_reserves.is_empty());
    for event in &world.event_reserves {
        assert!(!world.entities.contains_key(&event.root.id));
        let area = world
            .setpieces
            .iter()
            .find(|a| a.distance(event.root.pos) == 0.)
            .unwrap();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                let index = (y * world.map.width + x) as usize;
                assert_eq!(
                    world.map.cells[index].ground, original.cells[index].ground,
                    "unspawned event {:#x} changed ground at {x},{y}",
                    event.root.kind
                );
            }
        }
    }
    let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
    let account = store.list("event-site".into()).await.unwrap();
    let ch = store
        .create(account.id, content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let ent = world
        .entities
        .values()
        .find(|e| e.kind == 0x91f)
        .unwrap()
        .clone();
    let (packets, mut receiver) = mpsc::channel(128);
    let (lost, _) = watch::channel(None);
    let player = world
        .enter_at(
            ch,
            Some(Output { packets, lost }),
            Some(Pos {
                x: ent.pos.x + 3.,
                ..ent.pos
            }),
            AccountPresentation {
                guild: None,
                fame: 0,
                best_fame: BTreeMap::new(),
                credits: 0,
            },
        )
        .unwrap();
    world.updates(player, true).unwrap();
    assert_ne!(world.players[&player].quest, Some(ent.id));
    assert!(world.players[&player].known.contains(&ent.id));
    let reserve = world
        .event_reserves
        .iter()
        .find(|e| e.root.kind == crate::dungeon::trench::HERMIT)
        .unwrap();
    let root = reserve.root.id;
    let (cell, ground) = reserve
        .site
        .cells
        .iter()
        .find(|(index, cell)| {
            cell.ground != original.cells[*index].ground
                && world.walkable(Pos {
                    x: (*index % world.map.width as usize) as f32 + 0.5,
                    y: (*index / world.map.width as usize) as f32 + 0.5,
                })
        })
        .map(|(i, c)| (*i, c.ground))
        .unwrap();
    let point = (
        (cell % world.map.width as usize) as i16,
        (cell / world.map.width as usize) as i16,
    );
    let pos = Pos {
        x: f32::from(point.0) + 0.5,
        y: f32::from(point.1) + 0.5,
    };
    world.players.get_mut(&player).unwrap().pos = pos;
    world.updates(player, false).unwrap();
    world.event_waiting = Some(42);
    world.event_choice = Some(crate::dungeon::trench::HERMIT);
    assert!(matches!(
        world.earn_realm_event(42).unwrap(),
        EventAdmission::Waiting
    ));
    let (x, y) = world.map.spawns[0];
    world.players.get_mut(&player).unwrap().pos = Pos { x, y };
    world.updates(player, false).unwrap();
    let bag = world.spawn_entity(0x0507, pos, vec![]).unwrap();
    world
        .entities
        .get_mut(&bag)
        .unwrap()
        .extra_stats
        .push((54, account.id));
    assert!(matches!(
        world.earn_realm_event(42).unwrap(),
        EventAdmission::Waiting
    ));
    assert!(world.entities.contains_key(&bag));
    world.remove_entity(bag);
    let reserve = world
        .event_reserves
        .iter_mut()
        .find(|e| e.root.id == root)
        .unwrap();
    let marker = reserve.children.remove(
        reserve
            .children
            .iter()
            .position(|e| e.kind == 0xd65)
            .unwrap(),
    );
    assert!(world.earn_realm_event(42).is_err());
    assert_eq!(world.map.cells[cell].ground, original.cells[cell].ground);
    assert!(!world.entities.contains_key(&root));
    world
        .event_reserves
        .iter_mut()
        .find(|e| e.root.id == root)
        .unwrap()
        .children
        .push(marker);
    let before = directory.player_view(player).unwrap().tiles;
    let world = directory.worlds.get_mut(&-3).unwrap();
    let explored = (
        world.players[&player].character.tiles,
        world.players[&player].character.lifetime.tiles,
    );
    while receiver.try_recv().is_ok() {}
    assert!(
        matches!(world.earn_realm_event(42).unwrap(), EventAdmission::Spawned(id) if id == root)
    );
    assert_eq!(world.map.cells[cell].ground, ground);
    assert_eq!(world.quest_members(root, &[0xd65]).len(), 8);
    assert!(
        world.players[&player]
            .tile_updates
            .contains(&(point.0, point.1, ground))
    );
    world.updates(player, false).unwrap();
    assert_eq!(
        (
            world.players[&player].character.tiles,
            world.players[&player].character.lifetime.tiles
        ),
        explored
    );
    assert!(world.players[&player].tile_updates.is_empty());
    let update = std::iter::from_fn(|| receiver.try_recv().ok())
        .find(|p| p.id == wire::UPDATE)
        .unwrap();
    let mut reader = crate::wire::Reader::new(&update.body);
    let count = reader.i16().unwrap();
    let tiles = (0..count)
        .map(|_| {
            (
                reader.i16().unwrap(),
                reader.i16().unwrap(),
                reader.u8().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(tiles.contains(&(point.0, point.1, ground)));
    let after = directory.player_view(player).unwrap().tiles;
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(
        after.iter().find(|t| (t.x, t.y) == point).unwrap().ground,
        ground
    );
    assert_eq!(
        before.iter().find(|t| (t.x, t.y) == point).unwrap().ground,
        original.cells[cell].ground
    );
}

fn mandatory_initial_summons(kind: u16) -> usize {
    match kind {
        crate::dungeon::tomb::SPHINX => 4,
        crate::dungeon::trench::HERMIT => 16,
        0xd56 | 0xd59 | 0xd5f | 0xd50 | 0xe37 => 0,
        _ => unreachable!("reserved event kind validated at creation"),
    }
}

impl World {
    pub(super) fn sphinx_mechanics(&mut self, id: i32, target: Option<Pos>) -> Result<()> {
        let e = &self.entities[&id];
        if e.stage.is_none() {
            if e.kind == 0xd54 {
                self.landmark_birth(id, 0xd55, 4, 4, 4.)?;
                // The named-birth API can refuse at its encounter cap without returning an error.
                ensure!(
                    self.landmark_members(id, 0xd55).len() == 4,
                    "Sphinx requires four owned Reapers before phase entry"
                );
            }
            self.landmark_enter(id, 0);
            if self.entities[&id].kind == 0xd55 {
                self.entities.get_mut(&id).unwrap().next_aux =
                    self.now + self.rng.gen_range(1..=40) * 50;
            }
        }
        let e = &self.entities[&id];
        let stage = e.stage.context("Sphinx phase missing")?;
        let age = self.now - e.activated.context("Sphinx phase clock missing")?;
        if e.kind == 0xd55 {
            let orphan = self.landmark_members(id, 0xd54).is_empty();
            let next = if orphan && stage != 3 {
                Some(3)
            } else {
                match stage {
                    0 if self.now >= e.next_aux => Some(1),
                    1 if target.is_some_and(|p| p.distance(e.pos) < 2.) => Some(2),
                    1 | 2 if self.now >= e.next_aux => Some(0),
                    0..=3 => None,
                    _ => bail!("invalid Reaper phase"),
                }
            };
            if let Some(next) = next {
                self.landmark_enter(id, next);
                let duration = if next == 0 {
                    self.rng.gen_range(1..=40) * 50
                } else {
                    self.rng.gen_range(1..=100) * 50
                };
                self.entities.get_mut(&id).unwrap().next_aux = self.now + duration;
                if next == 3 {
                    self.landmark_fade(id, 1000);
                }
            }
            self.landmark_shield(id, true);
            return Ok(());
        }
        let next = match stage {
            0 if age >= 500 => Some(1),
            1 if age >= 6000 => Some(2),
            2 if age >= 2000 => Some(3),
            3 if age >= 6200 => Some(4),
            4 if age >= 2000 => Some(5),
            5 if age >= 6000 => Some(6),
            6 if e.hp < self.content.objects[&0xd54].hp * 15 / 100 => Some(7),
            6 if age >= 2000 => Some(1),
            7 if age >= 1900 => Some(1),
            0..=7 => None,
            _ => bail!("invalid Sphinx phase"),
        };
        if let Some(next) = next {
            self.landmark_enter(id, next);
            if matches!(next, 2 | 4 | 6) {
                self.flash_inland(id, 0xff00ff0c, 0.25, 8.);
            }
            if next == 5 {
                let e = self.entities.get_mut(&id).unwrap();
                e.script
                    .get_or_insert_with(landmark_ai::Script::default)
                    .choice = 0;
                e.next_aux = self.now + 1000;
            }
            if next == 7 {
                for child in self.landmark_members(id, 0xd55) {
                    self.landmark_enter(child, 3);
                    self.landmark_fade(child, 1000);
                }
            }
        }
        let e = &self.entities[&id];
        let stage = e.stage.unwrap();
        if stage == 5 && self.now >= e.next_aux {
            let choice = u8::from(self.rng.gen_bool(0.5));
            let e = self.entities.get_mut(&id).unwrap();
            e.script.as_mut().context("Sphinx script missing")?.choice = choice;
            e.next_aux = self.now + 1000;
        }
        self.landmark_shield(id, matches!(stage, 0 | 2 | 4 | 6 | 7));
        Ok(())
    }
    pub(super) fn sphinx_movement(&self, id: i32) -> Result<(Motion, f32)> {
        let e = &self.entities[&id];
        if e.kind == 0xd54 {
            return Ok((Motion::Wander, crate::encounter::nr_speed(0.5)));
        }
        Ok(match e.stage.context("Reaper movement phase missing")? {
            0 if e.pos.distance(e.home) > 10. => (
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 10.,
                },
                crate::encounter::nr_speed(3.),
            ),
            0 => (Motion::Wander, crate::encounter::nr_speed(3.)),
            1 | 3 => (Motion::Stationary, 0.),
            2 => (
                Motion::Chase {
                    sight: 10.,
                    stop: 3.,
                },
                crate::encounter::nr_speed(0.7),
            ),
            _ => bail!("invalid Reaper movement phase"),
        })
    }
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
            .filter(|e| quest::EVENT_BOSSES.contains(&e.kind))
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
                crate::encounter::validate(&self.content, kind)?;
                crate::loot::validate(&self.content, kind)?;
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
        // Reserve space, not visible arenas: SpawnEvent paints the site with its boss: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/Oryx.cs#L797-L813
        if let Some(waiting) = self.event_waiting {
            if waiting != source {
                return Ok(EventAdmission::Waiting);
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
        let owned = self.summon_occupancy();
        let clear: Vec<_> = self
            .event_reserves
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                // Admission is not a reservation: actual initial-group dispatch rechecks its budget.
                Some(event.root.kind) == self.event_choice
                    && owned + 1 + event.children.len() + mandatory_initial_summons(event.root.kind)
                        <= 256
                    && self.players.values().all(|p| {
                        p.pos.distance(event.root.pos) >= 32.
                            && event.site.area.distance(p.pos) >= 4.
                    })
                    && self.event_site_clear(&event.site)
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
        let retained = event.clone();
        let (id, kind, pos) = (event.root.id, event.root.kind, event.root.pos);
        for mut entity in std::iter::once(event.root).chain(event.children) {
            entity.next_aux += self.now;
            let id = entity.id;
            assert!(
                self.entities.insert(id, entity).is_none(),
                "reserved event identity reused"
            );
            self.index_actor(id);
        }
        let initialized = match kind {
            crate::dungeon::tomb::SPHINX => self.sphinx_mechanics(id, None),
            crate::dungeon::trench::HERMIT => self.ocean_mechanics(id, None, 0),
            0xd56 | 0xd59 | 0xd5f | 0xd50 | 0xe37 => Ok(()),
            _ => unreachable!("reserved event kind validated at creation"),
        };
        if let Err(error) = initialized {
            let born: Vec<_> = self
                .actors()
                .filter(|e| e.quest == Some(id) || *e.parent == Some(id))
                .map(|e| e.id)
                .collect();
            for child in born.into_iter().rev() {
                self.remove_entity(child);
            }
            let rollback = self.swap_event_site(&previous_site);
            self.event_reserves.insert(index, retained);
            rollback.context("earned event site rollback refused")?;
            return Err(error.context(
                "earned event initial group refused; original reserve retained for retry",
            ));
        }
        let tiles: Vec<_> = retained
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
        self.oryx_notice(match kind {
            0xd56 => "Your futile efforts are no match for a Skull Shrine!",
            0xd59 => "Your meager abilities cannot possibly challenge a Cube God!",
            0xd5f => "Behold my Pentaract, and despair!",
            0xd50 => "Cower in fear of my Lord of the Lost Lands!",
            0xd54 => "A Grand Sphinx is more than a match for this rabble.",
            0xd61 => "You stand NO CHANCE against my mighty Hermit God!",
            0xe37 => "A Ghost Ship has entered the realm.",
            _ => unreachable!("reserved event kind validated at creation"),
        });
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
    pub(super) fn populate_realm_events(&mut self) -> Result<()> {
        for kind in 0xd56..=0xd60 {
            quest::validate(&self.content, kind)?;
            crate::loot::validate(&self.content, kind)?;
        }
        // Two independent prebuilt sites enforce the approved lifetime cap without reusing dead identities.
        for kind in quest::EVENTS.into_iter().chain(quest::EVENTS) {
            let pos = self.place_named_setpiece(kind, &[1])?;
            let root = self.spawn_entity(kind, pos, vec![])?;
            let e = self.entities.get_mut(&root).unwrap();
            e.quest = Some(root);
            e.stage = Some(0);
            e.next_aux = match kind {
                0xd56 => 500,
                0xd59 => 1500,
                _ => 0,
            };
            if kind == 0xd5f {
                e.condition |= condition::bit(condition::INVINCIBLE);
                for n in 0..5 {
                    let a = n as f32 * std::f32::consts::TAU / 5.;
                    let tower = Pos {
                        x: pos.x + ((a.cos() * 15. + 17.) as i32 - 17) as f32,
                        y: pos.y + ((a.sin() * 15. + 17.) as i32 - 17) as f32,
                    };
                    let id = self.spawn_entity(0xd5e, tower, vec![])?;
                    let e = self.entities.get_mut(&id).unwrap();
                    e.quest = Some(root);
                    e.home = tower;
                    e.stage = Some(0);
                }
            }
            tracing::info!(entity = root, kind, "finite Realm event placed");
        }
        Ok(())
    }
    fn event_birth(&mut self, id: i32, kind: u16, count: usize, radius: f32) -> Result<Vec<i32>> {
        let old = self.quest_members(id, &[kind]);
        let e = &self.entities[&id];
        self.quest_birth(
            (e.quest.context("event ownership missing")?, e.home, e.pos),
            kind,
            usize::MAX,
            count,
            radius,
        )?;
        let children: Vec<_> = self
            .quest_members(id, &[kind])
            .into_iter()
            .filter(|id| !old.contains(id))
            .collect();
        let home = self.entities[&id].pos;
        for child in &children {
            let orbit = match kind {
                0xd58 => Some((1.5, 0.6, 15., 10., true)),
                0xd5a => Some((0.375, 0.075, 10., 5., false)),
                0xd5b => Some((1.05, 0.15, 5., 3., false)),
                0xd5c => Some((1.05, 0.15, 7.5, 3., false)),
                _ => None,
            }
            .map(
                |(speed, variance, radius, spread, random_direction)| Orbit {
                    speed: movement_speed(speed + self.rng.gen_range(-variance..variance)),
                    radius: radius + self.rng.gen_range(-spread..spread),
                    direction: if random_direction && self.rng.gen_bool(0.5) {
                        1.
                    } else {
                        -1.
                    },
                },
            );
            let phase = self.rng.gen_range(0.0..std::f32::consts::TAU);
            let e = self.entities.get_mut(child).unwrap();
            e.orbit = orbit;
            e.phase = phase;
            e.home = home;
            e.expires.set(None, *child, &mut self.timed_ids);
            e.activated = None;
            if kind == 0xd5a {
                e.next_aux = self.now + 1000;
            }
        }
        Ok(children)
    }
    fn event_reproduce(&mut self, id: i32, kind: u16, radius: f32, maximum: usize) -> Result<()> {
        let pos = self.entities[&id].pos;
        // Reference Reproduce caps nearby density, not the number born to one parent.
        if self
            .actors()
            .filter(|e| e.kind == kind && e.pos.distance(pos) < radius)
            .count()
            < maximum
        {
            self.event_birth(id, kind, 1, 0.)?;
        }
        Ok(())
    }
    pub(super) fn event_mechanics(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let kind = e.kind;
        if kind == 0xd5c {
            let nearby = |kind| {
                self.quest_members(id, &[kind])
                    .into_iter()
                    .any(|other| self.entities[&other].pos.distance(e.pos) < 10.)
            };
            let follow = e.stage == Some(1);
            if !follow && !nearby(0xd5a) {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.next_aux = self.now + 5000;
            } else if follow && (!nearby(0xd5b) || self.now >= e.next_aux) {
                self.entities.get_mut(&id).unwrap().stage = Some(0);
            }
            return Ok(());
        }
        if kind == 0xd5d {
            if self.now >= e.next_aux {
                let pos = e.pos;
                let target = self
                    .event_target(pos, 20.)
                    .filter(|t| t.x != pos.x && t.y != pos.y);
                let e = self.entities.get_mut(&id).unwrap();
                if let Some(target) = target {
                    let distance = pos.distance(target);
                    let c = (64. - distance * distance).abs().sqrt() / 2.;
                    e.home = Pos {
                        x: (pos.x + target.x) / 2. + c * (pos.y - target.y) / distance,
                        y: (pos.y + target.y) / 2. + c * (target.x - pos.x) / distance,
                    };
                    e.stage = Some(1);
                    e.goal = None;
                    let speed = movement_speed(2.) / 1.5
                        * if e.conditions(self.now) & condition::bit(condition::SLOWED) != 0 {
                            0.5
                        } else {
                            1.
                        };
                    e.next_aux =
                        self.now + (std::f32::consts::TAU * 8. / speed * 1000.).ceil() as u64;
                } else {
                    e.stage = Some(0);
                    e.next_aux = self.now + 5000;
                }
            }
            return Ok(());
        }
        if kind == 0xd5e {
            if self.now >= e.stage.unwrap_or(0) {
                let eyes = self
                    .quest_members(id, &[0xd5d])
                    .into_iter()
                    .filter(|eye| self.entities[eye].parent == Some(id))
                    .count();
                let count = if e.stage.unwrap_or(0) == 0 {
                    5usize.saturating_sub(eyes)
                } else {
                    usize::from(eyes < 5)
                };
                for eye in self.event_birth(id, 0xd5d, count, 1.)? {
                    let e = self.entities.get_mut(&eye).unwrap();
                    e.parent.set(Some(id), eye, &mut self.timed_ids);
                    e.home = e.pos;
                    e.stage = Some(0);
                }
                self.entities.get_mut(&id).unwrap().stage = Some(self.now + 5000);
            }
            let e = &self.entities[&id];
            if self.now >= e.next_aux
                && e.conditions(self.now) & condition::bit(condition::STUNNED) == 0
            {
                let pos = e.pos;
                let target = self.event_target(pos, 8.);
                if let Some(target) = target {
                    self.throw_grenade(id, target, 4., 100)?;
                }
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 5000;
            }
        } else if self.now >= e.next_aux {
            match kind {
                0xd56 => {
                    self.event_reproduce(id, 0xd57, 40., 20)?;
                    self.event_reproduce(id, 0xd58, 40., 20)?;
                }
                0xd59 => self.event_reproduce(id, 0xd5a, 30., 10)?,
                0xd5a => {
                    self.event_reproduce(id, 0xd5b, 12., 10)?;
                    self.event_reproduce(id, 0xd5c, 30., 10)?;
                }
                _ => return Ok(()),
            }
            self.entities.get_mut(&id).unwrap().next_aux = self.now
                + match kind {
                    0xd56 => 500,
                    0xd59 => 1500,
                    _ => 1000,
                };
        }
        Ok(())
    }
    pub(super) fn event_tower_death(&mut self, defeated: &Entity) -> Result<()> {
        let tag = defeated.quest.context("Pentaract tower has no encounter")?;
        let root = self
            .entities
            .get_mut(&tag)
            .context("Pentaract controller absent during tower defeat")?;
        ensure!(
            root.kind == 0xd5f,
            "Pentaract tower owner is not its controller"
        );
        for (character, damage) in &defeated.damage {
            *root.damage.entry(*character).or_default() += *damage;
        }
        let corpse = self.spawn_entity(0xd60, defeated.pos, vec![])?;
        let e = self.entities.get_mut(&corpse).unwrap();
        e.quest = Some(tag);
        e.home = defeated.home;
        e.damage = defeated.damage.clone();
        e.activated = Some(self.now);
        e.condition |= condition::bit(condition::INVINCIBLE);
        tracing::info!(
            tower = defeated.id,
            corpse,
            quest = tag,
            revive_at = self.now + 15000,
            "Pentaract tower down"
        );
        Ok(())
    }
    pub(super) fn event_step(&mut self) {
        let roots: Vec<_> = self
            .pentaract_ids
            .iter()
            .filter(|id| {
                self.non_population_ids.contains(id) && self.entities[id].quest == Some(**id)
            })
            .copied()
            .collect();
        for root in roots {
            if let Err(error) = self.pentaract_step(root) {
                tracing::error!(quest=root,%error,"Pentaract clock refused");
            }
        }
    }
    fn pentaract_step(&mut self, root: i32) -> Result<()> {
        let towers = self.quest_members(root, &[0xd5e]);
        let corpses = self.quest_members(root, &[0xd60]);
        ensure!(
            towers.len() + corpses.len() == 5,
            "Pentaract lost a tower identity"
        );
        if towers.is_empty() {
            for corpse in corpses {
                self.defeat_entity(corpse)?;
            }
            for eye in self.quest_members(root, &[0xd5d]) {
                self.remove_entity(eye);
            }
            self.finish_timed_defeat(root)?;
            tracing::info!(
                quest = root,
                "Pentaract completed after all five towers fell"
            );
        } else {
            for corpse in corpses {
                let e = &self.entities[&corpse];
                if e.conditions(self.now) & condition::bit(condition::STASIS) != 0 {
                    *self
                        .entities
                        .get_mut(&corpse)
                        .unwrap()
                        .activated
                        .as_mut()
                        .context("Pentaract corpse has no clock")? += 50;
                    continue;
                }
                if self.now - e.activated.context("Pentaract corpse has no clock")? >= 15000 {
                    let tower = self.quest_transform(corpse, 0xd5e)?;
                    let e = self.entities.get_mut(&tower).unwrap();
                    e.damage.clear();
                    e.stage = Some(0);
                    e.next_aux = self.now;
                    tracing::info!(quest = root, tower, "Pentaract tower revived");
                }
            }
        }
        Ok(())
    }
    fn event_target(&self, pos: Pos, range: f32) -> Option<Pos> {
        self.players
            .values()
            .filter(|p| {
                p.active()
                    && !p.paused
                    && p.conditions(self.now) & condition::bit(condition::INVISIBLE) == 0
            })
            .map(|p| p.pos)
            .chain(self.decoy_positions())
            .filter(|p| p.distance(pos) < range)
            .min_by(|a, b| a.distance(pos).total_cmp(&b.distance(pos)))
    }
    pub(super) fn event_movement(&self, id: i32, _t: u64) -> (Motion, f32) {
        let e = &self.entities[&id];
        let kind = e.kind;
        let own = |kind, radius| {
            self.quest_members(id, &[kind])
                .into_iter()
                .any(|id| self.entities[&id].pos.distance(e.pos) < radius)
        };
        let orbit = |kind| {
            let o = e
                .orbit
                .expect("event orbit parameters were not initialized at birth");
            (
                Motion::Orbit {
                    anchor: Anchor::QuestMember(kind),
                    radius: o.radius,
                },
                o.speed,
            )
        };
        match kind {
            0xd56 | 0xd5e..=0xd60 => (Motion::Stationary, 0.),
            0xd57 if own(0xd56, 30.) && !own(0xd56, 15.) => (
                Motion::Protect {
                    anchor: Anchor::QuestMember(0xd56),
                    distance: 15.,
                },
                movement_speed(0.3),
            ),
            0xd58 if own(0xd56, 40.) => orbit(0xd56),
            0xd5a if own(0xd59, 30.) => orbit(0xd59),
            0xd5b if own(0xd5a, 15.) => orbit(0xd5a),
            0xd5c if e.stage == Some(1) => (
                Motion::Chase {
                    sight: 10.,
                    stop: 1.,
                },
                movement_speed(0.75),
            ),
            0xd5c if own(0xd5a, 40.) => orbit(0xd5a),
            0xd5d if e.stage == Some(1) => (
                Motion::Orbit {
                    anchor: Anchor::Home,
                    radius: 8.,
                },
                movement_speed(2.),
            ),
            0xd5d if own(0xd5e, 20.) => (
                Motion::Protect {
                    anchor: Anchor::QuestMember(0xd5e),
                    distance: 6.,
                },
                movement_speed(2.),
            ),
            0xd5d => (Motion::Wander, movement_speed(2.) * 0.2),
            0xd58 => (Motion::Wander, movement_speed(1.5)),
            0xd5a => (Motion::Wander, movement_speed(0.375)),
            0xd5b | 0xd5c => (Motion::Wander, movement_speed(1.05)),
            _ => (Motion::Wander, movement_speed(0.3)),
        }
    }
    pub(super) fn event_shots(&self, id: i32, target: Pos, t: u64) -> Result<Vec<(u8, f32)>> {
        let e = &self.entities[&id];
        let heading = (target.y - e.pos.y).atan2(target.x - e.pos.x);
        let fan = |i, n| quest::fan(i, n, 10., heading);
        let mut shots = Vec::new();
        match e.kind {
            0xd56 | 0xd59 if t.is_multiple_of(750) => shots.extend(fan(0, 9)),
            0xd57 | 0xd58 if t.is_multiple_of(750) => shots.extend(fan(0, 2)),
            0xd5a if t.is_multiple_of(750) => shots.extend(fan(0, 4)),
            0xd5b if t.is_multiple_of(500) => shots.extend(fan(0, 1)),
            0xd5c if t.is_multiple_of(1500) => shots.extend(fan(0, 1)),
            0xd5d if t.is_multiple_of(1000) => shots.extend(fan(0, 1)),
            _ => {}
        }
        match e.kind {
            0xd59 if t.is_multiple_of(1500) => shots.extend(fan(1, 4)),
            0xd5a if t.is_multiple_of(1500) => shots.extend(fan(1, 1)),
            0xd5c if t.is_multiple_of(500) => shots.extend(fan(1, 2)),
            _ => {}
        }
        let prediction = match e.kind {
            0xd56 | 0xd5c => 1.,
            0xd59 => 0.5,
            _ => 0.,
        };
        self.predict_enemy_shots(id, target, &mut shots, |_| prediction)?;
        Ok(shots)
    }
}
