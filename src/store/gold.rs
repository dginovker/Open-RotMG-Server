use super::*;

pub(crate) const SCHEMA: &str = "CREATE TABLE gold_rewards(account INTEGER NOT NULL REFERENCES accounts(id),reward TEXT NOT NULL CHECK(reward IN ('discord_link','collection_top_half','first_bug_report','class_star_blue','class_star_red','class_star_orange','class_star_yellow','class_star_white')),awarded_at INTEGER NOT NULL CHECK(awarded_at>0),notified_at INTEGER CHECK(notified_at>0),PRIMARY KEY(account,reward));";
pub(crate) const DISCORD: &str = "discord_link";
const COLLECTION: &str = "collection_top_half";
pub(crate) const AMOUNT: i32 = 100;
const BUG: &str = "first_bug_report";

#[cfg(test)]
mod tests;

// These are project rewards, not historical payments: https://github.com/OpenRotMG/Open-RotMG-Server/issues/421
const CLASS_QUESTS: [(&str, i32, &str); 5] = [
    ("class_star_blue", 75, "reaching Blue Star rank"),
    ("class_star_red", 150, "reaching Red Star rank"),
    ("class_star_orange", 250, "reaching Orange Star rank"),
    ("class_star_yellow", 750, "reaching Yellow Star rank"),
    ("class_star_white", 2500, "reaching White Star rank"),
];

pub(super) fn check_schema(db: &Connection, version: i32) -> Result<()> {
    if version >= 18 {
        let sql: String = db.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='gold_rewards'",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            sql == SCHEMA.trim_end_matches(';'),
            "schema-18 gold reward metadata mismatch; restore the reviewed ledger"
        );
    }
    Ok(())
}

pub(super) fn migrate(db: &Connection, version: i32, exists: bool) -> Result<()> {
    if exists && version < 18 {
        db.execute_batch("ALTER TABLE gold_rewards RENAME TO old_gold_rewards;")?;
        db.execute_batch(SCHEMA)?;
        db.execute_batch(
            "INSERT INTO gold_rewards SELECT * FROM old_gold_rewards; DROP TABLE old_gold_rewards;",
        )?;
    } else if !exists {
        db.execute_batch(SCHEMA)?;
    }
    Ok(())
}

fn details(reward: &str) -> Result<(i32, &'static str)> {
    Ok(match reward {
        DISCORD => (AMOUNT, "linking your Discord account"),
        COLLECTION => (
            AMOUNT,
            "reaching the top 50% of the Collection Log hiscores",
        ),
        BUG => (50, "submitting your first bug report"),
        _ => CLASS_QUESTS
            .iter()
            .find(|r| r.0 == reward)
            .map(|r| (r.1, r.2))
            .with_context(|| format!("unknown gold reward {reward}"))?,
    })
}

pub(crate) struct Gold {
    pub balance: i32,
    pub rewards: Vec<String>,
}
impl Gold {
    pub fn message(&self) -> Result<String> {
        let rewards = self
            .rewards
            .iter()
            .map(|r| details(r))
            .collect::<Result<Vec<_>>>()?;
        Ok(format!(
            "You received {} Gold for {}!",
            rewards.iter().map(|r| r.0).sum::<i32>(),
            rewards
                .iter()
                .map(|r| r.1)
                .collect::<Vec<_>>()
                .join(" and ")
        ))
    }
}

pub(super) fn grant(db: &Connection, account: i32, reward: &str) -> Result<bool> {
    let (amount, _) = details(reward)?;
    if db.execute("INSERT INTO gold_rewards(account,reward,awarded_at) VALUES(?1,?2,unixepoch()) ON CONFLICT(account,reward) DO NOTHING", params![account,reward])? == 0 {
        return Ok(false);
    }
    ensure!(db.execute("UPDATE accounts SET credits=credits+?2 WHERE id=?1 AND credits BETWEEN 0 AND 2147483647-?2", params![account,amount])? == 1, "gold reward wallet missing, invalid or overflowing for account {account}");
    Ok(true)
}

pub(super) fn class_quests(
    db: &Connection,
    account: i32,
    stars: i32,
    classes: usize,
) -> Result<bool> {
    ensure!(classes > 0, "class-quest reward requires playable classes");
    if stars < classes as i32
        || db.query_row(
            "SELECT EXISTS(SELECT 1 FROM shore_bots WHERE account=?1)",
            [account],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Ok(false);
    }
    let mut granted = false;
    for (index, (reward, amount, _)) in CLASS_QUESTS.iter().enumerate() {
        if stars >= ((index + 1) * classes) as i32 && grant(db, account, reward)? {
            granted = true;
            tracing::info!(
                account,
                reward,
                amount,
                stars,
                "class-quest gold reward staged"
            );
        }
    }
    Ok(granted)
}

pub(super) fn collection(db: &Connection, account: i32) -> Result<bool> {
    let eligible: bool = db.query_row("WITH scores AS (
SELECT a.id,count(l.item) AS score FROM accounts a JOIN competitors c ON c.account=a.id LEFT JOIN collections l ON l.account=a.id
WHERE NOT EXISTS(SELECT 1 FROM shore_bots b WHERE b.account=a.id) GROUP BY a.id
), ranked AS (SELECT id,score,rank() OVER(ORDER BY score DESC) AS place,count(*) OVER() AS players FROM scores)
SELECT EXISTS(SELECT 1 FROM ranked WHERE id=?1 AND score>0 AND place<=(players+1)/2)
AND EXISTS(SELECT 1 FROM credentials WHERE account=?1)", [account], |r| r.get(0))?;
    if eligible {
        grant(db, account, COLLECTION)
    } else {
        Ok(false)
    }
}

impl Store {
    pub(crate) async fn reward_first_bug(&self, account: i32) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let human: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM shore_bots WHERE account=?1)", [account], |r| r.get(0))?;
            ensure!(human, "bug report reward requires a human game account");
            let granted = grant(&tx, account, BUG)?;
            tx.commit()?;
            tracing::info!(account, granted, "first bug report gold reward committed");
            Ok(())
        }).await
    }
    pub(crate) async fn gold(&self, account: i32) -> Result<Gold> {
        let legacy_base = self.legacy_base;
        let classes = self.classes.clone();
        self.call(move |db| {
            let tx = db.transaction()?;
            let balance = rewarded_presentation(&tx, account, legacy_base, &classes)?.0.credits;
            ensure!(balance >= 0, "negative gold balance for account {account}");
            let mut q = tx.prepare("SELECT reward FROM gold_rewards WHERE account=?1 AND notified_at IS NULL ORDER BY awarded_at,reward")?;
            let rewards = q.query_map([account], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            drop(q);
            tx.commit()?;
            Ok(Gold { balance, rewards })
        }).await
    }
    pub(crate) async fn swap_fame(&self, account: i32, fame: i32) -> Result<(i32, i32)> {
        ensure!(fame > 0 && fame % 2 == 0, "invalid fame swap amount {fame}");
        self.call(move |db| {
            let tx = db.transaction()?;
            let balance: i32 = tx.query_row("SELECT balance FROM account_fame WHERE account=?1", [account], |r| r.get(0)).context("account fame wallet missing")?;
            ensure!(balance >= fame, Rejected("Not enough fame."));
            tx.execute("UPDATE account_fame SET balance=balance-?2 WHERE account=?1", params![account, fame])?;
            tx.execute("UPDATE accounts SET credits=credits+?2 WHERE id=?1", params![account, fame / 2])?;
            let balances = tx.query_row("SELECT balance,credits FROM account_fame JOIN accounts ON accounts.id=account WHERE account=?1", [account], |r| Ok((r.get(0)?, r.get(1)?)))?;
            tx.commit()?;
            Ok(balances)
        }).await
    }
    pub(crate) async fn gold_notified(&self, account: i32, rewards: Vec<String>) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            for reward in rewards {
                ensure!(tx.execute("UPDATE gold_rewards SET notified_at=unixepoch() WHERE account=?1 AND reward=?2 AND notified_at IS NULL", params![account,reward])? == 1, "gold notification receipt changed for account {account}");
            }
            tx.commit()?;
            Ok(())
        }).await
    }
}
