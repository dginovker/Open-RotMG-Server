use super::*;

#[derive(Clone, Debug)]
pub struct Purchase {
    pub item: u16,
    pub slot: usize,
    pub price: i32,
    pub currency: u8,
}

impl Store {
    pub async fn purchase(
        &self,
        write: InventoryWrite,
        purchase: Purchase,
    ) -> Result<Option<(i32, i32)>> {
        ensure!(
            purchase.price > 0 && purchase.currency <= 1,
            "invalid purchase price or currency"
        );
        ensure!(
            (4..12).contains(&purchase.slot)
                && write.items.len() == 12
                && write.revision >= 0
                && write.revision < i32::MAX,
            "invalid purchase inventory"
        );
        self.call(move |db| {
            let tx = db.transaction()?;
            let registered: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM credentials WHERE account=?1)",
                [write.account], |r| r.get(0))?;
            ensure!(registered, "purchase requires a registered account");
            let raw: String = tx.query_row(
                "SELECT items FROM characters WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1",
                params![write.id, write.account, write.revision], |r| r.get(0))
                .context("purchase conflict: dead, foreign or stale character")?;
            let mut items: Vec<i32> = serde_json::from_str(&raw)?;
            ensure!(items.len() == 12 && items[purchase.slot] == -1, "purchase slot is not empty");
            items[purchase.slot] = i32::from(purchase.item);
            ensure!(items == write.items, "purchase must add exactly one item");
            let (read, debit) = match purchase.currency {
                0 => ("SELECT credits FROM accounts WHERE id=?1", "UPDATE accounts SET credits=credits-?2 WHERE id=?1 AND credits>=?2"),
                1 => ("SELECT balance FROM account_fame WHERE account=?1", "UPDATE account_fame SET balance=balance-?2 WHERE account=?1 AND balance>=?2"),
                _ => unreachable!(),
            };
            let balance: i32 = tx.query_row(read, [write.account], |r| r.get(0))
                .context("purchase wallet missing or invalid")?;
            ensure!(balance >= 0, "negative purchase wallet");
            if balance < purchase.price {
                return Ok(None);
            }
            ensure!(tx.execute(debit, params![write.account, purchase.price])? == 1, "purchase debit conflict");
            ensure!(tx.execute(
                "UPDATE characters SET items=?4,revision=revision+1 WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1",
                params![write.id, write.account, write.revision, serde_json::to_string(&items)?])? == 1,
                "purchase inventory conflict");
            tx.commit()?;
            Ok(Some((write.revision + 1, balance - purchase.price)))
        }).await
    }
}
