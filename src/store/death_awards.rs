use super::*;

pub const DEATHS_SQL: &str = "CREATE TABLE deaths(character INTEGER PRIMARY KEY REFERENCES characters(id),snapshot TEXT NOT NULL,killed_by TEXT NOT NULL,died_at INTEGER NOT NULL DEFAULT(unixepoch()),equipment_fame INTEGER NOT NULL CHECK(typeof(equipment_fame)='integer' AND equipment_fame BETWEEN 0 AND 2147483647),receipt TEXT NOT NULL CHECK(json_valid(receipt)))";
pub const CLAIMS_SQL: &str = "CREATE TABLE legacy_claims(account INTEGER NOT NULL REFERENCES accounts(id),class INTEGER NOT NULL CHECK(typeof(class)='integer' AND class BETWEEN 0 AND 65535),character INTEGER NOT NULL UNIQUE REFERENCES deaths(character),PRIMARY KEY(account,class))";
pub const CREDITS_SQL: &str = "CREATE TABLE level_credits(recipient INTEGER NOT NULL REFERENCES characters(id),reached_level INTEGER NOT NULL CHECK(typeof(reached_level)='integer' AND reached_level BETWEEN 2 AND 20),giver INTEGER NOT NULL REFERENCES characters(id),PRIMARY KEY(recipient,reached_level),CHECK(recipient<>giver))";
pub const CREDITS_INDEX_SQL: &str = "CREATE INDEX level_credits_by_giver ON level_credits(giver)";

pub(super) fn check_schema(db: &Connection, version: i32) -> Result<()> {
    let columns = {
        let mut q = db.prepare("SELECT name FROM pragma_table_xinfo('deaths') ORDER BY cid")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    ensure!(
        version != 16 || !columns.iter().any(|column| column == "receipt"),
        "private Fame schema16 is not canonical community schema16; preserve database and request an explicit migration"
    );
    let mut expected = vec!["character", "snapshot", "killed_by", "died_at"];
    if version >= 14 {
        expected.push("equipment_fame");
    }
    if version >= 17 {
        expected.push("receipt");
    }
    ensure!(
        (version == 0 && columns.is_empty()) || columns.iter().map(String::as_str).eq(expected),
        "death award schema/version/column disagreement; restore reviewed state"
    );
    if version >= 14 {
        let reference: bool = db.query_row("SELECT count(*)=1 AND count(*) FILTER(WHERE \"table\"='characters' AND \"from\"='character' AND \"to\"='id' AND seq=0 AND on_update='NO ACTION' AND on_delete='NO ACTION')=1 FROM pragma_foreign_key_list('deaths')",[],|r|r.get(0))?;
        ensure!(reference, "unsupported death character foreign key");
        for (column, kind, not_null, default, key) in [
            ("character", "INTEGER", false, None, 1),
            ("snapshot", "TEXT", true, None, 0),
            ("killed_by", "TEXT", true, None, 0),
            ("died_at", "INTEGER", true, Some("unixepoch()"), 0),
            ("equipment_fame", "INTEGER", true, None, 0),
        ] {
            let metadata: (String, bool, Option<String>, i32, i32) = db.query_row(
                "SELECT type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo('deaths') WHERE name=?1", [column],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            ensure!(
                metadata == (kind.into(), not_null, default.map(str::to_owned), key, 0),
                "unsupported canonical death metadata: {column}"
            );
        }
    }
    for (name, definition) in [
        ("deaths", DEATHS_SQL),
        ("legacy_claims", CLAIMS_SQL),
        ("level_credits", CREDITS_SQL),
        ("level_credits_by_giver", CREDITS_INDEX_SQL),
    ] {
        let sql: Option<String> = db
            .query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .optional()?;
        if version >= 17 {
            ensure!(
                sql.as_deref() == Some(definition),
                "unsupported canonical death fame metadata: {name}"
            );
        } else if name != "deaths" {
            ensure!(sql.is_none(), "unexpected {name} before schema 17");
        }
    }
    Ok(())
}

pub(super) fn migrate(db: &Connection, version: i32, legacy_base: [i32; 8]) -> Result<()> {
    if version < 14 {
        db.execute_batch("ALTER TABLE deaths ADD COLUMN equipment_fame INTEGER NOT NULL DEFAULT 0 CHECK(typeof(equipment_fame)='integer' AND equipment_fame BETWEEN 0 AND 2147483647)")?;
    }
    if version < 17 {
        let records = {
            let mut q=db.prepare("SELECT character,snapshot,killed_by,died_at,equipment_fame FROM deaths ORDER BY character")?;
            q.query_map([], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i32>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        db.execute_batch("ALTER TABLE deaths RENAME TO legacy_deaths")?;
        db.execute_batch(DEATHS_SQL)?;
        for (id, raw, killer, time, equipment) in records {
            let ch = decode_legacy_character(&raw, legacy_base)?;
            let receipt = DeathFameReceipt::paid(&ch, equipment, ReceiptPolicy::LegacyPaid)?;
            db.execute("INSERT INTO deaths(character,snapshot,killed_by,died_at,equipment_fame,receipt) VALUES(?1,?2,?3,?4,?5,?6)",params![id,raw,killer,time,equipment,serde_json::to_string(&receipt)?])?;
        }
        db.execute_batch(
            "DROP TABLE legacy_deaths; CREATE INDEX deaths_by_time ON deaths(died_at)",
        )?;
        db.execute_batch(CLAIMS_SQL)?;
        db.execute_batch(CREDITS_SQL)?;
        db.execute_batch(CREDITS_INDEX_SQL)?;
    }
    check_schema(db, 17)?;
    Ok(())
}

pub(super) fn validate_state(db: &Connection, legacy_base: [i32; 8]) -> Result<()> {
    let accounts = {
        let mut q = db.prepare("SELECT id FROM accounts")?;
        q.query_map([], |r| r.get::<_, i32>(0))?
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?
    };
    let mut characters = BTreeMap::new();
    let mut q = db.prepare("SELECT id,account,stats FROM characters")?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, i32>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, account, raw) = row?;
        let ch = decode_character(&raw, legacy_base)
            .with_context(|| format!("invalid current lifetime for character {id}"))?;
        ensure!(
            id > 0
                && account > 0
                && accounts.contains(&account)
                && (ch.id == 0 || ch.id == id)
                && ch.account == account
                && (1..=20).contains(&ch.level),
            "invalid current character ownership or level {id}"
        );
        characters.insert(id, (account, ch.level, ch.class));
    }
    let mut q=db.prepare("SELECT d.character,c.account,c.alive,d.snapshot,d.equipment_fame,d.receipt,d.died_at FROM deaths d LEFT JOIN characters c ON c.id=d.character")?;
    let mut claimed = std::collections::BTreeSet::new();
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, i32>(1)?,
            r.get::<_, bool>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i32>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })? {
        let (id, account, alive, raw, equipment, paid, time) = row?;
        let receipt = decode_receipt(&paid)?;
        let ch = receipt.character(&raw, legacy_base)?;
        ensure!(
            ch.id == id
                && ch.account == account
                && characters
                    .get(&id)
                    .is_some_and(|(_, _, class)| *class == ch.class)
                && !alive
                && !ch.alive
                && ch.hp == 0
                && ch.items.len() == 12
                && (1..=8_640_000_000_000).contains(&time),
            "invalid archived death ownership or snapshot {id}"
        );
        ensure!(
            receipt.base == ch.fame
                && equipment >= 0
                && receipt
                    .rows
                    .iter()
                    .find(|r| r.kind == BonusKind::WellEquipped)
                    .map_or(0, |r| r.amount)
                    == equipment,
            "death receipt/equipment disagreement {id}"
        );
        if receipt
            .rows
            .iter()
            .any(|r| r.kind == BonusKind::LegacyBuilder)
        {
            claimed.insert((account, ch.class, id));
        }
    }
    let mut q = db.prepare("SELECT account,class,character FROM legacy_claims")?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, u16>(1)?,
            r.get::<_, i32>(2)?,
        ))
    })? {
        let claim = row?;
        ensure!(
            claimed.remove(&claim),
            "invalid Legacy claim binding {claim:?}"
        );
    }
    ensure!(
        claimed.is_empty(),
        "paid Legacy receipt lacks its account/class claim"
    );
    let mut q = db.prepare("SELECT recipient,reached_level,giver FROM level_credits")?;
    let mut levels = std::collections::BTreeSet::new();
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, i32>(1)?,
            r.get::<_, i32>(2)?,
        ))
    })? {
        let (recipient, level, giver) = row?;
        ensure!(
            (2..=20).contains(&level)
                && recipient != giver
                && characters
                    .get(&recipient)
                    .is_some_and(|(_, current, _)| level <= *current)
                && characters.contains_key(&giver)
                && levels.insert((recipient, level)),
            "invalid level credit recipient={recipient} level={level} giver={giver}"
        );
    }
    let broken:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check('legacy_claims')) OR EXISTS(SELECT 1 FROM pragma_foreign_key_check('level_credits'))",[],|r|r.get(0))?;
    ensure!(!broken, "invalid fame claim or level credit foreign key");
    validate_wallets(db)
}

pub(super) fn validate_wallets(db: &Connection) -> Result<()> {
    let missing:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM accounts a LEFT JOIN account_fame f ON f.account=a.id WHERE f.account IS NULL)",[],|r|r.get(0))?;
    ensure!(
        !missing,
        "account fame wallet missing; restore reviewed state instead of recreating balances"
    );
    let broken: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM account_fame f LEFT JOIN (SELECT c.account,sum(json_extract(d.receipt,'$.total')) AS paid FROM deaths d JOIN characters c ON c.id=d.character GROUP BY c.account) d ON d.account=f.account WHERE f.earned<>coalesce(d.paid,0) OR f.balance<0 OR f.balance>f.earned OR f.earned>2147483647)",[],|r|r.get(0))?;
    ensure!(
        !broken,
        "archived death receipts disagree with account fame wallets; restore reviewed state"
    );
    guilds::validate_fame(db)
}

pub(super) fn save_assists(
    db: &Connection,
    ch: &Character,
    previous_level: i32,
    assists: &[LevelAssist],
) -> Result<()> {
    ensure!(
        assists.len() <= 19,
        "too many level assists for recipient {}",
        ch.id
    );
    let mut levels = std::collections::BTreeSet::new();
    for assist in assists {
        let level = i32::from(assist.reached_level);
        ensure!(
            (2..=20).contains(&level)
                && level <= ch.level
                && assist.giver > 0
                && assist.giver != ch.id
                && levels.insert(level),
            "invalid level assist for recipient {}: {assist:?}",
            ch.id
        );
        let prior: Option<i32> = db
            .query_row(
                "SELECT giver FROM level_credits WHERE recipient=?1 AND reached_level=?2",
                params![ch.id, level],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(giver) = prior {
            ensure!(giver == assist.giver, "level assist replay changed giver");
        } else {
            ensure!(
                level > previous_level,
                "cannot manufacture retroactive level credit"
            );
            ensure!(
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM characters WHERE id=?1 AND alive=1)",
                    [assist.giver],
                    |r| r.get::<_, bool>(0)
                )?,
                "level assist giver missing or already dead {}",
                assist.giver
            );
            db.execute(
                "INSERT INTO level_credits(recipient,reached_level,giver) VALUES(?1,?2,?3)",
                params![ch.id, level, assist.giver],
            )?;
        }
    }
    Ok(())
}

pub(super) fn history(db: &Connection, ch: &Character) -> Result<DeathHistory> {
    let mut history = DeathHistory {
        retained_deaths: 0,
        other_same_class_max_level: 0,
        legacy_claimed: db.query_row(
            "SELECT EXISTS(SELECT 1 FROM legacy_claims WHERE account=?1 AND class=?2)",
            params![ch.account, ch.class],
            |r| r.get(0),
        )?,
        prior_paid_max: 0,
    };
    let mut q=db.prepare("SELECT d.receipt FROM deaths d JOIN characters c ON c.id=d.character WHERE c.account=?1 AND c.id<>?2")?;
    for row in q.query_map(params![ch.account, ch.id], |r| r.get::<_, String>(0))? {
        let receipt = decode_receipt(&row?)?;
        history.retained_deaths = history
            .retained_deaths
            .checked_add(1)
            .context("retained death count overflow")?;
        history.prior_paid_max = history.prior_paid_max.max(receipt.total);
    }
    let mut q=db.prepare("SELECT stats FROM characters WHERE account=?1 AND id<>?2 AND json_extract(stats,'$.class')=?3")?;
    for row in q.query_map(params![ch.account, ch.id, ch.class], |r| {
        r.get::<_, String>(0)
    })? {
        let other: Character = serde_json::from_str(&row?)?;
        history.other_same_class_max_level = history.other_same_class_max_level.max(other.level);
    }
    Ok(history)
}

fn equipment_percent(ch: &Character, content: &Content) -> Result<u64> {
    ensure!(
        ch.items.len() == 12 && ch.fame >= 0,
        "invalid death equipment or base fame"
    );
    ch.effective(content)?;
    let mut percent = 0_u64;
    for kind in &ch.items[..4] {
        if *kind == -1 {
            continue;
        }
        let item = content.get(u16::try_from(*kind)?)?;
        ensure!(
            item.class == "Equipment" && !item.resurrects,
            "invalid permanent-death equipment {}",
            item.name
        );
        percent = percent
            .checked_add(u64::from(item.fame_bonus))
            .context("equipment percent overflow")?;
    }
    Ok(percent)
}

#[cfg(test)]
pub(super) fn equipment(ch: &Character, content: &Content) -> Result<i32> {
    let percent = equipment_percent(ch, content)?;
    // The May2012 calculator applies the combined percent once to the running death subtotal.
    let bonus = u64::try_from(ch.fame)?
        .checked_mul(percent)
        .context("equipment fame overflow")?
        / 100;
    let bonus = i32::try_from(bonus).context("equipment fame exceeds wire range")?;
    total(ch, bonus)?;
    Ok(bonus)
}

pub(super) fn total(ch: &Character, equipment: i32) -> Result<i32> {
    ensure!(
        ch.fame >= 0 && equipment >= 0,
        "invalid archived death fame"
    );
    ch.fame
        .checked_add(equipment)
        .context("total death fame overflow")
}

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BonusKind {
    Ancestor,
    LegacyBuilder,
    Pacifist,
    Thirsty,
    Mundane,
    BootsOnTheGround,
    TunnelRat,
    EnemyOfTheGods,
    SlayerOfTheGods,
    OryxSlayer,
    Accurate,
    Sharpshooter,
    Sniper,
    Explorer,
    Cartographer,
    TeamPlayer,
    LeaderOfMen,
    DoerOfDeeds,
    FriendOfTheCubes,
    WellEquipped,
    FirstBorn,
}

impl BonusKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Ancestor => "Ancestor",
            Self::LegacyBuilder => "Legacy Builder",
            Self::Pacifist => "Pacifist",
            Self::Thirsty => "Thirsty",
            Self::Mundane => "Mundane",
            Self::BootsOnTheGround => "Boots on the Ground",
            Self::TunnelRat => "Tunnel Rat",
            Self::EnemyOfTheGods => "Enemy of the Gods",
            Self::SlayerOfTheGods => "Slayer of the Gods",
            Self::OryxSlayer => "Oryx Slayer",
            Self::Accurate => "Accurate",
            Self::Sharpshooter => "Sharpshooter",
            Self::Sniper => "Sniper",
            Self::Explorer => "Explorer",
            Self::Cartographer => "Cartographer",
            Self::TeamPlayer => "Team Player",
            Self::LeaderOfMen => "Leader of Men",
            Self::DoerOfDeeds => "Doer of Deeds",
            Self::FriendOfTheCubes => "Friend of the Cubes",
            Self::WellEquipped => "Well Equipped",
            Self::FirstBorn => "First Born",
        }
    }
    pub fn description(self) -> &'static str {
        if self == Self::WellEquipped {
            "Bonus for equipment"
        } else {
            self.id()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BonusRow {
    pub kind: BonusKind,
    pub amount: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptPolicy {
    LegacyPaid,
    EquipmentOnly,
    Historical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeathFameReceipt {
    pub policy: ReceiptPolicy,
    #[serde(deserialize_with = "deserialize_fame_version")]
    pub version: u8,
    pub base: i32,
    pub rows: Vec<BonusRow>,
    pub total: i32,
}
pub fn decode_receipt(raw: &str) -> Result<DeathFameReceipt> {
    let receipt: DeathFameReceipt = serde_json::from_str(raw)?;
    receipt.validate()?;
    Ok(receipt)
}
impl DeathFameReceipt {
    pub fn paid(ch: &Character, equipment: i32, policy: ReceiptPolicy) -> Result<Self> {
        ensure!(
            policy != ReceiptPolicy::Historical,
            "historical receipts require the ordered evaluator"
        );
        let receipt = Self {
            policy,
            version: 1,
            base: ch.fame,
            total: total(ch, equipment)?,
            rows: if equipment > 0 {
                vec![BonusRow {
                    kind: BonusKind::WellEquipped,
                    amount: equipment,
                }]
            } else {
                vec![]
            },
        };
        receipt.validate()?;
        Ok(receipt)
    }
    pub fn character(&self, raw: &str, legacy_base: [i32; 8]) -> Result<Character> {
        let ch = if self.policy == ReceiptPolicy::LegacyPaid {
            decode_legacy_character(raw, legacy_base)
        } else {
            decode_character(raw, legacy_base)
        }?;
        ensure!(
            ch.fame == self.base,
            "death receipt base disagrees with archived snapshot"
        );
        Ok(ch)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported death receipt version {}",
            self.version
        );
        ensure!(
            self.base >= 0 && self.total >= 0 && self.rows.len() <= 21,
            "invalid death receipt bounds"
        );
        ensure!(
            self.policy == ReceiptPolicy::Historical
                || self
                    .rows
                    .iter()
                    .all(|row| row.kind == BonusKind::WellEquipped),
            "non-historical receipt contains a historical bonus"
        );
        let mut previous = None;
        let mut total = self.base;
        for row in &self.rows {
            ensure!(
                previous.is_none_or(|p| p < row.kind),
                "duplicate or out-of-order death bonus {:?}",
                row.kind
            );
            ensure!(row.amount >= 0, "negative death bonus {:?}", row.kind);
            total = total
                .checked_add(row.amount)
                .context("death receipt total overflow")?;
            previous = Some(row.kind);
        }
        ensure!(
            total == self.total,
            "death receipt base + rows disagrees with total"
        );
        Ok(())
    }
    fn award(&mut self, kind: BonusKind, percent: u64, flat: i32) -> Result<()> {
        let amount = u64::try_from(self.total)?
            .checked_mul(percent)
            .context("death bonus multiplication overflow")?
            / 100;
        let amount = i32::try_from(amount)
            .context("death bonus exceeds wire range")?
            .checked_add(flat)
            .context("death bonus flat overflow")?;
        self.total = self
            .total
            .checked_add(amount)
            .context("death total exceeds wire range")?;
        self.rows.push(BonusRow { kind, amount });
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DeathHistory {
    pub retained_deaths: u64,
    // No other retained class record has domain maximum zero; the dying character is excluded.
    pub other_same_class_max_level: i32,
    pub legacy_claimed: bool,
    // Empty prior PAID history has domain maximum zero, across all classes.
    pub prior_paid_max: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct PayoutBalances {
    pub wallet: (i32, i32),
    pub guild: Option<(i32, i32)>,
}
impl PayoutBalances {
    fn validate_credit(self, total: i32) -> Result<()> {
        for (name, (balance, earned)) in
            std::iter::once(("wallet", self.wallet)).chain(self.guild.map(|g| ("guild", g)))
        {
            ensure!(
                balance >= 0 && earned >= balance,
                "invalid {name} fame balances"
            );
            balance
                .checked_add(total)
                .with_context(|| format!("{name} fame balance overflow"))?;
            earned
                .checked_add(total)
                .with_context(|| format!("{name} earned fame overflow"))?;
        }
        Ok(())
    }
}

pub fn evaluate(
    ch: &Character,
    lifetime: &FameLifetime,
    history: DeathHistory,
    content: &Content,
    balances: PayoutBalances,
) -> Result<DeathFameReceipt> {
    use BonusKind::*;
    lifetime.validate()?;
    ensure!(
        (0..=20).contains(&history.other_same_class_max_level) && history.prior_paid_max >= 0,
        "invalid retained death history maxima"
    );
    let percent = equipment_percent(ch, content)?;
    let complete = lifetime.completeness == LifetimeCompleteness::Complete;
    let max_level = ch.level >= 20;
    let gods = |threshold| {
        complete
            && max_level
            && lifetime.kills != 0
            && u128::from(lifetime.god_kills) * 100 > u128::from(lifetime.kills) * threshold
    };
    let accuracy = |threshold| {
        complete
            && max_level
            && lifetime.shots != 0
            && u128::from(lifetime.projectile_hits) * 100 > u128::from(lifetime.shots) * threshold
    };
    let mut receipt = DeathFameReceipt {
        policy: ReceiptPolicy::Historical,
        version: 1,
        base: ch.fame,
        rows: Vec::new(),
        total: ch.fame,
    };
    for (kind, eligible, rate, flat) in [
        (Ancestor, history.retained_deaths == 0, 10, 20),
        (
            LegacyBuilder,
            ch.level >= 2
                && ch.level > history.other_same_class_max_level
                && !history.legacy_claimed,
            10,
            10,
        ),
        (
            Pacifist,
            complete && max_level && lifetime.projectile_hits == 0,
            25,
            0,
        ),
        (
            Thirsty,
            complete && max_level && !lifetime.inventory_potion_used,
            25,
            0,
        ),
        (
            Mundane,
            complete && max_level && !lifetime.ability_used,
            25,
            0,
        ),
        (
            BootsOnTheGround,
            complete && max_level && !lifetime.teleported,
            25,
            0,
        ),
        (
            TunnelRat,
            crate::dungeon::HistoricalDungeon::ALL
                .iter()
                .all(|d| lifetime.realm_exits.contains(d)),
            10,
            0,
        ),
        (EnemyOfTheGods, gods(10), 10, 0),
        (SlayerOfTheGods, gods(50), 10, 0),
        (OryxSlayer, lifetime.oryx_kills > 0, 10, 0),
        (Accurate, accuracy(25), 10, 0),
        (Sharpshooter, accuracy(50), 10, 0),
        (Sniper, accuracy(75), 10, 0),
        (Explorer, lifetime.tiles > 1_000_000, 5, 0),
        (Cartographer, lifetime.tiles > 4_000_000, 5, 0),
        (TeamPlayer, lifetime.level_assists > 100, 10, 0),
        (LeaderOfMen, lifetime.level_assists > 1000, 10, 0),
        (DoerOfDeeds, lifetime.qualified_quests > 1000, 10, 0),
        (
            FriendOfTheCubes,
            complete && max_level && lifetime.cube_kills == 0,
            10,
            0,
        ),
        (WellEquipped, percent > 0, percent, 0),
    ] {
        if eligible {
            receipt.award(kind, rate, flat)?;
        }
    }
    if receipt.total > history.prior_paid_max {
        receipt.award(FirstBorn, 10, 0)?;
    }
    receipt.validate()?;
    balances.validate_credit(receipt.total)?;
    Ok(receipt)
}
