use super::*;

impl World {
    pub(in crate::world) fn tomb_mechanics(&mut self, id: i32, target: Option<Pos>) -> Result<()> {
        let kind = self.entities[&id].kind;
        let fresh = !self.entities[&id]
            .tomb
            .as_ref()
            .context("Tomb actor state missing")?
            .initialized;
        if fresh {
            let e = self.entities.get_mut(&id).unwrap();
            e.stage.get_or_insert(0);
            let b = e.tomb.as_mut().unwrap();
            b.initialized = true;
            b.next_heal = self.now;
            b.next_spawn = self.now;
            if d::BOSSES.contains(&kind) {
                self.tomb_enter(id, 0, false)?;
            } else if (0xd1c..=0xd24).contains(&kind) {
                self.entities.get_mut(&id).unwrap().orbit = Some(realm_events::Orbit {
                    speed: nr_speed(1.),
                    radius: 2. + self.rng.gen_range(-0.5..=0.5),
                    direction: -1.,
                });
            }
        }
        if d::BOSSES.contains(&kind) {
            let paralyzed =
                self.entities[&id].conditions(self.now) & condition::bit(condition::PARALYZED) != 0;
            let e = self.entities.get_mut(&id).unwrap();
            let b = e.tomb.as_mut().unwrap();
            if paralyzed && !b.paralyzed {
                e.orbit
                    .as_mut()
                    .context("Tomb boss orbit missing")?
                    .direction *= -1.;
            }
            e.tomb.as_mut().unwrap().paralyzed = paralyzed;
            self.tomb_boss_step(id)?;
            let phase = self.entities[&id]
                .stage
                .context("Tomb boss phase missing")?;
            if kind == d::NUT
                && self.entities[&id].hp >= 5000
                && self.now >= self.entities[&id].tomb.as_ref().unwrap().next_heal
            {
                let peers = self.entities[&id]
                    .tomb
                    .as_ref()
                    .unwrap()
                    .peers
                    .context("Nut ally identities missing")?;
                for (peer, amount) in [(peers[1], 100), (peers[2], 100)] {
                    if self
                        .entities
                        .get(&peer)
                        .is_some_and(|e| e.pos.distance(self.entities[&id].pos) < 6.)
                    {
                        self.tomb_heal(peer, amount)?;
                    }
                }
                self.entities
                    .get_mut(&id)
                    .unwrap()
                    .tomb
                    .as_mut()
                    .unwrap()
                    .next_heal = self.now + 1000;
            }
            if matches!(phase, 5 | 6 | weapons::RAGE)
                && self.entities[&id].tomb.as_ref().unwrap().artifacts_unlocked
            {
                let base = match kind {
                    d::NUT => 0xd1c,
                    d::GEB => 0xd1f,
                    d::BES => 0xd22,
                    _ => unreachable!(),
                };
                if self.now >= self.entities[&id].tomb.as_ref().unwrap().next_spawn {
                    let peers = self.entities[&id]
                        .tomb
                        .as_ref()
                        .unwrap()
                        .peers
                        .context("artifact boss identities missing")?;
                    for (n, anchor) in [peers[2], peers[1], peers[0]].into_iter().enumerate() {
                        let child = base + n as u16;
                        if self.tomb_owned(id, child).is_empty() {
                            self.tomb_spawn(
                                Some(id),
                                self.entities[&id].pos,
                                child,
                                1,
                                0.,
                                Some(anchor),
                            )?;
                        }
                    }
                    self.entities
                        .get_mut(&id)
                        .unwrap()
                        .tomb
                        .as_mut()
                        .unwrap()
                        .next_spawn = self.now + 4000;
                }
            }
            if kind == d::GEB && (3..=7).contains(&phase) && self.now >= self.entities[&id].next_aux
            {
                let live = self.tomb_owned(id, 0xd1b).len();
                if live < 3 {
                    self.tomb_spawn(Some(id), self.entities[&id].pos, 0xd1b, 3 - live, 0., None)?;
                }
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 8000;
            }
            return Ok(());
        }
        let stage = self.entities[&id]
            .stage
            .context("Tomb actor phase missing")?;
        let age = self.now
            - self.entities[&id]
                .activated
                .context("Tomb actor clock missing")?;
        match kind {
            d::SARCOPHAGUS if stage == 0 && self.entities[&id].hp <= 12000 => {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.activated = Some(self.now);
            }
            d::SWITCH if stage == 0 => {
                let occupied = self.players.values().any(|p| {
                    p.active() && !p.paused && p.pos.distance(self.entities[&id].pos) < 1.
                });
                let e = self.entities.get_mut(&id).unwrap();
                let b = e.tomb.as_mut().unwrap();
                if !occupied {
                    b.press_since = None;
                } else {
                    b.press_since.get_or_insert(self.now);
                }
                if b.press_since.is_some_and(|since| self.now - since >= 3000) {
                    b.press_since = None;
                    e.stage = Some(1);
                    e.extra_stats.retain(|(stat, _)| *stat != 61);
                    e.extra_stats.push((61, 1));
                    let traps: Vec<_> = self
                        .actors()
                        .filter(|e| {
                            e.kind == d::TRAP
                                && e.tomb.as_ref().is_some_and(|b| b.anchor == Some(id))
                        })
                        .map(|e| (e.id, e.pos))
                        .collect();
                    for (trap, pos) in traps {
                        self.remove_entity(trap);
                        self.insert_entity(d::DISABLED_TRAP, pos, vec![])?;
                    }
                    tracing::info!(switch = id, "Tomb bomb room permanently deactivated");
                }
            }
            d::TRAP => {
                let anchor = self.entities[&id]
                    .tomb
                    .as_ref()
                    .unwrap()
                    .anchor
                    .context("Tomb trap switch identity missing")?;
                ensure!(
                    self.entities.contains_key(&anchor),
                    "Tomb trap switch disappeared"
                );
                if self.entities[&anchor].stage == Some(1) {
                    let pos = self.entities[&id].pos;
                    self.remove_entity(id);
                    self.insert_entity(d::DISABLED_TRAP, pos, vec![])?;
                } else if self.now >= self.entities[&id].tomb.as_ref().unwrap().next_spawn {
                    let angle = self.rng.gen_range(0. ..TAU);
                    let distance = self.rng.gen_range(0. ..6.);
                    let pos = self.entities[&id].pos;
                    self.throw_grenade(
                        id,
                        Pos {
                            x: pos.x + distance * angle.cos(),
                            y: pos.y + distance * angle.sin(),
                        },
                        3.,
                        120,
                    )?;
                    self.entities
                        .get_mut(&id)
                        .unwrap()
                        .tomb
                        .as_mut()
                        .unwrap()
                        .next_spawn = self.now + 1500;
                }
            }
            d::TREASURE => {
                if stage == 0 && self.entities[&id].hp < 49500 {
                    let e = self.entities.get_mut(&id).unwrap();
                    e.stage = Some(1);
                    e.activated = Some(self.now);
                    e.next_aux = self.now;
                }
                let stage = self.entities[&id].stage.unwrap();
                if (1..=8).contains(&stage) && self.now >= self.entities[&id].next_aux {
                    let angle = stage as f32 * 45f32.to_radians();
                    let origin = self.entities[&id].pos;
                    let pos = Pos {
                        x: origin.x + 5. * angle.cos(),
                        y: origin.y + 5. * angle.sin(),
                    };
                    self.tomb_spawn(Some(id), pos, 0xd2d + (stage as u16 - 1) % 4, 1, 0., None)?;
                    let e = self.entities.get_mut(&id).unwrap();
                    e.stage = Some(stage + 1);
                    e.next_aux = self.now + 1000;
                }
            }
            0xd2a | 0xd2b => {
                let anchor = self.entities[&id]
                    .tomb
                    .as_ref()
                    .unwrap()
                    .anchor
                    .context("worshipper sarcophagus identity missing")?;
                let alive = self.entities.contains_key(&anchor);
                if stage == 4 && age >= 2000 {
                    let e = self.entities.get_mut(&id).unwrap();
                    e.stage = Some(5);
                    e.activated = Some(self.now);
                } else if stage == 5 && age >= 50 {
                    self.finish_timed_defeat(id)?;
                    return Ok(());
                } else if stage < 4 {
                    if !alive && stage != 3 {
                        let e = self.entities.get_mut(&id).unwrap();
                        e.stage = Some(3);
                        e.activated = Some(self.now);
                        e.condition &= !condition::bit(condition::ARMORED);
                    } else if stage == 0
                        && self.entities[&id].hp * 10 < self.content.objects[&kind].hp * 3
                    {
                        let e = self.entities.get_mut(&id).unwrap();
                        e.stage = Some(1);
                        e.activated = Some(self.now);
                        e.condition |= condition::bit(condition::ARMORED);
                        self.flash_inland(id, 0xff333333, 0.5, 10.);
                    } else if stage == 1
                        && self.entities[&id].pos.distance(self.entities[&anchor].pos) < 1.
                    {
                        let e = self.entities.get_mut(&id).unwrap();
                        e.stage = Some(2);
                        e.activated = Some(self.now);
                        e.condition &= !condition::bit(condition::ARMORED);
                        e.tomb.as_mut().unwrap().next_heal = self.now;
                    } else if stage == 2 && age >= 5000 {
                        let e = self.entities.get_mut(&id).unwrap();
                        e.stage = Some(3);
                        e.activated = Some(self.now);
                        e.size = 120;
                        self.flash_inland(id, 0xffffff00, 0.5, 4.);
                    }
                    if self.now >= self.entities[&id].tomb.as_ref().unwrap().next_heal {
                        if alive
                            && matches!(self.entities[&id].stage, Some(0 | 3))
                            && self.entities[&id].pos.distance(self.entities[&anchor].pos) < 10.
                        {
                            self.tomb_heal(anchor, self.content.objects[&d::SARCOPHAGUS].hp)?;
                        }
                        if self.entities[&id].stage == Some(2) {
                            let healers: Vec<_> = self
                                .actors()
                                .filter(|e| {
                                    matches!(e.kind, 0xd2a | 0xd2b)
                                        && !matches!(e.stage, Some(4 | 5))
                                        && e.tomb.as_ref().is_some_and(|b| b.anchor == Some(anchor))
                                        && e.pos.distance(self.entities[&id].pos) < 1.
                                })
                                .map(|e| (e.id, e.kind))
                                .collect();
                            for (healer, kind) in healers {
                                self.tomb_heal(healer, self.content.objects[&kind].hp)?;
                            }
                        }
                        self.entities
                            .get_mut(&id)
                            .unwrap()
                            .tomb
                            .as_mut()
                            .unwrap()
                            .next_heal = self.now
                            + if self.entities[&id].stage == Some(2) {
                                800
                            } else {
                                1000
                            };
                    }
                }
            }
            0xd1a
                if stage == 0
                    && target.is_some_and(|p| p.distance(self.entities[&id].pos) < 2.) =>
            {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.activated = Some(self.now);
                self.flash_inland(id, 0xffff0000, 0.1, 2.);
            }
            0xd1a if stage == 1 && age >= 50 => {
                self.finish_timed_defeat(id)?;
                return Ok(());
            }
            _ => {}
        }
        if matches!(kind, 0xd11 | 0xd13 | 0xd15 | 0xd19 | 0xd1a)
            && self.now >= self.entities[&id].tomb.as_ref().unwrap().next_spawn
        {
            let desired: Vec<_> = match kind {
                0xd11 | 0xd13 | 0xd15 => vec![(kind - 1, 8)],
                0xd19 => vec![(0xd16, 2), (0xd17, 1), (0xd18, 1)],
                _ => vec![(0xd1b, 5)],
            };
            for (child, count) in desired {
                let alive = self.tomb_owned(id, child).len();
                if alive < count {
                    self.tomb_spawn(
                        Some(id),
                        self.entities[&id].pos,
                        child,
                        count - alive,
                        0.,
                        Some(id),
                    )?;
                }
            }
            self.entities
                .get_mut(&id)
                .unwrap()
                .tomb
                .as_mut()
                .unwrap()
                .next_spawn = self.now + if kind == 0xd1a { 5000 } else { 4000 };
        }
        if matches!(kind, 0xd19 | 0xd3b)
            && self.now >= self.entities[&id].tomb.as_ref().unwrap().next_heal
        {
            let targets: Vec<_> = self
                .actors()
                .filter(|e| {
                    (if kind == 0xd19 {
                        (0xd16..=0xd18).contains(&e.kind)
                    } else {
                        matches!(e.kind, 0xd3a | 0xd3c)
                    }) && e.pos.distance(self.entities[&id].pos) < 10.
                })
                .map(|e| e.id)
                .collect();
            for target in targets {
                self.tomb_heal(target, 100)?;
            }
            self.entities
                .get_mut(&id)
                .unwrap()
                .tomb
                .as_mut()
                .unwrap()
                .next_heal = self.now + 1000;
        }
        Ok(())
    }

    pub(in crate::world) fn tomb_movement(
        &mut self,
        id: i32,
        target: Option<Pos>,
    ) -> Result<(Motion, f32)> {
        let e = &self.entities[&id];
        let kind = e.kind;
        let phase = e.stage.context("Tomb movement phase missing")?;
        if d::BOSSES.contains(&kind) {
            if let Some(caller) = self.tomb_guard(id) {
                return Ok((
                    Motion::Protect {
                        anchor: Anchor::BoundEntity(caller),
                        distance: 0.35,
                    },
                    nr_speed(0.8),
                ));
            }
            self.entities
                .get_mut(&id)
                .unwrap()
                .tomb
                .as_mut()
                .unwrap()
                .guard = None;
            let e = &self.entities[&id];
            if phase == weapons::RAGE {
                let acquired = target.is_some_and(|p| e.pos.distance(p) < 10.);
                return Ok(if !acquired {
                    (Motion::Stationary, 0.)
                } else if kind == d::GEB {
                    (Motion::Retreat { distance: 6. }, nr_speed(0.5))
                } else {
                    (
                        Motion::Chase {
                            sight: 10.,
                            stop: if kind == d::BES { 1. } else { 0. },
                        },
                        nr_speed(0.6),
                    )
                });
            }
            let anchor = e
                .tomb
                .as_ref()
                .unwrap()
                .anchor
                .context("Tomb boss arena identity missing")?;
            ensure!(
                self.entities.contains_key(&anchor),
                "Tomb boss arena anchor disappeared"
            );
            let orbit = e.orbit.context("Tomb boss orbit missing")?;
            return Ok((
                Motion::Orbit {
                    anchor: Anchor::BoundEntity(anchor),
                    radius: orbit.radius,
                },
                orbit.speed,
            ));
        }
        if (0xd1c..=0xd24).contains(&kind) {
            let anchor = e
                .tomb
                .as_ref()
                .unwrap()
                .anchor
                .context("Tomb artifact bound boss identity missing")?;
            if self.entities.contains_key(&anchor) {
                let orbit = e.orbit.context("Tomb artifact orbit missing")?;
                return Ok((
                    Motion::Orbit {
                        anchor: Anchor::BoundEntity(anchor),
                        radius: orbit.radius,
                    },
                    orbit.speed,
                ));
            }
            return Ok(if target.is_some_and(|p| e.pos.distance(p) < 10.) {
                (
                    Motion::Chase {
                        sight: 10.,
                        stop: 1.,
                    },
                    nr_speed(0.85),
                )
            } else {
                (Motion::Stationary, 0.)
            });
        }
        if matches!(kind, 0xd2a | 0xd2b) {
            if matches!(phase, 4 | 5) {
                return Ok((
                    Motion::Chase {
                        sight: 20.,
                        stop: 0.,
                    },
                    nr_speed(1.),
                ));
            }
            let anchor = e
                .tomb
                .as_ref()
                .unwrap()
                .anchor
                .context("worshipper sarcophagus identity missing")?;
            if self.entities.contains_key(&anchor) {
                return Ok(if phase == 1 {
                    (
                        Motion::Protect {
                            anchor: Anchor::BoundEntity(anchor),
                            distance: 0.5,
                        },
                        nr_speed(0.6),
                    )
                } else if phase == 2 {
                    (Motion::Stationary, 0.)
                } else {
                    (
                        Motion::Orbit {
                            anchor: Anchor::BoundEntity(anchor),
                            radius: if kind == 0xd2a { 3. } else { 5. },
                        },
                        nr_speed(if phase == 3 { 1. } else { 0.6 }),
                    )
                });
            }
            return Ok(if target.is_some() {
                (
                    Motion::Chase {
                        sight: 14.,
                        stop: 1.,
                    },
                    nr_speed(0.6),
                )
            } else {
                (Motion::Wander, nr_speed(0.3))
            });
        }
        if matches!(kind, 0xd10 | 0xd12 | 0xd14)
            && let Some(anchor) = e
                .tomb
                .as_ref()
                .unwrap()
                .anchor
                .filter(|id| self.entities.contains_key(id))
        {
            return Ok((
                Motion::Orbit {
                    anchor: Anchor::BoundEntity(anchor),
                    radius: 2.,
                },
                nr_speed(0.6),
            ));
        }
        let (rate, sight, stop) = match kind {
            0xd10 | 0xd12 | 0xd14 => (0.6, 10., 1.),
            0xd11 | 0xd13 | 0xd15 => (0.3, 5., 3.),
            0xd16..=0xd19 => (0.7, 7., 1.),
            0xd1a => (2., 7., 4.),
            0xd1b => (2., 7., 1.),
            0xd3a => (0.8, 10., 1.),
            0xd3b => (0.8, 14., 10.),
            0xd3c => (0.8, 14., 8.4),
            _ => return Ok((Motion::Stationary, 0.)),
        };
        Ok(if target.is_some_and(|p| e.pos.distance(p) < sight) {
            (Motion::Chase { sight, stop }, nr_speed(rate))
        } else {
            (Motion::Wander, nr_speed(0.3))
        })
    }
}
