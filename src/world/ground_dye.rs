use super::*;

#[derive(Clone)]
pub(super) struct Reservation {
    bag: Entity,
    expiry: u64,
    textures: [u32; 2],
}

impl Reservation {
    pub(super) fn bag_id(&self) -> i32 {
        self.bag.id
    }
}

impl World {
    pub(super) fn reserve_ground_dye(&self, id: i32, slot: Slot) -> Result<Reservation> {
        let p = &self.players[&id];
        let bag = &self.entities[&slot.object];
        ensure!(
            bag.kind != 0x504
                && !self.vaults.contains_key(&bag.id)
                && !self.closed_vaults.contains_key(&bag.id)
                && self
                    .bag_expiry
                    .get(&bag.id)
                    .is_some_and(|expiry| *expiry > self.now),
            controller("appearance source must be an ephemeral ground bag, not a Vault chest")
        );
        ensure!(
            cooperative::ready(p) && p.known.contains(&bag.id),
            controller("appearance source is not known to a ready player")
        );
        Ok(Reservation {
            bag: bag.clone(),
            expiry: self.bag_expiry[&bag.id],
            textures: [p.character.tex1, p.character.tex2],
        })
    }

    pub(crate) fn validate_ground_dye(&self, plan: &InventoryPlan) -> Result<()> {
        let source = plan
            .ground_dye
            .as_ref()
            .context("ground dye reservation missing")?;
        let (slot, tex1, tex2) = plan.appearance.context("ground dye appearance missing")?;
        let p = self
            .players
            .get(&plan.player)
            .context("ground dye player disappeared")?;
        let bag = self
            .entities
            .get(&source.bag.id)
            .context("ground dye source disappeared")?;
        ensure!(
            p.pending
                && p.active()
                && p.character.hp > 0
                && p.character.id == plan.character
                && p.character.account == plan.account
                && p.character.revision == plan.revision
                && p.character.items == plan.items
                && [p.character.tex1, p.character.tex2] == source.textures,
            "ground dye character reservation changed"
        );
        // The full snapshot is the ephemeral bag's version; legal writers cannot mutate it while reserved.
        ensure!(
            slot.object == source.bag.id
                && slot.object != plan.player
                && source.bag.items.get(slot.slot) == Some(&slot.kind)
                && bag.kind == source.bag.kind
                && bag.pos.distance(source.bag.pos) == 0.
                && bag.items == source.bag.items
                && bag.extra_stats == source.bag.extra_stats
                && bag.drop_slots == source.bag.drop_slots
                && bag.reserved
                && bag.accessible(plan.account)
                && self.bag_expiry.get(&bag.id) == Some(&source.expiry)
                && !self.vaults.contains_key(&bag.id)
                && plan.vaults.is_empty(),
            "ground dye source identity, contents, ownership or reservation changed"
        );
        let mut consumed = source.bag.items.clone();
        consumed[slot.slot] = -1;
        ensure!(
            plan.bags == [(bag.id, consumed)],
            "ground dye source publication changed"
        );
        let item = self.content.get(u16::try_from(slot.kind)?)?;
        let mut textures = source.textures;
        for (texture, selected) in textures.iter_mut().zip(item.textures) {
            if let Some(selected) = selected {
                *texture = selected;
            }
        }
        ensure!(
            textures == [tex1, tex2],
            "ground dye appearance reservation changed"
        );
        Ok(())
    }
}
