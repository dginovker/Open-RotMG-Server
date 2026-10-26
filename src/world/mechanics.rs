use super::*;
use crate::{condition, encounter};

pub(super) struct Grenade {
    kind: u16,
    position: Pos,
    radius: f32,
    damage: i32,
    deadline: u64,
}

impl World {
    pub(super) fn heal_enemy(&mut self, healer: i32, target: i32, amount: i32) -> Result<i32> {
        ensure!(amount > 0, "nonpositive enemy healing amount");
        let kind = self
            .entities
            .get(&target)
            .context("enemy heal target missing")?
            .kind;
        let maximum = self.content.get(kind)?.hp;
        ensure!(maximum > 0, "enemy heal target has no selected HP");
        let e = self.entities.get_mut(&target).unwrap();
        let before = e.hp;
        if e.conditions(self.now) & condition::bit(condition::SICK) == 0 {
            e.hp = (e.hp + amount).min(maximum);
        }
        let gained = e.hp - before;
        if gained > 0 {
            let pos = e.pos;
            let mut out = Writer::new();
            out.i32(target).utf(&format!("+{gained}")).u32(0xff00ff00);
            for p in self
                .players
                .values()
                .filter(|p| p.known.contains(&healer) && p.known.contains(&target))
            {
                send(p, wire::NOTIFICATION, out.0.clone());
                send(p, wire::SHOW_EFFECT, cooperative::effect(1, target, pos));
                send(p, wire::SHOW_EFFECT, cooperative::effect(7, healer, pos));
            }
            tracing::debug!(healer, target, amount = gained, "ordinary enemy healed");
        }
        Ok(gained)
    }
    pub(super) fn escort_anchor(
        &self,
        id: i32,
        kind: u16,
    ) -> Result<Option<(i32, encounter::Anchor)>> {
        let e = self.entities.get(&id).context("escort entity missing")?;
        if let Some(parent) = *e.parent {
            let boss = self
                .entities
                .get(&parent)
                .context("escort's owned boss disappeared")?;
            ensure!(
                boss.kind == kind,
                "escort belongs to a different boss identity"
            );
            return Ok(Some((parent, encounter::Anchor::Parent)));
        }
        let mut bosses = self.entities.values().filter(|e| e.kind == kind);
        let boss = bosses.next().map(|e| e.id);
        ensure!(
            bosses.next().is_none(),
            "map-born escort has ambiguous boss ownership"
        );
        Ok(boss.map(|id| (id, encounter::Anchor::Kind(kind))))
    }
    pub(super) fn orbit_anchor(
        &self,
        id: i32,
        anchor: encounter::Anchor,
        player: impl Into<Option<Pos>>,
    ) -> Result<Pos> {
        let e = self.entities.get(&id).context("orbiting entity missing")?;
        match anchor {
            encounter::Anchor::Player => player
                .into()
                .context("player-relative movement lacks a visible target"),
            encounter::Anchor::QuestMember(kind) => {
                let tag = e.quest.context("quest anchor lacks encounter ownership")?;
                self.actors()
                    .filter(|x| x.quest == Some(tag) && x.kind == kind)
                    .min_by(|a, b| a.pos.distance(e.pos).total_cmp(&b.pos.distance(e.pos)))
                    .map(|x| x.pos)
                    .context("own quest anchor absent")
            }
            encounter::Anchor::Home => Ok(e.home),
            encounter::Anchor::CampLeader => {
                let camp = self
                    .camps
                    .get(e.camp.context("entity has no family camp")?)
                    .context("family camp missing")?;
                let leader = self.population[camp.start]
                    .entity
                    .context("family leader slot is empty")?;
                Ok(self
                    .entities
                    .get(&leader)
                    .context("family leader is no longer alive")?
                    .pos)
            }
            encounter::Anchor::BoundEntity(member) => {
                ensure!(
                    e.tomb
                        .as_ref()
                        .is_some_and(|brain| brain.anchor == Some(member)
                            || brain.guard.is_some_and(|(caller, _)| caller == member)),
                    "Tomb anchor ownership mismatch"
                );
                Ok(self
                    .entities
                    .get(&member)
                    .context("bound Tomb anchor absent")?
                    .pos)
            }
            encounter::Anchor::CampEntity(member) => {
                let owner = self
                    .entities
                    .get(&member)
                    .context("owned camp anchor absent")?;
                ensure!(
                    e.camp.is_some() && owner.camp == e.camp,
                    "camp anchor ownership mismatch"
                );
                Ok(owner.pos)
            }
            encounter::Anchor::CampMember(kind) => Ok(self.entities[&self
                .camp_member(id, kind)?
                .context("own-camp orbit member absent")?]
                .pos),
            encounter::Anchor::Parent => Ok(self
                .entities
                .get(&e.parent.context("entity has no explicit parent")?)
                .context("entity parent no longer exists")?
                .pos),
            encounter::Anchor::Kind(kind) => self
                .entities
                .values()
                .filter(|candidate| candidate.id != id && candidate.kind == kind)
                .min_by(|a, b| e.pos.distance(a.pos).total_cmp(&e.pos.distance(b.pos)))
                .map(|e| e.pos)
                .context("selected orbit anchor absent"),
        }
    }

    pub(super) fn child_positions(&self, parent: i32, radius: f32) -> Result<Vec<Pos>> {
        let home = self
            .entities
            .get(&parent)
            .context("summon parent missing")?
            .pos;
        let positions: Vec<_> = (0..32)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / 32.;
                Pos {
                    x: home.x + radius * angle.cos(),
                    y: home.y + radius * angle.sin(),
                }
            })
            .filter(|pos| self.walkable(*pos))
            .collect();
        Ok(positions)
    }

    pub(super) fn summon_limit(&self) -> usize {
        if self.name == "Manor of the Immortals" {
            crate::dungeon::manor::SUMMON_LIMIT
        } else {
            256
        }
    }

    pub(super) fn summon_occupancy(&self) -> usize {
        self.actors()
            .filter(|e| e.parent.is_some() || e.quest.is_some())
            .count()
            + self.named_births.len()
            + self.manor_births.len()
            + self.abyss_pending_count()
    }

    pub(super) fn spawn_children(
        &mut self,
        parent: i32,
        kind: u16,
        maximum: usize,
        radius: f32,
    ) -> Result<()> {
        crate::encounter::validate(&self.content, kind)?;
        crate::loot::validate(&self.content, kind)?;
        let count = self
            .actors()
            .filter(|e| e.parent == Some(parent) && e.kind == kind)
            .count();
        let missing = maximum.saturating_sub(count);
        if missing == 0 {
            return Ok(());
        }
        let occupied = self.summon_occupancy();
        let limit = self.summon_limit();
        ensure!(
            occupied + missing <= limit,
            "summon budget exhausted: {occupied}+{missing} exceeds {limit}"
        );
        let mut positions = self.child_positions(parent, radius)?;
        if self.entities[&parent].kind == 0xd90 {
            // Reference Follow jitters each ghost; distinct starts prevent deterministic lockstep: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/behaviors/Follow.cs
            positions.retain(|pos| {
                !self
                    .actors()
                    .any(|e| e.parent == Some(parent) && pos.distance(e.pos) < 0.25)
            });
        }
        ensure!(
            positions.len() >= missing,
            "insufficient walkable summon positions around {parent}"
        );
        for i in 0..missing {
            let id = self.spawn_entity(kind, positions[i * positions.len() / missing], vec![])?;
            self.entities
                .get_mut(&id)
                .unwrap()
                .parent
                .set(Some(parent), id, &mut self.timed_ids);
        }
        tracing::info!(
            parent,
            kind,
            spawned = missing,
            maximum,
            "bounded encounter children summoned"
        );
        Ok(())
    }

    pub(super) fn encounter_mechanics(
        &mut self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<()> {
        let target = target.into();
        if matches!(
            self.entities[&id].kind,
            crate::dungeon::beach::BOSS | crate::dungeon::beach::BUM
        ) {
            return self.beach_mechanics(id);
        }
        if crate::dungeon::jungle::ROSTER.contains(&self.entities[&id].kind) {
            return self.jungle_mechanics(id, elapsed);
        }
        if crate::dungeon::manor::ROSTER.contains(&self.entities[&id].kind) {
            return self.manor_mechanics(id, target, elapsed);
        }
        if crate::dungeon::davy::profile(self.entities[&id].kind).is_some() {
            return self.locker_mechanics(id, target, elapsed);
        }
        if crate::dungeon::tomb::contains(self.entities[&id].kind) {
            return self.tomb_mechanics(id, target);
        }
        if matches!(
            self.entities[&id].kind,
            crate::dungeon::snake::BOSS
                | crate::dungeon::snake::GUARD
                | crate::dungeon::snake::BUTTON
                | crate::dungeon::snake::SPAWNER
                | crate::dungeon::snake::DART
        ) {
            return self.snake_mechanics(id, elapsed);
        }
        if crate::dungeon::lab::ROSTER.contains(&self.entities[&id].kind) {
            return self.lab_mechanics(id, target, elapsed);
        }
        if crate::dungeon::trench::ROSTER.contains(&self.entities[&id].kind) {
            return self.ocean_mechanics(id, target, elapsed);
        }
        if crate::dungeon::spider::ROSTER.contains(&self.entities[&id].kind) {
            return self.spider_mechanics(id, target, elapsed);
        }
        if crate::encounter::landmarks::ROSTER.contains(&self.entities[&id].kind) {
            return self.landmark_mechanics(id, target, elapsed);
        }
        if crate::dungeon::abyss::ROSTER.contains(&self.entities[&id].kind) {
            return self.abyss_mechanics(id, target, elapsed);
        }
        if self.entities[&id].quest.is_some() {
            return self.quest_mechanics(id);
        }
        if crate::endgame::ROSTER.contains(&self.entities[&id].kind) {
            return self.finale_mechanics(id, target, elapsed);
        }
        let kind = self.entities[&id].kind;
        if crate::encounter::fauna::ROSTER.contains(&kind) {
            return self.fauna_mechanics(id, target, elapsed);
        }
        if matches!(kind, 0xdaa | 0xdac) {
            return Ok(());
        }
        if matches!(kind, 0x62b | 0x900..=0x902) {
            return self.midland_mechanics(id);
        }
        if kind == 0x91a || super::gods::CONSTRUCTS.contains(&kind) {
            return self.god_mechanics(id, elapsed);
        }
        let e = &self.entities[&id];
        let (kind, hp, pos, home) = (e.kind, e.hp, e.pos, e.home);
        let stage = encounter::stage(kind, hp, elapsed);
        let changed = e.stage != Some(stage);
        if kind == 0xd06 {
            if stage % 5 == 4 && hp < self.content.objects[&kind].hp / 5 {
                let e = self.entities.get_mut(&id).unwrap();
                e.activated = Some(self.now);
                e.stage = None;
                return self.encounter_mechanics(id, target, 0);
            }
            let e = self.entities.get_mut(&id).unwrap();
            if stage % 5 != 2 {
                e.blocked_since = None;
                e.reaction_until = 0;
            } else if self.now >= e.reaction_until {
                if e.last_step.x.hypot(e.last_step.y) >= 0.001 {
                    e.blocked_since = None;
                } else if self.now >= *e.blocked_since.get_or_insert(self.now) + 500 {
                    e.blocked_since = None;
                    e.reaction_until = self.now + 1000;
                    tracing::debug!(
                        entity = id,
                        until = e.reaction_until,
                        "blocked Limon entered shielded burst"
                    );
                }
            }
        }
        let shield = match kind {
            0xd06 => {
                elapsed % 43300 < 2000
                    || self.entities[&id].reaction_until > self.now
                    || (12300..32300).contains(&(elapsed % 43300))
                        && (elapsed % 43300 - 12300) % 5000 >= 1500
            }
            0xd07..=0xd0a => true,
            0xd90 => elapsed % 13000 < 3000,
            _ => false,
        };
        let phase_bits =
            condition::bit(condition::INVULNERABLE) | condition::bit(condition::ARMORED);
        let e = self.entities.get_mut(&id).unwrap();
        if matches!(kind, 0xd06..=0xd0a | 0xd90) {
            e.condition &= !phase_bits;
            if shield {
                e.condition |= condition::bit(condition::INVULNERABLE);
            }
            if kind == 0xd06 && stage % 5 == 4 {
                e.condition |= condition::bit(condition::ARMORED);
            }
        }
        if changed {
            e.stage = Some(stage);
            tracing::debug!(entity = id, kind, stage, "encounter phase entered");
            match kind {
                0x614 if stage == 1 => {
                    let mut out = Writer::new();
                    out.u8(15)
                        .i32(id)
                        .f32(0.6)
                        .f32(5.)
                        .f32(0.)
                        .f32(0.)
                        .u32(0xff666666);
                    for p in self
                        .players
                        .values()
                        .filter(|p| p.known.contains(&id) && p.pos.distance(pos) < 20.)
                    {
                        send(p, wire::SHOW_EFFECT, out.0.clone());
                    }
                    tracing::debug!(entity = id, "Goblin horde commanded by wounded mage");
                }
                0xd06 if stage % 5 == 3 => {
                    // The wider arena now fits these box ranges: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.SpriteWorld.cs#L126-L134
                    for radius in [9.5, 14.] {
                        for (kind, degrees) in
                            [(0xd07, 315f32), (0xd08, 225.), (0xd09, 135.), (0xd0a, 45.)]
                        {
                            let angle = degrees.to_radians();
                            let corner = Pos {
                                x: home.x + radius * angle.cos(),
                                y: home.y + radius * angle.sin(),
                            };
                            let child = self.spawn_entity(kind, corner, vec![])?;
                            let e = self.entities.get_mut(&child).unwrap();
                            e.parent.set(Some(id), child, &mut self.timed_ids);
                            e.activated = Some(self.now);
                            e.expires
                                .set(Some(self.now + 20000), child, &mut self.timed_ids);
                            e.condition = condition::bit(condition::INVULNERABLE);
                        }
                    }
                }
                0xd90 if stage.is_multiple_of(4) => {
                    for (kind, count) in [
                        (0xdaa, 1),
                        (0xdaf, 2),
                        (0xdac, 1),
                        (0xdad, 2),
                        (0xdae, 1),
                        (0xdab, 2),
                    ] {
                        self.spawn_children(id, kind, count, 2.)?;
                    }
                }
                _ => {}
            }
        }
        if kind == 0xdae
            && self.entities[&id].conditions(self.now) & condition::bit(condition::STUNNED) != 0
        {
            return Ok(());
        }
        if self.now < self.entities[&id].next_aux {
            return Ok(());
        }
        self.entities.get_mut(&id).unwrap().next_aux = self.now
            + match kind {
                0x173 | 0xdae => 5000,
                0x654 => 3000,
                _ => 1000,
            };
        match kind {
            0x173 => {
                self.spawn_children(id, 0x223, 2, 1.5)?;
                self.spawn_children(id, 0x224, 2, 2.)?;
            }
            0x654 if target.is_some_and(|p| pos.distance(p) < 8.) => {
                self.throw_grenade(id, target.unwrap(), 4., 150)?
            }
            0xd06 if stage % 5 == 4 => {
                self.spawn_children(id, 0x625, 1, 2.)?;
                self.spawn_children(id, 0x626, 2, 3.)?;
            }
            0xd90 if stage % 4 == 3 && elapsed % 13000 >= 3000 => {
                for kind in [0xdb0, 0xdb1, 0xdb2] {
                    self.spawn_children(id, kind, 2, 2.)?;
                }
            }
            0xdae => {
                self.escort_anchor(id, 0xd90)?;
                let allies: Vec<_> = self
                    .actors()
                    .filter(|e| {
                        (0xdaa..=0xdaf).contains(&e.kind)
                            && pos.distance(e.pos) < 5.
                            && e.parent == self.entities[&id].parent
                    })
                    .map(|e| e.id)
                    .collect();
                for ally in allies {
                    let amount = self.content.objects[&self.entities[&ally].kind].hp;
                    self.heal_enemy(id, ally, amount)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn telegraph_throw(&self, owner: i32, position: Pos, color: u32) {
        let mut out = Writer::new();
        out.u8(4)
            .i32(owner)
            .f32(position.x)
            .f32(position.y)
            .f32(0.)
            .f32(0.)
            .u32(color);
        for p in self.players.values().filter(|p| p.known.contains(&owner)) {
            send(p, wire::SHOW_EFFECT, out.0.clone());
        }
    }
    pub(super) fn throw_grenade(
        &mut self,
        owner: i32,
        position: Pos,
        radius: f32,
        damage: i32,
    ) -> Result<()> {
        ensure!(self.grenades.len() < 128, "grenade budget exhausted");
        if self.entities[&owner].conditions(self.now) & condition::bit(condition::STUNNED) != 0 {
            return Ok(());
        }
        self.telegraph_throw(owner, position, 0xffff0000);
        self.grenades.push(Grenade {
            kind: self.entities[&owner].kind,
            position,
            radius,
            damage,
            deadline: self.now + 1500,
        });
        Ok(())
    }

    pub(super) fn area_damage(
        &mut self,
        kind: u16,
        position: Pos,
        radius: f32,
        amount: i32,
    ) -> Result<()> {
        let mut out = Writer::new();
        out.f32(position.x)
            .f32(position.y)
            .f32(radius)
            .i16(i16::try_from(amount)?)
            .u8(0)
            .f32(0.)
            .i16(kind as i16);
        let ids: Vec<_> = self.players.keys().copied().collect();
        for id in ids {
            let p = &self.players[&id];
            if !p.character.alive || p.death_pending || p.departing {
                continue;
            }
            if p.pos.distance(position) < radius + 20. {
                send(p, 68, out.0.clone());
            }
            if p.paused || p.pos.distance(position) >= radius {
                continue;
            }
            let damage =
                condition::damage(amount, self.defense(id)?, p.conditions(self.now), false);
            let p = self.players.get_mut(&id).unwrap();
            p.character.hp = (p.character.hp - damage).max(0);
            if p.character.hp == 0 {
                p.death_pending = true;
                p.killer = self.content.objects[&kind].name.clone();
                p.bot_goal = None;
                p.bot_aim = None;
            }
            tracing::debug!(
                player = id,
                kind,
                damage,
                "authoritative encounter area damage"
            );
        }
        Ok(())
    }

    pub(super) fn grenade_step(&mut self) {
        let grenades = std::mem::take(&mut self.grenades);
        for grenade in grenades {
            if grenade.deadline > self.now {
                self.grenades.push(grenade);
                continue;
            }
            if let Err(error) = self.area_damage(
                grenade.kind,
                grenade.position,
                grenade.radius,
                grenade.damage,
            ) {
                tracing::error!(%error, "grenade damage rejected");
            }
        }
    }

    pub(super) fn mechanic_expiry(&mut self) {
        self.abyss_terminal_step();
        self.fauna_birth_step();
        self.named_birth_step();
        self.manor_birth_step();
        let due = |e: &Entity| {
            e.expires.is_some_and(|deadline| deadline <= self.now)
                || (!matches!(
                    e.kind,
                    crate::dungeon::manor::ROCK | crate::dungeon::manor::MINE
                ) && e
                    .parent
                    .is_some_and(|parent| !self.entities.contains_key(&parent))
                    && !Self::abyss_orphan(e))
        };
        let expired: Vec<_> = self
            .timed_ids
            .iter()
            .filter(|id| {
                let e = &self.entities[id];
                (self.actor_ids.contains(id)
                    || self.content.objects[&e.kind].passive_destructible())
                    && due(e)
            })
            .copied()
            .collect();
        for id in expired {
            let e = &self.entities[&id];
            if e.kind == 0x908
                && e.parent
                    .is_some_and(|owner| !self.entities.contains_key(&owner))
                && !Self::abyss_orphan(e)
            {
                tracing::error!(entity=id, parent=?e.parent, time_ms=self.now,"Abyss Protector missing parent without validated orphan transition");
            }
            if self.entities[&id].kind == 0xe38 && matches!(self.entities[&id].stage, Some(2 | 4)) {
                if let Err(error) = self.ghost_ship_mechanics(id, None, 0) {
                    tracing::error!(entity=id,%error,"earned Locker portal deadline refused");
                }
            } else if matches!(
                self.entities[&id].kind,
                0x91a | 0xd50 | crate::dungeon::manor::CREATURE
            ) && self.entities[&id]
                .expires
                .is_some_and(|deadline| deadline <= self.now)
            {
                if let Err(error) = self.finish_timed_defeat(id) {
                    tracing::error!(entity=id,%error,"timed enemy defeat failed");
                }
            } else {
                self.remove_entity(id);
            }
        }
    }

    pub(super) fn encounter_death(&mut self, defeated: &Entity) -> Result<()> {
        if crate::dungeon::lab::ROSTER.contains(&defeated.kind) {
            return self.lab_death(defeated);
        }
        if defeated.kind == crate::dungeon::beach::BUM {
            self.broadcast_text("Oryx the Mad God", "");
        }
        if crate::dungeon::jungle::ROSTER.contains(&defeated.kind) {
            self.jungle_death(defeated);
            return Ok(());
        }
        if crate::dungeon::manor::ROSTER.contains(&defeated.kind) {
            return self.manor_death(defeated);
        }
        if defeated.kind == 0xe37 {
            self.ghost_ship_locker_reward(defeated)?;
        }
        if crate::dungeon::tomb::contains(defeated.kind) {
            return self.tomb_death(defeated);
        }
        if crate::dungeon::trench::ROSTER.contains(&defeated.kind) {
            return self.ocean_death(defeated);
        }
        if defeated.kind == crate::dungeon::spider::EGG {
            return self.hatch_spiders(defeated);
        }
        if crate::encounter::fauna::ROSTER.contains(&defeated.kind) {
            return self.fauna_death(defeated);
        }
        if defeated.kind == 0xd5e && defeated.quest.is_some() {
            return self.event_tower_death(defeated);
        }
        if defeated.kind == crate::dungeon::abyss::IDOL {
            self.idol_entrance(defeated.home, true)?;
        }
        if defeated.kind == 0xd54 {
            let children: Vec<_> = self
                .actors()
                .filter(|e| e.kind == 0xd55 && e.quest == defeated.quest)
                .map(|e| e.id)
                .collect();
            for child in children {
                self.remove_entity(child);
            }
        }
        if let Some(tag) = defeated.quest {
            if defeated.kind == 0x929 {
                let king = self
                    .actors()
                    .find(|e| e.quest == Some(tag) && e.kind == 0x928)
                    .map(|e| e.id);
                if let Some(king) = king {
                    self.quest_transform(king, 0x92d)?;
                }
            } else if defeated.kind == 0x92c
                && !self
                    .actors()
                    .any(|e| e.quest == Some(tag) && e.kind == 0x928)
            {
                let count = self.rng.gen_range(2..=3);
                self.quest_spawn_survivors(defeated, 0x669, count)?;
            }
            return Ok(());
        }
        if crate::endgame::ROSTER.contains(&defeated.kind) {
            return self.finale_death(defeated);
        }
        let (kind, count) = match defeated.kind {
            0xd9b | 0xd9c => (0xd9d, 4),
            0xd9e => (0xd9f, 6),
            _ => return Ok(()),
        };
        crate::encounter::validate(&self.content, kind)?;
        crate::loot::validate(&self.content, kind)?;
        ensure!(
            self.actors()
                .filter(|e| self.content.objects[&e.kind].enemy)
                .count()
                + count
                <= 512,
            "slime split budget exhausted"
        );
        for i in 0..count {
            let angle = i as f32 * std::f32::consts::TAU / count as f32;
            let pos = Pos {
                x: defeated.pos.x + 0.2 * angle.cos(),
                y: defeated.pos.y + 0.2 * angle.sin(),
            };
            self.spawn_entity(kind, pos, vec![])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limon_box_uses_reference_corner_ranges_and_keeps_lifecycle() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        for seed in 0..32 {
            let map = crate::dungeon::sprite::generate(&content, seed).unwrap();
            let mut world = World::new(content.clone(), Arc::new(map)).unwrap();
            let boss = world
                .entities
                .values()
                .find(|e| e.kind == 0xd06)
                .unwrap()
                .id;
            let home = world.entities[&boss].home;
            world.now = 12300;
            world.encounter_mechanics(boss, None, 12300).unwrap();
            let turrets: Vec<_> = world
                .entities
                .values()
                .filter(|e| e.parent == Some(boss) && matches!(e.kind, 0xd07..=0xd0a))
                .map(|e| e.id)
                .collect();
            assert_eq!(turrets.len(), 8, "seed {seed}");
            for radius in [9.5, 14.] {
                let offset = radius / std::f32::consts::SQRT_2;
                for (kind, x, y) in [
                    (0xd07, 1., -1.),
                    (0xd08, -1., -1.),
                    (0xd09, -1., 1.),
                    (0xd0a, 1., 1.),
                ] {
                    let expected = Pos {
                        x: home.x + x * offset,
                        y: home.y + y * offset,
                    };
                    let child = turrets
                        .iter()
                        .map(|id| &world.entities[id])
                        .find(|e| e.kind == kind && (e.pos.distance(home) - radius).abs() < 0.001)
                        .unwrap_or_else(|| {
                            panic!("seed {seed}: missing {kind:#x} corner at radius {radius}")
                        });
                    assert!(child.pos.distance(expected) < 0.001);
                    assert_eq!(child.hp, 100000);
                    assert!(child.expires.is_some_and(|t| t == 32300));
                    assert_ne!(child.condition & condition::bit(condition::INVULNERABLE), 0);
                }
            }
            world.now = 12400;
            world.encounter_mechanics(boss, None, 12400).unwrap();
            assert_eq!(
                world
                    .entities
                    .values()
                    .filter(|e| e.parent == Some(boss))
                    .count(),
                8
            );
            assert_eq!(world.entities[&boss].hp, 9000);
            assert!(world.entities[&boss].home.distance(home) < 0.001);
            if seed % 2 == 0 {
                world.now = 32300;
            } else {
                world.remove_entity(boss);
            }
            world.mechanic_expiry();
            assert!(turrets.iter().all(|id| !world.entities.contains_key(id)));
        }
    }
}
