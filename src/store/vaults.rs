use super::*;

pub const VAULT_CHESTS: i32 = 20;
pub const VAULT_PRICE: i32 = 500;
const FULL_MASK: i32 = (1 << VAULT_CHESTS) - 1;

pub(super) fn provision(db: &Connection, account: i32) -> Result<()> {
    db.execute(
        "INSERT INTO vaults(account,chest,items,revision) VALUES(?1,0,?2,0)",
        params![account, serde_json::to_string(&[-1; 8])?],
    )?;
    Ok(())
}

pub(super) fn read(db: &Connection, account: i32) -> Result<Vec<Vault>> {
    let mask: i32 = db.query_row(
        "SELECT vault_mask FROM accounts WHERE id=?1",
        [account],
        |r| r.get(0),
    )?;
    ensure!(
        mask & 1 == 1 && mask & !FULL_MASK == 0,
        "invalid vault ownership for account {account}"
    );
    let mut q =
        db.prepare("SELECT chest,items,revision FROM vaults WHERE account=?1 ORDER BY chest")?;
    let rows = q
        .query_map([account], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i32>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut found = 0;
    let mut vaults = Vec::new();
    for (chest, items, revision) in rows {
        let vault = Vault {
            account,
            chest,
            items: serde_json::from_str(&items)?,
            revision,
        };
        validate(&vault)?;
        ensure!(found & (1 << chest) == 0, "duplicate vault chest identity");
        found |= 1 << chest;
        vaults.push(vault);
    }
    ensure!(
        found == mask,
        "vault ownership/rows disagree for account {account}; restore reviewed state"
    );
    Ok(vaults)
}

fn validate(vault: &Vault) -> Result<()> {
    ensure!(
        (0..VAULT_CHESTS).contains(&vault.chest)
            && vault.items.len() == 8
            && vault.items.iter().all(|i| (-1..=65535).contains(i))
            && (0..=i32::MAX).contains(&vault.revision),
        "invalid canonical eight-slot vault"
    );
    Ok(())
}

pub(super) fn save(db: &Connection, vault: &Vault) -> Result<()> {
    validate(vault)?;
    ensure!(vault.revision < i32::MAX, "vault revision exhausted");
    read(db, vault.account)?;
    let n = db.execute("UPDATE vaults SET items=?4,revision=revision+1 WHERE account=?1 AND chest=?2 AND revision=?3",
        params![vault.account, vault.chest, vault.revision, serde_json::to_string(&vault.items)?])?;
    ensure!(
        n == 1,
        "vault conflict: account {}, chest {}, revision {}",
        vault.account,
        vault.chest,
        vault.revision
    );
    Ok(())
}

pub(super) fn migrate(db: &Connection, version: i32) -> Result<()> {
    let mask: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('accounts') WHERE name='vault_mask')",
        [],
        |r| r.get(0),
    )?;
    // table_info omits generated columns, which must not disappear during migration.
    let columns = {
        let mut q = db.prepare("SELECT name FROM pragma_table_xinfo('vaults') ORDER BY cid")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let expected: &[&str] = if version >= 13 {
        &["account", "chest", "items", "revision"]
    } else {
        &["account", "items", "revision"]
    };
    ensure!(
        (0..=19).contains(&version)
            && mask == (version >= 13)
            && columns
                .iter()
                .map(String::as_str)
                .eq(expected.iter().copied()),
        "vault schema/version/column disagreement; restore reviewed state"
    );
    let account_reference: bool = db.query_row(
        r#"SELECT count(*)=1 AND count(*) FILTER(WHERE "table"='accounts' AND "from"='account' AND "to"='id' AND seq=0 AND on_update='NO ACTION' AND on_delete='NO ACTION')=1 FROM pragma_foreign_key_list('vaults')"#,
        [], |r| r.get(0),
    )?;
    ensure!(account_reference, "unsupported vault account foreign key");
    if version < 13 {
        db.execute_batch("ALTER TABLE accounts ADD COLUMN vault_mask INTEGER NOT NULL DEFAULT 1 CHECK(typeof(vault_mask)='integer' AND vault_mask BETWEEN 1 AND 1048575 AND (vault_mask & 1)=1);
ALTER TABLE vaults RENAME TO legacy_vaults;
CREATE TABLE vaults(account INTEGER NOT NULL REFERENCES accounts(id),chest INTEGER NOT NULL CHECK(typeof(chest)='integer' AND chest BETWEEN 0 AND 19),items TEXT NOT NULL CHECK(json_array_length(items)=8),revision INTEGER NOT NULL DEFAULT 0 CHECK(typeof(revision)='integer' AND revision BETWEEN 0 AND 2147483647),PRIMARY KEY(account,chest));
INSERT INTO vaults(account,chest,items,revision) SELECT account,0,items,revision FROM legacy_vaults;
INSERT INTO vaults(account,chest,items,revision) SELECT id,0,'[-1,-1,-1,-1,-1,-1,-1,-1]',0 FROM accounts WHERE id NOT IN(SELECT account FROM legacy_vaults);
DROP TABLE legacy_vaults;")?;
    }
    for (table, column, expected_type, expected_default, expected_key) in [
        ("vaults", "account", "INTEGER", None, 1),
        ("vaults", "chest", "INTEGER", None, 2),
        ("vaults", "items", "TEXT", None, 0),
        ("vaults", "revision", "INTEGER", Some("0"), 0),
        ("accounts", "vault_mask", "INTEGER", Some("1"), 0),
    ] {
        let (kind, not_null, default, key, hidden): (String, bool, Option<String>, i32, i32) = db.query_row(
            r#"SELECT type,"notnull",dflt_value,pk,hidden FROM pragma_table_xinfo(?1) WHERE name=?2"#,
            params![table, column], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        )?;
        ensure!(
            kind == expected_type
                && not_null
                && default.as_deref() == expected_default
                && key == expected_key
                && hidden == 0,
            "unsupported canonical vault metadata: {table}.{column}"
        );
    }
    let accounts = {
        let mut q = db.prepare("SELECT id FROM accounts")?;
        q.query_map([], |r| r.get::<_, i32>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for account in accounts {
        read(db, account)?;
    }
    let broken: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check('vaults'))",
        [],
        |r| r.get(0),
    )?;
    ensure!(!broken, "foreign vault ownership");
    Ok(())
}

impl Store {
    pub async fn vaults(&self, account: i32) -> Result<Vec<Vault>> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let result = read(&tx, account)?;
            tx.commit()?;
            Ok(result)
        })
        .await
    }

    pub async fn vault(&self, account: i32, chest: i32) -> Result<Vault> {
        self.vaults(account)
            .await?
            .into_iter()
            .find(|v| v.chest == chest)
            .context("vault chest is not owned")
    }

    pub async fn purchase_vault(
        &self,
        write: InventoryWrite,
        chest: i32,
    ) -> Result<Option<(i32, i32, Vault)>> {
        ensure!(
            (1..VAULT_CHESTS).contains(&chest) && (0..i32::MAX).contains(&write.revision),
            "invalid vault purchase reservation"
        );
        self.call(move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let owned = read(&tx, write.account)?;
            ensure!(!owned.iter().any(|v| v.chest == chest), "vault chest already owned");
            let (balance, registered): (i32, bool) = tx.query_row("SELECT credits,EXISTS(SELECT 1 FROM credentials WHERE account=accounts.id) FROM accounts WHERE id=?1", [write.account], |r| Ok((r.get(0)?,r.get(1)?)))?;
            ensure!(balance >= 0 && registered, "invalid balance or unregistered vault buyer");
            let (raw, items): (String, String) = tx.query_row("SELECT stats,items FROM characters WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1", params![write.id,write.account,write.revision], |r| Ok((r.get(0)?,r.get(1)?))).context("stale, dead or foreign vault buyer")?;
            let ch: Character = serde_json::from_str(&raw)?;
            ensure!(ch.hp > 0 && serde_json::from_str::<Vec<i32>>(&items)? == write.items, "vault buyer reservation changed");
            if balance < VAULT_PRICE { return Ok(None); }
            let balance = balance - VAULT_PRICE;
            ensure!(tx.execute("UPDATE accounts SET credits=?2,vault_mask=vault_mask|?3 WHERE id=?1", params![write.account,balance,1 << chest])? == 1, "vault buyer disappeared");
            let vault = Vault { account: write.account, chest, items: vec![-1;8], revision: 0 };
            tx.execute("INSERT INTO vaults(account,chest,items,revision) VALUES(?1,?2,?3,0)", params![vault.account,chest,serde_json::to_string(&vault.items)?])?;
            ensure!(tx.execute("UPDATE characters SET revision=revision+1 WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1", params![write.id,write.account,write.revision])? == 1, "vault purchase lost character reservation");
            read(&tx, write.account)?;
            tx.commit()?;
            tracing::info!(account=write.account,chest,price=VAULT_PRICE,balance,"vault ownership committed");
            Ok(Some((write.revision + 1,balance,vault)))
        }).await
    }
}
