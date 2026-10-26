use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Membership {
    pub id: i32,
    pub name: String,
    pub rank: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Guild {
    pub id: i32,
    pub name: String,
    pub level: i32,
    pub fame: i32,
    pub earned: i32,
    pub board: String,
}

pub enum GuildAction {
    Create(String),
    Join { guild: i32, inviter: i32 },
    Rank { name: String, rank: i32 },
    Remove(String),
    Upgrade { level: i32 },
}

pub const HALL_PRICES: [i32; 3] = [10_000, 100_000, 250_000];

pub fn allowed_rank(authority: i32, old: i32, new: i32) -> bool {
    [0, 10, 20, 30, 40].contains(&old)
        && [0, 10, 20, 30].contains(&new)
        && old != new
        && match authority {
            40 => old < 40,
            30 => old < 30 && new <= 30,
            20 => old < 20 && new < 20,
            _ => false,
        }
}

pub(super) fn migrate(db: &Connection, version: i32) -> Result<()> {
    if version >= 9 {
        for table in ["guilds", "guild_members"] {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )?;
            ensure!(
                exists,
                "schema-9 {table} missing; restore reviewed guild state instead of recreating it"
            );
        }
    } else {
        db.execute_batch("CREATE TABLE guilds(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 20 AND name=trim(name) AND name NOT GLOB '*[^A-Za-z ]*'),level INTEGER NOT NULL DEFAULT 1 CHECK(level BETWEEN 1 AND 4),fame INTEGER NOT NULL DEFAULT 0 CHECK(fame BETWEEN 0 AND 2147483647),earned INTEGER NOT NULL DEFAULT 0 CHECK(earned BETWEEN fame AND 2147483647),board TEXT NOT NULL DEFAULT '');
CREATE TABLE guild_members(account INTEGER PRIMARY KEY REFERENCES accounts(id),guild INTEGER NOT NULL REFERENCES guilds(id),rank INTEGER NOT NULL CHECK(rank IN (0,10,20,30,40)),fame INTEGER NOT NULL DEFAULT 0 CHECK(fame BETWEEN 0 AND 2147483647));
CREATE UNIQUE INDEX one_guild_founder ON guild_members(guild) WHERE rank=40;
CREATE INDEX guild_roster ON guild_members(guild);
CREATE TRIGGER guild_capacity BEFORE INSERT ON guild_members WHEN (SELECT count(*) FROM guild_members WHERE guild=NEW.guild)>=50 BEGIN SELECT RAISE(ABORT,'Guild is full.'); END;")?;
    }
    let broken: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check('guild_members')) OR EXISTS(SELECT 1 FROM guilds g LEFT JOIN guild_members m ON m.guild=g.id GROUP BY g.id HAVING count(m.account) NOT BETWEEN 1 AND 50 OR sum(m.rank=40)>1 OR coalesce(sum(m.fame),0)>g.earned)", [], |r|r.get(0))?;
    ensure!(
        !broken,
        "Guild membership or contribution integrity failed; restore reviewed state."
    );
    for name in ["one_guild_founder", "guild_roster", "guild_capacity"] {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?;
        ensure!(
            exists,
            "Guild schema object {name} missing; restore reviewed state."
        );
    }
    Ok(())
}

pub(super) fn membership(db: &Connection, account: i32) -> Result<Option<Membership>> {
    Ok(db.query_row(
        "SELECT g.id,g.name,m.rank FROM guild_members m LEFT JOIN guilds g ON g.id=m.guild WHERE m.account=?1",
        [account], |r| Ok(Membership { id:r.get(0)?, name:r.get(1)?, rank:r.get(2)? }),
    ).optional()?)
}

fn member(db: &Connection, account: i32) -> Result<Membership> {
    membership(db, account)?.context("You are not in a guild.")
}

fn named(db: &Connection, account: i32) -> Result<String> {
    let name: String = db.query_row("SELECT name FROM accounts WHERE id=?1", [account], |r| {
        r.get(0)
    })?;
    ensure!(
        name_chosen(account, &name),
        "Choose a name before joining or creating a guild."
    );
    Ok(name)
}

fn info(db: &Connection, id: i32) -> Result<Guild> {
    Ok(db.query_row(
        "SELECT id,name,level,fame,earned,board FROM guilds WHERE id=?1",
        [id],
        |r| {
            Ok(Guild {
                id: r.get(0)?,
                name: r.get(1)?,
                level: r.get(2)?,
                fame: r.get(3)?,
                earned: r.get(4)?,
                board: r.get(5)?,
            })
        },
    )?)
}

pub(super) fn validate_fame(db: &Connection) -> Result<()> {
    let broken:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM guild_members m JOIN account_fame f ON f.account=m.account WHERE m.fame>f.earned) OR (SELECT coalesce(sum(earned),0) FROM guilds)>(SELECT coalesce(sum(earned),0) FROM account_fame)",[],|r|r.get(0))?;
    ensure!(
        !broken,
        "Guild contribution or earnings exceed immutable paid account fame; restore reviewed state"
    );
    Ok(())
}

pub(super) fn credit_death(db: &Connection, account: i32, amount: i32) -> Result<()> {
    if let Some(member) = membership(db, account)? {
        ensure!(
            db.execute(
                "UPDATE guilds SET fame=fame+?2,earned=earned+?2 WHERE id=?1",
                params![member.id, amount]
            )? == 1,
            "Guild fame wallet missing."
        );
        ensure!(
            db.execute(
                "UPDATE guild_members SET fame=fame+?2 WHERE account=?1",
                params![account, amount]
            )? == 1,
            "Guild contribution missing."
        );
    }
    Ok(())
}

impl Store {
    pub async fn guild(&self, account: i32) -> Result<Guild> {
        self.call(move |db| info(db, member(db, account)?.id)).await
    }

    pub async fn guild_roster(&self, account: i32, offset: usize, count: usize) -> Result<String> {
        ensure!(
            offset <= 50 && (1..=50).contains(&count),
            "Invalid guild roster page."
        );
        self.call(move |db| {
            let tx = db.transaction()?;
            let guild = info(&tx, member(&tx, account)?.id)?;
            let mut out = format!("<Guild name=\"{}\" id=\"{}\"><TotalFame>{}</TotalFame><CurrentFame>{}</CurrentFame><HallType>Guild Hall {}</HallType>", xml_escape(&guild.name), guild.id, guild.earned, guild.fame, guild.level);
            {
                let mut q = tx.prepare("SELECT a.name,m.rank,m.fame FROM guild_members m JOIN accounts a ON a.id=m.account WHERE m.guild=?1 ORDER BY m.rank DESC,m.fame DESC,a.name COLLATE NOCASE LIMIT ?2 OFFSET ?3")?;
                for row in q.query_map(params![guild.id,count as i32,offset as i32], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i32>(1)?,r.get::<_,i32>(2)?)))? {
                    let (name,rank,fame) = row?;
                    out += &format!("<Member><Name>{}</Name><Rank>{rank}</Rank><Fame>{fame}</Fame></Member>", xml_escape(&name));
                }
            }
            out += "</Guild>";
            tx.commit()?;
            Ok(out)
        }).await
    }

    pub async fn guild_board(&self, account: i32, text: Option<String>) -> Result<String> {
        if let Some(text) = &text {
            ensure!(
                text.encode_utf16().count() <= 4096,
                "Guild Board is limited to 4096 characters."
            );
            ensure!(
                !text
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
                "Guild Board contains control characters."
            );
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            let member = member(&tx, account)?;
            if let Some(text) = text {
                ensure!(
                    member.rank >= 20,
                    "Only Officers, Leaders and the Founder may edit the Guild Board."
                );
                ensure!(
                    tx.execute(
                        "UPDATE guilds SET board=?2 WHERE id=?1",
                        params![member.id, text]
                    )? == 1,
                    "Guild missing."
                );
            }
            let board = info(&tx, member.id)?.board;
            tx.commit()?;
            Ok(board)
        })
        .await
    }

    // Prepared relations must not manufacture paid Fame or class-star history.
    pub(crate) async fn create_prepared_guild(
        &self,
        accounts: Vec<i32>,
        name: String,
    ) -> Result<Vec<(i32, AccountPresentation)>> {
        let base = self.legacy_base;
        let classes = self.classes.clone();
        self.call(move |db| {
            ensure!(!name.is_empty() && name.len()<=20 && name.trim()==name && name.bytes().all(|c|c.is_ascii_alphabetic()||c==b' '),Rejected("prepared guild name must contain 1–20 letters and spaces without surrounding spaces"));
            ensure!((1..=50).contains(&accounts.len()) && accounts.iter().collect::<std::collections::BTreeSet<_>>().len()==accounts.len(),Rejected("prepared guild requires 1–50 distinct registered actor accounts"));
            let tx=db.transaction()?;
            let taken:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM guilds WHERE name=?1 COLLATE NOCASE)",[&name],|r|r.get(0))?;
            ensure!(!taken,Rejected("prepared guild name is already taken"));
            for account in &accounts {
                ensure!(membership(&tx,*account)?.is_none(),Rejected("prepared actor is already in a guild"));
                read_presentation(&tx,*account,base,&classes)?;
            }
            tx.execute("INSERT INTO guilds(name) VALUES(?1)",[name])?;
            let guild=i32::try_from(tx.last_insert_rowid())?;
            for (index,account) in accounts.iter().enumerate() {
                tx.execute("INSERT INTO guild_members(account,guild,rank) VALUES(?1,?2,?3)",params![account,guild,if index==0 {40}else{0}])?;
            }
            let presentations=accounts.into_iter().map(|account|Ok((account,read_presentation(&tx,account,base,&classes)?.0))).collect::<Result<Vec<_>>>()?;
            tx.commit()?;Ok(presentations)
        }).await
    }

    pub async fn change_guild(&self, account: i32, action: GuildAction) -> Result<(i32, i32)> {
        let base = self.legacy_base;
        let classes = self.classes.clone();
        let operation = match &action {
            GuildAction::Create(_) => "create",
            GuildAction::Join { .. } => "join",
            GuildAction::Rank { .. } => "rank",
            GuildAction::Remove(_) => "remove",
            GuildAction::Upgrade { .. } => "upgrade",
        };
        let result = self.call(move |db| {
            let tx = db.transaction()?;
            let result = match action {
                GuildAction::Create(name) => {
                    ensure!(!name.is_empty() && name.len() <= 20 && name.bytes().all(|c| c.is_ascii_alphabetic() || c==b' '), "Guild names must contain 1–20 letters and spaces.");
                    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
                    ensure!(!name.is_empty(), "Guild name cannot be empty.");
                    named(&tx, account)?;
                    ensure!(membership(&tx, account)?.is_none(), "You are already in a guild.");
                    let taken: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM guilds WHERE name=?1 COLLATE NOCASE)", [&name], |r| r.get(0))?;
                    ensure!(!taken, "Guild name is already taken.");
                    let balance:i32=tx.query_row("SELECT balance FROM account_fame WHERE account=?1",[account],|r|r.get(0))?;
                    ensure!(balance>=1000,"Creating a guild costs 1000 Fame.");
                    ensure!(tx.execute("UPDATE account_fame SET balance=balance-1000 WHERE account=?1 AND balance>=1000", [account])? == 1, "Guild creation wallet conflict.");
                    tx.execute("INSERT INTO guilds(name) VALUES(?1)", [name])?;
                    let guild = i32::try_from(tx.last_insert_rowid())?;
                    tx.execute("INSERT INTO guild_members(account,guild,rank) VALUES(?1,?2,40)", params![account,guild])?;
                    (guild, account)
                }
                GuildAction::Join { guild, inviter } => {
                    named(&tx, account)?;
                    ensure!(membership(&tx, account)?.is_none(), "You are already in a guild.");
                    let inviter = member(&tx, inviter)?;
                    ensure!(inviter.id == guild && inviter.rank >= 20, "Guild invitation is no longer authorized.");
                    let (presentation, _) = read_presentation(&tx, account, base, &classes)?;
                    ensure!(presentation.stars() >= 1, "Earn your first class star before joining a guild.");
                    let count:i32=tx.query_row("SELECT count(*) FROM guild_members WHERE guild=?1",[guild],|r|r.get(0))?;
                    ensure!(count<50,"Guild is full.");
                    tx.execute("INSERT INTO guild_members(account,guild,rank) VALUES(?1,?2,0)", params![account,guild])?;
                    (guild, account)
                }
                GuildAction::Rank { name, rank } => {
                    let author = member(&tx, account)?;
                    let target: i32 = tx.query_row("SELECT id FROM accounts WHERE name=?1 COLLATE NOCASE", [name], |r| r.get(0)).optional()?.context("Player not found.")?;
                    let old = member(&tx, target)?;
                    ensure!(author.id == old.id && allowed_rank(author.rank, old.rank, rank), "You cannot make that guild rank change.");
                    tx.execute("UPDATE guild_members SET rank=?2 WHERE account=?1", params![target,rank])?;
                    (author.id, target)
                }
                GuildAction::Remove(name) => {
                    let author = member(&tx, account)?;
                    let target: i32 = tx.query_row("SELECT id FROM accounts WHERE name=?1 COLLATE NOCASE", [name], |r| r.get(0)).optional()?.context("Player not found.")?;
                    let old = member(&tx, target)?;
                    ensure!(author.id == old.id && (account == target || (author.rank >= 20 && author.rank > old.rank)), "You cannot remove that guild member.");
                    tx.execute("DELETE FROM guild_members WHERE account=?1", [target])?;
                    tx.execute("DELETE FROM guilds WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM guild_members WHERE guild=?1)", [author.id])?;
                    (author.id, target)
                }
                GuildAction::Upgrade { level } => {
                    let author = member(&tx, account)?;
                    ensure!(author.rank >= 30, "Only Leaders and the Founder may upgrade the Guild Hall.");
                    ensure!((1..=3).contains(&level), "Guild Hall is fully upgraded or its level is invalid.");
                    let price = HALL_PRICES[(level-1) as usize];
                    let guild=info(&tx,author.id)?;
                    ensure!(guild.level==level,"This upgrade is already purchased. Leave the hall empty for a few minutes.");
                    ensure!(guild.fame>=price,"Not enough Guild Fame.");
                    ensure!(tx.execute("UPDATE guilds SET fame=fame-?3,level=level+1 WHERE id=?1 AND level=?2 AND fame>=?3", params![author.id,level,price])? == 1, "Guild upgrade wallet conflict.");
                    (author.id, account)
                }
            };
            tx.commit()?;
            Ok(result)
        }).await?;
        tracing::info!(
            account,
            operation,
            guild = result.0,
            affected_account = result.1,
            "guild change committed"
        );
        Ok(result)
    }
}
