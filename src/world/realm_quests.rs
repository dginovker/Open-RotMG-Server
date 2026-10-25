// Behavior reference (GPL-3.0; data/REALM-SRC-LICENSE): https://github.com/moistosaurus/realm-src/tree/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db
// EntAncient/Lich/GhostKing/Phoenix/Oasis contracts, credited upstream to NR-Core and Skilly; Rust reconstruction, not official AI.
// Stable encounter identity survives transformations; summon parenthood would delete surviving followers.
use super::*;
use crate::{
    condition,
    encounter::{Anchor, Motion},
    quest,
};

impl World {
    pub(super) fn populate_authored_groves(
        &mut self,
        groves: &[crate::terrain::realms::Grove],
    ) -> Result<()> {
        use realm_setpieces::Area;
        let mut areas = Vec::new();
        for grove in groves {
            let [x, y, width, height] = grove.bounds;
            let area = Area {
                x,
                y,
                width,
                height,
            };
            let pos = Pos {
                x: grove.spawn[0] as f32,
                y: grove.spawn[1] as f32,
            };
            ensure!(
                x > 0
                    && y > 0
                    && width > 0
                    && height > 0
                    && x.checked_add(width).is_some_and(|v| v < self.map.width)
                    && y.checked_add(height).is_some_and(|v| v < self.map.height),
                "invalid authored Grove bounds {:?} in {:?}",
                grove.bounds,
                self.realm_layout
            );
            ensure!(
                pos.x >= x as f32
                    && pos.x < (x + width) as f32
                    && pos.y >= y as f32
                    && pos.y < (y + height) as f32
                    && self.walkable(pos)
                    && self
                        .map
                        .cell(pos.x, pos.y)
                        .is_some_and(|c| c.ground == 0x46 && [6, 7].contains(&c.terrain)),
                "blocked or invalid authored Grove anchor {:?} in {:?}",
                grove.spawn,
                self.realm_layout
            );
            ensure!(
                areas
                    .iter()
                    .chain(&self.setpieces)
                    .all(|a| area.separate(a)),
                "overlapping authored Grove {:?} in {:?}",
                grove.bounds,
                self.realm_layout
            );
            let trees = (y..y + height)
                .flat_map(|yy| (x..x + width).map(move |xx| (xx, yy)))
                .filter(|(xx, yy)| {
                    self.map.cells[(yy * self.map.width + xx) as usize].object == Some(0x194)
                })
                .count();
            ensure!(
                trees == grove.trees && trees > 0,
                "authored Grove {:?} in {:?}: expected {} cherry trees, found {trees}",
                grove.bounds,
                self.realm_layout,
                grove.trees
            );
            areas.push(area);
        }
        // Grove binds one Ent to its foliage; these hash-pinned image-guided anchors are engineered, not recovered coordinates: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/setpieces/Grove.cs
        for (grove, area) in groves.iter().zip(areas) {
            self.setpieces.push(area);
            let pos = Pos {
                x: grove.spawn[0] as f32,
                y: grove.spawn[1] as f32,
            };
            let id = self.spawn_entity(0x91f, pos, vec![])?;
            let e = self.entities.get_mut(&id).unwrap();
            e.quest = Some(id);
            e.stage = Some(0);
            e.size = 140;
            tracing::info!(
                layout = self.realm_layout,
                entity = id,
                x = pos.x,
                y = pos.y,
                "finite authored Grove Ent placed"
            );
        }
        Ok(())
    }

    pub(super) fn populate_named_quests(&mut self, authored_groves: bool) -> Result<()> {
        for kind in quest::ROSTER {
            quest::validate(&self.content, *kind)?;
            crate::loot::validate(&self.content, *kind)?;
        }
        for kind in 0x678..=0x67c {
            crate::encounter::validate(&self.content, kind)?;
            crate::loot::validate(&self.content, kind)?;
        }
        // Unlike the reference's 50 random tries, placement here realizes every request or errors.
        // GPL-3.0 requested site counts, not authenticated 123.5.1 population density:
        // https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/setpieces/SetPieces.cs
        for (kind, terrains, count) in [
            (0x91f, &[7, 6][..], 17..25),
            (0x91b, &[6, 7][..], 4..7),
            (0x928, &[4, 3][..], 8..15),
            (0x675, &[5, 2][..], 0..5),
            (0x678, &[8, 5][..], 0..5),
        ]
        .into_iter()
        .skip(usize::from(authored_groves))
        {
            let count = self.rng.gen_range(count);
            self.populate_named_sites(kind, terrains, count)?;
        }
        Ok(())
    }
    fn populate_named_sites(&mut self, kind: u16, terrains: &[u8], count: i32) -> Result<()> {
        ensure!(count >= 0, "negative named quest-site count");
        for _ in 0..count {
            let pos = self.place_named_setpiece(kind, terrains)?;
            let id = self.spawn_entity(kind, pos, vec![])?;
            let e = self.entities.get_mut(&id).unwrap();
            e.quest = Some(id);
            e.stage = (kind != 0x678).then_some(0);
            e.size = self.content.objects[&kind].size;
            tracing::info!(
                entity = id,
                kind,
                terrain = self.map.cell(pos.x, pos.y).unwrap().terrain,
                "finite named Realm quest placed"
            );
        }
        Ok(())
    }
    pub(super) fn quest_members(&self, id: i32, kinds: &[u16]) -> Vec<i32> {
        let tag = self.entities[&id].quest.expect("named quest membership");
        self.non_population_actors()
            .filter(|e| e.quest == Some(tag) && kinds.contains(&e.kind))
            .map(|e| e.id)
            .collect()
    }
    fn quest_leader(&self, id: i32) -> Option<i32> {
        self.quest_members(
            id,
            &[
                0x91f, 0x920, 0x91b, 0x91c, 0x928, 0x92d, 0x675, 0x6c2, 0x6c1,
            ],
        )
        .into_iter()
        .next()
    }
    fn quest_phase(&mut self, id: i32, stage: u64) {
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(stage);
        e.activated = Some(self.now);
        e.next_shot = self.now;
        e.next_aux = self.now;
        tracing::debug!(
            entity = id,
            kind = e.kind,
            quest = e.quest,
            stage,
            "named quest phase entered"
        );
        let text = match (e.kind, stage) {
            (0x91f, 1) => "Uhh. So... sleepy...",
            (0x91b, 1) => "New recruits for my undead army? How delightful!",
            (0x928, 1) => "No corporeal creature can kill my sorrow",
            (0x928, 7) => "I cannot be defeated while my loyal subjects sustain me!",
            (0x929, 3) => "Your secret soul master is dying, Your Majesty",
            (0x6c1, 0) => "Some die and are ashes, but I am ever reborn!",
            _ => return,
        };
        let packet = chat::text_packet(
            &format!("#{}", self.content.objects[&e.kind].name),
            id,
            5,
            text,
        );
        for p in self.players.values().filter(|p| p.known.contains(&id)) {
            send(p, packet.id, packet.body.clone());
        }
    }
    pub(super) fn quest_shield(&mut self, id: i32, shield: bool) {
        let e = self.entities.get_mut(&id).unwrap();
        e.condition &= !condition::bit(condition::INVULNERABLE);
        if shield {
            e.condition |= condition::bit(condition::INVULNERABLE);
        }
    }
    fn quest_spawn(
        &mut self,
        id: i32,
        kind: u16,
        maximum: usize,
        count: usize,
        radius: f32,
    ) -> Result<()> {
        let e = &self.entities[&id];
        self.quest_birth(
            (
                e.quest.context("quest summon has no encounter identity")?,
                e.home,
                e.pos,
            ),
            kind,
            maximum,
            count,
            radius,
        )
    }
    pub(super) fn quest_birth(
        &mut self,
        owner: (i32, Pos, Pos),
        kind: u16,
        maximum: usize,
        count: usize,
        radius: f32,
    ) -> Result<()> {
        crate::encounter::validate(&self.content, kind)?;
        let (tag, home, center) = owner;
        let own =
            self.actors().filter(|e| e.quest == Some(tag)).count() + self.pending_named(tag, None);
        let live = self
            .actors()
            .filter(|e| e.quest == Some(tag) && e.kind == kind)
            .count()
            + self.pending_named(tag, Some(kind));
        let count = count.min(maximum.saturating_sub(live));
        if count == 0 {
            return Ok(());
        }
        if own + count > 32 {
            tracing::debug!(
                quest = tag,
                kind,
                own,
                count,
                "named quest birth refused at encounter capacity"
            );
            return Ok(());
        }
        ensure!(
            self.actors()
                .filter(|e| e.parent.is_some() || e.quest.is_some())
                .count()
                + self.named_births.len()
                + self.abyss_pending_count()
                + count
                <= 256,
            "shared summon budget exhausted"
        );
        let mut positions = Vec::new();
        for n in 0..128 {
            let a = n as f32 * std::f32::consts::TAU / 32.;
            let r = radius * (1. + (n / 32) as f32 * 0.2);
            let p = Pos {
                x: center.x + r * a.cos(),
                y: center.y + r * a.sin(),
            };
            if self.walkable(p) && positions.iter().all(|p0: &Pos| p.distance(*p0) >= 0.5) {
                positions.push(p);
            }
        }
        ensure!(
            positions.len() >= count,
            "quest summon has insufficient walkable positions"
        );
        for n in 0..count {
            let p = positions[n * positions.len() / count];
            let child = self.spawn_entity(kind, p, vec![])?;
            let e = self.entities.get_mut(&child).unwrap();
            e.quest = Some(tag);
            e.home = home;
            e.stage = Some(0);
            e.activated = Some(self.now);
            let lifetime = match kind {
                // The Master's Decay is ordered by the King, not a lifetime: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.GhostKing.cs#L165-L240
                0x929 => None,
                0x91d => Some(130000),
                0x92a..=0x92c => Some(160000),
                _ => Some(90000),
            };
            e.expires
                .set(lifetime.map(|ms| self.now + ms), child, &mut self.timed_ids);
            e.size = self.content.objects[&kind].size;
            if matches!(kind, 0x91e | 0x923 | 0x929..=0x92c) {
                e.condition |= condition::bit(condition::INVULNERABLE);
            }
        }
        tracing::debug!(
            quest = tag,
            kind,
            count,
            maximum,
            "bounded named quest followers spawned"
        );
        Ok(())
    }
    pub(super) fn quest_transform(&mut self, id: i32, kind: u16) -> Result<i32> {
        quest::validate(&self.content, kind)?;
        let old = &self.entities[&id];
        ensure!(old.quest.is_some(), "quest transformation lacks ownership");
        let new = self.spawn_entity(kind, old.pos, vec![])?;
        let mut old = self.remove_entity(id);
        let e = self.entities.get_mut(&new).unwrap();
        e.quest = old.quest;
        e.home = old.home;
        e.damage = std::mem::take(&mut old.damage);
        e.stage = None;
        e.activated = Some(self.now);
        let expires = match kind {
            _ if quest::final_root(kind) || kind == 0x6c2 => None,
            0x924 => Some(self.now + 60000),
            0x92b | 0x92c => Some(self.now + 160000),
            _ => *old.expires,
        };
        e.expires.set(expires, new, &mut self.timed_ids);
        e.size = self.content.objects[&kind].size;
        if matches!(kind, 0x920 | 0x6c2 | 0x92b | 0x92c) {
            e.condition |= condition::bit(condition::INVULNERABLE);
        }
        tracing::info!(
            entity = id,
            replacement = new,
            kind,
            quest = e.quest,
            "named quest transformed without rewards"
        );
        Ok(new)
    }
    pub(super) fn quest_spawn_survivors(
        &mut self,
        defeated: &Entity,
        kind: u16,
        count: usize,
    ) -> Result<()> {
        let tag = defeated.quest.context("quest survivor ownership absent")?;
        self.quest_birth((tag, defeated.home, defeated.pos), kind, 24, count, 0.5)
    }
    pub(super) fn quest_lethal_transform(&mut self, id: i32) -> Result<bool> {
        let e = &self.entities[&id];
        if e.quest.is_none() {
            return Ok(false);
        }
        let kind = match e.kind {
            0x675 => 0x6c2,
            0x6c2 => 0x6c1,
            0x92a => 0x92b,
            0x92b => 0x92c,
            _ => return Ok(false),
        };
        self.quest_transform(id, kind)?;
        Ok(true)
    }
    fn quest_tier(&self, id: i32) -> u64 {
        let e = &self.entities[&id];
        let lost = self.content.objects[&e.kind].hp - e.hp;
        if lost > 37500 {
            3
        } else if lost > 14400 {
            2
        } else if lost > 4500 {
            1
        } else {
            0
        }
    }
    pub(super) fn quest_mechanics(&mut self, id: i32) -> Result<()> {
        if (0xd56..=0xd60).contains(&self.entities[&id].kind) {
            return self.event_mechanics(id);
        }
        if self.entities[&id].stage.is_none() {
            self.quest_phase(id, 0);
        }
        let e = &self.entities[&id];
        let (kind, hp, stage, t) = (
            e.kind,
            e.hp,
            e.stage.unwrap_or(0),
            self.now - e.activated.unwrap(),
        );
        if matches!(kind, 0x91b | 0x91f | 0x928) {
            match stage {
                0 if hp < self.content.objects[&kind].hp => {
                    self.quest_phase(id, 1);
                    self.quest_shield(id, true);
                }
                1 if t >= 2500 => {
                    self.quest_phase(id, 2);
                    self.quest_shield(id, false);
                }
                2 if t >= 6000 || hp < 261000 => {
                    let tier = self.quest_tier(id);
                    self.quest_phase(id, 3 + tier);
                    self.quest_shield(id, kind != 0x91b);
                    match kind {
                        0x91f => {
                            for (k, n) in [
                                (0x923, usize::from(tier == 1)),
                                (0x921, [0, 2, 5, 11][tier as usize]),
                            ] {
                                self.quest_spawn(id, k, n, n, 3.)?;
                            }
                        }
                        0x91b => {
                            let n = [0, 2, 3, 5][tier as usize];
                            self.quest_spawn(id, 0x91d, 8, n, 5.5)?;
                        }
                        0x928 => self.ghost_wave(id, tier, false)?,
                        _ => unreachable!(),
                    }
                }
                3..=6 => {
                    let tier = (stage - 3) as usize;
                    if kind == 0x91f {
                        if t >= if tier < 2 { 3000 } else { 5000 } {
                            self.quest_transform(id, 0x920)?;
                        } else if tier >= 2
                            && t > 0
                            && t.is_multiple_of(if tier == 3 { 400 } else { 1000 })
                        {
                            self.quest_spawn(id, 0x923, if tier == 3 { 6 } else { 3 }, 1, 3.)?;
                        }
                    } else if kind == 0x91b {
                        let delay = [3000, 20000, 27000, 30000][tier];
                        if t >= delay {
                            self.quest_transform(id, 0x91c)?;
                        } else {
                            if t == [0, 15000, 22000, 25000][tier] && tier >= 2 {
                                self.quest_spawn(id, 0x91d, 8, 3, 5.5)?;
                            }
                            let (max, interval) = match tier {
                                1 if t >= 15000 => (1, 9000),
                                2 => (2, 2000),
                                3 => (5, 3000),
                                _ => (0, 1),
                            };
                            if max > 0
                                && t > 0
                                && (if tier == 1 { t - 15000 } else { t }).is_multiple_of(interval)
                            {
                                self.quest_spawn(id, 0x91e, max, 1, 3.)?;
                            }
                        }
                    } else if kind == 0x928 {
                        let delay = [1000, 32000, 32000, 62000][tier];
                        if tier == 3 && t == 30000 {
                            self.ghost_wave(id, 3, false)?;
                        }
                        if tier > 0 && t == delay - 2000 {
                            self.ghost_wave(id, tier as u64, true)?;
                        }
                        if t >= delay {
                            self.quest_phase(id, 7);
                        }
                    }
                }
                7 if kind == 0x928 && t >= 140000 => {
                    let pos = self.entities[&id].pos;
                    for ghost in self.quest_members(id, &[0x929, 0x92a, 0x92b, 0x92c]) {
                        if self.entities[&ghost].pos.distance(pos) <= 30. {
                            self.remove_entity(ghost);
                        }
                    }
                    self.quest_transform(id, 0x92d)?;
                }
                _ => {}
            }
            return Ok(());
        }
        match kind {
            0x920 => {
                // The readable reference's timed transitions are reachable while growth is invulnerable; HP triggers are not.
                let phase = (t / 1600).min(9);
                let changed = e.stage != Some(phase);
                if changed {
                    self.entities.get_mut(&id).unwrap().stage = Some(phase);
                    tracing::debug!(entity = id, phase, "Ent growth entered");
                }
                self.entities.get_mut(&id).unwrap().size = 160 + phase as i32 * 20;
                if phase == 9 && changed {
                    for sprite in self.quest_members(id, &[0x923]) {
                        self.quest_transform(sprite, 0x924)?;
                    }
                    if !self.quest_members(id, &[0x924]).is_empty() {
                        self.entities.get_mut(&id).unwrap().condition |=
                            condition::bit(condition::ARMORED);
                    }
                }
                self.quest_shield(id, t < 16400 || (t - 16400) % 8000 < 4000);
                if t > 0 && t.is_multiple_of(3000) {
                    self.quest_spawn(id, 0x922, 3, 1, 2.)?;
                }
            }
            0x91c => {
                if t == 0 || t.is_multiple_of(4000) {
                    for (k, n, initial) in [(0x65f, 4, 2), (0x660, 2, 1), (0x661, 1, 0)] {
                        self.quest_spawn(id, k, n, if t == 0 { initial } else { 1 }, 2.)?;
                    }
                }
            }
            0x91d | 0x924 => {
                let shield = kind == 0x91d && hp < 1800 && t % 5000 < 2500;
                self.quest_shield(id, shield);
                if t.is_multiple_of(200)
                    && self.entities[&id].conditions(self.now) & condition::bit(condition::STUNNED)
                        == 0
                    && let Some(leader) = self.quest_leader(id).filter(|leader| {
                        self.entities[leader].pos.distance(self.entities[&id].pos) < 15.
                    })
                {
                    self.heal_enemy(
                        id,
                        leader,
                        self.content.objects[&self.entities[&leader].kind].hp,
                    )?;
                }
                if kind == 0x924
                    && t > 0
                    && t.is_multiple_of(3000)
                    && self.quest_leader(id).is_some()
                {
                    self.quest_spawn(id, 0x921, 23, 1, 2.)?;
                }
            }
            0x91e => self.quest_shield(id, t % 10000 < 7000),
            0x923 => self.quest_shield(id, true),
            0x929 => {
                if hp < 94000 {
                    if stage != 3 {
                        self.quest_phase(id, 3);
                        for king in self.quest_members(id, &[0x928]) {
                            self.quest_transform(king, 0x92d)?;
                        }
                    } else if t >= 3000 {
                        self.remove_entity(id);
                    }
                } else {
                    let next = if hp < 98000 {
                        2
                    } else if hp < 99000 {
                        1
                    } else {
                        0
                    };
                    let t = if next != stage {
                        self.quest_phase(id, next);
                        0
                    } else {
                        t
                    };
                    let intro = u64::from(next > 0) * 1000;
                    self.quest_shield(
                        id,
                        t < intro || (t - intro) % [10000, 7000, 4000][next as usize] < 1000,
                    );
                }
            }
            0x92a..=0x92c => {
                let period = match kind {
                    0x92a => 10000,
                    0x92b => 7000,
                    _ => 4000,
                };
                let intro = u64::from(kind != 0x92a) * 1000;
                self.quest_shield(id, t < intro || (t - intro) % period < 1000);
            }
            0x675 | 0x6c1 => {
                if t == 0 {
                    self.quest_spawn(id, 0x676, 8, if kind == 0x675 { 4 } else { 1 }, 2.)?;
                    self.quest_spawn(id, 0x677, 8, if kind == 0x675 { 4 } else { 1 }, 2.5)?;
                }
                let interval = if kind == 0x675 { 5000 } else { 1000 };
                let births = if kind == 0x675 { 8 } else { 2 };
                if t > 0 && t % interval == 0 && t / interval <= births {
                    let k = if (t / interval) % 2 == 0 {
                        0x676
                    } else {
                        0x677
                    };
                    self.quest_spawn(id, k, 8, 1, 2.)?;
                }
            }
            0x6c2 => {
                self.quest_shield(id, t < 2000);
                self.entities.get_mut(&id).unwrap().size =
                    if (t / 800) % 2 == 0 { 150 } else { 130 };
            }
            _ => {}
        }
        Ok(())
    }
    fn ghost_wave(&mut self, id: i32, tier: u64, company: bool) -> Result<()> {
        let mut kinds = Vec::new();
        if company || tier == 0 {
            kinds.push(0x929);
        }
        let (small, medium, large) = if tier == 0 {
            (4, 0, 0)
        } else if company {
            if tier == 1 { (2, 3, 0) } else { (0, 2, 3) }
        } else if tier == 1 {
            (3, 3, 0)
        } else {
            (3, 0, 3)
        };
        for (kind, count) in [(0x92a, small), (0x92b, medium), (0x92c, large)] {
            kinds.extend(std::iter::repeat_n(kind, count));
        }
        let center = self.entities[&id].pos;
        // TossEnemy delays birth until the 1500ms throw lands: https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/attack/TossEnemy.cs
        for (i, &kind) in kinds.iter().enumerate() {
            let angle = i as f32 * std::f32::consts::TAU / kinds.len() as f32;
            let position = Pos {
                x: center.x + 4. * angle.cos(),
                y: center.y + 4. * angle.sin(),
            };
            self.named_toss(id, kind, if kind == 0x929 { 1 } else { 24 }, &[position])?;
        }
        Ok(())
    }
    pub(super) fn quest_movement(
        &self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> (Motion, f32) {
        if (0xd56..=0xd60).contains(&self.entities[&id].kind) {
            return self.event_movement(id, elapsed);
        }
        let target = target.into();
        let e = &self.entities[&id];
        let leash = match e.kind {
            0x91f | 0x920 => 6.,
            0x91b | 0x91c | 0x91d | 0x6c1 => 5.,
            0x923 | 0x924 | 0x91e => 11.,
            0x929..=0x92c => 7.,
            0x675 => 2.,
            0x928 if e.stage.unwrap_or(0) >= 7 => 8.,
            0x928 => 3.,
            _ => 16.,
        };
        if e.pos.distance(e.home) > leash {
            return (
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 1.,
                },
                12.,
            );
        }
        let (motion, rate_tenths) = match e.kind {
            0x6c2 => (Motion::Stationary, 0.),
            0x92d => (Motion::Pace { distance: 3. }, 5.),
            0x928 if e.stage == Some(0) => (Motion::Pace { distance: 3. }, 3.),
            0x928 if e.stage.unwrap_or(0) >= 7 => (
                Motion::Follow {
                    sight: 10.,
                    stop: 2.,
                    idle: 0.,
                    duration: 2000,
                    cooldown: 2000,
                },
                6.,
            ),
            0x91c => {
                if let Some(bearer) = self.quest_members(id, &[0x91d]).first()
                    && e.pos.distance(self.entities[bearer].pos) > 2.
                {
                    (
                        Motion::Protect {
                            anchor: Anchor::QuestMember(self.entities[bearer].kind),
                            distance: 2.,
                        },
                        9.,
                    )
                } else {
                    (Motion::Wander, 5.)
                }
            }
            0x676 | 0x677 | 0x65f..=0x661 => {
                let protection = if e.kind == 0x677 { 11. } else { 10. };
                let birdman = matches!(e.kind, 0x676 | 0x677);
                if let Some(leader) = self.quest_leader(id)
                    && e.pos.distance(self.entities[&leader].pos) > protection
                {
                    (
                        Motion::Protect {
                            anchor: Anchor::QuestMember(self.entities[&leader].kind),
                            distance: protection,
                        },
                        if birdman { 5. } else { 10. },
                    )
                } else if target.is_some_and(|p| {
                    e.pos.distance(p) < 10.
                        && e.pos.distance(p) > if e.kind == 0x676 { 9. } else { 7. }
                }) {
                    (
                        Motion::Chase {
                            sight: 10.,
                            stop: if e.kind == 0x676 { 9. } else { 7. },
                        },
                        if birdman { 10. } else { 12. },
                    )
                } else {
                    (Motion::Wander, if birdman { 5. } else { 4. })
                }
            }
            0x921 | 0x922 => {
                let (distance, speed) = if e.kind == 0x921 {
                    (7., 2.5)
                } else {
                    (4., 5.5)
                };
                if let Some(leader) = self.quest_leader(id)
                    && e.pos.distance(self.entities[&leader].pos) > distance
                {
                    (
                        Motion::Protect {
                            anchor: Anchor::QuestMember(self.entities[&leader].kind),
                            distance,
                        },
                        speed,
                    )
                } else if e.kind == 0x921 {
                    (
                        Motion::Chase {
                            sight: 9.,
                            stop: 1.,
                        },
                        speed,
                    )
                } else {
                    (Motion::Wander, speed)
                }
            }
            0x923 | 0x924 if target.is_some_and(|p| e.pos.distance(p) < 7.) => (
                Motion::Orbit {
                    anchor: Anchor::Player,
                    radius: 4.,
                },
                15.,
            ),
            0x923 | 0x924 => (Motion::Stationary, 0.),
            0x91d if e.hp < 1800 => (
                Motion::Wander,
                if elapsed % 5000 < 2500 { 13. } else { 10. },
            ),
            0x91d if elapsed % 3000 < 1500 => {
                let (speed, sight) = if e.hp >= 3900 { (5.5, 5.) } else { (6.5, 10.) };
                if target.is_some_and(|p| e.pos.distance(p) < sight) {
                    (
                        Motion::Orbit {
                            anchor: Anchor::Player,
                            radius: 4.,
                        },
                        speed,
                    )
                } else {
                    (Motion::Stationary, 0.)
                }
            }
            0x91d if target.is_some_and(|p| e.pos.distance(p) < 2.) => (
                Motion::Retreat { distance: 2. },
                if e.hp >= 3900 { 5.5 } else { 6.5 },
            ),
            0x91d => (Motion::Wander, if e.hp >= 3900 { 5.5 } else { 6.5 }),
            0x91e if elapsed % 10000 >= 7000 => (Motion::Stationary, 0.),
            0x91e => (Motion::Wander, 10.),
            0x929..=0x92c
                if self.entities[&id].condition & condition::bit(condition::INVULNERABLE) == 0 =>
            {
                (Motion::Stationary, 0.)
            }
            0x929..=0x92c => (Motion::Wander, 20.),
            0x920 => (Motion::Wander, 2.),
            0x6c1 => (Motion::Wander, 9.),
            0x675 => (Motion::Wander, 4.),
            0x91f if e.stage == Some(0) => (Motion::Wander, 1.),
            _ => (Motion::Wander, 4.),
        };
        // NR-Core adds a base motor rate; a linear speed conversion loses it on slow quest bosses.
        (
            motion,
            if rate_tenths == 0. {
                0.
            } else {
                (5.55 * rate_tenths / 10. + 0.74) * 1.5
            },
        )
    }
    pub(super) fn quest_shots(&self, id: i32, target: Pos, t: u64) -> Result<Vec<(u8, f32)>> {
        if (0xd56..=0xd60).contains(&self.entities[&id].kind) {
            return self.event_shots(id, target, t);
        }
        let e = &self.entities[&id];
        let k = e.kind;
        let s = e.stage.unwrap_or(0);
        let heading = (target.y - e.pos.y).atan2(target.x - e.pos.x);
        let ring = |index, n, offset| quest::ring(index, n, offset);
        let fan = |index, n, angle| quest::fan(index, n, angle, heading);
        let mut shots = match k {
            0x91f | 0x91b | 0x928 if s == 2 => match k {
                0x91f if t.is_multiple_of(800) => ring(0, 2, heading),
                0x928 if t.is_multiple_of(1000) => fan(0, 4, 30.),
                0x91b if [200, 400, 2200, 2400, 4200, 4400].contains(&t) => ring(1, 3, heading),
                _ => vec![],
            },
            0x928 if s >= 7 && t.is_multiple_of(1000) => fan(0, 1, 0.),
            0x920 => {
                let stage = (t / 1600).min(9) as u8;
                let mut shots = if t.is_multiple_of(1000) {
                    if matches!(stage, 1 | 4 | 7) {
                        ring(stage, 3, heading)
                    } else {
                        fan(stage, 1, 0.)
                    }
                } else {
                    vec![]
                };
                if stage == 9
                    && e.condition & condition::bit(condition::INVULNERABLE) == 0
                    && t.is_multiple_of(700)
                {
                    shots.extend(fan(3, 3, 120.));
                }
                shots
            }
            0x91c => {
                let mut shots = if t.is_multiple_of(800) {
                    fan(0, 2, 7.)
                } else {
                    vec![]
                };
                if t % 14000 >= 8000 && t.is_multiple_of(1000) {
                    shots.extend(fan(1, 4, 7.));
                }
                shots
            }
            0x91d if e.hp >= 3900 && t % 900 == 400 => ring(0, 3, heading),
            0x91d if (1800..3900).contains(&e.hp) && t % 600 == 200 => fan(0, 3, 15.),
            0x91d if e.hp < 1800 && t.is_multiple_of(800) => fan(1, 1, 0.),
            0x91e => {
                let phase = t % 10000;
                let mut shots = if phase < 7000 && phase.is_multiple_of(700)
                    || phase >= 7000 && (phase - 7000).is_multiple_of(700)
                {
                    fan(0, 1, 0.)
                } else {
                    vec![]
                };
                if phase >= 7200 && (phase - 7200).is_multiple_of(700) {
                    shots.extend(fan(0, 2, 70.));
                }
                shots
            }
            0x921 if t.is_multiple_of(600) => fan(0, 1, 0.),
            0x922 if t.is_multiple_of(1000) => fan(0, 1, 0.),
            0x923 | 0x924 if t.is_multiple_of(1000) => fan(0, 4, 10.),
            0x929 if s == 3 && t == 2950 => ring(0, 8, 22.5f32.to_radians()),
            0x929
                if s < 3
                    && e.condition & condition::bit(condition::INVULNERABLE) == 0
                    && t.is_multiple_of(400) =>
            {
                ring(0, 4, [0f32, 45., 22.5][s as usize].to_radians())
            }
            0x92a..=0x92c
                if e.condition & condition::bit(condition::INVULNERABLE) == 0
                    && t.is_multiple_of(if k == 0x92a { 400 } else { 800 }) =>
            {
                ring(
                    0,
                    if k == 0x92c { 8 } else { 4 },
                    if k == 0x92a { 0. } else { 45f32.to_radians() },
                )
            }
            0x675 if t.is_multiple_of(600) => fan(0, 4, 7.),
            0x676 | 0x677 | 0x65f..=0x661 if t.is_multiple_of(1000) => fan(0, 1, 0.),
            0x669 if t.is_multiple_of(250) => fan(0, 1, 0.),
            0x6c1 => {
                let cycle = if t < 1800 { t } else { (t - 1800) % 7300 };
                if t < 1800 && [500, 1000].contains(&t) {
                    ring(0, 12, if t == 500 { 10f32 } else { 25. }.to_radians())
                } else if t >= 1800 && cycle < 2800 && cycle.is_multiple_of(600) {
                    fan(1, 4, 7.)
                } else if t >= 1800 && cycle >= 2800 {
                    let x = cycle - 2800;
                    let mut out = if [500, 1000].contains(&x) {
                        ring(0, 12, if x == 500 { 10f32 } else { 25. }.to_radians())
                    } else {
                        vec![]
                    };
                    if x.is_multiple_of(350) {
                        out.extend(fan(1, 1, 0.));
                    }
                    out
                } else {
                    vec![]
                }
            }
            _ => vec![],
        };
        let factor = match k {
            0x675 | 0x6c1 | 0x677 => 0.5,
            0x921 => 0.4,
            0x91d if (1800..3900).contains(&e.hp) => 0.1,
            0x91e => {
                if t % 10000 < 7000 {
                    0.2
                } else {
                    0.3
                }
            }
            _ => 0.,
        };
        self.predict_enemy_shots(id, target, &mut shots, |_| factor)?;
        Ok(shots)
    }
}

#[cfg(test)]
#[test]
fn pyre_creates_one_public_chest_without_repeating_it_during_rebirth() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map {
        width: 192,
        height: 192,
        cells: vec![
            crate::content::Cell {
                ground: 0x46,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0
            };
            192 * 192
        ],
        spawns: vec![(1., 1.)],
    };
    map.cells[96 * 192 + 96].terrain = 5;
    let original = Arc::new(map);
    let mut world = World::new(content.clone(), original.clone()).unwrap();
    world.encounter = true;
    world.name = "Realm".to_owned();
    world.rng = rand::rngs::StdRng::seed_from_u64(12351);
    world.populate_named_sites(0x675, &[5, 2], 1).unwrap();
    let roots: Vec<_> = world
        .entities
        .values()
        .filter(|e| e.kind == 0x675)
        .collect();
    assert_eq!(roots.len(), 1);
    let root = roots[0];
    let tag = root.id;
    let home = root.home;
    let area = world.setpieces.last().unwrap();
    assert_eq!((area.width, area.height), (30, 30));
    assert_eq!(
        (root.pos.x - area.x as f32, root.pos.y - area.y as f32),
        (15.5, 15.5)
    );
    assert_eq!(
        (root.pos.x, root.pos.y, home.x, home.y),
        (96.5, 96.5, 96.5, 96.5)
    );
    assert_eq!((root.hp, root.size, root.stage), (3000, 100, Some(0)));
    assert!(root.published && root.quest == Some(tag) && root.camp.is_none());
    let chests: Vec<_> = world
        .entities
        .values()
        .filter(|e| e.kind == 0x501)
        .collect();
    assert_eq!(
        chests.len(),
        1,
        "Pyre must create exactly one startup chest"
    );
    let chest = chests[0];
    let chest_id = chest.id;
    let items = chest.items.clone();
    assert_eq!(
        (chest.pos.x, chest.pos.y, chest.home.x, chest.home.y),
        (96.5, 96.5, 96.5, 96.5)
    );
    assert!(
        chest.published && chest.quest.is_none() && chest.parent.is_none() && chest.camp.is_none()
    );
    assert!(chest.accessible(1) && chest.accessible(2));
    assert!(!chest.extra_stats.iter().any(|(key, _)| *key == 54));
    assert_eq!(items.len(), 8);
    let eligible = crate::loot::collectibles(&content);
    for item in items.iter().filter(|item| **item >= 0) {
        assert_eq!(content.objects[&(*item as u16)].class, "Equipment");
        assert!(eligible.contains(&(*item as u16)));
    }
    let index = 96 * 192 + 96;
    assert!(world.cells.ids(index).contains(&tag) && world.cells.ids(index).contains(&chest_id));
    let cell = &world.map.cells[index];
    assert_eq!(
        (
            cell.ground,
            cell.object,
            cell.config.as_str(),
            cell.terrain,
            cell.region
        ),
        (0xf2, None, "", 5, 0)
    );
    assert!(!world.bag_expiry.contains_key(&chest_id));
    let mut current = tag;
    for expected in [0x6c2, 0x6c1] {
        world
            .damage_enemy(-1, current, 1000000, true, &[], None)
            .unwrap();
        assert!(world.critical_deaths.is_empty());
        let replacement = world
            .entities
            .values()
            .find(|e| e.kind == expected && e.quest == Some(tag))
            .unwrap();
        current = replacement.id;
        assert_eq!((replacement.home.x, replacement.home.y), (home.x, home.y));
        assert_eq!(
            world
                .entities
                .values()
                .filter(|e| e.kind == 0x501)
                .map(|e| e.id)
                .collect::<Vec<_>>(),
            vec![chest_id]
        );
        assert_eq!(world.entities[&chest_id].items, items);
        if expected == 0x6c2 {
            world.quest_mechanics(current).unwrap();
            world.now += 2000;
            world.quest_mechanics(current).unwrap();
        }
    }
    world
        .damage_enemy(-1, current, 1000000, true, &[], None)
        .unwrap();
    assert_eq!(world.critical_deaths, vec![tag]);
    world
        .damage_enemy(-1, current, 1000000, true, &[], None)
        .unwrap();
    assert_eq!(world.critical_deaths, vec![tag]);
    assert_eq!(
        world
            .entities
            .values()
            .filter(|e| e.kind == 0x501)
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        vec![chest_id]
    );
    world.entities.get_mut(&chest_id).unwrap().items.fill(-1);
    world.now += 120000;
    world.step();
    assert!(
        world.entities[&chest_id].accessible(1)
            && world.entities[&chest_id]
                .items
                .iter()
                .all(|item| *item == -1)
    );
    assert!(!world.bag_expiry.contains_key(&chest_id));
    let mut missing = Content::load(std::path::Path::new("data")).unwrap();
    missing
        .objects
        .retain(|_, o| !(o.tier == Some(5) && crate::loot::WEAPONS.contains(&o.slot_type)));
    let mut invalid = World::new(Arc::new(missing), original).unwrap();
    invalid.rng = rand::rngs::StdRng::seed_from_u64(12351);
    let error = invalid.populate_named_sites(0x675, &[5, 2], 1).unwrap_err();
    assert!(format!("{error:#}").contains("landmark 0x675 chest has an empty selected tier"));
    assert!(
        invalid
            .entities
            .values()
            .all(|e| !matches!(e.kind, 0x675 | 0x501))
    );
}

#[cfg(test)]
#[test]
fn tower_population_uses_the_primary_fixed_anchor_and_preserves_native_birth() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut openings = BTreeSet::new();
    for seed in 0..16 {
        for (terrain, owned) in [(3, false), (4, false), (3, true)] {
            let mut map = Map {
                width: 192,
                height: 192,
                cells: vec![
                    crate::content::Cell {
                        ground: 0x46,
                        object: None,
                        config: String::new(),
                        terrain: 0,
                        region: 0
                    };
                    192 * 192
                ],
                spawns: vec![(1., 1.)],
            };
            let center = 96 * 192 + 96;
            let destination = 96 * 192 + 102;
            map.cells[center].terrain = terrain;
            map.cells[center].object = Some(0x100);
            map.cells[destination] = crate::content::Cell {
                ground: 0x46,
                object: Some(0x105),
                config: "name:native;size:137".into(),
                terrain,
                region: 0,
            };
            let original = Arc::new(map);
            let mut world = World::new(content.clone(), original.clone()).unwrap();
            let native = world.cells.ids(destination)[0];
            let tree = world.cells.ids(center)[0];
            let template = match &world.entities[&native].state {
                entity_store::EntityState::Shared(t) => t.clone(),
                _ => panic!("native Bush does not use shared scenery state"),
            };
            if owned {
                world.entities.get_mut(&native).unwrap().name = "native".into();
            }
            world.rng = rand::rngs::StdRng::seed_from_u64(seed);
            world.populate_named_sites(0x928, &[4, 3], 1).unwrap();
            let roots: Vec<_> = world
                .entities
                .values()
                .filter(|e| e.kind == 0x928)
                .collect();
            assert_eq!(roots.len(), 1);
            let root = roots[0];
            let area = world.setpieces.last().unwrap();
            assert_eq!((area.width, area.height), (27, 27));
            assert_eq!(
                (root.pos.x - area.x as f32, root.pos.y - area.y as f32),
                (19., 13.),
                "seed {seed}"
            );
            assert_eq!((root.pos.x, root.pos.y), (102., 96.));
            assert_eq!((root.home.x, root.home.y), (102., 96.));
            assert_eq!((root.hp, root.size, root.stage), (300000, 95, Some(0)));
            assert!(root.published && root.quest == Some(root.id) && root.camp.is_none());
            assert!(world.walkable(root.pos));
            assert!(world.cells.ids(destination).contains(&root.id));
            assert!(world.quest_members(root.id, &[0x929]).is_empty());
            let cell = &world.map.cells[destination];
            assert_eq!(
                (
                    cell.ground,
                    cell.object,
                    cell.config.as_str(),
                    cell.terrain,
                    cell.region
                ),
                (0x46, Some(0x105), "name:native;size:137", terrain, 0)
            );
            assert!(world.cells.ids(destination).contains(&native));
            assert_eq!(world.cells.ids(destination).len(), 2);
            let e = &world.entities[&native];
            assert_eq!((e.name.as_str(), e.size), ("native", 137));
            if owned {
                assert!(matches!(&e.state, entity_store::EntityState::Owned(_)));
            } else {
                assert!(
                    matches!(&e.state, entity_store::EntityState::Shared(t) if Arc::ptr_eq(t, &template))
                );
            }
            assert!(!world.entities.contains_key(&tree));
            assert!(world.map.cells[center].object.is_none());
            assert_eq!(
                (
                    original.cells[destination].ground,
                    original.cells[destination].object,
                    original.cells[destination].config.as_str()
                ),
                (0x46, Some(0x105), "name:native;size:137")
            );
            let sides = [(13, 0), (26, 13), (13, 26), (0, 13)]
                .into_iter()
                .enumerate()
                .filter_map(|(side, (x, y))| {
                    world.map.cells[((area.y + y) * 192 + area.x + x) as usize]
                        .object
                        .is_none()
                        .then_some(side)
                })
                .collect::<Vec<_>>();
            assert_eq!(sides.len(), 1, "seed {seed}: Tower opening");
            openings.insert(sides[0]);
        }
    }
    assert_eq!(openings.len(), 4);
}

#[cfg(test)]
#[test]
fn lich_population_uses_the_fixed_anchor_and_preserves_old_native_holes() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut walls = 0;
    for (seed, width, height, old_x, old_y, old_hole, keep) in [
        (8, 25, 26, 12, 13, true, false),
        (13, 26, 25, 12, 12, false, false),
        (1, 25, 26, 12, 12, true, false),
        (12, 26, 25, 13, 12, false, true),
    ] {
        for (terrain, owned) in [(6, false), (7, false), (6, true)] {
            let mut map = Map {
                width: 192,
                height: 192,
                cells: vec![
                    crate::content::Cell {
                        ground: 0x46,
                        object: None,
                        config: String::new(),
                        terrain: 0,
                        region: 0
                    };
                    192 * 192
                ],
                spawns: vec![(1., 1.)],
            };
            let (x, y) = (96 - width / 2 + 13, 96 - height / 2 + 13);
            let destination = y * 192 + x;
            let old = (96 - height / 2 + old_y) * 192 + 96 - width / 2 + old_x;
            map.cells[96 * 192 + 96].terrain = terrain;
            map.cells[destination].terrain = terrain;
            for y in 96 - height / 2..96 - height / 2 + height {
                for x in 96 - width / 2..96 - width / 2 + width {
                    map.cells[y * 192 + x].object = Some(0x105);
                    map.cells[y * 192 + x].config = "name:native;size:137".to_owned();
                }
            }
            let original = Arc::new(map);
            let mut world = World::new(content.clone(), original.clone()).unwrap();
            let native_ids: Vec<_> = (0..192 * 192)
                .map(|i| world.cells.ids(i).first().copied())
                .collect();
            let birth_native = world.cells.ids(destination)[0];
            let template = match &world.entities[&birth_native].state {
                entity_store::EntityState::Shared(t) => t.clone(),
                _ => panic!("native Bush does not use shared scenery state"),
            };
            if owned {
                world.entities.get_mut(&birth_native).unwrap().name = "native".to_owned();
            }
            let retained = old_hole.then(|| world.cells.ids(old)[0]);
            world.rng = rand::rngs::StdRng::seed_from_u64(seed);
            world.populate_named_sites(0x91b, &[6, 7], 1).unwrap();
            let roots: Vec<_> = world
                .entities
                .values()
                .filter(|e| e.kind == 0x91b)
                .collect();
            assert_eq!(roots.len(), 1);
            let root = roots[0];
            let area = world.setpieces.last().unwrap();
            assert_eq!((area.width, area.height), (width as i32, height as i32));
            assert_eq!(
                (root.pos.x - area.x as f32, root.pos.y - area.y as f32),
                (13., 13.),
                "seed {seed}"
            );
            assert_eq!((root.pos.x, root.pos.y), (x as f32, y as f32));
            assert_eq!((root.home.x, root.home.y), (root.pos.x, root.pos.y));
            assert_eq!((root.hp, root.size, root.stage), (300000, 120, Some(0)));
            assert!(root.published && root.quest == Some(root.id) && root.camp.is_none());
            assert!(world.cells.ids(destination).contains(&root.id));
            let cell = &world.map.cells[destination];
            assert_eq!(
                (
                    cell.ground,
                    cell.object,
                    cell.config.as_str(),
                    cell.terrain,
                    cell.region
                ),
                if keep {
                    (0x46, Some(0x105), "name:native;size:137", terrain, 0)
                } else {
                    (0xad, None, "", terrain, 0)
                },
                "seed {seed}, terrain {terrain}, owned {owned}"
            );
            assert_eq!(world.entities.contains_key(&birth_native), keep);
            assert_eq!(world.cells.ids(destination).contains(&birth_native), keep);
            assert_eq!(world.cells.ids(destination).len(), 1 + usize::from(keep));
            if keep {
                let native = &world.entities[&birth_native];
                assert_eq!((native.name.as_str(), native.size), ("native", 137));
                if owned {
                    assert!(matches!(&native.state, entity_store::EntityState::Owned(_)));
                } else {
                    assert!(
                        matches!(&native.state, entity_store::EntityState::Shared(t) if Arc::ptr_eq(t, &template))
                    );
                }
            }
            for (dx, dy) in [(0., 0.), (1., 0.), (-1., 0.), (0., 1.), (0., -1.)] {
                assert!(
                    world.walkable(Pos {
                        x: root.pos.x + dx,
                        y: root.pos.y + dy
                    }),
                    "seed {seed}: {dx},{dy}"
                );
            }
            if let Some(id) = retained {
                assert!(world.cells.ids(old).contains(&id));
                assert_eq!(
                    (world.entities[&id].name.as_str(), world.entities[&id].size),
                    ("native", 137)
                );
                assert_eq!(world.map.cells[old].config, "name:native;size:137");
            }
            assert!(!Arc::ptr_eq(&original, &world.map));
            assert_eq!(
                (
                    original.cells[destination].ground,
                    original.cells[destination].object,
                    original.cells[destination].config.as_str()
                ),
                (0x46, Some(0x105), "name:native;size:137")
            );
            let wall_sites: Vec<_> = world
                .entities
                .values()
                .filter(|e| e.kind == 0x162)
                .map(|e| (e.id, e.pos))
                .collect();
            for &(id, pos) in &wall_sites {
                let index = pos.y as usize * 192 + pos.x as usize;
                let cell = &world.map.cells[index];
                let input = &original.cells[index];
                assert_eq!(
                    (
                        cell.ground,
                        cell.object,
                        &cell.config,
                        cell.terrain,
                        cell.region
                    ),
                    (
                        0xad,
                        input.object,
                        &input.config,
                        input.terrain,
                        input.region
                    ),
                    "seed {seed}, terrain {terrain}, rubble {index}"
                );
                let native = &world.entities[&native_ids[index].unwrap()];
                assert_eq!(
                    (native.kind, native.name.as_str(), native.size),
                    (0x105, "native", 137)
                );
                assert_eq!((native.pos.x, native.pos.y), (pos.x, pos.y));
                assert!(
                    matches!(&native.state, entity_store::EntityState::Shared(t) if Arc::ptr_eq(t, &template))
                );
                assert_eq!(world.cells.ids(index).len(), 2);
                assert!(
                    world.cells.ids(index).contains(&native.id)
                        && world.cells.ids(index).contains(&id)
                );
                assert_eq!(world.entities[&id].hp, 100);
                assert!(world.entities[&id].published && !world.walkable(pos));
                assert!(!world.actor_ids.contains(&id));
                walls += 1;
            }
            if let Some((id, pos)) = wall_sites.first().copied() {
                let index = pos.y as usize * 192 + pos.x as usize;
                world.damage_enemy(-1, id, 1000, true, &[], None).unwrap();
                assert!(!world.entities.contains_key(&id) && world.walkable(pos));
                assert_eq!(world.cells.ids(index), &[native_ids[index].unwrap()]);
                let cell = &world.map.cells[index];
                assert_eq!(
                    (cell.ground, cell.object, cell.config.as_str()),
                    (0xad, Some(0x105), "name:native;size:137")
                );
                let native = &world.entities[&native_ids[index].unwrap()];
                assert_eq!(
                    (native.kind, native.name.as_str(), native.size),
                    (0x105, "native", 137)
                );
                assert!(
                    matches!(&native.state, entity_store::EntityState::Shared(t) if Arc::ptr_eq(t, &template))
                );
            }
            for y in 96 - height / 2..96 - height / 2 + height {
                for x in 96 - width / 2..96 - width / 2 + width {
                    let cell = &original.cells[y * 192 + x];
                    assert_eq!(
                        (cell.ground, cell.object, cell.config.as_str()),
                        (0x46, Some(0x105), "name:native;size:137")
                    );
                }
            }
        }
    }
    assert!(walls > 0);
}

#[cfg(test)]
#[test]
fn grove_root_uses_source_anchor_for_birth_habitat_and_spacing() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    for (label, existing_x, destination_habitat, accepted) in [
        ("exact 32-unit boundary", 32., true, true),
        ("inside 32-unit boundary", 95.75, true, false),
        ("destination outside habitat", 20., false, false),
    ] {
        let mut map = Map {
            width: 192,
            height: 192,
            cells: vec![
                crate::content::Cell {
                    ground: 0x46,
                    object: None,
                    config: String::new(),
                    terrain: 0,
                    region: 0,
                };
                192 * 192
            ],
            spawns: vec![(1., 1.)],
        };
        // The adjacent candidate's destination is outside habitat, isolating the spacing boundary.
        map.cells[63 * 192 + 63].terrain = 7;
        if destination_habitat {
            map.cells[64 * 192 + 64].terrain = 7;
        }
        let map = Arc::new(map);
        let mut world = World::new(content.clone(), map.clone()).unwrap();
        world.rng = rand::rngs::StdRng::seed_from_u64(12351);
        let previous = world
            .spawn_entity(
                0x91b,
                Pos {
                    x: existing_x,
                    y: 64.,
                },
                vec![],
            )
            .unwrap();
        world.entities.get_mut(&previous).unwrap().quest = Some(previous);
        let result = world.populate_named_sites(0x91f, &[7], 1);
        assert_eq!(result.is_ok(), accepted, "{label}: {result:?}");
        if accepted {
            let root = world.entities.values().find(|e| e.kind == 0x91f).unwrap();
            assert_eq!((root.pos.x, root.pos.y), (64., 64.), "{label}");
            assert_eq!((root.home.x, root.home.y), (root.pos.x, root.pos.y));
            assert!(root.published && root.hp > 0 && root.quest == Some(root.id));
            assert_eq!(root.size, 140);
            assert!(world.walkable(root.pos));
            assert_eq!(world.map.cell(root.pos.x, root.pos.y).unwrap().terrain, 7);
            assert!(world.cells.ids(64 * 192 + 64).contains(&root.id));
            assert!(root.pos.distance(world.entities[&previous].pos) >= 32.);
        } else {
            assert!(world.entities.values().all(|e| e.kind != 0x91f));
            assert!(
                Arc::ptr_eq(&map, &world.map),
                "rejected site mutated ground"
            );
        }
    }
}

#[cfg(test)]
#[test]
fn named_site_populations_match_reference_ranges() {
    let data = std::path::Path::new("data");
    let content = Arc::new(Content::load(data).unwrap());
    let mut oasis_sites = 0;
    for source in crate::terrain::realms::sources(data).unwrap() {
        let layout = source.name().to_owned();
        let groves = source.groves().unwrap();
        let original = source.open(&content).unwrap();
        // Fixed boundary requests avoid depending on a lucky population draw.
        let mut dense = World::new(content.clone(), source.open(&content).unwrap()).unwrap();
        dense.realm_layout = Some(layout.clone());
        dense.rng = rand::rngs::StdRng::seed_from_u64(12351);
        assert!(dense.populate_named_sites(0x928, &[4, 3], -1).is_err());
        for (kind, terrains, count) in [
            (0x91f, &[7, 6][..], 24),
            (0x91b, &[6, 7][..], 6),
            (0x928, &[4, 3][..], 14),
            (0x675, &[5, 2][..], 0),
            (0x678, &[8, 5][..], 0),
        ] {
            dense.populate_named_sites(kind, terrains, count).unwrap();
            let roots: Vec<_> = dense.entities.values().filter(|e| e.kind == kind).collect();
            assert_eq!(roots.len(), count as usize, "{layout}: boundary {kind:#x}");
            assert!(
                roots
                    .iter()
                    .all(|e| e.published && e.hp > 0 && e.quest == Some(e.id))
            );
        }
        let ids: BTreeSet<_> = dense
            .entities
            .values()
            .filter(|e| quest::ROOTS.contains(&e.kind))
            .map(|e| e.id)
            .collect();
        let mut cycle = crate::endgame::RealmCycle::new(ids.clone()).unwrap();
        assert_eq!(ids.len(), 44);
        for id in ids {
            assert!(cycle.defeated(id, 0));
            assert!(!cycle.defeated(id, 0));
        }
        assert_eq!(
            cycle.stage,
            crate::endgame::RealmStage::Closing {
                deadline: crate::endgame::CLOSE_DELAY_MS,
            }
        );
        drop(dense);
        let mut directory = Directory::configured(
            data,
            content.clone(),
            85,
            12351,
            Some(12351),
            vec![source],
            2,
        )
        .unwrap();
        for (world_id, world) in directory.worlds.iter().filter(|(_, w)| w.name == "Realm") {
            let roots: Vec<_> = world
                .entities
                .values()
                .filter(|e| quest::ROOTS.contains(&e.kind))
                .collect();
            for (kind, range) in [
                (crate::endgame::HEROES[0], 3..5),
                (crate::endgame::HEROES[1], 4..7),
            ] {
                let heroes: Vec<_> = world.entities.values().filter(|e| e.kind == kind).collect();
                assert!(
                    range.contains(&heroes.len()),
                    "{layout}: {kind:#x} has {} landmark heroes, expected {range:?}",
                    heroes.len()
                );
                assert!(
                    heroes
                        .iter()
                        .all(|e| e.published && e.hp > 0 && e.quest.is_none() && e.camp.is_none())
                );
            }
            for (kind, range) in [
                (0x91f, 17..25),
                (0x91b, 4..7),
                (0x928, 8..15),
                (0x675, 0..5),
                (0x678, 0..5),
            ] {
                let count = roots.iter().filter(|e| e.kind == kind).count();
                assert!(
                    range.contains(&count),
                    "{layout}: {kind:#x} has {count} roots, expected {range:?}"
                );
            }
            for king in roots.iter().filter(|e| e.kind == 0x928) {
                let area = world
                    .setpieces
                    .iter()
                    .find(|a| king.home.x == (a.x + 19) as f32 && king.home.y == (a.y + 13) as f32)
                    .unwrap();
                assert_eq!((area.width, area.height), (27, 27));
                assert_eq!((king.pos.x, king.pos.y), (king.home.x, king.home.y));
                assert!(world.walkable(king.pos));
                let i = king.home.y as usize * world.map.width as usize + king.home.x as usize;
                let a = &original.cells[i];
                let b = &world.map.cells[i];
                assert!([4, 3].contains(&b.terrain));
                assert_eq!(
                    (a.ground, a.object, &a.config, a.terrain, a.region),
                    (b.ground, b.object, &b.config, b.terrain, b.region),
                    "{layout}: Tower birth cell repainted"
                );
                assert!(world.cells.ids(i).contains(&king.id));
            }
            let expected: BTreeSet<_> = world
                .entities
                .values()
                .filter(|e| {
                    crate::endgame::HEROES.contains(&e.kind) || quest::ROOTS.contains(&e.kind)
                })
                .map(|e| e.id)
                .collect();
            assert_eq!(
                directory.endgame.realms[world_id].cycle.remaining, expected,
                "{layout}: only actual living roots bind closure"
            );
            if !groves.is_empty() {
                assert_eq!(
                    roots.iter().filter(|e| e.kind == 0x91f).count(),
                    groves.len(),
                    "{layout}: no additional random groves"
                );
                for grove in &groves {
                    let [x, y] = grove.spawn;
                    let matching: Vec<_> = roots
                        .iter()
                        .filter(|e| e.kind == 0x91f && e.pos.x == x as f32 && e.pos.y == y as f32)
                        .collect();
                    assert_eq!(
                        matching.len(),
                        1,
                        "{layout}: empty or duplicate authored Grove at {x},{y}"
                    );
                    let ent = matching[0];
                    assert_eq!((ent.hp, ent.stage, ent.size), (300000, Some(0), 140));
                    assert!(
                        world
                            .cells
                            .ids(y as usize * world.map.width as usize + x as usize)
                            .contains(&ent.id)
                    );
                    let [left, top, width, height] = grove.bounds;
                    assert!(
                        world
                            .setpieces
                            .iter()
                            .any(|a| (a.x, a.y, a.width, a.height) == (left, top, width, height))
                    );
                    for yy in top..top + height {
                        for xx in left..left + width {
                            let i = (yy * world.map.width + xx) as usize;
                            let a = &original.cells[i];
                            let b = &world.map.cells[i];
                            assert_eq!(
                                (a.ground, a.object, &a.config, a.terrain, a.region),
                                (b.ground, b.object, &b.config, b.terrain, b.region),
                                "{layout}: grove cell {xx},{yy} repainted"
                            );
                        }
                    }
                }
            }
            for (index, root) in roots.iter().enumerate() {
                assert!(root.published && root.hp > 0 && root.camp.is_none());
                assert_eq!(root.quest, Some(root.id));
                if matches!(root.kind, 0x91f | 0x91b) {
                    if root.kind == 0x91b {
                        assert!(
                            world.walkable(root.pos),
                            "{layout}: blocked Lich {}",
                            root.id
                        );
                    }
                    let area = world
                        .setpieces
                        .iter()
                        .find(|a| a.distance(root.pos) == 0.)
                        .unwrap();
                    if root.kind != 0x91f || groves.is_empty() {
                        assert_eq!(
                            (root.pos.x - area.x as f32, root.pos.y - area.y as f32),
                            (13., 13.)
                        );
                    }
                    assert_eq!((root.home.x, root.home.y), (root.pos.x, root.pos.y));
                    assert!(
                        [6, 7].contains(&world.map.cell(root.pos.x, root.pos.y).unwrap().terrain)
                    );
                }
                if root.kind == 0x678 {
                    oasis_sites += 1;
                    let palms: Vec<_> = world
                        .entities
                        .values()
                        .filter(|e| e.kind == 0x18e && e.pos.distance(root.pos) <= 13.)
                        .collect();
                    assert_eq!(palms.len(), 40, "{layout}: Oasis {}", root.id);
                    for palm in palms {
                        let cell = world.map.cell(palm.pos.x, palm.pos.y).unwrap();
                        assert_eq!((cell.ground, cell.object), (0x46, Some(0x18e)));
                        assert!(palm.published && matches!(palm.size, 120 | 140));
                        assert!(!world.walkable(palm.pos));
                        let index =
                            palm.pos.y as usize * world.map.width as usize + palm.pos.x as usize;
                        assert!(world.cells.ids(index).contains(&palm.id));
                    }
                }
                for other in &roots[index + 1..] {
                    assert!(root.pos.distance(other.pos) >= 32.);
                }
            }
        }
        let world = directory.worlds.get_mut(&-3).unwrap();
        for (kind, actual) in [(0x91f, 0x920), (0x91b, 0x91c), (0x928, 0x92d)] {
            let roots: Vec<_> = world
                .entities
                .values()
                .filter(|e| e.kind == kind)
                .map(|e| e.id)
                .collect();
            let grown = world.quest_transform(roots[0], actual).unwrap();
            assert_eq!(world.entities[&grown].quest, Some(roots[0]));
            assert_eq!(world.entities[&roots[1]].quest, Some(roots[1]));
            assert_eq!(world.entities[&roots[1]].kind, kind);
            assert!(
                directory.endgame.realms[&-3]
                    .cycle
                    .remaining
                    .contains(&roots[0])
            );
            if kind == 0x91b {
                for (entity, tag) in [(grown, roots[0]), (roots[1], roots[1])] {
                    let e = &world.entities[&entity];
                    world
                        .quest_birth((tag, e.home, e.pos), 0x91d, 1, 1, 0.5)
                        .unwrap();
                    let children = world.quest_members(entity, &[0x91d]);
                    assert_eq!(children.len(), 1);
                    assert_eq!(world.entities[&children[0]].quest, Some(tag));
                }
            }
        }
    }
    assert!(
        oasis_sites > 0,
        "sampled construction must exercise actual Oasis palms"
    );
}

#[cfg(test)]
#[test]
fn malformed_authored_groves_fail_before_publishing_roots_or_mutating_map() {
    use crate::{content::Cell, terrain::realms::Grove};
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map {
        width: 64,
        height: 64,
        cells: vec![
            Cell {
                ground: 0x46,
                object: None,
                config: String::new(),
                terrain: 6,
                region: 0
            };
            64 * 64
        ],
        spawns: vec![(1.5, 1.5)],
    };
    map.cells[20 * 64 + 20].object = Some(0x194);
    let map = Arc::new(map);
    let good = Grove {
        bounds: [20, 20, 25, 25],
        spawn: [33, 33],
        trees: 1,
    };
    for groves in [
        vec![Grove {
            bounds: [63, 63, 25, 25],
            ..good.clone()
        }],
        vec![
            good.clone(),
            Grove {
                spawn: [20, 20],
                ..good.clone()
            },
        ],
        vec![good.clone(), good.clone()],
        vec![Grove {
            trees: 2,
            ..good.clone()
        }],
    ] {
        let mut world = World::new(content.clone(), map.clone()).unwrap();
        let before = world.entities.len();
        assert!(world.populate_authored_groves(&groves).is_err());
        assert!(world.setpieces.is_empty());
        assert_eq!(world.entities.len(), before);
        assert!(Arc::ptr_eq(&map, &world.map));
    }
}

#[cfg(test)]
mod ghost_throw_tests {
    use super::*;

    #[tokio::test]
    async fn ghost_waves_throw_one_ring_before_owned_children_can_move() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
        for cell in &mut map.cells {
            cell.ground = 0x61;
            cell.object = None;
        }
        let map = Arc::new(map);
        let store = crate::store::Store::open(std::path::Path::new(":memory:"), &content).unwrap();
        let account = store.list("ghost-throw".into()).await.unwrap();
        let ch = store
            .create(account.id, content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        for tier in 0..4 {
            for company in [false, true] {
                let mut w = World::new(content.clone(), map.clone()).unwrap();
                for id in w.entities.keys().copied().collect::<Vec<_>>() {
                    w.remove_entity(id);
                }
                w.now = 10000;
                let center = Pos { x: 12.5, y: 12.5 };
                let root = w.spawn_entity(0x928, center, vec![]).unwrap();
                w.entities.get_mut(&root).unwrap().quest = Some(root);
                let (tx, mut rx) = mpsc::channel(64);
                let player = w
                    .enter(
                        ch.clone(),
                        Some(Output {
                            packets: tx,
                            lost: watch::channel(None).0,
                        }),
                    )
                    .unwrap();
                w.players.get_mut(&player).unwrap().known.insert(root);
                w.ghost_wave(root, tier, company).unwrap();
                let expected = if tier == 0 { 5 } else { 6 };
                assert_eq!(
                    w.entities
                        .values()
                        .filter(|e| (0x929..=0x92c).contains(&e.kind))
                        .count(),
                    0,
                    "tier {tier}, company {company}: no minions before landing"
                );
                assert_eq!(w.pending_named(root, None), expected);
                let mut landings = Vec::new();
                while let Ok(packet) = rx.try_recv() {
                    if packet.id != wire::SHOW_EFFECT {
                        continue;
                    }
                    let mut r = wire::Reader::new(&packet.body);
                    assert_eq!(r.u8().unwrap(), 4);
                    assert_eq!(r.i32().unwrap(), root);
                    let position = Pos {
                        x: r.f32().unwrap(),
                        y: r.f32().unwrap(),
                    };
                    assert!((position.distance(center) - 4.).abs() < 0.001);
                    assert_eq!(r.f32().unwrap(), 0.);
                    assert_eq!(r.f32().unwrap(), 0.);
                    assert_eq!(r.i32().unwrap() as u32, 0xffffbf00);
                    landings.push(position);
                }
                assert_eq!(landings.len(), expected);
                for (i, position) in landings.iter().enumerate() {
                    let angle = i as f32 * std::f32::consts::TAU / expected as f32;
                    assert!(
                        position.distance(Pos {
                            x: center.x + 4. * angle.cos(),
                            y: center.y + 4. * angle.sin()
                        }) < 0.001
                    );
                }
                w.now = 11499;
                w.named_birth_step();
                assert_eq!(w.pending_named(root, None), expected);
                w.now = 11500;
                w.named_birth_step();
                assert_eq!(w.pending_named(root, None), 0);
                let children: Vec<_> = w
                    .entities
                    .values()
                    .filter(|e| (0x929..=0x92c).contains(&e.kind))
                    .collect();
                assert_eq!(children.len(), expected);
                assert_eq!(
                    children.iter().filter(|e| e.kind == 0x929).count(),
                    usize::from(company || tier == 0)
                );
                for child in children {
                    assert_eq!(child.quest, Some(root));
                    assert!(landings.iter().any(|p| p.distance(child.pos) < 0.001));
                    assert_eq!(child.activated, Some(11500));
                    assert_eq!(child.stage, Some(0));
                    assert_ne!(child.condition & condition::bit(condition::INVULNERABLE), 0);
                }
            }
        }
    }

    #[tokio::test]
    async fn ghost_master_waits_with_the_invulnerable_king_until_overly_long_combat() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
        for cell in &mut map.cells {
            cell.ground = 0x61;
            cell.object = None;
        }
        let mut w = World::new(content, Arc::new(map)).unwrap();
        for id in w.entities.keys().copied().collect::<Vec<_>>() {
            w.remove_entity(id);
        }
        let root = w
            .spawn_entity(0x928, Pos { x: 12.5, y: 12.5 }, vec![])
            .unwrap();
        w.entities.get_mut(&root).unwrap().quest = Some(root);
        w.damage_enemy(-1, root, 1, true, &[], None).unwrap();
        let mut waited = None;
        for _ in 0..3600 {
            let Some(king) = w.entities.get(&root) else {
                break;
            };
            if king.stage == Some(7) {
                let since = *waited.get_or_insert(w.now);
                assert_ne!(king.condition & condition::bit(condition::INVULNERABLE), 0);
                assert_eq!(
                    w.quest_members(root, &[0x929]).len() + w.pending_named(root, Some(0x929)),
                    1,
                    "Ghost Master vanished {}ms into the king's invulnerable wait",
                    w.now - since
                );
            }
            w.now += 50;
            w.named_birth_step();
            w.mechanic_expiry();
            w.quest_mechanics(root).unwrap();
        }
        assert!(!w.entities.contains_key(&root));
        assert_eq!(w.now - waited.unwrap(), 140000);
        assert!(
            !w.entities
                .values()
                .any(|e| e.quest == Some(root) && e.kind == 0x929)
        );
        assert_eq!(
            w.entities
                .values()
                .filter(|e| e.quest == Some(root) && e.kind == 0x92d)
                .count(),
            1
        );
    }
}
