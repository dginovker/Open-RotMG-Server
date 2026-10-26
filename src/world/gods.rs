use super::*;
use crate::{
    condition,
    encounter::{self, Anchor, Motion},
};
pub(super) const GODS: &[u16] = &[
    0x651, 0x652, 0x654, 0x655, 0x656, 0x657, 0x658, 0x659, 0x91a, 0x6d8,
];
pub(super) const CONSTRUCTS: &[u16] = &[0x905, 0x906, 0x907];

impl World {
    pub(super) fn god_movement(
        &self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<(Motion, f32)> {
        let target = target.into();
        let e = &self.entities[&id];
        if e.pos.distance(e.home) > 24. {
            return Ok((
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 1.,
                },
                6.,
            ));
        }
        let chase = |speed, sight, stop| (Motion::Chase { sight, stop }, speed);
        if e.kind == 0x651 && self.name != "Abyss of Demons" {
            let profile = encounter::profile(e.kind).expect("White Demon profile");
            return Ok((profile.motion, profile.speed));
        }
        Ok(match e.kind {
            0x652 => (Motion::Wander, 2.),
            0x653 => {
                let camp = &self.camps[e.camp.context("Sprite Child has no bounded family")?];
                let leader = self.population[camp.start]
                    .entity
                    .and_then(|id| self.entities.get(&id));
                // Freezing at the catch-up radius strands siblings; the reference yields to wandering: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.Mountain.cs#L66-L70
                if leader.is_some_and(|leader| e.pos.distance(leader.pos) > 2.) {
                    (
                        Motion::Protect {
                            anchor: Anchor::CampLeader,
                            distance: 2.,
                        },
                        4.,
                    )
                } else {
                    (Motion::Wander, 4.)
                }
            }
            // Legacy orbit constructors take radius first; Swirling's center never follows a player.
            0x905 => (
                Motion::Orbit {
                    anchor: Anchor::Home,
                    radius: 2.,
                },
                5.,
            ),
            0x906 | 0x907 => {
                let camp = &self.camps[e.camp.context("construct lacks its trio")?];
                if self.population[camp.start]
                    .entity
                    .is_some_and(|id| self.entities.contains_key(&id))
                {
                    (
                        Motion::Orbit {
                            anchor: Anchor::CampLeader,
                            radius: 4.,
                        },
                        5.,
                    )
                } else {
                    (Motion::Wander, 8.)
                }
            }
            0x91a if e.hp < 400 => (Motion::Stationary, 0.),
            0x91a if elapsed % 3750 < 2500 => chase(6., 7., 0.),
            0x91a => (Motion::Stationary, 0.),
            0x6d8 => match elapsed % 6000 {
                0..2000 => chase(8., 15., 5.),
                2000..4000 => (
                    Motion::Orbit {
                        anchor: Anchor::Player,
                        radius: 5.,
                    },
                    8.,
                ),
                _ => (Motion::Tangential, 8.),
            },
            kind if GODS.contains(&kind) => {
                // Pursuit is shorter than firing (White 9/10, Medusa 7/12): https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.Mountain.cs
                let sight = if kind == 0x651 { 9. } else { 7. };
                if target.is_none_or(|p| e.pos.distance(p) > sight) {
                    (Motion::Wander, 4.)
                } else {
                    chase(
                        match kind {
                            0x655 | 0x656 | 0x658 => 10.,
                            0x659 => 4.,
                            _ => 6.,
                        },
                        sight,
                        4.,
                    )
                }
            }
            _ => bail!("unsupported Godlands movement {:#x}", e.kind),
        })
    }
    pub(super) fn god_mechanics(&mut self, id: i32, elapsed: u64) -> Result<()> {
        let e = &self.entities[&id];
        let (kind, hp, pos, expiry) = (e.kind, e.hp, e.pos, e.expires);
        let stage = encounter::stage(kind, hp, elapsed);
        if e.stage != Some(stage) {
            self.entities.get_mut(&id).unwrap().stage = Some(stage);
            tracing::debug!(entity = id, kind, stage, "Godlands phase entered");
        }
        if kind == 0x91a {
            let dying = hp < 400;
            if dying && expiry.is_none() {
                let e = self.entities.get_mut(&id).unwrap();
                e.expires
                    .set(Some(self.now + 1000), id, &mut self.timed_ids);
                e.next_shot = self.now;
                let mut out = Writer::new();
                out.u8(15)
                    .i32(id)
                    .f32(0.3)
                    .f32(3.)
                    .f32(0.)
                    .f32(0.)
                    .u32(0xffff0000);
                for p in self
                    .players
                    .values()
                    .filter(|p| p.known.contains(&id) && p.pos.distance(pos) < 20.)
                {
                    send(p, wire::SHOW_EFFECT, out.0.clone());
                }
                tracing::debug!(
                    entity = id,
                    deadline = self.now + 1000,
                    "Djinn self-destruction telegraphed"
                );
            }
            let e = self.entities.get_mut(&id).unwrap();
            if dying || elapsed % 3750 < 2500 {
                e.condition |= condition::bit(condition::INVULNERABLE);
            } else {
                e.condition &= !condition::bit(condition::INVULNERABLE);
            }
        }
        if CONSTRUCTS.contains(&kind) {
            if kind != 0x905 && hp < 400 && self.entities[&id].expires.is_none() {
                let e = self.entities.get_mut(&id).unwrap();
                e.expires.set(Some(self.now + 50), id, &mut self.timed_ids);
                e.next_shot = self.now;
                tracing::debug!(entity = id, "wounded construct bursts and withdraws");
            }
            if self.now >= self.entities[&id].next_aux {
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 1000;
                let target_kind = match kind {
                    0x905 => 0x907,
                    0x906 => 0x905,
                    0x907 => 0x906,
                    _ => unreachable!(),
                };
                if let Some(target) = self
                    .camp_member(id, target_kind)?
                    .filter(|target| self.entities[target].pos.distance(pos) <= 5.)
                {
                    self.heal_enemy(id, target, 2500)?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn god_shots(
        &self,
        id: i32,
        target: Pos,
        heading: f32,
        elapsed: u64,
    ) -> Result<Vec<(u8, f32)>> {
        let e = &self.entities[&id];
        if e.kind == 0x91a && e.hp < 400 {
            return Ok(if e.expires == Some(self.now + 50) {
                (0..12)
                    .map(|i| (0, i as f32 * std::f32::consts::TAU / 12.))
                    .collect()
            } else {
                vec![]
            });
        }
        let mut shots = encounter::phased_attacks(e.kind, e.hp, heading, elapsed);
        if matches!(e.kind, 0x652 | 0x656 | 0x658) && e.pos.distance(target) > 10. {
            shots.retain(|(index, _)| *index != 1);
        }
        self.predict_enemy_shots(id, target, &mut shots, |index| match (e.kind, index) {
            (0x651 | 0x655 | 0x658, _) | (0x652 | 0x656 | 0x6d8 | 0x640 | 0x649, 1) => 1.,
            (0x656 | 0x649, 0) => 0.5,
            (0x609, _) if elapsed % 6000 < 2000 => 0.3,
            (0x609, _) if elapsed % 6000 < 4000 => 0.5,
            _ => 0.,
        })?;
        Ok(shots)
    }
    pub(super) fn predict_enemy_shots(
        &self,
        id: i32,
        target: Pos,
        shots: &mut [(u8, f32)],
        factor: impl Fn(u8) -> f32,
    ) -> Result<()> {
        if shots.is_empty() {
            return Ok(());
        }
        let e = &self.entities[&id];
        let heading = (target.y - e.pos.y).atan2(target.x - e.pos.x);
        let player = self
            .players
            .values()
            .find(|p| p.pos.x == target.x && p.pos.y == target.y);
        if e.kind == 0x651 && self.name != "Abyss of Demons" {
            if let Some(p) = player {
                let history = p.white_demon_aim_history[0];
                let previous = (history.y - e.pos.y).atan2(history.x - e.pos.x);
                for (index, angle) in shots.iter_mut() {
                    *angle += (heading - previous) / 0.1
                        * (self.content.objects[&e.kind].projectiles[index].speed / 100.)
                        * factor(*index);
                }
            } else {
                ensure!(
                    self.decoy_positions()
                        .any(|p| p.x == target.x && p.y == target.y),
                    "predictive target is neither a current player nor a live decoy"
                );
            }
            return Ok(());
        }
        let velocity = if let Some(p) = player {
            if self.now.saturating_sub(p.motion.at) <= 200 {
                p.motion.velocity
            } else {
                Pos { x: 0., y: 0. }
            }
        } else {
            ensure!(
                self.decoy_positions()
                    .any(|p| p.x == target.x && p.y == target.y),
                "predictive target is neither a current player nor a live decoy"
            );
            Pos { x: 0., y: 0. }
        };
        for (index, angle) in shots.iter_mut() {
            let factor = factor(*index);
            if factor == 0. {
                continue;
            }
            let spec = &self.content.objects[&e.kind].projectiles[index];
            ensure!(spec.speed > 0., "predictive God projectile has no speed");
            let horizon =
                (e.pos.distance(target) / (spec.speed / 10.)).min(spec.lifetime / 1000.) * factor;
            let aim = Pos {
                x: target.x + velocity.x * horizon,
                y: target.y + velocity.y * horizon,
            };
            *angle += (aim.y - e.pos.y).atan2(aim.x - e.pos.x) - heading;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn leviathan_preserves_all_three_fans_at_the_same_nominal_cadence() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
        let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
        for cell in &mut map.cells {
            cell.ground = 0x61;
            cell.object = None;
        }
        map.spawns = vec![(12.5, 12.5)];
        let mut world = World::new(content, Arc::new(map)).unwrap();
        world.encounter = true;
        let account = store.list("leviathan-audit".into()).await.unwrap();
        let character = store
            .create(
                account.id,
                world.content.get(crate::content::WIZARD).unwrap(),
            )
            .await
            .unwrap();
        world.enter(character, None).unwrap();
        let god = world
            .spawn_entity(0x6d8, Pos { x: 12.5, y: 22.5 }, vec![])
            .unwrap();
        world
            .apply_condition(god, condition::PARALYZED, 10_000)
            .unwrap();
        let mut births: [Vec<u64>; 3] = Default::default();
        for tick in 1..=180 {
            world.now = tick * 50;
            world.npc_step();
            let shots = std::mem::take(&mut world.bullets);
            if shots.is_empty() {
                continue;
            }
            let phase = ((world.now - 50) / 1500 % 3) as usize;
            assert_eq!(shots.len(), [9, 12, 2][phase]);
            assert!(
                shots
                    .iter()
                    .all(|b| b.owner == god && b.spec.speed == if phase == 2 { 85. } else { 67. })
            );
            births[phase].push(world.now);
        }
        for (phase, actual) in births.iter().enumerate() {
            let expected: Vec<_> = (0..2)
                .flat_map(|cycle| {
                    (0..5).map(move |i| 50 + cycle * 4500 + phase as u64 * 1500 + i * 300)
                })
                .collect();
            assert_eq!(
                actual, &expected,
                "Leviathan phase {phase} must emit every 300 ms"
            );
        }
    }

    #[tokio::test]
    async fn sprite_children_wander_when_caught_up_without_separation_or_population_changes() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
        let map = Map {
            width: 64,
            height: 64,
            cells: vec![
                crate::content::Cell {
                    ground: 0x60,
                    object: None,
                    config: String::new(),
                    terrain: 1,
                    region: 0
                };
                64 * 64
            ],
            spawns: vec![(32.5, 40.5)],
        };
        let mut world = World::new(content, Arc::new(map)).unwrap();
        world.name = "Realm".into();
        world.encounter = true;
        world.safe = false;
        world.rng = rand::rngs::StdRng::seed_from_u64(472);
        world.configure_population().unwrap();
        let family = population::FAMILIES
            .iter()
            .find(|f| f.leader == 0x652)
            .unwrap();
        let center = Pos { x: 32.5, y: 32.5 };
        let positions = (0..family.size())
            .map(|i| Pos {
                x: center.x + i as f32,
                y: center.y,
            })
            .collect();
        world.place_camp(family, positions).unwrap();
        let god = world.population[0].entity.unwrap();
        let children: Vec<_> = world
            .population
            .iter()
            .skip(1)
            .filter_map(|s| s.entity)
            .collect();
        assert_eq!(children.len(), 3);
        assert_eq!(world.population.len(), 6);
        assert_eq!(world.population_count(1), 4);
        let account = store.list("sprite-child-regression".into()).await.unwrap();
        let ch = store
            .create(
                account.id,
                world.content.get(crate::content::WIZARD).unwrap(),
            )
            .await
            .unwrap();
        world.enter(ch, None).unwrap();
        // A stationary parent isolates the missing idle branch from parent-driven catch-up.
        world
            .apply_condition(god, condition::PARALYZED, 5000)
            .unwrap();
        world
            .apply_condition(children[2], condition::PARALYZED, 5000)
            .unwrap();
        let stack = Pos {
            x: center.x + 0.5,
            y: center.y,
        };
        for id in &children {
            world.move_entity_colliding(*id, stack, 10., true).unwrap();
        }
        let starting: Vec<_> = children.iter().map(|id| world.entities[id].pos).collect();
        world.now += 50;
        world.npc_step();
        for (index, id) in children[..2].iter().enumerate() {
            assert!(
                world.entities[id].pos.distance(starting[index]) > 0.1,
                "caught-up Sprite Child {id} stayed stacked"
            );
            assert!(matches!(
                world.god_movement(*id, None, 0).unwrap(),
                (Motion::Wander, 4.)
            ));
        }
        assert!(
            world.entities[&children[0]]
                .pos
                .distance(world.entities[&children[1]].pos)
                > 0.01
        );
        assert_eq!(world.entities[&children[2]].pos.distance(starting[2]), 0.);
        assert_eq!(world.entities[&god].pos.distance(center), 0.);
        assert_eq!(world.entities[&god].hp, 2000);
        assert!(children.iter().all(|id| world.entities[id].hp == 50));
        let far = Pos {
            x: center.x + 5.,
            y: center.y,
        };
        world
            .move_entity_colliding(children[0], far, 10., true)
            .unwrap();
        assert!(matches!(
            world.god_movement(children[0], None, 0).unwrap(),
            (
                Motion::Protect {
                    anchor: Anchor::CampLeader,
                    distance: 2.
                },
                4.
            )
        ));
        world.now += 50;
        world.npc_step();
        assert!(world.entities[&children[0]].pos.distance(center) < far.distance(center));
        world.remove_entity(god);
        assert!(children.iter().all(|id| world.entities.contains_key(id)));
        assert_eq!(world.population_count(1), 3);
        assert!(children.iter().all(|id| matches!(
            world.god_movement(*id, None, 0).unwrap(),
            (Motion::Wander, 4.)
        )));
        world
            .move_entity_colliding(
                children[0],
                Pos {
                    x: center.x + 25.,
                    y: center.y,
                },
                30.,
                true,
            )
            .unwrap();
        assert!(matches!(
            world.god_movement(children[0], None, 0).unwrap(),
            (
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 1.
                },
                6.
            )
        ));
    }
}

#[cfg(test)]
#[path = "gods_white_demon_tests.rs"]
mod white_demon_tests;
