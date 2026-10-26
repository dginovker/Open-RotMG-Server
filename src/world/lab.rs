// GPL-3.0 comparator: realm-src@1855c017eab870d13f0b61b1cf0b9e66a01922ff,
// BehaviorDb.Lab.cs (sebastianfra12, ossimc82); data/REALM-SRC-LICENSE.
use super::*;
use crate::{
    condition,
    dungeon::lab,
    encounter::{Anchor, Motion, nr_speed},
};

pub(super) struct Toss {
    owner: i32,
    pub(super) kind: u16,
    position: Pos,
    deadline: u64,
}

impl World {
    pub(super) fn lab_step(&mut self) {
        let pools: Vec<_> = self
            .world_effect_actors()
            .filter(|e| matches!(e.kind, lab::HEXXER | lab::DEHEXXER))
            .map(|e| (e.kind, e.pos))
            .collect();
        for (id, player) in &mut self.players {
            if !player.character.alive || player.death_pending || player.departing {
                player.lab_conditions = 0;
                continue;
            }
            if player.paused {
                continue;
            }
            let inside = |center: Pos| {
                (center.x as i32 - player.pos.x as i32).abs() < 2
                    && (center.y as i32 - player.pos.y as i32).abs() < 2
            };
            // Actor geometry is reconstructed, not inferred from particle extents or water IDs; blue wins overlaps.
            let blue = pools
                .iter()
                .any(|(kind, pos)| *kind == lab::DEHEXXER && inside(*pos));
            let old = player.lab_conditions;
            if blue {
                player.lab_conditions = 0;
                player.statuses.remove(&condition::HEXED);
            } else if pools
                .iter()
                .any(|(kind, pos)| *kind == lab::HEXXER && inside(*pos))
            {
                player.lab_conditions =
                    condition::bit(condition::HEXED) | condition::bit(condition::SPEEDY) | STUNNED;
            }
            if old != player.lab_conditions {
                tracing::info!(
                    player = id,
                    before = old,
                    after = player.lab_conditions,
                    remedy = blue,
                    "Lab pool source conditions changed"
                );
            }
        }
        let mut i = 0;
        while i < self.lab_tosses.len() {
            if self.lab_tosses[i].deadline > self.now {
                i += 1;
                continue;
            }
            let toss = self.lab_tosses.remove(i);
            if let Err(error) = self.lab_spawn(toss.owner, toss.kind, toss.position) {
                tracing::error!(owner=toss.owner, kind=toss.kind, %error, "admitted Lab toss failed to land");
            }
        }
    }

    fn lab_spawn(&mut self, owner: i32, kind: u16, position: Pos) -> Result<i32> {
        ensure!(self.walkable(position), "Lab summon landing blocked");
        crate::encounter::validate(&self.content, kind)?;
        crate::loot::validate(&self.content, kind)?;
        ensure!(
            self.summon_count() + self.lab_tosses.len() < 256,
            "Lab shared summon budget exhausted"
        );
        let id = self.spawn_entity(kind, position, vec![])?;
        // TossObject's admitted 1500ms landing survives its thrower's death.
        let parent = self.entities.contains_key(&owner).then_some(owner);
        self.entities
            .get_mut(&id)
            .unwrap()
            .parent
            .set(parent, id, &mut self.timed_ids);
        Ok(id)
    }

    fn lab_stage(&mut self, id: i32, stage: u64) {
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(stage);
        e.activated = Some(self.now);
        e.next_aux = self.now;
        e.next_shot = self.now;
        tracing::info!(
            entity = id,
            kind = e.kind,
            stage,
            "Lab encounter phase entered"
        );
    }

    pub(super) fn lab_mechanics(
        &mut self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<()> {
        let e = &self.entities[&id];
        let kind = e.kind;
        if kind == lab::BOSS {
            let mut stage = e.stage.unwrap_or(0);
            // Approved reconstruction makes the comparator's idle-only rage gate global and irreversible.
            let rage = e.hp < self.content.objects[&kind].hp / 5;
            let duration = if stage.is_multiple_of(3) {
                12000
            } else if stage % 3 == 1 || stage == 13 {
                10000
            } else {
                15000
            };
            if e.stage.is_none() || rage && stage < 12 || elapsed >= duration {
                stage = if rage && stage < 12 {
                    12
                } else if e.stage.is_none() {
                    0
                } else if stage >= 12 {
                    if stage == 12 { 13 } else { 12 }
                } else {
                    (stage + 1) % 12
                };
                self.lab_stage(id, stage);
                let hide = stage < 12 && stage % 3 == 2;
                let e = self.entities.get_mut(&id).unwrap();
                if hide {
                    e.condition |= condition::bit(condition::INVINCIBLE);
                } else {
                    e.condition &= !condition::bit(condition::INVINCIBLE);
                }
                self.landmark_texture(id, i32::from(hide));
                if hide {
                    let cages: Vec<_> = self
                        .entities
                        .values()
                        .filter(|e| e.kind == lab::CAGE)
                        .map(|e| e.pos)
                        .collect();
                    for position in cages {
                        for kind in [0x1e23, 0x1e24, 0x1e25] {
                            if !self.actors().any(|e| {
                                e.parent == Some(id)
                                    && e.kind == kind
                                    && e.home.distance(position) < 0.1
                            }) {
                                // Selected cages occupy their cell: release inward without bypassing collision.
                                let home = self.entities[&id].home;
                                let landing = Pos {
                                    x: position.x + (home.x - position.x).signum(),
                                    y: position.y + (home.y - position.y).signum(),
                                };
                                let child = self.lab_spawn(id, kind, landing)?;
                                self.entities.get_mut(&child).unwrap().home = position;
                            }
                        }
                    }
                }
            }
            let hide = stage < 12 && stage % 3 == 2;
            let safe = (stage / 3) as u16;
            let fixtures: Vec<_> = self
                .entities
                .values()
                .filter(|e| matches!(e.kind, lab::BUBBLE | lab::CAGE | 0x993..=0x996))
                .map(|e| e.id)
                .collect();
            for fixture in fixtures {
                let e = self.entities.get_mut(&fixture).unwrap();
                e.parent.set(Some(id), fixture, &mut self.timed_ids);
                if (0x993..=0x996).contains(&e.kind) {
                    e.stage = Some(u64::from(!hide && stage < 12 && e.kind != 0x993 + safe));
                }
                let kind = e.kind;
                if kind == lab::BUBBLE {
                    self.landmark_texture(
                        fixture,
                        if hide {
                            1 + ((self.now / 1000) % 2) as i32
                        } else {
                            0
                        },
                    );
                }
                if kind == lab::CAGE {
                    self.landmark_texture(fixture, if hide { 2 } else { 0 });
                }
            }
            if !hide
                && self.now >= self.entities[&id].next_aux
                && self.entities[&id].conditions(self.now) & STUNNED == 0
            {
                if let Some(position) = target.filter(|p| p.distance(self.entities[&id].pos) < 5.) {
                    let kind = if stage == 13 || stage < 12 && stage % 3 == 1 && stage < 3 {
                        0x12aa
                    } else {
                        0x978
                    };
                    ensure!(
                        self.actors().filter(|e| e.parent == Some(id)).count()
                            + self.lab_tosses.iter().filter(|t| t.owner == id).count()
                            < 128,
                        "Dr Terrible owned summon budget exhausted"
                    );
                    ensure!(self.walkable(position), "Lab toss target blocked");
                    self.telegraph_throw(id, position, 0xffffbf00);
                    self.lab_tosses.push(Toss {
                        owner: id,
                        kind,
                        position,
                        deadline: self.now + 1500,
                    });
                }
                self.entities.get_mut(&id).unwrap().next_aux =
                    self.now + if stage >= 12 { 1500 } else { 2000 };
            }
        } else if kind == 0x978 {
            if elapsed >= 2000 && e.stage.is_none() {
                self.lab_stage(id, 1);
                self.entities.get_mut(&id).unwrap().expires.set(
                    Some(self.now + 50),
                    id,
                    &mut self.timed_ids,
                );
            }
        } else if matches!(kind, 0x980 | 0x1e24) {
            let stage = e.stage.unwrap_or(0);
            if e.hp < self.content.objects[&kind].hp / 5 && stage < 3 {
                self.lab_stage(id, 3);
            } else if stage == 3 && elapsed >= 2000 {
                self.lab_stage(id, 4);
                self.entities.get_mut(&id).unwrap().expires.set(
                    Some(self.now + 50),
                    id,
                    &mut self.timed_ids,
                );
            } else if stage == 0 && elapsed >= 10000 {
                self.lab_stage(id, 1);
            } else if stage == 1 && elapsed >= 3000 {
                self.lab_stage(id, 2);
            }
        } else if kind == 0x983 {
            let maximum = self.content.objects[&kind].hp;
            let stage = [3, 2, 1]
                .into_iter()
                .filter(|quarter| i64::from(e.hp) * 4 < i64::from(maximum) * quarter)
                .count() as u64;
            let e = self.entities.get_mut(&id).unwrap();
            e.stage = Some(stage);
            e.size = [150, 150, 175, 200][stage as usize];
        } else if kind == lab::CREATION {
            let (stage, pos) = (e.stage, e.pos);
            let inside = |pool| {
                self.actors().any(|p| {
                    p.kind == pool
                        && (p.pos.x as i32 - pos.x as i32).abs() < 2
                        && (p.pos.y as i32 - pos.y as i32).abs() < 2
                })
            };
            match stage {
                // Coils AND Dr Terrible open the room: https://web.archive.org/web/20261025211723/https://wikiwiki.jp/rmd/Tesla%20Coil (2012-08-30). realm-src's coil-only Open Wall is marked "need fix".
                None if self.cleared && !self.actors().any(|e| e.kind == lab::TESLA) => {
                    let walls: Vec<_> = self
                        .entities
                        .values()
                        .filter(|e| e.kind == lab::EMERGENCY)
                        .map(|e| e.id)
                        .collect();
                    for &wall in &walls {
                        self.remove_entity(wall);
                    }
                    tracing::info!(walls = walls.len(), "Mad Lab secret room opened");
                    self.lab_stage(id, 1);
                }
                // Invincible until the Hexxer turns it into a random pet; a DeHexxer reverts it: https://web.archive.org/web/20261025211809/https://wikiwiki.jp/rmd/Horrific%20Creation
                Some(1) if inside(lab::HEXXER) => {
                    let pet = self.rng.gen_range(2..=7);
                    self.entities.get_mut(&id).unwrap().condition &=
                        !condition::bit(condition::INVINCIBLE);
                    self.landmark_texture(id, pet);
                    self.lab_stage(id, 2);
                }
                Some(2) if inside(lab::DEHEXXER) => {
                    self.entities.get_mut(&id).unwrap().condition |=
                        condition::bit(condition::INVINCIBLE);
                    self.landmark_texture(id, 0);
                    self.lab_stage(id, 1);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn lab_movement(&self, id: i32) -> (Motion, f32) {
        let e = &self.entities[&id];
        if e.kind == lab::BOSS && e.stage.is_some_and(|s| s < 12 && s % 3 == 2) {
            (
                Motion::Protect {
                    anchor: Anchor::Home,
                    distance: 0.1,
                },
                nr_speed(1.),
            )
        } else if e.kind == lab::CREATION
            && e.stage == Some(2)
            && e.activated.is_some_and(|t| self.now >= t + 10000)
        {
            // The pet returns to a DeHexxer "after a while" (https://web.archive.org/web/20261025211809/https://wikiwiki.jp/rmd/Horrific%20Creation); 10 s is reconstruction.
            (
                Motion::Protect {
                    anchor: Anchor::Kind(lab::DEHEXXER),
                    distance: 0.1,
                },
                nr_speed(0.5),
            )
        } else {
            let p = lab::profile(e.kind);
            (p.motion, p.speed)
        }
    }

    pub(super) fn lab_shots(&self, id: i32, target: Pos) -> Vec<(u8, f32)> {
        let e = &self.entities[&id];
        let heading = (target.y - e.pos.y).atan2(target.x - e.pos.x);
        let fan = |index, count: usize, gap: f32, angle: f32| {
            (0..count)
                .map(|n| {
                    (
                        index,
                        angle + (n as f32 - (count - 1) as f32 / 2.) * gap.to_radians(),
                    )
                })
                .collect()
        };
        match e.kind {
            0x978 if e.stage == Some(1) => {
                fan(0, 6, 60., 22.5f32.to_radians() + 150f32.to_radians())
            }
            0x978 => vec![],
            0x993..=0x996 if e.stage != Some(1) => vec![],
            0x993..=0x996 => fan(0, 20, 18., 22.5f32.to_radians() + 171f32.to_radians()),
            // Phase 1 fires 3 H Creation Waves at the player: https://web.archive.org/web/20261025211809/https://wikiwiki.jp/rmd/Horrific%20Creation; the 20° gap is reconstruction.
            lab::CREATION if e.stage == Some(1) => fan(1, 3, 20., heading),
            0x990 => vec![(0, 0.)],
            0x991 => vec![(0, std::f32::consts::PI)],
            0x979 | 0x1e25 | 0x12aa => fan(0, 2, 20., heading),
            0x980 | 0x1e24 if e.stage == Some(3) => vec![],
            0x980 | 0x1e24 if e.stage == Some(4) => {
                fan(2, 8, 45., 22.5f32.to_radians() + 157.5f32.to_radians())
            }
            0x980 | 0x1e24 => vec![(u8::from(e.stage.is_some_and(|s| s > 0)), heading)],
            // The comparator names nonexistent projectile 1; selected 123.5.1 defines only 0.
            0x981 | 0x1e23 => vec![(0, heading)],
            0x982 => {
                let mut shots = fan(0, 3, 20., heading);
                shots.extend(fan(1, 4, 20., heading));
                shots
            }
            0x983 => fan(
                e.stage.unwrap_or(0) as u8,
                if e.stage.unwrap_or(0) == 0 { 3 } else { 2 },
                20.,
                heading,
            ),
            _ => vec![],
        }
    }

    pub(super) fn lab_death(&mut self, defeated: &Entity) -> Result<()> {
        if defeated.kind == lab::BOSS {
            let children: Vec<_> = self
                .actors()
                .filter(|e| e.parent == Some(defeated.id))
                .map(|e| e.id)
                .collect();
            for child in children {
                let e = self.entities.get_mut(&child).unwrap();
                e.parent.set(None, child, &mut self.timed_ids);
                if (0x993..=0x996).contains(&e.kind) {
                    e.stage = Some(0);
                }
                if e.kind == lab::BUBBLE {
                    self.landmark_texture(child, 0);
                }
            }
            let cages: Vec<_> = self
                .entities
                .values()
                .filter(|e| e.kind == lab::CAGE)
                .map(|e| e.id)
                .collect();
            for cage in cages {
                self.entities
                    .get_mut(&cage)
                    .unwrap()
                    .parent
                    .set(None, cage, &mut self.timed_ids);
                self.landmark_texture(cage, 0);
            }
        } else if defeated.kind == 0x982 {
            for n in 0..3 {
                let angle = n as f32 * std::f32::consts::TAU / 3.;
                let position = Pos {
                    x: defeated.pos.x + 0.2 * angle.cos(),
                    y: defeated.pos.y + 0.2 * angle.sin(),
                };
                self.spawn_entity(0x979, position, vec![])?;
            }
        }
        Ok(())
    }

    pub(super) fn lab_loot(&mut self, boss: &Entity) -> Result<()> {
        // Loots.cs compares threshold .32/.1 against integer damage, not a fraction of boss HP.
        let owners: BTreeSet<_> = self
            .players
            .values()
            .filter(|p| {
                p.character.alive
                    && !p.death_pending
                    && !p.departing
                    && boss.damage.get(&p.character.id).is_some_and(|n| *n > 0)
            })
            .map(|p| p.character.account)
            .collect();
        for account in owners {
            let mut rewards: Vec<_> = crate::loot::rewards(&self.content, lab::BOSS, &mut self.rng)
                .into_iter()
                .map(i32::from)
                .collect();
            rewards.push(0xa35);
            self.loot_bags(boss.pos, Some(account), &rewards)?;
        }
        Ok(())
    }
}
