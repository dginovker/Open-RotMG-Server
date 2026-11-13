use super::*;
use anyhow::bail;
#[cfg(test)]
mod tests;
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use sha2::{Digest, Sha256};

#[derive(serde::Serialize)]
pub(crate) struct LocalAccount {
    pub account: i32,
    pub name: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub expected: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalAccountEdit {
    pub account: i32,
    pub expected: String,
    pub username: String,
    pub password: String,
}
fn expectation(account: i32, username: &Option<String>, hash: &Option<String>) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(account, username, hash))?)
    ))
}
pub(crate) fn password_key(verifier: &str) -> String {
    format!("{:x}", Sha256::digest(verifier.as_bytes()))
}
pub(crate) fn local_accounts(
    db: &Connection,
    passwords: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<LocalAccount>> {
    let mut query = db.prepare("SELECT a.id,a.name,c.email,c.password_hash FROM accounts a LEFT JOIN credentials c ON c.account=a.id ORDER BY a.id")?;
    let rows = query.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (account, name, username, hash) = row?;
        let password = match (
            hash.as_ref().and_then(|h| passwords.get(&password_key(h))),
            &hash,
        ) {
            (Some(clear), Some(verifier)) => {
                let parsed = PasswordHash::new(verifier)
                    .map_err(|_| anyhow::anyhow!("invalid local password verifier"))?;
                match Argon2::default().verify_password(clear.as_bytes(), &parsed) {
                    Ok(()) => Some(clear.clone()),
                    Err(argon2::password_hash::Error::Password) => None,
                    Err(_) => bail!("local password verification failed"),
                }
            }
            _ => None,
        };
        Ok(LocalAccount {
            account,
            name,
            expected: expectation(account, &username, &hash)?,
            username,
            password,
        })
    })
    .collect()
}
pub(crate) fn validate_local_edit(edit: &LocalAccountEdit) -> Result<String> {
    ensure!(
        edit.account > 0
            && edit.expected.len() == 64
            && edit.expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid account expectation"
    );
    accounts::password(&edit.password)?;
    accounts::email(&edit.username)
}
pub(crate) fn edit_local_account(
    tx: &rusqlite::Transaction<'_>,
    edit: &LocalAccountEdit,
) -> Result<()> {
    let username = validate_local_edit(edit)?;
    let (old_username, old_hash): (Option<String>, Option<String>) = tx.query_row("SELECT c.email,c.password_hash FROM accounts a LEFT JOIN credentials c ON c.account=a.id WHERE a.id=?1", [edit.account], |r| Ok((r.get(0)?, r.get(1)?)))?;
    ensure!(
        expectation(edit.account, &old_username, &old_hash)? == edit.expected,
        "account changed; refresh before editing"
    );
    let taken: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM credentials WHERE email=?1 AND account<>?2)",
        params![username, edit.account],
        |r| r.get(0),
    )?;
    ensure!(!taken, "username belongs to another local account");
    let verifier = accounts::hash(&edit.password)?;
    tx.execute("INSERT INTO credentials(account,email,password_hash) VALUES(?1,?2,?3) ON CONFLICT(account) DO UPDATE SET email=excluded.email,password_hash=excluded.password_hash", params![edit.account, username, verifier])?;
    for table in [
        "password_resets",
        "email_password_resets",
        "recovery_codes",
        "discord_codes",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE account=?1"),
            [edit.account],
        )?;
    }
    Ok(())
}
