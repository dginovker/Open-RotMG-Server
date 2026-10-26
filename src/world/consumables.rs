use super::*;
use crate::content::{ConsumableEffect, Object};

#[derive(Clone)]
pub(super) struct Boost {
    source: (i32, i32),
    activation: usize,
    index: usize,
    amount: i32,
    until: Option<u64>,
}
impl Player {
    pub(super) fn effective(&self, content: &Content, now: u64) -> Result<[i32; 8]> {
        let mut values = self.character.effective(content)?;
        for boost in &self.boosts {
            if boost.until.is_some_and(|until| until > now) {
                values[boost.index] = values[boost.index]
                    .checked_add(boost.amount)
                    .context("temporary stat overflow")?;
            }
        }
        Ok(values)
    }
}
impl World {
    pub(crate) fn validate_prepared_base_stats(&self, id: i32, values: [i32; 8]) -> Result<()> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.boosts
                .iter()
                .filter(|b| b.source == (0, 0) && b.until.is_some_and(|until| until > self.now))
                .all(|b| values[b.index]
                    .checked_add(b.amount.min(0))
                    .is_some_and(|value| value >= 0)),
            controller("base stats conflict with active prepared modifier")
        );
        Ok(())
    }

    pub(crate) fn prepare_stat_modifier(
        &mut self,
        id: i32,
        index: usize,
        amount: i32,
        duration_ms: u64,
    ) -> Result<()> {
        ensure!(
            (2..=7).contains(&index),
            controller("prepared modifier requires a panel stat index 2–7")
        );
        ensure!(
            amount != 0 && (-100..=100).contains(&amount),
            controller("prepared modifier amount must be nonzero and within -100–100")
        );
        ensure!(
            (1..=60_000).contains(&duration_ms),
            controller("prepared modifier duration must be 1–60000ms")
        );
        let until = self
            .now
            .checked_add(duration_ms)
            .context("prepared modifier deadline overflow")?;
        let p = self.players.get(&id).context("player missing")?;
        let value = p.effective(&self.content, self.now)?[index]
            .checked_add(amount)
            .context("prepared modifier overflow")?;
        ensure!(
            (0..=32767).contains(&value),
            controller("prepared modifier effective stat out of range")
        );
        // Positive consumables and equipment may disappear before this prepared modifier expires.
        ensure!(
            p.character.base[index]
                .checked_add(amount.min(0))
                .is_some_and(|v| v >= 0),
            controller("prepared modifier exceeds base stat")
        );
        let active = |b: &&Boost| b.until.is_none_or(|until| until > self.now);
        ensure!(
            p.boosts.iter().filter(active).count() < 128,
            controller("temporary stat effect budget exhausted")
        );
        ensure!(
            !p.boosts.iter().filter(active).any(|b| b.source == (0, 0)),
            controller("prepared modifier already active")
        );
        let p = self.players.get_mut(&id).unwrap();
        p.boosts
            .retain(|b| b.until.is_none_or(|until| until > self.now));
        p.boosts.push(Boost {
            source: (0, 0),
            activation: 0,
            index,
            amount,
            until: Some(until),
        });
        self.updates(id, true)
    }

    pub(super) fn expire_consumable_boosts(&mut self) {
        for p in self.players.values_mut() {
            p.boosts
                .retain(|boost| boost.until.is_none_or(|until| until > self.now));
            let values = p
                .effective(&self.content, self.now)
                .expect("validated temporary stats");
            p.character.hp = p.character.hp.min(values[0]);
            p.character.mp = p.character.mp.min(values[1]);
        }
    }
    fn consumable_targets(&self, id: i32, range: Option<f32>) -> Vec<i32> {
        let position = self.players[&id].pos;
        self.players
            .iter()
            .filter(|(other, p)| {
                p.active()
                    && !p.paused
                    && !p.pending
                    && p.character.hp > 0
                    && match range {
                        Some(range) => p.pos.distance(position) < range,
                        None => **other == id,
                    }
            })
            .map(|(id, _)| *id)
            .collect()
    }
    pub(super) fn begin_consumable(
        &mut self,
        id: i32,
        slot: Slot,
        item: &Object,
    ) -> Result<InventoryPlan> {
        ensure!(
            item.consumable,
            controller("item is not a supported consumable")
        );
        ensure!(
            !item.consumable_effects.is_empty()
                && item.consumable_effects.len() == item.activations,
            "item is not a supported consumable"
        );
        for effect in &item.consumable_effects {
            if let ConsumableEffect::Unsupported(name) = effect {
                bail!("item activation not implemented: {name}");
            }
        }
        let next = if let Some(name) = &item.successor {
            let next = self
                .content
                .objects
                .values()
                .find(|next| &next.name == name)
                .context("consumable successor is missing")?;
            ensure!(
                next.consumable
                    && next.slot_type == item.slot_type
                    && item
                        .doses
                        .is_some_and(|doses| doses > 1 && next.doses == Some(doses - 1)),
                "invalid consumable dose succession"
            );
            i32::from(next.kind)
        } else {
            -1
        };
        let source = (
            self.players[&id].character.id,
            self.players[&id].character.revision,
        );
        let mut reservations = Vec::new();
        let mut counts = BTreeMap::<i32, usize>::new();
        for (activation, effect) in item.consumable_effects.iter().enumerate() {
            if let ConsumableEffect::Boost {
                index,
                amount,
                range,
                ..
            } = effect
            {
                for target in self.consumable_targets(id, *range) {
                    *counts.entry(target).or_default() += 1;
                    reservations.push((
                        target,
                        Boost {
                            source,
                            activation,
                            index: *index,
                            amount: *amount,
                            until: None,
                        },
                    ));
                }
            }
        }
        for (target, count) in counts {
            let p = self.players.get_mut(&target).unwrap();
            p.boosts
                .retain(|b| b.until.is_none_or(|until| until > self.now));
            ensure!(
                p.boosts.len() + count <= 128,
                controller("temporary stat effect budget exhausted")
            );
        }
        for (target, boost) in reservations {
            self.players.get_mut(&target).unwrap().boosts.push(boost);
        }
        let mut plan = self.consumable_plan(id, slot, next)?;
        plan.effects = item.consumable_effects.clone();
        plan.potion = (slot.object == id && item.is_potion()).then_some(slot);
        Ok(plan)
    }
    pub(super) fn discard_consumable(&mut self, source: (i32, i32)) {
        for p in self.players.values_mut() {
            p.boosts.retain(|b| b.until.is_some() || b.source != source);
        }
    }
    pub(super) fn consumable_feedback(
        &self,
        target: i32,
        notification: Option<Vec<u8>>,
        range: Option<f32>,
    ) {
        let (kind, radius) = match range {
            Some(range) => (5, range),
            None => (1, 0.),
        };
        let visual = cooperative::effect(kind, target, Pos { x: radius, y: 0. });
        for p in self
            .players
            .values()
            .filter(|p| p.active() && p.character.hp > 0 && !p.pending && p.known.contains(&target))
        {
            if let Some(body) = &notification {
                send(p, wire::NOTIFICATION, body.clone());
            }
            send(p, wire::SHOW_EFFECT, visual.clone());
        }
    }
    pub(super) fn consumable_restored(&self, target: i32, amount: i32, color: u32) {
        if amount > 0 {
            let mut out = Writer::new();
            out.i32(target).utf(&format!("+{amount}")).u32(color);
            self.consumable_feedback(target, Some(out.0), None);
        }
    }
    pub(super) fn finish_consumable(
        &mut self,
        player: i32,
        source: (i32, i32),
        effects: Vec<ConsumableEffect>,
    ) -> Result<()> {
        for (activation, effect) in effects.iter().enumerate() {
            if let ConsumableEffect::Boost {
                duration, range, ..
            } = effect
            {
                for p in self.players.values_mut() {
                    if p.active() && p.character.hp > 0 {
                        for boost in p.boosts.iter_mut().filter(|b| {
                            b.source == source && b.activation == activation && b.until.is_none()
                        }) {
                            boost.until = Some(
                                self.now
                                    .checked_add(*duration)
                                    .context("boost deadline overflow")?,
                            );
                        }
                    }
                }
                if self.players[&player].active() && self.players[&player].character.hp > 0 {
                    self.consumable_feedback(player, None, *range);
                }
                continue;
            }
            if !self.players[&player].active() || self.players[&player].character.hp <= 0 {
                continue;
            }
            match *effect {
                ConsumableEffect::Heal {
                    amount,
                    range: Some(range),
                } => self.consumable_heal_nova(player, amount, range)?,
                ConsumableEffect::Heal {
                    amount,
                    range: None,
                } => {
                    let p = self.players.get_mut(&player).unwrap();
                    if p.conditions(self.now) & crate::condition::bit(crate::condition::SICK) == 0 {
                        let before = p.character.hp;
                        p.character.hp =
                            (p.character.hp + amount).min(p.effective(&self.content, self.now)?[0]);
                        let restored = p.character.hp - before;
                        self.consumable_restored(player, restored, 0xff00ff00);
                    }
                }
                ConsumableEffect::Mana { amount, range } => {
                    for target in self.consumable_targets(player, range) {
                        let p = self.players.get_mut(&target).unwrap();
                        if p.conditions(self.now) & crate::condition::bit(crate::condition::QUIET)
                            == 0
                        {
                            let before = p.character.mp;
                            p.character.mp = (p.character.mp + amount)
                                .min(p.effective(&self.content, self.now)?[1]);
                            let restored = p.character.mp - before;
                            // Match requested cyan without changing shared notification styling: https://github.com/OpenRotMG/Open-RotMG-Server/issues/322
                            self.consumable_restored(target, restored, 0xff00ffff);
                        }
                    }
                    if let Some(range) = range {
                        self.consumable_feedback(player, None, Some(range));
                    }
                }
                ConsumableEffect::Condition { effect, duration } => {
                    self.apply_condition(player, effect, duration)?;
                    self.consumable_feedback(player, None, Some(1.));
                }
                ConsumableEffect::Clear { effect, check } => {
                    let p = self.players.get_mut(&player).unwrap();
                    if check.is_none_or(|c| p.conditions(self.now) & crate::condition::bit(c) != 0)
                    {
                        p.statuses.remove(&effect);
                        p.lab_conditions &= !crate::condition::bit(effect);
                    }
                }
                ConsumableEffect::Cleanse => {
                    let p = self.players.get_mut(&player).unwrap();
                    p.statuses
                        .retain(|effect, _| !matches!(effect, 2..=11 | 14 | 16 | 27 | 28));
                    p.lab_conditions &= !(crate::condition::bit(crate::condition::HEXED) | STUNNED);
                    self.consumable_feedback(player, None, Some(1.));
                }
                ConsumableEffect::Boost { .. } => unreachable!(),
                ConsumableEffect::Unsupported(ref name) => {
                    bail!("unvalidated committed consumable effect: {name}")
                }
            }
        }
        self.discard_consumable(source);
        if !effects.is_empty() {
            tracing::info!(
                player = player,
                revision = source.1,
                effects = effects.len(),
                "consumable effects published after inventory commit"
            );
        }
        Ok(())
    }
}
