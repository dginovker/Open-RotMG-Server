use super::*;
use crate::{
    condition,
    encounter::{Anchor, Motion, nr_speed},
};

impl World {
    pub(super) fn flamer_mechanics(&mut self, id: i32, target: Option<Pos>) -> Result<()> {
        let e = &self.entities[&id];
        let old = e.stage;
        let mut stage = old.unwrap_or(0);
        if stage == 0 && target.is_none_or(|p| e.pos.distance(p) <= 2. || e.pos.distance(p) >= 10.)
        {
            stage = 1;
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 4000;
            self.flash_inland(id, 0xffffff00, 0.2, 20.);
        } else if stage == 1 && self.now >= e.next_aux {
            stage = 2;
        } else if stage == 2 && e.size <= 70 {
            stage = 0;
        }
        if self.entities[&id].hp < 200 && stage != 0 {
            stage = 3;
        }
        let e = self.entities.get_mut(&id).unwrap();
        if old != Some(stage) {
            e.activated = Some(self.now);
            e.stage = Some(stage);
            tracing::debug!(entity = id, stage, "Flamer phase entered");
        }
        match stage {
            1 => e.size = (e.size + 10).min(130),
            2 => e.size = (e.size - 10).max(70),
            3 => {
                if e.expires.is_none() {
                    e.expires.set(Some(self.now + 50), id, &mut self.timed_ids);
                }
            }
            0 => {}
            _ => bail!("invalid Flamer phase"),
        }
        Ok(())
    }
    pub(super) fn swarm_mechanics(&mut self, id: i32, target: Option<Pos>, elapsed: u64) {
        let state = elapsed / 4600 * 2 + u64::from(elapsed % 4600 >= 3000);
        let e = self.entities.get_mut(&id).unwrap();
        if e.stage != Some(state) {
            e.stage = Some(state);
            e.reaction_until = 0;
            e.next_aux = 0;
        }
        if e.orbit.is_none() {
            e.orbit = Some(realm_events::Orbit {
                speed: nr_speed(1.9),
                radius: 3.5,
                direction: -1.,
            });
        }
        if state.is_multiple_of(2)
            && self.now >= e.next_aux
            && e.conditions(self.now) & condition::bit(condition::PARALYZED) == 0
            && target.is_some_and(|p| {
                let d = e.pos.distance(p);
                d > 3.5 && d < 11.
            })
        {
            e.reaction_until = self.now + 1000;
            e.next_aux = self.now + 6000;
        }
    }
    pub(super) fn swarm_movement(
        &self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> (Motion, f32) {
        let e = &self.entities[&id];
        if elapsed % 4600 >= 3000 {
            return if target.is_some_and(|p| e.pos.distance(p) < 5.) {
                (Motion::Retreat { distance: 5. }, nr_speed(2.))
            } else {
                (Motion::Wander, nr_speed(0.4))
            };
        }
        if self.now < e.reaction_until
            && target.is_some_and(|p| e.pos.distance(p) > 3.5 && e.pos.distance(p) < 11.)
        {
            return (
                Motion::Chase {
                    sight: 11.,
                    stop: 3.5,
                },
                nr_speed(4.),
            );
        }
        if target.is_some_and(|p| e.pos.distance(p) < 12.) {
            return (
                Motion::Orbit {
                    anchor: Anchor::Player,
                    radius: 3.5,
                },
                nr_speed(1.9),
            );
        }
        (Motion::Wander, nr_speed(0.4))
    }
}
