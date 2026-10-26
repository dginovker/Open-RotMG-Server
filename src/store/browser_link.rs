use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserIdentity {
    pub account: i32,
    pub version: String,
}

fn identity(db: &Connection, account: i32) -> Result<(BrowserIdentity, String)> {
    let (name, verifier): (String, String) = db.query_row(
        "SELECT a.name,c.password_hash FROM accounts a JOIN credentials c ON c.account=a.id WHERE a.id=?1 AND NOT EXISTS(SELECT 1 FROM shore_bots b WHERE b.account=a.id)",
        [account], |r| Ok((r.get(0)?,r.get(1)?)),
    ).context("registered game authentication required")?;
    Ok((
        BrowserIdentity {
            account,
            version: format!("{:x}", Sha256::digest(verifier.as_bytes())),
        },
        name,
    ))
}

impl Store {
    pub(crate) async fn browser_identity(&self, account: i32) -> Result<BrowserIdentity> {
        self.call(move |db| Ok(identity(db, account)?.0)).await
    }

    pub(crate) async fn link_browser_discord(
        &self,
        expected: BrowserIdentity,
        discord: String,
    ) -> Result<String> {
        super::hiscores::discord_id(&discord)?;
        self.call(move |db| {
            let tx = db.transaction()?;
            let (current, name) = identity(&tx, expected.account)?;
            ensure!(current == expected, "game authentication changed; sign in again");
            let conflict: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM discord_links WHERE (account=?1 OR discord_id=?2) AND NOT(account=?1 AND discord_id=?2))",
                params![current.account,discord], |r| r.get(0),
            )?;
            ensure!(!conflict, "a game or Discord account is already linked; links are never replaced");
            tx.execute("INSERT INTO discord_links(account,discord_id) VALUES(?1,?2) ON CONFLICT(account) DO NOTHING", params![current.account,discord])?;
            tx.execute("INSERT INTO discord_recovery(account,enabled_at,last_issued_at) VALUES(?1,?2,0) ON CONFLICT(account) DO NOTHING", params![current.account,super::hiscores::now()?])?;
            tx.execute("DELETE FROM recovery_codes WHERE account=?1", [current.account])?;
            let rewarded = gold::grant(&tx, current.account, gold::DISCORD)?;
            tx.commit()?;
            tracing::info!(account=current.account, rewarded, "browser Discord linking and recovery enabled");
            Ok(name)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn linking_enables_recovery_atomically_once_and_never_replaces_identities() {
        let content = Content::load(Path::new("data")).unwrap();
        let store = Store::open(Path::new(":memory:"), &content).unwrap();
        let id = store
            .register(
                "browser-guest".into(),
                "browser@example.test".into(),
                "test-password".into(),
            )
            .await
            .unwrap();
        let proof = store.browser_identity(id).await.unwrap();
        let discord = "12345678901234567".to_string();
        store.choose_name(id, "BrowserQA".into()).await.unwrap();
        let legacy_code = store.issue_discord_code(id).await.unwrap();
        assert_eq!(
            store
                .link_browser_discord(proof.clone(), discord.clone())
                .await
                .unwrap(),
            "BrowserQA"
        );
        store.issue_reset(discord.clone(), None).await.unwrap();
        store
            .link_browser_discord(proof.clone(), discord.clone())
            .await
            .unwrap();
        let credits: i32 = store
            .call(move |db| {
                Ok(
                    db.query_row("SELECT credits FROM accounts WHERE id=?1", [id], |r| {
                        r.get(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(credits, 200);
        assert!(
            store
                .link_browser_discord(proof.clone(), "12345678901234568".into())
                .await
                .is_err()
        );
        let other = store
            .register(
                "other-browser-guest".into(),
                "other-browser@example.test".into(),
                "test-password".into(),
            )
            .await
            .unwrap();
        assert!(
            store
                .link_browser_discord(
                    store.browser_identity(other).await.unwrap(),
                    discord.clone()
                )
                .await
                .is_err()
        );
        store.disable_recovery(id).await.unwrap();
        assert!(store.issue_reset(discord.clone(), None).await.is_err());
        store
            .link_browser_discord(proof.clone(), discord.clone())
            .await
            .unwrap();
        let reset = store.issue_reset(discord.clone(), None).await.unwrap();
        let consent = store.issue_recovery_code(id).await.unwrap();
        let peer_discord = "12345678901234569".to_string();
        store
            .link_browser_discord(
                store.browser_identity(other).await.unwrap(),
                peer_discord.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            store.unlink_discord(discord.clone()).await.unwrap(),
            Some(id)
        );
        assert_eq!(store.unlink_discord(discord.clone()).await.unwrap(), None);
        assert!(store.linked_account(discord.clone()).await.is_err());
        assert!(store.reset_account(reset.clone()).await.is_err());
        let prepared = store
            .prepare_password("unused-password".into())
            .await
            .unwrap();
        assert!(store.finish_reset(reset, prepared).await.is_err());
        assert!(
            store
                .issue_reset(discord.clone(), Some(consent))
                .await
                .is_err()
        );
        assert!(
            store
                .redeem_discord_code(legacy_code, discord.clone())
                .await
                .is_err()
        );
        assert_eq!(store.linked_account(peer_discord).await.unwrap(), other);
        assert_eq!(store.browser_identity(id).await.unwrap(), proof);
        store
            .link_browser_discord(proof.clone(), discord.clone())
            .await
            .unwrap();
        store
            .call(move |db| {
                let credits: i32 =
                    db.query_row("SELECT credits FROM accounts WHERE id=?1", [id], |r| {
                        r.get(0)
                    })?;
                assert_eq!(
                    credits, 200,
                    "Unlink/relink must not repeat the Gold reward"
                );
                Ok(())
            })
            .await
            .unwrap();
        let verifier = store
            .prepare_password("changed-password".into())
            .await
            .unwrap();
        store.change_password(id, verifier).await.unwrap();
        assert!(store.link_browser_discord(proof, discord).await.is_err());
    }

    #[tokio::test]
    async fn reward_failure_rolls_back_link_and_recovery() {
        let content = Content::load(Path::new("data")).unwrap();
        let store = Store::open(Path::new(":memory:"), &content).unwrap();
        let id = store
            .register(
                "rollback-browser".into(),
                "rollback-browser@example.test".into(),
                "test-password".into(),
            )
            .await
            .unwrap();
        store
            .call(move |db| {
                db.execute("UPDATE accounts SET credits=2147483647 WHERE id=?1", [id])?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            store
                .link_browser_discord(
                    store.browser_identity(id).await.unwrap(),
                    "12345678901234567".into()
                )
                .await
                .is_err()
        );
        store
            .call(move |db| {
                for table in ["discord_links", "discord_recovery", "gold_rewards"] {
                    let count: i32 = db.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE account=?1"),
                        [id],
                        |r| r.get(0),
                    )?;
                    assert_eq!(count, 0);
                }
                Ok(())
            })
            .await
            .unwrap();
    }
}
