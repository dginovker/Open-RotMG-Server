use super::*;
use crate::{condition, endgame as finale};
use finale::{BossBrain, GUARDIANS, GuardianBrain, ORYX, ORYX2, OryxPhase};

#[derive(Default)]
pub(super) struct Actors {
    pub(super) bosses: BTreeMap<i32, BossBrain>,
    guardians: BTreeMap<i32, GuardianBrain>,
}

impl World {
    fn hero_reinforcements(&mut self, id: i32, initial: bool) -> Result<()> {
        let waves = finale::summons(self.entities[&id].kind);
        ensure!(!waves.is_empty(), "hero reinforcement roster missing");
        let selected = if initial {
            None
        } else {
            Some(self.rng.gen_range(0..=waves.len()))
        };
        for (index, wave) in waves.iter().enumerate() {
            if selected.is_some_and(|choice| choice != index) {
                continue;
            }
            let current = self
                .entities
                .values()
                .filter(|e| e.parent == Some(id) && e.kind == wave.kind)
                .count();
            let count = (current + self.rng.gen_range(1..=3)).min(wave.count);
            self.finale_wave(id, finale::Wave { count, ..*wave })?;
        }
        Ok(())
    }
    fn armor_mechanics(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let old = e.stage;
        let mut stage = old.unwrap_or(1);
        if stage == 1 && e.hp < self.content.objects[&e.kind].hp / 5 {
            stage = 2;
            let e = self.entities.get_mut(&id).unwrap();
            e.reaction_until = self.now + 1500;
            e.next_aux = self.now;
        } else if stage == 2 && self.now >= e.reaction_until {
            stage = 1;
        }
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(stage);
        e.condition &= !condition::bit(condition::INVULNERABLE);
        if stage == 2 {
            e.condition |= condition::bit(condition::INVULNERABLE);
        }
        if old != Some(stage) {
            e.extra_stats.retain(|(stat, _)| *stat != 61);
            e.extra_stats.push((61, i32::from(stage == 1)));
            tracing::debug!(entity = id, stage, "Suit of Armor phase entered");
        }
        if stage == 2 && self.now >= e.next_aux {
            e.next_aux = self.now + 50;
            self.heal_enemy(id, id, 200)?;
        }
        Ok(())
    }
    fn finale_wave(&mut self, parent: i32, wave: finale::Wave) -> Result<()> {
        ensure!(
            wave.lifetime > 0,
            "endgame wave requires a bounded lifetime"
        );
        self.spawn_children(parent, wave.kind, wave.count, wave.radius)?;
        for e in self.entities.matching_mut(|e| {
            e.parent == Some(parent) && e.kind == wave.kind && e.expires.is_none()
        }) {
            let id = e.id;
            e.expires
                .set(Some(self.now + wave.lifetime), id, &mut self.timed_ids);
            e.activated.get_or_insert(self.now);
            if matches!(e.kind, 0x174a | 0x174b | 0x944) {
                let rate = match e.kind {
                    0x174a => 1.,
                    0x174b => 1.3,
                    0x944 => 0.1,
                    _ => unreachable!(),
                };
                e.orbit = Some(realm_events::Orbit {
                    speed: crate::encounter::nr_speed(
                        rate + self.rng.gen_range(-rate * 0.1..rate * 0.1),
                    ),
                    radius: if e.kind == 0x944 {
                        self.rng.gen_range(0.5..1.5)
                    } else if e.kind == 0x174a {
                        1.
                    } else {
                        9.
                    },
                    direction: -1.,
                });
            }
            if matches!(e.kind, 0x1748 | 0x174a | 0x174b | 0xde4) {
                e.condition |= condition::bit(condition::INVULNERABLE);
            }
        }
        Ok(())
    }
    pub(super) fn finale_mechanics(
        &mut self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<()> {
        let target = target.into();
        let e = &self.entities[&id];
        let (kind, hp, pos) = (e.kind, e.hp, e.pos);
        if GUARDIANS.contains(&kind) {
            let peer = self
                .entities
                .values()
                .any(|e| e.id != id && GUARDIANS.contains(&e.kind) && e.hp > 0);
            let idle = peer
                && self.entities[&id].stage.is_none_or(|s| s == 0)
                && target.is_none_or(|p| pos.distance(p) > 10.);
            let facts = if idle {
                None
            } else {
                Some(
                    self.finale
                        .guardians
                        .entry(id)
                        .or_default()
                        .update(peer, self.now)?,
                )
            };
            let e = self.entities.get_mut(&id).unwrap();
            e.stage = Some(if idle { 0 } else { 1 });
            e.condition &=
                !(condition::bit(condition::PARALYZED) | condition::bit(condition::INVULNERABLE));
            if idle {
                e.condition |=
                    condition::bit(condition::PARALYZED) | condition::bit(condition::INVULNERABLE);
            }
            let Some(facts) = facts else { return Ok(()) };
            if facts.invulnerable {
                e.condition |= condition::bit(condition::INVULNERABLE);
            }
            if let Some(sword) = self.finale.guardians[&id].awaiting_sword() {
                ensure!(
                    self.entities.contains_key(&sword),
                    "bound Guardian sword disappeared without its actual defeat"
                );
            }
            if facts.summon_sword {
                self.spawn_children(id, 0xd7c, 1, 5.)?;
                let sword = self
                    .entities
                    .values()
                    .find(|e| e.parent == Some(id) && e.kind == 0xd7c)
                    .context("Guardian sword spawn did not produce its bound identity")?
                    .id;
                self.finale
                    .guardians
                    .get_mut(&id)
                    .unwrap()
                    .bind_sword(sword)?;
                if facts.heal {
                    let e = self.entities.get_mut(&id).unwrap();
                    if e.conditions(self.now) & condition::bit(condition::SICK) == 0 {
                        e.hp = (e.hp + 7200).min(self.content.objects[&kind].hp);
                    }
                }
                tracing::info!(
                    guardian = id,
                    sword,
                    "surviving Guardian recovered and bound its real sword"
                );
            }
            return Ok(());
        }
        if kind == 0xd7e {
            return self.armor_mechanics(id);
        }
        if kind == 0x944 && self.entities[&id].orbit.is_none() {
            self.entities.get_mut(&id).unwrap().orbit = Some(realm_events::Orbit {
                speed: crate::encounter::nr_speed(0.1 + self.rng.gen_range(-0.01..0.01)),
                radius: self.rng.gen_range(0.5..1.5),
                direction: -1.,
            });
        }
        if matches!(kind, 0x67e..=0x682) {
            self.entities.get_mut(&id).unwrap().stage.get_or_insert(0);
        }
        if finale::HEROES.contains(&kind) {
            let initial = self.entities[&id].stage.is_none();
            if initial {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(0);
            }
            if initial || self.now >= self.entities[&id].next_aux {
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 2000;
                self.hero_reinforcements(id, initial)?;
            }
            return Ok(());
        }
        if kind == 0x174a && elapsed >= 10000 && self.entities[&id].size != 200 {
            self.entities.get_mut(&id).unwrap().size = 200;
            tracing::debug!(entity = id, "inner Guardian Element grew");
        }
        if matches!(kind, ORYX | ORYX2) {
            let mut brain = self.finale.bosses.entry(id).or_default().clone();
            let facts = brain.update(kind, hp, self.content.objects[&kind].hp, elapsed)?;
            let e = self.entities.get_mut(&id).unwrap();
            e.condition &=
                !(condition::bit(condition::INVULNERABLE) | condition::bit(condition::PARALYZED));
            if facts.invulnerable {
                e.condition |= condition::bit(condition::INVULNERABLE);
            }
            if facts.phase == OryxPhase::PrepareRage {
                e.condition |= condition::bit(condition::PARALYZED);
            }
            for wave in facts.waves {
                self.finale_wave(id, wave)?;
            }
            if facts.changed {
                self.entities.get_mut(&id).unwrap().next_shot = self.now;
                tracing::info!(entity=id,kind,phase=?facts.phase,"Oryx phase committed");
            }
            self.finale.bosses.insert(id, brain);
        }
        if matches!(kind, 0x1748 | 0x174a | 0x174b | 0xde4) {
            self.entities.get_mut(&id).unwrap().condition |=
                condition::bit(condition::INVULNERABLE);
        }
        if self.now < self.entities[&id].next_aux {
            return Ok(());
        }
        self.entities.get_mut(&id).unwrap().next_aux = self.now
            + if matches!(kind, 0xde2 | 0xdf1) {
                1000
            } else {
                5000
            };
        for wave in finale::summons(kind) {
            self.finale_wave(id, *wave)?;
        }
        match kind {
            ORYX if target.is_some()
                && matches!(
                    self.finale.bosses[&id].phase(),
                    Some(OryxPhase::Artifacts | OryxPhase::Rage)
                ) =>
            {
                self.throw_grenade(id, target.unwrap(), 4., 120)?
            }
            ORYX2 if self.finale.bosses[&id].phase() == Some(OryxPhase::Rage) => self.finale_wave(
                id,
                finale::Wave {
                    kind: 0xde2,
                    count: 1,
                    radius: 7.,
                    lifetime: 10_000,
                },
            )?,
            0xde2 if target.is_some_and(|p| pos.distance(p) < 2.) => {
                self.area_damage(
                    kind,
                    pos,
                    3.,
                    self.content.objects[&kind].projectiles[&0].min,
                )?;
                self.entities.get_mut(&id).unwrap().expires.set(
                    Some(self.now + 50),
                    id,
                    &mut self.timed_ids,
                );
            }
            0xdf1 if target.is_some_and(|p| pos.distance(p) < 2.) => {
                self.area_damage(kind, pos, 2., 40)?
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn finale_shots(
        &self,
        id: i32,
        heading: f32,
        elapsed: u64,
    ) -> Result<Vec<(u8, f32)>> {
        let e = &self.entities[&id];
        if GUARDIANS.contains(&e.kind) && e.stage == Some(0) {
            return Ok(vec![]);
        }
        if let Some(schedule) = self.npc_attack_schedule(id, elapsed)? {
            return Ok(schedule.snapshot(heading));
        }
        Ok(finale::attacks(
            e.kind,
            e.hp,
            self.content.objects[&e.kind].hp,
            heading,
            elapsed,
        ))
    }
    pub(super) fn hero_movement(
        &mut self,
        id: i32,
        target: Option<Pos>,
        delta: u64,
    ) -> Result<(crate::encounter::Motion, f32, bool)> {
        use crate::encounter::{Anchor, Motion};
        let e = &self.entities[&id];
        let paralyzed = e.conditions(self.now) & condition::bit(condition::PARALYZED) != 0;
        let stage = e
            .stage
            .context("activated Realm hero movement phase missing")?;
        let distance = target.map(|p| e.pos.distance(p));
        let (motion, speed, completed, next) = match e.kind {
            0x668 => match stage {
                0 => (Motion::WanderHome { radius: 1. }, 3., false, 1),
                1 => (
                    if paralyzed || distance.is_some_and(|d| d > 2. && d < 10.) {
                        Motion::Chase {
                            sight: 10.,
                            stop: 2.,
                        }
                    } else {
                        Motion::Stationary
                    },
                    3.,
                    !paralyzed && distance.is_none_or(|d| d <= 2. || d >= 10.),
                    2,
                ),
                2 => (
                    Motion::Protect {
                        anchor: Anchor::Home,
                        distance: 1.,
                    },
                    3.,
                    !paralyzed
                        && (e.pos.x - e.home.x).abs() <= 1.
                        && (e.pos.y - e.home.y).abs() <= 1.,
                    0,
                ),
                _ => bail!("invalid Red Demon movement phase {stage}"),
            },
            0x67e..=0x682 => {
                if distance.is_none_or(|d| d >= 10.) {
                    let e = self.entities.get_mut(&id).unwrap();
                    if e.reaction_until > self.now.saturating_sub(delta) {
                        e.reaction_until += delta;
                    }
                    return Ok((Motion::Wander, 10., false));
                }
                match stage {
                    0 => (
                        Motion::Chase {
                            sight: 10.,
                            stop: 2.,
                        },
                        10.,
                        !paralyzed && distance.is_some_and(|d| d <= 2.),
                        1,
                    ),
                    1 => (Motion::Retreat { distance: 8. }, 8., false, 0),
                    _ => bail!("invalid Cyclops movement phase {stage}"),
                }
            }
            _ => bail!("unsupported queued Realm hero {:#x}", e.kind),
        };
        let timed: Option<u64> = if e.kind == 0x668 && stage < 2 {
            Some(5000)
        } else if e.kind != 0x668 && stage == 1 {
            Some(1000)
        } else {
            None
        };
        let e = self.entities.get_mut(&id).unwrap();
        if let Some(duration) = timed
            && e.reaction_until == 0
        {
            e.reaction_until = self.now + duration.saturating_sub(delta);
        }
        let completed = completed || timed.is_some() && self.now >= e.reaction_until;
        e.stage = Some(if completed { next } else { stage });
        if completed {
            e.reaction_until = 0;
            tracing::debug!(
                entity = id,
                stage = next,
                "Realm hero movement child completed"
            );
        }
        // ReturnSpawn's axis-aligned completion must not move again before the next child.
        Ok((
            if completed && timed.is_none() {
                Motion::Stationary
            } else {
                motion
            },
            speed,
            completed,
        ))
    }
    pub(super) fn finale_movement(
        &mut self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<(crate::encounter::Motion, f32)> {
        use crate::encounter::{Anchor, Motion, nr_speed};
        let e = &self.entities[&id];
        match e.kind {
            0xd7e if e.stage == Some(2) => return Ok((Motion::Stationary, 0.)),
            0xd7e => {
                return Ok((
                    Motion::Follow {
                        sight: 10.,
                        stop: 2.,
                        idle: nr_speed(0.2),
                        duration: 0,
                        cooldown: 0,
                    },
                    nr_speed(0.4),
                ));
            }
            0xd82 => {
                let charge = elapsed % 9000 >= 5000;
                return Ok((
                    Motion::Follow {
                        sight: 10.,
                        stop: 1.,
                        idle: nr_speed(if charge { 0.3 } else { 0.2 }),
                        duration: 0,
                        cooldown: 0,
                    },
                    nr_speed(if charge { 1.2 } else { 0.4 }),
                ));
            }
            0xd83 => {
                return Ok((
                    Motion::Follow {
                        sight: 10.,
                        stop: 3.,
                        idle: nr_speed(0.2),
                        duration: 0,
                        cooldown: 0,
                    },
                    nr_speed(if elapsed % 7500 >= 5000 { 0.7 } else { 0.6 }),
                ));
            }
            0x174a | 0x174b => {
                let orbit = e
                    .orbit
                    .context("Guardian Element orbit was not initialized at birth")?;
                return Ok((
                    Motion::Orbit {
                        anchor: Anchor::Parent,
                        radius: orbit.radius,
                    },
                    orbit.speed,
                ));
            }
            0x944 => {
                if let Some((_, anchor)) = self
                    .escort_anchor(id, ORYX2)?
                    .filter(|(boss, _)| e.pos.distance(self.entities[boss].pos) < 10.)
                {
                    let orbit = e
                        .orbit
                        .context("Henchman orbit was not initialized at activation")?;
                    return Ok((
                        Motion::Orbit {
                            anchor,
                            radius: orbit.radius,
                        },
                        orbit.speed,
                    ));
                }
                return Ok((
                    Motion::Follow {
                        sight: 8.,
                        stop: 3.,
                        idle: nr_speed(0.2),
                        duration: 0,
                        cooldown: 0,
                    },
                    nr_speed(0.2),
                ));
            }
            ORYX if self
                .finale
                .bosses
                .get(&id)
                .context("moving Oryx brain missing")?
                .phase()
                == Some(OryxPhase::Rage) =>
            {
                return Ok(if target.is_some() {
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
            ORYX2
                if self
                    .finale
                    .bosses
                    .get(&id)
                    .context("moving Oryx2 brain missing")?
                    .phase()
                    == Some(OryxPhase::Rage) =>
            {
                return Ok((
                    Motion::Chase {
                        sight: 25.,
                        stop: 3.,
                    },
                    8.,
                ));
            }
            _ => {}
        }
        Ok(crate::encounter::movement(e.kind, e.hp, elapsed))
    }
    pub(super) fn finale_death(&mut self, defeated: &Entity) -> Result<()> {
        ensure!(
            defeated.hp <= 0,
            "endgame defeat callback requires actual zero health"
        );
        if defeated.kind == 0xd7c {
            let parent = defeated
                .parent
                .context("defeated Guardian sword has no actual parent")?;
            self.finale
                .guardians
                .get_mut(&parent)
                .context("Guardian sword parent brain missing")?
                .sword_defeated(defeated.id, self.now)?;
            tracing::info!(
                sword = defeated.id,
                guardian = parent,
                "actual bound Guardian sword defeat credited"
            );
        }
        if GUARDIANS.contains(&defeated.kind) {
            let survivor = self
                .entities
                .values()
                .find(|e| GUARDIANS.contains(&e.kind) && e.hp > 0)
                .map(|e| e.id);
            if let Some(id) = survivor {
                self.finale
                    .guardians
                    .entry(id)
                    .or_default()
                    .update(false, self.now)?;
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.condition |= condition::bit(condition::INVULNERABLE);
            } else {
                ensure!(!self.cleared, "Guardian pair completion replayed");
                self.cleared = true;
                tracing::info!("actual Guardian pair defeated; Chamber binding requested");
            }
        }
        if matches!(defeated.kind, ORYX | ORYX2) {
            ensure!(!self.cleared, "Oryx completion replayed");
            self.cleared = true;
            if defeated.kind == ORYX {
                let locked = self.spawn_exit(finale::LOCKED_CELLAR, defeated.pos)?;
                self.bag_expiry.insert(locked, self.now + 120_000);
            } else {
                self.spawn_exit(0x712, defeated.pos)?;
            }
            for e in self.entities.matching_mut(|e| e.kind == 0x71d) {
                e.published = true;
            }
            tracing::info!(
                boss = defeated.kind,
                "actual Oryx defeat opened earned exits"
            );
        }
        self.finale.bosses.remove(&defeated.id);
        self.finale.guardians.remove(&defeated.id);
        Ok(())
    }
}
