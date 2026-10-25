use super::*;
use crate::{
    condition,
    dungeon::davy::{self, BOSS, KEYS, LIT, UNLIT},
};

#[derive(Default)]
pub(super) struct Locker {
    pub keys: u8,
    boss: i32,
    lamps: BTreeSet<i32>,
    doors: Vec<(i32, usize)>,
    respawns: Vec<(u16, Pos, Option<i32>, u64)>,
    all_lit_at: Option<u64>,
    vulnerable_until: Option<u64>,
    lights: BTreeMap<i32, (u64, Option<u64>)>,
}

impl World {
    pub(super) fn prepare_locker(&mut self) -> Result<()> {
        let bosses: Vec<_> = self
            .entities
            .values()
            .filter(|e| e.kind == BOSS)
            .map(|e| e.id)
            .collect();
        ensure!(
            bosses.len() == 1,
            "Davy Locker requires exactly one captain"
        );
        let boss = bosses[0];
        let lamps = self
            .entities
            .values()
            .filter(|e| e.kind == UNLIT)
            .map(|e| e.id)
            .collect::<BTreeSet<_>>();
        ensure!(
            lamps.len() == 8,
            "Davy Locker requires eight unlit lanterns"
        );
        for lamp in &lamps {
            ensure!(
                self.entities[lamp].pos.distance(self.entities[&boss].home) < 11.,
                "Davy lantern outside captain's room"
            );
        }
        let doors = self
            .entities
            .values()
            .filter_map(|e| davy::door_color(e.kind).map(|c| (e.id, c)))
            .collect();
        let respawns = self
            .actors()
            .filter(|e| matches!(e.kind, 0xe31 | 0xe3a))
            .map(|e| (e.kind, e.home, Some(e.id), 0))
            .collect();
        self.entities.get_mut(&boss).unwrap().published = false;
        self.locker = Some(Locker {
            boss,
            lamps,
            doors,
            respawns,
            ..Locker::default()
        });
        Ok(())
    }
    pub(super) fn locker_key_ui(&self, player: i32) {
        let Some(locker) = &self.locker else {
            return;
        };
        let p = &self.players[&player];
        for text in std::iter::once("showKeyUI").chain(
            davy::COLORS
                .iter()
                .enumerate()
                .filter_map(|(color, name)| (locker.keys & (1 << color) != 0).then_some(*name)),
        ) {
            let mut out = Writer::new();
            out.i32(0).utf(text);
            send(p, 9, out.0);
        }
    }
    fn locker_notice(&self, text: &str) {
        let mut out = Writer::new();
        out.i32(0).utf(text);
        for p in self.players.values() {
            send(p, 9, out.0.clone());
        }
    }
    pub(super) fn light_locker_lantern(&mut self, id: i32) -> Result<()> {
        let locker = self.locker.as_ref().context("lantern has no Locker")?;
        ensure!(
            locker.lamps.contains(&id) && self.entities[&id].kind == UNLIT,
            "unowned or already lit lantern"
        );
        let boss = locker.boss;
        let pos = self.entities[&id].pos;
        self.remove_entity(id);
        let lit = self.spawn_entity(LIT, pos, vec![])?;
        let e = self.entities.get_mut(&lit).unwrap();
        e.activated = Some(self.now);
        let locker = self.locker.as_mut().unwrap();
        locker.lamps.remove(&id);
        locker.lamps.insert(lit);
        locker.lights.insert(lit, (self.now, None));
        tracing::info!(
            lantern = id,
            lit,
            boss,
            "Locker lantern lit by ordinary damage"
        );
        Ok(())
    }
    fn reset_locker_lantern(&mut self, locker: &mut Locker, id: i32) -> Result<()> {
        let pos = self.entities[&id].home;
        self.remove_entity(id);
        let unlit = self.spawn_entity(UNLIT, pos, vec![])?;
        locker.lamps.remove(&id);
        locker.lamps.insert(unlit);
        ensure!(
            locker.lights.remove(&id).is_some(),
            "reset lantern lacks its own clock"
        );
        Ok(())
    }
    pub(super) fn locker_step(&mut self) -> Result<()> {
        let Some(mut locker) = self.locker.take() else {
            return Ok(());
        };
        let result = self.step_locker(&mut locker);
        self.locker = Some(locker);
        result
    }
    fn step_locker(&mut self, locker: &mut Locker) -> Result<()> {
        let players: Vec<_> = self
            .players
            .values()
            .filter(|p| p.active() && !p.paused)
            .map(|p| p.pos)
            .collect();
        if !players.is_empty() {
            self.locker_respawn(locker, &players)?;
        }
        let souls: Vec<_> = self
            .actors()
            .filter(|e| {
                e.kind == 0xe48
                    && e.stage == Some(1)
                    && e.expires.is_some_and(|at| self.now + 150 >= at)
            })
            .map(|e| e.id)
            .collect();
        for id in souls {
            self.locker_mechanics(id, None, 0)?;
        }
        for (color, key) in KEYS.iter().enumerate() {
            if locker.keys & (1 << color) != 0 {
                continue;
            }
            let found = self
                .actors()
                .find(|e| e.kind == *key && players.iter().any(|p| p.distance(e.pos) < 1.))
                .map(|e| e.id);
            if let Some(id) = found {
                self.remove_entity(id);
                locker.keys |= 1 << color;
                self.locker_notice(davy::COLORS[color]);
                tracing::info!(
                    key = id,
                    color = davy::COLORS[color],
                    "shared Locker key discovered"
                );
            }
        }
        let mut opened = Vec::new();
        for (id, color) in &locker.doors {
            if locker.keys & (1 << color) != 0
                && players
                    .iter()
                    .any(|p| p.distance(self.entities[id].pos) < 1.6)
            {
                opened.push(*id);
            }
        }
        // Remove both halves together; the key authorizes this door, not every door in the maze.
        for id in opened {
            let Some(door) = self.entities.get(&id) else {
                continue;
            };
            let pos = door.pos;
            let color = davy::door_color(self.entities[&id].kind).unwrap();
            let pair: Vec<_> = locker
                .doors
                .iter()
                .filter(|(other, c)| *c == color && self.entities[other].pos.distance(pos) < 1.5)
                .map(|(id, _)| *id)
                .collect();
            for id in pair {
                self.remove_entity(id);
                locker.doors.retain(|(door, _)| *door != id);
                tracing::info!(
                    door = id,
                    color = davy::COLORS[color],
                    "Locker door touched and opened"
                );
            }
        }
        ensure!(
            locker.lamps.len() == 8 && locker.lamps.iter().all(|id| self.entities.contains_key(id)),
            "Locker lost an encounter-owned lantern"
        );
        if !self.entities.contains_key(&locker.boss) {
            return self.locker_lamps(locker);
        }
        let boss = locker.boss;
        let home = self.entities[&boss].home;
        if self.entities[&boss].stage.is_none() {
            // Doors govern ordinary access without forbidding legitimate teleport shortcuts.
            if players.iter().any(|p| p.distance(home) < 6.) {
                let e = self.entities.get_mut(&boss).unwrap();
                e.stage = Some(0);
                e.activated = Some(self.now);
                e.published = true;
                e.size = 0;
                e.extra_stats = vec![(61, 4)];
                tracing::info!(boss, "Davy Jones appearing after room approach");
            } else {
                return self.locker_lamps(locker);
            }
        }
        let age = self.now
            - self.entities[&boss]
                .activated
                .context("Davy clock missing")?;
        if self.entities[&boss].stage == Some(0) {
            self.entities.get_mut(&boss).unwrap().size = (((age / 125) as i32) * 25).min(100);
            if age < 500 {
                return self.locker_lamps(locker);
            }
            self.entities.get_mut(&boss).unwrap().stage = Some(1);
            self.entities.get_mut(&boss).unwrap().extra_stats = vec![(61, 3)];
            tracing::info!(boss, "Davy Jones lantern cycle started");
        }
        let all_lit = locker.lamps.iter().all(|id| self.entities[id].kind == LIT);
        if self.entities[&boss].stage == Some(1) {
            if all_lit {
                let since = *locker.all_lit_at.get_or_insert(self.now);
                if self.now >= since + 1000 {
                    locker.vulnerable_until = Some(self.now + 5000);
                    let e = self.entities.get_mut(&boss).unwrap();
                    e.condition &= !condition::bit(condition::INVULNERABLE);
                    e.stage = Some(2);
                    e.extra_stats = vec![(61, 5)];
                    tracing::info!(
                        boss,
                        until = self.now + 5000,
                        "all eight lanterns opened Davy damage window"
                    );
                }
            } else {
                locker.all_lit_at = None;
            }
        }
        if locker
            .vulnerable_until
            .is_some_and(|until| self.now >= until)
        {
            locker.vulnerable_until = None;
            locker.all_lit_at = None;
            let e = self.entities.get_mut(&boss).unwrap();
            e.condition |= condition::bit(condition::INVULNERABLE);
            e.stage = Some(3);
            e.extra_stats = vec![(61, 4)];
            tracing::info!(
                boss,
                "Davy shield restored while awaiting lantern deactivation"
            );
        }
        self.locker_lamps(locker)?;
        if self.entities[&boss].stage == Some(3) && locker.lights.is_empty() {
            let e = self.entities.get_mut(&boss).unwrap();
            e.stage = Some(1);
            e.extra_stats = vec![(61, 3)];
            tracing::info!(boss, "Davy lantern cycle restarted");
        }
        if !players.is_empty()
            && self.entities[&boss].stage == Some(1)
            && self.now >= self.entities[&boss].next_aux
        {
            self.spawn_children(boss, 0xe50, 4, 5.)?;
            self.entities.get_mut(&boss).unwrap().next_aux = self.now
                + if self.entities[&boss].hp < self.content.objects[&BOSS].hp / 2 {
                    3000
                } else {
                    6000
                };
        }
        Ok(())
    }
    // FSoD@6fd20aa BehaviorDb.DavyJones.cs gives each lantern its own clock, not a captain-phase timer.
    fn locker_lamps(&mut self, locker: &mut Locker) -> Result<()> {
        // One snapshot makes simultaneous bursts independent of entity iteration order.
        let all_lit = locker.lamps.iter().all(|id| self.entities[id].kind == LIT);
        for (id, (lit_at, burst_at)) in locker.lights.clone() {
            if burst_at.is_some_and(|at| self.now >= at + 1000) {
                self.reset_locker_lantern(locker, id)?;
            } else if burst_at.is_none() && self.now >= lit_at + 5000 && all_lit {
                locker.lights.get_mut(&id).unwrap().1 = Some(self.now);
                let e = self
                    .entities
                    .get_mut(&id)
                    .context("lit lantern disappeared")?;
                e.stage = Some(2);
                e.activated = Some(self.now);
                if let Some(boss) = self.entities.get_mut(&locker.boss) {
                    boss.reaction_until = self.now + 3000;
                }
                tracing::info!(lantern = id, "Locker lantern paralyze burst armed");
            } else if burst_at.is_none() && self.now >= lit_at + 15000 {
                self.reset_locker_lantern(locker, id)?;
            }
        }
        Ok(())
    }
    fn locker_respawn(&mut self, locker: &mut Locker, players: &[Pos]) -> Result<()> {
        for (kind, pos, entity, at) in &mut locker.respawns {
            if entity.is_some_and(|id| !self.entities.contains_key(&id)) {
                *entity = None;
                *at = self.now + 5000;
            }
            if entity.is_none() && self.now >= *at && players.iter().any(|p| p.distance(*pos) < 12.)
            {
                ensure!(
                    self.actors()
                        .filter(|e| davy::ROSTER.contains(&e.kind))
                        .count()
                        < 256,
                    "Locker population budget exhausted"
                );
                let id = self.spawn_entity(*kind, *pos, vec![])?;
                *entity = Some(id);
                tracing::debug!(entity = id, kind, "bounded Locker ghost respawn");
            }
        }
        Ok(())
    }
    pub(super) fn locker_mechanics(
        &mut self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<()> {
        let kind = self.entities[&id].kind;
        if matches!(kind, 0xe31 | 0xe3a | 0xe50) {
            let e = self.entities.get_mut(&id).unwrap();
            let growth = if kind == 0xe3a { 35 } else { 30 };
            e.size = (growth + (elapsed / 50) as i32 * growth).min(120);
            if kind == 0xe50 && e.size == 120 {
                e.condition &= !condition::bit(condition::INVULNERABLE);
            }
        }
        if kind == 0xe43 {
            let e = self.entities.get_mut(&id).unwrap();
            e.condition &= !condition::bit(condition::ARMORED);
            if elapsed % 5700 >= 2850 {
                e.condition |= condition::bit(condition::ARMORED);
            }
        }
        if kind == 0xe45 && self.now >= self.entities[&id].next_aux {
            self.spawn_children(id, 0xe48, 4, 3.)?;
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 5000;
        }
        if kind == 0xe48 {
            let e = &self.entities[&id];
            let stage = e.stage.unwrap_or(0);
            let next = match stage {
                0 if target.is_some_and(|p| p.distance(e.pos) < 4.) => Some(1),
                1 if e.expires.is_some_and(|at| self.now + 150 >= at) => Some(2),
                _ => None,
            };
            if let Some(next) = next {
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(next);
                e.activated = Some(self.now);
                let lifetime = if next == 1 { 2350 } else { 150 };
                e.expires
                    .set(Some(self.now + lifetime), id, &mut self.timed_ids);
            }
        }
        Ok(())
    }
    pub(super) fn locker_movement(&self, id: i32, elapsed: u64) -> (crate::encounter::Motion, f32) {
        use crate::encounter::{Anchor, Motion, nr_speed};
        let e = &self.entities[&id];
        match e.kind {
            BOSS if e.stage == Some(1) && self.now < e.reaction_until => (
                Motion::Chase {
                    sight: 12.,
                    stop: 1.,
                },
                nr_speed(0.4),
            ),
            BOSS if e.stage != Some(1) => (
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 0.,
                },
                nr_speed(0.1),
            ),
            0xe31 if elapsed % 4700 >= 2850 => (Motion::Retreat { distance: 3. }, nr_speed(0.2)),
            0xe42 if elapsed % 4700 >= 2850 => (Motion::Circle, nr_speed(0.2)),
            0xe48 if e.stage == Some(1) => (
                Motion::Charge {
                    sight: 8.,
                    cooldown: 2000,
                    delay: 0,
                    stop: 0.,
                    idle: 0.,
                },
                nr_speed(0.5),
            ),
            0xe48 if e.stage == Some(2) => (Motion::Stationary, 0.),
            kind => {
                let p = crate::dungeon::davy::profile(kind).expect("validated Locker actor");
                (p.motion, p.speed)
            }
        }
    }
}
