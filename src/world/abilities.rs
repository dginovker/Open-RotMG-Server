use super::*;
use crate::content::{Ability, Object};

const THROW_MS: u64 = 1500;
const MAX_ACTIVE: usize = 256;

#[derive(Default)]
pub(super) struct State {
    thrown: Vec<Thrown>,
    poisons: Vec<Poison>,
    pub(super) objects: BTreeMap<i32, Deployed>,
}
struct Thrown {
    owner: i32,
    position: Pos,
    at: u64,
    effect: Ability,
}
struct Poison {
    owner: i32,
    start: u64,
    duration: u64,
    tick: u64,
    targets: Vec<(i32, i32, i32)>,
}
pub(super) struct Deployed {
    owner: i32,
    until: u64,
    pulse: u64,
    kind: Deployment,
}
enum Deployment {
    Trap {
        radius: f32,
        damage: i32,
        effect: u8,
        duration: u64,
    },
    Decoy {
        angle: f32,
        speed: f32,
        moving_until: u64,
    },
    Bomb,
}
impl World {
    // Client getTimer cooldowns continue during skipped physics ticks: https://github.com/moistosaurus/realm-cli/blob/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e/src/com/company/assembleegameclient/objects/Player.as#L899-L917
    pub(super) fn ability_clock_ms(&self) -> Result<u64> {
        Ok(self
            .clock
            .now()
            .checked_duration_since(self.attack_epoch)
            .context("ability clock regressed")?
            .as_millis() as u64)
    }

    pub(super) fn spend_ability(
        &mut self,
        id: i32,
        slot: Slot,
        item: &Object,
        pos: Pos,
    ) -> Result<()> {
        let now = self.ability_clock_ms()?;
        let p = &self.players[&id];
        ensure!(
            p.persistence_failure.is_none(),
            "ability caster is unavailable"
        );
        ensure!(
            p.character.alive
                && !p.death_pending
                && !p.departing
                && (!p.pending || p.progress_started)
                && !p.paused
                && p.persistence_failure.is_none(),
            controller("ability caster is unavailable")
        );
        ensure!(
            slot.object == id && slot.kind == item.kind as i32 && p.character.items[1] == slot.kind,
            controller("ability item does not match equipped item")
        );
        ensure!(
            slot.slot == 1 && item.slot_type == self.content.get(p.character.class)?.slots[1],
            controller("ability must be equipped by a compatible class")
        );
        ensure!(
            p.tx.as_ref()
                .is_none_or(|tx| !tx.lost.borrow().is_some() && !tx.packets.is_closed()),
            controller("ability connection is closed")
        );
        ensure!(
            p.conditions(self.now) & crate::condition::bit(crate::condition::QUIET) == 0,
            controller("cannot cast while Quiet")
        );
        ensure!(
            p.character.mp >= item.mana_cost,
            controller("not enough mana")
        );
        ensure!(now >= p.ability_ready_ms, controller("ability cooldown"));
        let reach = item
            .abilities
            .iter()
            .filter_map(|a| {
                if let Ability::Teleport { distance } = a {
                    Some(*distance)
                } else {
                    None
                }
            })
            .fold(15., f32::max);
        ensure!(
            pos.x.is_finite()
                && pos.y.is_finite()
                && p.pos.distance(pos) <= reach
                && self.map.cell(pos.x, pos.y).is_some(),
            controller("invalid ability target or target too far")
        );
        let ability_ready_ms = now
            .checked_add(item.cooldown_ms)
            .context("ability cooldown overflow")?;
        let p = self.players.get_mut(&id).unwrap();
        p.character.mp -= item.mana_cost;
        p.character.lifetime.ability_used = true;
        p.ability_ready_ms = ability_ready_ms;
        tracing::info!(
            player = id,
            item = item.kind,
            mana_cost = item.mana_cost,
            cooldown_ms = item.cooldown_ms,
            ability_clock_ms = now,
            simulation_time_ms = self.now,
            "ability activated"
        );
        Ok(())
    }
    fn ability_allies(&self, id: i32, range: Option<f32>) -> Vec<i32> {
        let origin = self.players[&id].pos;
        self.players
            .iter()
            .filter(|(target, p)| {
                p.character.alive
                    && !p.death_pending
                    && !p.departing
                    && !p.paused
                    && p.persistence_failure.is_none()
                    && match range {
                        Some(r) => origin.distance(p.pos) <= r,
                        None => **target == id,
                    }
            })
            .map(|(id, _)| *id)
            .collect()
    }
    fn ability_enemies(&self, pos: Pos, radius: f32) -> Vec<i32> {
        let mut ids: Vec<_> = self
            .near_entities(
                Pos {
                    x: pos.x - radius,
                    y: pos.y - radius,
                },
                Pos {
                    x: pos.x + radius,
                    y: pos.y + radius,
                },
            )
            .filter(|e| {
                e.hp > 0 && self.content.objects[&e.kind].enemy && e.pos.distance(pos) <= radius
            })
            .map(|e| e.id)
            .collect();
        ids.sort_unstable();
        ids
    }
    fn ability_effect(&self, kind: u8, owner: i32, a: Pos, b: Pos, color: u32) {
        let mut out = Writer::new();
        out.u8(kind)
            .i32(owner)
            .f32(a.x)
            .f32(a.y)
            .f32(b.x)
            .f32(b.y)
            .u32(color);
        for p in self.players.values().filter(|p| p.known.contains(&owner)) {
            send(p, wire::SHOW_EFFECT, out.0.clone());
        }
    }
    pub(super) fn activate_ability(
        &mut self,
        id: i32,
        slot: Slot,
        item: &Object,
        pos: Pos,
    ) -> Result<()> {
        ensure!(
            item.abilities.len() == item.activations,
            "ability activation count mismatch"
        );
        ensure!(
            self.ability_state.thrown.len()
                + self.ability_state.poisons.len()
                + self.ability_state.objects.len()
                + item.abilities.len()
                <= MAX_ACTIVE,
            controller("active ability budget exhausted")
        );
        let origin = self.players[&id].pos;
        for effect in &item.abilities {
            match effect {
                Ability::Shoot => {
                    ensure!(
                        (1..=5).contains(&item.shots)
                            && item.activations == 1
                            && item.projectiles.contains_key(&0),
                        "unsupported ability projectile layout"
                    );
                }
                Ability::Nova => {
                    ensure!(
                        item.projectiles.contains_key(&0),
                        "ability has no projectile zero"
                    );
                }
                Ability::Teleport { distance } => {
                    ensure!(
                        origin.distance(pos) <= *distance && self.walkable(pos),
                        controller("teleport target is blocked or outside ability range")
                    );
                }
                Ability::Trap { .. } => {
                    ensure!(self.walkable(pos), controller("trap target is blocked"));
                    self.content.get(0x711)?;
                }
                Ability::Decoy { .. } => {
                    self.content.get(0x715)?;
                }
                Ability::SpiritBomb => {
                    self.content.get(0x1409)?;
                }
                _ => {}
            }
        }
        self.spend_ability(id, slot, item, pos)?;
        let heading = if item
            .abilities
            .iter()
            .any(|a| matches!(a, Ability::Decoy { .. }))
        {
            // A stationary caster has no recent movement heading; legacy decoys choose a random direction.
            Some(match self.players[&id].last_motion {
                Some((angle, at)) if self.now - at <= 100 => angle,
                _ => self.rng.gen_range(0. ..std::f32::consts::TAU),
            })
        } else {
            None
        };
        for effect in &item.abilities {
            tracing::debug!(
                player = id,
                item = item.kind,
                ?effect,
                "ability activation applied"
            );
            match *effect {
                // Native clients report their own ability projectiles; other controllers have no client.
                Ability::Shoot if self.players[&id].controller != Controller::Human => {
                    self.ability_shots(id, item, origin, pos)?
                }
                Ability::Shoot => {}
                Ability::Nova => self.ability_nova(id, item, pos)?,
                Ability::Heal { amount, range } => self.heal_ability(id, item, amount, range)?,
                Ability::Condition {
                    effect,
                    duration,
                    range,
                } => {
                    for target in self.ability_allies(id, range) {
                        self.apply_condition(target, effect, duration)?;
                    }
                }
                Ability::Clear {
                    effect,
                    check,
                    range,
                } => {
                    for target in self.ability_allies(id, Some(range)) {
                        let p = self.players.get_mut(&target).unwrap();
                        if check
                            .is_none_or(|c| p.conditions(self.now) & crate::condition::bit(c) != 0)
                        {
                            p.statuses.remove(&effect);
                        }
                    }
                }
                Ability::Cleanse { range } => {
                    for target in self.ability_allies(id, Some(range)) {
                        self.players
                            .get_mut(&target)
                            .unwrap()
                            .statuses
                            .retain(|effect, _| !matches!(effect, 2..=11 | 14 | 16 | 27 | 28));
                    }
                }
                Ability::Poison { .. } | Ability::Trap { .. } => {
                    self.ability_effect(
                        4,
                        id,
                        pos,
                        origin,
                        if matches!(effect, Ability::Poison { .. }) {
                            0xffddff00
                        } else {
                            0xff9000ff
                        },
                    );
                    self.ability_state.thrown.push(Thrown {
                        owner: id,
                        position: pos,
                        at: self.now + THROW_MS,
                        effect: effect.clone(),
                    });
                }
                Ability::Drain { radius, damage } => {
                    self.ability_effect(7, id, pos, origin, 0xffff0000);
                    self.ability_effect(
                        8,
                        id,
                        pos,
                        Pos {
                            x: pos.x + radius,
                            y: pos.y,
                        },
                        0xffff0000,
                    );
                    let mut total = 0;
                    for target in self.ability_enemies(pos, radius) {
                        total += self.damage_enemy(id, target, damage, false, &[], None)?;
                    }
                    if total > 0 {
                        self.heal_ability(id, item, total, radius)?;
                    }
                }
                Ability::Stasis { duration } => {
                    self.ability_effect(
                        12,
                        id,
                        pos,
                        Pos {
                            x: pos.x + 3.,
                            y: pos.y,
                        },
                        0xffffffff,
                    );
                    for target in self.ability_enemies(pos, 3.) {
                        let flags = self.entities[&target].conditions(self.now);
                        if flags
                            & (crate::condition::bit(crate::condition::STASIS)
                                | crate::condition::bit(crate::condition::STASIS_IMMUNE))
                            == 0
                        {
                            self.apply_condition(target, crate::condition::STASIS, duration)?;
                        } else if flags & crate::condition::bit(crate::condition::STASIS_IMMUNE)
                            != 0
                        {
                            let mut out = Writer::new();
                            out.i32(target).utf("Immune").u32(0xff00ff00);
                            for p in self.players.values().filter(|p| p.known.contains(&target)) {
                                send(p, wire::NOTIFICATION, out.0.clone());
                            }
                            tracing::debug!(player = id, target, "stasis target immune");
                        }
                    }
                }
                Ability::Teleport { .. } => {
                    let p = self.players.get_mut(&id).unwrap();
                    p.pos = pos;
                    p.movement_credit = 0.78;
                    p.last_motion = None;
                    let mut out = Writer::new();
                    out.i32(id).f32(pos.x).f32(pos.y);
                    for (peer, p) in &mut self.players {
                        if p.known.contains(&id) {
                            reconciliation::goto(p, *peer == id, out.0.clone());
                        }
                    }
                    self.ability_effect(2, id, pos, origin, 0xffffffff);
                }
                Ability::Decoy { duration, angle } => {
                    let object = self.spawn_entity(0x715, origin, vec![])?;
                    self.ability_state.objects.insert(
                        object,
                        Deployed {
                            owner: id,
                            until: self.now + duration,
                            pulse: self.now,
                            kind: Deployment::Decoy {
                                angle: heading.unwrap() + angle,
                                speed: self.players[&id].speed(&self.content, self.now)? * 1000.,
                                moving_until: self.now + duration / 2,
                            },
                        },
                    );
                }
                Ability::Lightning {
                    damage,
                    targets,
                    condition,
                    color,
                } => self.ability_lightning(id, pos, damage, targets, condition, color)?,
                Ability::Daze { radius, damage } => {
                    self.ability_effect(
                        8,
                        id,
                        pos,
                        Pos {
                            x: pos.x + radius,
                            y: pos.y,
                        },
                        0xffffffff,
                    );
                    for target in self.ability_enemies(pos, radius) {
                        self.damage_enemy(
                            id,
                            target,
                            damage,
                            false,
                            &[(crate::condition::DAZED, 3000)],
                            None,
                        )?;
                    }
                }
                Ability::SpiritBomb => {
                    // This preserved item predates the modern six-blast prism: one 300-damage spirit bomb.
                    let object = self.spawn_entity(0x1409, origin, vec![])?;
                    self.ability_state.objects.insert(
                        object,
                        Deployed {
                            owner: id,
                            until: self.now + 3000,
                            pulse: self.now,
                            kind: Deployment::Bomb,
                        },
                    );
                }
            }
        }
        Ok(())
    }
    fn ability_shots(&mut self, id: i32, item: &Object, origin: Pos, target: Pos) -> Result<()> {
        let time = self.now as i32;
        let aim = (target.y - origin.y).atan2(target.x - origin.x);
        // Like the client, every pellet leaves the central muzzle at its own spread angle.
        let start = Pos {
            x: origin.x + 0.3 * aim.cos(),
            y: origin.y + 0.3 * aim.sin(),
        };
        let p = self.players.get_mut(&id).unwrap();
        p.ability_shot = Some((item.kind, time, true, 0));
        let first = p.next_bullet;
        for i in 0..item.shots {
            let spread = item.arc.to_radians() * (i as f32 - (item.shots - 1) as f32 / 2.);
            self.shoot(
                id,
                time,
                (first + i as u8) % 128,
                item.kind,
                start,
                aim + spread,
            )?;
        }
        Ok(())
    }
    fn ability_lightning(
        &mut self,
        owner: i32,
        pos: Pos,
        damage: i32,
        maximum: usize,
        effect: Option<(u8, u64)>,
        color: u32,
    ) -> Result<()> {
        let origin = self.players[&owner].pos;
        let aim = (pos.y - origin.y).atan2(pos.x - origin.x);
        let eligible = |e: &&Entity| {
            self.content.objects[&e.kind].enemy
                && e.hp > 0
                && e.conditions(self.now)
                    & (crate::condition::bit(crate::condition::STASIS)
                        | crate::condition::bit(crate::condition::INVINCIBLE)
                        | crate::condition::bit(crate::condition::INVULNERABLE))
                    == 0
        };
        let first = self
            .ability_enemies(origin, 14.)
            .into_iter()
            .map(|id| &self.entities[&id])
            .filter(eligible)
            .filter(|e| {
                origin.distance(e.pos) <= 14.
                    && ((e.pos.y - origin.y).atan2(e.pos.x - origin.x) - aim)
                        .sin()
                        .atan2(((e.pos.y - origin.y).atan2(e.pos.x - origin.x) - aim).cos())
                        .abs()
                        <= std::f32::consts::FRAC_PI_4
            })
            .min_by(|a, b| origin.distance(a.pos).total_cmp(&origin.distance(b.pos)))
            .map(|e| e.id);
        let mut chain = Vec::new();
        if let Some(first) = first {
            chain.push(first);
            while chain.len() < maximum {
                let from = self.entities[chain.last().unwrap()].pos;
                let next = self
                    .ability_enemies(from, 10.)
                    .into_iter()
                    .map(|id| &self.entities[&id])
                    .filter(eligible)
                    .filter(|e| !chain.contains(&e.id) && from.distance(e.pos) <= 10.)
                    .min_by(|a, b| from.distance(a.pos).total_cmp(&from.distance(b.pos)))
                    .map(|e| e.id);
                match next {
                    Some(id) => chain.push(id),
                    None => break,
                }
            }
        }
        let mut previous = owner;
        if chain.is_empty() {
            for offset in [
                0.,
                -std::f32::consts::FRAC_PI_4,
                std::f32::consts::FRAC_PI_4,
            ] {
                let angle = aim + offset;
                self.ability_effect(
                    7,
                    owner,
                    Pos {
                        x: origin.x + 14. * angle.cos(),
                        y: origin.y + 14. * angle.sin(),
                    },
                    Pos { x: 350., y: 0. },
                    color,
                );
            }
        }
        // Announce every chain segment before lethal damage can remove its previous endpoint.
        for target in &chain {
            self.ability_effect(
                11,
                previous,
                self.entities[target].pos,
                Pos { x: 350., y: 0. },
                color,
            );
            previous = *target;
        }
        let effects = effect.into_iter().collect::<Vec<_>>();
        for target in chain {
            self.damage_enemy(owner, target, damage, false, &effects, None)?;
        }
        Ok(())
    }
    fn ability_nova(&mut self, id: i32, item: &Object, pos: Pos) -> Result<()> {
        let spec = item
            .projectiles
            .get(&0)
            .context("ability has no projectile zero")?
            .clone();
        let p = self.players.get_mut(&id).unwrap();
        p.character.shots += 20;
        p.character.lifetime.shots = p
            .character
            .lifetime
            .shots
            .checked_add(20)
            .context("lifetime ability shots overflow")?;
        let mut messages = Vec::new();
        for i in 0..20 {
            let bullet = p.server_bullet;
            p.server_bullet = 128 + (bullet.wrapping_add(1) % 128);
            let angle = i as f32 * std::f32::consts::TAU / 20.;
            let damage = self.rng.gen_range(spec.min..=spec.max);
            self.bullets.push(Bullet {
                owner: id,
                source_kind: p.character.class,
                id: bullet,
                friendly: true,
                start: pos,
                pos,
                angle,
                born: self.now,
                damage,
                spec: spec.clone(),
                hit: BTreeSet::new(),
                hit_enemy: false,
            });
            let mut out = Writer::new();
            out.u8(bullet)
                .i32(id)
                .i16(item.kind as i16)
                .f32(pos.x)
                .f32(pos.y)
                .f32(angle)
                .i16(damage as i16);
            messages.push(out.0);
        }
        for body in messages {
            for (peer, p) in &mut self.players {
                if p.known.contains(&id)
                    && (*peer == id || p.pos.distance(pos) <= 20.)
                    && (*peer != id || native_hits::expect_shoot_ack(p, vec![], &self.clock))
                {
                    send(p, 13, body.clone());
                }
            }
        }
        Ok(())
    }
    fn ability_owner(&self, id: i32) -> bool {
        self.players.get(&id).is_some_and(|p| {
            p.character.alive
                && !p.death_pending
                && !p.departing
                && !p.paused
                && p.persistence_failure.is_none()
        })
    }
    pub(super) fn decoy_positions(&self) -> impl Iterator<Item = Pos> + '_ {
        self.ability_state
            .objects
            .iter()
            .filter(|(_, o)| matches!(o.kind, Deployment::Decoy { .. }))
            .map(|(id, _)| self.entities[id].pos)
    }
    pub(super) fn clear_abilities(&mut self, owner: i32) {
        self.ability_state.thrown.retain(|t| t.owner != owner);
        self.ability_state.poisons.retain(|p| p.owner != owner);
        let ids: Vec<_> = self
            .ability_state
            .objects
            .iter()
            .filter(|(_, o)| o.owner == owner)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.remove_entity(id);
        }
    }
    pub(super) fn ability_step(&mut self) {
        for thrown in std::mem::take(&mut self.ability_state.thrown) {
            if !self.ability_owner(thrown.owner) {
                continue;
            }
            if self.now < thrown.at {
                self.ability_state.thrown.push(thrown);
                continue;
            }
            match thrown.effect {
                Ability::Poison {
                    radius,
                    damage,
                    duration,
                } => {
                    self.ability_effect(
                        8,
                        thrown.owner,
                        thrown.position,
                        Pos {
                            x: thrown.position.x + radius,
                            y: thrown.position.y,
                        },
                        0xffddff00,
                    );
                    let targets = self
                        .ability_enemies(thrown.position, radius)
                        .into_iter()
                        .map(|id| {
                            let e = &self.entities[&id];
                            (
                                id,
                                crate::condition::damage(
                                    damage,
                                    self.content.objects[&e.kind].defense,
                                    e.conditions(self.now),
                                    false,
                                ),
                                0,
                            )
                        })
                        .collect();
                    self.ability_state.poisons.push(Poison {
                        owner: thrown.owner,
                        start: self.now,
                        duration,
                        tick: self.now,
                        targets,
                    });
                }
                Ability::Trap {
                    radius,
                    damage,
                    effect,
                    duration,
                } => {
                    let id = self
                        .spawn_entity(0x711, thrown.position, vec![])
                        .expect("validated trap landing");
                    self.ability_state.objects.insert(
                        id,
                        Deployed {
                            owner: thrown.owner,
                            until: self.now + 10000,
                            pulse: self.now,
                            kind: Deployment::Trap {
                                radius,
                                damage,
                                effect,
                                duration,
                            },
                        },
                    );
                }
                _ => unreachable!("only thrown abilities enter the landing queue"),
            }
        }
        for mut poison in std::mem::take(&mut self.ability_state.poisons) {
            if !self.ability_owner(poison.owner) {
                continue;
            }
            let elapsed = (self.now - poison.start).min(poison.duration);
            if self.now - poison.tick >= 250 || elapsed == poison.duration {
                poison.tick = self.now;
                poison
                    .targets
                    .retain(|(id, _, _)| self.entities.contains_key(id));
                for (target, total, paid) in &mut poison.targets {
                    let due = (i64::from(*total) * elapsed as i64 / poison.duration as i64) as i32;
                    let amount = due - *paid;
                    *paid = due;
                    if amount > 0 {
                        self.damage_enemy(poison.owner, *target, amount, true, &[], None)
                            .expect("validated poison damage");
                    }
                }
            }
            if elapsed < poison.duration && !poison.targets.is_empty() {
                self.ability_state.poisons.push(poison);
            }
        }
        for (id, mut object) in std::mem::take(&mut self.ability_state.objects) {
            if !self.entities.contains_key(&id) {
                continue;
            }
            if !self.ability_owner(object.owner) {
                self.remove_entity(id);
                continue;
            }
            let pos = self.entities[&id].pos;
            let expired = self.now >= object.until;
            match object.kind {
                Deployment::Trap {
                    radius,
                    damage,
                    effect,
                    duration,
                } => {
                    let triggered = !self.ability_enemies(pos, radius / 2.).is_empty();
                    if triggered || expired {
                        self.ability_effect(
                            8,
                            object.owner,
                            pos,
                            Pos {
                                x: pos.x + radius,
                                y: pos.y,
                            },
                            0xff9000ff,
                        );
                        for target in self.ability_enemies(pos, radius) {
                            self.damage_enemy(
                                object.owner,
                                target,
                                damage,
                                false,
                                &[(effect, duration)],
                                None,
                            )
                            .expect("validated trap damage");
                        }
                        self.remove_entity(id);
                        continue;
                    }
                    if self.now >= object.pulse {
                        self.ability_effect(
                            10,
                            id,
                            Pos {
                                x: radius / 2.,
                                y: 0.,
                            },
                            pos,
                            0xff9000ff,
                        );
                        object.pulse = self.now + 500;
                    }
                }
                Deployment::Decoy {
                    angle,
                    speed,
                    moving_until,
                } => {
                    if expired {
                        self.ability_effect(5, id, Pos { x: 1., y: 0. }, pos, 0xffff0000);
                        self.remove_entity(id);
                        continue;
                    }
                    if self.now < moving_until {
                        let goal = Pos {
                            x: pos.x + angle.cos() * speed * 0.05,
                            y: pos.y + angle.sin() * speed * 0.05,
                        };
                        if self.walkable(goal) {
                            self.move_entity(id, goal, speed * 0.05)
                                .expect("validated decoy movement");
                        }
                    }
                }
                Deployment::Bomb => {
                    if expired {
                        self.ability_effect(5, id, Pos { x: 4., y: 0. }, pos, 0xffffffff);
                        for target in self.ability_enemies(pos, 4.) {
                            self.damage_enemy(object.owner, target, 300, false, &[], None)
                                .expect("validated spirit bomb damage");
                        }
                        self.remove_entity(id);
                        continue;
                    }
                }
            }
            self.ability_state.objects.insert(id, object);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ability_cooldown_uses_elapsed_unpaused_time_not_world_steps() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let content = Arc::new(Content::load(&data).unwrap());
        let map = Arc::new(Map::load(&data.join("tutorial.wmap"), &content).unwrap());
        let mut world = World::new(content, map).unwrap();
        let character: Character = serde_json::from_value(serde_json::json!({
            "schema":1,"lifetime":crate::store::FameLifetime::complete(),
            "class":0x30e,"base":[670,385,75,25,50,75,40,60],
            "id":1,"account":1,"name":"Cooldown","level":20,"exp":0,
            "fame":0,"hp":670,"mp":385,"revision":0,"alive":true,
            "items":[-1,0xa2e,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
        }))
        .unwrap();
        let id = world.enter(character, None).unwrap();
        let pos = world.players[&id].pos;
        let item = world.content.get(0xa2e).unwrap().clone();
        assert_eq!(item.cooldown_ms, 500);
        let slot = Slot {
            object: id,
            slot: 1,
            kind: 0xa2e,
        };
        world.spend_ability(id, slot, &item, pos).unwrap();
        let mp = world.players[&id].character.mp;
        assert!(
            world
                .spend_ability(id, slot, &item, pos)
                .unwrap_err()
                .to_string()
                .contains("ability cooldown")
        );
        assert_eq!(world.players[&id].character.mp, mp);
        world.clock.set_paused(true);
        std::thread::sleep(Duration::from_millis(550));
        assert!(world.spend_ability(id, slot, &item, pos).is_err());
        world.clock.set_paused(false);
        assert!(world.spend_ability(id, slot, &item, pos).is_err());
        std::thread::sleep(Duration::from_millis(550));
        assert_eq!(world.now, 0);
        world
            .spend_ability(id, slot, &item, pos)
            .expect("real cooldown elapsed despite no simulation ticks");
        assert_eq!(world.players[&id].character.mp, mp - item.mana_cost);
        assert!(world.spend_ability(id, slot, &item, pos).is_err());
    }

    #[test]
    fn autonomous_shoot_ability_fans_from_one_muzzle_under_native_condition_rules() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let content = Arc::new(Content::load(&data).unwrap());
        let map = Arc::new(Map::load(&data.join("tutorial.wmap"), &content).unwrap());
        let mut world = World::new(content, map).unwrap();
        let character: Character = serde_json::from_value(serde_json::json!({
            "schema":1,"lifetime":crate::store::FameLifetime::complete(),
            "class":0x31e,"base":[770,252,50,40,50,50,75,50],
            "id":1,"account":1,"name":"Shield","level":20,"exp":0,
            "fame":0,"hp":770,"mp":252,"revision":0,"alive":true,
            "items":[-1,0xa09,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
        }))
        .unwrap();
        let id = world.enter(character, None).unwrap();
        world.players.get_mut(&id).unwrap().controller = Controller::Autonomous;
        // Native ability shots stay legal while Paralyzed or Stunned; only the weapon is suppressed.
        for condition in [crate::condition::PARALYZED, crate::condition::STUNNED] {
            world.apply_condition(id, condition, 5000).unwrap();
        }
        let pos = world.players[&id].pos;
        let shield = world.content.get(0xa09).unwrap().clone();
        assert_eq!(shield.shots, 2);
        let slot = Slot {
            object: id,
            slot: 1,
            kind: 0xa09,
        };
        let target = Pos {
            x: pos.x,
            y: pos.y + 2.,
        };
        world.use_item(id, slot, target).unwrap();
        let central = std::f32::consts::FRAC_PI_2;
        let half = shield.arc.to_radians() / 2.;
        let shots: Vec<_> = world.bullets.iter().filter(|b| b.owner == id).collect();
        assert_eq!(shots.len(), 2);
        let muzzle = Pos {
            x: pos.x + 0.3 * central.cos(),
            y: pos.y + 0.3 * central.sin(),
        };
        for (shot, angle) in shots.iter().zip([central - half, central + half]) {
            assert!(
                (shot.angle - angle).abs() < 1e-5,
                "{} != {angle}",
                shot.angle
            );
            assert!(shot.start.distance(muzzle) < 1e-5);
        }
        assert_eq!(world.players[&id].character.mp, 252 - shield.mana_cost);
        assert!(world.players[&id].ability_shot.is_none());
        std::thread::sleep(Duration::from_millis(shield.cooldown_ms + 50));
        world
            .apply_condition(id, crate::condition::QUIET, 1000)
            .unwrap();
        assert!(world.use_item(id, slot, target).is_err());
        assert_eq!(world.bullets.len(), 2);
    }
}
