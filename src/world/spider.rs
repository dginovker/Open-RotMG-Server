use super::*;
use crate::{
    condition,
    dungeon::spider::{BOSS, EGG, HATCHLING},
};

impl World {
    pub(super) fn spider_mechanics(
        &mut self,
        id: i32,
        target: impl Into<Option<Pos>>,
        elapsed: u64,
    ) -> Result<()> {
        let target = target.into();
        let e = &self.entities[&id];
        if e.kind == EGG {
            if target.is_some_and(|target| e.pos.distance(target) < 0.5) {
                self.finish_timed_defeat(id)?;
            }
            return Ok(());
        }
        if e.kind != BOSS {
            return Ok(());
        }
        if e.stage.is_none() && target.is_none() {
            self.entities.get_mut(&id).unwrap().activated = Some(self.now);
            return Ok(());
        }
        if e.stage.is_none() || e.stage == Some(0) && elapsed >= 1500 {
            let tossing = e.stage.is_none();
            let home = e.pos;
            let positions: Vec<_> = (0..9)
                .map(|i| {
                    let (radius, degrees) = if i < 6 {
                        (10., i as f32 * 60.)
                    } else {
                        (6., (i - 6) as f32 * 120.)
                    };
                    let angle = degrees.to_radians();
                    (
                        0x937 + i,
                        Pos {
                            x: home.x + radius * angle.cos(),
                            y: home.y + radius * angle.sin(),
                        },
                    )
                })
                .collect();
            ensure!(
                self.summon_count()
                    + self.named_births.len()
                    + self.abyss_pending_count()
                    + positions.len()
                    <= 256,
                "Spider Den summon budget exhausted"
            );
            ensure!(
                positions.iter().all(|(_, pos)| self.walkable(*pos)),
                "Arachna web formation requires a clear arena"
            );
            for (kind, _) in &positions {
                crate::encounter::validate(&self.content, *kind)?;
                crate::loot::validate(&self.content, *kind)?;
            }
            if tossing {
                for (_, pos) in positions {
                    self.telegraph_throw(id, pos, 0xffffbf00);
                }
                let e = self.entities.get_mut(&id).unwrap();
                e.stage = Some(0);
                e.next_shot = self.now + 2000;
                tracing::info!(entity = id, spokes = 9, "Arachna web tosses telegraphed");
            } else {
                for (kind, pos) in positions {
                    let child = self.spawn_entity(kind, pos, vec![])?;
                    let e = self.entities.get_mut(&child).unwrap();
                    e.parent.set(Some(id), child, &mut self.timed_ids);
                    e.activated = Some(self.now);
                }
                self.entities.get_mut(&id).unwrap().stage = Some(1);
                tracing::info!(entity = id, spokes = 9, "Arachna web formation established");
            }
        }
        let e = self.entities.get_mut(&id).unwrap();
        if elapsed < 2000 {
            e.condition |= condition::bit(condition::INVULNERABLE);
        } else {
            e.condition &= !condition::bit(condition::INVULNERABLE);
            if e.stage != Some(2) {
                e.stage = Some(2);
                tracing::info!(entity = id, "Arachna attack phase entered");
            }
        }
        Ok(())
    }
    pub(super) fn spider_loot(&mut self, boss: &Entity) -> Result<()> {
        let shared: Vec<_> = crate::loot::rewards(&self.content, BOSS, &mut self.rng)
            .into_iter()
            .map(i32::from)
            .collect();
        self.loot_bags(boss.pos, None, &shared)?;
        // The comparator's 0.32 threshold compares integer damage, not a percentage of Arachna's HP.
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
            self.loot_bags(boss.pos, Some(account), &[0x707])?;
        }
        Ok(())
    }
    pub(super) fn hatch_spiders(&mut self, egg: &Entity) -> Result<()> {
        let count = self.rng.gen_range(2..=7);
        ensure!(
            self.actors()
                .filter(|e| self.content.objects[&e.kind].enemy)
                .count()
                + count
                <= 512,
            "Spider hatchling budget exhausted"
        );
        let positions: Vec<_> = (0..count)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / count as f32;
                Pos {
                    x: egg.pos.x + 0.2 * angle.cos(),
                    y: egg.pos.y + 0.2 * angle.sin(),
                }
            })
            .collect();
        ensure!(
            positions.iter().all(|pos| self.walkable(*pos)),
            "Spider egg lacks clear hatch positions"
        );
        for pos in positions {
            self.spawn_entity(HATCHLING, pos, vec![])?;
        }
        tracing::debug!(entity = egg.id, count, "Spider egg hatched");
        Ok(())
    }
}
