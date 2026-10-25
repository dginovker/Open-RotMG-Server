use super::*;
use crate::dungeon::jungle::{COIL, EGG, SNAKE};

impl World {
    pub(super) fn populate_coil_snakes(&mut self) -> Result<()> {
        for _ in 0..2 {
            // Match the period guide's midland ruins: https://web.archive.org/web/20120420071116id_/http://forums.wildshadow.com/wiki/realm-mad-god/great-coil-snake
            let pos = self.place_named_setpiece(COIL, &[6, 7])?;
            let items = crate::loot::landmark_chest(&self.content, COIL, &mut self.rng)?;
            self.spawn_entity(0x501, pos, items)?;
            let id = self.spawn_entity(COIL, pos, vec![])?;
            self.entities.get_mut(&id).unwrap().quest = Some(id);
            tracing::info!(
                entity = id,
                x = pos.x,
                y = pos.y,
                "Great Coil Snake guards a Jungle entrance temple"
            );
        }
        Ok(())
    }

    // The older queue hatches surviving eggs after five seconds, unlike later TransformOnDeath forks.
    // https://github.com/trapped/rotmg_svr/blob/593e490803fe0ad168ead408cacd5287d778acfe/wServer/logic/db/BehaviorDb.ForbiddenJungle.cs
    pub(super) fn coil_mechanics(&mut self, id: i32, elapsed: u64) -> Result<()> {
        let e = self.entities.get_mut(&id).unwrap();
        if e.stage.is_none() {
            e.stage = Some(0);
            e.activated.get_or_insert(self.now);
            if e.kind == COIL {
                e.quest = Some(id);
                e.next_aux = self.now + 3000;
            }
        }
        if e.kind == SNAKE {
            return Ok(());
        }
        if e.kind == EGG {
            if elapsed < 5000 {
                return Ok(());
            }
            let egg = self.remove_entity(id);
            let tag = egg.quest.context("Coil egg lacks encounter ownership")?;
            let count = self.rng.gen_range(1..=2);
            self.quest_birth((tag, egg.home, egg.pos), SNAKE, 16, count, 0.3)?;
            let born: Vec<_> = self
                .actors()
                .filter(|e| {
                    e.quest == Some(tag) && e.kind == SNAKE && e.activated == Some(self.now)
                })
                .map(|e| e.id)
                .collect();
            for child in born {
                self.configure_landmark_child(child);
            }
            tracing::debug!(
                egg = id,
                count,
                "Great Snake Egg hatched without defeat credit"
            );
            return Ok(());
        }
        e.stage = Some(u64::from(elapsed % 6500 >= 5000));
        if self.now < e.next_aux {
            return Ok(());
        }
        e.next_aux = self.now + if elapsed % 4000 >= 3000 { 1000 } else { 3000 };
        let center = e.pos;
        let reserved = self
            .actors()
            .filter(|e| e.quest == Some(id))
            .map(|e| {
                if e.kind == EGG {
                    2
                } else {
                    usize::from(e.kind == SNAKE)
                }
            })
            .sum::<usize>()
            + 2 * self.pending_named(id, Some(EGG));
        if reserved + 2 > 16 {
            return Ok(());
        }
        let positions: Vec<_> = [(4., 0.), (0., 4.), (-4., 0.), (0., -4.)]
            .map(|(x, y)| Pos {
                x: center.x + x,
                y: center.y + y,
            })
            .into_iter()
            .filter(|p| self.walkable(*p))
            .collect();
        ensure!(
            !positions.is_empty(),
            "Coil Snake has no clear cardinal egg landing"
        );
        let position = *positions.choose(&mut self.rng).unwrap();
        self.named_toss(id, EGG, 5, &[position])?;
        Ok(())
    }
}
