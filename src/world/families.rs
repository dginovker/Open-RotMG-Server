use super::*;
use crate::encounter::{Anchor, Motion};

impl World {
    pub(super) fn family_movement(
        &mut self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<(Motion, f32)> {
        let target = target.into();
        let e = &self.entities[&id];
        let camp = self
            .camps
            .get(e.camp.context("family member lacks camp")?)
            .context("family camp missing")?;
        let leader = self.population[camp.start]
            .entity
            .and_then(|id| self.entities.get(&id));
        if e.pos.distance(e.home) > 12. {
            return Ok((
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 1.,
                },
                6.,
            ));
        }
        let Some(leader) = leader else {
            return Ok(if e.kind == 0x954 {
                (Motion::Retreat { distance: 10. }, 6.)
            } else if matches!(e.kind, 0x600 | 0x601 | 0x615 | 0x616 | 0x618 | 0x619) {
                crate::encounter::movement(e.kind, e.hp, elapsed)
            } else {
                (Motion::Wander, 4.)
            });
        };
        if e.id == leader.id {
            return Ok(match e.kind {
                0x609 => {
                    if elapsed % 6000 < 2000 && target.is_none_or(|p| e.pos.distance(p) >= 11.) {
                        self.entities.get_mut(&id).unwrap().activated = None;
                        return Ok((Motion::Stationary, 0.));
                    }
                    match elapsed % 6000 {
                        0..2000 => (Motion::Pace { distance: 5. }, 8.),
                        2000..4000 => (
                            Motion::Chase {
                                sight: 10.5,
                                stop: 3.,
                            },
                            6.,
                        ),
                        _ => (Motion::Retreat { distance: 5. }, 6.),
                    }
                }
                0x604 => (Motion::Wander, 2.),
                0x953 if e.hp <= 80 => (Motion::Retreat { distance: 5. }, 4.),
                0x614 if e.hp <= 182 => (Motion::Retreat { distance: 6. }, 5.),
                0x601 | 0x953 | 0x60d | 0x617 | 0x61a | 0x614 => {
                    crate::encounter::movement(e.kind, e.hp, elapsed)
                }
                _ => bail!("unsupported family leader {:#x}", e.kind),
            });
        }
        if e.pos.distance(leader.pos) > 10. {
            return Ok((
                Motion::Protect {
                    anchor: Anchor::CampLeader,
                    distance: 1.,
                },
                12.,
            ));
        }
        let protect = |distance, speed| {
            if e.pos.distance(leader.pos) > distance {
                (
                    Motion::Protect {
                        anchor: Anchor::CampLeader,
                        distance,
                    },
                    speed,
                )
            } else {
                (Motion::Wander, 4.)
            }
        };
        Ok(match e.kind {
            0x600 | 0x601 => crate::encounter::movement(e.kind, e.hp, elapsed),
            0x603 => protect(1., 4.),
            0x954 if e.pos.distance(leader.pos) > 3. => protect(3., 8.),
            0x954 => (
                Motion::Chase {
                    sight: 6.,
                    stop: 0.,
                },
                8.,
            ),
            0x60a => protect(6., 8.),
            0x60c => protect(6., 12.),
            0x616 | 0x619 if e.pos.distance(leader.pos) > 6. => protect(6., 11.),
            0x615 | 0x618 if e.pos.distance(leader.pos) > 6. => protect(6., 12.),
            0x616 | 0x619 if target.is_some_and(|p| e.pos.distance(p) < 8.) => (
                Motion::Chase {
                    sight: 8.,
                    stop: 4.,
                },
                8.,
            ),
            0x615 | 0x618 if target.is_some_and(|p| e.pos.distance(p) < 8.) => (
                Motion::Chase {
                    sight: 8.,
                    stop: 1.,
                },
                8.5,
            ),
            0x615 | 0x616 | 0x618 | 0x619 => protect(6., 11.),
            0x612 | 0x613 if leader.hp <= 182 => (
                Motion::Chase {
                    sight: 10.5,
                    stop: 1.5,
                },
                8.,
            ),
            0x612 | 0x613 if elapsed % 3000 < 2500 => (
                Motion::Chase {
                    sight: 8.,
                    stop: 2.,
                },
                8.,
            ),
            0x612 | 0x613 => (
                Motion::Orbit {
                    anchor: Anchor::CampLeader,
                    radius: 2.,
                },
                8.,
            ),
            0x605 => (
                Motion::Orbit {
                    anchor: Anchor::CampLeader,
                    radius: 6.,
                },
                5.,
            ),
            0x607 => protect(6., 6.),
            0x606 if target.is_some_and(|p| e.pos.distance(p) < 3.) => (
                Motion::Chase {
                    sight: 3.,
                    stop: 1.,
                },
                4.,
            ),
            0x606 => protect(6., 6.),
            _ => bail!("unsupported family follower {:#x}", e.kind),
        })
    }
}
