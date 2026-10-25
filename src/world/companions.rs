use super::*;
use crate::content::Object;

#[derive(Clone)]
pub(crate) struct Plan {
    pub source: Slot,
    pub permanent: bool,
    pub selected: Option<u16>,
    entity: Option<i32>,
    cooldown: u64,
}
struct Companion {
    owner: i32,
    kind: u16,
    permanent: bool,
    published: bool,
    until: Option<u64>,
    pulse: u64,
    remaining: u8,
}
#[derive(Default)]
pub(super) struct State {
    active: BTreeMap<i32, Companion>,
    bleeds: BTreeMap<i32, (u64, u64)>,
}
impl State {
    pub(super) fn contains(&self, entity: &i32) -> bool {
        self.active.contains_key(entity)
    }
}
// Firecrackers expire after their last pulse; drake lifetimes come from the period eggs table.
fn duration(kind: u16) -> Result<Option<u64>> {
    match kind {
        0x1401 => Ok(Some(60_000)),
        0x1400 | 0x1402..=0x1405 | 0x140a => Ok(Some(30_000)),
        0x1407 | 0x1408 => Ok(None),
        _ => bail!("This temporary companion is not implemented yet."),
    }
}
fn castle_successor(content: &Content, item: &Object) -> Result<i32> {
    let doses = match item.kind {
        // The selected terminal pail omits Doses; the loader represents it as one dose.
        0xc34..=0xc38 => Some((0xc39 - item.kind) as u8),
        _ => bail!("Sand Castle activation requires a selected Sand Pail"),
    };
    let metadata = item
        .companion
        .as_ref()
        .context("Sand Pail activation missing")?;
    ensure!(
        item.doses == doses
            && item.class == "Equipment"
            && item.consumable
            && item.slot_type == 10
            && item.activations == 1
            && !metadata.permanent
            && metadata.object == "Sand Castle",
        "invalid selected Sand Pail dose metadata"
    );
    if item.kind == 0xc38 {
        ensure!(item.successor.is_none(), "last Sand Pail has a successor");
        return Ok(-1);
    }
    let next = content.named(
        item.successor
            .as_deref()
            .context("Sand Pail successor missing")?,
    )?;
    ensure!(
        next.kind == item.kind + 1,
        "Sand Pail successor is not the next dose"
    );
    castle_successor(content, next)?;
    Ok(i32::from(next.kind))
}

impl World {
    fn reserve_companion(
        &mut self,
        owner: i32,
        kind: u16,
        permanent: bool,
        pos: Pos,
    ) -> Result<i32> {
        ensure!(
            self.companions.active.len() < 128,
            controller("companion population limit reached")
        );
        if !permanent {
            duration(kind)?;
            if matches!(kind, 0x1407 | 0x1408) {
                // Prove all ten cumulative [-2, 2) jumps valid before consumption or RNG draws.
                ensure!(
                    pos.x.is_finite()
                        && pos.y.is_finite()
                        && self.map.cell(pos.x - 20., pos.y - 20.).is_some()
                        && self.map.cell(pos.x + 10., pos.y + 10.).is_some(),
                    controller(
                        "Firecrackers need at least 20 tiles from the left/top and more than 10 from the right/bottom world boundary."
                    )
                );
            }
            ensure!(
                self.companions
                    .active
                    .values()
                    .filter(|p| p.owner == owner && !p.permanent)
                    .count()
                    < 8,
                controller("at most eight temporary companions may be active")
            );
        }
        let entity = self.spawn_entity(kind, pos, vec![])?;
        let e = self.entities.get_mut(&entity).unwrap();
        e.published = false;
        e.reserved = true;
        for (stat, value) in [32, 33]
            .into_iter()
            .zip(self.content.objects[&kind].textures)
        {
            if let Some(value) = value {
                e.extra_stats.push((stat, value as i32));
            }
        }
        self.companions.active.insert(
            entity,
            Companion {
                owner,
                kind,
                permanent,
                published: false,
                until: None,
                pulse: self.now,
                remaining: if matches!(kind, 0x1407 | 0x1408) {
                    10
                } else {
                    0
                },
            },
        );
        Ok(entity)
    }
    pub(super) fn attach_companion(&mut self, owner: i32, kind: u16, pos: Pos) -> Result<()> {
        self.content.permanent_companion(kind)?;
        let id = self.reserve_companion(owner, kind, true, pos)?;
        self.publish_companion(id)
    }
    fn publish_companion(&mut self, id: i32) -> Result<()> {
        let pet = self
            .companions
            .active
            .get_mut(&id)
            .context("reserved companion lost")?;
        ensure!(!pet.published, "companion already published");
        pet.published = true;
        if !pet.permanent {
            pet.until = duration(pet.kind)?.map(|duration| self.now + duration);
        }
        pet.pulse = self.now
            + if pet.remaining > 0 {
                0
            } else if pet.kind == 0x140a {
                1000
            } else {
                50
            };
        let e = self
            .entities
            .get_mut(&id)
            .context("reserved companion entity lost")?;
        ensure!(
            e.reserved && !e.published,
            "companion publication reservation lost"
        );
        e.reserved = false;
        e.published = true;
        tracing::info!(
            companion = id,
            owner = pet.owner,
            kind = pet.kind,
            until = pet.until,
            "companion published"
        );
        Ok(())
    }
    pub(super) fn begin_companion(
        &mut self,
        id: i32,
        slot: Slot,
        item: &Object,
    ) -> Result<InventoryPlan> {
        let metadata = item
            .companion
            .as_ref()
            .context("companion activation metadata missing")?;
        ensure!(
            item.class == "Equipment" && item.slot_type == 10 && item.activations == 1,
            "invalid companion item activation"
        );
        let kind = self.content.companion_kind(&metadata.object)?;
        let p = &self.players[&id];
        if metadata.permanent {
            ensure!(
                p.last_companion
                    .is_none_or(|last| self.now >= last + metadata.cooldown),
                controller("companion generator cooldown")
            );
        } else {
            ensure!(item.consumable, "temporary companion must consume its egg");
            if let Some(duration) = duration(kind)? {
                self.now
                    .checked_add(duration)
                    .context("companion deadline overflow")?;
            }
        }
        let selected =
            if metadata.permanent && !item.consumable && p.character.pet_kind == Some(kind) {
                None
            } else {
                Some(kind)
            };
        let pos = p.pos;
        let mut items = p.character.items.clone();
        if item.consumable {
            items[slot.slot] = if kind == 0x140a {
                castle_successor(&self.content, item)?
            } else {
                -1
            };
        }
        let entity = if selected.is_some() {
            Some(self.reserve_companion(id, kind, metadata.permanent, pos)?)
        } else {
            None
        };
        let mut plan = self.plan(id, items, None);
        plan.companion = Some(Plan {
            source: slot,
            permanent: metadata.permanent,
            selected,
            entity,
            cooldown: metadata.cooldown,
        });
        self.players.get_mut(&id).unwrap().pending = true;
        tracing::info!(
            player = id,
            item = item.kind,
            kind,
            entity,
            selected,
            "companion staged; item not committed yet"
        );
        Ok(plan)
    }
    pub(super) fn discard_companion(&mut self, plan: &Option<Plan>) -> Result<()> {
        if let Some(entity) = plan.as_ref().and_then(|p| p.entity) {
            let pet = self
                .companions
                .active
                .get(&entity)
                .context("unpublished companion lost")?;
            ensure!(!pet.published, "cannot discard a published companion");
            self.remove_companion(entity);
        }
        Ok(())
    }
    pub(super) fn finish_companion(&mut self, owner: i32, plan: Option<Plan>) -> Result<()> {
        let Some(plan) = plan else { return Ok(()) };
        if plan.permanent {
            let old: Vec<_> = self
                .companions
                .active
                .iter()
                .filter(|(id, p)| p.owner == owner && p.permanent && Some(**id) != plan.entity)
                .map(|(id, _)| *id)
                .collect();
            for id in old {
                self.remove_companion(id);
            }
            let p = self
                .players
                .get_mut(&owner)
                .context("committed companion owner lost")?;
            p.character.pet_kind = plan.selected;
            p.last_companion = Some(self.now);
        }
        if let Some(entity) = plan.entity {
            self.publish_companion(entity)?;
        }
        tracing::info!(
            player = owner,
            selected = plan.selected,
            permanent = plan.permanent,
            cooldown = plan.cooldown,
            "companion selection committed"
        );
        Ok(())
    }
    fn remove_companion(&mut self, entity: i32) {
        self.companions.active.remove(&entity);
        if self.entities.contains_key(&entity) {
            self.entities.get_mut(&entity).unwrap().reserved = false;
            self.remove_entity(entity);
        }
    }
    pub(super) fn clear_companions(&mut self, owner: i32) {
        let ids: Vec<_> = self
            .companions
            .active
            .iter()
            .filter(|(_, p)| p.owner == owner)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.remove_companion(id);
        }
    }
    fn companion_effect(&self, id: i32, radius: f32) {
        let body = cooperative::effect(5, id, Pos { x: radius, y: 0. });
        for p in self.players.values().filter(|p| p.known.contains(&id)) {
            send(p, wire::SHOW_EFFECT, body.clone());
        }
    }
    fn firecracker_pulse(&mut self, id: i32, kind: u16) -> Result<()> {
        let pos = self.entities[&id].pos;
        let pos = Pos {
            x: pos.x + self.rng.gen_range(-2..2) as f32,
            y: pos.y + self.rng.gen_range(-2..2) as f32,
        };
        self.position_entity(id, pos)?;
        let radius = if kind == 0x1407 {
            1
        } else {
            self.rng.gen_range(2..4)
        } as f32;
        let color = self.rng.gen_range(0..0xffffff);
        let mut effect = Writer::new();
        effect
            .u8(5)
            .i32(id)
            .f32(radius)
            .f32(0.)
            .f32(0.)
            .f32(0.)
            .u32(color);
        let mut sound = Writer::new();
        sound.i32(id).u8(0);
        for p in self.players.values().filter(|p| p.known.contains(&id)) {
            send(p, wire::SHOW_EFFECT, effect.0.clone());
            if p.pos.distance(pos) <= 25. {
                send(p, wire::PLAY_SOUND, sound.0.clone());
            }
        }
        let targets: Vec<_> = self
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
        for target in &targets {
            self.damage_enemy(0, *target, 100, true, &[], None)?;
        }
        let pet = self.companions.active.get_mut(&id).unwrap();
        pet.remaining -= 1;
        pet.pulse = self.now
            + if kind == 0x1408 && pet.remaining == 9 {
                250
            } else {
                100
            };
        if pet.remaining == 0 {
            pet.until = Some(self.now + 1);
        }
        tracing::info!(
            companion = id,
            kind,
            x = pos.x,
            y = pos.y,
            radius,
            color,
            targets = targets.len(),
            remaining = pet.remaining,
            tick = self.now,
            "firecracker pulse"
        );
        Ok(())
    }
    pub(super) fn companion_step(&mut self) -> Result<()> {
        let ids: Vec<_> = self.companions.active.keys().copied().collect();
        for id in ids {
            let pet = &self.companions.active[&id];
            if !pet.published {
                continue;
            }
            let (owner, kind) = (pet.owner, pet.kind);
            if !self.entities.contains_key(&id)
                || pet.until.is_some_and(|until| until <= self.now)
                || self
                    .players
                    .get(&owner)
                    .is_none_or(|p| !p.active() || p.character.hp <= 0)
            {
                self.remove_companion(id);
                continue;
            }
            let owner = &self.players[&owner];
            if owner.paused {
                continue;
            }
            if matches!(kind, 0x1407 | 0x1408) {
                if pet.remaining > 0 && pet.pulse <= self.now {
                    self.firecracker_pulse(id, kind)?;
                }
                continue;
            }
            if kind == 0x140a {
                if pet.pulse <= self.now {
                    self.companions.active.get_mut(&id).unwrap().pulse = self.now + 1000;
                    let position = self.entities[&id].pos;
                    // Numeric rules are a conservative reconstruction; selected XML specifies nearby healing only.
                    let mut healed = Vec::new();
                    for (target, p) in &mut self.players {
                        if !p.active()
                            || p.paused
                            || p.pending
                            || p.death_started
                            || p.character.hp <= 0
                            || p.pos.distance(position) >= 3.
                            || p.conditions(self.now)
                                & crate::condition::bit(crate::condition::SICK)
                                != 0
                        {
                            continue;
                        }
                        let cap = p.effective(&self.content, self.now)?[0];
                        let amount = (cap - p.character.hp).clamp(0, 10);
                        p.character.hp += amount;
                        if amount > 0 {
                            healed.push((*target, amount));
                        }
                    }
                    for (target, amount) in healed {
                        self.consumable_restored(target, amount, 0xff00ff00);
                    }
                }
                continue;
            }
            let goal = owner.pos;
            let pos = self.entities[&id].pos;
            let distance = pos.distance(goal);
            if distance > 12. {
                self.position_entity(id, goal)?;
            } else if distance > 1. {
                let step = (distance - 1.).min(0.3);
                self.position_entity(
                    id,
                    Pos {
                        x: pos.x + (goal.x - pos.x) * step / distance,
                        y: pos.y + (goal.y - pos.y) * step / distance,
                    },
                )?;
            }
            let pet = self.companions.active.get_mut(&id).unwrap();
            if pet.permanent || pet.pulse > self.now {
                continue;
            }
            pet.pulse = self.now
                + match kind {
                    0x1400 | 0x1404 => 300,
                    0x1401 => 500,
                    0x1403 => 2000,
                    0x1402 | 0x1405 => 1000,
                    _ => bail!("unvalidated companion pulse {kind:#x}"),
                };
            let position = self.entities[&id].pos;
            if kind == 0x1401 {
                let mut target: Option<(i32, f32, i32)> = None;
                for (id, p) in &self.players {
                    let cap = p.effective(&self.content, self.now)?[0];
                    let distance = p.pos.distance(position);
                    if p.active()
                        && !p.paused
                        && p.character.hp > 0
                        && p.character.hp < cap
                        && distance <= 5.
                        && p.conditions(self.now) & crate::condition::bit(crate::condition::SICK)
                            == 0
                        && target.is_none_or(|(_, nearest, _)| distance < nearest)
                    {
                        target = Some((*id, distance, cap));
                    }
                }
                if let Some((target, _, cap)) = target {
                    let p = self.players.get_mut(&target).unwrap();
                    p.character.hp = (p.character.hp + 25).min(cap);
                    self.companion_effect(target, 1.);
                }
                continue;
            }
            let target = self
                .entities
                .values()
                .filter(|e| {
                    e.hp > 0
                        && self.content.objects[&e.kind].enemy
                        && e.pos.distance(position) <= 5.
                        && !crate::condition::protected(e.conditions(self.now))
                })
                .min_by(|a, b| {
                    a.pos
                        .distance(position)
                        .total_cmp(&b.pos.distance(position))
                })
                .map(|e| e.id);
            let Some(target) = target else {
                continue;
            };
            self.companion_effect(target, 1.);
            match kind {
                0x1400 => {
                    let center = self.entities[&target].pos;
                    let targets: Vec<_> = self
                        .entities
                        .values()
                        .filter(|e| {
                            e.hp > 0
                                && self.content.objects[&e.kind].enemy
                                && e.pos.distance(center) <= 1.
                        })
                        .map(|e| e.id)
                        .collect();
                    for target in targets {
                        // Period drake damage does not establish player loot or weapon credit.
                        self.damage_enemy(0, target, 35, false, &[], None)?;
                    }
                }
                0x1402 => self.apply_condition(target, crate::condition::PARALYZED, 5000)?,
                0x1403 => self.apply_condition(target, crate::condition::STASIS, 5000)?,
                0x1404 => self.apply_condition(target, crate::condition::SLOWED, 10000)?,
                0x1405 => {
                    self.apply_condition(target, crate::condition::BLEEDING, 10000)?;
                    self.companions
                        .bleeds
                        .entry(target)
                        .and_modify(|bleed| bleed.0 = self.now + 10000)
                        .or_insert((self.now + 10000, self.now + 1000));
                }
                _ => bail!("unvalidated companion behavior {kind:#x}"),
            }
        }
        let targets: Vec<_> = self.companions.bleeds.keys().copied().collect();
        for target in targets {
            let (until, next) = self.companions.bleeds[&target];
            if !self.entities.contains_key(&target) || self.now > until {
                self.companions.bleeds.remove(&target);
                continue;
            }
            if self.now >= next {
                self.companions.bleeds.get_mut(&target).unwrap().1 = self.now + 1000;
                self.damage_enemy(0, target, 30, true, &[], None)?;
            }
        }
        Ok(())
    }
}
