use super::*;
use crate::{
    condition,
    encounter::{Anchor, Motion, nr_speed},
};
impl World {
    pub(super) fn populate_lost_lands(&mut self) -> Result<()> {
        for kind in 0xd50..=0xd53 {
            crate::encounter::validate(&self.content, kind)?;
            crate::loot::validate(&self.content, kind)?;
        }
        let pos = self.place_named_setpiece(0xd50, &[1])?;
        let id = self.spawn_entity(0xd50, pos, vec![])?;
        self.landmark_bind(id)?;
        Ok(())
    }
    pub(super) fn lost_lands_mechanics(&mut self, id: i32, target: Option<Pos>) -> Result<()> {
        let kind = self.entities[&id].kind;
        if kind != 0xd50 {
            let root = self.landmark_members(id, 0xd50).into_iter().next();
            if root.is_none() {
                self.entities.get_mut(&id).unwrap().stage = Some(0);
                self.landmark_fade(id, 500);
                return Ok(());
            }
            if kind == 0xd53 {
                let e = &self.entities[&id];
                let need = 2usize.saturating_sub(
                    e.script
                        .as_ref()
                        .context("guardian script missing")?
                        .spawned,
                );
                if need > 0 {
                    let born = self.landmark_birth(id, 0xd52, 10, need, 1.)?;
                    for child in &born {
                        self.entities.get_mut(child).unwrap().parent.set(
                            Some(id),
                            *child,
                            &mut self.timed_ids,
                        );
                    }
                    self.entities
                        .get_mut(&id)
                        .unwrap()
                        .script
                        .as_mut()
                        .unwrap()
                        .spawned += born.len();
                }
                let low = self.entities[&id].hp < self.content.objects[&kind].hp / 4;
                self.entities.get_mut(&id).unwrap().stage = Some(u64::from(low));
            } else {
                self.entities.get_mut(&id).unwrap().stage = Some(0);
            }
            if kind == 0xd51 {
                self.entities
                    .get_mut(&id)
                    .unwrap()
                    .orbit
                    .get_or_insert(realm_events::Orbit {
                        speed: nr_speed(0.3),
                        radius: 4.,
                        direction: -1.,
                    });
            }
            return Ok(());
        }
        if self.entities[&id].stage.is_none() {
            self.landmark_enter(id, 0);
        }
        let e = &self.entities[&id];
        let stage = e.stage.unwrap();
        let max = self.content.objects[&kind].hp;
        let age = self.now - e.activated.context("lost-lands phase clock missing")?;
        let next = if stage < 9 && e.hp < max / 10 {
            Some(9)
        } else {
            match stage {
                0 if e.hp < max * 99 / 100 => Some(1),
                1 if age >= 100 => Some(2),
                2 if age >= 3100 => Some(3),
                3 if target.is_some_and(|p| e.pos.distance(p) < 1.) => Some(4),
                3 if age >= 10000 => Some(5),
                4 if age >= 5000 => Some(5),
                5 if age >= 2000 => Some(6),
                6 if age >= 5000 => Some(7),
                7 if !self.landmark_members(id, 0xd51).is_empty() => Some(8),
                8 if self.landmark_members(id, 0xd51).is_empty() => Some(1),
                9 if age >= 2000 => Some(10),
                0..=10 => None,
                _ => bail!("invalid lost-lands phase"),
            }
        };
        if let Some(next) = next {
            self.landmark_enter(id, next);
            if matches!(next, 1 | 2 | 7) {
                self.entities.get_mut(&id).unwrap().next_aux = self.now;
            }
            if next == 3 {
                let e = self.entities.get_mut(&id).unwrap();
                e.script.as_mut().unwrap().choice = 0;
                e.next_aux = self.now + 2000;
                e.reaction_until = self.now + 3000;
            }
            if next == 9 {
                self.flash_inland(id, 0xffff0000, 0.1, 20.);
            }
            if next == 10 {
                let e = self.entities.get_mut(&id).unwrap();
                e.statuses.remove(&condition::STUNNED);
                e.expires.set(Some(self.now + 50), id, &mut self.timed_ids);
            }
        }
        let e = &self.entities[&id];
        let stage = e.stage.unwrap();
        let stunned = e.conditions(self.now) & condition::bit(condition::STUNNED) != 0;
        if matches!(stage, 1 | 2)
            && self.now >= e.next_aux
            && !stunned
            && let Some(target) = target.filter(|p| e.pos.distance(*p) < 5.)
        {
            self.named_toss(id, 0xd53, 5, &[target])?;
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 1000;
        }
        if stage == 7 && self.now >= self.entities[&id].next_aux && !stunned {
            let positions = self.landmark_circle(id, 4., 8);
            self.named_toss(id, 0xd51, 8, &positions)?;
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 5000;
        }
        // The two-second leaf return and three-second random selection have independent clocks.
        if stage == 3 {
            let e = &self.entities[&id];
            if self.now >= e.next_aux {
                let e = self.entities.get_mut(&id).unwrap();
                e.script.as_mut().unwrap().choice = 0;
                e.next_aux = self.now + 2000;
            } else if self.now >= e.reaction_until {
                let choice = u8::from(self.rng.gen_bool(0.5));
                let e = self.entities.get_mut(&id).unwrap();
                if e.script.as_ref().unwrap().choice != choice {
                    e.next_aux = self.now + 2000;
                }
                e.script.as_mut().unwrap().choice = choice;
                e.reaction_until = self.now + 3000;
            }
        }
        self.landmark_shield(id, matches!(stage, 8..=10));
        self.landmark_texture(
            id,
            match stage {
                5 | 6 | 9 | 10 => 3,
                8 => 1,
                _ => 0,
            },
        );
        Ok(())
    }
    pub(super) fn lost_lands_movement(
        &self,
        id: i32,
        target: Option<Pos>,
    ) -> Result<(Motion, f32)> {
        let e = &self.entities[&id];
        Ok(match e.kind {
            0xd50 => match e.stage.context("lost-lands movement phase missing")? {
                1 | 3 => (Motion::Wander, nr_speed(0.8)),
                4 => (
                    Motion::Follow {
                        sight: 20.,
                        stop: 3.,
                        idle: nr_speed(0.4),
                        duration: 0,
                        cooldown: 0,
                    },
                    nr_speed(1.),
                ),
                _ => (Motion::Stationary, 0.),
            },
            0xd51 if !self.landmark_members(id, 0xd50).is_empty() => (
                Motion::Orbit {
                    anchor: Anchor::QuestMember(0xd50),
                    radius: 4.,
                },
                nr_speed(0.3),
            ),
            0xd51 => (Motion::Stationary, 0.),
            0xd52 if target.is_some_and(|p| e.pos.distance(p) < 2.) => {
                (Motion::Retreat { distance: 2. }, nr_speed(0.5))
            }
            0xd53 if e.stage == Some(1) => {
                if target.is_some_and(|p| e.pos.distance(p) < 5.) {
                    (Motion::Retreat { distance: 5. }, nr_speed(0.6))
                } else {
                    (Motion::Wander, nr_speed(0.2))
                }
            }
            kind => {
                let p = crate::encounter::landmarks::profile(kind);
                (p.motion, p.speed)
            }
        })
    }
}
