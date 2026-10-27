use super::*;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use rusqlite::OptionalExtension;

pub(super) fn hash(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|v| v.to_string())
        .map_err(|_| anyhow::anyhow!("password hashing failed"))
}
fn email(value: &str) -> Result<String> {
    // HELLO credentials must fit the server's 2048-bit PKCS#1 v1.5 block.
    ensure!(
        value.len() <= 245 && value.is_ascii(),
        "email must be ASCII and at most 245 bytes for native login"
    );
    let (local, domain) = value.split_once('@').context("invalid email address")?;
    ensure!(
        !local.is_empty()
            && local.len() <= 64
            && !local.starts_with('.')
            && !local.ends_with('.')
            && !local.contains("..")
            && local
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
            && domain.contains('.')
            && domain.split('.').all(|label| !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')),
        "invalid email address"
    );
    Ok(value.to_ascii_lowercase())
}
pub(super) fn password(value: &str) -> Result<()> {
    ensure!(
        (5..=128).contains(&value.len()) && !value.chars().any(char::is_control),
        "password must be 5–128 bytes without control characters"
    );
    Ok(())
}
fn guest(db: &Connection, guid: &str, create: bool, authentication: bool) -> Result<i32> {
    let rejected = |reason: &'static str| -> anyhow::Error {
        if authentication {
            InvalidCredentials.into()
        } else {
            anyhow::anyhow!(reason)
        }
    };
    // Reserve the email namespace so an empty password can never create a shadow login.
    if !(!guid.is_empty()
        && guid.len() <= 128
        && guid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
    {
        return Err(rejected("invalid guest identity"));
    }
    let found: Option<(i32, bool)> = db.query_row(
        "SELECT id,EXISTS(SELECT 1 FROM credentials WHERE account=accounts.id) FROM accounts WHERE guid=?1",
        [guid], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    if let Some((id, registered)) = found {
        if registered {
            return Err(rejected(
                "guest identity has been converted; sign in with your email and password",
            ));
        }
        return Ok(id);
    }
    if !create {
        return Err(rejected("unknown guest; load character list first"));
    }
    db.execute("INSERT INTO accounts(guid,name) VALUES(?1,'Guest')", [guid])?;
    let id = i32::try_from(db.last_insert_rowid())?;
    vaults::provision(db, id)?;
    db.execute(
        "INSERT INTO account_fame(account,balance,earned) VALUES(?1,0,0)",
        [id],
    )?;
    db.execute(
        "UPDATE accounts SET name=?2 WHERE id=?1",
        params![id, format!("Guest{id}")],
    )?;
    Ok(id)
}
pub(super) fn human_name_taken(db: &Connection, name: &str, except: i32) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id!=?2 AND name=?1 COLLATE NOCASE AND (NOT EXISTS(SELECT 1 FROM shore_bots WHERE account=accounts.id) OR EXISTS(SELECT 1 FROM credentials WHERE account=accounts.id)))",
        params![name, except], |r| r.get(0),
    )?)
}

pub(super) fn named_account(db: &Connection, name: &str) -> Result<Option<i32>> {
    // Historical persona rows keep their names after a human claims the active name.
    let (humans, human, oldest): (i32, Option<i32>, Option<i32>) = db.query_row(
        "WITH matching AS (
            SELECT id,NOT EXISTS(SELECT 1 FROM shore_bots WHERE account=accounts.id)
                OR EXISTS(SELECT 1 FROM credentials WHERE account=accounts.id) AS human
            FROM accounts WHERE name=?1 COLLATE NOCASE
        ) SELECT count(*) FILTER(WHERE human),min(id) FILTER(WHERE human),min(id) FROM matching",
        [name],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    ensure!(
        humans <= 1,
        Rejected("Ambiguous human name ownership; contact the server operator.")
    );
    Ok(if humans == 1 { human } else { oldest })
}

type NameChoiceReady = (i32, i32, Vec<i32>);

fn name_choice_check(
    db: &Connection,
    account: i32,
    name: &str,
    allow_paid: bool,
) -> Result<std::result::Result<NameChoiceReady, &'static str>> {
    if !(3..=10).contains(&name.len()) || !name.bytes().all(|c| c.is_ascii_alphabetic()) {
        return Ok(Err("names must be 3–10 letters"));
    }
    let (current, registered, balance): (String, bool, i32) = db.query_row(
        "SELECT name,EXISTS(SELECT 1 FROM credentials WHERE account=accounts.id),credits FROM accounts WHERE id=?1",
        [account], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    ensure!(balance >= 0, "invalid account Gold balance");
    if !registered {
        return Ok(Err("register before choosing a name"));
    }
    let price = if name_chosen(account, &current) {
        1000
    } else {
        0
    };
    if price > 0 && !allow_paid {
        return Ok(Err("Paid name changes must be made in game."));
    }
    if human_name_taken(db, name, 0)? {
        return Ok(Err("name already taken"));
    }
    if balance < price {
        return Ok(Err("Not enough Gold."));
    }
    let bots = db
        .prepare("SELECT id FROM accounts WHERE name=?1 COLLATE NOCASE ORDER BY id")?
        .query_map([name], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Ok((price, balance, bots)))
}

impl Store {
    #[cfg(test)]
    pub async fn choose_name(&self, account: i32, name: String) -> Result<()> {
        match self.claim_name_choice(account, name, false, vec![]).await? {
            NameChoice::Committed { .. } => Ok(()),
            NameChoice::Rejected(reason) => Err(Rejected(reason).into()),
        }
    }
    pub(crate) async fn prepare_name_choice(
        &self,
        account: i32,
        name: String,
        allow_paid: bool,
    ) -> Result<std::result::Result<Vec<i32>, &'static str>> {
        self.call(move |db| {
            Ok(name_choice_check(db, account, &name, allow_paid)?.map(|(_, _, bots)| bots))
        })
        .await
    }
    pub(crate) async fn claim_name_choice(
        &self,
        account: i32,
        name: String,
        allow_paid: bool,
        stopped: Vec<i32>,
    ) -> Result<NameChoice> {
        self.call(move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let (price, balance, bots) = match name_choice_check(&tx, account, &name, allow_paid)? {
                Ok(ready) => ready,
                Err(reason) => return Ok(NameChoice::Rejected(reason)),
            };
            // Store-only callers cannot publish over a controller they have not settled.
            if bots.iter().any(|id| !stopped.contains(id)) {
                return Ok(NameChoice::Rejected("name already taken"));
            }
            let balance = balance - price;
            ensure!(
                tx.execute(
                    "UPDATE accounts SET name=?2,credits=?3 WHERE id=?1",
                    params![account, name, balance]
                )? == 1,
                "name choice account missing"
            );
            tx.commit()?;
            tracing::info!(
                account,
                name,
                price,
                balance,
                ?stopped,
                "account name choice committed"
            );
            Ok(NameChoice::Committed { balance })
        })
        .await
    }
    pub(super) async fn guest(&self, guid: String, create: bool) -> Result<i32> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let id = guest(&tx, &guid, create, true)?;
            tx.commit()?;
            Ok(id)
        })
        .await
    }
    pub async fn register(
        &self,
        guid: String,
        new_email: String,
        new_password: String,
    ) -> Result<i32> {
        let email = email(&new_email)?;
        password(&new_password)?;
        let permit = self
            .hashes
            .clone()
            .try_acquire_owned()
            .context("account service busy; retry shortly")?;
        let verifier = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash(&new_password)
        })
        .await??;
        let id = self
            .call(move |db| {
                let tx = db.transaction()?;
                let id = guest(&tx, &guid, true, false)?;
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM credentials WHERE email=?1)",
                    [&email],
                    |r| r.get(0),
                )?;
                ensure!(!exists, "email already registered; sign in instead");
                tx.execute(
                    "INSERT INTO credentials(account,email,password_hash) VALUES(?1,?2,?3)",
                    params![id, email, verifier],
                )?;
                tx.commit()?;
                Ok(id)
            })
            .await?;
        tracing::info!(
            account = id,
            "guest account registered; guest authentication revoked"
        );
        Ok(id)
    }
    pub async fn authenticate(&self, guid: String, clear: String) -> Result<i32> {
        if clear.is_empty() {
            return self.guest(guid, false).await;
        }
        let email = email(&guid).map_err(|_| InvalidCredentials)?;
        password(&clear).map_err(|_| InvalidCredentials)?;
        let found: Option<(i32, String)> = self
            .call(move |db| {
                Ok(db
                    .query_row(
                        "SELECT account,password_hash FROM credentials WHERE email=?1",
                        [email],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?)
            })
            .await?;
        let verifier = match &found {
            Some((_, hash)) => hash.clone(),
            None => self.dummy_hash.clone(),
        };
        let permit = self
            .hashes
            .clone()
            .try_acquire_owned()
            .context("account service busy; retry shortly")?;
        let valid = tokio::task::spawn_blocking(move || -> Result<bool> {
            let _permit = permit;
            let hash = PasswordHash::new(&verifier)
                .map_err(|_| anyhow::anyhow!("invalid stored password verifier"))?;
            match Argon2::default().verify_password(clear.as_bytes(), &hash) {
                Ok(()) => Ok(true),
                Err(argon2::password_hash::Error::Password) => Ok(false),
                Err(_) => anyhow::bail!("invalid stored password verifier"),
            }
        })
        .await??;
        ensure!(valid, InvalidCredentials);
        let (id, _) = found.ok_or(InvalidCredentials)?;
        Ok(id)
    }
}
