use super::*;
use crate::{
    condition,
    dungeon::abyss::{self, FLOOR, IDOL, LAVA, MALPHAS},
};

pub(super) fn reward_groups(
    content: &Content,
    kind: u16,
    owners: &[Option<i32>],
    rng: &mut impl Rng,
) -> Vec<(Option<i32>, Vec<i32>)> {
    owners
        .iter()
        .map(|owner| {
            let mut items = Vec::new();
            for item in crate::loot::rewards(content, kind, rng) {
                let item = i32::from(item);
                if !items.contains(&item) || matches!(item, 0xa22 | 0xa23) {
                    items.push(item);
                }
            }
            (*owner, items)
        })
        .collect()
}

impl World {
    pub(super) fn abyss_loot(&mut self, defeated: &Entity) -> Result<()> {
        let mut characters = BTreeMap::new();
        for p in self
            .players
            .values()
            .filter(|p| p.character.alive && !p.death_pending && !p.departing)
        {
            let Some(&damage) = defeated.damage.get(&p.character.id).filter(|n| **n > 0) else {
                continue;
            };
            let account = p.character.account;
            ensure!(
                account > 0,
                "Abyss loot enemy={} kind={:#x} character={} invalid account={account}",
                defeated.id,
                defeated.kind,
                p.character.id
            );
            if let Some((previous, _)) = characters.insert(p.character.id, (account, damage)) {
                ensure!(
                    previous == account,
                    "Abyss loot enemy={} character={} conflicting accounts={previous}/{account}",
                    defeated.id,
                    p.character.id
                );
            }
        }
        let mut accounts = BTreeMap::<i32, i64>::new();
        for (account, damage) in characters.into_values() {
            *accounts.entry(account).or_default() += i64::from(damage);
        }
        let private = matches!(defeated.kind, MALPHAS | IDOL);
        let owners: Vec<_> = if private {
            let hp = i64::from(self.content.get(defeated.kind)?.hp);
            accounts
                .into_iter()
                .filter(|(_, damage)| damage * 100 >= hp)
                .map(|(account, _)| Some(account))
                .collect()
        } else if accounts.is_empty() {
            vec![]
        } else {
            vec![None]
        };
        let groups = reward_groups(&self.content, defeated.kind, &owners, &mut self.rng);
        self.abyss_package_loot(defeated, groups)
    }

    pub(super) fn abyss_package_loot(
        &mut self,
        defeated: &Entity,
        groups: Vec<(Option<i32>, Vec<i32>)>,
    ) -> Result<()> {
        for (owner, items) in groups {
            tracing::info!(
                enemy = defeated.id,
                kind = defeated.kind,
                owner,
                ?items,
                "Abyss legitimate defeat roll group"
            );
            self.loot_bags(defeated.pos, owner, &items).with_context(|| format!("Abyss loot packaging enemy={} kind={:#x} account={owner:?} items={items:?}", defeated.id, defeated.kind))?;
        }
        Ok(())
    }

    pub(super) fn change_ground(&mut self, i: usize, ground: u8) {
        self.change_grounds(&[(i, ground)]);
    }

    fn change_grounds(&mut self, changes: &[(usize, u8)]) {
        let width = self.map.width as usize;
        let map = Arc::make_mut(&mut self.map);
        for &(i, ground) in changes {
            map.cells[i].ground = ground;
        }
        for p in self.players.values() {
            let tiles: Vec<_> = changes
                .iter()
                .filter_map(|&(i, ground)| {
                    let tile = ((i % width) as i16, (i / width) as i16);
                    p.tiles.contains(&tile).then_some((tile, ground))
                })
                .collect();
            for batch in tiles.chunks(i16::MAX as usize) {
                let mut out = Writer::new();
                out.i16(batch.len() as i16);
                for &((x, y), ground) in batch {
                    out.i16(x).i16(y).u8(ground);
                }
                out.i16(0).i16(0);
                send(p, wire::UPDATE, out.0);
            }
        }
    }

    pub(super) fn idol_entrance(&mut self, home: Pos, restore: bool) -> Result<()> {
        let region = self
            .map
            .cell(home.x, home.y)
            .context("Idol home outside map")?
            .region;
        ensure!(region >= 16, "Idol has no generated treasure-room region");
        if !restore {
            let entrance: Vec<_> = self
                .map
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.region == region && c.ground == FLOOR)
                .map(|(i, _)| i)
                .collect();
            ensure!(
                !entrance.is_empty(),
                "Idol treasure room has no dry entrance"
            );
            for i in entrance {
                self.lava_patches.insert(i, (u64::MAX, FLOOR));
                self.change_ground(i, LAVA);
            }
        }
        if restore {
            let patches: Vec<_> = self
                .lava_patches
                .iter()
                .filter(|(i, _)| self.map.cells[**i].region == region)
                .map(|(i, (_, g))| (*i, *g))
                .collect();
            for &(i, _) in &patches {
                self.lava_patches.get_mut(&i).unwrap().0 = self.now;
            }
            self.change_grounds(&patches);
        }
        tracing::info!(region, restore, "Abyss Idol treasure entrance changed");
        Ok(())
    }

    pub(super) fn abyss_step(&mut self) {
        let restored: Vec<_> = self
            .lava_patches
            .iter()
            .filter(|(i, (deadline, _))| {
                *deadline <= self.now && self.map.cells[**i].ground == LAVA
            })
            .map(|(i, (_, ground))| (*i, *ground))
            .collect();
        for (i, ground) in restored {
            self.change_ground(i, ground);
        }
    }

    fn abyss_flash(&self, id: i32, color: u32, period: f32, duration: f32) {
        let mut out = Writer::new();
        out.u8(15)
            .i32(id)
            .f32(period)
            .f32((duration / period).round())
            .f32(0.)
            .f32(0.)
            .u32(color);
        for p in self.players.values().filter(|p| p.known.contains(&id)) {
            send(p, wire::SHOW_EFFECT, out.0.clone());
        }
    }

    pub(super) fn abyss_mechanics(
        &mut self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<()> {
        let target = target.into();
        let e = &self.entities[&id];
        let kind = e.kind;
        if kind == 0x909 {
            if e.stage.is_none()
                && (elapsed >= 5000
                    || e.hp < self.content.objects[&kind].hp
                    || target.is_some_and(|p| e.pos.distance(p) < 1.3))
            {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(1);
                e.next_aux = self.now + 600;
                e.expires.set(Some(self.now + 650), id, &mut self.timed_ids);
                self.abyss_flash(id, 0xffffffff, 0.1, 0.6);
            }
            return Ok(());
        }
        if kind == 0x908 {
            self.abyss_protector(id)?;
            return Ok(());
        }
        if kind == 0x90b {
            self.abyss_flamer(id)?;
            if self.abyss_flamer_due(id) {
                self.finish_abyss_flamer(id)?;
            }
            return Ok(());
        }
        if kind == MALPHAS {
            return self.abyss_boss(id, elapsed);
        }
        if kind != IDOL {
            return Ok(());
        }
        let home = self.entities[&id].home;
        if self.entities[&id].stage.is_none() {
            self.entities.get_mut(&id).unwrap().stage = Some(0);
            self.idol_entrance(home, false)?;
        }
        if kind == IDOL && self.now >= self.entities[&id].next_aux {
            let region = self.map.cell(home.x, home.y).unwrap().region;
            let mut candidates: Vec<_> = self
                .map
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.region == region && c.ground != LAVA && c.object.is_none())
                .map(|(i, _)| i)
                .collect();
            candidates.shuffle(&mut self.rng);
            for i in candidates.into_iter().take(6) {
                let old = self.map.cells[i].ground;
                self.lava_patches.insert(i, (self.now + 2000, old));
                self.change_ground(i, LAVA);
            }
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 2500;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(super) enum Runtime {
    Boss(Boss),
    Protector {
        owner: i32,
        orphan: bool,
    },
    Flamer {
        outer: bool,
        attacker: Option<i32>,
        terminal: Option<u64>,
        dismissal_age: Option<u64>,
        emitted: bool,
    },
}
#[derive(Clone, Debug)]
pub(super) struct Boss {
    pub(super) identity: u64,
    pub(super) entered: u64,
    pub(super) anchor: Pos,
    protector: u64,
    missile: Option<u64>,
    inner: Option<u64>,
    growth: Option<u64>,
    landings: Vec<Landing>,
}
#[derive(Clone, Debug)]
struct Landing {
    position: Pos,
    deadline: u64,
}

impl Runtime {
    pub(super) fn suspend(&mut self, delta: u64) {
        if let Self::Boss(b) = self {
            b.entered += delta;
            b.protector += delta;
            for clock in [&mut b.missile, &mut b.inner, &mut b.growth]
                .into_iter()
                .flatten()
            {
                *clock += delta;
            }
            for landing in &mut b.landings {
                landing.deadline += delta;
            }
        }
    }
}

impl World {
    pub(super) fn abyss_pending_count(&self) -> usize {
        self.actors()
            .map(|e| match &e.abyss {
                Some(Runtime::Boss(b)) => b.landings.len(),
                _ => 0,
            })
            .sum()
    }
    fn abyss_budget_available(&self) -> bool {
        self.summon_occupancy() < 256
    }
    pub(super) fn abyss_orphan(e: &Entity) -> bool {
        matches!(e.abyss, Some(Runtime::Protector { owner, orphan: true }) if e.kind == 0x908 && e.parent == Some(owner) && e.orbit.is_none() && e.goal.is_none_or(|goal| goal.distance(e.pos) == 0.))
    }
    pub(super) fn orphan_abyss(&mut self, removed: &Entity) {
        if removed.kind != MALPHAS {
            return;
        }
        if let Some(Runtime::Boss(b)) = &removed.abyss {
            tracing::debug!(
                parent = removed.id,
                cancelled = b.landings.len(),
                "Abyss pending births cancelled by parent removal"
            );
        }
        for child in self
            .entities
            .matching_mut(|e| e.kind == 0x908 && e.parent == Some(removed.id))
        {
            if let Some(Runtime::Protector { owner, orphan }) = &mut child.abyss
                && *owner == removed.id
            {
                *orphan = true;
                child.orbit = None;
                child.goal = None;
                child.motion = None;
                tracing::info!(
                    entity = child.id,
                    parent = removed.id,
                    time_ms = self.now,
                    "Abyss Protector became stationary orphan"
                );
            } else {
                tracing::error!(
                    entity = child.id,
                    parent = removed.id,
                    "Abyss Protector removal found uninitialized ownership"
                );
            }
        }
    }
    fn abyss_protector(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let owner = e
            .parent
            .with_context(|| format!("Abyss Protector {id} has no parent"))?;
        if Self::abyss_orphan(e) {
            ensure!(
                !self.entities.contains_key(&owner),
                "Abyss orphan {id} still has live parent {owner}"
            );
            return Ok(());
        }
        let parent = self
            .entities
            .get(&owner)
            .with_context(|| format!("Abyss Protector {id} missing Malphas parent {owner}"))?;
        ensure!(
            parent.kind == MALPHAS,
            "Abyss Protector {id} parent {owner} is not Malphas"
        );
        match e.abyss {
            None => {}
            Some(Runtime::Protector {
                owner: lineage,
                orphan: false,
            }) => ensure!(
                lineage == owner,
                "Abyss Protector {id} parent/lineage mismatch"
            ),
            _ => bail!("Abyss Protector {id} invalid runtime state"),
        }
        let e = self.entities.get_mut(&id).unwrap();
        e.abyss = Some(Runtime::Protector {
            owner,
            orphan: false,
        });
        e.orbit = Some(realm_events::Orbit {
            speed: 6.,
            radius: 9.,
            direction: 1.,
        });
        Ok(())
    }
    fn abyss_flamer(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        if let Some(owner) = *e.parent {
            ensure!(
                self.entities.get(&owner).is_some_and(|p| p.kind == MALPHAS),
                "Abyss Flamer {id} invalid Malphas parent {owner}"
            );
        }
        if e.abyss.is_none() {
            let dismissal_age = e
                .parent
                .map(|owner| {
                    let activated = self.entities[&owner]
                        .activated
                        .context("Abyss Flamer parent lacks activation")?;
                    Ok::<u64, anyhow::Error>(
                        self.now
                            .checked_sub(activated)
                            .context("Abyss Flamer parent activation is in the future")?
                            / 50200
                            * 50200
                            + 47000,
                    )
                })
                .transpose()?;
            self.entities.get_mut(&id).unwrap().abyss = Some(Runtime::Flamer {
                outer: false,
                attacker: None,
                terminal: None,
                dismissal_age,
                emitted: false,
            });
        }
        ensure!(
            matches!(self.entities[&id].abyss, Some(Runtime::Flamer { .. })),
            "Abyss Flamer {id} invalid runtime state"
        );
        Ok(())
    }
    pub(super) fn abyss_flamer_armed(e: &Entity) -> bool {
        matches!(
            e.abyss,
            Some(Runtime::Flamer {
                terminal: Some(_),
                ..
            })
        )
    }
    pub(super) fn abyss_flamer_damage(
        &mut self,
        id: i32,
        owner: i32,
        actual: i32,
        lethal: bool,
    ) -> Result<()> {
        if actual == 0 {
            return Ok(());
        }
        self.abyss_flamer(id)?;
        let attacker = if owner == 0 {
            None
        } else {
            Some(
                self.players
                    .get(&owner)
                    .context("Abyss Flamer damage has no real attacker")?
                    .character
                    .id,
            )
        };
        let e: &mut EntityData = self.entities.get_mut(&id).unwrap();
        tracing::debug!(entity=id, parent=?e.parent, owner, ?attacker, actual, hp=e.hp, lethal, "Abyss Flamer accepted damage");
        let Some(Runtime::Flamer {
            attacker: last,
            terminal,
            ..
        }) = &mut e.abyss
        else {
            unreachable!()
        };
        // Companion damage has no player credit and must not erase a real contributor.
        if attacker.is_some() {
            *last = attacker;
        }
        if lethal {
            *terminal = Some(self.now);
            self.abyss_flamer_burst(id, "lethal")?;
        } else if e.hp <= 200 && terminal.is_none() {
            *terminal = Some(self.now + 300);
            e.goal = None;
            self.abyss_flash(id, 0xffff0000, 0.3, 0.3);
            tracing::info!(entity=id, parent=?self.entities[&id].parent, deadline=self.now+300, ?attacker, hp=self.entities[&id].hp, "Abyss Flamer terminal armed");
        }
        Ok(())
    }
    fn abyss_flamer_burst(&mut self, id: i32, cause: &str) -> Result<()> {
        ensure!(
            self.bullets.len() + self.detached_bullets + 8 <= 2048,
            "Abyss Flamer {id} terminal projectile budget exhausted"
        );
        let schedule = self
            .npc_attack_schedule(id, 0)?
            .context("Abyss Flamer terminal schedule missing")?;
        let shots = self.scheduled_attacks(id, None, &schedule)?;
        ensure!(
            shots.len() == 8,
            "Abyss Flamer {id} terminal channel must emit eight rays, got {}",
            shots.len()
        );
        let packets = self.emit_enemy_shots(id, shots);
        for packet in packets {
            self.send_enemy_shot(id, packet);
        }
        let e: &mut EntityData = self.entities.get_mut(&id).unwrap();
        let Some(Runtime::Flamer {
            emitted, terminal, ..
        }) = &mut e.abyss
        else {
            bail!("Abyss Flamer {id} terminal runtime missing")
        };
        ensure!(!*emitted, "Abyss Flamer {id} terminal emitted twice");
        *emitted = true;
        tracing::info!(entity=id, parent=?e.parent, deadline=?terminal, cause, rays=8, hp=e.hp, time_ms=self.now, "Abyss Flamer terminal burst emitted");
        Ok(())
    }
    fn abyss_flamer_due(&self, id: i32) -> bool {
        let e = &self.entities[&id];
        let Some(Runtime::Flamer {
            terminal: Some(deadline),
            emitted: false,
            ..
        }) = e.abyss
        else {
            return false;
        };
        if deadline > self.now {
            return false;
        }
        if let Some(owner) = *e.parent {
            let Some(parent) = self.entities.get(&owner) else {
                return false;
            };
            if parent.kind != MALPHAS {
                return false;
            }
            let Some(Runtime::Flamer {
                dismissal_age: Some(dismissal_age),
                ..
            }) = e.abyss
            else {
                return false;
            };
            let Some(activated) = parent.activated else {
                return false;
            };
            if deadline > activated + dismissal_age {
                return false;
            }
        }
        true
    }
    fn finish_abyss_flamer(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let Some(Runtime::Flamer { attacker, .. }) = e.abyss else {
            bail!("Abyss Flamer {id} timed defeat lacks terminal runtime")
        };
        if let Some(attacker) = attacker {
            ensure!(
                e.damage.get(&attacker).is_some_and(|damage| *damage > 0),
                "Abyss Flamer {id} timed defeat lacks attributed damage for {attacker}"
            );
        }
        self.abyss_flamer_burst(id, "threshold-deadline")?;
        self.entities.get_mut(&id).unwrap().hp = 0;
        if let Some(p) = self.players.values_mut().find(|p| {
            Some(p.character.id) == attacker
                && p.character.alive
                && !p.death_pending
                && !p.departing
        }) {
            p.character.kills += 1;
        }
        let mut notice = Writer::new();
        notice.i32(id).u8(0).i16(0).u8(1).u8(0).i32(0);
        for p in self.players.values().filter(|p| p.known.contains(&id)) {
            send(p, 47, notice.0.clone());
        }
        self.defeat_entity(id)?;
        tracing::info!(
            entity = id,
            ?attacker,
            time_ms = self.now,
            "Abyss Flamer timed legitimate defeat"
        );
        Ok(())
    }
    pub(super) fn abyss_terminal_step(&mut self) {
        let ids: Vec<_> = self
            .abyss_flamer_ids
            .iter()
            .copied()
            .filter(|id| self.abyss_flamer_due(*id))
            .collect();
        for id in ids {
            if let Err(error) = self.finish_abyss_flamer(id) {
                tracing::error!(entity=id,%error,"Abyss terminal deadline refused");
            }
        }
    }
    fn abyss_birth(&mut self, owner: i32, kind: u16, position: Pos, outer: bool) -> Result<i32> {
        ensure!(
            self.walkable(position),
            "Abyss birth owner={owner} kind={kind:#x} blocked position=({}, {})",
            position.x,
            position.y
        );
        let id = self.spawn_entity(kind, position, vec![])?;
        let e = self.entities.get_mut(&id).unwrap();
        e.parent.set(Some(owner), id, &mut self.timed_ids);
        e.activated = Some(self.now);
        match kind {
            0x908 => {
                self.abyss_protector(id)?;
            }
            0x909 => {
                self.entities.get_mut(&id).unwrap().expires.set(
                    Some(self.now + 5650),
                    id,
                    &mut self.timed_ids,
                );
            }
            0x90b => {
                self.abyss_flamer(id)?;
                let Some(Runtime::Flamer {
                    outer: birth_outer, ..
                }) = &mut self.entities.get_mut(&id).unwrap().abyss
                else {
                    unreachable!()
                };
                *birth_outer = outer;
            }
            _ => bail!("invalid Abyss birth kind {kind:#x}"),
        }
        tracing::info!(
            entity = id,
            parent = owner,
            kind,
            outer,
            deadline = self.now,
            "Abyss owned birth landed"
        );
        Ok(id)
    }
    fn abyss_direct_birth(
        &mut self,
        owner: i32,
        kind: u16,
        limit: usize,
        radius: f32,
    ) -> Result<()> {
        let count = self
            .actors()
            .filter(|e| {
                e.parent == Some(owner)
                    && e.kind == kind
                    && (kind != 0x90b
                        || matches!(e.abyss, Some(Runtime::Flamer { outer: false, .. })))
            })
            .count();
        if count >= limit {
            return Ok(());
        }
        if !self.abyss_budget_available() {
            tracing::warn!(
                parent = owner,
                kind,
                deadline = self.now,
                "Abyss birth refused at shared budget"
            );
            return Ok(());
        }
        let position=self.child_positions(owner,radius)?.first().copied().with_context(||format!("Abyss birth owner={owner} kind={kind:#x} no walkable ring position radius={radius}"))?;
        self.abyss_birth(owner, kind, position, false)?;
        Ok(())
    }
    fn abyss_throw_flamers(&mut self, owner: i32) -> Result<()> {
        let home = self.entities[&owner].pos;
        let positions = [
            Pos {
                x: home.x + 6.,
                y: home.y,
            },
            Pos {
                x: home.x,
                y: home.y + 6.,
            },
            Pos {
                x: home.x - 6.,
                y: home.y,
            },
            Pos {
                x: home.x,
                y: home.y - 6.,
            },
        ];
        for position in positions {
            ensure!(
                self.walkable(position),
                "Abyss throw admission owner={owner} kind=0x90b blocked position=({}, {})",
                position.x,
                position.y
            );
        }
        for position in positions {
            if !self.abyss_budget_available() {
                tracing::warn!(
                    parent = owner,
                    deadline = self.now,
                    "Abyss throw refused at shared budget"
                );
                break;
            }
            let Some(Runtime::Boss(b)) = &mut self.entities.get_mut(&owner).unwrap().abyss else {
                bail!("Abyss throw owner {owner} runtime missing")
            };
            b.landings.push(Landing {
                position,
                deadline: self.now + 1500,
            });
            self.telegraph_throw(owner, position, 0xffff0000);
            tracing::info!(
                parent = owner,
                kind = 0x90b,
                deadline = self.now + 1500,
                x = position.x,
                y = position.y,
                "Abyss throw admitted and reserved"
            );
        }
        Ok(())
    }
    fn abyss_landings(&mut self, owner: i32) -> Result<()> {
        let Some(Runtime::Boss(b)) = &mut self.entities.get_mut(&owner).unwrap().abyss else {
            bail!("Abyss landing owner {owner} runtime missing")
        };
        let landings = std::mem::take(&mut b.landings);
        let mut error = None;
        for landing in landings {
            if self.now < landing.deadline {
                let Some(Runtime::Boss(b)) = &mut self.entities.get_mut(&owner).unwrap().abyss
                else {
                    unreachable!()
                };
                b.landings.push(landing);
            } else if !self.walkable(landing.position) {
                let failure = anyhow::anyhow!(
                    "Abyss blocked landing owner={owner} kind=0x90b position=({}, {}) deadline={}",
                    landing.position.x,
                    landing.position.y,
                    landing.deadline
                );
                tracing::error!(parent=owner,deadline=landing.deadline,%failure,"Abyss landing reservation released");
                error.get_or_insert(failure);
            } else {
                self.abyss_birth(owner, 0x90b, landing.position, true)?;
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(())
    }
    fn abyss_boss(&mut self, id: i32, elapsed: u64) -> Result<()> {
        let phase = abyss::phase(elapsed);
        let identity = elapsed / 50200 * 7 + phase;
        let age = elapsed % 50200 - [0, 9000, 11500, 23500, 36500, 39000, 47000][phase as usize];
        let entered = self
            .now
            .checked_sub(age)
            .context("Abyss phase age exceeds world time")?;
        let activation = self
            .now
            .checked_sub(elapsed)
            .context("Abyss encounter age exceeds world time")?;
        let e = self.entities.get_mut(&id).unwrap();
        let pos = e.pos;
        let e: &mut EntityData = e;
        e.activated.get_or_insert(activation);
        if e.abyss.is_none() {
            e.abyss = Some(Runtime::Boss(Boss {
                identity: u64::MAX,
                entered,
                anchor: pos,
                protector: activation + 1000,
                missile: None,
                inner: None,
                growth: None,
                landings: vec![],
            }));
        }
        let Some(Runtime::Boss(b)) = &mut e.abyss else {
            bail!("Abyss Malphas {id} invalid runtime state")
        };
        let changed = b.identity != identity;
        if changed {
            tracing::info!(entity=id,parent=?e.parent,identity,phase,deadline=entered,x=pos.x,y=pos.y,cancelled=b.landings.len(),"Abyss encounter phase entered");
            b.identity = identity;
            b.entered = entered;
            b.anchor = pos;
            b.missile = (phase == 0).then_some(entered + 1800);
            b.inner = (phase == 5).then_some(entered + 500);
            b.growth = matches!(phase, 2..=5).then_some(self.now);
            b.landings.clear();
            e.goal = None;
        }
        e.stage = Some(phase);
        e.condition &= !condition::bit(condition::INVULNERABLE);
        let shield = match phase {
            0 => !(2000..6000).contains(&age),
            1 | 4 | 5 => true,
            3 => age < 3200 || age.saturating_sub(1800) / 1400 % 3 != 1,
            _ => false,
        };
        if shield {
            e.condition |= condition::bit(condition::INVULNERABLE);
        }
        if b.growth.is_some_and(|deadline| self.now >= deadline) {
            let (target, delta) = match phase {
                2 => (30, 11),
                3 => (140, 11),
                4 => (100, 11),
                5 => (100, 14),
                _ => unreachable!(),
            };
            e.size += (target - e.size).clamp(-delta, delta);
            b.growth = (e.size != target).then_some(self.now + 150);
        }
        if changed {
            match phase {
                1 | 4 => self.abyss_flash(id, 0xff00ff00, 0.25, 2.5),
                5 => self.abyss_throw_flamers(id)?,
                6 => {
                    let children: Vec<_> = self
                        .actors()
                        .filter(|e| e.kind == 0x90b && e.parent == Some(id))
                        .map(|e| e.id)
                        .collect();
                    for child in children {
                        if self.abyss_flamer_due(child) {
                            self.finish_abyss_flamer(child)?;
                        } else {
                            self.remove_entity(child);
                        }
                    }
                    self.abyss_flash(id, 0xff484848, 0.6, 3.2);
                }
                _ => {}
            }
        }
        self.abyss_landings(id)?;
        let Some(Runtime::Boss(b)) = &mut self.entities.get_mut(&id).unwrap().abyss else {
            unreachable!()
        };
        let protector = self.now >= b.protector;
        if protector {
            b.protector = self.now + 1000;
        }
        let missile = b.missile.is_some_and(|deadline| self.now >= deadline);
        if missile {
            b.missile = Some(self.now + 1800);
        }
        let inner = b.inner.is_some_and(|deadline| self.now >= deadline);
        if inner {
            b.inner = Some(self.now + 500);
        }
        if protector {
            self.abyss_direct_birth(id, 0x908, 3, 9.)?;
        }
        if missile {
            self.abyss_direct_birth(id, 0x909, 4, 1.)?;
        }
        if inner {
            self.abyss_direct_birth(id, 0x90b, 5, 0.7)?;
        }
        Ok(())
    }
    pub(super) fn abyss_movement(&self, id: i32) -> Result<(crate::encounter::Motion, f32)> {
        use crate::encounter::Motion;
        let e = &self.entities[&id];
        if e.kind == 0x908 {
            ensure!(
                matches!(e.abyss, Some(Runtime::Protector { .. })),
                "Abyss Protector {id} movement lacks ownership"
            );
            if Self::abyss_orphan(e) {
                return Ok((Motion::Stationary, 0.));
            }
        }
        if e.kind == 0x909 && e.stage == Some(1) || e.kind == 0x90b && Self::abyss_flamer_armed(e) {
            return Ok((Motion::Stationary, 0.));
        }
        if e.kind == MALPHAS {
            let Some(Runtime::Boss(b)) = &e.abyss else {
                bail!("Abyss Malphas {id} movement lacks phase")
            };
            return Ok((
                Motion::Wander,
                [3.6075, 4.44, 7.77, 2.775, 4.44, 4.44, 4.44][(b.identity % 7) as usize],
            ));
        }
        let p = abyss::profile(e.kind);
        Ok((p.motion, p.speed))
    }
    pub(super) fn abyss_malphas_goal(&mut self, id: i32, target: Option<Pos>) -> Result<Pos> {
        let e = &self.entities[&id];
        let Some(Runtime::Boss(b)) = &e.abyss else {
            bail!("Abyss Malphas {id} goal lacks phase")
        };
        let anchor = b.anchor;
        let phase = b.identity % 7;
        if e.pos.distance(anchor) > 5. {
            return Ok(anchor);
        }
        let chase = match phase {
            0 => Some(8.),
            5 => Some(9.),
            _ => None,
        };
        if let Some(target) =
            target.filter(|target| chase.is_some_and(|range| e.pos.distance(*target) < range))
        {
            return Ok(if e.pos.distance(target) > 2. {
                target
            } else {
                e.pos
            });
        }
        if phase == 6
            && let Some(target) = target.filter(|target| e.pos.distance(*target) < 4.)
        {
            let angle = (e.pos.y - target.y).atan2(e.pos.x - target.x);
            return Ok(Pos {
                x: target.x + 4. * angle.cos(),
                y: target.y + 4. * angle.sin(),
            });
        }
        if let Some(goal) = e
            .goal
            .filter(|goal| goal.distance(e.pos) >= 0.1 && goal.distance(anchor) <= 2.01)
        {
            return Ok(goal);
        }
        let angle = self.rng.gen_range(0. ..std::f32::consts::TAU);
        let radius = self.rng.gen_range(0. ..2.);
        Ok(Pos {
            x: anchor.x + radius * angle.cos(),
            y: anchor.y + radius * angle.sin(),
        })
    }
}
