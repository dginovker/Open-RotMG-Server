use super::*;
use crate::{
    condition,
    dungeon::jungle::*,
    encounter::{self, attacks::Schedule},
};

impl World {
    pub(super) fn jungle_initialize(&mut self) -> Result<()> {
        let bindings: Vec<_> = self
            .actors()
            .filter_map(|e| {
                let owner = match e.kind {
                    BOSS_TOTEM => BOSS,
                    BABY => BASILISK,
                    SHAMAN | WARRIOR | HUNTER => FIRE,
                    _ => return None,
                };
                let parent = self
                    .actors()
                    .filter(|p| p.kind == owner && p.pos.distance(e.pos) < 15.)
                    .min_by(|a, b| a.pos.distance(e.pos).total_cmp(&b.pos.distance(e.pos)))
                    .map(|p| p.id);
                Some((e.id, owner, parent))
            })
            .collect();
        for (id, owner, parent) in bindings {
            if owner == BOSS {
                self.entities.get_mut(&id).unwrap().quest.set(
                    Some(parent.context("Jungle boss totem has no encounter owner")?),
                    id,
                    &mut self.quest_ids,
                );
            } else if let Some(parent) = parent {
                self.entities.get_mut(&id).unwrap().parent.set(
                    Some(parent),
                    id,
                    &mut self.timed_ids,
                );
            }
        }
        Ok(())
    }

    pub(super) fn jungle_walkable(&self, kind: u16, pos: Pos) -> bool {
        matches!(kind, BASILISK | BABY | SPIRIT)
            && self
                .map
                .cell(pos.x, pos.y)
                .is_some_and(|c| self.content.grounds[&c.ground] > 0.)
            && self
                .entities_at(pos)
                .all(|e| !self.content.objects[&e.kind].blocks || foliage(e.kind))
    }

    fn jungle_enter(&mut self, id: i32, stage: u64) {
        let e = self.entities.get_mut(&id).unwrap();
        e.stage = Some(stage);
        e.activated = Some(self.now);
        e.next_shot = self.now;
        tracing::info!(
            entity = id,
            kind = e.kind,
            stage,
            hp = e.hp,
            time_ms = self.now,
            "Jungle encounter phase entered"
        );
    }

    pub(super) fn jungle_mechanics(&mut self, id: i32, elapsed: u64) -> Result<()> {
        let kind = self.entities[&id].kind;
        if matches!(kind, COIL | EGG | SNAKE) {
            return self.coil_mechanics(id, elapsed);
        }
        if kind == BOSS {
            if self.entities[&id].stage.is_none() {
                self.jungle_enter(id, 0);
            }
            let e = &self.entities[&id];
            let stage = e.stage.unwrap();
            // The target-matching 4,200-HP reference puts the irreversible transition at 2,100 HP.
            // https://wikiwiki.jp/rmd/Mixcoatl%20the%20Masked%20God
            let next = if stage < 4 && e.hp <= self.content.objects[&kind].hp / 2 {
                Some(4)
            } else {
                match stage {
                    0 if e.hp < self.content.objects[&kind].hp => Some(1),
                    1 if elapsed >= 5000 => Some(if self.rng.gen_bool(0.5) { 2 } else { 3 }),
                    2 | 3 if elapsed >= 5000 => Some(1),
                    4 if e.pos.distance(e.home) <= 0.2 => Some(5),
                    5 if elapsed >= 7500 => Some(6),
                    _ => None,
                }
            };
            if let Some(stage) = next {
                self.jungle_enter(id, stage);
                if stage == 1 {
                    let totems: Vec<_> = self
                        .actors()
                        .filter(|e| e.kind == BOSS_TOTEM && e.quest == Some(id))
                        .map(|e| e.id)
                        .collect();
                    ensure!(
                        totems.len() == 4,
                        "Mixcoatl requires four owned boss totems"
                    );
                    for totem in totems {
                        self.spawn_children(totem, SPIRIT, 2, 1.5)?;
                    }
                }
            }
            let e = self.entities.get_mut(&id).unwrap();
            let stage = e.stage.unwrap();
            e.condition &= !condition::bit(condition::INVULNERABLE);
            if matches!(stage, 1 | 4 | 5) {
                e.condition |= condition::bit(condition::INVULNERABLE);
            }
            let age = self.now - e.activated.unwrap();
            let texture = match stage {
                1 => 3 + (age / 200 % 4) as i32,
                5 => 1 + (age / 500 % 2) as i32,
                _ => 0,
            };
            e.extra_stats = vec![(61, texture)];
        } else if kind == BOSS_TOTEM {
            if self.entities[&id].stage.is_none() {
                self.jungle_enter(id, 0);
            }
            ensure!(
                self.entities[&id].quest.is_some(),
                "unbound Jungle boss totem"
            );
        } else {
            let initial = self.entities[&id].stage.is_none();
            if initial {
                self.jungle_enter(id, 0);
            }
            if kind == TOTEM && self.now >= self.entities[&id].next_aux {
                self.spawn_children(id, SPIRIT, 3, 2.)?;
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 2000;
            }
            if matches!(kind, SHAMAN | FIRE) && self.now >= self.entities[&id].next_aux {
                let pos = self.entities[&id].pos;
                let group: Vec<_> = self
                    .actors()
                    .filter(|e| {
                        matches!(e.kind, SHAMAN | WARRIOR | HUNTER)
                            && e.pos.distance(pos) < 8.
                            && (kind == SHAMAN || e.parent == Some(id))
                    })
                    .map(|e| e.id)
                    .collect();
                for target in group {
                    self.heal_enemy(id, target, 30)?;
                }
                self.entities.get_mut(&id).unwrap().next_aux = self.now + 2000;
            }
        }
        Ok(())
    }

    pub(super) fn jungle_schedule(&self, id: i32) -> Result<Schedule> {
        let e = &self.entities[&id];
        let (stage, age) = if e.kind == BOSS_TOTEM {
            let owner = e.quest.context("unbound Jungle boss totem")?;
            match self.entities.get(&owner) {
                Some(boss) => match boss.stage {
                    Some(stage) => (
                        stage,
                        self.now - boss.activated.context("Mixcoatl phase clock missing")?,
                    ),
                    None => (0, 0),
                },
                None => (0, 0),
            }
        } else {
            (
                e.stage.context("Jungle actor has no phase")?,
                self.now - e.activated.context("Jungle actor has no phase clock")?,
            )
        };
        Ok(encounter::jungle::schedule(e.kind, stage, age))
    }

    pub(super) fn jungle_death(&mut self, defeated: &Entity) {
        let children: Vec<_> = self
            .actors()
            .filter(|e| e.parent == Some(defeated.id))
            .map(|e| e.id)
            .collect();
        for id in children {
            self.entities
                .get_mut(&id)
                .unwrap()
                .parent
                .set(None, id, &mut self.timed_ids);
        }
    }
}
