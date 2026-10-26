use super::*;
use crate::encounter::{Motion, nr_speed};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Child {
    Protect,
    Follow,
}
#[derive(Clone, Copy, Debug, Default)]
enum Protection {
    #[default]
    Unknown,
    Protecting,
    Protected,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Escort {
    priority: Option<Child>,
    protection: Protection,
    direction: Option<Pos>,
    remaining: f32,
}
#[derive(PartialEq)]
enum Cycle {
    NotStarted,
    InProgress,
    Completed,
}

impl World {
    pub(super) fn lair_escort_step(
        &mut self,
        id: i32,
        target: Option<Pos>,
        delta: u64,
    ) -> Result<()> {
        let boss = self.escort_anchor(id, 0xd90)?;
        let start = self.entities[&id].pos;
        let flags = self.entities[&id].conditions(self.now);
        let paralyzed = flags & crate::condition::bit(crate::condition::PARALYZED) != 0;
        let mut state = self
            .entities
            .get_mut(&id)
            .unwrap()
            .lair_escort
            .take()
            .unwrap_or_default();
        let mut primary = (Motion::Stationary, None, 0.);
        let selected = state.priority;
        for child in match selected {
            Some(Child::Protect) => &[Child::Protect][..],
            Some(Child::Follow) => &[Child::Follow][..],
            None => &[Child::Protect, Child::Follow][..],
        } {
            let status = if paralyzed {
                Cycle::NotStarted
            } else {
                match child {
                    Child::Protect => {
                        if let Some((boss, anchor)) =
                            boss.filter(|(boss, _)| start.distance(self.entities[boss].pos) < 10.)
                        {
                            let pos = self.entities[&boss].pos;
                            let distance = start.distance(pos);
                            if matches!(state.protection, Protection::Unknown)
                                || matches!(state.protection, Protection::Protected)
                                    && distance > 6.
                            {
                                state.protection = Protection::Protecting;
                            }
                            if matches!(state.protection, Protection::Protecting) && distance > 1. {
                                primary = (
                                    Motion::Protect {
                                        anchor,
                                        distance: 1.,
                                    },
                                    Some(pos),
                                    nr_speed(0.625),
                                );
                                Cycle::InProgress
                            } else {
                                state.protection = Protection::Protected;
                                Cycle::Completed
                            }
                        } else {
                            state.protection = Protection::Unknown;
                            Cycle::NotStarted
                        }
                    }
                    Child::Follow => {
                        let motion = Motion::Follow {
                            sight: 10.,
                            stop: 7.,
                            idle: 0.,
                            duration: 0,
                            cooldown: 0,
                        };
                        let goal = self.follow_goal(id, target, motion, delta);
                        let status = match self.entities[&id].following {
                            Following::Acquired => Cycle::InProgress,
                            Following::Resting => Cycle::Completed,
                            Following::Unknown => Cycle::NotStarted,
                        };
                        if status != Cycle::NotStarted {
                            primary = (motion, goal, nr_speed(0.75));
                        }
                        status
                    }
                }
            };
            if selected.is_some() {
                if status != Cycle::InProgress {
                    state.priority = None;
                }
            } else if status == Cycle::InProgress {
                state.priority = Some(*child);
                break;
            } else {
                // Prioritize defaults to its first child even when neither child progresses.
                state.priority = Some(Child::Protect);
            }
        }
        let slow = if flags & crate::condition::bit(crate::condition::SLOWED) != 0 {
            0.5
        } else {
            1.
        };
        let (motion, goal, speed) = primary;
        if selected != state.priority {
            tracing::debug!(entity=id, ?selected, next=?state.priority, protection=?state.protection, ?motion, "Lair escort priority changed");
        }
        self.entities.get_mut(&id).unwrap().motion = Some(motion);
        self.entities.get_mut(&id).unwrap().goal = goal;
        if let Some(goal) = goal
            && let Err(error) = self.move_entity_colliding(
                id,
                goal,
                speed / 1.5 * delta as f32 / 1000. * slow,
                true,
            )
        {
            tracing::debug!(entity=id, %error, "Lair primary movement blocked");
        }
        // The registration's Wander is a sibling, not a fallback when the primary rests.
        if !paralyzed {
            if state.remaining <= 0. {
                let component = std::f32::consts::FRAC_1_SQRT_2;
                state.direction = Some(Pos {
                    x: if self.rng.gen_bool(0.5) {
                        component
                    } else {
                        -component
                    },
                    y: if self.rng.gen_bool(0.5) {
                        component
                    } else {
                        -component
                    },
                });
                state.remaining = 0.6;
            }
            let direction = state.direction.expect("active Lair Wander has a direction");
            let pos = self.entities[&id].pos;
            let step = nr_speed(0.25) / 1.5 * delta as f32 / 1000. * slow;
            let goal = Pos {
                x: pos.x + direction.x * step,
                y: pos.y + direction.y * step,
            };
            if let Err(error) = self.move_entity_colliding(id, goal, step, true) {
                tracing::debug!(entity=id, %error, "Lair sibling Wander blocked");
            }
            state.remaining -= step;
        }
        let e = self.entities.get_mut(&id).unwrap();
        e.last_step = Pos {
            x: e.pos.x - start.x,
            y: e.pos.y - start.y,
        };
        e.lair_escort = Some(state);
        Ok(())
    }
}
