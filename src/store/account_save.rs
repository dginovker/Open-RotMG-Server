use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const DISCLOSURE: &str = "Personal progress only. Guild membership, social targets, trade receipts, rankings and external integrations do not transfer. Guild contribution and Discord Realm Close counts are inert history; Discord commands are unavailable offline. Names, artwork and death descriptions are not anonymized.";
pub(super) const ASSIST_COUNT: &str = "SELECT (SELECT count(*) FROM level_credits WHERE giver=?1)+coalesce((SELECT earned FROM portable_assists WHERE character=?1),0)";

const SCHEMA: &str = "CREATE TABLE save_namespace(id INTEGER PRIMARY KEY CHECK(id=1),namespace TEXT NOT NULL);
CREATE TABLE account_origins(account INTEGER PRIMARY KEY REFERENCES accounts(id),namespace TEXT NOT NULL,source_account INTEGER NOT NULL,UNIQUE(namespace,source_account));
CREATE TABLE portable_assists(character INTEGER PRIMARY KEY REFERENCES characters(id),earned INTEGER NOT NULL CHECK(earned>=0));
CREATE TABLE portable_consumed_levels(recipient INTEGER NOT NULL REFERENCES characters(id),reached_level INTEGER NOT NULL CHECK(reached_level BETWEEN 2 AND 20),PRIMARY KEY(recipient,reached_level));
CREATE TABLE portable_history(account INTEGER PRIMARY KEY REFERENCES accounts(id),guild_fame INTEGER NOT NULL CHECK(guild_fame>=0),realm_unused INTEGER NOT NULL CHECK(realm_unused>=0),realm_spent INTEGER NOT NULL CHECK(realm_spent>=0));";
pub(super) fn migrate(db: &Connection, version: i32) -> Result<()> {
    if version < 22 {
        db.execute_batch(SCHEMA)?;
        db.execute("INSERT INTO save_namespace VALUES(1,?1)", [random_key()])?;
    }
    for statement in SCHEMA.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let name = statement
            .split_whitespace()
            .nth(2)
            .context("invalid portable schema definition")?
            .split('(')
            .next()
            .unwrap();
        let sql: String =
            db.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                r.get(0)
            })?;
        ensure!(sql == statement, "portable save metadata mismatch: {name}");
    }
    let namespace: String =
        db.query_row("SELECT namespace FROM save_namespace WHERE id=1", [], |r| {
            r.get(0)
        })?;
    ensure!(key(&namespace), "invalid save namespace");
    Ok(())
}
fn random_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn key(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub namespace: String,
    pub account: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedCharacter {
    pub character: Character,
    pub assist_earned: u64,
    pub consumed_levels: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Death {
    pub character: Character,
    pub killed_by: String,
    pub died_at: i64,
    pub equipment_fame: i32,
    pub receipt: death_awards::DeathFameReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chest {
    pub chest: i32,
    pub items: Vec<i32>,
    pub revision: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Picture {
    pub id: i32,
    pub name: String,
    pub data_type: i32,
    pub tags: String,
    pub png: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reward {
    pub reward: String,
    pub awarded_at: i64,
    pub notified_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSave {
    pub format: u32,
    pub content: String,
    pub exported_at: u64,
    pub origin: Origin,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub name_chosen: bool,
    pub gold: i32,
    pub fame_balance: i32,
    pub fame_earned: i32,
    pub slots: i32,
    pub slot_purchases: Vec<(i32, i32)>,
    pub vault_mask: i32,
    pub characters: Vec<SavedCharacter>,
    pub deaths: Vec<Death>,
    pub claims: Vec<(u16, i32)>,
    pub vaults: Vec<Chest>,
    pub collections: Vec<(u16, i64)>,
    #[serde(default)]
    pub collection_sources: Vec<(u16, String)>,
    pub rewards: Vec<Reward>,
    pub pictures: Vec<Picture>,
    pub guild_fame: i64,
    pub realm_unused: i64,
    pub realm_spent: i64,
}
fn rows<T>(
    db: &Connection,
    sql: &str,
    account: i32,
    f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    Ok(db
        .prepare(sql)?
        .query_map([account], f)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
fn strict_character(raw: &str, base: [i32; 8]) -> Result<Character> {
    let ch = decode_character(raw, base)?;
    ensure!(
        crate::backup::known_fields(&serde_json::from_str(raw)?, &serde_json::to_value(&ch)?),
        "unknown persisted character field"
    );
    Ok(ch)
}
fn texture(tex: u32) -> Result<()> {
    // 123.5.1 TextureRedrawer.getTexture uses a type byte and an atlas/color value, not a picture ID.
    ensure!(
        matches!(tex >> 24, 0 | 1 | 4 | 5 | 9 | 10) && (tex >> 24 != 0 || tex == 0),
        "unresolved external/editor texture {tex:#x}"
    );
    Ok(())
}
impl AccountSave {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "account save exceeds 32 MiB"
        );
        let saved: Self = serde_json::from_slice(bytes).context("invalid account save")?;
        ensure!(
            crate::backup::known_fields(
                &serde_json::from_slice(bytes)?,
                &serde_json::to_value(&saved)?
            ),
            "unknown nested account save field"
        );
        saved.validate()?;
        Ok(saved)
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(email) = &self.email {ensure!(accounts::email(email)? == *email, "invalid saved email");}
        ensure!(
            self.format == 1 && self.content == "123.5.1" && self.exported_at > 0,
            "unsupported account save format/content/time"
        );
        ensure!(
            key(&self.origin.namespace) && self.origin.account > 0,
            "invalid account origin"
        );
        ensure!(
            self.name.len() <= 64
                && !self.name.is_empty()
                && !self.name.chars().any(char::is_control),
            "invalid saved name"
        );
        ensure!(
            self.gold >= 0
                && self.fame_balance >= 0
                && self.fame_earned >= self.fame_balance
                && self.guild_fame >= 0
                && self.realm_unused >= 0
                && self.realm_spent >= 0,
            "invalid personal balance"
        );
        ensure!(
            self.characters.len() <= 10000 && self.pictures.len() <= pictures::MAX_PER_ACCOUNT,
            "account save capacity exceeded"
        );
        ensure!(
            self.collections.iter().all(|(_, time)| *time > 0),
            "invalid collection timestamp"
        );
        let mut sources = BTreeSet::new();
        ensure!(
            self.collection_sources
                .iter()
                .all(|(item, source)| sources.insert(*item)
                    && self.collections.iter().any(|(id, _)| id == item)
                    && !source.trim().is_empty()
                    && source.chars().count() <= 128
                    && !source.chars().any(char::is_control)),
            "invalid collection source"
        );
        let mut ids = BTreeSet::new();
        for row in &self.characters {
            let ch = &row.character;
            ensure!(
                ch.id > 0
                    && ch.account == self.origin.account
                    && ids.insert(ch.id)
                    && ch.revision >= 0
                    && ch.items.len() == 12
                    && ch.items.iter().all(|i| (-1..=65535).contains(i))
                    && row.assist_earned <= i64::MAX as u64,
                "invalid character identity/inventory/assists"
            );
            ensure!(
                ch.schema == 1
                    && (1..=20).contains(&ch.level)
                    && ch.exp >= level_start(ch.level)?
                    && ch.fame >= 0
                    && ch.hp >= 0
                    && ch.mp >= 0,
                "invalid character progression"
            );
            ch.lifetime.validate()?;
            texture(ch.tex1)?;
            texture(ch.tex2)?;
            ensure!(
                row.consumed_levels
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .len()
                    == row.consumed_levels.len()
                    && row
                        .consumed_levels
                        .iter()
                        .all(|l| (2..=20).contains(l) && i32::from(*l) <= ch.level),
                "invalid consumed levels"
            );
        }
        for death in &self.deaths {
            ensure!(
                ids.contains(&death.character.id) && death.character.account == self.origin.account,
                "invalid death ownership"
            );
            texture(death.character.tex1)?;
            texture(death.character.tex2)?;
            death.receipt.validate()?;
        }
        for pic in &self.pictures {
            ensure!(pic.id > 0, "invalid artwork identity");
            pictures::metadata(&pic.name, pic.data_type, &pic.tags)?;
            pictures::validate_png(pic.data_type, &pic.png)?;
        }
        Ok(())
    }
}
pub(crate) fn export(db: &Connection, account: i32, base: [i32; 8]) -> Result<AccountSave> {
    ensure!(
        !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM shore_bots WHERE account=?1)",
            [account],
            |r| r.get::<_, bool>(0)
        )?,
        "community actors are not portable human accounts"
    );
    let (name, gold, slots, vault_mask): (String, i32, i32, i32) = db.query_row(
        "SELECT name,credits,character_slots,vault_mask FROM accounts WHERE id=?1",
        [account],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let origin = db
        .query_row(
            "SELECT namespace,source_account FROM account_origins WHERE account=?1",
            [account],
            |r| {
                Ok(Origin {
                    namespace: r.get(0)?,
                    account: r.get(1)?,
                })
            },
        )
        .optional()?
        .unwrap_or(Origin {
            namespace: db.query_row(
                "SELECT namespace FROM save_namespace WHERE id=1",
                [],
                |r| r.get(0),
            )?,
            account,
        });
    let mut characters = Vec::new();
    for (id, raw, items, revision, alive) in rows(
        db,
        "SELECT id,stats,items,revision,alive FROM characters WHERE account=?1 ORDER BY id",
        account,
        |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i32>(3)?,
                r.get::<_, bool>(4)?,
            ))
        },
    )? {
        let mut ch = strict_character(&raw, base)?;
        ch.id = id;
        ch.account = origin.account;
        ch.items = serde_json::from_str(&items)?;
        ch.revision = revision;
        ch.alive = alive;
        let earned = db
            .query_row(ASSIST_COUNT, [id], |r| r.get::<_, u64>(0))?
            .max(ch.lifetime.level_assists);
        let consumed = rows(
            db,
            "SELECT reached_level FROM level_credits WHERE recipient=?1 UNION SELECT reached_level FROM portable_consumed_levels WHERE recipient=?1 ORDER BY reached_level",
            id,
            |r| r.get(0),
        )?;
        characters.push(SavedCharacter {
            character: ch,
            assist_earned: earned,
            consumed_levels: consumed,
        });
    }
    let mut deaths = Vec::new();
    for (raw, killed_by, died_at, equipment_fame, receipt) in rows(
        db,
        "SELECT d.snapshot,d.killed_by,d.died_at,d.equipment_fame,d.receipt FROM deaths d JOIN characters c ON c.id=d.character WHERE c.account=?1 ORDER BY d.character",
        account,
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    )? {
        let receipt = death_awards::decode_receipt(&receipt)?;
        let mut ch = receipt.character(&raw, base)?;
        ensure!(
            crate::backup::known_fields(&serde_json::from_str(&raw)?, &serde_json::to_value(&ch)?),
            "unknown archived character field"
        );
        ch.account = origin.account;
        deaths.push(Death {
            character: ch,
            killed_by,
            died_at,
            equipment_fame,
            receipt,
        });
    }
    let (fame_balance, fame_earned) = db.query_row(
        "SELECT balance,earned FROM account_fame WHERE account=?1",
        [account],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let history: (i64, i64, i64) = db
        .query_row(
            "SELECT guild_fame,realm_unused,realm_spent FROM portable_history WHERE account=?1",
            [account],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .unwrap_or((0, 0, 0));
    let guild: i64 = db.query_row(
        "SELECT coalesce(sum(fame),0) FROM guild_members WHERE account=?1",
        [account],
        |r| r.get(0),
    )?;
    let (unused,spent):(i64,i64)=db.query_row("SELECT count(*) FILTER(WHERE used_request IS NULL),count(*) FILTER(WHERE used_request IS NOT NULL) FROM discord_realm_close_tokens WHERE account=?1",[account],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let email = db.query_row("SELECT email FROM credentials WHERE account=?1", [account], |r| r.get::<_,String>(0)).optional()?;
    let saved = AccountSave {
        format: 1,
        content: "123.5.1".into(),
        exported_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        origin,
        email,
        name_chosen: name != format!("Guest{account}"),
        name,
        gold,
        fame_balance,
        fame_earned,
        slots,
        vault_mask,
        characters,
        deaths,
        claims: rows(
            db,
            "SELECT class,character FROM legacy_claims WHERE account=?1 ORDER BY class",
            account,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?,
        slot_purchases: rows(
            db,
            "SELECT timer,slots FROM character_slot_purchases WHERE account=?1 ORDER BY slots",
            account,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?,
        vaults: vaults::read(db, account)?
            .into_iter()
            .map(|v| Chest {
                chest: v.chest,
                items: v.items,
                revision: v.revision,
            })
            .collect(),
        collections: rows(
            db,
            "SELECT item,found_at FROM collections WHERE account=?1 ORDER BY item",
            account,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?,
        collection_sources: if db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))?
            >= 23
        {
            rows(
                db,
                "SELECT item,source FROM collections WHERE account=?1 AND source IS NOT NULL ORDER BY item",
                account,
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
        } else {
            Vec::new()
        },
        rewards: rows(
            db,
            "SELECT reward,awarded_at,notified_at FROM gold_rewards WHERE account=?1 ORDER BY reward",
            account,
            |r| {
                Ok(Reward {
                    reward: r.get(0)?,
                    awarded_at: r.get(1)?,
                    notified_at: r.get(2)?,
                })
            },
        )?,
        pictures: rows(
            db,
            "SELECT id,name,data_type,tags,png FROM pictures WHERE account=?1 ORDER BY id",
            account,
            |r| {
                Ok(Picture {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    data_type: r.get(2)?,
                    tags: r.get(3)?,
                    png: r.get(4)?,
                })
            },
        )?,
        guild_fame: history
            .0
            .checked_add(guild)
            .context("guild history overflow")?,
        realm_unused: history.1.checked_add(unused).context("token overflow")?,
        realm_spent: history.2.checked_add(spent).context("token overflow")?,
    };
    saved.validate()?;
    ensure!(
        serde_json::to_vec(&saved)?.len() as u64 <= MAX_BYTES,
        "account save exceeds 32 MiB"
    );
    character_slots::read(db, account)?;
    death_awards::validate_state(db, base)?;
    Ok(saved)
}

#[derive(Serialize)]
pub struct ImportResult {
    pub account: i32,
    pub login: String,
    pub replaced: bool,
    pub disclosure: &'static str,
}
pub(crate) struct ImportExpectation {
    pub input_sha256: String,
    pub expected: String,
}
pub(crate) fn expected(
    save: &AccountSave,
    plan: &ImportResult,
    input_sha256: &str,
) -> Result<String> {
    let identity = serde_json::to_vec(&(
        "account-import-v1",
        input_sha256,
        &save.origin.namespace,
        save.origin.account,
        plan.account,
        &plan.login,
        plan.replaced,
    ))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}
pub(crate) fn validate_expected(expected: &str) -> Result<()> {
    ensure!(
        key(expected),
        "required preview expectation is malformed; re-preview this file"
    );
    Ok(())
}
pub(crate) fn check_expected(
    save: &AccountSave,
    plan: &ImportResult,
    expectation: &ImportExpectation,
) -> Result<()> {
    validate_expected(&expectation.expected)?;
    ensure!(
        expectation.expected == expected(save, plan, &expectation.input_sha256)?,
        "preview expectation is malformed or changed; re-preview this file and confirm again"
    );
    Ok(())
}
pub(crate) fn preview(db: &Connection, save: &AccountSave) -> Result<ImportResult> {
    let version: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        matches!(version, 21..=23),
        "unsupported local preview schema {version}"
    );
    let existing: Option<i32> = if version == 21 {
        None
    } else {
        db.query_row(
            "SELECT account FROM account_origins WHERE namespace=?1 AND source_account=?2",
            params![save.origin.namespace, save.origin.account],
            |r| r.get(0),
        )
        .optional()?
    };
    let next: i32 = db.query_row("SELECT coalesce(max(id),0)+1 FROM accounts", [], |r| {
        r.get(0)
    })?;
    let account = existing.unwrap_or(next);
    let login = save.email.clone().context("This export has no account email. Export your registered account again before importing.")?;
    let name = if save.name_chosen {
        save.name.clone()
    } else {
        format!("Guest{account}")
    };
    ensure!(
        !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM credentials WHERE email=?1 AND account<>?2)",
            params![login, account],
            |r| r.get::<_, bool>(0)
        )?,
        "unrelated local login collision"
    );
    ensure!(
        !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE name=?1 COLLATE NOCASE AND id<>?2)",
            params![name, account],
            |r| r.get::<_, bool>(0)
        )?,
        "unrelated local name collision"
    );
    Ok(ImportResult {
        account,
        login,
        replaced: existing.is_some(),
        disclosure: DISCLOSURE,
    })
}
fn retained_trade_participation(db: &Connection, account: i32) -> Result<bool> {
    let owned: BTreeSet<i32> = rows(
        db,
        "SELECT id FROM characters WHERE account=?1",
        account,
        |r| r.get(0),
    )?
    .into_iter()
    .collect();
    let mut q = db.prepare("SELECT owners,writes FROM trade_transactions")?;
    for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (owners, writes) = row?;
        let owners: Vec<i32> = serde_json::from_str(&owners)?;
        let value: serde_json::Value = serde_json::from_str(&writes)?;
        let writes: Vec<InventoryWrite> = serde_json::from_value(value.clone())?;
        ensure!(
            crate::backup::known_fields(&value, &serde_json::to_value(&writes)?),
            "unknown retained trade write field"
        );
        if owners.contains(&account)
            || writes
                .iter()
                .any(|w| w.account == account || owned.contains(&w.id))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn import(
    db: &mut Connection,
    save: &AccountSave,
    password: &str,
    replace: bool,
    base: [i32; 8],
    expectation: Option<ImportExpectation>,
) -> Result<ImportResult> {
    save.validate()?;
    accounts::password(password)?;
    let hash = accounts::hash(password)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let plan =
        preview(&tx, save).context("local import identity unavailable; re-preview this file")?;
    if let Some(expectation) = expectation {
        check_expected(save, &plan, &expectation)?;
    }
    ensure!(
        !plan.replaced || replace,
        "origin already imported; explicit replacement and backup required"
    );
    ensure!(
        !replace || plan.replaced,
        "replacement requires matching stable origin"
    );
    let account = plan.account;
    let login = plan.login;
    let name = if save.name_chosen {
        save.name.clone()
    } else {
        format!("Guest{account}")
    };
    let mut next_character: i32 =
        tx.query_row("SELECT coalesce(max(id),0)+1 FROM characters", [], |r| {
            r.get(0)
        })?;
    let mut mapping = BTreeMap::new();
    for row in &save.characters {
        mapping.insert(row.character.id, next_character);
        next_character = next_character
            .checked_add(1)
            .context("character IDs exhausted")?;
    }
    if plan.replaced {
        ensure!(
            !retained_trade_participation(&tx, account)?,
            "replacement cannot preserve retained local trade history; retain state and backup, use a separate local save or full backup restore"
        );
        let related:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM guild_members WHERE account=?1) OR EXISTS(SELECT 1 FROM discord_links WHERE account=?1) OR EXISTS(SELECT 1 FROM discord_gold_grants WHERE account=?1) OR EXISTS(SELECT 1 FROM discord_realm_close_tokens WHERE account=?1)",[account],|r|r.get(0))?;
        ensure!(
            !related,
            "replacement cannot preserve functioning local guild/integration history; retain state and backup, use a fresh separate local save or restore a full local backup"
        );
        for table in [
            "discord_codes",
            "recovery_codes",
            "password_resets",
            "email_password_resets",
            "email_verifications",
            "verified_emails",
            "email_send_attempts",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE account=?1"), [account])?;
        }
        // Retiring an edge must not erase credit earned by an unrelated local character.
        tx.execute("INSERT INTO portable_assists(character,earned) SELECT giver,count(*) FROM level_credits WHERE recipient IN (SELECT id FROM characters WHERE account=?1) AND giver NOT IN (SELECT id FROM characters WHERE account=?1) GROUP BY giver ON CONFLICT(character) DO UPDATE SET earned=earned+excluded.earned",[account])?;
        tx.execute("INSERT OR IGNORE INTO portable_consumed_levels SELECT recipient,reached_level FROM level_credits WHERE giver IN (SELECT id FROM characters WHERE account=?1) AND recipient NOT IN (SELECT id FROM characters WHERE account=?1)",[account])?;
        tx.execute("DELETE FROM level_credits WHERE giver IN (SELECT id FROM characters WHERE account=?1) OR recipient IN (SELECT id FROM characters WHERE account=?1)",[account])?;
        for table in ["portable_assists", "portable_consumed_levels"] {
            let column = if table == "portable_assists" {
                "character"
            } else {
                "recipient"
            };
            tx.execute(&format!("DELETE FROM {table} WHERE {column} IN (SELECT id FROM characters WHERE account=?1)"),[account])?;
        }
        for table in [
            "legacy_claims",
            "character_slot_purchases",
            "vaults",
            "collections",
            "gold_rewards",
            "pictures",
            "portable_history",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE account=?1"), [account])?;
        }
        tx.execute(
            "DELETE FROM deaths WHERE character IN (SELECT id FROM characters WHERE account=?1)",
            [account],
        )?;
        tx.execute("DELETE FROM characters WHERE account=?1", [account])?;
        tx.execute("UPDATE accounts SET guid=?2,name=?3,credits=?4,character_slots=?5,vault_mask=?6 WHERE id=?1",params![account,random_key(),name,save.gold,save.slots,save.vault_mask])?;
    } else {
        tx.execute("INSERT INTO accounts(id,guid,name,credits,character_slots,vault_mask) VALUES(?1,?2,?3,?4,?5,?6)",params![account,random_key(),name,save.gold,save.slots,save.vault_mask])?;
        tx.execute(
            "INSERT INTO account_origins VALUES(?1,?2,?3)",
            params![account, save.origin.namespace, save.origin.account],
        )?;
    }
    tx.execute("INSERT INTO credentials VALUES(?1,?2,?3) ON CONFLICT(account) DO UPDATE SET email=excluded.email,password_hash=excluded.password_hash",params![account,login,hash])?;
    tx.execute("INSERT INTO account_fame VALUES(?1,?2,?3) ON CONFLICT(account) DO UPDATE SET balance=excluded.balance,earned=excluded.earned",params![account,save.fame_balance,save.fame_earned])?;
    tx.execute(
        "INSERT INTO portable_history VALUES(?1,?2,?3,?4)",
        params![
            account,
            save.guild_fame,
            save.realm_unused,
            save.realm_spent
        ],
    )?;
    let remap = |ch: &Character| -> Result<Character> {
        let mut ch = ch.clone();
        ch.id = *mapping
            .get(&ch.id)
            .context("unknown owned character reference")?;
        ch.account = account;
        if !save.name_chosen {
            ch.name = name.clone();
        }
        Ok(ch)
    };
    for row in &save.characters {
        let ch = remap(&row.character)?;
        tx.execute("INSERT INTO characters(id,account,stats,items,revision,alive) VALUES(?1,?2,?3,?4,?5,?6)",params![ch.id,account,serde_json::to_string(&ch)?,serde_json::to_string(&ch.items)?,ch.revision,ch.alive])?;
        tx.execute(
            "INSERT INTO portable_assists VALUES(?1,?2)",
            params![ch.id, row.assist_earned],
        )?;
        for level in &row.consumed_levels {
            tx.execute(
                "INSERT INTO portable_consumed_levels VALUES(?1,?2)",
                params![ch.id, level],
            )?;
        }
    }
    for death in &save.deaths {
        let ch = remap(&death.character)?;
        tx.execute(
            "INSERT INTO deaths VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                ch.id,
                serde_json::to_string(&ch)?,
                death.killed_by,
                death.died_at,
                death.equipment_fame,
                serde_json::to_string(&death.receipt)?
            ],
        )?;
    }
    for (class, id) in &save.claims {
        tx.execute(
            "INSERT INTO legacy_claims VALUES(?1,?2,?3)",
            params![
                account,
                class,
                mapping.get(id).context("unknown claim character")?
            ],
        )?;
    }
    for (timer, slots) in &save.slot_purchases {
        tx.execute(
            "INSERT INTO character_slot_purchases VALUES(?1,?2,?3)",
            params![account, timer, slots],
        )?;
    }
    for chest in &save.vaults {
        tx.execute(
            "INSERT INTO vaults VALUES(?1,?2,?3,?4)",
            params![
                account,
                chest.chest,
                serde_json::to_string(&chest.items)?,
                chest.revision
            ],
        )?;
    }
    for (item, time) in &save.collections {
        tx.execute(
            "INSERT INTO collections(account,item,found_at) VALUES(?1,?2,?3)",
            params![account, item, time],
        )?;
    }
    for (item, source) in &save.collection_sources {
        tx.execute(
            "UPDATE collections SET source=?3 WHERE account=?1 AND item=?2",
            params![account, item, source],
        )?;
    }
    for reward in &save.rewards {
        tx.execute(
            "INSERT INTO gold_rewards(account,reward,awarded_at,notified_at) VALUES(?1,?2,?3,?4)",
            params![
                account,
                reward.reward,
                reward.awarded_at,
                reward.notified_at
            ],
        )?;
    }
    let mut picture_id: i64 =
        tx.query_row("SELECT next_id FROM picture_sequence WHERE id=1", [], |r| {
            r.get(0)
        })?;
    let mut picture_ids = BTreeSet::new();
    for pic in &save.pictures {
        ensure!(picture_ids.insert(pic.id), "duplicate artwork ID");
        tx.execute(
            "INSERT INTO pictures VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                picture_id,
                account,
                pic.name,
                pic.data_type,
                pic.tags,
                pic.png
            ],
        )?;
        picture_id = picture_id
            .checked_add(1)
            .context("picture sequence exhausted")?;
    }
    tx.execute(
        "UPDATE picture_sequence SET next_id=?1 WHERE id=1",
        [picture_id],
    )?;
    let total: usize = tx.query_row("SELECT count(*) FROM pictures", [], |r| r.get(0))?;
    pictures::validate_capacity(save.pictures.len(), total)?;
    character_slots::read(&tx, account)?;
    vaults::read(&tx, account)?;
    death_awards::validate_state(&tx,base).context("replacement/import cannot preserve local shared history; retain state and backup, use a separate local save or full backup restore")?;
    tx.commit()?;
    Ok(ImportResult {
        account,
        login,
        replaced: plan.replaced,
        disclosure: DISCLOSURE,
    })
}
impl Store {
    pub(crate) async fn export_account_save(&self, account: i32) -> Result<AccountSave> {
        let base = self.legacy_base;
        self.call(move |db| {
            let tx = db.transaction()?;
            let saved = export(&tx, account, base)?;
            tx.commit()?;
            Ok(saved)
        })
        .await
    }
    // Private domain validation has no staged file; CLI commits use the checked entry point.
    pub(crate) async fn import_account_save(
        &self,
        save: AccountSave,
        password: String,
        replace: bool,
    ) -> Result<ImportResult> {
        self.import_account_save_with_expectation(save, password, replace, None)
            .await
    }
    pub(crate) async fn import_expected_account_save(
        &self,
        save: AccountSave,
        password: String,
        replace: bool,
        expectation: ImportExpectation,
    ) -> Result<ImportResult> {
        self.import_account_save_with_expectation(save, password, replace, Some(expectation))
            .await
    }
    async fn import_account_save_with_expectation(
        &self,
        save: AccountSave,
        password: String,
        replace: bool,
        expectation: Option<ImportExpectation>,
    ) -> Result<ImportResult> {
        let base = self.legacy_base;
        for ch in save
            .characters
            .iter()
            .map(|r| &r.character)
            .chain(save.deaths.iter().map(|d| &d.character))
        {
            ch.effective(&self.content)?;
            if let Some(kind) = ch.pet_kind {
                self.content.permanent_companion(kind)?;
            }
        }
        self.call(move |db| import(db, &save, &password, replace, base, expectation))
            .await
    }
}

pub(crate) fn verify_database(db: &Connection, content: &Content) -> Result<()> {
    let version: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        matches!(version, 21..=23),
        "unsupported local snapshot schema {version}"
    );
    if version >= 22 {
        migrate(db, version)?;
    } else {
        let unexpected:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN ('save_namespace','account_origins','portable_assists','portable_consumed_levels','portable_history'))",[],|r|r.get(0))?;
        ensure!(
            !unexpected,
            "schema-21 snapshot contains schema-22 portable metadata; restore the matching schema instead of ignoring it"
        );
    }
    check_required_state(db, version)?;
    death_awards::check_schema(db, version)?;
    gold::check_schema(db, version)?;
    discord_gold::check_schema(db, version)?;
    guilds::migrate(db, version)?;
    email::migrate(db, version)?;
    pictures::migrate(db, version)?;
    character_slots::migrate(db, version)?;
    vaults::migrate(db, version)?;
    let bad_collection: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM collections WHERE typeof(found_at)<>'integer' OR found_at<=0)",
        [],
        |r| r.get(0),
    )?;
    ensure!(!bad_collection, "invalid collection timestamp");
    let base = content.classes.get(&WIZARD).context("Wizard missing")?.base;
    death_awards::validate_state(db, base)?;
    let mut q = db.prepare("SELECT stats,items,revision,alive FROM characters")?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i32>(2)?,
            r.get::<_, bool>(3)?,
        ))
    })? {
        let (raw, items, revision, alive) = row?;
        let ch = strict_character(&raw, base)?;
        let class = content
            .classes
            .get(&ch.class)
            .context("unknown character class")?;
        let items: Vec<i32> = serde_json::from_str(&items)?;
        ensure!(
            items.len() == 12
                && items.iter().all(|i| (-1..=65535).contains(i))
                && revision >= 0
                && (!alive || ch.alive)
                && ch
                    .base
                    .iter()
                    .zip(class.caps)
                    .all(|(v, cap)| *v >= 0 && *v <= cap),
            "invalid stored character inventory/class"
        );
    }
    if version >= 22 {
        let broken:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM account_origins o LEFT JOIN portable_history h ON h.account=o.account WHERE h.account IS NULL) OR EXISTS(SELECT 1 FROM portable_consumed_levels l JOIN characters c ON c.id=l.recipient WHERE l.reached_level>json_extract(c.stats,'$.level'))",[],|r|r.get(0))?;
        ensure!(!broken, "inconsistent portable account metadata");
        for origin in rows(
            db,
            "SELECT namespace,source_account FROM account_origins WHERE account<>?1",
            -1,
            |r| {
                Ok(Origin {
                    namespace: r.get(0)?,
                    account: r.get(1)?,
                })
            },
        )? {
            ensure!(
                key(&origin.namespace) && origin.account > 0,
                "invalid persisted account origin"
            );
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;
