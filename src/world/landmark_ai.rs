// Oasis.cs / SpawnGroup.cs, realm-src@1855c017 (GPL-3.0; data/REALM-SRC-LICENSE), NR-Core and Skilly.
use super::*;
use crate::{
    condition,
    encounter::{Anchor, Motion, landmarks, nr_speed},
};

#[derive(Clone, Debug, Default)]
pub(super) struct Script {
    pub spawned: usize,
    pub choice: u8,
    pub clones_spawned: usize,
    pub clone_tossed: bool,
    pub no_experience: bool,
}
impl World {
    pub(super) fn landmark_bind(&mut self, id: i32) -> Result<Option<i32>> {
        let e = &self.entities[&id];
        if let Some(tag) = e.quest {
            return Ok(Some(tag));
        }
        let kind = e.kind;
        let root = landmarks::owner(kind);
        if landmarks::ROOTS.contains(&kind) {
            self.entities.get_mut(&id).unwrap().quest = Some(id);
            return Ok(Some(id));
        }
        if matches!(kind, 0x65a..=0x65e) {
            return Ok(None);
        }
        let mut roots = self.entities.values().filter(|e| e.kind == root);
        let owner = roots.next().map(|e| e.id);
        ensure!(
            roots.next().is_none(),
            "map-born landmark follower has ambiguous ownership"
        );
        if let Some(owner) = owner {
            let tag = self
                .landmark_bind(owner)?
                .context("landmark root lacks ownership")?;
            self.entities.get_mut(&id).unwrap().quest = Some(tag);
            return Ok(Some(tag));
        }
        Ok(None)
    }
    pub(super) fn landmark_members(&self, id: i32, kind: u16) -> Vec<i32> {
        let Some(tag) = self.entities[&id].quest else {
            return vec![];
        };
        self.actors()
            .filter(|e| e.quest == Some(tag) && e.kind == kind)
            .map(|e| e.id)
            .collect()
    }
    pub(super) fn landmark_birth(
        &mut self,
        id: i32,
        kind: u16,
        maximum: usize,
        count: usize,
        radius: f32,
    ) -> Result<Vec<i32>> {
        let old = self.landmark_members(id, kind);
        let e = &self.entities[&id];
        self.quest_birth(
            (
                e.quest
                    .context("landmark birth lacks encounter ownership")?,
                e.home,
                e.pos,
            ),
            kind,
            maximum,
            count,
            radius,
        )?;
        let born = self
            .landmark_members(id, kind)
            .into_iter()
            .filter(|id| !old.contains(id))
            .collect::<Vec<_>>();
        for child in &born {
            self.configure_landmark_child(*child);
        }
        Ok(born)
    }
    pub(super) fn landmark_enter(&mut self, id: i32, stage: u64) {
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(stage);
        e.activated = Some(self.now);
        e.next_shot = self.now;
        tracing::debug!(entity = id, stage, "landmark phase entered");
    }
    pub(super) fn landmark_shield(&mut self, id: i32, on: bool) {
        let e = self.entities.get_mut(&id).unwrap();
        if on {
            e.condition |= condition::bit(condition::INVULNERABLE);
        } else {
            e.condition &= !condition::bit(condition::INVULNERABLE);
        }
    }
    pub(super) fn landmark_fade(&mut self, id: i32, delay: u64) {
        let e = self.entities.get_mut(&id).unwrap();
        let deadline = e
            .expires
            .map_or(self.now + delay, |d| d.min(self.now + delay));
        e.expires.set(Some(deadline), id, &mut self.timed_ids);
    }
    pub(super) fn landmark_circle(&self, id: i32, radius: f32, count: usize) -> Vec<Pos> {
        let p = self.entities[&id].pos;
        (0..count)
            .map(|n| {
                let a = n as f32 * std::f32::consts::TAU / count as f32;
                Pos {
                    x: p.x + radius * a.cos(),
                    y: p.y + radius * a.sin(),
                }
            })
            .collect()
    }
    pub(super) fn landmark_texture(&mut self, id: i32, texture: i32) {
        let stats = &mut self.entities.get_mut(&id).unwrap().extra_stats;
        if let Some((_, value)) = stats.iter_mut().find(|(stat, _)| *stat == 61) {
            *value = texture;
        } else {
            stats.push((61, texture));
        }
    }
    fn crystal_mechanics(&mut self, id: i32, target: Option<Pos>) -> Result<()> {
        let e = &self.entities[&id];
        let stage = e.stage.context("crystal phase missing")?;
        let age = self.now - e.activated.context("crystal clock missing")?;
        let max = self.content.objects[&e.kind].hp;
        let next = match stage {
            0 if target.is_some_and(|p| e.pos.distance(p) < 10.) => Some(1),
            1 if e.hp < max - max / 10000 => Some(2),
            2 if e.hp < max - max / 500 => Some(3),
            3 if e.hp < max * 95 / 100 => Some(4),
            3 if age >= 60000 => Some(6),
            4 if age >= 4000 => Some(5),
            5 if age >= 50 => {
                let new = self.transform_fauna(id, 0x941)?;
                self.landmark_enter(new, 0);
                self.landmark_shield(new, true);
                self.entities.get_mut(&new).unwrap().next_aux = self.now + 200;
                return Ok(());
            }
            6 if age >= 5000 => Some(1),
            0..=6 => None,
            _ => bail!("invalid crystal phase"),
        };
        if let Some(next) = next {
            self.landmark_enter(id, next);
            if matches!(next, 3 | 6) {
                self.entities.get_mut(&id).unwrap().next_aux = self.now;
            }
            if matches!(next, 4 | 6) {
                self.flash_inland(id, 0xff000000, 0.5, 8.);
            }
        }
        let e = &self.entities[&id];
        let stage = e.stage.unwrap();
        if matches!(stage, 3 | 6)
            && self.now >= e.next_aux
            && e.conditions(self.now) & condition::bit(condition::STUNNED) == 0
        {
            self.heal_enemy(id, id, max)?;
            self.entities.get_mut(&id).unwrap().next_aux =
                self.now + if stage == 3 { 5000 } else { 1000 };
        }
        if stage == 4 {
            let e = self.entities.get_mut(&id).unwrap();
            e.size = (e.size - 2).max(80);
        }
        self.landmark_shield(id, matches!(stage, 4 | 5));
        Ok(())
    }
    fn prisoner_mechanics(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let stage = e.stage.context("prisoner phase missing")?;
        let age = self.now - e.activated.context("prisoner clock missing")?;
        let next = match stage {
            0 if age >= 3500 => Some(1),
            1 if age >= 18000 || e.hp < self.content.objects[&e.kind].hp * 3 / 10 => {
                Some(if age >= 18000 { 2 } else { 5 })
            }
            2 if age >= 3000 => Some(3),
            3 if age >= 16000 => Some(4),
            4 if age >= 5000 => Some(5),
            5 if age >= 10000 => Some(6),
            6 if age >= 14000 => Some(7),
            7 if age >= 3000 => Some(1),
            0..=7 => None,
            _ => bail!("invalid prisoner phase"),
        };
        if let Some(next) = next {
            self.landmark_enter(id, next);
            if next == 3 {
                let e = self.entities.get_mut(&id).unwrap();
                let script = e.script.as_mut().unwrap();
                script.clones_spawned = 0;
                script.clone_tossed = false;
                e.reaction_until = self.now + 200;
            }
            if matches!(next, 2 | 4 | 7) {
                self.flash_inland(id, 0xff00ff00, 0.2, 15.);
            }
        }
        let e = &self.entities[&id];
        let stage = e.stage.unwrap();
        let age = self.now - e.activated.unwrap();
        let shield = match stage {
            0 | 2 | 4 | 7 => true,
            1 => matches!(age / 200 % 8, 0 | 1 | 2 | 5 | 6),
            3 => age % 4200 < 3000,
            5 => age % 3800 < 2600,
            6 => age % 3600 < 2600,
            _ => unreachable!(),
        };
        if stage == 3 {
            let e = &self.entities[&id];
            let script = e.script.as_ref().unwrap();
            if !script.clone_tossed
                && e.conditions(self.now) & condition::bit(condition::STUNNED) == 0
            {
                let p = e.pos;
                let positions =
                    [(5f32, 0f32), (5., 240.), (7., 60.), (7., 300.)].map(|(r, a)| Pos {
                        x: p.x + r * a.to_radians().cos(),
                        y: p.y + r * a.to_radians().sin(),
                    });
                self.named_toss(id, 0x942, 8, &positions)?;
                self.entities
                    .get_mut(&id)
                    .unwrap()
                    .script
                    .as_mut()
                    .unwrap()
                    .clone_tossed = true;
            }
            let e = &self.entities[&id];
            if e.script.as_ref().unwrap().clones_spawned < 4 && self.now >= e.reaction_until {
                let born = self.landmark_birth(id, 0x942, 8, 1, 0.)?;
                for child in &born {
                    self.entities
                        .get_mut(child)
                        .unwrap()
                        .script
                        .get_or_insert_with(Script::default)
                        .no_experience = true;
                }
                let e = self.entities.get_mut(&id).unwrap();
                e.script.as_mut().unwrap().clones_spawned += born.len();
                e.reaction_until = self.now + 200;
            }
        }
        if stage == 6 {
            let e = self.entities.get_mut(&id).unwrap();
            e.size = (e.size + 13).min(260);
        }
        if stage == 7 {
            let e = self.entities.get_mut(&id).unwrap();
            e.size = (e.size - 20).max(100);
        }
        self.landmark_shield(id, shield);
        let e = &self.entities[&id];
        if e.script
            .as_ref()
            .context("prisoner script missing")?
            .spawned
            < 3
            && self.now >= e.next_aux
        {
            let born = self.landmark_birth(id, 0x943, 3, 1, 1.)?.len();
            let e = self.entities.get_mut(&id).unwrap();
            e.script.as_mut().unwrap().spawned += born;
            e.next_aux = self.now + 200;
        }
        Ok(())
    }
    pub(super) fn landmark_mechanics(
        &mut self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<()> {
        self.landmark_bind(id)?;
        self.entities
            .get_mut(&id)
            .unwrap()
            .script
            .get_or_insert_with(Script::default);
        let e = &self.entities[&id];
        let kind = e.kind;
        let initial = e.stage.is_none();
        if matches!(kind, 0xe37 | 0xe38 | 0xe39 | 0xe3b | 0xe3c | 0xe3d | 0xe4f) {
            return self.ghost_ship_mechanics(id, target, elapsed);
        }
        if matches!(kind, 0xd50..=0xd53) {
            return self.lost_lands_mechanics(id, target);
        }
        if matches!(kind, 0x65a..=0x65e) {
            if initial || self.now >= e.next_aux {
                let choice = u64::from(self.rng.gen_bool(0.5));
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(choice);
                e.next_aux = self.now + 500;
            }
            return Ok(());
        }
        if matches!(kind, 0x679..=0x67c) {
            self.entities.get_mut(&id).unwrap().stage = Some(0);
            return Ok(());
        }
        if matches!(kind, 0x935 | 0x941) {
            if initial {
                self.landmark_enter(id, 0);
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 200;
            }
            return if kind == 0x935 {
                self.crystal_mechanics(id, target)
            } else {
                self.prisoner_mechanics(id)
            };
        }
        if kind == 0x943 {
            if self.landmark_members(id, 0x941).is_empty() {
                self.landmark_fade(id, 500);
            }
            if initial {
                self.entities.get_mut(&id).unwrap().stage = Some(0);
            }
            self.landmark_shield(id, elapsed < 800);
            let e = self.entities.get_mut(&id).unwrap();
            e.orbit.get_or_insert(realm_events::Orbit {
                speed: nr_speed(1.),
                radius: 9.,
                direction: -1.,
            });
            return Ok(());
        }
        if kind == 0x942 {
            self.entities.get_mut(&id).unwrap().stage = Some(0);
            self.landmark_fade(id, 17000);
            return Ok(());
        }
        if matches!(kind, 0xd54 | 0xd55) {
            return self.sphinx_mechanics(id, target);
        }
        if !initial && self.now < e.next_aux {
            return Ok(());
        }
        match kind {
            0x678 => {
                let spawned = e.script.as_ref().unwrap().spawned;
                let count = if initial {
                    10
                } else {
                    usize::from(spawned < 20)
                };
                ensure!(
                    self.actors()
                        .filter(|e| e.parent.is_some() || e.quest.is_some())
                        .count()
                        + self.named_births.len()
                        + count
                        <= 256,
                    "Oasis group exceeds shared summon budget"
                );
                for _ in 0..count {
                    let child = self.rng.gen_range(0x679..=0x67c);
                    self.landmark_birth(id, child, 20, 1, 0.5)?;
                    self.entities
                        .get_mut(&id)
                        .unwrap()
                        .script
                        .as_mut()
                        .unwrap()
                        .spawned += 1;
                }
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 5000;
            }
            0x925 => {
                for (child, min, max) in [
                    (0x65a, 3, 5),
                    (0x65b, 2, 4),
                    (0x65c, 1, 3),
                    (0x65d, 1, 2),
                    (0x65e, 1, 1),
                ] {
                    let count = self.rng.gen_range(min..=max);
                    self.landmark_birth(id, child, max, count, 1.)?;
                }
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 10000;
            }
            _ => bail!("unsupported landmark mechanic {kind:#x}"),
        }
        self.entities.get_mut(&id).unwrap().stage = Some(0);
        Ok(())
    }
    pub(super) fn landmark_movement(
        &self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<(Motion, f32)> {
        let e = &self.entities[&id];
        if matches!(
            e.kind,
            0xe37 | 0xe38 | 0xe39 | 0xe3b | 0xe3c | 0xe3d | 0xe4f
        ) {
            return self.ghost_ship_movement(id);
        }
        if matches!(e.kind, 0xd50..=0xd53) {
            return self.lost_lands_movement(id, target);
        }
        if e.kind == 0x941 {
            let stage = e.stage.context("prisoner movement phase missing")?;
            let speed = nr_speed(match stage {
                0 => 0.,
                1 => 0.3,
                2 => 0.5,
                3 | 4 => 0.85,
                5 => 0.6,
                6 | 7 => 0.2,
                _ => bail!("invalid prisoner movement phase"),
            });
            return Ok(if stage == 0 {
                (Motion::Stationary, 0.)
            } else {
                (Motion::WanderHome { radius: 7. }, speed)
            });
        }
        if e.kind == 0x943 {
            if e.pos.distance(e.home) > 12. {
                return Ok((
                    Motion::Protect {
                        anchor: Anchor::Home,
                        distance: 12.,
                    },
                    nr_speed(if elapsed < 800 { 3.6 } else { 1. }),
                ));
            }
            return Ok(if elapsed < 800 {
                (Motion::WanderHome { radius: 12. }, nr_speed(3.6))
            } else if ((elapsed - 800) / 2000).is_multiple_of(2)
                && !self.landmark_members(id, 0x941).is_empty()
            {
                (
                    Motion::Orbit {
                        anchor: Anchor::QuestMember(0x941),
                        radius: 9.,
                    },
                    nr_speed(1.),
                )
            } else {
                (Motion::WanderHome { radius: 12. }, nr_speed(1.))
            });
        }
        if matches!(e.kind, 0xd54 | 0xd55) {
            return self.sphinx_movement(id);
        }
        if e.kind == 0x678 {
            return Ok(if e.pos.distance(e.home) > 2. {
                (
                    Motion::Protect {
                        anchor: Anchor::Home,
                        distance: 1.,
                    },
                    nr_speed(0.3),
                )
            } else {
                (Motion::Wander, nr_speed(0.4))
            });
        }
        if matches!(e.kind, 0x679..=0x67c) {
            let protection = 10. + f32::from(e.kind - 0x679);
            let approach = 9. - 2. * f32::from(e.kind - 0x679);
            if let Some(root) = self.landmark_members(id, 0x678).into_iter().next()
                && e.pos.distance(self.entities[&root].pos) > protection
            {
                return Ok((
                    Motion::Protect {
                        anchor: Anchor::QuestMember(0x678),
                        distance: protection,
                    },
                    nr_speed(0.5),
                ));
            }
            return Ok(
                if target.is_some_and(|p| e.pos.distance(p) > approach && e.pos.distance(p) < 10.) {
                    (
                        Motion::Chase {
                            sight: 10.,
                            stop: approach,
                        },
                        nr_speed(1.),
                    )
                } else {
                    (Motion::Wander, nr_speed(0.5))
                },
            );
        }
        if e.pos.distance(e.home) > 16. {
            return Ok((
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 1.,
                },
                8.,
            ));
        }
        if matches!(e.kind, 0x65a..=0x65e) {
            if elapsed % 3000 >= 2500 {
                return Ok(if target.is_some_and(|p| e.pos.distance(p) < 10.) {
                    (Motion::Retreat { distance: 10. }, 8.)
                } else {
                    (Motion::Stationary, 0.)
                });
            }
            if target.is_some_and(|p| e.pos.distance(p) < 10.) {
                return Ok(if e.stage == Some(1) {
                    (
                        Motion::Orbit {
                            anchor: Anchor::Player,
                            radius: 2.,
                        },
                        8.,
                    )
                } else if target.is_some_and(|p| e.pos.distance(p) > 2.) {
                    (
                        Motion::Chase {
                            sight: 10.,
                            stop: 2.,
                        },
                        8.,
                    )
                } else {
                    (Motion::Stationary, 0.)
                });
            }
            return Ok((Motion::Stationary, 0.));
        }
        let profile = landmarks::profile(e.kind);
        Ok((profile.motion, profile.speed))
    }
}
