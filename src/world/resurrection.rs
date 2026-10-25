use super::*;

impl Directory {
    pub fn finish_resurrection(&mut self, id: i32, result: Result<Character>) -> Result<()> {
        let origin = self.location(id)?;
        let world = self
            .worlds
            .get_mut(&origin)
            .context("resurrection world missing")?;
        let result = result.and_then(|restored| {
            let p = world
                .players
                .get(&id)
                .context("resurrection owner missing")?;
            ensure!(
                p.death_pending
                    && p.death_started
                    && p.character.alive
                    && restored.alive
                    && restored.hp > 0
                    && restored.account == p.character.account
                    && restored.id == p.character.id
                    && restored.revision == p.character.revision + 1
                    && p.death_lifetime.as_ref() == Some(&restored.lifetime),
                "stale or foreign resurrection completion"
            );
            Ok(restored)
        });
        let mut restored = world.committed(id, result)?;
        let p = world
            .players
            .get_mut(&id)
            .context("resurrection owner missing")?;
        p.acknowledge_lifetime(restored.lifetime.clone())?;
        restored.lifetime = p.character.lifetime.clone();
        p.character = restored;
        p.death_pending = false;
        p.death_started = false;
        p.death_lifetime = None;
        // Stay untargetable and reject inventory/input changes until the owned Nexus transfer consumes its grant.
        p.departing = true;
        p.statuses.clear();
        p.bot_goal = None;
        p.bot_aim = None;
        let connected =
            p.tx.as_ref()
                .is_some_and(|o| !*o.lost.borrow() && !o.packets.is_closed());
        tracing::info!(
            player = id,
            character = p.character.id,
            revision = p.character.revision,
            connected,
            "resurrection committed; amulet consumed and character preserved"
        );
        world.clear_abilities(id);
        world.clear_companions(id);
        if connected {
            self.return_to_nexus(id, origin, true);
        }
        Ok(())
    }
}
