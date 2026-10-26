use super::*;

#[test]
fn community_schema17_migrates_canonical15_and16_without_recreating_committed_feature_tables() {
    let content = Content::load(Path::new("data")).unwrap();
    for version in [15, 16] {
        for missing in ["bot_minds", "trade_transactions"] {
            let dir =
                std::env::temp_dir().join(format!("community-schema-{}", rand::random::<u64>()));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("game.db");
            drop(Store::open(&path, &content).unwrap());
            let db = Connection::open(&path).unwrap();
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                    .unwrap(),
                19
            );
            if version == 16 {
                db.pragma_update(None, "user_version", 16).unwrap();
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    Store::open(&path, &content)
                        .err()
                        .unwrap()
                        .to_string()
                        .contains("private Fame schema16")
                );
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
            db.execute_batch("DROP TABLE legacy_claims; DROP TABLE level_credits; ALTER TABLE deaths DROP COLUMN receipt;").unwrap();
            if version == 15 {
                db.execute_batch("DROP TABLE bot_minds; DROP TABLE trade_transactions;")
                    .unwrap();
            }
            db.pragma_update(None, "user_version", version).unwrap();
            drop(Store::open(&path, &content).unwrap());
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                    .unwrap(),
                19
            );
            for table in ["bot_minds", "trade_transactions"] {
                assert!(
                    db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                        [table],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
                );
            }
            db.execute_batch(&format!("DROP TABLE {missing}")).unwrap();
            assert!(
                Store::open(&path, &content)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("schema-16")
            );
        }
    }
}

#[test]
fn community_ambiguous_experimental13_and14_require_explicit_preserved_state_migration() {
    let content = Content::load(Path::new("data")).unwrap();
    for version in [13, 14] {
        for table in ["bot_minds", "trade_transactions"] {
            let dir = std::env::temp_dir().join(format!(
                "community-ambiguous-{version}-{table}-{}",
                rand::random::<u64>()
            ));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("game.db");
            let db = Connection::open(&path).unwrap();
            db.execute_batch(&format!(
                "PRAGMA user_version={version}; CREATE TABLE {table}(account INTEGER PRIMARY KEY);"
            ))
            .unwrap();
            let before: String = db
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let error = Store::open(&path, &content).err().unwrap();
            assert!(error.to_string().contains("request an explicit migration"));
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                    .unwrap(),
                version
            );
            assert_eq!(
                db.query_row(
                    "SELECT sql FROM sqlite_schema WHERE name=?1",
                    [table],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                before
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "rejection must not rewrite ambiguous state"
            );
        }
    }
}

async fn fresh_schema_account(content: &Content) -> (std::path::PathBuf, i32) {
    let dir = std::env::temp_dir().join(format!(
        "community-schema-preserved-{}",
        rand::random::<u64>()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("game.db");
    eprintln!("fresh schema fixture: {}", path.display());
    let store = Store::open(&path, content).unwrap();
    let ch = store
        .persona_character(
            "persona:v1:Schema".into(),
            vec![Character::fresh(content.get(crate::content::WIZARD).unwrap()).unwrap()],
            vec![[-1; 8]],
            0,
            None,
        )
        .await
        .unwrap();
    store
        .save_bot_mind(crate::bots::pursuits::Mind::new(ch.account))
        .await
        .unwrap();
    let mut vault = store.vault(ch.account, 0).await.unwrap();
    vault.items[0] = 0xa22;
    store
        .exchange_transaction(
            vec![InventoryWrite {
                id: ch.id,
                account: ch.account,
                revision: ch.revision,
                items: ch.items,
            }],
            vec![vault],
            None,
            Some(("schema-fixture-receipt".into(), None)),
        )
        .await
        .unwrap();
    drop(store);
    let db = Connection::open(&path).unwrap();
    db.execute("UPDATE accounts SET vault_mask=3 WHERE id=?1", [ch.account])
        .unwrap();
    db.execute("INSERT INTO vaults(account,chest,items,revision) VALUES(?1,1,'[2594,-1,-1,-1,-1,-1,-1,-1]',7)", [ch.account]).unwrap();
    (path, ch.account)
}

fn canonical_schema_rows(db: &Connection) -> Vec<String> {
    [
        "SELECT json_group_array(json_array(id,guid,name,credits,character_slots,vault_mask)) FROM accounts",
        "SELECT json_group_array(json_array(id,account,stats,items,revision,alive)) FROM characters",
        "SELECT json_group_array(json_array(account,chest,items,revision)) FROM vaults",
        "SELECT json_group_array(json_array(account,balance,earned)) FROM account_fame",
    ].map(|sql| db.query_row(sql, [], |r| r.get(0)).unwrap()).to_vec()
}

#[tokio::test]
async fn community_schema17_migrates_canonical16_and_committed_reopen_preserves_account_character_vault_mind_and_receipt()
 {
    let content = Content::load(Path::new("data")).unwrap();
    let (path, account) = fresh_schema_account(&content).await;
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE legacy_claims; DROP TABLE level_credits; ALTER TABLE deaths DROP COLUMN receipt; PRAGMA user_version=16;").unwrap();
    let before = canonical_schema_rows(&db);
    let feature_rows = |db: &Connection| -> Vec<String> {
        ["SELECT json_group_array(json_array(account,revision,mind)) FROM bot_minds",
         "SELECT json_group_array(json_array(id,owners,writes,revisions,delivery,market_key)) FROM trade_transactions"]
        .map(|sql| db.query_row(sql, [], |r| r.get(0)).unwrap()).to_vec()
    };
    let features = feature_rows(&db);
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        16
    );
    let reopened = Store::open(&path, &content)
        .expect("committed schema17 must reopen without losing canonical or community state");
    assert_eq!(reopened.vaults(account).await.unwrap().len(), 2);
    assert_eq!(
        reopened.bot_mind(account).await.unwrap().unwrap().revision,
        1
    );
    assert_eq!(canonical_schema_rows(&db), before);
    assert_eq!(feature_rows(&db), features);
    drop(reopened);
    db.execute_batch("PRAGMA user_version=20").unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        Store::open(&path, &content)
            .err()
            .unwrap()
            .to_string()
            .contains("unsupported game database schema 20")
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn community_schema17_upgrades_fresh_canonical13_and14_with_owned_vault_rows_intact() {
    let content = Content::load(Path::new("data")).unwrap();
    for version in [13, 14] {
        let (path, account) = fresh_schema_account(&content).await;
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DROP TABLE legacy_claims; DROP TABLE level_credits; ALTER TABLE deaths DROP COLUMN receipt; DROP TABLE bot_minds; DROP TABLE trade_transactions; DROP TABLE pictures; DROP TABLE picture_sequence;").unwrap();
        // Canonical13 predates equipment awards; both versions predate picture and community tables.
        if version == 13 {
            db.execute_batch("ALTER TABLE deaths DROP COLUMN equipment_fame")
                .unwrap();
        }
        db.pragma_update(None, "user_version", version).unwrap();
        death_awards::check_schema(&db, version).unwrap();
        vaults::migrate(&db, version).unwrap();
        let before = canonical_schema_rows(&db);
        let migrated = Store::open(&path, &content).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            19
        );
        assert_eq!(canonical_schema_rows(&db), before);
        assert_eq!(migrated.vaults(account).await.unwrap().len(), 2);
        assert!(migrated.bot_mind(account).await.unwrap().is_none());
        for table in [
            "bot_minds",
            "trade_transactions",
            "pictures",
            "picture_sequence",
        ] {
            assert!(
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                    [table],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
            );
        }
        drop(migrated);
        drop(Store::open(&path, &content).unwrap());
        assert_eq!(canonical_schema_rows(&db), before);
    }
}
