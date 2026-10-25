use super::*;

const LIFETIME_MS: u64 = 10_000;
const CAPACITY: usize = 32;

pub(super) struct Witness {
    source: i32,
    portal: i32,
    kind: u16,
    entrant: i32,
    account: i32,
    name: String,
    observed_ms: u64,
    observers: Vec<(i32, i32, i32)>,
}

fn living(p: &Player) -> bool {
    p.character.alive
        && !p.death_started
        && p.character.hp > 0
        && !p.death_pending
        && p.persistence_failure.is_none()
}
pub(super) fn observing(p: &Player) -> bool {
    living(p)
        && !p.departing
        && !p.paused
        && p.tx
            .as_ref()
            .is_none_or(|tx| !*tx.lost.borrow() && !tx.packets.is_closed())
}
fn sees(w: &World, observer: i32, target: i32) -> bool {
    let p = &w.players[&observer];
    p.known.contains(&target) && w.visible(observer).contains(&target)
}
impl Directory {
    pub(super) fn capture_dungeon_entry(&self, id: i32, key: &[u8]) -> Option<Witness> {
        let grant = self.grants.get(key)?;
        let source = grant.origin?;
        let world = self.worlds.get(&source)?;
        let destination = self.worlds.get(&grant.destination)?;
        let portal = grant.entry_portal?;
        let binding = self.portals.get(&portal)?;
        let entity = world.entities.get(&portal)?;
        let entrant = world.players.get(&id)?;
        if world.owner.is_some()
            || world.guild.is_some()
            || destination.owner.is_some()
            || destination.guild.is_some()
            || !crate::dungeon::DUNGEONS
                .iter()
                .any(|(kind, _, name, _)| *kind == entity.kind && *name == destination.name)
            || binding.origin != source
            || binding.destination != Some(grant.destination)
            || binding.expires <= self.tick
            || !entity.published
            || entity.conditions(world.now) & crate::condition::bit(crate::condition::INVISIBLE)
                != 0
            || !living(entrant)
            || entrant.paused
            || entrant.character.account != grant.account
            || entrant.character.id != grant.character
            || entrant.conditions(world.now) & crate::condition::bit(crate::condition::INVISIBLE)
                != 0
            || entrant.pos.distance(entity.pos) > 2.
        {
            tracing::debug!(
                player = id,
                portal,
                "dungeon entry observation suppressed: source visibility or public binding changed"
            );
            return None;
        }
        let observers = world
            .players
            .iter()
            .filter(|(observer, p)| {
                **observer != id
                    && observing(p)
                    && sees(world, **observer, id)
                    && sees(world, **observer, portal)
            })
            .map(|(id, p)| (*id, p.character.account, p.character.id))
            .collect();
        Some(Witness {
            source,
            portal,
            kind: entity.kind,
            entrant: id,
            account: grant.account,
            name: entrant.character.name.clone(),
            observed_ms: self.tick * 50,
            observers,
        })
    }

    // Both entry paths invoke this only after enter_admitted and the lifecycle binding succeed.
    pub(crate) fn complete_dungeon_entry(&mut self, key: &[u8], player: i32) -> Result<usize> {
        let grant = self
            .grants
            .get(key)
            .context("completed entry grant missing")?;
        if grant.entry_witness.is_none() {
            return Ok(0);
        }
        let state = grant.authority.state();
        ensure!(
            state & (TRANSFER_REVOKED | TRANSFER_RETIRED) == 0
                && state & (TRANSFER_ADMITTED | TRANSFER_CONSUMED | TRANSFER_DEPARTED)
                    == TRANSFER_ADMITTED | TRANSFER_CONSUMED | TRANSFER_DEPARTED,
            "entry completion lacks successful source/arrival authority"
        );
        let destination_world = self
            .worlds
            .get(&grant.destination)
            .context("completed entry world missing")?;
        let public = destination_world.owner.is_none()
            && destination_world.guild.is_none()
            && crate::dungeon::DUNGEONS
                .iter()
                .any(|(_, _, name, _)| *name == destination_world.name);
        let entrant = destination_world
            .players
            .get(&player)
            .context("completed entry player missing")?;
        ensure!(
            living(entrant)
                && entrant.character.account == grant.account
                && entrant.character.id == grant.character,
            "completed entry character changed"
        );
        let destination = grant.destination;
        let witness = self
            .grants
            .get_mut(key)
            .unwrap()
            .entry_witness
            .take()
            .expect("witness checked above");
        if !public {
            tracing::debug!(
                player,
                "dungeon entry observation suppressed: destination became private"
            );
            return Ok(0);
        }
        let now = self.tick * 50;
        let portal_expires_ms = self.portals.get(&witness.portal).map(|b| b.expires * 50);
        let Some(world) = self.worlds.get_mut(&witness.source) else {
            return Ok(0);
        };
        if now >= witness.observed_ms + LIFETIME_MS
            || world.owner.is_some()
            || world.guild.is_some()
            || !self.portals.get(&witness.portal).is_some_and(|b| {
                b.origin == witness.source
                    && b.destination == Some(destination)
                    && b.expires > self.tick
            })
            || !world.entities.get(&witness.portal).is_some_and(|e| {
                e.published
                    && e.kind == witness.kind
                    && e.conditions(world.now) & crate::condition::bit(crate::condition::INVISIBLE)
                        == 0
            })
        {
            tracing::debug!(
                player,
                portal = witness.portal,
                "dungeon entry observation suppressed: expired source portal"
            );
            return Ok(0);
        }
        let mut delivered = 0;
        for (id, account, character) in witness.observers {
            if !world.players.get(&id).is_some_and(|p| {
                observing(p) && p.character.account == account && p.character.id == character
            }) || !sees(world, id, witness.portal)
            {
                continue;
            }
            let p = world.players.get_mut(&id).unwrap();
            p.dungeon_entries.retain(|e| e.expires_ms > now);
            if p.dungeon_entries.len() >= CAPACITY {
                tracing::warn!(
                    observer = id,
                    "dungeon entry observation capacity reached; event suppressed"
                );
                continue;
            }
            p.dungeon_entries.push_back(view::ObservedDungeonEntry {
                sequence: witness.entrant,
                source_world: witness.source,
                portal: witness.portal,
                portal_kind: witness.kind,
                entrant: witness.entrant,
                account: witness.account,
                name: witness.name.clone(),
                observed_ms: witness.observed_ms,
                // World-local perception cannot consult Directory while it owns this world.
                expires_ms: (witness.observed_ms + LIFETIME_MS)
                    .min(portal_expires_ms.expect("live portal binding checked")),
            });
            delivered += 1;
        }
        tracing::info!(
            player,
            source = witness.source,
            portal = witness.portal,
            entrant = witness.entrant,
            account = witness.account,
            delivered,
            "successful dungeon entry locally observed"
        );
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests;
