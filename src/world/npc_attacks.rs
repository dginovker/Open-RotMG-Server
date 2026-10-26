use super::*;
use crate::{
    condition,
    encounter::attacks::{Action, Aim, Prediction, Schedule},
};

impl World {
    pub(super) fn suspend_npc(&mut self, id: i32, delta: u64) {
        let e = self.entities.get_mut(&id).expect("suspended NPC missing");
        if let Some(activated) = &mut e.activated {
            *activated += delta;
        }
        if e.next_shot >= self.now.saturating_sub(delta) {
            e.next_shot += delta;
        }
        if e.next_aux >= self.now.saturating_sub(delta) {
            e.next_aux += delta;
        }
        if let Some(since) = &mut e.blocked_since {
            *since += delta;
        }
        if e.reaction_until > self.now.saturating_sub(delta) {
            e.reaction_until += delta;
        }
        e.attacks.suspend(delta);
        if let Some(runtime) = &mut e.abyss {
            runtime.suspend(delta);
        }
        if let Some(charge) = &mut e.charge {
            charge.suspend(delta);
        }
        if let Some(tomb) = &mut e.tomb {
            tomb.suspend(delta);
        }
    }
    pub(super) fn npc_attack_schedule(&self, id: i32, elapsed: u64) -> Result<Option<Schedule>> {
        use crate::{
            encounter::attacks,
            endgame::{ORYX, ORYX2, OryxPhase},
        };
        let e = &self.entities[&id];
        if e.kind == crate::dungeon::snake::DART {
            let active = if let Some(owner) = *e.parent {
                ensure!(
                    self.entities
                        .get(&owner)
                        .is_some_and(|e| e.kind == crate::dungeon::snake::SPAWNER),
                    "Snake Pit dart {id} has a missing or invalid room owner {owner}"
                );
                self.actors()
                    .any(|e| e.kind == crate::dungeon::snake::GUARD && e.parent == Some(owner))
            } else {
                false
            };
            let mut schedule = attacks::schedule(e.kind, e.hp, elapsed)
                .context("Snake Pit dart schedule missing")?;
            schedule.state = u64::from(active);
            schedule.attacks[0].enabled = active;
            let neighbor = self
                .map
                .cell(e.home.x + 1., e.home.y)
                .context("Snake Pit dart has no adjacent map cell")?;
            schedule.attacks[0].aim = Aim::Fixed(if neighbor.object.is_some() {
                std::f32::consts::PI
            } else {
                0.
            });
            return Ok(Some(schedule));
        }
        if crate::dungeon::abyss::ROSTER.contains(&e.kind)
            || e.kind == 0x651 && self.name == "Abyss of Demons"
        {
            use crate::dungeon::abyss::{self, IDOL, MALPHAS};
            let elapsed = if e.kind == MALPHAS {
                let Some(super::abyss::Runtime::Boss(b)) = &e.abyss else {
                    bail!("Abyss Malphas {id} attack runtime missing")
                };
                ensure!(
                    b.identity == elapsed / 50200 * 7 + abyss::phase(elapsed),
                    "Abyss Malphas {id} attack/phase identity mismatch"
                );
                b.identity / 7 * 50200
                    + [0, 9000, 11500, 23500, 36500, 39000, 47000][(b.identity % 7) as usize]
                    + self
                        .now
                        .checked_sub(b.entered)
                        .context("Abyss phase entry is in the future")?
            } else {
                elapsed
            };
            let mut schedule = attacks::abyss_schedule(e.kind, elapsed);
            if e.kind == 0x90b
                && let Some(super::abyss::Runtime::Flamer {
                    terminal: Some(deadline),
                    emitted,
                    ..
                }) = e.abyss
            {
                schedule.state = 1;
                schedule.age = self.now.saturating_sub(deadline);
                schedule.attacks[0].enabled = false;
                schedule.attacks[1].enabled = !emitted && self.now >= deadline;
            }
            if matches!(e.kind, MALPHAS | IDOL) {
                ensure!(
                    e.stage
                        == Some(if e.kind == MALPHAS {
                            abyss::phase(elapsed)
                        } else {
                            0
                        }),
                    "Abyss attack phase missing or inconsistent: entity={id} kind={:#x} stage={:?} elapsed={elapsed}",
                    e.kind,
                    e.stage
                );
            }
            if e.kind == 0x909 {
                match e.stage {
                    None => schedule.attacks[0].enabled = false,
                    Some(1) => {
                        schedule.state = 1;
                        let armed = e
                            .next_aux
                            .checked_sub(600)
                            .context("Abyss missile arm deadline missing")?;
                        schedule.age = self
                            .now
                            .checked_sub(armed)
                            .context("Abyss missile armed in the future")?;
                    }
                    Some(stage) => bail!("invalid Abyss missile attack stage {stage}"),
                }
            }
            return Ok(Some(schedule));
        }
        if matches!(
            e.kind,
            crate::dungeon::manor::KAGE | crate::dungeon::manor::MINE
        ) {
            // Entry resets rotation, but the caller's age predates mechanics: https://github.com/UniverseRealms/vanilla-7.0/blob/ba1fa22c58c24b4a2403860116656492d0133748/realm-server-master/Game/Logic/Behaviors/Shoot.cs#L55-L59
            let age = self
                .now
                .checked_sub(e.activated.context("Manor entrance phase clock missing")?)
                .context("Manor entrance phase clock starts in the future")?;
            return Ok(Some(attacks::manor_entry_schedule(
                e.kind,
                e.stage.context("Manor entrance phase missing")?,
                age,
            )));
        }
        if crate::dungeon::jungle::ROSTER.contains(&e.kind) {
            return Ok(Some(self.jungle_schedule(id)?));
        }
        if crate::dungeon::davy::profile(e.kind).is_some() {
            let mut schedule = attacks::davy_schedule(e.kind, e.stage.unwrap_or(0), elapsed);
            if e.kind == 0xe50 {
                let owner = e
                    .parent
                    .and_then(|id| self.entities.get(&id))
                    .context("senior ghost lacks its captain")?;
                schedule.attacks[0].count = if owner.hp < self.content.objects[&owner.kind].hp / 2 {
                    3
                } else {
                    2
                };
            }
            return Ok(Some(schedule));
        }
        if crate::dungeon::tomb::contains(e.kind) {
            return Ok(Some(crate::encounter::tomb::schedule(
                e.kind,
                e.stage.context("Tomb attack phase missing")?,
                elapsed,
                self.tomb_guard(id).is_some(),
            )));
        }
        if crate::encounter::landmarks::ROSTER.contains(&e.kind) {
            let state = e.stage.context("landmark attack phase missing")?;
            let script = e
                .script
                .as_ref()
                .context("landmark attack script missing")?;
            return Ok(Some(crate::encounter::landmarks::schedule(
                e.kind,
                e.hp,
                state,
                script.choice,
                if matches!(e.kind, 0xd54 | 0xd55) {
                    self.now - e.activated.context("Sphinx phase clock missing")?
                } else {
                    elapsed
                },
            )));
        }
        if e.kind == ORYX2 {
            let brain = self
                .finale
                .bosses
                .get(&id)
                .context("activated Oryx2 brain missing")?;
            let phase = brain.phase().context("activated Oryx2 phase missing")?;
            let age = match phase {
                OryxPhase::Opening => elapsed,
                OryxPhase::PrepareRage => brain
                    .rage_elapsed(elapsed)
                    .context("rage preparation clock missing")?,
                OryxPhase::Rage => brain
                    .rage_elapsed(elapsed)
                    .context("rage clock missing")?
                    .checked_sub(10000)
                    .context("rage began before protected preparation completed")?,
                _ => bail!("invalid Oryx2 phase"),
            };
            return Ok(Some(attacks::oryx2_schedule(phase, age)));
        }
        if e.kind == 0xd7e {
            let stage = e.stage.context("activated Suit of Armor phase missing")?;
            let age = if stage == 2 {
                self.now - (e.reaction_until - 1500)
            } else {
                elapsed
            };
            return Ok(Some(attacks::armor_schedule(stage, age)));
        }
        let mut schedule = match attacks::schedule(e.kind, e.hp, elapsed) {
            Some(schedule) => Some(schedule),
            None => {
                attacks::endgame_schedule(e.kind, e.hp, self.content.objects[&e.kind].hp, elapsed)
            }
        };
        if matches!(e.kind, 0x903 | 0x904) {
            let stage = e.stage.context("Flamer attack phase missing")?;
            let s = schedule.as_mut().context("Flamer schedule missing")?;
            s.state = stage;
            s.attacks[0].enabled = stage == 1;
            s.attacks[1].enabled = stage == 3;
        }
        if e.kind == 0x7f00 && !matches!(e.stage, Some(2 | 3)) {
            for attack in &mut schedule
                .as_mut()
                .context("Sumo Master schedule missing")?
                .attacks
            {
                attack.enabled = false;
            }
        }
        if e.kind == 0x20d {
            schedule
                .as_mut()
                .context("Gray Blob schedule missing")?
                .attacks[0]
                .enabled = e.stage == Some(1);
        }
        if e.kind == ORYX && e.hp <= self.content.objects[&e.kind].hp / 5 {
            schedule
                .as_mut()
                .context("Oryx rage attack schedule missing")?
                .age = self
                .finale
                .bosses
                .get(&id)
                .context("activated Oryx brain missing")?
                .rage_elapsed(elapsed)
                .context("Oryx rage entry clock missing")?;
        }
        if e.kind == 0xd06 && e.reaction_until > self.now {
            schedule
                .as_mut()
                .context("Limon attack schedule missing")?
                .attacks[2]
                .enabled = true;
        }
        Ok(schedule)
    }
    pub(super) fn legacy_enemy_shots(
        &self,
        id: i32,
        target: Pos,
        elapsed: u64,
    ) -> Result<Vec<(u8, f32)>> {
        let e = &self.entities[&id];
        let kind = e.kind;
        ensure!(
            !(crate::dungeon::abyss::ROSTER.contains(&kind)
                || kind == 0x651 && self.name == "Abyss of Demons"),
            "scheduled Abyss attacker reached legacy dispatch: entity={id} kind={kind:#x}"
        );
        let heading = (target.y - e.pos.y).atan2(target.x - e.pos.x);
        if crate::dungeon::lab::ROSTER.contains(&kind) {
            let mut shots = self.lab_shots(id, target);
            if matches!(kind, 0x980 | 0x981 | 0x1e23 | 0x1e24) {
                self.predict_enemy_shots(
                    id,
                    target,
                    &mut shots,
                    |index| if index < 2 { 1. } else { 0. },
                )?;
            }
            Ok(shots)
        } else if crate::dungeon::trench::ROSTER.contains(&kind) {
            self.ocean_shots(id, target)
        } else if e.quest.is_some() {
            self.quest_shots(id, target, elapsed)
        } else if crate::endgame::ROSTER.contains(&kind) {
            self.finale_shots(id, heading, elapsed)
        } else if matches!(kind, 0x900..=0x902) {
            self.shield_shots(id, heading)
        } else if gods::GODS.contains(&kind)
            || gods::CONSTRUCTS.contains(&kind)
            || midlands::AIMED.contains(&kind)
        {
            self.god_shots(id, target, heading, elapsed)
        } else {
            Ok(crate::encounter::phased_attacks(
                kind, e.hp, heading, elapsed,
            ))
        }
    }
    pub(super) fn scheduled_attacks(
        &mut self,
        id: i32,
        target: Option<Pos>,
        schedule: &Schedule,
    ) -> Result<Vec<(u8, f32)>> {
        let due = self
            .entities
            .get_mut(&id)
            .context("scheduled attacker missing")?
            .attacks
            .due(schedule, self.now);
        let pos = self.entities[&id].pos;
        let mut shots = vec![];
        for slot in due {
            let mut attack = schedule.attacks[slot];
            let heading = match attack.aim {
                Aim::Fixed(angle) => angle,
                Aim::Target => {
                    let Some(target) = target.filter(|p| pos.distance(*p) <= attack.range) else {
                        continue;
                    };
                    (target.y - pos.y).atan2(target.x - pos.x)
                }
            };
            match attack.action {
                Action::Grenade { radius, damage } => {
                    self.throw_grenade(
                        id,
                        target.context("targeted grenade has no target")?,
                        radius,
                        damage,
                    )?;
                }
                Action::Shoot { projectile, choice } => {
                    let projectile = match choice {
                        Some(other) if self.rng.gen_bool(0.5) => {
                            attack.count = other.count;
                            attack.gap = other.gap;
                            attack.offset = other.offset;
                            other.projectile
                        }
                        _ => projectile,
                    };
                    let mut volley = attack.shots(projectile, heading);
                    let factor = match attack.predict {
                        Prediction::None => 0.,
                        Prediction::Lead(factor) => factor,
                        Prediction::Chance(chance) => {
                            if self.rng.gen_bool(chance) {
                                1.
                            } else {
                                0.
                            }
                        }
                    };
                    if factor > 0. {
                        self.predict_enemy_shots(
                            id,
                            target.context("predictive attack has no target")?,
                            &mut volley,
                            |_| factor,
                        )?;
                    }
                    if !(self.entities[&id].kind == 0x90b
                        && Self::abyss_flamer_armed(&self.entities[&id]))
                        && self.entities[&id].conditions(self.now)
                            & condition::bit(condition::DAZED)
                            != 0
                    {
                        volley = enemy_shots::dazed(volley);
                    }
                    shots.extend(volley);
                }
            }
            tracing::trace!(
                entity = id,
                slot,
                phase = schedule.state,
                phase_age = schedule.age,
                now = self.now,
                interval = ?attack.interval,
                delay = attack.delay,
                range = attack.range,
                "NPC weapon deadline fired"
            );
        }
        Ok(shots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn both_sprite_gods_lead_only_the_quiet_projectile() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
        for cell in &mut map.cells {
            cell.ground = 0x61;
            cell.object = None;
        }
        let target = Pos { x: 12.5, y: 12.5 };
        map.spawns = vec![(target.x, target.y)];
        let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
        let account = store.list("sprite-aim".into()).await.unwrap();
        let character = store
            .create(account.id, content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        let mut w = World::new(content, Arc::new(map)).unwrap();
        let player = w.enter(character, None).unwrap();
        for kind in [0x652, 0xd05] {
            let id = w
                .spawn_entity(kind, Pos { x: 12.5, y: 4.5 }, vec![])
                .unwrap();
            for velocity in [-3., 0., 3.] {
                w.now += 1000;
                w.players.get_mut(&player).unwrap().motion = ObservedMotion {
                    position: target,
                    time: 0,
                    velocity: Pos { x: velocity, y: 0. },
                    at: w.now,
                };
                let schedule = w.npc_attack_schedule(id, 0).unwrap().unwrap();
                let shots = w.scheduled_attacks(id, Some(target), &schedule).unwrap();
                assert_eq!(shots.len(), 5);
                for ((index, angle), offset) in shots[..4].iter().zip([-15f32, -5., 5., 15.]) {
                    assert_eq!(*index, 0);
                    assert!(
                        (*angle - std::f32::consts::FRAC_PI_2 - offset.to_radians()).abs() < 1e-5
                    );
                }
                let (index, angle) = shots[4];
                assert_eq!(index, 1);
                let expected = 8f32.atan2(velocity * 8. / 6.);
                assert!(
                    (angle - expected).abs() < 1e-5,
                    "kind {kind:#x}, velocity {velocity}: Quiet {angle} != {expected}"
                );
            }
            w.remove_entity(id);
        }
    }
}
