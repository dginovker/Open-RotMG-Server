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
                0x91f => (0x46, Some(0x194)),
                0x91b
                    if self.realm_layout.as_deref() == Some("classic-world-13-reconstruction") =>
                {
                    (0xad, Some(0x162))
                }
                0x91b => (0xad, Some(0x160)),
                0xdc2 => (0xc5, Some(0x1f2)),
                0x928 | 0x67d => (0x60, Some(0x120)),
                0x678
                    if matches!(
                        self.realm_layout.as_deref(),
                        Some(
                            "classic-world-10-reconstruction"
                                | "classic-world-11-reconstruction"
                                | "classic-world-13-reconstruction"
                        )
                    ) =>
                {
                    (0xf5, Some(0x18e))
                }
                0x678 => (0x46, Some(0x18e)),
                0x668 => (0xf4, None),
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
            ensure!(
                site.chest.is_some() == matches!(site.kind, 0x668 | 0x67d | 0x678 | 0xdc2)
                    && (site.kind != 0xdc2 || site.chest == Some(site.spawn)),
                "authored site {:#x} has incorrect chest metadata",
                site.kind
            );
            if let Some([cx, cy]) = site.chest {
                ensure!(
                    cx >= x
                        && cx < x + width
                        && cy >= y
                        && cy < y + height
                        && self.walkable(Pos {
                            x: cx as f32,
                            y: cy as f32
                        }),
                    "blocked or out-of-site authored chest {:?} in {:?}",
                    site.chest,
                    self.realm_layout
                );
            }
            let count = (y..y + height)
                .flat_map(|yy| (x..x + width).map(move |xx| (xx, yy)))
                .filter(|(xx, yy)| {
                    let cell = &self.map.cells[(yy * self.map.width + xx) as usize];
                    match scenery {
                        Some(kind) => cell.object == Some(kind),
                        None => cell.ground == ground,
                    }
                })
                .count();
            ensure!(
                count == site.support && count > 0,
                "authored quest-site {:#x} {:?} in {:?}: expected {} support cells, found {count}",
                site.kind,
                site.bounds,
                self.realm_layout,
                site.support
            );
            areas.push(area);
        }
        // Enter bosses/chests without repainting; anchors are engineered against hash-bound maps, not recovered: https://github.com/Zolmex/kithio-as3-source/tree/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/realm/setpieces
        for (site, area) in sites.iter().zip(areas) {
            self.setpieces.push(area);
            if site.kind == 0x928 {
                // Authored cells and their shared map stay immutable; exclusion needs only the area.
                self.ghost_towers.push(area);
            }
            let pos = Pos {
                x: site.spawn[0] as f32,
                y: site.spawn[1] as f32,
            };
            if site.kind == 0xdc2 {
                self.spawn_coil_site(pos)?;
            } else {
                self.spawn_named_quest(site.kind, pos)?;
                if let Some([x, y]) = site.chest {
                    let chest = Pos {
                        x: x as f32,
                        y: y as f32,
                    };
                    if site.kind == 0x668 {
                        self.place_red_demon_platform(chest)?;
                    }
                    self.spawn_landmark_chests(site.kind, chest)?;
                }
            }
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
            (0x91f, &[7, 6][..], 17_usize..25),
            (0x91b, &[6, 7][..], 4..7),
            (0x928, &[4, 3][..], 8..15),
            (0x675, &[5, 2][..], 0..5),
            (0x678, &[8, 5][..], 0..5),
            // Period "Setpiece Location: Low Plains"; LowSand and 5..9 are SetPieces.cs only: https://web.archive.org/web/20120610073636id_/http://forums.wildshadow.com:80/wiki/realm-mad-god/deathmage
            (0x925, &[8, 9][..], 5..10),
        ] {
            let authored = sites.iter().filter(|site| site.kind == kind).count();
            if kind == 0x91f && authored > 0 {
                continue;
            }
            let requested = self.rng.gen_range(count);
            let additional = requested.saturating_sub(authored);
            tracing::info!(
                kind,
                authored,
                requested,
                additional,
                "Realm named site census"
            );
            self.populate_named_sites(kind, terrains, additional)?;
        }
        Ok(())
    }
    fn populate_named_sites(&mut self, kind: u16, terrains: &[u8], count: usize) -> Result<()> {
        for _ in 0..count {
            let pos = self.place_named_setpiece(kind, terrains)?;
            self.spawn_named_quest(kind, pos)?;
        }
        Ok(())
    }
    fn place_red_demon_platform(&mut self, pos: Pos) -> Result<()> {
        // The chest tile is always Partial Red Floor, so standing there is not lava: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/realm/setpieces/LavaFissure.cs#L71
        let ground = self
            .map
            .cell(pos.x, pos.y)
            .context("Red Demon chest outside map")?
            .ground;
        ensure!(ground == 0xf4, "Red Demon chest is not on Lava Blend");
        let floor = self.content.get(0x153)?;
        ensure!(
            floor.static_object && floor.protects_ground && !floor.blocks,
            "Partial Red Floor cannot protect a fissure chest"
        );
        ensure!(
            !self.entities_at(pos).any(|e| e.kind == 0x153),
            "Red Demon chest already has a Partial Red Floor"
        );
        self.spawn_entity(
            0x153,
            Pos {
                x: pos.x.floor() + 0.5,
                y: pos.y.floor() + 0.5,
            },
            vec![],
        )?;
        Ok(())
    }
    fn spawn_named_quest(&mut self, kind: u16, pos: Pos) -> Result<()> {
        let id = self.spawn_entity(kind, pos, vec![])?;
        let e = self.entities.get_mut(&id).unwrap();
        e.quest.set(Some(id), id, &mut self.quest_ids);
        e.stage = (!matches!(kind, 0x678 | 0x668 | 0x67d)).then_some(0);
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
        // The shrine's two 20-skull density limits also need room for their root: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/db/BehaviorDb.SkullShrine.cs#L16-L17
        let capacity = match self.entities.get(&tag) {
            Some(root) if root.kind == 0xd56 => 41,
            _ => 32,
        };
        if own + count > capacity {
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
                <= self.summon_limit(),
            SummonBudget("shared summon budget exhausted".into())
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
                0 if f64::from(hp) / f64::from(self.content.objects[&kind].hp) < 0.99999 => {
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
                let phase = (t / 1600).min(9);
                let changed = e.stage != Some(phase);
                if changed {
                    self.entities.get_mut(&id).unwrap().stage = Some(phase);
                    tracing::debug!(entity = id, phase, "Ent growth entered");
                }
                self.entities.get_mut(&id).unwrap().size = 160 + phase as i32 * 20;
                if phase == 9 && changed {
                    let pos = self.entities[&id].pos;
                    let within = |other: Pos, squared: f32| {
                        let dx = other.x - pos.x;
                        let dy = other.y - pos.y;
                        dx * dx + dy * dy < squared
                    };
                    for sprite in self.quest_members(id, &[0x923]) {
                        if within(self.entities[&sprite].pos, 2500.) {
                            self.quest_transform(sprite, 0x924)?;
                        }
                    }
                    if self
                        .quest_members(id, &[0x924])
                        .iter()
                        .any(|sprite| within(self.entities[sprite].pos, 900.))
                    {
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
                    if t >= 2000 && ((t - 2000) / 800) % 2 == 0 {
                        150
                    } else {
                        130
                    };
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
        // These states inherit no motor, including the shared home leash.
        // https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/logic/db/BehaviorDb.EntAncient.cs#L263-L277
        // https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/logic/db/BehaviorDb.Lich.cs#L263-L286
        if (matches!(e.kind, 0x91f | 0x928) && matches!(e.stage, Some(3..=6)))
            || (e.kind == 0x924 && elapsed < 1000)
            || (e.kind == 0x91e && elapsed % 10000 >= 7000)
        {
            return (Motion::Stationary, 0.);
        }
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
                    // Reselecting by kind can change the gated bearer: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/logic/behaviors/Protect.cs
                    (
                        Motion::Protect {
                            anchor: Anchor::QuestEntity(*bearer),
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
            0x91f if e.stage == Some(1) => (Motion::Wander, 2.),
            0x91f if e.stage == Some(2) => (Motion::Wander, 3.),
            0x91b if matches!(e.stage, Some(0 | 3)) => (Motion::Wander, 5.),
            0x91b if matches!(e.stage, Some(1 | 2)) => (Motion::Wander, 3.5),
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
            0x924 if t < 1000 => vec![],
            0x923 | 0x924 if t.is_multiple_of(1000) => fan(0, 4, 10.),
            // fixedAngle is the fan center, not its first arrow: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/logic/behaviors/Shoot.cs#L99-L107
            0x929 if s == 3 && t == 2950 => quest::fan(0, 8, 45., 22.5f32.to_radians()),
            0x929
                if s < 3
                    && e.condition & condition::bit(condition::INVULNERABLE) == 0
                    && t.is_multiple_of(400) =>
            {
                quest::fan(0, 4, 90., [0f32, 45., 22.5][s as usize].to_radians())
            }
            0x92a..=0x92c
                if e.condition & condition::bit(condition::INVULNERABLE) == 0
                    && t.is_multiple_of(if k == 0x92a { 400 } else { 800 }) =>
            {
                quest::fan(
                    0,
                    if k == 0x92c { 8 } else { 4 },
                    if k == 0x92c { 45. } else { 90. },
                    [0f32, 45., 22.5][(k - 0x92a) as usize].to_radians(),
                )
            }
            0x675 if t.is_multiple_of(600) => fan(0, 4, 7.),
            0x676 | 0x677 | 0x65f..=0x661 if t.is_multiple_of(1000) => fan(0, 1, 0.),
            0x669 if t.is_multiple_of(250) => fan(0, 1, 0.),
            // Even-count fixedAngle fans are centered, not first-ray offsets: https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/logic/behaviors/Shoot.cs#L99-L107
            0x6c1 => {
                let cycle = if t < 1800 { t } else { (t - 1800) % 7300 };
                if t < 1800 && [500, 1000].contains(&t) {
                    quest::fan(0, 12, 30., if t == 500 { 10f32 } else { 25. }.to_radians())
                } else if t >= 1800 && cycle < 2800 && cycle.is_multiple_of(600) {
                    fan(1, 4, 7.)
                } else if t >= 1800 && cycle >= 2800 {
                    let x = cycle - 2800;
                    let mut out = if [500, 1000].contains(&x) {
                        quest::fan(0, 12, 30., if x == 500 { 10f32 } else { 25. }.to_radians())
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
        self.predict_enemy_shots(id, target, &mut shots, |index| {
            if k == 0x6c1 && index == 0 { 0. } else { factor }
        })?;
        Ok(shots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ghost_king_tower_keeps_eastern_master_spawnable() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let map = Map {
            width: 160,
            height: 160,
            cells: vec![
                crate::content::Cell {
                    ground: 0x18,
                    object: None,
                    config: String::new(),
                    terrain: 4,
                    region: 0,
                };
                160 * 160
            ],
            spawns: vec![(0.5, 0.5)],
        };
        let mut world = World::new(content, Arc::new(map)).unwrap();
        world.rng = rand::SeedableRng::seed_from_u64(1296);
        let home = world.place_named_setpiece(0x928, &[4]).unwrap();
        world.spawn_named_quest(0x928, home).unwrap();
        let root = *world.quest_ids.iter().next().unwrap();
        world
            .position_entity(
                root,
                Pos {
                    x: home.x + 3.,
                    y: home.y,
                },
            )
            .unwrap();
        let king = world.entities.get_mut(&root).unwrap();
        king.activated = Some(0);
        king.hp -= 4;
        assert!(world.walkable(Pos {
            x: home.x + 7.,
            y: home.y
        }));
        world.quest_mechanics(root).unwrap();
        assert_eq!(world.entities[&root].stage, Some(1));
        world.now = 2500;
        world.quest_mechanics(root).unwrap();
        world.now = 8500;
        world.quest_mechanics(root).unwrap();
        assert_eq!(world.entities[&root].stage, Some(3));
        assert_eq!(world.pending_named(root, Some(0x929)), 1);
        world.now += 1500;
        world.named_birth_step();
        let masters = world.quest_members(root, &[0x929]);
        assert_eq!(masters.len(), 1);
        let master = masters[0];
        assert_eq!(world.entities[&master].pos.x, home.x + 7.);
        assert_eq!(world.entities[&master].quest, Some(root));
        assert_eq!(world.entities[&master].home.x, home.x);
        world.entities.get_mut(&master).unwrap().hp = 93999;
        world.quest_mechanics(master).unwrap();
        assert!(!world.entities.contains_key(&root));
        let actual = world.entities.values().find(|e| e.kind == 0x92d).unwrap();
        assert_eq!(actual.quest, Some(root));
        assert_eq!((actual.home.x, actual.home.y), (home.x, home.y));
    }

    #[test]
    fn ghost_waves_keep_retired_class_escorts_out_of_the_replacement_roster() {
        for (tier, company, expected) in [
            (0, false, [1, 4, 0, 0]),
            (1, false, [0, 3, 3, 0]),
            (2, false, [0, 3, 0, 3]),
            (3, false, [0, 3, 0, 3]),
            (1, true, [1, 2, 3, 0]),
            (2, true, [1, 0, 2, 3]),
            (3, true, [1, 0, 2, 3]),
        ] {
            let (mut world, _) = super::super::movement_tests::fixture(0x18, false);
            // XML survival is not spawn evidence; the compiled primary uses only the replacement roster:
            // https://github.com/Zolmex/kithio-as3-source/blob/d15f321edcce0ef890001890c6991c25a1b0f718/Server%20Source/wServer/logic/db/BehaviorDb.GhostKing.cs
            for kind in 0x662..=0x667 {
                let object = world.content.get(kind).unwrap();
                assert_eq!(object.terrain, 0);
                assert_eq!(object.spawn_weight, None);
                assert!(!quest::ROSTER.contains(&kind));
                assert!(
                    population::FAMILIES
                        .iter()
                        .all(|f| f.leader != kind && f.roles.iter().all(|r| r.kind != kind))
                );
            }
            world
                .spawn_named_quest(0x928, Pos { x: 16., y: 16. })
                .unwrap();
            let root = *world.quest_ids.iter().next().unwrap();
            world.ghost_wave(root, tier, company).unwrap();
            assert_eq!(
                world.pending_named(root, None),
                expected.iter().sum::<usize>()
            );
            world.now += 1500;
            world.named_birth_step();
            assert_eq!(world.pending_named(root, None), 0);
            assert_eq!(
                world.quest_actors(root).count(),
                1 + expected.iter().sum::<usize>()
            );
            for (kind, count) in (0x929..=0x92c).zip(expected) {
                assert_eq!(
                    world.quest_members(root, &[kind]).len(),
                    count,
                    "tier={tier}, company={company}, kind={kind:#x}"
                );
            }
            assert!(world.actors().all(|e| !(0x662..=0x667).contains(&e.kind)));
        }
    }

    #[test]
    fn shrine_capacity_allows_both_colours_and_replacement_without_raising_other_caps() {
        let (mut world, _) = super::super::movement_tests::fixture(0x18, false);
        let pos = Pos { x: 8.5, y: 8.5 };
        world.spawn_named_quest(0xd56, pos).unwrap();
        let root = world.actors().find(|e| e.kind == 0xd56).unwrap().id;
        for now in (500..=15000).step_by(500) {
            world.now = now;
            world.event_mechanics(root).unwrap();
        }
        for kind in [0xd57, 0xd58] {
            assert_eq!(world.quest_members(root, &[kind]).len(), 20);
        }
        assert_eq!(world.quest_actors(root).count(), 41);
        let defeated = world.quest_members(root, &[0xd57])[0];
        world.remove_entity(defeated);
        world.now += 500;
        world.event_mechanics(root).unwrap();
        assert_eq!(world.quest_members(root, &[0xd57]).len(), 20);
        assert!(!world.entities.contains_key(&defeated));
        assert_eq!(world.quest_actors(root).count(), 41);

        world.spawn_named_quest(0xd59, pos).unwrap();
        let cube = world.actors().find(|e| e.kind == 0xd59).unwrap().id;
        world
            .quest_birth((cube, pos, pos), 0xd5a, 32, 31, 5.)
            .unwrap();
        assert_eq!(world.quest_actors(cube).count(), 32);
        world
            .quest_birth((cube, pos, pos), 0xd5a, 32, 1, 5.)
            .unwrap();
        assert_eq!(world.quest_actors(cube).count(), 32);
    }

    #[test]
    fn authored_red_demon_chest_protects_only_its_tile_without_repainting_lava() {
        let (mut world, player) = super::super::movement_tests::fixture(0xf4, false);
        let original = world.map.clone();
        let pos = world.players[&player].pos;
        let initial = world.players[&player].character.hp;
        world.ground_damage(player, 1000, pos).unwrap();
        let damaged = world.players[&player].character.hp;
        assert!(damaged < initial);
        world
            .populate_authored_sites(&[crate::terrain::realms::Site {
                kind: 0x668,
                bounds: [5, 5, 7, 7],
                spawn: [8, 8],
                chest: Some([8, 8]),
                support: 49,
            }])
            .unwrap();
        assert!(Arc::ptr_eq(&original, &world.map));
        let floor = world.entities_at(pos).find(|e| e.kind == 0x153).unwrap();
        assert_eq!((floor.pos.x, floor.pos.y), (8.5, 8.5));
        let floor = floor.id;
        assert!(world.entities_at(pos).any(|e| e.kind == 0x501));
        world.updates(player, true).unwrap();
        assert!(world.players[&player].known.contains(&floor));
        world.ground_damage(player, 1601, pos).unwrap();
        assert_eq!(world.players[&player].character.hp, damaged);
        world
            .ground_damage(player, 2202, Pos { x: 9.5, y: 8.5 })
            .unwrap();
        assert!(world.players[&player].character.hp < damaged);
    }

    #[test]
    fn authored_sites_bind_finite_quests_without_repainting_or_duplicate_liches() {
        let path = std::path::Path::new("data");
        let content = Arc::new(Content::load(path).unwrap());
        for source in crate::terrain::realms::sources(path).unwrap() {
            let sites = source.sites().unwrap();
            let expected = match source.name() {
                "classic-world-01-reconstruction" => [5, 7, 8, 6],
                "classic-world-02-reconstruction" => [1, 6, 0, 8],
                "classic-world-03-reconstruction" => [4, 3, 0, 6],
                "classic-world-04-reconstruction" | "classic-world-10-reconstruction" => {
                    [0, 0, 1, 0]
                }
                "classic-world-05-reconstruction"
                | "classic-world-11-reconstruction"
                | "classic-world-13-reconstruction" => [0, 0, 2, 0],
                "classic-world-06-reconstruction"
                | "classic-world-07-reconstruction"
                | "classic-world-08-reconstruction"
                | "classic-world-09-reconstruction"
                | "classic-world-12-reconstruction" => [0; 4],
                "realm-fsod-1" | "realm-fsod-2" | "realm-fsod-3" => [0; 4],
                name => panic!("unreviewed authored-site roster {name}"),
            };
            for (kind, count) in [0x928, 0x67d, 0x678, 0x668].into_iter().zip(expected) {
                assert_eq!(sites.iter().filter(|s| s.kind == kind).count(), count);
            }
            let map = source.open(&content).unwrap();
            let mut world = World::new(content.clone(), map.clone()).unwrap();
            world.realm_layout = Some(source.name().into());
            world.rng = rand::rngs::StdRng::seed_from_u64(595);
            if !sites.is_empty() {
                let mut invalid = sites.clone();
                invalid.last_mut().unwrap().support += 1;
                assert!(
                    world
                        .populate_authored_sites(&invalid)
                        .unwrap_err()
                        .to_string()
                        .contains("expected")
                );
                assert!(world.setpieces.is_empty());
            }
            if matches!(
                source.name(),
                "classic-world-10-reconstruction" | "classic-world-11-reconstruction"
            ) {
                let mut wrong_source = World::new(content.clone(), map.clone()).unwrap();
                wrong_source.realm_layout = Some("classic-world-05-reconstruction".into());
                assert!(
                    wrong_source
                        .populate_authored_sites(&sites)
                        .unwrap_err()
                        .to_string()
                        .contains("blocked or invalid authored quest-site anchor")
                );
                assert!(wrong_source.setpieces.is_empty());
            }
            if source.name() == "classic-world-13-reconstruction" {
                let mut wrong_source = World::new(content.clone(), map.clone()).unwrap();
                wrong_source.realm_layout = Some("classic-world-05-reconstruction".into());
                for kind in [0x91b, 0x678] {
                    let site = sites.iter().find(|s| s.kind == kind).unwrap().clone();
                    assert!(wrong_source.populate_authored_sites(&[site]).is_err());
                    assert!(wrong_source.setpieces.is_empty());
                }
            }
            world.populate_authored_sites(&sites).unwrap();
            assert!(Arc::ptr_eq(&map, &world.map));
            assert_eq!(world.setpieces.len(), sites.len());
            let authored_liches = sites.iter().filter(|s| s.kind == 0x91b).count();
            assert_eq!(
                authored_liches,
                match source.name() {
                    "classic-world-01-reconstruction" | "classic-world-04-reconstruction" => 6,
                    "classic-world-05-reconstruction"
                    | "classic-world-06-reconstruction"
                    | "classic-world-10-reconstruction" => 5,
                    "classic-world-07-reconstruction" => 10,
                    "classic-world-08-reconstruction" => 6,
                    "classic-world-09-reconstruction" => 3,
                    "classic-world-11-reconstruction" => 9,
                    "classic-world-12-reconstruction" => 8,
                    "classic-world-13-reconstruction" => 4,
                    _ => 0,
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
                assert_eq!(
                    actor.stage,
                    (!matches!(site.kind, 0x678 | 0x668 | 0x67d | 0xdc2)).then_some(0)
                );
                assert_eq!(actor.hp, content.objects[&site.kind].hp);
                assert!(world.non_population_ids.contains(&actor.id));
            }
            world.populate_coil_snakes(&sites).unwrap();
            assert_eq!(
                world.actors().filter(|e| e.kind == 0xdc2).count(),
                match source.name() {
                    "classic-world-04-reconstruction" | "classic-world-09-reconstruction" => 16,
                    "classic-world-05-reconstruction" | "classic-world-13-reconstruction" => 22,
                    "classic-world-06-reconstruction" => 14,
                    "classic-world-07-reconstruction" => 21,
                    "classic-world-08-reconstruction" => 11,
                    "classic-world-10-reconstruction" => 17,
                    "classic-world-11-reconstruction" => 25,
                    "classic-world-12-reconstruction" => 8,
                    _ => 2,
                }
            );
            world.populate_named_quests(&sites).unwrap();
            world.populate_realm_heroes(&sites).unwrap();
            assert!(world.actors().all(|e| !(0x662..=0x667).contains(&e.kind)));
            for (kind, minimum, maximum) in
                [(0x928, 8, 14), (0x668, 3, 4), (0x67d, 4, 6), (0x678, 0, 4)]
            {
                let authored = sites.iter().filter(|s| s.kind == kind).count();
                let total = world.entities.values().filter(|e| e.kind == kind).count();
                assert!(
                    total >= authored.max(minimum) && total <= authored.max(maximum),
                    "{kind:#x}: {total}, authored {authored}"
                );
            }
            for site in &sites {
                if let Some([x, y]) = site.chest {
                    assert!(
                        world.entities.values().any(|e| e.kind == 0x501
                            && e.pos.x == x as f32
                            && e.pos.y == y as f32
                            && (site.kind == 0xdc2 || e.items.iter().any(|item| *item != -1))),
                        "missing authored chest for {:#x} at {x},{y} in {}",
                        site.kind,
                        source.name()
                    );
                    if site.kind == 0x668 {
                        let pos = Pos {
                            x: x as f32,
                            y: y as f32,
                        };
                        assert!(
                            world.entities_at(pos).any(|e| {
                                e.kind == 0x153 && world.content.objects[&e.kind].protects_ground
                            }),
                            "Red Demon chest at {x},{y} in {} has no Partial Red Floor",
                            source.name()
                        );
                    }
                }
            }
            let liches = world.entities.values().filter(|e| e.kind == 0x91b).count();
            assert!((authored_liches.max(4)..=authored_liches.max(6)).contains(&liches));
        }
    }

    #[test]
    fn full_population_preserves_classic_landmarks_lava_and_routes() {
        for (name, side, lava_count, groves, coils, liches) in [
            ("classic-world-01-reconstruction", 2100, 703, 19, 2, 6..=6),
            ("classic-world-02-reconstruction", 2100, 864, 24, 2, 4..=6),
            ("classic-world-03-reconstruction", 2100, 629, 21, 2, 4..=6),
            ("classic-world-04-reconstruction", 2000, 506, 21, 16, 6..=6),
            ("classic-world-05-reconstruction", 2100, 702, 19, 22, 5..=6),
            ("classic-world-06-reconstruction", 2100, 698, 25, 14, 5..=6),
            (
                "classic-world-07-reconstruction",
                2100,
                660,
                20,
                21,
                10..=10,
            ),
            ("classic-world-08-reconstruction", 2000, 581, 17, 11, 6..=6),
            ("classic-world-09-reconstruction", 2100, 426, 14, 16, 4..=6),
            ("classic-world-10-reconstruction", 2000, 460, 16, 17, 5..=6),
            ("classic-world-11-reconstruction", 2100, 577, 20, 25, 9..=9),
            ("classic-world-12-reconstruction", 2100, 635, 31, 8, 8..=8),
            ("classic-world-13-reconstruction", 2000, 634, 23, 22, 4..=6),
        ] {
            let path = std::path::Path::new("data");
            let content = Arc::new(Content::load(path).unwrap());
            let mut sources = crate::terrain::realms::sources(path).unwrap();
            let index = sources.iter().position(|s| s.name() == name).unwrap();
            let seed = (0..256)
                .find(|seed| {
                    rand::rngs::StdRng::seed_from_u64(*seed).gen_range(0..sources.len()) == index
                })
                .unwrap();
            eprintln!("{name} native catalog-selection seed: {seed}");
            let source = sources.swap_remove(index);
            let sites = source.sites().unwrap();
            let original = source.open(&content).unwrap();
            let lava: Vec<_> = original
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.ground == 244)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(lava.len(), lava_count);
            let directory =
                Directory::configured(path, content, 85, seed, Some(seed), vec![source], 2)
                    .unwrap();
            for id in [-3] {
                let world = &directory.worlds[&id];
                assert_eq!(world.realm_layout.as_deref(), Some(name));
                assert_eq!((world.map.width, world.map.height), (side, side));
                assert_eq!(world.map.spawns.len(), 12);
                assert_eq!(world.actors().filter(|e| e.kind == 0x91f).count(), groves);
                assert_eq!(world.actors().filter(|e| e.kind == 0xdc2).count(), coils);
                assert!(liches.contains(&world.actors().filter(|e| e.kind == 0x91b).count()));
                assert!(
                    world
                        .actors()
                        .filter(|e| e.population_slot.is_some())
                        .count()
                        > 100
                );
                let width = world.map.width as usize;
                let (x, y) = world.map.spawns[0];
                let origin = y.floor() as usize * width + x.floor() as usize;
                let mut reachable = vec![false; world.map.cells.len()];
                let mut pending = vec![origin];
                reachable[origin] = true;
                while let Some(i) = pending.pop() {
                    let (x, y) = ((i % width) as i32, (i / width) as i32);
                    for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
                        let pos = Pos {
                            x: (x + dx) as f32 + 0.5,
                            y: (y + dy) as f32 + 0.5,
                        };
                        if world.walkable(pos) {
                            let n = pos.y as usize * width + pos.x as usize;
                            if !reachable[n] && world.map.cells[n].ground != 244 {
                                reachable[n] = true;
                                pending.push(n);
                            }
                        }
                    }
                }
                for &(x, y) in &world.map.spawns {
                    assert!(reachable[y.floor() as usize * width + x.floor() as usize]);
                }
                for site in &sites {
                    let cell = site.spawn[1] as usize * width + site.spawn[0] as usize;
                    // Red Demons and their chests intentionally stand on Fissure lava, not dry ground.
                    let hazard_approach = site.kind == 0x668
                        && world.map.cells[cell].ground == 244
                        && [(0, -1), (-1, 0), (1, 0), (0, 1)]
                            .into_iter()
                            .any(|(dx, dy)| {
                                (1..=8)
                                    .map(|distance| Pos {
                                        x: (site.spawn[0] + dx * distance) as f32 + 0.5,
                                        y: (site.spawn[1] + dy * distance) as f32 + 0.5,
                                    })
                                    .take_while(|pos| world.walkable(*pos))
                                    .any(|pos| reachable[pos.y as usize * width + pos.x as usize])
                            });
                    assert!(
                        reachable[cell] || hazard_approach,
                        "populated {name} Realm {id} lacks an approach to authored {:#x} at {:?}",
                        site.kind,
                        site.spawn
                    );
                }
                for i in &lava {
                    let (a, b) = (&original.cells[*i], &world.map.cells[*i]);
                    assert_eq!(
                        (a.ground, a.object, &a.config, a.terrain, a.region),
                        (b.ground, b.object, &b.config, b.terrain, b.region),
                        "{name} Realm {id} changed lava cell {i}"
                    );
                }
                for site in &sites {
                    let [x, y, width, height] = site.bounds;
                    for yy in y..y + height {
                        for xx in x..x + width {
                            let i = (yy * world.map.width + xx) as usize;
                            let (a, b) = (&original.cells[i], &world.map.cells[i]);
                            assert_eq!(
                                (a.ground, a.object, &a.config, a.terrain, a.region),
                                (b.ground, b.object, &b.config, b.terrain, b.region),
                                "{name} Realm {id} changed landmark cell {xx},{yy}"
                            );
                        }
                    }
                    assert!(world.walkable(Pos {
                        x: site.spawn[0] as f32,
                        y: site.spawn[1] as f32
                    }));
                    assert_eq!(
                        world
                            .actors()
                            .filter(|e| e.kind == site.kind
                                && e.home.x == site.spawn[0] as f32
                                && e.home.y == site.spawn[1] as f32)
                            .count(),
                        1
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[test]
fn ghost_volleys_use_primary_centered_fixed_angles() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    let mut world = World::new(content, Arc::new(map)).unwrap();
    let home = Pos { x: 12.5, y: 12.5 };
    let mut character = Character::fresh(world.content.get(0x30e).unwrap()).unwrap();
    character.account = 1;
    character.id = 1;
    let player = world.enter(character, None).unwrap();
    for (kind, stage, time, expected) in [
        (0x929, 0, 400, vec![-135f32, -45., 45., 135.]),
        (0x929, 1, 400, vec![-90., 0., 90., 180.]),
        (0x929, 2, 400, vec![-112.5, -22.5, 67.5, 157.5]),
        (
            0x929,
            3,
            2950,
            vec![-135., -90., -45., 0., 45., 90., 135., 180.],
        ),
        (0x92a, 0, 400, vec![-135., -45., 45., 135.]),
        (0x92b, 0, 800, vec![-90., 0., 90., 180.]),
        (
            0x92c,
            0,
            800,
            vec![-135., -90., -45., 0., 45., 90., 135., 180.],
        ),
    ] {
        let id = world.spawn_entity(kind, home, vec![]).unwrap();
        let e = world.entities.get_mut(&id).unwrap();
        e.stage = Some(stage);
        e.condition = 0;
        for target in [Pos { x: 17.5, y: 12.5 }, Pos { x: 12.5, y: 17.5 }] {
            world.players.get_mut(&player).unwrap().pos = target;
            let shots = world.quest_shots(id, target, time).unwrap();
            assert_eq!(shots.len(), expected.len(), "kind={kind:#x}, stage={stage}");
            for ((index, angle), degrees) in shots.iter().zip(&expected) {
                assert_eq!(*index, 0);
                let error = (*angle - degrees.to_radians() + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                assert!(
                    error.abs() < 0.00001,
                    "kind={kind:#x}, stage={stage}, angle={angle}, expected={degrees}"
                );
            }
            assert!(world.quest_shots(id, target, time + 50).unwrap().is_empty());
        }
        if stage != 3 {
            world.quest_shield(id, true);
            assert!(world.quest_shots(id, home, time).unwrap().is_empty());
        }
        world.remove_entity(id);
    }
}

#[cfg(test)]
#[test]
fn phoenix_egg_pulses_only_after_its_shield_expires() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    let mut world = World::new(content, Arc::new(map)).unwrap();
    world.now = 5000;
    let mut character = Character::fresh(world.content.get(0x30e).unwrap()).unwrap();
    character.id = 1;
    character.account = 1;
    let player = world.enter(character, None).unwrap();
    world
        .spawn_named_quest(0x675, Pos { x: 12.5, y: 12.5 })
        .unwrap();
    let lord = *world.quest_ids.iter().next().unwrap();
    world
        .damage_enemy(player, lord, 10000, false, &[], None)
        .unwrap();
    let egg = world
        .quest_actors(lord)
        .find(|e| e.kind == 0x6c2)
        .unwrap()
        .id;
    assert!(!world.entities.contains_key(&lord));
    for (elapsed, size) in [
        (0, 130),
        (750, 130),
        (800, 130),
        (1500, 130),
        (1950, 130),
        (2000, 150),
        (2750, 150),
        (2800, 130),
        (3550, 130),
        (3600, 150),
    ] {
        world.now = 5000 + elapsed;
        world.quest_mechanics(egg).unwrap();
        let e = &world.entities[&egg];
        assert_eq!(e.size, size, "elapsed={elapsed}");
        assert_eq!(
            e.condition & condition::bit(condition::INVULNERABLE) != 0,
            elapsed < 2000
        );
        assert_eq!(*e.quest, Some(lord));
        assert_eq!(
            world
                .damage_enemy(player, egg, 10, false, &[], None)
                .unwrap(),
            if elapsed < 2000 { 0 } else { 10 }
        );
    }
    world
        .damage_enemy(player, egg, 10000, false, &[], None)
        .unwrap();
    assert!(!world.entities.contains_key(&egg));
    let reborn = world.quest_actors(lord).find(|e| e.kind == 0x6c1).unwrap();
    assert_eq!(reborn.hp, 4200);
    assert_eq!(reborn.size, 100);
    assert_eq!(reborn.damage[&1], 8000);
    assert_eq!(world.players[&player].character.exp, 0);
    assert!(world.critical_deaths.is_empty());
    let reborn = reborn.id;
    let p = world.players.get_mut(&player).unwrap();
    p.pos = Pos { x: 14.5, y: 12.5 };
    p.motion.at = world.now;
    p.motion.velocity = Pos { x: 0., y: 2. };
    for (time, center) in [(500, 10f32), (1000, 25.), (5100, 10.), (5600, 25.)] {
        let shots = world
            .quest_shots(reborn, Pos { x: 14.5, y: 12.5 }, time)
            .unwrap();
        assert_eq!(shots.len(), 12);
        for (n, (index, angle)) in shots.iter().enumerate() {
            assert_eq!(*index, 0);
            assert!((*angle - (center - 165. + 30. * n as f32).to_radians()).abs() < 0.00001);
        }
    }
    for (time, count) in [(1800, 4), (2400, 4), (4600, 1), (4950, 1)] {
        let shots = world
            .quest_shots(reborn, Pos { x: 14.5, y: 12.5 }, time)
            .unwrap();
        assert_eq!(shots.len(), count);
        assert!(shots.iter().all(|(index, _)| *index == 1));
    }
}

#[cfg(test)]
#[test]
fn quest_idle_requires_the_strict_primary_hp_ratio() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/tutorial.wmap"), &content).unwrap();
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    let mut world = World::new(content, Arc::new(map)).unwrap();
    world.now = 12300;
    let home = Pos { x: 12.5, y: 12.5 };
    for kind in [0x91f, 0x928, 0x91b] {
        let root = world.spawn_entity(kind, home, vec![]).unwrap();
        world
            .entities
            .get_mut(&root)
            .unwrap()
            .quest
            .set(Some(root), root, &mut world.quest_ids);
        world.quest_mechanics(root).unwrap();
        assert_eq!(world.entities[&root].hp, 300000);
        let rng = rand::RngCore::next_u64(&mut world.rng.clone());
        for loss in 1..=4 {
            assert_eq!(
                world.damage_enemy(-1, root, 8, false, &[], None).unwrap(),
                1
            );
            world.now += 100;
            world.quest_mechanics(root).unwrap();
            let entity = &world.entities[&root];
            assert_eq!(entity.hp, 300000 - loss);
            assert_eq!(
                entity.stage,
                Some(u64::from(loss == 4)),
                "kind={kind:#x}, loss={loss}"
            );
            assert_eq!(
                entity.condition & condition::bit(condition::INVULNERABLE) != 0,
                loss == 4
            );
            assert_eq!(
                (entity.home.x, entity.home.y, *entity.quest),
                (home.x, home.y, Some(root))
            );
        }
        assert_eq!(
            world.damage_enemy(-1, root, 8, false, &[], None).unwrap(),
            0
        );
        assert_eq!(rand::RngCore::next_u64(&mut world.rng.clone()), rng);
        assert!(world.critical_deaths.is_empty());
    }
}

#[cfg(test)]
#[test]
fn ent_sprite_orders_use_strict_owned_radii() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let mut map = Map::load(std::path::Path::new("data/nexus.wmap"), &content).unwrap();
    for cell in &mut map.cells {
        cell.ground = 0x61;
        cell.object = None;
    }
    let map = Arc::new(map);
    let home = Pos { x: 12.5, y: 12.5 };
    let armored = condition::bit(condition::ARMORED);
    for (kind, radius) in [(0x923, 50.), (0x924, 30.), (0x923, 30.)] {
        for offset in [-0.5, 0., 0.5] {
            let d = radius + offset;
            let transforms = kind == 0x923 && d < 50.;
            let mut world = World::new(content.clone(), map.clone()).unwrap();
            let root = world.spawn_entity(0x920, home, vec![]).unwrap();
            let foreign = world
                .spawn_entity(0x920, Pos { x: 15.5, y: 15.5 }, vec![])
                .unwrap();
            for id in [root, foreign] {
                let e = world.entities.get_mut(&id).unwrap();
                e.quest.set(Some(id), id, &mut world.quest_ids);
                e.stage = Some(8);
                e.activated = Some(0);
                e.condition &= !armored;
            }
            let mut fixtures = Vec::new();
            for (owner, k, pos) in [
                (
                    root,
                    kind,
                    Pos {
                        x: home.x + d,
                        y: home.y,
                    },
                ),
                (
                    root,
                    kind,
                    Pos {
                        x: home.x,
                        y: home.y + d,
                    },
                ),
                (foreign, 0x923, Pos { x: 13.5, y: 12.5 }),
                (foreign, 0x924, Pos { x: 12.5, y: 13.5 }),
            ] {
                let id = world.spawn_entity(k, pos, vec![]).unwrap();
                let e = world.entities.get_mut(&id).unwrap();
                e.quest.set(Some(owner), id, &mut world.quest_ids);
                e.home = home;
                fixtures.push((id, owner, k, pos));
            }
            world.now = 14400;
            let rng = rand::RngCore::next_u64(&mut world.rng.clone());
            let deaths = world.critical_deaths.len();
            let count = world.entities.len();
            let hp = world.entities[&root].hp;
            world.quest_mechanics(root).unwrap();
            assert_eq!(world.entities[&root].stage, Some(9));
            assert_eq!(world.entities[&root].hp, hp);
            assert_eq!(
                world.entities[&root].condition & armored != 0,
                d < 30.,
                "kind={kind:#x}, d={d}"
            );
            assert_eq!(world.entities[&foreign].condition & armored, 0);
            assert_eq!(world.entities[&foreign].stage, Some(8));
            for owner in [root, foreign] {
                assert_eq!(world.quest_members(owner, &[0x920, 0x923, 0x924]).len(), 3);
                assert!(world.quest_ids.contains(&owner));
                assert_eq!(*world.entities[&owner].quest, Some(owner));
            }
            for (old, owner, k, pos) in fixtures {
                let replaced = owner == root && transforms;
                let expected = if replaced { 0x924 } else { k };
                let matches: Vec<_> = world
                    .quest_members(owner, &[expected])
                    .into_iter()
                    .filter(|id| {
                        let p = world.entities[id].pos;
                        p.x == pos.x && p.y == pos.y
                    })
                    .collect();
                assert_eq!(matches.len(), 1, "kind={kind:#x}, d={d}, old={old}");
                let new = matches[0];
                if replaced {
                    assert_ne!(new, old);
                    assert!(!world.entities.contains_key(&old));
                    assert!(!world.quest_ids.contains(&old));
                    assert!(!world.non_population_ids.contains(&old));
                } else {
                    assert_eq!(new, old);
                }
                let e = &world.entities[&new];
                assert_eq!((e.kind, *e.quest), (expected, Some(owner)));
                assert_eq!((e.home.x, e.home.y), (home.x, home.y));
                assert!(world.quest_ids.contains(&new));
                assert!(world.non_population_ids.contains(&new));
            }
            assert_eq!(world.entities.len(), count);
            assert_eq!(world.critical_deaths.len(), deaths);
            // Replacement spawns may draw RNG; only non-transform paths promise equality here.
            if !transforms {
                assert_eq!(rand::RngCore::next_u64(&mut world.rng.clone()), rng);
            }
        }
    }
}

#[cfg(test)]
#[test]
fn quest_preparation_motors_preserve_other_phases() {
    let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
    let map = Map {
        width: 32,
        height: 32,
        cells: vec![
            crate::content::Cell {
                ground: 0x61,
                object: None,
                config: String::new(),
                terrain: 0,
                region: 0,
            };
            32 * 32
        ],
        spawns: vec![(8.5, 8.5)],
    };
    let mut world = World::new(content, Arc::new(map)).unwrap();
    world.name = "Realm".into();
    world.encounter = true;
    world.safe = false;
    let character: Character = serde_json::from_value(serde_json::json!({
        "schema":1,"lifetime":crate::store::FameLifetime::complete(),
        "class":0x300,"base":[200,100,15,0,75,15,15,10],
        "id":1,"account":1,"name":"Wander","level":20,"exp":0,
        "fame":0,"hp":200,"mp":100,"revision":0,"alive":true,
        "items":[0xa14,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1]
    }))
    .unwrap();
    world.enter(character, None).unwrap();
    let home = Pos { x: 12.5, y: 12.5 };
    for (kind, stage, offset, nominal) in [
        (0x91f, 0, 0., 1.295_f32),
        (0x91f, 1, 0., 1.85),
        (0x91f, 2, 0., 2.405),
        (0x91f, 3, 0., 0.),
        (0x91f, 3, 10., 0.),
        (0x91f, 4, 0., 0.),
        (0x91f, 4, 10., 0.),
        (0x91f, 5, 0., 0.),
        (0x91f, 5, 10., 0.),
        (0x91f, 6, 0., 0.),
        (0x91f, 6, 10., 0.),
        (0x920, 0, 0., 1.85),
        (0x928, 3, 0., 0.),
        (0x928, 3, 10., 0.),
        (0x928, 4, 0., 0.),
        (0x928, 4, 10., 0.),
        (0x928, 5, 0., 0.),
        (0x928, 5, 10., 0.),
        (0x928, 6, 0., 0.),
        (0x928, 6, 10., 0.),
        (0x91b, 0, 0., 3.515),
        (0x91b, 1, 0., 2.6825),
        (0x91b, 2, 0., 2.6825),
        (0x91b, 3, 0., 3.515),
        (0x91b, 4, 0., 2.96),
    ] {
        world.now += 50;
        let id = world.spawn_entity(kind, home, vec![]).unwrap();
        let position = Pos {
            x: home.x + offset,
            y: home.y,
        };
        world.position_entity(id, position).unwrap();
        let e = world.entities.get_mut(&id).unwrap();
        e.quest.set(Some(id), id, &mut world.quest_ids);
        e.stage = Some(stage);
        e.activated = Some(world.now);
        e.next_shot = u64::MAX;
        e.motion = Some(crate::encounter::Motion::Wander);
        e.goal = Some(Pos {
            x: position.x + 1.,
            y: position.y,
        });
        let hp = e.hp;
        let rng = rand::RngCore::next_u64(&mut world.rng.clone());
        world.npc_step();
        let e = &world.entities[&id];
        let distance = nominal * 0.05;
        let tolerance = 2. * f32::EPSILON * position.x;
        assert!(
            (e.pos.x - position.x - distance).abs() <= tolerance,
            "kind={kind:#x}, stage={stage}, offset={offset}: displacement {} != {distance}",
            e.pos.x - position.x
        );
        assert_eq!(
            (e.pos.y, e.home.x, e.home.y, *e.quest, e.hp),
            (position.y, home.x, home.y, Some(id), hp)
        );
        assert_eq!(rand::RngCore::next_u64(&mut world.rng.clone()), rng);
        assert!(world.critical_deaths.is_empty());
        world.remove_entity(id);
    }
    for (kind, age, offset, moving, bullets) in [
        (0x924, 0, 0., false, 0),
        (0x924, 0, 12., false, 0),
        (0x924, 999, 0., false, 0),
        (0x924, 999, 12., false, 0),
        (0x924, 1000, 0., true, 4),
        (0x924, 1000, 12., true, 0),
        (0x923, 0, 0., true, 4),
    ] {
        world.now += 50;
        let id = world.spawn_entity(kind, home, vec![]).unwrap();
        let position = Pos {
            x: home.x + offset,
            y: home.y,
        };
        world.position_entity(id, position).unwrap();
        let e = world.entities.get_mut(&id).unwrap();
        e.quest.set(Some(id), id, &mut world.quest_ids);
        e.stage = (age > 0).then_some(0);
        e.activated = Some(world.now - if age == 0 { 50 } else { age });
        e.next_shot = if age == 0 { u64::MAX } else { world.now };
        let hp = e.hp;
        let rng = rand::RngCore::next_u64(&mut world.rng.clone());
        world.npc_step();
        let e = &world.entities[&id];
        assert_eq!(
            (e.pos.x, e.pos.y) != (position.x, position.y),
            moving,
            "kind={kind:#x}, age={age}, offset={offset}"
        );
        assert_eq!(
            world.bullets.iter().filter(|b| b.owner == id).count(),
            bullets,
            "kind={kind:#x}, age={age}, offset={offset}"
        );
        assert_eq!(
            (e.home.x, e.home.y, *e.quest, e.hp, e.stage, e.activated),
            (home.x, home.y, Some(id), hp, Some(0), Some(world.now - age))
        );
        if offset == 0. {
            assert_eq!(
                e.next_shot,
                world.now + crate::encounter::profile(kind).unwrap().interval
            );
        }
        if moving {
            assert_eq!(
                e.motion,
                Some(if offset == 0. {
                    crate::encounter::Motion::Orbit {
                        anchor: crate::encounter::Anchor::Player,
                        radius: 4.,
                    }
                } else {
                    crate::encounter::Motion::Protect {
                        anchor: crate::encounter::Anchor::Home,
                        distance: 1.,
                    }
                })
            );
        } else {
            assert_eq!(e.motion, Some(crate::encounter::Motion::Stationary));
            assert_eq!(e.bullet, 0);
            assert_eq!(rand::RngCore::next_u64(&mut world.rng.clone()), rng);
        }
        assert!(world.players.values().all(|p| p.character.alive));
        assert!(world.critical_deaths.is_empty());
        world.remove_entity(id);
    }
    // Preserve one tick while probing pre-entered phases without unsigned age underflow.
    world.now = world.now.max(10000);
    world.last_npc_step = world.now;
    for (age, offset) in [
        (6999_u64, 0_f32),
        (6999, 12.),
        (7000, 0.),
        (7000, 12.),
        (9999, 0.),
        (9999, 12.),
        (10000, 0.),
        (10000, 12.),
    ] {
        use crate::encounter::{Anchor, Motion};
        world.now += 50;
        let attack = age % 10000 >= 7000;
        let id = world.spawn_entity(0x91e, home, vec![]).unwrap();
        let position = Pos {
            x: home.x + offset,
            y: home.y,
        };
        world.position_entity(id, position).unwrap();
        let entered = if offset == 0. {
            Motion::Wander
        } else {
            Motion::Protect {
                anchor: Anchor::Home,
                distance: 1.,
            }
        };
        let activated = world.now - age;
        let e = world.entities.get_mut(&id).unwrap();
        e.quest.set(Some(id), id, &mut world.quest_ids);
        e.stage = Some(0);
        e.activated = Some(activated);
        e.next_shot = u64::MAX;
        assert_eq!(e.hp, 4000);
        e.motion = Some(entered);
        e.goal = Some(if offset == 0. {
            Pos {
                x: position.x + 1.,
                y: position.y,
            }
        } else {
            home
        });
        let rng = rand::RngCore::next_u64(&mut world.rng.clone());
        world.npc_step();
        let e = &world.entities[&id];
        assert_eq!(
            (e.pos.x, e.pos.y) != (position.x, position.y),
            !attack,
            "91e age={age}, offset={offset}"
        );
        let dx = if attack {
            0.
        } else if offset == 0. {
            0.3145
        } else {
            -0.4
        };
        assert!(
            (e.pos.x - position.x - dx).abs() <= 2. * f32::EPSILON * position.x,
            "91e age={age}, offset={offset}, dx={}",
            e.pos.x - position.x
        );
        assert_eq!(e.pos.y, position.y);
        assert_eq!(
            e.motion,
            Some(if attack { Motion::Stationary } else { entered })
        );
        assert_eq!(
            (e.home.x, e.home.y, *e.quest, e.hp, e.stage, e.activated),
            (home.x, home.y, Some(id), 4000, Some(0), Some(activated))
        );
        assert_eq!(e.next_shot, u64::MAX);
        assert_eq!(e.bullet, 0);
        assert!(!world.bullets.iter().any(|b| b.owner == id));
        assert_eq!(rand::RngCore::next_u64(&mut world.rng.clone()), rng);
        assert!(world.players.values().all(|p| p.character.alive));
        assert!(world.critical_deaths.is_empty());
        world.remove_entity(id);
    }
    for (offsets, distance) in [
        (vec![4., 1.25], 0.28675_f32),
        (vec![4., -2.5], 0.28675),
        (vec![1.25, 4.], 0.17575),
        (vec![], 0.17575),
    ] {
        world.now += 50;
        let boss = world.spawn_entity(0x91c, home, vec![]).unwrap();
        let mut members = vec![boss];
        for &offset in &offsets {
            members.push(
                world
                    .spawn_entity(
                        0x91d,
                        Pos {
                            x: home.x + offset,
                            y: home.y,
                        },
                        vec![],
                    )
                    .unwrap(),
            );
        }
        for member in &members {
            let e = world.entities.get_mut(member).unwrap();
            e.quest.set(Some(boss), *member, &mut world.quest_ids);
            e.stage = Some(0);
            e.activated = Some(world.now);
            e.next_shot = u64::MAX;
        }
        let e = world.entities.get_mut(&boss).unwrap();
        e.motion = Some(crate::encounter::Motion::Wander);
        e.goal = Some(Pos {
            x: home.x + 1.,
            y: home.y,
        });
        let hp = e.hp;
        world.npc_step();
        let e = &world.entities[&boss];
        assert!(
            (e.pos.x - home.x - distance).abs() <= 2. * f32::EPSILON * home.x,
            "Actual Lich must retain its chosen bearer and moving controls: offsets={offsets:?}, position={:?}",
            e.pos
        );
        assert_eq!(
            (e.pos.y, e.home.x, e.home.y, *e.quest, e.hp),
            (home.y, home.x, home.y, Some(boss), hp)
        );
        assert!(world.critical_deaths.is_empty());
        for member in members {
            world.remove_entity(member);
        }
    }
    let boss = world.spawn_entity(0x91c, home, vec![]).unwrap();
    let bearer = world.spawn_entity(0x91d, home, vec![]).unwrap();
    let foreign_boss = world.spawn_entity(0x91c, home, vec![]).unwrap();
    let foreign = world.spawn_entity(0x91d, home, vec![]).unwrap();
    let unowned = world.spawn_entity(0x91c, home, vec![]).unwrap();
    for (id, tag) in [
        (boss, boss),
        (bearer, boss),
        (foreign_boss, foreign_boss),
        (foreign, foreign_boss),
    ] {
        world
            .entities
            .get_mut(&id)
            .unwrap()
            .quest
            .set(Some(tag), id, &mut world.quest_ids);
    }
    let anchor = |id| crate::encounter::Anchor::QuestEntity(id);
    let position = world
        .orbit_anchor(boss, anchor(bearer), None::<Pos>)
        .unwrap();
    assert_eq!((position.x, position.y), (home.x, home.y));
    for (caller, target, error) in [
        (boss, foreign, "quest entity anchor ownership mismatch"),
        (
            unowned,
            bearer,
            "quest entity anchor lacks encounter ownership",
        ),
        (boss, i32::MAX, "own quest entity anchor absent"),
    ] {
        assert!(
            world
                .orbit_anchor(caller, anchor(target), None::<Pos>)
                .unwrap_err()
                .to_string()
                .contains(error)
        );
    }
    assert!(
        world
            .orbit_anchor(
                boss,
                crate::encounter::Anchor::BoundEntity(bearer),
                None::<Pos>
            )
            .is_err()
    );
    world.remove_entity(bearer);
    assert!(
        world
            .orbit_anchor(boss, anchor(bearer), None::<Pos>)
            .unwrap_err()
            .to_string()
            .contains("own quest entity anchor absent")
    );
    assert!(world.quest_members(boss, &[0x91d]).is_empty());
    for id in [boss, foreign_boss, foreign, unowned] {
        world.remove_entity(id);
    }
}

#[cfg(test)]
#[test]
fn ghost_king_towers_keep_authored_cells_and_hold_no_gods() {
    let path = std::path::Path::new("data");
    let content = Arc::new(Content::load(path).unwrap());
    let source = crate::terrain::realms::sources(path)
        .unwrap()
        .into_iter()
        .find(|s| s.name() == "classic-world-01-reconstruction")
        .unwrap();
    let authored = source.open(&content).unwrap();
    let authored_towers: Vec<_> = source
        .sites()
        .unwrap()
        .into_iter()
        .filter(|site| site.kind == 0x928)
        .map(|site| site.bounds)
        .collect();
    assert!(!authored_towers.is_empty());
    let directory = Directory::configured(path, content, 16, 1, Some(1), vec![source], 2).unwrap();
    let world = &directory.worlds[&-3];
    let kings: Vec<_> = world
        .entities
        .values()
        .filter(|e| e.kind == 0x928)
        .map(|e| e.pos)
        .collect();
    assert!(!kings.is_empty(), "world 1 has no Ghost King");
    assert!(world.ghost_towers.len() >= kings.len());
    assert!(authored_towers.iter().all(|bounds| {
        world
            .ghost_towers
            .iter()
            .any(|area| *bounds == [area.x, area.y, area.width, area.height])
    }));
    assert!(world.ghost_towers.iter().all(|area| {
        let fixed = authored_towers.contains(&[area.x, area.y, area.width, area.height]);
        (area.y..area.y + area.height).all(|y| {
            (area.x..area.x + area.width).all(|x| {
                let pos = Pos {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                };
                let cell = world.map.cell(pos.x, pos.y).unwrap();
                if fixed {
                    let original = authored.cell(pos.x, pos.y).unwrap();
                    assert_eq!(
                        (
                            cell.ground,
                            cell.object,
                            &cell.config,
                            cell.terrain,
                            cell.region
                        ),
                        (
                            original.ground,
                            original.object,
                            &original.config,
                            original.terrain,
                            original.region
                        )
                    );
                } else {
                    assert_ne!(cell.terrain, 1);
                }
                !world.family_ground(pos, 1)
            })
        })
    }));
    assert!(
        world
            .entities
            .values()
            .filter(|e| {
                world.content.objects[&e.kind].god && world.content.objects[&e.kind].terrain == 1
            })
            .all(|e| !world.inside_ghost_tower(e.pos))
    );
}
