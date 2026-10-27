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
    pub(super) fn populate_authored_sites(
        &mut self,
        sites: &[crate::terrain::realms::Site],
    ) -> Result<()> {
        use realm_setpieces::Area;
        let mut areas = Vec::new();
        for site in sites {
            let (ground, scenery) = match site.kind {
                0x91f => (0x46, 0x194),
                0x91b => (0xad, 0x160),
                kind => bail!("unsupported authored quest-site kind {kind:#x}"),
            };
            let [x, y, width, height] = site.bounds;
            let area = Area {
                x,
                y,
                width,
                height,
            };
            let pos = Pos {
                x: site.spawn[0] as f32,
                y: site.spawn[1] as f32,
            };
            ensure!(
                x > 0
                    && y > 0
                    && width > 0
                    && height > 0
                    && x.checked_add(width).is_some_and(|v| v < self.map.width)
                    && y.checked_add(height).is_some_and(|v| v < self.map.height),
                "invalid authored quest-site bounds {:?} in {:?}",
                site.bounds,
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
                        .is_some_and(|c| c.ground == ground
                            && (site.kind != 0x91f || [6, 7].contains(&c.terrain))),
                "blocked or invalid authored quest-site anchor {:?} in {:?}",
                site.spawn,
                self.realm_layout
            );
            ensure!(
                areas
                    .iter()
                    .chain(&self.setpieces)
                    .all(|a| area.separate(a)),
                "overlapping authored quest-site {:?} in {:?}",
                site.bounds,
                self.realm_layout
            );
            let count = (y..y + height)
                .flat_map(|yy| (x..x + width).map(move |xx| (xx, yy)))
                .filter(|(xx, yy)| {
                    self.map.cells[(yy * self.map.width + xx) as usize].object == Some(scenery)
                })
                .count();
            ensure!(
                count == site.scenery && count > 0,
                "authored quest-site {:?} in {:?}: expected {} objects {scenery:#x}, found {count}",
                site.bounds,
                self.realm_layout,
                site.scenery
            );
            areas.push(area);
        }
        // Bind the boss without repainting reconstructed scenery; these anchors are engineered, not recovered: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/realm/setpieces/LichyTemple.cs
        for (site, area) in sites.iter().zip(areas) {
            self.setpieces.push(area);
            self.spawn_named_quest(
                site.kind,
                Pos {
                    x: site.spawn[0] as f32,
                    y: site.spawn[1] as f32,
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn populate_named_quests(
        &mut self,
        sites: &[crate::terrain::realms::Site],
    ) -> Result<()> {
        for kind in quest::ROSTER {
            quest::validate(&self.content, *kind)?;
            crate::loot::validate(&self.content, *kind)?;
        }
        for kind in (0x678..=0x67c).chain([0x925]).chain(0x65a..=0x65e) {
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
            // Period "Setpiece Location: Low Plains"; LowSand and 5..9 are SetPieces.cs only: https://web.archive.org/web/20120610073636id_/http://forums.wildshadow.com:80/wiki/realm-mad-god/deathmage
            (0x925, &[8, 9][..], 5..10),
        ]
        .into_iter()
        .filter(|(kind, _, _)| !sites.iter().any(|site| site.kind == *kind))
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
            self.spawn_named_quest(kind, pos)?;
        }
        Ok(())
    }
    fn spawn_named_quest(&mut self, kind: u16, pos: Pos) -> Result<()> {
        let id = self.spawn_entity(kind, pos, vec![])?;
        let e = self.entities.get_mut(&id).unwrap();
        e.quest.set(Some(id), id, &mut self.quest_ids);
        e.stage = (kind != 0x678).then_some(0);
        e.size = self.content.objects[&kind].size;
        tracing::info!(
            layout = self.realm_layout,
            entity = id,
            kind,
            x = pos.x,
            y = pos.y,
            terrain = self.map.cell(pos.x, pos.y).unwrap().terrain,
            "finite named Realm quest placed"
        );
        Ok(())
    }
    pub(super) fn quest_members(&self, id: i32, kinds: &[u16]) -> Vec<i32> {
        let tag = self.entities[&id].quest.expect("named quest membership");
        self.quest_actors(tag)
            .filter(|e| self.non_population_ids.contains(&e.id) && kinds.contains(&e.kind))
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
            quest = *e.quest,
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
        let own = self.quest_actors(tag).count() + self.pending_named(tag, None);
        let live = self.quest_actors(tag).filter(|e| e.kind == kind).count()
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
            self.summon_count() + self.named_births.len() + self.abyss_pending_count() + count
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
            e.quest.set(Some(tag), child, &mut self.quest_ids);
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
        tracing::info!(
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
        e.quest.set(*old.quest, new, &mut self.quest_ids);
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
            quest = *e.quest,
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
mod tests {
    use super::*;

    #[test]
    fn authored_sites_bind_finite_quests_without_repainting_or_duplicate_liches() {
        let path = std::path::Path::new("data");
        let content = Arc::new(Content::load(path).unwrap());
        for source in crate::terrain::realms::sources(path).unwrap() {
            let sites = source.sites().unwrap();
            let map = source.open(&content).unwrap();
            let mut world = World::new(content.clone(), map.clone()).unwrap();
            world.realm_layout = Some(source.name().into());
            world.rng = rand::rngs::StdRng::seed_from_u64(595);
            if !sites.is_empty() {
                let mut invalid = sites.clone();
                invalid.last_mut().unwrap().scenery += 1;
                assert!(
                    world
                        .populate_authored_sites(&invalid)
                        .unwrap_err()
                        .to_string()
                        .contains("expected")
                );
                assert!(world.setpieces.is_empty());
            }
            world.populate_authored_sites(&sites).unwrap();
            assert!(Arc::ptr_eq(&map, &world.map));
            assert_eq!(world.setpieces.len(), sites.len());
            let authored_liches = sites.iter().filter(|s| s.kind == 0x91b).count();
            assert_eq!(
                authored_liches,
                if source.name() == "classic-world-01-reconstruction" {
                    6
                } else {
                    0
                }
            );
            for site in &sites {
                let actors: Vec<_> = world
                    .entities
                    .values()
                    .filter(|e| {
                        e.kind == site.kind
                            && e.pos.x == site.spawn[0] as f32
                            && e.pos.y == site.spawn[1] as f32
                    })
                    .collect();
                assert_eq!(actors.len(), 1);
                let actor = actors[0];
                assert_eq!(*actor.quest, Some(actor.id));
                assert_eq!(actor.stage, Some(0));
                assert_eq!(actor.hp, content.objects[&site.kind].hp);
                assert!(world.non_population_ids.contains(&actor.id));
            }
            world.populate_named_quests(&sites).unwrap();
            let liches = world.entities.values().filter(|e| e.kind == 0x91b).count();
            assert!(if authored_liches > 0 {
                liches == authored_liches
            } else {
                (4..7).contains(&liches)
            });
        }
    }
}
