// GPL-3.0 reference: realm-src@1855c017eab870d13f0b61b1cf0b9e66a01922ff, BehaviorDb.{Hermit,OceanTrench}.cs; data/REALM-SRC-LICENSE.
use super::realm_events::Orbit;
use super::*;
use crate::{
    condition,
    dungeon::trench,
    encounter::{Anchor, Motion, nr_speed as speed},
};
use std::f32::consts::TAU;

fn due(time: u64, interval: u64) -> bool {
    time % interval < 50
}
fn fan(out: &mut Vec<(u8, f32)>, index: u8, heading: f32, count: usize, angle: f32) {
    for n in 0..count {
        out.push((
            index,
            heading + (n as f32 - (count - 1) as f32 / 2.) * angle.to_radians(),
        ));
    }
}
fn ring(out: &mut Vec<(u8, f32)>, index: u8, count: usize, offset: f32) {
    for n in 0..count {
        out.push((index, offset + n as f32 * TAU / count as f32));
    }
}
impl World {
    fn ocean_stage(&mut self, id: i32, stage: u64) {
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(stage);
        e.activated = Some(self.now);
        e.next_shot = self.now;
        tracing::debug!(
            entity = id,
            kind = e.kind,
            stage,
            "ocean encounter phase entered"
        );
    }
    fn ocean_children(&self, id: i32, kind: u16) -> Vec<i32> {
        self.actors()
            .filter(|e| e.parent == Some(id) && e.kind == kind)
            .map(|e| e.id)
            .collect()
    }
    fn ocean_spawn(&mut self, parent: i32, kind: u16, pos: Pos) -> Result<i32> {
        crate::encounter::validate(&self.content, kind)?;
        crate::loot::validate(&self.content, kind)?;
        ensure!(
            self.actors()
                .filter(|e| e.parent.is_some() || e.quest.is_some())
                .count()
                + self.named_births.len()
                + self.abyss_pending_count()
                < 256,
            "ocean summon budget exhausted"
        );
        ensure!(
            self.ocean_walkable(kind, pos),
            "ocean summon site is blocked: {kind:#x} at {},{}",
            pos.x,
            pos.y
        );
        let id = self.insert_entity(kind, pos, vec![])?;
        let quest = self.entities[&parent].quest;
        let e = self.entities.get_mut(&id).unwrap();
        e.parent.set(Some(parent), id, &mut self.timed_ids);
        e.quest = quest;
        Ok(id)
    }
    pub(super) fn ocean_reward_position(&self, e: &Entity) -> Result<Pos> {
        // Floating enemies must leave collectible rewards on reachable shallows, not deep water.
        if e.kind == trench::HERMIT {
            return [(0., 5.), (5., 0.), (-5., 0.), (0., -5.)]
                .into_iter()
                .map(|(x, y)| Pos {
                    x: e.home.x + x,
                    y: e.home.y + y,
                })
                .find(|p| self.walkable(*p))
                .context("Hermit has no reachable reward site");
        }
        if e.kind == 0xd62 && !self.walkable(e.pos) {
            ensure!(
                self.walkable(e.home),
                "floating Hermit minion has no reachable reward site"
            );
            return Ok(e.home);
        }
        Ok(e.pos)
    }
    pub(super) fn ocean_bag_position(&mut self, e: &Entity) -> Result<Pos> {
        if e.kind != trench::HERMIT {
            return self.ocean_reward_position(e);
        }
        // Release 7.0 server data rolls Hermit loot at eight drop locations: https://www.realmeye.com/forum/t/guide-to-loot-and-drop-rates/60740
        let (x, y) = *super::ocean_setpiece::HERMIT_SPAWNERS
            .choose(&mut self.rng)
            .expect("eight Hermit drop sites");
        let site = Pos {
            x: e.home.x + x as f32 - 15.5,
            y: e.home.y + y as f32 - 13.,
        };
        ensure!(
            self.walkable(site),
            "Hermit drop site {},{} is not collectible",
            site.x,
            site.y
        );
        Ok(site)
    }
    fn ocean_wave(&mut self, id: i32, kind: u16) -> Result<()> {
        let positions: Vec<_> = self
            .ocean_children(id, 0xd65)
            .into_iter()
            .map(|s| self.entities[&s].pos)
            .collect();
        ensure!(
            positions.len() == 8,
            "Hermit requires its eight owned tentacle spawners"
        );
        ensure!(
            self.actors()
                .filter(|e| e.parent.is_some() || e.quest.is_some())
                .count()
                + self.named_births.len()
                + self.abyss_pending_count()
                + positions.len()
                <= 256,
            "Hermit wave exceeds summon budget"
        );
        ensure!(
            positions.iter().all(|p| self.ocean_walkable(kind, *p)),
            "Hermit wave has a blocked spawner"
        );
        for position in positions {
            self.ocean_spawn(id, kind, position)?;
        }
        Ok(())
    }
    pub(super) fn ocean_walkable(&self, kind: u16, pos: Pos) -> bool {
        self.walkable(pos)
            || ((0xd61..=0xd64).contains(&kind)
                && self
                    .map
                    .cell(pos.x, pos.y)
                    .is_some_and(|c| c.ground == 0xbc)
                && self
                    .entities_at(pos)
                    .all(|e| !self.content.objects[&e.kind].blocks))
    }
    fn ocean_toss(
        &mut self,
        id: i32,
        kind: u16,
        count: usize,
        radius: f32,
        offset: f32,
    ) -> Result<()> {
        let pos = self.entities[&id].pos;
        let mut positions = Vec::with_capacity(count);
        for n in 0..count {
            let angle = if kind == 0x1705 {
                ensure!(
                    count == 3,
                    "Coral Gift requires its selected three-point pattern"
                );
                [45f32, 135., 235.][n].to_radians()
            } else {
                offset + n as f32 * TAU / count as f32
            };
            // A toss must stop before cave walls rather than creating an unreachable summon.
            let steps = (radius * 10.).ceil() as usize;
            let destination = (0..=steps)
                .map(|n| {
                    let distance = radius * n as f32 / steps as f32;
                    Pos {
                        x: pos.x + distance * angle.cos(),
                        y: pos.y + distance * angle.sin(),
                    }
                })
                .take_while(|p| self.ocean_walkable(kind, *p))
                .last()
                .context("ocean toss has no unobstructed origin")?;
            if destination.distance(pos) + 0.05 < radius {
                tracing::debug!(
                    parent = id,
                    kind,
                    radius,
                    actual_radius = destination.distance(pos),
                    "ocean toss stopped at cave boundary"
                );
            }
            positions.push(destination);
        }
        ensure!(
            self.actors()
                .filter(|e| e.parent.is_some() || e.quest.is_some())
                .count()
                + self.named_births.len()
                + self.abyss_pending_count()
                + count
                <= 256,
            "ocean toss exceeds summon budget"
        );
        for position in positions {
            self.ocean_spawn(id, kind, position)?;
        }
        Ok(())
    }
    pub(super) fn ocean_mechanics(
        &mut self,
        id: i32,
        _target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<()> {
        let kind = self.entities[&id].kind;
        if matches!(kind, 0xd62..=0xd64) && self.entities[&id].orbit.is_none() {
            let (value, radius) = if kind == 0xd63 {
                (0.3, 6.)
            } else {
                (
                    self.rng.gen_range(0.4..0.8),
                    if kind == 0xd62 {
                        self.rng.gen_range(8.5..11.5)
                    } else {
                        self.rng.gen_range(5.5..6.5)
                    },
                )
            };
            self.entities.get_mut(&id).unwrap().orbit = Some(Orbit {
                speed: speed(value),
                radius,
                direction: -1.,
            });
        }
        let e = &self.entities[&id];
        let stage = e.stage;
        match kind {
            trench::HERMIT => match stage {
                Some(0) => {
                    let minions = self.ocean_children(id, 0xd62).len();
                    let missing = 20usize.saturating_sub(minions).min(8);
                    ensure!(
                        self.summon_occupancy() + 8 + missing <= self.summon_limit(),
                        "Hermit initial wave and minions exceed summon budget"
                    );
                    // The mandatory wave cannot commit before its companion minions can be admitted.
                    ensure!(
                        (0..32)
                            .filter(|n| {
                                let a = *n as f32 * TAU / 32.;
                                self.walkable(Pos {
                                    x: e.pos.x + 10. * a.cos(),
                                    y: e.pos.y + 10. * a.sin(),
                                })
                            })
                            .count()
                            >= missing,
                        "Hermit initial minions have insufficient walkable summon positions"
                    );
                    self.ocean_wave(id, 0xd64)?;
                    self.spawn_children(id, 0xd62, (minions + 8).min(20), 10.)?;
                    self.ocean_stage(id, 1);
                    let e = self.entities.get_mut(&id).unwrap();
                    e.condition |= condition::bit(condition::INVINCIBLE);
                    e.extra_stats = vec![(61, 2)];
                }
                Some(1) if elapsed >= 1000 => {
                    let maximum = (self.ocean_children(id, 0xd62).len() + 8).min(20);
                    self.spawn_children(id, 0xd62, maximum, 10.)?;
                    self.ocean_stage(id, 2);
                    self.entities.get_mut(&id).unwrap().extra_stats = vec![(61, 3)];
                }
                Some(2) if self.ocean_children(id, 0xd64).is_empty() => {
                    let maximum = (self.ocean_children(id, 0xd62).len() + 8).min(28);
                    self.spawn_children(id, 0xd62, maximum, 10.)?;
                    self.ocean_stage(id, 3);
                    self.entities.get_mut(&id).unwrap().extra_stats = vec![(61, 2)];
                }
                Some(3) if elapsed >= 100 => {
                    self.ocean_wave(id, 0xd63)?;
                    self.ocean_stage(id, 4);
                    let e = self.entities.get_mut(&id).unwrap();
                    e.condition &= !condition::bit(condition::INVINCIBLE);
                    e.extra_stats = vec![(61, 0)];
                }
                Some(4) if elapsed >= 6000 => {
                    for child in self.ocean_children(id, 0xd63) {
                        self.ocean_stage(child, 2);
                        self.entities.get_mut(&child).unwrap().expires.set(
                            Some(self.now + 50),
                            child,
                            &mut self.timed_ids,
                        );
                    }
                    self.ocean_stage(id, 5);
                }
                Some(5) if elapsed >= 6000 => {
                    self.ocean_stage(id, 0);
                    self.entities.get_mut(&id).unwrap().condition |=
                        condition::bit(condition::INVINCIBLE);
                }
                _ => {}
            },
            0x1706 => {
                if stage.is_none() {
                    self.ocean_toss(id, 0x1707, 4, 14., 0.)?;
                    self.ocean_stage(id, 0);
                } else if stage == Some(0) && e.hp < self.content.objects[&kind].hp {
                    let wait = if e.hp < self.content.objects[&kind].hp / 2 {
                        500
                    } else {
                        3000
                    };
                    self.ocean_stage(id, 1);
                    self.entities.get_mut(&id).unwrap().next_aux = self.now + wait;
                } else if stage == Some(0) && self.ocean_children(id, 0x1707).is_empty() {
                    self.ocean_toss(id, 0x1707, 4, 14., 0.)?;
                } else if stage == Some(1)
                    && e.hp < self.content.objects[&kind].hp / 2
                    && e.next_aux > self.now + 500
                {
                    self.entities.get_mut(&id).unwrap().next_aux = self.now + 500;
                } else if stage == Some(1) && self.now >= e.next_aux {
                    let strong = e.hp < self.content.objects[&kind].hp / 2;
                    let next = self.rng.gen_range(2..=5);
                    // A hit mid-volley must not switch the already selected low/high-health pattern.
                    self.ocean_stage(id, next + if strong { 4 } else { 0 });
                    if next == 3 {
                        let angle = self.rng.gen_range(0. ..TAU);
                        self.ocean_toss(id, 0x1702, 1, 5., angle)?;
                    }
                    if next == 5 {
                        self.ocean_toss(id, 0x1702, 4, 5., TAU / 8.)?;
                    }
                } else if let Some(s @ 2..=9) = stage {
                    let strong = s >= 6;
                    let s = if strong { s - 4 } else { s };
                    let tridents = if strong { 4 } else { 2 };
                    if s == 2 && elapsed == 500
                        || s == 3
                            && elapsed.is_multiple_of(250)
                            && elapsed < tridents * 250
                            && (!strong || elapsed < 750)
                    {
                        let angle = self.rng.gen_range(0. ..TAU);
                        self.ocean_toss(id, 0x1702, 1, 5., angle)?;
                    }
                    if s == 5 && elapsed == 0 {
                        self.ocean_toss(id, 0x1702, 4, 5., TAU / 8.)?;
                    }
                    let duration = match s {
                        2 | 4 => 1100,
                        3 => (tridents - 1) * 250 + 100,
                        _ => 1000,
                    };
                    if elapsed >= duration {
                        self.ocean_stage(id, 1);
                        self.entities.get_mut(&id).unwrap().next_aux =
                            self.now + if strong { 500 } else { 3000 };
                    }
                }
            }
            0x1702 => {
                if stage.is_none() {
                    self.ocean_toss(id, 0x1703, 6, 1., TAU / 12.)?;
                    self.ocean_stage(id, 0);
                    self.entities.get_mut(&id).unwrap().expires.set(
                        Some(self.now + 550),
                        id,
                        &mut self.timed_ids,
                    );
                }
            }
            0x1703 => {
                if stage.is_none() {
                    self.ocean_stage(id, 0);
                    self.entities.get_mut(&id).unwrap().expires.set(
                        Some(self.now + 50),
                        id,
                        &mut self.timed_ids,
                    );
                }
            }
            0x1704 => {
                if stage.is_none() {
                    self.ocean_stage(id, 0);
                    self.entities.get_mut(&id).unwrap().expires.set(
                        Some(self.now + 12000),
                        id,
                        &mut self.timed_ids,
                    );
                    self.ocean_say(id, "Is King Alexander alive?");
                }
                self.entities.get_mut(&id).unwrap().extra_stats =
                    vec![(61, (elapsed / 250 % 2) as i32)];
            }
            0x1705 => {
                self.entities.get_mut(&id).unwrap().extra_stats =
                    vec![(61, (elapsed / 500 % 3) as i32)];
            }
            0x1707 => {
                let e = self.entities.get_mut(&id).unwrap();
                e.size = (e.size + 11).min(100);
            }
            0x170c => {
                if matches!(elapsed % 20200, 0 | 100) && self.ocean_children(id, 0x170b).len() < 8 {
                    let angle = self.rng.gen_range(0. ..TAU);
                    self.ocean_toss(id, 0x170b, 1, 3., angle)?;
                }
            }
            0x170d => {
                if stage.is_none() {
                    self.spawn_children(id, 0x170e, 4, 3.)?;
                    self.ocean_stage(id, 0);
                } else if due(elapsed, 10000) {
                    self.spawn_children(id, 0x170e, 8, 3.)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn ocean_movement(
        &self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> (Motion, f32) {
        // Shared NPCs may now tick untargeted; aquatic movement still requires acquisition.
        let Some(target) = target else {
            return (Motion::Stationary, 0.);
        };
        let e = &self.entities[&id];
        let distance = e.pos.distance(target);
        let chase = |sight, stop| Motion::Chase { sight, stop };
        let home = |distance| Motion::Protect {
            anchor: Anchor::Home,
            distance,
        };
        match e.kind {
            trench::HERMIT if matches!(e.stage, Some(4 | 5)) => (
                if e.pos.distance(e.home) > 5. {
                    home(0.)
                } else {
                    Motion::Wander
                },
                speed(0.3),
            ),
            0xd62 | 0xd64 => {
                let orbit = e.orbit.expect("activated Hermit follower orbit");
                let radius = orbit.radius;
                (
                    if distance < 4. {
                        chase(4., 1.)
                    } else if e.parent.is_some_and(|id| self.entities.contains_key(&id)) {
                        Motion::Orbit {
                            anchor: Anchor::Parent,
                            radius,
                        }
                    } else {
                        Motion::Wander
                    },
                    if distance < 4. {
                        speed(0.6)
                    } else {
                        orbit.speed
                    },
                )
            }
            0xd63
                if e.stage != Some(2)
                    && e.parent.is_some_and(|id| self.entities.contains_key(&id)) =>
            {
                (
                    Motion::Orbit {
                        anchor: Anchor::Parent,
                        radius: 6.,
                    },
                    speed(0.3),
                )
            }
            0x1706 => (
                if matches!(e.stage, Some(4 | 8)) {
                    home(1.)
                } else {
                    Motion::Wander
                },
                speed(0.3),
            ),
            0x1707 => (
                if e.pos.distance(e.home) > 2. {
                    home(0.)
                } else if distance < 4. {
                    chase(4., 1.)
                } else {
                    Motion::Stationary
                },
                speed(0.2),
            ),
            0x1700 => (
                if distance < 9. {
                    chase(9., 2.)
                } else if e.pos.distance(e.home) > 3. {
                    home(0.)
                } else {
                    Motion::Wander
                },
                speed(if distance < 9. { 0.6 } else { 0.3 }),
            ),
            0x1708 => (
                if distance < 9. {
                    chase(9., 1.)
                } else {
                    Motion::Stationary
                },
                speed(0.7),
            ),
            0x1709 => (
                if self
                    .entities
                    .values()
                    .any(|mare| mare.kind == 0x170a && e.pos.distance(mare.pos) < 10.)
                {
                    Motion::Orbit {
                        anchor: Anchor::Kind(0x170a),
                        radius: 2.,
                    }
                } else {
                    Motion::Wander
                },
                speed(0.2),
            ),
            0x170a => (
                if distance < 8. && elapsed % 4000 < 500 {
                    chase(8., 0.)
                } else {
                    Motion::Wander
                },
                speed(if distance < 8. && elapsed % 4000 < 500 {
                    2.
                } else {
                    0.4
                }),
            ),
            0x170c => (
                if distance < 12. {
                    chase(12., 1.)
                } else {
                    Motion::Stationary
                },
                speed(0.8),
            ),
            0x170e if elapsed % 2400 < 900 => (
                if e.pos.distance(e.home) > 10. {
                    home(0.)
                } else {
                    chase(10., 4.)
                },
                speed(0.3),
            ),
            _ => (Motion::Stationary, 0.),
        }
    }
    pub(super) fn ocean_shots(&self, id: i32, target: Pos) -> Result<Vec<(u8, f32)>> {
        let e = &self.entities[&id];
        let t = self.now - e.activated.expect("active ocean encounter");
        let d = e.pos.distance(target);
        let heading = (target.y - e.pos.y).atan2(target.x - e.pos.x);
        let mut out = Vec::new();
        match e.kind {
            trench::HERMIT if e.stage == Some(4) && d < 20. && due(t, 300) => {
                fan(&mut out, 0, heading, 3, 5.)
            }
            trench::HERMIT if e.stage == Some(5) && due(t, 1000) => {
                ring(&mut out, 1, 2, (t / 1000) as f32 * TAU / 8.)
            }
            0xd62 if d < 6. => {
                if due(t, 1000) {
                    fan(&mut out, 0, heading, 3, 10.);
                }
                if due(t, 2600) {
                    fan(&mut out, 1, heading, 2, 20.);
                }
            }
            0xd63 => {
                if e.stage == Some(2) {
                    ring(&mut out, 0, 8, TAU / 8.);
                } else if due(t, 400) {
                    out.push((0, (t / 400) as f32 * TAU / 12.));
                }
            }
            0xd64 if d < 3. && due(t, 500) => ring(&mut out, 0, 8, heading),
            0x1706 => {
                let strong = e.stage.is_some_and(|s| s >= 6);
                match e.stage.map(|s| if s >= 6 { s - 4 } else { s }) {
                    Some(2) if d < 8.8 && t <= 1000 && due(t, 500) => {
                        ring(&mut out, 0, if strong { 16 } else { 8 }, heading)
                    }
                    Some(3) if d < 21. && due(t, 250) && t < if strong { 1000 } else { 500 } => {
                        for n in 0..4 {
                            fan(
                                &mut out,
                                2,
                                heading + n as f32 * TAU / 4. + (t / 250 % 2) as f32 * TAU / 8.,
                                2,
                                25.,
                            );
                        }
                    }
                    Some(4) if d < 18. && t <= 1000 && due(t, 500) => {
                        ring(&mut out, 3, 30, 6f32.to_radians())
                    }
                    _ => {}
                }
            }
            0x1702 if t == 500 && d < 4.4 => fan(&mut out, 0, 0., 5, 70.),
            0x1703 if t == 0 && d < 3.8 => fan(&mut out, 0, 0., 5, 70.),
            0x1707 if due(t, 1000) => {
                for (i, r) in [1.8, 2.5, 3.3, 4.2].into_iter().enumerate() {
                    if d < r {
                        out.push((i as u8, heading));
                    }
                }
            }
            0x1700 if d < 9. => {
                if due(t, 500) {
                    fan(&mut out, 0, heading, 3, 10.);
                }
                if due(t, 2000) {
                    ring(&mut out, 2, 6, 0.);
                }
            }
            0x1700 if d < 12. && due(t + 750, 1000) => out.push((1, heading)),
            0x1708 if d < 9. => {
                if due(t, 2000) {
                    out.push((1, heading));
                }
                if due(t + 750, 1000) {
                    out.push((0, heading));
                }
                if due(t + 500, 1000) {
                    fan(&mut out, 0, heading, 3, 10.);
                }
            }
            0x1709 if d < 9. => {
                for (offset, count) in [(250, 1), (500, 2), (750, 3)] {
                    if due(t + 1000 - offset, 1000) {
                        fan(&mut out, 0, heading, count, 5.);
                    }
                }
            }
            0x170a if t % 8000 < 5000 && d < 9. && due(t, 500) => {
                fan(&mut out, 1, heading, 3, 120.)
            }
            0x170a if t % 8000 >= 5000 && d < 10. => {
                let s = t % 8000 - 5000;
                for (offset, angle) in [(500, 0f32), (1000, 45.), (1500, 135.)] {
                    if s == offset {
                        fan(&mut out, 0, heading + angle.to_radians(), 8, 10.);
                    }
                }
            }
            0x170b if d < 10. && due(t, 100) => out.push((0, heading)),
            0x170c if d < 10. => {
                if due(t, 100) {
                    out.push((0, heading));
                }
                if t % 20200 >= 200 && due(t - 200, 250) {
                    fan(&mut out, 1, heading, 4, 15.);
                }
            }
            0x170e if d < 8. => {
                if t % 2400 < 900 && due(t % 2400, 300) {
                    out.push((0, heading));
                } else if t % 2400 >= 900 && due(t % 2400 - 900, 750) {
                    ring(&mut out, 1, 6, 2f32.to_radians());
                }
            }
            _ => {}
        }
        if e.kind == 0xd62 && out.iter().any(|(index, _)| *index == 1) {
            self.predict_enemy_shots(
                id,
                target,
                &mut out,
                |index| if index == 1 { 0.8 } else { 0. },
            )?;
        }
        Ok(out)
    }
    pub(super) fn ocean_death(&mut self, e: &Entity) -> Result<()> {
        if e.kind == trench::HERMIT {
            for id in self
                .ocean_children(e.id, 0xd65)
                .into_iter()
                .chain(self.ocean_children(e.id, 0xd63))
            {
                self.remove_entity(id);
            }
        } else if e.kind == trench::THESSAL {
            self.spawn_entity(trench::VENT, e.pos, vec![])?;
            if self.rng.gen_bool(0.1) {
                let id = self.spawn_entity(0x1704, e.pos, vec![])?;
                let child = self.entities.get_mut(&id).unwrap();
                child.condition |= condition::bit(condition::INVINCIBLE);
                child
                    .expires
                    .set(Some(self.now + 12000), id, &mut self.timed_ids);
            }
        }
        Ok(())
    }
    fn ocean_say(&self, id: i32, text: &str) {
        let packet = chat::text_packet(
            &format!("#{}", self.content.objects[&self.entities[&id].kind].name),
            id,
            5,
            text,
        );
        for p in self
            .players
            .values()
            .filter(|p| p.pos.distance(self.entities[&id].pos) < 20.)
        {
            send(p, packet.id, packet.body.clone());
        }
    }
    pub(super) fn ocean_response(&mut self, speaker: i32, text: &str) -> Result<()> {
        if !text
            .trim()
            .trim_end_matches('.')
            .eq_ignore_ascii_case("Yes, he lives")
        {
            return Ok(());
        }
        let p = &self.players[&speaker];
        let wounded: Vec<_> = self
            .actors()
            .filter(|e| {
                e.kind == 0x1704
                    && e.stage == Some(0)
                    && p.known.contains(&e.id)
                    && e.pos.distance(p.pos) < 10.
            })
            .map(|e| e.id)
            .collect();
        for id in wounded {
            self.ocean_toss(id, 0x1705, 3, 5., TAU / 8.)?;
            for gift in self.ocean_children(id, 0x1705) {
                self.entities
                    .get_mut(&gift)
                    .unwrap()
                    .parent
                    .set(None, gift, &mut self.timed_ids);
            }
            self.ocean_say(id, "Thank you kind sailor.");
            self.remove_entity(id);
        }
        Ok(())
    }
    pub(super) fn ocean_step(&mut self) {
        if self.name != "Ocean Trench" || !self.now.is_multiple_of(150) {
            return;
        }
        let vents: Vec<_> = self
            .entities
            .values()
            .filter(|e| e.kind == trench::VENT)
            .map(|e| e.pos)
            .collect();
        for (id, p) in self.players.iter_mut().filter(|(_, p)| {
            p.character.alive && !p.paused && !p.departing && !p.death_pending && !p.death_started
        }) {
            if vents.iter().any(|v| v.distance(p.pos) < 1.) {
                p.oxygen = (p.oxygen + 8).min(100);
            } else if p.oxygen > 0 {
                p.oxygen = (p.oxygen - 2).max(0);
            } else {
                p.character.hp = (p.character.hp - 10).max(0);
                if p.character.hp == 0 {
                    p.death_pending = true;
                    p.killer = "suffocation".into();
                    p.bot_goal = None;
                    p.bot_aim = None;
                }
                tracing::debug!(
                    player = id,
                    hp = p.character.hp,
                    "suffocation damage committed"
                );
            }
        }
    }
}
