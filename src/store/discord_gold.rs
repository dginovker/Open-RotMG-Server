use super::*;

pub(super) const SCHEMA: &str = "CREATE TABLE discord_gold_grants(request_id TEXT PRIMARY KEY NOT NULL,account INTEGER NOT NULL REFERENCES accounts(id),discord_id TEXT NOT NULL,actor_id TEXT NOT NULL,amount INTEGER NOT NULL CHECK(typeof(amount)='integer' AND amount BETWEEN 1 AND 2147483647));";

pub(super) fn check_schema(db: &Connection, version: i32) -> Result<()> {
    if version >= 19 {
        let sql: Option<String> = db
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='discord_gold_grants'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        ensure!(
            sql.as_deref() == Some(SCHEMA.trim_end_matches(';')),
            "schema-19 Discord gold receipts missing or mismatched; restore reviewed state instead of allowing duplicate grants"
        );
    }
    Ok(())
}

impl Store {
    pub async fn grant_discord_gold(
        &self,
        discord: String,
        amount: i32,
        request: String,
        actor: String,
    ) -> Result<(i32, i32)> {
        for id in [&discord, &request, &actor] {
            hiscores::discord_id(id)?;
        }
        ensure!(amount > 0, "gold amount must be positive");
        self.call(move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let previous: Option<(i32, String, String, i32)> = tx.query_row(
                "SELECT account,discord_id,actor_id,amount FROM discord_gold_grants WHERE request_id=?1",
                [&request], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ).optional()?;
            let replayed = previous.is_some();
            let account = if let Some((account, target, author, value)) = previous {
                ensure!((target, author, value) == (discord.clone(), actor.clone(), amount), "gold request ID reused with different parameters");
                account
            } else {
                let account: i32 = tx.query_row(
                    "SELECT l.account FROM discord_links l JOIN credentials c ON c.account=l.account JOIN accounts a ON a.id=l.account WHERE l.discord_id=?1 AND a.guid IS NOT NULL AND NOT EXISTS(SELECT 1 FROM shore_bots b WHERE b.account=l.account)",
                    [&discord], |r| r.get(0),
                ).optional()?.context("target user has no linked registered game account; they must use /link first")?;
                ensure!(tx.execute("UPDATE accounts SET credits=credits+?2 WHERE id=?1 AND credits BETWEEN 0 AND 2147483647-?2", params![account,amount])? == 1, "gold balance would exceed 2147483647 or wallet is invalid");
                tx.execute("INSERT INTO discord_gold_grants(request_id,account,discord_id,actor_id,amount) VALUES(?1,?2,?3,?4,?5)", params![request,account,discord,actor,amount])?;
                account
            };
            let balance: i32 = tx.query_row("SELECT credits FROM accounts WHERE id=?1", [account], |r| r.get(0))?;
            tx.commit()?;
            tracing::info!(account, discord_id=%discord, actor_id=%actor, request_id=%request, amount, balance, replayed, "Discord gold grant confirmed");
            Ok((account, balance))
        }).await
    }
}

#[cfg(test)]
mod tests;
