use super::hiscores::{code_hash, discord_id, now};
use super::*;

fn reset_hash(token: &str) -> Result<String> {
    ensure!(
        token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid or expired reset link"
    );
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(token.as_bytes())))
}
fn reset_owner(db: &Connection, hash: &str, time: i64) -> Result<i32> {
    db.query_row("SELECT p.account FROM password_resets p JOIN discord_recovery r ON r.account=p.account JOIN credentials c ON c.account=p.account JOIN accounts a ON a.id=p.account WHERE p.hash=?1 AND p.expires_at>?2 AND a.guid IS NOT NULL", params![hash,time], |r| r.get(0)).optional()?.context("invalid or expired reset link")
}
fn replace_password(tx: &rusqlite::Transaction<'_>, account: i32, verifier: &str) -> Result<()> {
    ensure!(
        tx.execute(
            "UPDATE credentials SET password_hash=?2 WHERE account=?1",
            params![account, verifier]
        )? == 1,
        "registered credential missing"
    );
    for table in ["password_resets", "recovery_codes", "discord_codes"] {
        tx.execute(&format!("DELETE FROM {table} WHERE account=?1"), [account])?;
    }
    Ok(())
}
impl Store {
    pub async fn change_password(&self, account: i32, verifier: String) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            replace_password(&tx, account, &verifier)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn linked_account(&self, discord: String) -> Result<i32> {
        discord_id(&discord)?;
        self.call(move |db| {
            db.query_row("SELECT l.account FROM discord_links l JOIN credentials c ON c.account=l.account JOIN accounts a ON a.id=l.account WHERE l.discord_id=?1 AND a.guid IS NOT NULL AND NOT EXISTS(SELECT 1 FROM shore_bots b WHERE b.account=l.account)", [discord], |r| r.get(0)).optional()?.context("no authenticated game account linked to your Discord identity")
        }).await
    }
    pub async fn issue_recovery_code(&self, account: i32) -> Result<String> {
        let time = now()?;
        let code = format!("{:032x}", rand::random::<u128>());
        let hash = code_hash(&code)?;
        self.call(move |db| {
            let tx = db.transaction()?;
            let eligible: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM discord_links l JOIN credentials c ON c.account=l.account WHERE l.account=?1)", [account], |r| r.get(0))?;
            ensure!(eligible, Rejected("link your registered game account using /link in Discord first"));
            let previous: Option<i64> = tx.query_row("SELECT issued_at FROM recovery_codes WHERE account=?1", [account], |r| r.get(0)).optional()?;
            ensure!(previous.is_none_or(|t| time-t >= 60), Rejected("wait one minute before requesting another recovery code"));
            tx.execute("DELETE FROM recovery_codes WHERE expires_at<=?1", [time])?;
            tx.execute("INSERT INTO recovery_codes(account,hash,issued_at,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(account) DO UPDATE SET hash=excluded.hash,issued_at=excluded.issued_at,expires_at=excluded.expires_at", params![account,hash,time,time+300])?;
            tx.commit()?;
            Ok(())
        }).await?;
        tracing::info!(account, "private recovery consent code issued");
        Ok(code)
    }
    pub async fn issue_reset(&self, discord: String, code: Option<String>) -> Result<String> {
        discord_id(&discord)?;
        let consent_hash = code.map(|c| code_hash(&c)).transpose()?;
        let time = now()?;
        let token = format!(
            "{:032x}{:032x}",
            rand::random::<u128>(),
            rand::random::<u128>()
        );
        let hash = reset_hash(&token)?;
        let account = self.call(move |db| {
            let tx = db.transaction()?;
            let account: i32 = tx.query_row("SELECT l.account FROM discord_links l JOIN credentials c ON c.account=l.account JOIN accounts a ON a.id=l.account WHERE l.discord_id=?1 AND a.guid IS NOT NULL AND NOT EXISTS(SELECT 1 FROM shore_bots b WHERE b.account=l.account)", [discord], |r| r.get(0)).optional()?.context("no authenticated game account linked to your Discord identity")?;
            if let Some(consent_hash) = consent_hash {
                let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM recovery_codes WHERE account=?1 AND hash=?2 AND expires_at>?3)", params![account,consent_hash,time], |r| r.get(0))?;
                ensure!(valid, "invalid or expired recovery consent code");
                tx.execute("INSERT INTO discord_recovery(account,enabled_at,last_issued_at) VALUES(?1,?2,0) ON CONFLICT(account) DO NOTHING", params![account,time])?;
                tx.execute("DELETE FROM recovery_codes WHERE account=?1", [account])?;
            }
            let previous: i64 = tx.query_row("SELECT last_issued_at FROM discord_recovery WHERE account=?1", [account], |r| r.get(0)).optional()?.context("recovery is not enabled; use /link in Discord while signed into your game account")?;
            ensure!(time-previous >= 60, "wait one minute before requesting another reset link");
            tx.execute("DELETE FROM password_resets WHERE expires_at<=?1", [time])?;
            tx.execute("INSERT INTO password_resets(account,hash,issued_at,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(account) DO UPDATE SET hash=excluded.hash,issued_at=excluded.issued_at,expires_at=excluded.expires_at", params![account,hash,time,time+900])?;
            tx.execute("UPDATE discord_recovery SET last_issued_at=?2 WHERE account=?1", params![account,time])?;
            tx.commit()?;
            Ok(account)
        }).await?;
        tracing::info!(account, "private password reset link issued");
        Ok(token)
    }
    pub async fn unlink_discord(&self, discord: String) -> Result<Option<i32>> {
        discord_id(&discord)?;
        self.call(move |db| {
            let tx = db.transaction()?;
            let account: Option<i32> = tx
                .query_row(
                    "SELECT account FROM discord_links WHERE discord_id=?1",
                    [&discord],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(account) = account {
                for table in [
                    "password_resets",
                    "recovery_codes",
                    "discord_codes",
                    "discord_recovery",
                    "discord_links",
                ] {
                    tx.execute(&format!("DELETE FROM {table} WHERE account=?1"), [account])?;
                }
            }
            tx.commit()?;
            tracing::info!(account, "Discord association and recovery revoked");
            Ok(account)
        })
        .await
    }
    pub async fn disable_recovery(&self, account: i32) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            for table in ["password_resets", "recovery_codes", "discord_recovery"] {
                tx.execute(&format!("DELETE FROM {table} WHERE account=?1"), [account])?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
        tracing::info!(account, "Discord password recovery revoked");
        Ok(())
    }
    pub async fn reset_account(&self, token: String) -> Result<i32> {
        let hash = reset_hash(&token)?;
        self.call(move |db| reset_owner(db, &hash, now()?)).await
    }
    pub async fn prepare_password(&self, password: String) -> Result<String> {
        accounts::password(&password)?;
        let permit = self
            .hashes
            .clone()
            .try_acquire_owned()
            .context("account service busy; retry shortly")?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            accounts::hash(&password)
        })
        .await?
    }
    pub async fn finish_reset(&self, token: String, verifier: String) -> Result<i32> {
        let hash = reset_hash(&token)?;
        let account = self
            .call(move |db| {
                let tx = db.transaction()?;
                let account = reset_owner(&tx, &hash, now()?)?;
                replace_password(&tx, account, &verifier)?;
                tx.commit()?;
                Ok(account)
            })
            .await?;
        tracing::info!(account, "password reset committed; reset secrets revoked");
        Ok(account)
    }
}
