use crate::content::{Class, Content, Object, WIZARD};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::mpsc};
use tokio::sync::oneshot;

mod account_actions;
mod accounts;
pub(crate) mod browser_link;
mod character_slots;
mod discord_gold;
mod vaults;
pub use vaults::{VAULT_CHESTS, VAULT_PRICE};
mod consumables;

pub(crate) use character_slots::SlotReplay;
pub(crate) mod death_awards;
pub(crate) mod gold;
mod guilds;
mod hiscores;
mod legends;
pub use guilds::{Guild, GuildAction, Membership as GuildMembership};
pub(crate) mod pictures;

mod purchases;
mod resurrection;
mod social;
pub use purchases::Purchase;
mod updates;
pub use social::AccountLists;
pub use updates::{UpdateFeed, UpdatePost, UpdateSnapshot};

#[derive(Debug)]
pub(crate) enum NameChoice {
    Rejected(&'static str),
    Committed { balance: i32 },
}

#[derive(Debug)]
pub(crate) struct Rejected(pub &'static str);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Rejected {}

#[derive(Debug)]
pub struct InvalidCredentials;
impl std::fmt::Display for InvalidCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid email or password")
    }
}
impl std::error::Error for InvalidCredentials {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountPresentation {
    pub guild: Option<GuildMembership>,
    pub credits: i32,
    pub fame: i32,
    pub best_fame: BTreeMap<u16, i32>,
}
impl AccountPresentation {
    const THRESHOLDS: [i32; 5] = [20, 150, 400, 800, 2000];

    pub fn stars(&self) -> i32 {
        self.best_fame
            .values()
            .map(|best| {
                Self::THRESHOLDS
                    .iter()
                    .filter(|threshold| best >= *threshold)
                    .count() as i32
            })
            .sum()
    }

    pub fn next_quest(&self, class: u16, current_fame: i32) -> i32 {
        let best = self
            .best_fame
            .get(&class)
            .copied()
            .unwrap_or(0)
            .max(current_fame);
        Self::THRESHOLDS
            .into_iter()
            .find(|threshold| *threshold > best)
            .unwrap_or(-1)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct XpBoost {
    pub active: bool,
    pub bonus_exp: i32,
}

fn deserialize_fame_version<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<u8, D::Error> {
    let version = u8::deserialize(decoder)?;
    if version != 1 {
        return Err(serde::de::Error::custom(format!(
            "unsupported fame version {version}"
        )));
    }
    Ok(version)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifetimeCompleteness {
    Complete,
    KnownOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FameLifetime {
    #[serde(deserialize_with = "deserialize_fame_version")]
    pub version: u8,
    pub completeness: LifetimeCompleteness,
    pub shots: u64,
    pub projectile_hits: u64,
    pub kills: u64,
    pub god_kills: u64,
    pub cube_kills: u64,
    pub oryx_kills: u64,
    pub inventory_potion_used: bool,
    pub ability_used: bool,
    pub teleported: bool,
    pub tiles: u64,
    pub level_assists: u64,
    pub qualified_quests: u64,
    pub realm_exits: std::collections::BTreeSet<crate::dungeon::HistoricalDungeon>,
}
impl FameLifetime {
    pub fn complete() -> Self {
        Self {
            version: 1,
            completeness: LifetimeCompleteness::Complete,
            shots: 0,
            projectile_hits: 0,
            kills: 0,
            god_kills: 0,
            cube_kills: 0,
            oryx_kills: 0,
            inventory_potion_used: false,
            ability_used: false,
            teleported: false,
            tiles: 0,
            level_assists: 0,
            qualified_quests: 0,
            realm_exits: std::collections::BTreeSet::new(),
        }
    }
    pub fn known_only() -> Self {
        Self {
            completeness: LifetimeCompleteness::KnownOnly,
            ..Self::complete()
        }
    }
    pub fn merge(&mut self, other: &Self) -> Result<()> {
        self.validate()?;
        other.validate()?;
        ensure!(
            self.completeness == other.completeness,
            "lifetime completeness changed during merge"
        );
        self.shots = self.shots.max(other.shots);
        self.projectile_hits = self.projectile_hits.max(other.projectile_hits);
        self.kills = self.kills.max(other.kills);
        self.god_kills = self.god_kills.max(other.god_kills);
        self.cube_kills = self.cube_kills.max(other.cube_kills);
        self.oryx_kills = self.oryx_kills.max(other.oryx_kills);
        self.tiles = self.tiles.max(other.tiles);
        self.level_assists = self.level_assists.max(other.level_assists);
        self.qualified_quests = self.qualified_quests.max(other.qualified_quests);
        self.inventory_potion_used |= other.inventory_potion_used;
        self.ability_used |= other.ability_used;
        self.teleported |= other.teleported;
        self.realm_exits.extend(&other.realm_exits);
        Ok(())
    }
    pub fn follows(&self, previous: &Self) -> Result<()> {
        self.validate()?;
        ensure!(
            self.completeness == previous.completeness
                && self.shots >= previous.shots
                && self.projectile_hits >= previous.projectile_hits
                && self.kills >= previous.kills
                && self.god_kills >= previous.god_kills
                && self.cube_kills >= previous.cube_kills
                && self.oryx_kills >= previous.oryx_kills
                && (!previous.inventory_potion_used || self.inventory_potion_used)
                && (!previous.ability_used || self.ability_used)
                && (!previous.teleported || self.teleported)
                && self.tiles >= previous.tiles
                && self.level_assists >= previous.level_assists
                && self.qualified_quests >= previous.qualified_quests
                && self.realm_exits.is_superset(&previous.realm_exits),
            "character snapshot lost committed lifetime history"
        );
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported fame lifetime version {}",
            self.version
        );
        ensure!(
            self.projectile_hits <= self.shots,
            "fame projectile hits exceed admitted shots"
        );
        ensure!(
            self.god_kills <= self.kills
                && self.cube_kills <= self.kills
                && self.oryx_kills <= self.kills,
            "fame classified kills exceed total kills"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelAssist {
    pub reached_level: u8,
    pub giver: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self")]
pub struct Character {
    pub schema: u8,
    pub lifetime: FameLifetime,
    pub class: u16,
    pub base: [i32; 8],
    #[serde(default)]
    pub tex1: u32,
    #[serde(default)]
    pub tex2: u32,
    #[serde(default)]
    pub pet_kind: Option<u16>,
    pub id: i32,
    pub account: i32,
    pub name: String,
    pub level: i32,
    pub exp: i32,
    // Pre-booster schema-one writers had no boost metadata; present metadata must be complete.
    #[serde(default)]
    pub xp_boost: XpBoost,
    pub fame: i32,
    pub hp: i32,
    pub mp: i32,
    pub items: Vec<i32>,
    pub revision: i32,
    pub alive: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub shots: u64,
    #[serde(default)]
    pub hits: u64,
    #[serde(default)]
    pub kills: u64,
    #[serde(default)]
    pub quests: u64,
    #[serde(default)]
    pub tiles: u64,
    #[serde(default)]
    pub pirate_caves: u64,
    // Earlier schema-one writers could not enter these dungeons.
    #[serde(default)]
    pub dungeons: [u64; 3],
    #[serde(default)]
    pub abysses: u64,
    #[serde(default)]
    pub trenches: u64,
    #[serde(default)]
    pub spider_dens: u64,
    #[serde(default)]
    pub mad_labs: u64,
    #[serde(default)]
    pub jungles: u64,
    #[serde(default)]
    pub manors: u64,
    #[serde(default)]
    pub beaches: u64,
    #[serde(default)]
    pub davy_lockers: u64,
    #[serde(default)]
    pub tombs: u64,
}
// Keep one field declaration while validating every direct Character decode, not just Store readers.
impl<'de> Deserialize<'de> for Character {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        let ch = Self::deserialize(decoder)?;
        if ch.schema != 1 {
            return Err(serde::de::Error::custom(format!(
                "unsupported character schema {}",
                ch.schema
            )));
        }
        ch.lifetime.validate().map_err(serde::de::Error::custom)?;
        Ok(ch)
    }
}
impl Serialize for Character {
    fn serialize<S: serde::Serializer>(&self, encoder: S) -> std::result::Result<S::Ok, S::Error> {
        self.lifetime
            .validate()
            .map_err(serde::ser::Error::custom)?;
        Self::serialize(self, encoder)
    }
}

pub fn level_start(level: i32) -> Result<i32> {
    ensure!((1..=20).contains(&level), "invalid character level {level}");
    Ok(50 * (level - 1).pow(2))
}
pub fn experience_goal(level: i32) -> Result<i32> {
    level_start(level)?;
    Ok(50 + (level - 1) * 100)
}
impl Character {
    /// A level-one character of `class` with its starting equipment, not yet bound to an account.
    pub fn fresh(class: &Object) -> Result<Self> {
        Ok(Character {
            schema: 1,
            lifetime: FameLifetime::complete(),
            class: class.kind,
            base: [
                class.hp,
                class.mp,
                class.attack,
                class.defense,
                class.speed,
                class.dexterity,
                class.vitality,
                class.wisdom,
            ],
            tex1: 0,
            tex2: 0,
            pet_kind: None,
            id: 0,
            account: 0,
            name: String::new(),
            level: 1,
            exp: 0,
            xp_boost: XpBoost::default(),
            fame: 0,
            hp: class.hp,
            mp: class.mp,
            items: class.equipment.clone(),
            revision: 0,
            alive: true,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            shots: 0,
            hits: 0,
            kills: 0,
            quests: 0,
            tiles: 0,
            pirate_caves: 0,
            dungeons: [0; 3],
            jungles: 0,
            abysses: 0,
            trenches: 0,
            spider_dens: 0,
            mad_labs: 0,
            manors: 0,
            beaches: 0,
            davy_lockers: 0,
            tombs: 0,
        })
    }
    pub fn gain_experience(
        &mut self,
        content: &Content,
        amount: i32,
        rng: &mut impl rand::Rng,
    ) -> Result<bool> {
        self.effective(content)?;
        ensure!(amount >= 0, "negative experience reward");
        let mut next = self.clone();
        let bonus = if next.xp_boost.active { amount } else { 0 };
        next.exp = next
            .exp
            .checked_add(amount)
            .and_then(|exp| exp.checked_add(bonus))
            .context("experience overflow")?;
        next.xp_boost.bonus_exp = next
            .xp_boost
            .bonus_exp
            .checked_add(bonus)
            .context("bonus experience overflow")?;
        // Expired boost XP must never turn into Fame on a later award.
        next.fame = next
            .exp
            .checked_sub(next.xp_boost.bonus_exp)
            .context("invalid bonus experience")?
            / 1000;
        let class = &content.classes[&next.class];
        let previous = next.level;
        while next.level < 20 && next.exp >= level_start(next.level + 1)? {
            next.level += 1;
            for (i, (min, max)) in class.growth.iter().enumerate() {
                next.base[i] = next.base[i]
                    .checked_add(rng.gen_range(*min..=*max))
                    .context("growth overflow")?
                    .min(class.caps[i]);
            }
        }
        if next.level == 20 {
            next.xp_boost.active = false;
        }
        if next.level > previous {
            let effective = next.effective(content)?;
            next.hp = effective[0];
            next.mp = effective[1];
        }
        *self = next;
        Ok(self.level > previous)
    }
    pub fn effective(&self, content: &Content) -> Result<[i32; 8]> {
        let class = content
            .classes
            .get(&self.class)
            .context("unknown character class")?;
        level_start(self.level)?;
        self.lifetime.validate()?;
        ensure!(
            (0..=self.exp).contains(&self.xp_boost.bonus_exp)
                && (!self.xp_boost.active || self.level < 20),
            "invalid XP boost state: level={}, experience={}, bonus={}, active={}",
            self.level,
            self.exp,
            self.xp_boost.bonus_exp,
            self.xp_boost.active
        );
        ensure!(
            self.schema == 1 && self.items.len() == 12,
            "invalid character schema or inventory"
        );
        ensure!(
            self.base
                .iter()
                .zip(class.caps)
                .all(|(n, cap)| *n >= 0 && *n <= cap),
            "invalid base stats"
        );
        let slots = &content.get(self.class)?.slots;
        let mut stats = self.base;
        for (slot, item) in self.items[..4].iter().enumerate() {
            if *item == -1 {
                continue;
            }
            let item = content.get(u16::try_from(*item)?)?;
            ensure!(
                item.slot_type == slots[slot] && !item.unsupported_equipment,
                "unsupported or incompatible equipped item {} in slot {slot}",
                item.name
            );
            for (id, amount) in &item.bonuses {
                let index = crate::content::STATS
                    .iter()
                    .position(|(stat, _)| stat == id)
                    .context("unsupported equipment stat")?;
                stats[index] = stats[index]
                    .checked_add(*amount)
                    .context("effective stat overflow")?;
            }
        }
        ensure!(stats.iter().all(|v| *v >= 0), "negative effective stat");
        Ok(stats)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub id: i32,
    pub name: String,
    pub next_id: i32,
    pub character_slots: i32,
    pub next_slot_price: i32,
    pub guild: Option<GuildMembership>,
    pub credits: i32,
    pub fame: i64,
    pub earned_fame: i64,
    pub best_levels: std::collections::BTreeMap<u16, i32>,
    pub best_fame: BTreeMap<u16, i32>,
    pub characters: Vec<Character>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Vault {
    pub account: i32,
    pub chest: i32,
    pub items: Vec<i32>,
    pub revision: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryWrite {
    pub id: i32,
    pub account: i32,
    pub revision: i32,
    pub items: Vec<i32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GiftParticipant {
    pub account: i32,
    pub character: i32,
    pub revision: i32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GiftAcceptance {
    pub transaction: String,
    pub session: [i32; 3],
    pub own: GiftParticipant,
    pub peer: GiftParticipant,
    pub peer_player: i32,
    pub peer_name: String,
    pub giving: bool,
    pub writes_hash: [u8; 32],
}
impl GiftAcceptance {
    pub(crate) fn payload_hash(giving: bool, writes: &str) -> [u8; 32] {
        use sha2::Digest;
        let mut hash = sha2::Sha256::new();
        hash.update([u8::from(giving)]);
        hash.update(writes.as_bytes());
        hash.finalize().into()
    }
}
type Job = Box<dyn FnOnce(&mut Connection) + Send>;
#[cfg(test)]
type LifecycleReplyGate =
    std::sync::Arc<std::sync::Mutex<Option<(oneshot::Sender<()>, mpsc::Receiver<()>)>>>;
enum Creation {
    Normal(Vec<(u16, i32)>),
    Prepared(Option<String>),
}
#[derive(Clone)]
pub struct Store {
    #[cfg(test)]
    lifecycle_lost_reply: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    lifecycle_jobs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    lifecycle_reply_gate: LifecycleReplyGate,
    #[cfg(test)]
    lifecycle_current_reply_gate: LifecycleReplyGate,
    tx: mpsc::Sender<Job>,
    legacy_base: [i32; 8],
    classes: std::sync::Arc<std::collections::BTreeMap<u16, Class>>,
    content: std::sync::Arc<Content>,
    hashes: std::sync::Arc<tokio::sync::Semaphore>,
    dummy_hash: String,
    dyes: std::sync::Arc<BTreeMap<u16, [Option<u32>; 2]>>,
}
pub(crate) fn decode_character(raw: &str, _legacy_base: [i32; 8]) -> Result<Character> {
    let ch: Character = serde_json::from_str(raw)?;
    ensure!(ch.schema == 1, "unsupported character schema {}", ch.schema);
    Ok(ch)
}
pub(crate) fn decode_legacy_character(raw: &str, legacy_base: [i32; 8]) -> Result<Character> {
    let mut value: serde_json::Value = serde_json::from_str(raw)?;
    match value.get("schema") {
        None => {
            // The version-zero writer only supported level-one Wizards.
            ensure!(
                value.get("class").is_none() && value.get("base").is_none(),
                "class metadata requires a schema version"
            );
            ensure!(
                value["level"] == 1 && value["exp"] == 0,
                "unrecognized legacy character progression"
            );
            value["schema"] = 1.into();
            value["class"] = WIZARD.into();
            value["base"] = serde_json::to_value(legacy_base)?;
        }
        Some(schema) => ensure!(schema == 1, "unsupported character schema {schema}"),
    }
    if value.get("lifetime").is_none() {
        let mut lifetime = FameLifetime::known_only();
        if let Some(tiles) = value.get("tiles") {
            lifetime.tiles = serde_json::from_value(tiles.clone())?;
        }
        value["lifetime"] = serde_json::to_value(lifetime)?;
    }
    decode_character(&value.to_string(), legacy_base)
}
impl Store {
    pub fn open(path: &Path, content: &Content) -> Result<Self> {
        let legacy_base = content
            .classes
            .get(&WIZARD)
            .context("Wizard class missing")?
            .base;
        let mut c = Connection::open(path)?;
        let version: i32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(
            matches!(
                version,
                0 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19
            ),
            "unsupported game database schema {version}"
        );
        let trades_exist: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='trade_transactions')", [], |r| r.get(0))?;
        let minds_exist: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='bot_minds')",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            !matches!(version, 13 | 14) || !(trades_exist || minds_exist),
            "experimental community schema {version} conflicts with canonical migrations; preserve this database and request an explicit migration"
        );
        death_awards::check_schema(&c, version)?;
        ensure!(
            version < 16 || trades_exist,
            "schema-16 trade receipts missing; restore committed settlements instead of allowing duplicate payment"
        );
        ensure!(
            version < 16 || minds_exist,
            "schema-16 account minds missing; restore reviewed state instead of erasing purposes"
        );
        let rewards_exist: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='gold_rewards')",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            version < 10 || rewards_exist,
            "gold reward history missing; restore reviewed state instead of allowing duplicate grants"
        );
        gold::check_schema(&c, version)?;
        discord_gold::check_schema(&c, version)?;
        let credits_exist: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('accounts') WHERE name='credits')",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            version < 8 || credits_exist,
            "schema-8 gold balance missing; restore reviewed state instead of recreating balances"
        );
        let wallet_exists: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='account_fame')",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            version < 5 || wallet_exists,
            "schema-5 fame wallet table missing; restore reviewed state instead of recreating balances"
        );
        let lists_exist: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='account_lists')",
            [], |r| r.get(0),
        )?;
        ensure!(
            version < 6 || lists_exist,
            "schema-6 account lists missing; restore reviewed state instead of clearing preferences"
        );
        let updates_exist: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type=\'table\' AND name=\'update_feed\')", [], |r| r.get(0),
        )?;
        ensure!(
            version < 7 || updates_exist,
            "schema-7 update feed missing; restore reviewed state instead of clearing published news"
        );
        if version >= 17 {
            guilds::migrate(&c, version)?;
            death_awards::validate_state(&c, legacy_base)?;
        }
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS accounts(id INTEGER PRIMARY KEY,guid TEXT NOT NULL UNIQUE,name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS shore_bots(name TEXT PRIMARY KEY,account INTEGER NOT NULL UNIQUE REFERENCES accounts(id));
CREATE TABLE IF NOT EXISTS characters(id INTEGER PRIMARY KEY,account INTEGER NOT NULL REFERENCES accounts(id),stats TEXT NOT NULL,items TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 0,alive INTEGER NOT NULL DEFAULT 1);
CREATE TABLE IF NOT EXISTS deaths(character INTEGER PRIMARY KEY REFERENCES characters(id),snapshot TEXT NOT NULL,killed_by TEXT NOT NULL,died_at INTEGER NOT NULL DEFAULT(unixepoch()));
CREATE TABLE IF NOT EXISTS vaults(account INTEGER PRIMARY KEY REFERENCES accounts(id),items TEXT NOT NULL CHECK(json_array_length(items)=8),revision INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS credentials(account INTEGER PRIMARY KEY REFERENCES accounts(id),email TEXT NOT NULL UNIQUE,password_hash TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS competitors(account INTEGER PRIMARY KEY REFERENCES accounts(id));
CREATE TABLE IF NOT EXISTS collections(account INTEGER NOT NULL REFERENCES accounts(id),item INTEGER NOT NULL CHECK(item BETWEEN 0 AND 65535),found_at INTEGER NOT NULL,PRIMARY KEY(account,item));
CREATE TABLE IF NOT EXISTS discord_links(account INTEGER PRIMARY KEY REFERENCES accounts(id),discord_id TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS discord_codes(account INTEGER PRIMARY KEY REFERENCES accounts(id),hash TEXT NOT NULL UNIQUE,issued_at INTEGER NOT NULL,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS account_fame(account INTEGER PRIMARY KEY REFERENCES accounts(id),balance INTEGER NOT NULL CHECK(balance BETWEEN 0 AND 2147483647),earned INTEGER NOT NULL CHECK(earned BETWEEN balance AND 2147483647));
CREATE TABLE IF NOT EXISTS discord_recovery(account INTEGER PRIMARY KEY REFERENCES discord_links(account),enabled_at INTEGER NOT NULL,last_issued_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS recovery_codes(account INTEGER PRIMARY KEY REFERENCES accounts(id),hash TEXT NOT NULL UNIQUE,issued_at INTEGER NOT NULL,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS password_resets(account INTEGER PRIMARY KEY REFERENCES accounts(id),hash TEXT NOT NULL UNIQUE,issued_at INTEGER NOT NULL,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS account_lists(account INTEGER NOT NULL REFERENCES accounts(id),list_id INTEGER NOT NULL CHECK(list_id IN (0,1)),target INTEGER NOT NULL REFERENCES accounts(id),PRIMARY KEY(account,list_id,target),CHECK(account<>target));
CREATE TABLE IF NOT EXISTS update_feed(id INTEGER PRIMARY KEY CHECK(id=1),snapshot TEXT NOT NULL CHECK(json_valid(snapshot)),synced_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS deaths_by_time ON deaths(died_at);")?;
        let journal: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        let synchronous: i32 = c.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        ensure!(
            journal == "wal" || path == Path::new(":memory:"),
            "WAL unavailable: {journal}"
        );
        ensure!(
            synchronous == 2,
            "FULL durability unavailable: synchronous={synchronous}"
        );
        tracing::info!(%journal, synchronous, "account store durability configured");
        let migration = c.transaction()?;
        migration.execute_batch("CREATE TABLE IF NOT EXISTS trade_transactions(id TEXT PRIMARY KEY,owners TEXT NOT NULL,writes TEXT NOT NULL,revisions TEXT NOT NULL,delivery TEXT,market_key TEXT UNIQUE);
 CREATE TABLE IF NOT EXISTS bot_minds(account INTEGER PRIMARY KEY REFERENCES accounts(id),revision INTEGER NOT NULL CHECK(revision>0),mind TEXT NOT NULL CHECK(json_valid(mind)));")?;
        guilds::migrate(&migration, version)?;
        gold::migrate(&migration, version, rewards_exist)?;
        if version < 19 {
            migration.execute_batch(&discord_gold::SCHEMA.replacen(
                "CREATE TABLE",
                "CREATE TABLE IF NOT EXISTS",
                1,
            ))?;
        }
        discord_gold::check_schema(&migration, 19)?;
        if !credits_exist {
            migration.execute_batch("ALTER TABLE accounts ADD COLUMN credits INTEGER NOT NULL DEFAULT 100 CHECK(credits BETWEEN 0 AND 2147483647)")?;
        }
        character_slots::migrate(&migration, version)?;
        vaults::migrate(&migration, version)?;
        death_awards::migrate(&migration, version, legacy_base)?;
        pictures::migrate(&migration, version)?;
        let records = {
            let mut q = migration.prepare("SELECT id,stats FROM characters ORDER BY id")?;
            q.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut migrated = 0;
        for (id, raw) in records {
            let ch = if version < 17 {
                decode_legacy_character(&raw, legacy_base)
            } else {
                decode_character(&raw, legacy_base)
            }
            .with_context(|| format!("character {id} migration"))?;
            let definition = content
                .classes
                .get(&ch.class)
                .context("persisted character has unknown class")?;
            ensure!(
                (1..=20).contains(&ch.level)
                    && ch
                        .base
                        .iter()
                        .zip(definition.caps)
                        .all(|(v, cap)| *v >= 0 && *v <= cap),
                "invalid persisted class stats for character {id}"
            );
            let legacy_value: serde_json::Value = serde_json::from_str(&raw)?;
            if version < 17
                && (legacy_value.get("schema").is_none() || legacy_value.get("lifetime").is_none())
            {
                let mut value = legacy_value;
                if value.get("schema").is_none() {
                    value["schema"] = 1.into();
                    value["class"] = ch.class.into();
                    value["base"] = serde_json::to_value(ch.base)?;
                }
                if value.get("lifetime").is_none() {
                    value["lifetime"] = serde_json::to_value(&ch.lifetime)?;
                }
                migration.execute(
                    "UPDATE characters SET stats=?2 WHERE id=?1",
                    params![id, serde_json::to_string(&value)?],
                )?;
                migrated += 1;
            }
        }
        if !wallet_exists {
            let mut earned = std::collections::BTreeMap::<i32, i64>::new();
            {
                let mut q = migration.prepare(
                "SELECT c.account,d.snapshot FROM deaths d JOIN characters c ON c.id=d.character",
            )?;
                for row in q.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))? {
                    let (account, raw) = row?;
                    let ch = decode_legacy_character(&raw, legacy_base)?;
                    ensure!(
                        ch.account == account && !ch.alive && ch.fame >= 0,
                        "invalid death credit"
                    );
                    let amount = earned.entry(account).or_default();
                    *amount = amount
                        .checked_add(i64::from(ch.fame))
                        .context("account fame overflow")?;
                }
            }
            let accounts = {
                let mut q = migration.prepare("SELECT id FROM accounts")?;
                q.query_map([], |r| r.get::<_, i32>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for account in accounts {
                let amount = earned.get(&account).copied().unwrap_or(0);
                migration.execute(
                    "INSERT INTO account_fame(account,balance,earned) VALUES(?1,?2,?2)",
                    params![account, amount],
                )?;
            }
        } else {
            let missing: bool = migration.query_row("SELECT EXISTS(SELECT 1 FROM accounts a LEFT JOIN account_fame f ON f.account=a.id WHERE f.account IS NULL)", [], |r| r.get(0))?;
            ensure!(
                !missing,
                "account fame wallet missing; restore reviewed state instead of recreating balances"
            );
        }
        death_awards::validate_wallets(&migration)?;
        death_awards::validate_state(&migration, legacy_base)?;
        migration.pragma_update(None, "user_version", 19)?;
        migration.commit()?;
        if migrated > 0 {
            tracing::info!(
                migrated,
                from_schema = 0,
                to_schema = 1,
                "character metadata migrated; death snapshots unchanged"
            );
        }
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("account-store".into())
            .spawn(move || {
                let mut c = c;
                for job in rx {
                    job(&mut c);
                }
            })?;
        Ok(Self {
            #[cfg(test)]
            lifecycle_lost_reply: Default::default(),
            #[cfg(test)]
            lifecycle_jobs: Default::default(),
            #[cfg(test)]
            lifecycle_reply_gate: Default::default(),
            #[cfg(test)]
            lifecycle_current_reply_gate: Default::default(),
            tx,
            legacy_base,
            classes: std::sync::Arc::new(content.classes.clone()),
            content: std::sync::Arc::new(content.clone()),
            hashes: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
            dummy_hash: accounts::hash("unknown-account")?,
            dyes: std::sync::Arc::new(
                content
                    .objects
                    .values()
                    .filter(|item| item.class == "Dye" && item.activate == "Dye")
                    .map(|item| (item.kind, item.textures))
                    .collect(),
            ),
        })
    }
    #[cfg(test)]
    pub(crate) fn lifecycle_test_pause_reply(&self) -> (oneshot::Receiver<()>, mpsc::Sender<()>) {
        let (entered, ready) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        assert!(
            self.lifecycle_reply_gate
                .lock()
                .unwrap()
                .replace((entered, wait))
                .is_none()
        );
        (ready, release)
    }

    #[cfg(test)]
    pub(crate) fn lifecycle_test_jobs(&self) -> usize {
        self.lifecycle_jobs
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(test)]
    pub(crate) async fn lifecycle_test_job<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.call(f).await
    }
    async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        #[cfg(test)]
        let lost = self
            .lifecycle_lost_reply
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        #[cfg(test)]
        let jobs = self.lifecycle_jobs.clone();
        #[cfg(test)]
        let gate = self.lifecycle_reply_gate.lock().unwrap().take();
        #[cfg(test)]
        let current_reply = self.lifecycle_current_reply_gate.clone();
        #[cfg(test)]
        jobs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.tx
            .send(Box::new(move |c| {
                let result = f(c);
                #[cfg(test)]
                let current = current_reply.lock().unwrap().take();
                #[cfg(test)]
                assert!(
                    gate.is_none() || current.is_none(),
                    "dispatch and SQL-write gates target the same reply"
                );
                #[cfg(test)]
                for (entered, wait) in gate.into_iter().chain(current) {
                    let _ = entered.send(());
                    if wait.recv().is_err() {
                        let _ = tx.send(Err(anyhow::anyhow!("lifecycle Store gate abandoned")));
                        return;
                    }
                }
                #[cfg(test)]
                jobs.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                #[cfg(test)]
                if lost {
                    return;
                }
                let _ = tx.send(result);
            }))
            .map_err(|_| anyhow::anyhow!("account store stopped"))?;
        rx.await.context("account worker failed")?
    }
    pub(crate) async fn account_guid(&self, account: i32) -> Result<String> {
        self.call(move |db| {
            Ok(
                db.query_row("SELECT guid FROM accounts WHERE id=?1", [account], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
    }
    pub async fn list(&self, guid: String) -> Result<Account> {
        let id = self.guest(guid, true).await?;
        self.list_id(id).await
    }
    pub async fn characters(&self, guid: String, password: String) -> Result<Account> {
        let id = if password.is_empty() {
            self.guest(guid, true).await?
        } else {
            self.authenticate(guid, password).await?
        };
        self.list_id(id).await
    }
    pub async fn presentation(&self, account: i32) -> Result<AccountPresentation> {
        let legacy_base = self.legacy_base;
        let classes = self.classes.clone();
        self.call(move |db| {
            let tx = db.transaction()?;
            let (presentation, _) = rewarded_presentation(&tx, account, legacy_base, &classes)?;
            tx.commit()?;
            Ok(presentation)
        })
        .await
    }
    async fn list_id(&self, id: i32) -> Result<Account> {
        let legacy_base = self.legacy_base;
        let classes = self.classes.clone();
        let account = self.call(move|c|{let tx=c.transaction()?;
            let name:String=tx.query_row("SELECT name FROM accounts WHERE id=?1",[id],|r|r.get(0))?;
            let character_slots=character_slots::read(&tx,id)?;
            let next_slot_price=character_slots::price(character_slots)?;
            let mut characters=Vec::new();{let mut q=tx.prepare("SELECT id,stats,items,revision,alive FROM characters WHERE account=?1 AND alive=1 ORDER BY id")?;let rows=q.query_map([id],|r|Ok((r.get::<_,i32>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i32>(3)?,r.get::<_,bool>(4)?)))?;
            for row in rows{let(cid,stats,items,rev,alive)=row?;let mut ch:Character=serde_json::from_str(&stats)?;ch.id=cid;ch.account=id;ch.name=name.clone();ch.items=serde_json::from_str(&items)?;ch.revision=rev;ch.alive=alive;characters.push(ch);}}
            let next_id:i32=tx.query_row("SELECT coalesce(max(id),0)+1 FROM characters WHERE account=?1",[id],|r|r.get(0))?;let best_levels = { let mut q = tx.prepare("SELECT json_extract(stats,'$.class'),max(json_extract(stats,'$.level')) FROM characters WHERE account=?1 GROUP BY json_extract(stats,'$.class')")?; q.query_map([id], |r| Ok((r.get::<_,u16>(0)?, r.get::<_,i32>(1)?)))?.collect::<rusqlite::Result<std::collections::BTreeMap<_,_>>>()? };let (presentation,earned_fame) = rewarded_presentation(&tx, id, legacy_base, &classes)?;tx.commit()?;Ok(Account{id,name,next_id,character_slots,next_slot_price,guild:presentation.guild,credits:presentation.credits,fame:i64::from(presentation.fame),earned_fame,best_levels,best_fame:presentation.best_fame,characters})}).await?;
        self.vaults(account.id).await?;
        Ok(account)
    }
    pub(crate) async fn bot_mind(
        &self,
        account: i32,
    ) -> Result<Option<crate::bots::pursuits::Mind>> {
        self.call(move |db| {
            let raw: Option<(i64, String)> = db
                .query_row(
                    "SELECT revision,mind FROM bot_minds WHERE account=?1",
                    [account],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            raw.map(|(revision, raw)| {
                let mind: crate::bots::pursuits::Mind =
                    serde_json::from_str(&raw).context("decode durable account mind")?;
                mind.validate()?;
                ensure!(
                    mind.account == account && mind.revision == revision,
                    "durable account mind binding/revision changed"
                );
                Ok(mind)
            })
            .transpose()
        })
        .await
    }
    pub(crate) async fn save_bot_mind(&self, mut mind: crate::bots::pursuits::Mind) -> Result<i64> {
        mind.validate()?;
        self.call(move |db| {
            let expected = mind.revision;
            mind.revision = expected
                .checked_add(1)
                .context("account mind revision exhausted")?;
            let raw = serde_json::to_string(&mind)?;
            let changed = if expected == 0 {
                db.execute(
                    "INSERT INTO bot_minds(account,revision,mind) VALUES(?1,?2,?3)",
                    params![mind.account, mind.revision, raw],
                )?
            } else {
                db.execute(
                    "UPDATE bot_minds SET revision=?1,mind=?2 WHERE account=?3 AND revision=?4",
                    params![mind.revision, raw, mind.account, expected],
                )?
            };
            ensure!(changed == 1, "stale account mind write rejected");
            Ok(mind.revision)
        })
        .await
    }
    pub async fn shore_account(&self, identity: String) -> Result<Option<i32>> {
        self.call(move |db| {
            Ok(db
                .query_row(
                    "SELECT account FROM shore_bots WHERE name=?1",
                    [identity],
                    |r| r.get(0),
                )
                .optional()?)
        })
        .await
    }
    /// Provisions a persona account on first use, rebuilds its roster once every character has died,
    /// and returns the `pick`th living character.
    pub async fn persona_character(
        &self,
        identity: String,
        roster: Vec<Character>,
        vault: Vec<[i32; 8]>,
        pick: usize,
        starting_stock: Option<i32>,
    ) -> Result<Character> {
        ensure!(
            starting_stock.is_none_or(|kind| (0..=65535).contains(&kind)),
            "invalid persona stock kind"
        );
        ensure!(
            (1..=14).contains(&roster.iter().filter(|ch| ch.alive).count())
                && (1..=20).contains(&vault.len()),
            "invalid persona provisioning"
        );
        let name = identity
            .strip_prefix("persona:v1:")
            .context("unknown persona snapshot version")?
            .to_owned();
        let guid = self.call(move |db| {
            let tx = db.transaction()?;
            let existing: Option<(String, i32)> = tx.query_row(
                "SELECT guid,accounts.id FROM accounts JOIN shore_bots ON accounts.id=shore_bots.account WHERE shore_bots.name=?1",
                [&identity], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            let taken = accounts::human_name_taken(&tx, &name, 0)?;
            ensure!(!taken, anyhow::anyhow!(Rejected("persona name is already in use")).context(format!("persona {identity} already belongs to another account")));
            let (guid, account) = match existing {
                Some(found) => found,
                None => {
                    let guid = format!("persona-{:032x}", rand::random::<u128>());
                    tx.execute("INSERT INTO accounts(guid,name,character_slots,vault_mask) VALUES(?1,?2,14,?3)", params![guid, name, (1 << vault.len()) - 1])?;
                    let account = i32::try_from(tx.last_insert_rowid())?;
                    for (chest, items) in vault.iter().enumerate() {
                        tx.execute("INSERT INTO vaults(account,chest,items,revision) VALUES(?1,?2,?3,0)", params![account, chest, serde_json::to_string(items)?])?;
                    }
                    tx.execute("INSERT INTO account_fame(account,balance,earned) VALUES(?1,0,0)", [account])?;
                    tx.execute("INSERT INTO shore_bots(name,account) VALUES(?1,?2)", params![identity, account])?;
                    tracing::info!(account, identity, chests = vault.len(), ?vault, "persona account provisioned");
                    (guid, account)
                }
            };
            let living: i64 = tx.query_row("SELECT count(*) FROM characters WHERE account=?1 AND alive=1", [account], |r| r.get(0))?;
            if living == 0 {
                let mut roster = roster;
                if let Some(kind) = starting_stock {
                    let merchant = roster.iter_mut().find(|ch| ch.alive).context("persona stock has no living owner")?;
                    // Rebuilds must withdraw durable holdings too, never replay the initial grant.
                    for mut chest in vaults::read(&tx, account)? {
                        let mut moved = 0;
                        for item in chest.items.iter_mut().filter(|item| **item == kind) {
                            if let Some(slot) = merchant.items.iter_mut().skip(7).find(|slot| **slot == -1) {
                                *slot = *item;
                                *item = -1;
                                moved += 1;
                            }
                        }
                        if moved > 0 { vaults::save(&tx, &chest)?; }
                        tracing::info!(account, identity, kind, moved, chest=chest.chest, "persona held stock withdrawn from durable vault");
                    }
                }
                for mut ch in roster {
                    if !ch.alive {
                        let enough: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM characters WHERE account=?1 AND json_extract(stats,'$.class')=?2 AND json_extract(stats,'$.level')>=?3)", params![account, ch.class, ch.level], |r| r.get(0))?;
                        if enough { continue; }
                    }
                    ch.account = account;
                    ch.name.clone_from(&name);
                    tx.execute("INSERT INTO characters(account,stats,items,alive) VALUES(?1,?2,?3,?4)",
                        params![account, serde_json::to_string(&ch)?, serde_json::to_string(&ch.items)?, ch.alive])?;
                    tracing::info!(account, identity, class = ch.class, level = ch.level, fame = ch.fame, alive=ch.alive, items = ?ch.items, base = ?ch.base, "persona snapshot character provisioned (synthetic prehistory, not earned gameplay)");
                }
            }
            character_slots::read(&tx, account)?;
            vaults::read(&tx, account)?;
            tx.commit()?;
            Ok(guid)
        }).await?;
        let mut account = self.list(guid).await?;
        ensure!(
            !account.characters.is_empty(),
            "persona has no living character after provisioning"
        );
        let index = pick % account.characters.len();
        Ok(account.characters.swap_remove(index))
    }
    pub(crate) async fn persona_name_available(&self, account: i32, name: String) -> Result<()> {
        self.call(move |db| {
            let taken = accounts::human_name_taken(db, &name, account)?;
            ensure!(!taken, Rejected("persona name is already in use"));
            Ok(())
        })
        .await
    }
    pub(crate) async fn retire_persona(&self, identity: String, account: i32) -> Result<()> {
        ensure!(
            identity.starts_with("persona:v1:") && !identity.starts_with("persona:v1:retired:"),
            "not an active preset identity"
        );
        self.call(move |db| {
            // Keep all characters, resources and receipts for rollback; only the active registry binding changes.
            let changed = db.execute(
                "UPDATE shore_bots SET name=?3 WHERE name=?1 AND account=?2",
                params![
                    identity,
                    account,
                    format!("persona:v1:retired:{account}:{identity}")
                ],
            )?;
            ensure!(changed == 1, "persona retirement binding changed");
            tracing::info!(
                account,
                identity,
                "preset session archived without deleting progress"
            );
            Ok(())
        })
        .await
    }
    pub async fn create(&self, account: i32, class: &Object) -> Result<Character> {
        ensure!(
            self.classes.contains_key(&class.kind) && class.class == "Player",
            "unknown playable class"
        );
        let requirements = self
            .classes
            .get(&class.kind)
            .context("class metadata missing")?
            .requires
            .clone();
        self.create_character(account, class, Creation::Normal(requirements))
            .await
    }
    pub(crate) async fn create_prepared(
        &self,
        account: i32,
        class: &Object,
        name: Option<String>,
    ) -> Result<Character> {
        tracing::info!(
            account,
            class = class.kind,
            "explicit prepared creation requested; no prerequisite progress granted"
        );
        self.create_character(account, class, Creation::Prepared(name))
            .await
    }
    async fn create_character(
        &self,
        account: i32,
        class: &Object,
        creation: Creation,
    ) -> Result<Character> {
        ensure!(
            self.classes.contains_key(&class.kind) && class.class == "Player",
            "unknown playable class"
        );
        let kind = class.kind;
        let fresh = Character::fresh(class)?;
        self.call(move |c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let slots = character_slots::read(&tx, account)?;
            let normal = matches!(&creation, Creation::Normal(_));
            let (requirements, prepared_name) = match creation {
                Creation::Normal(requirements) => (requirements, None),
                Creation::Prepared(name) => {
                    let count: i64 = tx.query_row("SELECT count(*) FROM characters WHERE account=?1 AND alive=1", [account], |r| r.get(0))?;
                    ensure!(count == 0, "prepared creation cannot implicitly replace a living character");
                    (Vec::new(), name)
                }
            };
            for (required_class, required_level) in requirements {
                let best: Option<i32> = tx.query_row(
                    "SELECT max(json_extract(stats,\'$.level\')) FROM characters WHERE account=?1 AND json_extract(stats,\'$.class\')=?2",
                    params![account, required_class], |r| r.get(0),
                )?;
                ensure!(best.is_some_and(|level| level >= required_level), anyhow::anyhow!(Rejected("class is locked")).context(format!("class {kind:#x} is locked: requires {required_class:#x} level {required_level}")));
            }
            if normal {
                let living: i32 = tx.query_row("SELECT count(*) FROM characters WHERE account=?1 AND alive=1", [account], |r| r.get(0))?;
                ensure!(living < slots, Rejected("No free character slots."));
            }
            if let Some(name) = prepared_name {
                tx.execute("UPDATE accounts SET name=?1 WHERE id=?2", params![name,account])?;
            }
            let name: String =
                tx.query_row("SELECT name FROM accounts WHERE id=?1", [account], |r| {
                    r.get(0)
                })?;
            let mut ch = Character {
                account,
                name,
                ..fresh
            };
            tx.execute(
                "INSERT INTO characters(account,stats,items) VALUES(?1,?2,?3)",
                params![
                    account,
                    serde_json::to_string(&ch)?,
                    serde_json::to_string(&ch.items)?
                ],
            )?;
            ch.id = i32::try_from(tx.last_insert_rowid())?;
            tx.commit()?;
            Ok(ch)
        })
        .await
    }
    pub(crate) async fn character_alive(&self, id: i32, guid: String) -> Result<bool> {
        self.call(move |c| Ok(c.query_row("SELECT c.alive FROM characters c JOIN accounts a ON a.id=c.account WHERE c.id=?1 AND a.guid=?2",params![id,guid],|r|r.get(0))?)).await
    }
    pub async fn load(&self, account: i32, id: i32) -> Result<Character> {
        self.call(move|c|{let(stats,items,revision,name):(String,String,i32,String)=c.query_row("SELECT c.stats,c.items,c.revision,a.name FROM characters c JOIN accounts a ON a.id=c.account WHERE c.id=?1 AND c.account=?2 AND c.alive=1",params![id,account],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).context("character does not belong to this account or is dead")?;let mut ch:Character=serde_json::from_str(&stats)?;ch.id=id;ch.account=account;ch.name=name;ch.items=serde_json::from_str(&items)?;ch.revision=revision;ch.alive=true;Ok(ch)}).await
    }
    pub async fn delete_character(&self, account: i32, id: i32) -> Result<()> {
        self.call(move |db| {
            // Class unlocks depend on lifetime character records, not just surviving characters.
            let n = db.execute(
                "UPDATE characters SET alive=0,items=?3,revision=revision+1,stats=json_set(stats,'$.alive',json('false'),'$.hp',0,'$.items',json(?3),'$.revision',revision+1) WHERE id=?1 AND account=?2 AND alive=1",
                params![id, account, serde_json::to_string(&vec![-1; 12])?],
            )?;
            ensure!(n == 1, Rejected("character does not belong to this account or is no longer alive"));
            Ok(())
        })
        .await?;
        tracing::info!(
            account,
            character = id,
            "character deleted without death rewards"
        );
        Ok(())
    }
    pub async fn exchange(
        &self,
        writes: Vec<InventoryWrite>,
        vaults: Vec<Vault>,
    ) -> Result<Vec<i32>> {
        self.exchange_collected(writes, vaults, None).await
    }
    pub async fn exchange_collected(
        &self,
        writes: Vec<InventoryWrite>,
        vaults: Vec<Vault>,
        collected: Option<u16>,
    ) -> Result<Vec<i32>> {
        self.exchange_transaction(writes, vaults, collected, None)
            .await
    }
    pub(crate) async fn exchange_trade(
        &self,
        transaction: String,
        writes: Vec<InventoryWrite>,
        delivery: Option<crate::bots::pursuits::market::Delivery>,
    ) -> Result<Vec<i32>> {
        ensure!(
            transaction.len() == 32
                && transaction.chars().all(|c| c.is_ascii_hexdigit())
                && writes.len() == 2,
            "invalid trade transaction identity"
        );
        if let Some(d) = &delivery {
            ensure!(
                *d == d.terms.delivery(d.ordinal)?,
                "invalid installment identity or quantities"
            );
            ensure!(
                writes
                    .iter()
                    .map(|w| w.account)
                    .collect::<std::collections::BTreeSet<_>>()
                    == d.terms.accounts.into_iter().collect(),
                "installment owners differ from negotiated accounts"
            );
        }
        self.exchange_transaction(writes, vec![], None, Some((transaction, delivery)))
            .await
    }
    pub(crate) async fn gift_committed(&self, acceptance: GiftAcceptance) -> Result<bool> {
        ensure!(
            acceptance.transaction.len() == 32
                && acceptance
                    .transaction
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit())
                && acceptance.session[1] > 0
                && acceptance.session[2] == acceptance.own.character
                && acceptance.peer_player > 0
                && acceptance.own.account != acceptance.peer.account
                && [acceptance.own, acceptance.peer]
                    .iter()
                    .all(|p| p.account > 0
                        && p.character > 0
                        && (0..i32::MAX).contains(&p.revision)),
            "invalid gift acceptance witness"
        );
        self.call(move |c| {
            let row: Option<(String, String, String, Option<String>)> = c
                .query_row(
                    "SELECT owners,writes,revisions,delivery FROM trade_transactions WHERE id=?1",
                    [&acceptance.transaction],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((owners, raw_writes, revisions, delivery)) = row else {
                return Ok(false);
            };
            let owners: Vec<i32> = serde_json::from_str(&owners)?;
            let writes: Vec<InventoryWrite> = serde_json::from_str(&raw_writes)?;
            let revisions: Vec<i32> = serde_json::from_str(&revisions)?;
            ensure!(
                delivery.is_none()
                    && writes.len() == 2
                    && revisions.len() == 2
                    && owners == writes.iter().map(|w| w.account).collect::<Vec<_>>()
                    && [acceptance.own, acceptance.peer]
                        .iter()
                        .all(|p| writes.iter().any(|w| w.account == p.account
                            && w.id == p.character
                            && w.revision == p.revision))
                    && writes
                        .iter()
                        .zip(revisions)
                        .all(|(w, revision)| w.revision.checked_add(1) == Some(revision))
                    && GiftAcceptance::payload_hash(acceptance.giving, &raw_writes)
                        == acceptance.writes_hash,
                "gift committed transaction payload/participants mismatch: {}",
                acceptance.transaction
            );
            Ok(true)
        })
        .await
    }
    pub(crate) async fn market_receipts(
        &self,
        account: i32,
        version: u64,
    ) -> Result<Vec<crate::bots::pursuits::market::Delivery>> {
        self.call(move |c| {
            let mut q=c.prepare("SELECT delivery FROM trade_transactions WHERE delivery IS NOT NULL AND EXISTS(SELECT 1 FROM json_each(owners) WHERE value=?1)")?;
            let mut receipts=Vec::new();
            for row in q.query_map([account],|r|r.get::<_,String>(0))? {
                let d:crate::bots::pursuits::market::Delivery=serde_json::from_str(&row?)?;
                if d.terms.version==version { receipts.push(d); }
            }
            receipts.sort_by_key(|d|d.ordinal);
            Ok(receipts)
        }).await
    }
    async fn exchange_transaction(
        &self,
        writes: Vec<InventoryWrite>,
        vaults: Vec<Vault>,
        collected: Option<u16>,
        receipt: Option<(String, Option<crate::bots::pursuits::market::Delivery>)>,
    ) -> Result<Vec<i32>> {
        ensure!(
            collected.is_none() || writes.len() == 1,
            "collection requires one pickup owner"
        );
        let collector = writes.first().context("missing inventory owner")?.account;
        ensure!(
            (1..=2).contains(&writes.len()),
            "exchange must contain one or two characters"
        );
        let mut chests = std::collections::BTreeSet::new();
        ensure!(vaults.len() <= 2, "too many vault exchange sources");
        for vault in &vaults {
            ensure!(chests.insert(vault.chest), "exchange repeats a chest");
            ensure!(
                writes.len() == 1 && writes[0].account == vault.account,
                "vault exchange must use its owner's character"
            );
            ensure!(
                vault.items.len() == 8 && vault.items.iter().all(|i| (-1..=65535).contains(i)),
                "invalid eight-slot vault"
            );
            ensure!(
                vault.revision >= 0 && vault.revision < i32::MAX,
                "invalid vault revision"
            );
        }
        let mut owners = std::collections::BTreeSet::new();
        for write in &writes {
            ensure!(owners.insert(write.account), "exchange repeats an account");
            ensure!(
                write.items.len() == 12 && write.items.iter().all(|i| (-1..=65535).contains(i)),
                "invalid twelve-slot inventory"
            );
            ensure!(
                write.revision >= 0 && write.revision < i32::MAX,
                "invalid inventory revision"
            );
        }
        self.call(move |c| {
            let tx = c.transaction()?;
            let raw_writes = serde_json::to_string(&writes)?;
            if let Some((id,delivery)) = &receipt {
                let prior:Option<(String,String,Option<String>)>=tx.query_row("SELECT writes,revisions,delivery FROM trade_transactions WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
                if let Some((previous,revisions,recorded))=prior {
                    ensure!(previous==raw_writes && recorded==delivery.as_ref().map(serde_json::to_string).transpose()?, "trade transaction replay changed its payload");
                    return Ok(serde_json::from_str(&revisions)?);
                }
            }
            let mut revisions = Vec::new();
            for write in &writes {
                let n = tx.execute("UPDATE characters SET items=?4,revision=revision+1 WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1", params![write.id, write.account, write.revision, serde_json::to_string(&write.items)?])?;
                ensure!(n == 1, "inventory conflict: character {}, account {}, expected revision {}", write.id, write.account, write.revision);
                revisions.push(write.revision + 1);
            }
            let rewarded = if let Some(item) = collected {
                tx.execute("INSERT OR IGNORE INTO collections(account,item,found_at) VALUES(?1,?2,unixepoch())", params![collector,item])? > 0
                    && gold::collection(&tx, collector)?
            } else { false };
            for vault in vaults { vaults::save(&tx, &vault)?; }
            if let Some((id,delivery))=receipt {
                let owners=serde_json::to_string(&writes.iter().map(|w|w.account).collect::<Vec<_>>())?;
                let market_key=delivery.as_ref().map(|d|format!("{:x}:{}",d.terms.version,d.ordinal));
                tx.execute("INSERT INTO trade_transactions(id,owners,writes,revisions,delivery,market_key) VALUES(?1,?2,?3,?4,?5,?6)",params![id,owners,raw_writes,serde_json::to_string(&revisions)?,delivery.as_ref().map(serde_json::to_string).transpose()?,market_key])?;
            }
            tx.commit()?;
            if rewarded {
                tracing::info!(account=collector, amount=gold::AMOUNT, reward="collection_top_half", "gold reward committed");
            }
            Ok(revisions)
        }).await
    }
    pub async fn consume_stat(
        &self,
        mut ch: Character,
        slot: Option<usize>,
        index: usize,
        amount: i32,
        vaults: Vec<Vault>,
    ) -> Result<i32> {
        let (kind, step) = match index {
            0 => (0xae9, 5),
            1 => (0xaea, 5),
            2 => (0xa1f, 1),
            3 => (0xa20, 1),
            4 => (0xa21, 1),
            5 => (0xa4c, 1),
            6 => (0xa34, 1),
            7 => (0xa35, 1),
            _ => anyhow::bail!("unsupported permanent potion stat"),
        };
        ensure!(
            amount > 0
                && amount <= step
                && ch.items.len() == 12
                && slot.is_none_or(|slot| ch.items.get(slot) == Some(&kind)),
            "permanent potion ownership or activation mismatch"
        );
        ensure!(
            self.content.get(u16::try_from(kind)?)?.is_potion(),
            "permanent consumable lacks selected potion metadata"
        );
        let cap = self
            .classes
            .get(&ch.class)
            .context("unknown potion class")?
            .caps[index];
        ensure!(
            ch.alive
                && ch.hp > 0
                && ch.base[index] >= 0
                && ch.base[index] < cap
                && amount == (cap - ch.base[index]).min(step),
            "dead character or permanent stat already capped"
        );
        ensure!(
            ch.revision >= 0 && ch.revision < i32::MAX,
            "invalid potion revision"
        );
        ensure!(
            vaults.len() <= 1
                && vaults
                    .iter()
                    .all(|v| slot.is_none() && v.account == ch.account),
            "potion vault owner mismatch"
        );
        self.call(move |db| {
            let tx = db.transaction()?;
            let (raw, items): (String, String) = tx.query_row(
                "SELECT stats,items FROM characters WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1",
                params![ch.id, ch.account, ch.revision], |r| Ok((r.get(0)?, r.get(1)?)))
                .context("permanent potion conflict: dead, foreign or stale character")?;
            let saved: Character = serde_json::from_str(&raw)?;
            ensure!(saved.schema == ch.schema && saved.class == ch.class && saved.base == ch.base
                && saved.level == ch.level && saved.exp == ch.exp && saved.fame == ch.fame
                && saved.pirate_caves == ch.pirate_caves && saved.dungeons == ch.dungeons && saved.abysses == ch.abysses && saved.trenches == ch.trenches && saved.spider_dens == ch.spider_dens && saved.mad_labs == ch.mad_labs && saved.jungles == ch.jungles && saved.manors == ch.manors && saved.beaches == ch.beaches && saved.davy_lockers == ch.davy_lockers && saved.tombs == ch.tombs
                && serde_json::from_str::<Vec<i32>>(&items)? == ch.items,
                "permanent potion reservation changed");
            ch.lifetime.follows(&saved.lifetime)?;
            ch.base[index] += amount;
            if let Some(slot) = slot {
                ch.items[slot] = -1;
                ch.lifetime.inventory_potion_used = true;
            }
            ch.revision += 1;
            let n = tx.execute("UPDATE characters SET stats=?4,items=?5,revision=revision+1 WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1",
                params![ch.id, ch.account, ch.revision - 1, serde_json::to_string(&ch)?, serde_json::to_string(&ch.items)?])?;
            ensure!(n == 1, "permanent potion commit lost its reservation");
            for vault in vaults { vaults::save(&tx, &vault)?; }
            tx.commit()?;
            Ok(ch.revision)
        }).await
    }
    pub async fn progress(&self, ch: Character) -> Result<()> {
        self.progress_with_assists(ch, vec![]).await
    }
    pub async fn progress_with_assists(
        &self,
        ch: Character,
        assists: Vec<LevelAssist>,
    ) -> Result<()> {
        self.write_progress(vec![(ch, assists)]).await
    }

    async fn write_progress(&self, characters: Vec<(Character, Vec<LevelAssist>)>) -> Result<()> {
        ensure!(
            (1..=85).contains(&characters.len()),
            "invalid progress batch size"
        );
        let mut accounts = std::collections::BTreeSet::new();
        ensure!(
            characters.iter().all(|(ch, _)| accounts.insert(ch.account)),
            "progress batch repeats an account"
        );
        self.call(move|db| {
            let tx=db.transaction()?;
            for (ch,assists) in characters {
                let raw:String=tx.query_row("SELECT stats FROM characters WHERE id=?1 AND account=?2 AND alive=1 AND revision=?3",params![ch.id,ch.account,ch.revision],|r|r.get(0)).with_context(||format!("progress conflict: dead, missing or stale character {} at revision {}",ch.id,ch.revision))?;
                let saved:Character=serde_json::from_str(&raw)?;
                ensure!(ch.alive && ch.schema==saved.schema && ch.class==saved.class && ch.level>=saved.level && ch.exp>=saved.exp && ch.fame>=saved.fame && ch.xp_boost.bonus_exp>=saved.xp_boost.bonus_exp,"progress snapshot lost committed experience");
                ch.lifetime.follows(&saved.lifetime)?;
                tx.execute("UPDATE characters SET stats=?3 WHERE id=?1 AND account=?2",params![ch.id,ch.account,serde_json::to_string(&ch)?])?;
                death_awards::save_assists(&tx,&ch,saved.level,&assists)?;
            }
            tx.commit()?;
            Ok(())
        }).await
    }
    pub(crate) async fn source_character_alive(
        &self,
        account: i32,
        character: i32,
    ) -> Result<bool> {
        self.call(move |db| {
            db.query_row(
                "SELECT alive FROM characters WHERE account=?1 AND id=?2",
                params![account, character],
                |r| r.get::<_, bool>(0),
            )
            .context("source character identity missing")
        })
        .await
    }
    pub async fn assist_count(&self, giver: i32) -> Result<u64> {
        self.call(move |db| {
            ensure!(
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM characters WHERE id=?1)",
                    [giver],
                    |r| r.get::<_, bool>(0)
                )?,
                "assist giver missing {giver}"
            );
            Ok(db.query_row(
                "SELECT count(*) FROM level_credits WHERE giver=?1",
                [giver],
                |r| r.get(0),
            )?)
        })
        .await
    }
    pub async fn death(&self, mut ch: Character, killed_by: String) -> Result<()> {
        ch.xp_boost.active = false;
        ensure!(
            !ch.alive && ch.hp == 0 && ch.fame >= 0 && ch.revision >= 0 && ch.revision < i32::MAX,
            "death snapshot must be dead with nonnegative fame and a valid revision"
        );
        let content = self.content.clone();
        self.call(move |db| {
            let tx = db.transaction()?;
            let (raw, items): (String, String) = tx.query_row(
                "SELECT stats,items FROM characters WHERE id=?1 AND account=?2 AND alive=1 AND revision=?3",
                params![ch.id, ch.account, ch.revision], |r| Ok((r.get(0)?, r.get(1)?)))
                .context("death conflict: duplicate, foreign or stale character")?;
            let saved: Character = serde_json::from_str(&raw)?;
            ensure!(saved.class == ch.class && saved.schema == ch.schema && ch.exp >= saved.exp
                && ch.fame >= saved.fame && ch.xp_boost.bonus_exp >= saved.xp_boost.bonus_exp
                && serde_json::from_str::<Vec<i32>>(&items)? == ch.items,
                "death snapshot lost committed progress or equipment");
            ch.lifetime.follows(&saved.lifetime)?;
            ch.lifetime.level_assists=ch.lifetime.level_assists.max(tx.query_row("SELECT count(*) FROM level_credits WHERE giver=?1",[ch.id],|r|r.get::<_,u64>(0))?);
            let history=death_awards::history(&tx,&ch)?;
            let wallet=tx.query_row("SELECT balance,earned FROM account_fame WHERE account=?1",[ch.account],|r|Ok((r.get(0)?,r.get(1)?))).context("account fame wallet missing")?;
            let guild=guilds::membership(&tx,ch.account)?.map(|m|tx.query_row("SELECT fame,earned FROM guilds WHERE id=?1",[m.id],|r|Ok((r.get(0)?,r.get(1)?)))).transpose()?;
            let receipt=death_awards::evaluate(&ch,&ch.lifetime,history,&content,death_awards::PayoutBalances{wallet,guild})?;
            let bonus=receipt.rows.iter().find(|r|r.kind==death_awards::BonusKind::WellEquipped).map_or(0,|r|r.amount);
            let total = receipt.total;
            let awards=serde_json::to_string(&receipt.rows)?;
            let snapshot = serde_json::to_string(&ch)?;
            let n = tx.execute("UPDATE characters SET alive=0,stats=?3,items=?4,revision=revision+1 WHERE id=?1 AND account=?2 AND alive=1 AND revision=?5",
                params![ch.id,ch.account,snapshot,serde_json::to_string(&[-1;12])?,ch.revision])?;
            ensure!(n == 1, "death lost its reservation");
            tx.execute("INSERT INTO deaths(character,snapshot,killed_by,equipment_fame,receipt) VALUES(?1,?2,?3,?4,?5)",
                params![ch.id,snapshot,killed_by,bonus,serde_json::to_string(&receipt)?])?;
            if receipt.rows.iter().any(|r|r.kind==death_awards::BonusKind::LegacyBuilder){tx.execute("INSERT INTO legacy_claims(account,class,character) VALUES(?1,?2,?3)",params![ch.account,ch.class,ch.id])?;}
            ensure!(tx.execute("UPDATE account_fame SET balance=balance+?2,earned=earned+?2 WHERE account=?1",params![ch.account,total])? == 1,
                "account fame wallet missing");
            guilds::credit_death(&tx,ch.account,total)?;
            tx.commit()?;
            tracing::info!(account=ch.account, character=ch.id, policy=?receipt.policy, completeness=?ch.lifetime.completeness, base_fame=ch.fame, awards=%awards, equipment_fame=bonus, total_fame=total, "permanent death award committed");
            Ok(())
        }).await
    }
    pub async fn fame(&self, account: i32, id: i32) -> Result<String> {
        let legacy_base = self.legacy_base;
        // The legacy field is called CreatedOn, but the client labels it as the death date.
        self.call(move|db|{let (snapshot,killer,died_at,paid):(String,String,u64,String)=db.query_row("SELECT snapshot,killed_by,died_at,receipt FROM deaths JOIN characters ON characters.id=deaths.character WHERE characters.account=?1 AND deaths.character=?2",params![account,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;let receipt=death_awards::decode_receipt(&paid)?;let c = receipt.character(&snapshot,legacy_base)?;ensure!(receipt.base==c.fame,"receipt base disagreement");ensure!(c.id==id && c.account==account && !c.alive && c.hp==0 && c.items.len()==12,"invalid archived death snapshot");let total=receipt.total;
        let [shots,hits,tiles,kills,gods,oryx,quests,assists]=match receipt.policy {
            death_awards::ReceiptPolicy::Historical=>{let l=&c.lifetime;[l.shots,l.projectile_hits,l.tiles,l.kills,l.god_kills,l.oryx_kills,l.qualified_quests,l.level_assists]},
            death_awards::ReceiptPolicy::LegacyPaid|death_awards::ReceiptPolicy::EquipmentOnly=>[c.shots,c.hits,c.tiles,c.kills,0,0,c.quests,0],
        };
        let equipment=receipt.rows.iter().map(|r|format!("<Bonus id=\"{}\" desc=\"{}\">{}</Bonus>",r.kind.id(),r.kind.description(),r.amount)).collect::<String>();ensure!(died_at>0 && died_at<=8_640_000_000_000,"invalid recorded death time");Ok(format!("<Fame><Char id=\"{}\"><Account><Name>{}</Name></Account><ObjectType>{}</ObjectType><Level>{}</Level><Tex1>{}</Tex1><Tex2>{}</Tex2></Char><CreatedOn>{}</CreatedOn><KilledBy>{}</KilledBy><TotalFame>{}</TotalFame><Shots>{}</Shots><ShotsThatDamage>{}</ShotsThatDamage><TilesUncovered>{}</TilesUncovered><MonsterKills>{}</MonsterKills><GodKills>{}</GodKills><OryxKills>{}</OryxKills><QuestsCompleted>{}</QuestsCompleted><PirateCavesCompleted>{}</PirateCavesCompleted><UndeadLairsCompleted>{}</UndeadLairsCompleted><AbyssOfDemonsCompleted>{}</AbyssOfDemonsCompleted><SnakePitsCompleted>{}</SnakePitsCompleted><SpiderDensCompleted>{}</SpiderDensCompleted><SpriteWorldsCompleted>{}</SpriteWorldsCompleted><TombsCompleted>{}</TombsCompleted><LevelUpAssists>{}</LevelUpAssists><BaseFame>{}</BaseFame>{}</Fame>",c.id,xml_escape(&c.name),c.class,c.level,c.tex1,c.tex2,died_at,xml_escape(&killer),total,shots,hits,tiles,kills,gods,oryx,quests,c.pirate_caves,c.dungeons[2],c.abysses,c.dungeons[0],c.spider_dens,c.dungeons[1],c.tombs,assists,c.fame,equipment))}).await
    }
    #[cfg(test)]
    pub async fn rename(&self, account: i32, name: String) -> Result<()> {
        ensure!(
            (3..=10).contains(&name.len()) && name.bytes().all(|c| c.is_ascii_alphabetic()),
            "names must be 3–10 letters"
        );
        self.call(move |c| {
            let n = c.execute(
                "UPDATE accounts SET name=?2 WHERE id=?1",
                params![account, name],
            )?;
            ensure!(n == 1, "account missing");
            Ok(())
        })
        .await
    }
}
fn read_presentation(
    db: &Connection,
    account: i32,
    legacy_base: [i32; 8],
    classes: &BTreeMap<u16, Class>,
) -> Result<(AccountPresentation, i64)> {
    let (balance, earned, credits): (i64, i64, i32) = db
        .query_row(
            "SELECT balance,earned,credits FROM account_fame JOIN accounts ON accounts.id=account_fame.account WHERE account=?1",
            [account],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .context("read account fame wallet")?;
    ensure!(credits >= 0, "invalid account gold balance");
    let fame = i32::try_from(balance).context("account fame exceeds wire range")?;
    ensure!(
        fame >= 0 && earned >= balance && earned <= i64::from(i32::MAX),
        "invalid account fame wallet"
    );
    let mut best_fame = BTreeMap::<u16, i32>::new();
    // SQL owns live identities; a committed death's immutable snapshot remains authoritative.
    let mut q = db.prepare("SELECT c.id,CASE WHEN d.character IS NULL THEN json_set(c.stats,'$.id',c.id,'$.account',c.account) ELSE d.snapshot END,d.receipt FROM characters c LEFT JOIN deaths d ON d.character=c.id WHERE c.account=?1")?;
    for row in q.query_map([account], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })? {
        let (id, raw, paid) = row?;
        let receipt = paid
            .as_deref()
            .map(death_awards::decode_receipt)
            .transpose()
            .with_context(|| format!("decode class death receipt {id}"))?;
        let committed_death = receipt.is_some();
        let ch = match &receipt {
            Some(receipt) => receipt.character(&raw, legacy_base),
            None => decode_character(&raw, legacy_base),
        }
        .with_context(|| format!("decode saved class record {id}"))?;
        let class = classes
            .get(&ch.class)
            .with_context(|| format!("unknown class in saved class record {id}"))?;
        ensure!(
            ch.id == id
                && ch.account == account
                && (!committed_death || (!ch.alive && ch.hp == 0))
                && ch.fame >= 0
                && (1..=20).contains(&ch.level)
                && ch.items.len() == 12
                && ch
                    .base
                    .iter()
                    .zip(class.caps)
                    .all(|(value, cap)| *value >= 0 && *value <= cap),
            "invalid saved class record {id}"
        );
        // Before the 2021 base-only change, class quests included death bonuses: https://steamcommunity.com/games/200210/announcements/detail/4032392655081525895
        let score = match receipt {
            Some(receipt) => receipt.total,
            None => ch.fame,
        };
        let best = best_fame.entry(ch.class).or_default();
        *best = (*best).max(score);
    }
    Ok((
        AccountPresentation {
            guild: guilds::membership(db, account)?,
            credits,
            fame,
            best_fame,
        },
        earned,
    ))
}
fn rewarded_presentation(
    db: &Connection,
    account: i32,
    legacy_base: [i32; 8],
    classes: &BTreeMap<u16, Class>,
) -> Result<(AccountPresentation, i64)> {
    let (mut presentation, earned) = read_presentation(db, account, legacy_base, classes)?;
    if gold::class_quests(db, account, presentation.stars(), classes.len())? {
        presentation.credits =
            db.query_row("SELECT credits FROM accounts WHERE id=?1", [account], |r| {
                r.get(0)
            })?;
    }
    Ok((presentation, earned))
}
pub fn name_chosen(account: i32, name: &str) -> bool {
    name != format!("Guest{account}")
}
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
impl Account {
    pub fn xml(&self, news: &str, usage: f64) -> String {
        let mut chars = String::new();
        for c in &self.characters {
            chars += &format!(
                "<Char id=\"{}\"><ObjectType>{}</ObjectType><Level>{}</Level><Exp>{}</Exp><CurrentFame>{}</CurrentFame><Tex1>{}</Tex1><Tex2>{}</Tex2><Equipment>{}</Equipment><MaxHitPoints>{}</MaxHitPoints><HitPoints>{}</HitPoints><MaxMagicPoints>{}</MaxMagicPoints><MagicPoints>{}</MagicPoints><Attack>{}</Attack><Defense>{}</Defense><Speed>{}</Speed><Dexterity>{}</Dexterity><HpRegen>{}</HpRegen><MpRegen>{}</MpRegen></Char>",
                c.id,
                c.class,
                c.level,
                c.exp,
                c.fame,
                c.tex1,
                c.tex2,
                c.items
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                c.base[0],
                c.hp,
                c.base[1],
                c.mp,
                c.base[2],
                c.base[3],
                c.base[4],
                c.base[5],
                c.base[6],
                c.base[7]
            );
        }
        let chosen = if name_chosen(self.id, &self.name) {
            "<NameChosen/>"
        } else {
            ""
        };
        let guild = self
            .guild
            .as_ref()
            .map(|g| {
                format!(
                    "<Guild id=\"{}\"><Name>{}</Name><Rank>{}</Rank></Guild>",
                    g.id,
                    xml_escape(&g.name),
                    g.rank
                )
            })
            .unwrap_or_default();
        let records: String = self.best_levels.iter().map(|(class, level)| {
            let fame = self.best_fame.get(class).copied().unwrap_or(0);
            format!("<ClassStats objectType=\"{class}\"><BestLevel>{level}</BestLevel><BestFame>{fame}</BestFame></ClassStats>")
        }).collect();
        format!(
            "<Chars nextCharId=\"{}\" maxNumChars=\"{}\">{chars}<Account><AccountId>{}</AccountId><Name>{}</Name>{chosen}<Stats><Fame>{}</Fame><TotalFame>{}</TotalFame>{records}</Stats><Credits>{}</Credits><NextCharSlotPrice>{}</NextCharSlotPrice>{guild}</Account><Servers><Server><Name>Local development</Name><DNS>127.0.0.1</DNS><Lat>0</Lat><Long>0</Long><Usage>{usage}</Usage></Server></Servers><News>{news}</News></Chars>",
            self.next_id,
            self.character_slots,
            self.id,
            xml_escape(&self.name),
            self.fame,
            self.earned_fame,
            self.credits,
            self.next_slot_price
        )
    }
}
#[cfg(test)]
mod class_quest_tests;
#[cfg(test)]
mod community_schema_tests;
