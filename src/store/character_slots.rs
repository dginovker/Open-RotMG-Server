use super::*;

#[derive(Debug)]
pub struct SlotReplay;
impl std::fmt::Display for SlotReplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Character slot request timer already used; reload your character list.")
    }
}
impl std::error::Error for SlotReplay {}

pub(super) fn price(slots: i32) -> Result<i32> {
    Ok(match slots {
        1 => 600,
        2 => 800,
        3..=14 => 1000,
        _ => anyhow::bail!("invalid character slot capacity {slots}"),
    })
}

pub(super) fn read(db: &Connection, account: i32) -> Result<i32> {
    let slots: i32 = db.query_row(
        "SELECT character_slots FROM accounts WHERE id=?1",
        [account],
        |r| r.get(0),
    )?;
    price(slots)?;
    let mut q = db.prepare(
        "SELECT timer,slots FROM character_slot_purchases WHERE account=?1 ORDER BY slots",
    )?;
    let receipts = q
        .query_map([account], |r| {
            Ok((r.get::<_, i32>(0)?, r.get::<_, i32>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        receipts.iter().map(|(_, n)| *n).eq(2..=slots)
            && receipts
                .iter()
                .map(|(timer, _)| *timer)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == receipts.len(),
        "character slot purchase history inconsistent for account {account}"
    );
    let living: i32 = db.query_row(
        "SELECT count(*) FROM characters WHERE account=?1 AND alive=1",
        [account],
        |r| r.get(0),
    )?;
    ensure!(
        living <= slots,
        "living characters exceed capacity for account {account}"
    );
    Ok(slots)
}

pub(super) fn migrate(db: &Connection, version: i32) -> Result<()> {
    let column: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('accounts') WHERE name='character_slots')",
        [],
        |r| r.get(0),
    )?;
    let receipts: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='character_slot_purchases')", [], |r| r.get(0))?;
    ensure!(
        column == receipts && (version < 12 || column),
        "character slot capacity or purchase history missing; restore reviewed state"
    );
    if !column {
        db.execute_batch("ALTER TABLE accounts ADD COLUMN character_slots INTEGER NOT NULL DEFAULT 1 CHECK(typeof(character_slots)='integer' AND character_slots BETWEEN 1 AND 14);
CREATE TABLE character_slot_purchases(account INTEGER NOT NULL REFERENCES accounts(id),timer INTEGER NOT NULL CHECK(typeof(timer)='integer' AND timer BETWEEN -2147483648 AND 2147483647),slots INTEGER NOT NULL CHECK(typeof(slots)='integer' AND slots BETWEEN 2 AND 14),PRIMARY KEY(account,timer),UNIQUE(account,slots));
DROP INDEX IF EXISTS one_living_character;
CREATE INDEX living_characters_by_account ON characters(account) WHERE alive=1;
CREATE TRIGGER character_slot_insert BEFORE INSERT ON characters WHEN NEW.alive=1 AND NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account AND character_slots>(SELECT count(*) FROM characters WHERE account=NEW.account AND alive=1)) BEGIN SELECT RAISE(ABORT,'No free character slots.'); END;
CREATE TRIGGER character_slot_update BEFORE UPDATE OF account,alive ON characters WHEN NEW.alive=1 AND NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account AND character_slots>(SELECT count(*) FROM characters WHERE account=NEW.account AND alive=1 AND id<>OLD.id)) BEGIN SELECT RAISE(ABORT,'No free character slots.'); END;
CREATE TRIGGER character_slot_capacity BEFORE UPDATE OF character_slots ON accounts WHEN NEW.character_slots<(SELECT count(*) FROM characters WHERE account=NEW.id AND alive=1) BEGIN SELECT RAISE(ABORT,'Living characters exceed capacity.'); END;")?;
    }
    for name in [
        "living_characters_by_account",
        "character_slot_insert",
        "character_slot_update",
        "character_slot_capacity",
    ] {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?;
        ensure!(
            exists,
            "character slot schema object {name} missing; restore reviewed state"
        );
    }
    let old: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='one_living_character')",
        [],
        |r| r.get(0),
    )?;
    ensure!(!old, "obsolete one-living-character index remains");
    let accounts = {
        let mut q = db.prepare("SELECT id FROM accounts")?;
        q.query_map([], |r| r.get::<_, i32>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for account in accounts {
        read(db, account)?;
    }
    let broken: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check('character_slot_purchases'))",
        [],
        |r| r.get(0),
    )?;
    ensure!(!broken, "character slot purchase ownership inconsistent");
    Ok(())
}

impl Store {
    pub async fn purchase_character_slot(&self, account: i32, timer: i32) -> Result<()> {
        let (before, cost) = self.call(move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let registered: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM credentials WHERE account=?1)", [account], |r| r.get(0))?;
            ensure!(registered, Rejected("Register an account before buying character slots."));
            let slots = read(&tx, account)?;
            let replay: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM character_slot_purchases WHERE account=?1 AND timer=?2)", params![account, timer], |r| r.get(0))?;
            if replay { return Err(SlotReplay.into()); }
            ensure!(slots < 14, Rejected("Character slot limit of 14 reached."));
            // The native request has no quote; distinct timers buy at the current durable price.
            let cost = price(slots)?;
            let next = slots.checked_add(1).context("character slot capacity overflow")?;
            let changed = tx.execute("UPDATE accounts SET credits=credits-?2,character_slots=?3 WHERE id=?1 AND credits>=?2 AND character_slots=?4", params![account, cost, next, slots])?;
            ensure!(changed == 1, Rejected("Not enough Gold for the current character slot price."));
            tx.execute("INSERT INTO character_slot_purchases(account,timer,slots) VALUES(?1,?2,?3)", params![account, timer, next])?;
            read(&tx, account)?;
            tx.commit()?;
            Ok((slots, cost))
        }).await?;
        tracing::info!(
            account,
            timer,
            price = cost,
            previous_capacity = before,
            capacity = before + 1,
            "character slot purchase committed"
        );
        Ok(())
    }
}
