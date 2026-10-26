use super::*;
use crate::condition;
use crate::dungeon::snake::{BOSS, BUTTON, DART, GUARD, PET, SPAWNER, SWARM, phase, shield};

impl World {
    pub(super) fn snake_mechanics(&mut self, id: i32, elapsed: u64) -> Result<()> {
        if self.entities[&id].kind != BOSS {
            return self.snake_treasure(id);
        }
        let (stage, age) = phase(elapsed);
        let changed = self.entities[&id].stage != Some(stage);
        let e = self.entities.get_mut(&id).unwrap();
        e.condition &= !condition::bit(condition::INVULNERABLE);
        if shield(elapsed) {
            e.condition |= condition::bit(condition::INVULNERABLE);
        }
        if changed {
            e.stage = Some(stage);
            e.next_aux = self.now;
            tracing::info!(
                entity = id,
                kind = BOSS,
                stage,
                hp = e.hp,
                "Stheno timed phase entered"
            );
        }
        self.spawn_children(id, PET, 4, 10.)?;
        // Orbit's angle must follow actual position and the motor's speed: https://github.com/moistosaurus/realm-src/blob/1855c017eab870d13f0b61b1cf0b9e66a01922ff/wServer/logic/behaviors/Orbit.cs#L68-L86
        for pet_id in &self.actor_ids {
            let pet = &self.entities[pet_id];
            if pet.kind == PET && pet.parent == Some(id) && pet.orbit.is_none() {
                self.entities.get_mut(pet_id).unwrap().orbit = Some(realm_events::Orbit {
                    speed: crate::encounter::nr_speed(7.5),
                    radius: 10.,
                    direction: 1.,
                });
            }
        }
        if stage % 3 != 1 || age >= 16000 {
            let children: Vec<_> = self
                .actors()
                .filter(|e| e.parent == Some(id) && e.kind == SWARM)
                .map(|e| e.id)
                .collect();
            for child in children {
                self.remove_entity(child);
            }
        } else if self.now >= self.entities[&id].next_aux {
            let count = self
                .actors()
                .filter(|e| e.parent == Some(id) && e.kind == SWARM)
                .count();
            self.spawn_children(id, SWARM, (count + 1).min(8), 2.5)?;
            self.entities.get_mut(&id).unwrap().next_aux = self.now + 750;
        }
        Ok(())
    }

    fn snake_treasure(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        if e.kind != BUTTON || e.stage.is_some() {
            return Ok(());
        }
        let pos = e.pos;
        if !self
            .players
            .values()
            .any(|p| p.active() && p.pos.distance(pos) <= 0.5)
        {
            return Ok(());
        }
        let spawners: Vec<_> = self
            .actors()
            .filter(|e| e.kind == SPAWNER && e.home.distance(pos) < 15.)
            .map(|e| e.id)
            .collect();
        ensure!(
            spawners.len() == 1,
            "Snake Pit button {id} requires one local Guard spawner, found {}",
            spawners.len()
        );
        let owner = spawners[0];
        let center = self.entities[&owner].home;
        let traps: Vec<_> = self
            .actors()
            .filter(|e| e.kind == DART && e.home.distance(center) < 15.)
            .map(|e| e.id)
            .collect();
        ensure!(
            traps.len() == 10,
            "Snake Pit spawner {owner} requires ten local dart sources, found {}",
            traps.len()
        );
        self.spawn_children(owner, GUARD, 1, 0.)?;
        let guard = self
            .actors()
            .find(|e| e.kind == GUARD && e.parent == Some(owner))
            .context("Snake Pit Guard missing after successful summon")?
            .id;
        self.entities.get_mut(&guard).unwrap().size = 100;
        for trap in traps {
            self.entities.get_mut(&trap).unwrap().parent.set(
                Some(owner),
                trap,
                &mut self.timed_ids,
            );
        }
        let button = self.entities.get_mut(&id).unwrap();
        button.stage = Some(1);
        button.extra_stats.push((61, 1));
        tracing::info!(
            button = id,
            spawner = owner,
            guard,
            "Snake Pit treasure room activated once"
        );
        Ok(())
    }
}
