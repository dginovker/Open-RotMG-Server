use super::*;
use crate::{content::Object, store::Purchase};

// Provisional fixed prices: realm-src 1855c017, vendors/MerchantLists.cs; not authenticated 123.5.1 prices.
// The recovered map has stock but no prices. Unlisted merchandise must never inherit a free offer.
fn price(item: &Object) -> Option<(i32, u8)> {
    Some(match item.kind {
        0x705 | 0x706 => (25, 0),
        0x701 | 0x70a | 0x70b | 0x71f | 0xc19 => (50, 0),
        0xc23 => (80, 0),
        // Target-era offer, not later timed boosters: wikiwiki.jp/rmd/version/build123.5.0.
        0xc42 => (100, 0),
        0xc2e => (200, 0),
        0xc11 => (300, 0),
        0x710 => (400, 0),
        0xb3e => (11250, 1),
        // The recovered stock has five doses; retain the comparator's fixed elixir offer.
        0xb16 => (500, 1),
        // Preserved pre-Pet-Yard table: wikiwiki.jp/rmd/old/ペット and アイテム売り場.
        0x1513 => (190, 0),
        0x151b | 0x151e => (490, 0),
        0xa50 => (35, 1),
        0xa51 => (50, 1),
        0xb0d => (150, 1),
        0xb0f => (100, 1),
        // Preserved old-shop table: https://wikiwiki.jp/rmd/アイテム売り場#DrakeEgg
        0xaa9 | 0xaf3 => (190, 1),
        0xaf1 => (180, 1),
        0xaef => (690, 1),
        _ if item.class == "Dye" => (if item.name.contains("Cloth") { 120 } else { 60 }, 0),
        _ => return None,
    })
}

fn value(e: &Entity, stat: u8) -> Result<i32> {
    e.extra_stats
        .iter()
        .find(|(id, _)| *id == stat)
        .map(|(_, value)| *value)
        .with_context(|| format!("merchant {} missing stat {stat}", e.id))
}

pub(super) fn configure(e: &mut Entity, content: &Content) -> Result<()> {
    let item = content.get(u16::try_from(value(e, 34)?)?)?;
    ensure!(
        matches!(item.class.as_str(), "Equipment" | "Dye"),
        "merchant stock is not an item"
    );
    ensure!(value(e, 42)? >= 0, "invalid merchant stock count");
    if let Some((price, currency)) = price(item) {
        e.extra_stats
            .extend([(36, price), (40, i32::from(currency))]);
    } else {
        tracing::warn!(merchant=e.id, item=item.kind, name=%item.name, "merchant unavailable: no sourced shop price configured");
    }
    // The static stock does not rotate; don't advertise the recovered snapshot's expired countdown.
    e.extra_stats.retain(|(id, _)| *id != 43);
    e.extra_stats.push((43, -1));
    if !e.extra_stats.iter().any(|(id, _)| *id == 45) {
        e.extra_stats.push((45, 0));
    }
    Ok(())
}

pub(crate) fn reply(success: bool, message: &str) -> Packet {
    let mut out = Writer::new();
    out.i32(i32::from(!success)).utf(message);
    Packet {
        id: wire::BUY_RESULT,
        body: out.0,
    }
}

#[derive(Clone)]
pub(crate) struct VaultPurchase {
    pub seller: i32,
    pub opened: i32,
    pub chest: i32,
}

impl World {
    pub fn buy(&mut self, id: i32, merchant: i32) -> Result<InventoryPlan> {
        let p = self.players.get(&id).context("purchase owner missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "Cannot buy while busy, paused or dead."
        );
        ensure!(
            p.active() && !p.pending && !p.paused && p.character.hp > 0,
            controller("Cannot buy while busy, paused or dead.")
        );
        if self.closed_vaults.contains_key(&merchant) {
            return self.buy_vault(id, merchant);
        }
        let e = self
            .entities
            .get(&merchant)
            .ok_or_else(|| controller("Merchant no longer exists."))?;
        ensure!(
            self.content.get(e.kind)?.class == "Merchant"
                && e.accessible(p.character.account)
                && p.known.contains(&merchant)
                && p.pos.distance(e.pos) < 1.5,
            controller("Move closer to the merchant.")
        );
        ensure!(
            !e.reserved,
            controller("This merchant is processing another purchase.")
        );
        ensure!(value(e, 42)? > 0, controller("This merchant is sold out."));
        ensure!(
            p.presentation.stars() >= value(e, 45)?,
            controller("Insufficient star rank.")
        );
        let item = u16::try_from(value(e, 34)?)?;
        let price = value(e, 36).context("This item has no configured shop price.")?;
        let currency = u8::try_from(value(e, 40)?)?;
        ensure!(
            price > 0 && currency <= 1,
            "Merchant has an invalid price or currency."
        );
        let slot = (4..12)
            .find(|slot| p.character.items[*slot] == -1)
            .ok_or_else(|| controller("Not enough inventory space."))?;
        let mut items = p.character.items.clone();
        items[slot] = i32::from(item);
        self.cancel_trade(id, "Trade cancelled by purchase.");
        let mut plan = self.plan(id, items, None);
        plan.purchase = Some((
            merchant,
            Purchase {
                item,
                slot,
                price,
                currency,
            },
        ));
        self.players.get_mut(&id).unwrap().pending = true;
        self.entities.get_mut(&merchant).unwrap().reserved = true;
        Ok(plan)
    }

    fn buy_vault(&mut self, id: i32, seller: i32) -> Result<InventoryPlan> {
        let p = &self.players[&id];
        let account = p.character.account;
        let e = self
            .entities
            .get(&seller)
            .context("Closed chest no longer exists.")?;
        let chest = *self
            .closed_vaults
            .get(&seller)
            .context("Unknown closed chest identity.")?;
        ensure!(
            self.name == "Vault"
                && self.owner == Some(account)
                && e.kind == 0x505
                && e.accessible(account),
            "Move closer to your closed Vault chest."
        );
        ensure!(
            p.known.contains(&seller) && p.pos.distance(e.pos) < 1.5,
            controller("Move closer to your closed Vault chest.")
        );
        ensure!(
            !e.reserved
                && self.vaults.len() < crate::store::VAULT_CHESTS as usize
                && !self.vaults.values().any(|v| v.chest == chest),
            controller("Chest is already owned or reserved.")
        );
        ensure!(
            value(e, 36)? == crate::store::VAULT_PRICE && value(e, 40)? == 0,
            "Invalid chest sale or insufficient star rank."
        );
        ensure!(
            p.presentation.stars() >= value(e, 45)?,
            controller("Invalid chest sale or insufficient star rank.")
        );
        let pos = e.pos;
        let items = p.character.items.clone();
        // Allocate the actual owned object before spending; publication must not depend on a later spawn.
        let opened = self.spawn_entity(0x504, pos, vec![-1; 8])?;
        let e = self.entities.get_mut(&opened).unwrap();
        e.published = false;
        e.reserved = true;
        e.extra_stats.push((54, account));
        self.cancel_trade(id, "Trade cancelled by Vault purchase.");
        let mut plan = self.plan(id, items, None);
        plan.vault_purchase = Some(VaultPurchase {
            seller,
            opened,
            chest,
        });
        self.players.get_mut(&id).unwrap().pending = true;
        self.entities.get_mut(&seller).unwrap().reserved = true;
        Ok(plan)
    }

    pub fn finish_vault_purchase(
        &mut self,
        plan: InventoryPlan,
        result: Result<Option<(i32, i32, Vault)>>,
    ) -> Result<()> {
        let player = plan.player;
        let publication = self.publish_vault_purchase(plan, result);
        if let Err(error) = &publication {
            self.poison(player, format!("{error:#}"))?;
        }
        publication
    }

    fn publish_vault_purchase(
        &mut self,
        plan: InventoryPlan,
        result: Result<Option<(i32, i32, Vault)>>,
    ) -> Result<()> {
        let committed = result?;
        let purchase = plan
            .vault_purchase
            .context("Vault purchase reservation missing")?;
        let p = self
            .players
            .get(&plan.player)
            .context("Vault buyer disappeared")?;
        let seller = self
            .entities
            .get(&purchase.seller)
            .context("Vault seller disappeared")?;
        let opened = self
            .entities
            .get(&purchase.opened)
            .context("Reserved owned chest disappeared")?;
        ensure!(
            p.pending
                && p.persistence_failure.is_none()
                && p.character.id == plan.character
                && p.character.account == plan.account
                && p.character.revision == plan.revision
                && p.character.items == plan.items
                && self.owner == Some(plan.account)
                && self.closed_vaults.get(&purchase.seller) == Some(&purchase.chest)
                && seller.kind == 0x505
                && seller.reserved
                && seller.accessible(plan.account)
                && value(seller, 36)? == crate::store::VAULT_PRICE
                && value(seller, 40)? == 0
                && opened.kind == 0x504
                && opened.reserved
                && !opened.published
                && opened.pos.distance(seller.pos) == 0.
                && opened.items == vec![-1; 8]
                && opened.extra_stats.contains(&(54, plan.account)),
            "Vault purchase identity, ownership or reservation changed"
        );
        if let Some((revision, balance, vault)) = committed {
            ensure!(
                plan.revision.checked_add(1) == Some(revision)
                    && balance >= 0
                    && vault.account == plan.account
                    && vault.chest == purchase.chest
                    && vault.revision == 0
                    && vault.items == opened.items
                    && !self.vaults.values().any(|v| v.chest == vault.chest),
                "invalid committed Vault purchase receipt"
            );
            self.entities.get_mut(&purchase.seller).unwrap().reserved = false;
            self.remove_entity(purchase.seller);
            self.closed_vaults.remove(&purchase.seller);
            let opened = self.entities.get_mut(&purchase.opened).unwrap();
            opened.reserved = false;
            opened.published = true;
            self.vaults.insert(purchase.opened, vault);
            let p = self.players.get_mut(&plan.player).unwrap();
            p.character.revision = revision;
            p.presentation.credits = balance;
            p.pending = false;
            let packet = reply(true, "Vault chest purchased!");
            send(p, packet.id, packet.body);
            tracing::info!(
                player = plan.player,
                account = plan.account,
                chest = purchase.chest,
                entity = purchase.opened,
                balance,
                revision,
                "paid Vault chest published"
            );
        } else {
            self.entities.get_mut(&purchase.opened).unwrap().reserved = false;
            self.remove_entity(purchase.opened);
            self.entities.get_mut(&purchase.seller).unwrap().reserved = false;
            let p = self.players.get_mut(&plan.player).unwrap();
            p.pending = false;
            let packet = reply(false, "Not enough gold.");
            send(p, packet.id, packet.body);
        }
        Ok(())
    }

    pub fn finish_purchase(
        &mut self,
        plan: InventoryPlan,
        result: Result<Option<(i32, i32)>>,
    ) -> Result<()> {
        let (merchant, purchase) = plan
            .purchase
            .as_ref()
            .context("purchase reservation missing")?;
        let merchant = *merchant;
        let p = self
            .players
            .get(&plan.player)
            .context("purchase owner disappeared")?;
        ensure!(
            p.pending
                && p.character.revision == plan.revision
                && p.character.account == plan.account
                && p.character.id == plan.character,
            "stale or foreign purchase completion"
        );
        let e = self
            .entities
            .get(&merchant)
            .context("reserved merchant disappeared")?;
        ensure!(
            e.reserved
                && value(e, 42)? > 0
                && value(e, 34)? == i32::from(purchase.item)
                && value(e, 36)? == purchase.price
                && value(e, 40)? == i32::from(purchase.currency),
            "merchant reservation changed"
        );
        let committed = self.committed(plan.player, result)?;
        let Some((revision, balance)) = committed else {
            self.entities.get_mut(&merchant).unwrap().reserved = false;
            let p = self.players.get_mut(&plan.player).unwrap();
            p.pending = false;
            let message = if purchase.currency == 0 {
                "Not enough gold."
            } else {
                "Not enough fame."
            };
            let packet = reply(false, message);
            send(p, packet.id, packet.body);
            tracing::info!(
                player = plan.player,
                merchant,
                price = purchase.price,
                currency = purchase.currency,
                "purchase declined: insufficient funds"
            );
            return Ok(());
        };
        let p = self.players.get_mut(&plan.player).unwrap();
        match purchase.currency {
            0 => p.presentation.credits = balance,
            1 => p.presentation.fame = balance,
            _ => unreachable!(),
        }
        let e = self.entities.get_mut(&merchant).unwrap();
        e.reserved = false;
        let (_, stock) = e.extra_stats.iter_mut().find(|(id, _)| *id == 42).unwrap();
        *stock -= 1;
        tracing::info!(
            player = plan.player,
            account = plan.account,
            character = plan.character,
            merchant,
            item = purchase.item,
            price = purchase.price,
            currency = purchase.currency,
            balance,
            stock,
            revision,
            "purchase committed"
        );
        self.finish_inventory(plan, Ok(revision))
    }
}
