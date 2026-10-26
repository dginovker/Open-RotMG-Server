use super::*;
use sha2::Digest;

impl Directory {
    pub(crate) fn prepared_postclear_key_audit(
        &self,
        view: Option<&view::PlayerView>,
    ) -> Result<Option<serde_json::Value>> {
        if let Some(view) = view {
            let own = self
                .worlds
                .get(&view.session.world)
                .context("audit own world missing")?;
            if own.name != "Oryx's Chamber" || !own.cleared {
                return Ok(None);
            }
        } else {
            ensure!(
                self.clock.paused(),
                "terminal opportunity audit requires paused boundary"
            );
        }
        let positive: Vec<_> = self.worlds.keys().copied().filter(|id| *id > 0).collect();
        let allocation_complete = self.next_world > 0
            && usize::try_from(self.next_world - 1).ok() == Some(positive.len())
            && positive.iter().all(|id| *id < self.next_world);
        let mut actors = Vec::new();
        let mut keys = Vec::new();
        let mut pending = false;
        let mut opening = false;
        let mut cellar = false;
        let mut storage = false;
        for (world, w) in &self.worlds {
            storage |= w.owner.is_some() || w.name == "Vault";
            cellar |= w.name == "Wine Cellar";
            opening |= w.portal_requests.iter().any(|(kind, _)| *kind == 0x700);
            for (player, p) in &w.players {
                actors.push(serde_json::json!({"account":p.character.account,"character":p.character.id,"world":world,"player":player}));
                pending |= p.pending || p.death_pending || p.persistence_failure.is_some();
                if p.character
                    .items
                    .contains(&i32::from(crate::endgame::INCANTATION))
                {
                    keys.push(serde_json::json!({"world":world,"player":player,"account":p.character.account}));
                }
            }
            for e in w.entities.values() {
                if w.content.objects[&e.kind].class == "Container"
                    && e.items.contains(&i32::from(crate::endgame::INCANTATION))
                {
                    keys.push(serde_json::json!({"world":world,"container":e.id}));
                }
                if matches!(e.kind, 0x700 | 0x721) {
                    opening |= e.reserved || e.kind == 0x700 && !e.published;
                    cellar |= e.kind == 0x700 && e.published;
                }
            }
        }
        let mut audit = serde_json::json!({"directory_tick":self.tick,"clock_paused":self.clock.paused(),"next_world":self.next_world,"positive_world_ids":positive,
            "allocation_complete":allocation_complete,"storage_world_seen":storage,"actors":actors,
            "key_sources":keys,"any_pending":pending,"pending_opening":opening,"open_cellar":cellar});
        if let Some(view) = view {
            let raw = serde_json::to_string(view)?;
            audit["normal_view_sha256"] =
                serde_json::json!(format!("{:x}", sha2::Sha256::digest(raw.as_bytes())));
            audit["normal_view_raw"] = serde_json::json!(raw);
            audit["session"] = serde_json::to_value(&view.session)?;
            audit["tick"] = serde_json::json!(view.tick);
            audit["time_ms"] = serde_json::json!(view.time_ms);
            audit["chamber_cleared"] = serde_json::json!(true);
        }
        Ok(Some(audit))
    }
}
