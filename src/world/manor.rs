use super::*;
use crate::{
    condition,
    dungeon::manor::{self, *},
    encounter::Motion,
};

pub(super) struct Toss {
    parent: i32,
    kind: u16,
    position: Pos,
    deadline: u64,
    lifetime: u64,
}

impl World {
    pub(super) fn manor_toss(
        &mut self,
        parent: i32,
        kind: u16,
        position: Pos,
        lifetime: u64,
    ) -> Result<()> {
        crate::encounter::validate(&self.content, kind)?;
        crate::loot::validate(&self.content, kind)?;
        if self.walkable(position) {
            ensure!(
                self.summon_occupancy() < self.summon_limit(),
                "Manor toss exceeds summon budget"
            );
            self.manor_births.push(Toss {
                parent,
                kind,
                position,
                // Phoenix TossEnemy lands after its flight timer, not alongside the telegraph.
                deadline: self.now + 1500,
                lifetime,
            });
        } else {
            tracing::debug!(parent, kind, ?position, "Manor toss has a blocked landing");
        }
        self.telegraph_throw(parent, position, 0xffffbf00);
        Ok(())
    }

    pub(super) fn manor_birth_step(&mut self) {
        let mut i = 0;
        while i < self.manor_births.len() {
            if self.manor_births[i].deadline > self.now {
                i += 1;
                continue;
            }
            let toss = self.manor_births.remove(i);
            if toss.kind == CREATURE && !self.entities.contains_key(&toss.parent) {
                tracing::debug!(
                    parent = toss.parent,
                    "Ruthven's death canceled his pending coffin"
                );
                continue;
            }
            if let Err(error) =
                self.manor_child(toss.parent, toss.kind, toss.position, Some(toss.lifetime))
            {
                tracing::error!(parent=toss.parent,kind=toss.kind,position=?toss.position,%error,"admitted Manor toss failed to land");
            }
        }
    }

    fn manor_child(
        &mut self,
        parent: i32,
        kind: u16,
        pos: Pos,
        lifetime: Option<u64>,
    ) -> Result<i32> {
        ensure!(
            self.summon_occupancy() < self.summon_limit(),
            "Manor summon budget exhausted"
        );
        ensure!(
            self.walkable(pos),
            "Manor summon {kind:#x} has blocked landing at {pos:?}"
        );
        crate::encounter::validate(&self.content, kind)?;
        crate::loot::validate(&self.content, kind)?;
        let child = self.spawn_entity(kind, pos, vec![])?;
        let e = self.entities.get_mut(&child).unwrap();
        e.activated = Some(self.now);
        e.parent.set(Some(parent), child, &mut self.timed_ids);
        e.expires
            .set(lifetime.map(|ms| self.now + ms), child, &mut self.timed_ids);
        Ok(child)
    }
    fn manor_clear_swarms(&mut self, boss: i32) {
        let roots: BTreeSet<_> = self
            .actors()
            .filter(|e| e.parent == Some(boss) && matches!(e.kind, SWARM | DECOY))
            .map(|e| e.id)
            .collect();
        let children: Vec<_> = self
            .actors()
            .filter(|e| roots.contains(&e.id) || e.parent.is_some_and(|p| roots.contains(&p)))
            .map(|e| e.id)
            .collect();
        for id in children {
            self.remove_entity(id);
        }
    }
    pub(super) fn manor_mechanics(
        &mut self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> Result<()> {
        let e = &self.entities[&id];
        let (kind, pos, hp) = (e.kind, e.pos, e.hp);
        if matches!(kind, KAGE | MINE) {
            return self.manor_entry_mechanics(id, target, elapsed);
        }
        if kind == BOSS {
            let (stage, age) = manor::phase(elapsed);
            let part = stage % 3;
            if e.stage != Some(stage) {
                if part == 0 {
                    self.manor_clear_swarms(id);
                }
                if part == 2 {
                    ensure!(
                        self.summon_occupancy() + 49 <= self.summon_limit(),
                        "Ruthven bat formation exceeds summon budget"
                    );
                    let mut sites = vec![];
                    let mut cells = BTreeSet::new();
                    for i in 0..33 {
                        let a = i as f32 * std::f32::consts::TAU / 32.;
                        let r = if i == 0 { 0. } else { 2. };
                        let tile = (
                            (pos.x + r * a.cos()).floor() as i32,
                            (pos.y + r * a.sin()).floor() as i32,
                        );
                        let site = Pos {
                            x: tile.0 as f32 + 0.5,
                            y: tile.1 as f32 + 0.5,
                        };
                        if cells.insert(tile) && self.walkable(site) {
                            sites.push(site);
                        }
                    }
                    ensure!(
                        sites.len() >= 4,
                        "Ruthven bat formation requires four walkable nearby cells"
                    );
                    for i in 0..4 {
                        let (root, bat, count, site) = if i == 0 {
                            (SWARM, ESCORT, 15, sites[0])
                        } else {
                            (
                                DECOY,
                                CHASER,
                                10,
                                sites[1 + (i - 1) * (sites.len() - 1) / 3],
                            )
                        };
                        let group = self.manor_child(id, root, site, None)?;
                        self.spawn_children(group, bat, count, 0.4)?;
                    }
                }
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(stage);
                e.goal = None;
                e.next_aux = 0;
                tracing::info!(
                    entity = id,
                    stage,
                    part,
                    hp,
                    "Ruthven transformation phase entered"
                );
            }
            let e = self.entities.get_mut(&id).unwrap();
            e.condition &=
                !(condition::bit(condition::INVULNERABLE) | condition::bit(condition::INVINCIBLE));
            // Preserve the queued opening before bat immunity, not uninterrupted ritual protection.
            // https://github.com/cp-nilly/nillys-realm-phoenix/blob/e5eb61dddd921ef605fbcd09c7b352f4af9504e6/wServer/logic/db/BehaviorDb.Manor.cs#L70-L83
            if part == 1 && (750..6650).contains(&age) {
                e.condition |= condition::bit(condition::INVULNERABLE);
            } else if part == 2 {
                e.condition |= condition::bit(condition::INVINCIBLE);
            }
            e.extra_stats = vec![(
                61,
                if part == 2 {
                    1
                } else if part == 1 {
                    2
                } else {
                    0
                },
            )];
            if part == 1 && age >= 2650 && e.next_aux == 0 {
                let sites = [(12., 0.), (-12., 0.), (0., 12.), (0., -12.)].map(|(dx, dy)| Pos {
                    x: pos.x + dx,
                    y: pos.y + dy,
                });
                let landed = sites.iter().filter(|p| self.walkable(**p)).count();
                ensure!(
                    self.summon_occupancy() + landed <= self.summon_limit(),
                    "Ruthven coffin formation exceeds summon budget"
                );
                // The period corridor tactic relies on wall-bound throws failing, not being relocated.
                // https://www.reddit.com/r/RotMG/comments/u6lp3/damn_those_lil_feratu/
                for site in sites {
                    self.manor_toss(id, CREATURE, site, 20000)?;
                }
                self.entities.get_mut(&id).unwrap().next_aux = 1;
                tracing::info!(
                    entity = id,
                    attempted = 4,
                    landed,
                    x = pos.x,
                    y = pos.y,
                    "Ruthven coffin tosses scheduled"
                );
            }
        } else if kind == 0x1727 && e.stage.is_none() {
            let count = self.rng.gen_range(15..=20);
            self.spawn_children(id, BAT, count, 0.4)?;
            self.entities.get_mut(&id).unwrap().stage = Some(0);
        } else if kind == COFFIN {
            if hp < self.content.objects[&COFFIN].hp && e.next_aux == 0 {
                self.entities.get_mut(&id).unwrap().next_aux = 1;
                if self.rng.gen_ratio(1, 5) {
                    let new = self.transform_fauna(id, CREATURE)?;
                    self.entities.get_mut(&new).unwrap().expires.set(
                        Some(self.now + 20000),
                        new,
                        &mut self.timed_ids,
                    );
                    tracing::info!(
                        entity = id,
                        replacement = new,
                        "false treasure coffin awakened without rewards"
                    );
                    return Ok(());
                }
            }
            let e = &self.entities[&id];
            let stage = if hp <= 8000 {
                2
            } else if hp <= 15000 {
                1
            } else {
                0
            };
            let old = e.stage.unwrap_or(0);
            if old < 1 && stage >= 1 {
                self.spawn_children(id, BAT, 15, 1.5)?;
            }
            if old < 2 && stage == 2 {
                let alive = self
                    .actors()
                    .filter(|e| e.parent == Some(id) && e.kind == BAT)
                    .count();
                self.spawn_children(id, BAT, alive + 8, 2.)?;
                self.spawn_children(id, 0x1728, 2, 3.)?;
            }
            self.entities.get_mut(&id).unwrap().stage = Some(stage);
        } else if kind == CREATURE && e.expires.is_none() {
            self.entities.get_mut(&id).unwrap().expires.set(
                Some(self.now + 20000),
                id,
                &mut self.timed_ids,
            );
        } else if kind == 0x1729
            && self.now >= e.next_aux
            && let Some(site) = target
        {
            if self.walkable(site) {
                self.manor_toss(id, ROCK, site, 3050)?;
            }
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 3000;
        } else if kind == ROCK && e.expires.is_none() {
            self.entities.get_mut(&id).unwrap().expires.set(
                Some(self.now + 3050),
                id,
                &mut self.timed_ids,
            );
        }
        Ok(())
    }
    pub(super) fn manor_movement(
        &mut self,
        id: i32,
        target: Option<Pos>,
        elapsed: u64,
    ) -> (Motion, f32) {
        let e = &self.entities[&id];
        if e.kind == KAGE {
            return match e.stage.expect("Kage movement phase") {
                2 => (
                    Motion::WanderHome { radius: 1. },
                    crate::encounter::nr_speed(0.4),
                ),
                3 => (
                    Motion::Chase {
                        sight: 20.,
                        stop: 1.,
                    },
                    crate::encounter::nr_speed(0.8),
                ),
                0 | 1 => (Motion::Stationary, 0.),
                _ => unreachable!("invalid Kage movement phase"),
            };
        }
        if e.kind == BOSS {
            match manor::phase(elapsed).0 % 3 {
                0 => (
                    Motion::WanderHome { radius: 1. },
                    manor::profile(BOSS).speed,
                ),
                1 => (Motion::Stationary, 0.),
                2 => (
                    Motion::Chase {
                        sight: 20.,
                        stop: 2.,
                    },
                    manor::profile(BAT).speed,
                ),
                _ => unreachable!(),
            }
        } else {
            let idle = match e.kind {
                BAT => target.is_none_or(|p| e.pos.distance(p) <= 2.),
                CHASER => target.is_none(),
                ESCORT => {
                    e.pos.distance(
                        self.entities[&e.parent.expect("Manor escort requires its swarm")].pos,
                    ) <= 4.
                }
                _ => false,
            };
            if idle {
                let e = self.entities.get_mut(&id).unwrap();
                if e.stage.is_none() {
                    e.home = e.pos;
                    e.stage = Some(0);
                }
                return (Motion::WanderHome { radius: 2. }, 1. / 1.5);
            }
            if matches!(e.kind, 0x1727..=0x1729) && target.is_none() {
                return (Motion::Stationary, 0.);
            }
            let p = manor::profile(e.kind);
            (p.motion, p.speed)
        }
    }
    pub(super) fn manor_death(&mut self, e: &Entity) -> Result<()> {
        if e.kind == BOSS {
            self.manor_clear_swarms(e.id);
            let children: Vec<_> = self
                .actors()
                .filter(|x| x.parent == Some(e.id))
                .map(|e| e.id)
                .collect();
            for child in children {
                self.remove_entity(child);
            }
        } else if e.kind == CREATURE {
            let count = self.rng.gen_range(1..=4);
            let sites: Vec<_> = (0..32)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 32.;
                    Pos {
                        x: e.pos.x + 0.2 * a.cos(),
                        y: e.pos.y + 0.2 * a.sin(),
                    }
                })
                .filter(|p| self.walkable(*p))
                .collect();
            ensure!(
                sites.len() >= count,
                "Coffin Creature brood has insufficient walkable positions"
            );
            ensure!(
                self.summon_occupancy() + count <= self.summon_limit(),
                "Coffin Creature brood exceeds summon budget"
            );
            for i in 0..count {
                let pos = sites[i * sites.len() / count];
                if let Some(parent) = *e.parent {
                    self.manor_child(parent, FERATU, pos, Some(30000))?;
                } else {
                    self.spawn_entity(FERATU, pos, vec![])?;
                }
            }
            tracing::debug!(entity = e.id, count, "Coffin Creature released Lil Feratus");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coffin_vulnerability_window_precedes_ruthvens_bats() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let map = Arc::new(manor::generate(&content, 12351).unwrap());
        let mut world = World::new(content, map).unwrap();
        let boss = world.actors().find(|e| e.kind == BOSS).unwrap().id;
        for (elapsed, invulnerable, invincible) in [
            (9100, false, false),
            (9800, false, false),
            (9850, true, false),
            (15700, true, false),
            (15750, false, false),
            (16700, false, false),
            (16750, false, true),
            (21750, false, false),
        ] {
            world.now = elapsed;
            world.manor_mechanics(boss, None, elapsed).unwrap();
            let condition = world.entities[&boss].condition;
            assert_eq!(
                (
                    condition & condition::bit(condition::INVULNERABLE) != 0,
                    condition & condition::bit(condition::INVINCIBLE) != 0,
                ),
                (invulnerable, invincible),
                "Ruthven protection at {elapsed}ms"
            );
        }
    }
}
