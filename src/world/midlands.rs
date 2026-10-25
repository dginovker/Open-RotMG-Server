use super::*;
use crate::encounter::{Anchor, Motion};
pub(super) const AIMED: &[u16] = &[0x640, 0x649, 0x650, 0x64f];
pub(super) fn member(kind: u16) -> bool {
    matches!(kind,0x61c..=0x61f|0x629..=0x62c|0x63a..=0x63c|0x63e..=0x640|0x642..=0x645|0x647..=0x64d|0x64f..=0x650|0x900..=0x902)
}
impl World {
    pub(super) fn midland_movement(
        &self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<(Motion, f32)> {
        let target = target.into();
        let e = &self.entities[&id];
        if e.pos.distance(e.home) > 16. {
            return Ok((
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 1.,
                },
                10.,
            ));
        }
        let chase = |speed, sight, stop| (Motion::Chase { sight, stop }, speed);
        if e.kind == 0x6c6 {
            return Ok(chase(4., 10., 2.));
        }
        let c = &self.camps[e.camp.context("inland family member lacks its camp")?];
        let owner = c.family.owner(e.kind);
        let leader = if matches!(e.kind, 0x63e | 0x64f) {
            Some(id)
        } else {
            self.camp_member(id, owner)?
        };
        let orphan = leader.is_none();
        if orphan {
            return Ok((
                Motion::Wander,
                if matches!(e.kind, 0x901 | 0x902) {
                    1.
                } else {
                    4.
                },
            ));
        }
        if leader != Some(id) && e.pos.distance(self.entities[&leader.unwrap()].pos) > 12. {
            return Ok((
                Motion::Protect {
                    anchor: if owner == c.family.leader {
                        Anchor::CampLeader
                    } else {
                        Anchor::CampMember(owner)
                    },
                    distance: 6.,
                },
                12.,
            ));
        }
        Ok(match e.kind {
            0x61f if elapsed % 5000 >= 2500 => (Motion::Wander, 4.),
            0x61f => chase(10., 15., 1.),
            0x61c | 0x61e => {
                if target.is_some_and(|p| e.pos.distance(p) < 10.) {
                    chase(10., 10., 1.)
                } else {
                    (
                        Motion::Protect {
                            anchor: Anchor::CampLeader,
                            distance: 3.,
                        },
                        10.,
                    )
                }
            }
            0x61d => {
                if target.is_some_and(|p| e.pos.distance(p) < 10.) {
                    chase(10., 10., 4.)
                } else {
                    (
                        Motion::Protect {
                            anchor: Anchor::CampLeader,
                            distance: 10.,
                        },
                        12.,
                    )
                }
            }
            0x629 | 0x62c => chase(12., 10., 1.),
            0x62a => {
                if self.camp_member(id, 0x62b)?.is_some() {
                    (
                        Motion::Orbit {
                            anchor: Anchor::CampMember(0x62b),
                            radius: 2.,
                        },
                        12.,
                    )
                } else {
                    (Motion::Wander, 4.)
                }
            }
            0x62b => chase(12., 10., 7.),
            0x63a | 0x63b | 0x63e => chase(10., 10., 1.),
            0x63c => chase(10., 10., 7.),
            0x640 => chase(10., 10., 1.),
            0x63f => (Motion::Wander, 4.),
            0x642 | 0x644 => (
                Motion::Maintain {
                    sight: 10.,
                    distance: 8.,
                },
                4.,
            ),
            0x643 if elapsed % 3000 < 1500 => chase(12., 15., 2.),
            0x643 => (
                Motion::Protect {
                    anchor: Anchor::CampLeader,
                    distance: 2.,
                },
                12.,
            ),
            0x645 => (
                Motion::Maintain {
                    sight: 10.,
                    distance: 8.,
                },
                4.,
            ),
            0x647 | 0x648 => chase(12., 10., 7.),
            0x649 => chase(10., 10., 7.),
            0x650 => chase(10., 10., 1.),
            0x64a | 0x64c | 0x64d => chase(12., 10., 7.),
            0x64b => chase(12., 7., 1.),
            0x64f => chase(15., 10., 7.),
            0x900 if e.expires.is_some() => (Motion::Stationary, 0.),
            0x900 | 0x902 => {
                if self.camp_member(id, 0x901)?.is_some() {
                    (
                        Motion::StrictOrbit {
                            anchor: Anchor::CampMember(0x901),
                            radius: 2.,
                        },
                        6.,
                    )
                } else {
                    (Motion::Wander, 1.)
                }
            }
            0x901 => (Motion::Wander, 0.5),
            _ => bail!("unsupported inland family movement {:#x}", e.kind),
        })
    }
    fn shield_members(&self, id: i32) -> Result<Vec<i32>> {
        let camp = self.entities[&id]
            .camp
            .context("Shield Orc has no command camp")?;
        Ok(self
            .actors()
            .filter(|e| e.camp == Some(camp) && matches!(e.kind, 0x900..=0x902))
            .map(|e| e.id)
            .collect())
    }
    pub(super) fn flash_inland(&self, id: i32, color: u32, period: f32, cycles: f32) {
        let mut out = Writer::new();
        out.u8(15)
            .i32(id)
            .f32(period)
            .f32(cycles)
            .f32(0.)
            .f32(0.)
            .u32(color);
        for p in self.players.values().filter(|p| p.known.contains(&id)) {
            send(p, wire::SHOW_EFFECT, out.0.clone());
        }
    }
    pub(super) fn shield_burst(&self, id: i32) -> bool {
        let e = &self.entities[&id];
        e.kind == 0x900
            && e.stage != Some(3)
            && e.camp.is_some()
            && self
                .camp_member(id, 0x900)
                .expect("owned Shield Orc command camp")
                .is_some_and(|key| self.entities[&key].expires == Some(self.now + 50))
    }
    pub(super) fn shield_shots(&self, id: i32, heading: f32) -> Result<Vec<(u8, f32)>> {
        if self.shield_burst(id) {
            return Ok((0..10)
                .map(|i| (0, i as f32 * std::f32::consts::TAU / 10.))
                .collect());
        }
        let Some(key) = self.camp_member(id, 0x900)? else {
            return Ok(vec![]);
        };
        Ok(if self.entities[&key].expires.is_some() {
            vec![]
        } else {
            vec![(0, heading)]
        })
    }
    pub(super) fn midland_mechanics(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let (kind, hp, max_hp) = (e.kind, e.hp, self.content.objects[&e.kind].hp);
        if kind == 0x62b {
            if self.now >= e.next_aux {
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 500;
                if let Some(king) = self.camp_member(id, 0x62c)?
                    && self.entities[&king].pos.distance(self.entities[&id].pos) <= 10.
                {
                    self.heal_enemy(id, king, 100)?;
                }
            }
            return Ok(());
        }
        ensure!(
            matches!(kind, 0x900..=0x902),
            "unsupported inland mechanic {kind:#x}"
        );
        if kind == 0x900 && hp < 500 && e.expires.is_none() {
            let e = self.entities.get_mut(&id).unwrap();
            e.expires
                .set(Some(self.now + 1000), id, &mut self.timed_ids);
            for member in self.shield_members(id)? {
                self.flash_inland(member, 0xffff0000, 0.5, 2.);
                self.entities.get_mut(&member).unwrap().stage = Some(2);
                self.npc_global_ids.insert(member);
            }
            tracing::debug!(
                entity = id,
                deadline = self.now + 1000,
                "Shield Orc command camp telegraphed final burst"
            );
        }
        let Some(key) = self.camp_member(id, 0x900)? else {
            if self.entities[&id].stage != Some(4) {
                self.entities.get_mut(&id).unwrap().stage = Some(4);
                tracing::debug!(entity = id, "Shield Orc survivor lost its commander");
            }
            return Ok(());
        };
        if self.entities[&key].expires.is_some() {
            return Ok(());
        }
        if self.entities[&id].stage.is_none() {
            self.flash_inland(id, 0xff000000, 10., 1.);
        }
        let stage = u64::from(hp < max_hp / 2);
        self.entities.get_mut(&id).unwrap().stage = Some(stage);
        if self.now >= self.entities[&id].next_aux {
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 500;
            if hp < max_hp / 2 {
                let pos = self.entities[&id].pos;
                for target in self.shield_members(id)? {
                    if self.entities[&target].pos.distance(pos) <= 7. {
                        self.heal_enemy(id, target, 250)?;
                    }
                }
            }
        }
        Ok(())
    }
}
