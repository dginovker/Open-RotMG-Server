use super::*;
use rand::SeedableRng;
use std::path::PathBuf;
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("account-save-{}", rand::random::<u64>()));
    std::fs::create_dir(&root).unwrap();
    root
}
fn dump(db: &Connection) -> Vec<String> {
    let mut result = vec![format!(
        "user_version={}",
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap()
    )];
    let mut schema = db
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
        .unwrap();
    for row in schema
        .query_map([], |r| {
            Ok((0..4)
                .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
    {
        result.push(row.unwrap());
    }
    let mut q = db
        .prepare("SELECT name,sql FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap();
    for row in q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
    {
        let (name, sql) = row.unwrap();
        result.push(sql);
        let mut q = db
            .prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid"))
            .unwrap();
        let n = q.column_count();
        for row in q
            .query_map([], |r| {
                Ok((0..n)
                    .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                    .collect::<Vec<_>>()
                    .join("|"))
            })
            .unwrap()
        {
            result.push(format!("{name}:{}", row.unwrap()));
        }
    }
    result
}
async fn fixture() -> (PathBuf, Content, Store, AccountSave) {
    let root = root();
    let source = root.join("source");
    crate::backup::run(&[
        "--backup-fixture".into(),
        source.to_str().unwrap().into(),
        "data".into(),
    ])
    .await
    .unwrap();
    let content = Content::load(Path::new("data")).unwrap();
    let store = Store::open(&source.join("game.db"), &content).unwrap();
    let deleted = store.create(2, content.get(WIZARD).unwrap()).await.unwrap();
    store.delete_character(2, deleted.id).await.unwrap();
    let mut living = store.create(2, content.get(WIZARD).unwrap()).await.unwrap();
    living.lifetime.level_assists = 7;
    let generator = content
        .objects
        .values()
        .find(|o| o.companion.as_ref().is_some_and(|c| c.permanent))
        .unwrap();
    living.pet_kind = Some(
        content
            .companion_kind(&generator.companion.as_ref().unwrap().object)
            .unwrap(),
    );
    living.xp_boost.active = true;
    living.xp_boost.bonus_exp = 0;
    living.tex1 = 0x01ff0000;
    living.tex2 = 0x04000006;
    living
        .gain_experience(&content, 50, &mut rand::rngs::StdRng::seed_from_u64(37))
        .unwrap();
    store
        .progress_with_assists(
            living.clone(),
            vec![LevelAssist {
                reached_level: 2,
                giver: 1,
            }],
        )
        .await
        .unwrap();
    let mut peer = store.load(1, 1).await.unwrap();
    peer.gain_experience(&content, 500, &mut rand::rngs::StdRng::seed_from_u64(38))
        .unwrap();
    store
        .progress_with_assists(
            peer,
            vec![LevelAssist {
                reached_level: 5,
                giver: living.id,
            }],
        )
        .await
        .unwrap();
    store.call(|db| {
        db.execute("INSERT INTO discord_realm_close_tokens VALUES('PRIVATE_REQUEST_CANARY',2,'PRIVATE_DISCORD_CANARY','PRIVATE_ACTOR_CANARY',NULL)",[])?;
        db.execute("INSERT INTO discord_realm_close_tokens VALUES('PRIVATE_SPENT_CANARY',2,'PRIVATE_DISCORD_CANARY','PRIVATE_ACTOR_CANARY','PRIVATE_USED_CANARY')",[])?;
        db.execute("INSERT INTO email_send_attempts VALUES(2,'PRIVATE_IP_CANARY',123)",[])?;
        Ok(())
    }).await.unwrap();
    let save = store.export_account_save(2).await.unwrap();
    (root, content, store, save)
}
#[tokio::test]
async fn account_save_complete_linked_human_roundtrip_privacy_auth_and_future_earnings() {
    let (root, content, source, _save) = fixture().await;
    let source_db = Connection::open(root.join("source/game.db")).unwrap();
    let before = dump(&source_db);
    let bytes = serde_json::to_vec(&source.export_account_save(2).await.unwrap()).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    for canary in [
        "PRIVATE_",
        "BACKUP_FIXTURE_SECRET",
        "backup-fixture-password",
        "password_hash",
        "discord_id",
        "actor_id",
        "used_request",
        "guild_members",
        "bot_minds",
        "trade_transactions",
    ] {
        assert!(!text.contains(canary), "privacy canary {canary}");
    }
    assert_eq!(dump(&source_db), before);
    let save = AccountSave::decode(&bytes).unwrap();
    assert!(save.guild_fame > 0);
    assert_eq!((save.realm_unused, save.realm_spent), (1, 1));
    assert_eq!(save.characters.len(), 4);
    assert_eq!(save.deaths.len(), 2);
    assert!(!save.claims.is_empty());
    assert_eq!(save.pictures.len(), 1);
    let dest = root.join("local.db");
    let local = Store::open(&dest, &content).unwrap();
    let unrelated = local
        .register(
            "unrelated".into(),
            "unrelated@example.test".into(),
            "other-password".into(),
        )
        .await
        .unwrap();
    let peer = local
        .create(unrelated, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    local
        .save_picture(
            unrelated,
            "Unrelated".into(),
            2,
            "".into(),
            save.pictures[0].png.clone(),
            false,
        )
        .await
        .unwrap();
    local
        .save_picture(
            unrelated,
            "Unrelated Two".into(),
            2,
            "".into(),
            save.pictures[0].png.clone(),
            false,
        )
        .await
        .unwrap();
    let result = local
        .import_account_save(save.clone(), "new-local-password".into(), false)
        .await
        .unwrap();
    assert_eq!(result.account, 2);
    assert_eq!(
        local
            .authenticate(result.login.clone(), "new-local-password".into())
            .await
            .unwrap(),
        result.account
    );
    assert!(
        local
            .authenticate(result.login.clone(), "backup-fixture-password".into())
            .await
            .is_err()
    );
    assert!(
        local
            .authenticate(
                "backup-false@example.invalid".into(),
                "backup-fixture-password".into()
            )
            .await
            .is_err()
    );
    assert!(
        local
            .authenticate("BACKUP_FIXTURE_SECRET_false".into(), "".into())
            .await
            .is_err()
    );
    assert_eq!(local.load(unrelated, peer.id).await.unwrap(), peer);
    let carried = local.export_account_save(result.account).await.unwrap();
    fn normalized(mut saved: AccountSave) -> serde_json::Value {
        saved.exported_at = 1;
        let ids: BTreeMap<_, _> = saved
            .characters
            .iter()
            .enumerate()
            .map(|(i, c)| (c.character.id, i as i32 + 1))
            .collect();
        for row in &mut saved.characters {
            row.character.id = ids[&row.character.id];
        }
        for death in &mut saved.deaths {
            death.character.id = ids[&death.character.id];
        }
        for (_, id) in &mut saved.claims {
            *id = ids[id];
        }
        for (i, picture) in saved.pictures.iter_mut().enumerate() {
            picture.id = i as i32 + 1;
        }
        serde_json::to_value(saved).unwrap()
    }
    assert_eq!(normalized(carried.clone()), normalized(save.clone()));
    let source_unlocks = source.list_id(2).await.unwrap().best_levels;
    let local_unlocks = local.list_id(result.account).await.unwrap().best_levels;
    for kind in content.classes.keys() {
        assert_eq!(
            content.unlocked(*kind, &source_unlocks).unwrap(),
            content.unlocked(*kind, &local_unlocks).unwrap()
        );
    }
    let purchased = source.export_account_save(1).await.unwrap();
    let purchased_local = Store::open(Path::new(":memory:"), &content).unwrap();
    let purchased_result = purchased_local
        .import_account_save(purchased.clone(), "purchased-password".into(), false)
        .await
        .unwrap();
    assert_eq!(
        normalized(
            purchased_local
                .export_account_save(purchased_result.account)
                .await
                .unwrap()
        ),
        normalized(purchased)
    );
    assert_eq!(carried.origin.namespace, save.origin.namespace);
    assert_eq!(carried.fame_earned, save.fame_earned);
    assert_eq!(carried.realm_spent, 1);
    assert_ne!(carried.pictures[0].id, save.pictures[0].id);
    assert_eq!(carried.pictures[0].png, save.pictures[0].png);
    assert_eq!(
        local.picture(carried.pictures[0].id).await.unwrap(),
        save.pictures[0].png
    );
    assert_eq!(
        carried
            .deaths
            .iter()
            .map(|d| d.receipt.total)
            .collect::<Vec<_>>(),
        save.deaths
            .iter()
            .map(|d| d.receipt.total)
            .collect::<Vec<_>>()
    );
    assert_eq!(carried.claims.len(), save.claims.len());
    let mut living = local.list_id(result.account).await.unwrap().characters[0].clone();
    assert_eq!(local.assist_count(living.id).await.unwrap(), 7);
    let mut peer = local.load(unrelated, peer.id).await.unwrap();
    peer.gain_experience(&content, 50, &mut rand::rngs::StdRng::seed_from_u64(39))
        .unwrap();
    let assist = LevelAssist {
        reached_level: 2,
        giver: living.id,
    };
    local
        .progress_with_assists(peer.clone(), vec![assist])
        .await
        .unwrap();
    local
        .progress_with_assists(peer, vec![assist])
        .await
        .unwrap();
    assert_eq!(local.assist_count(living.id).await.unwrap(), 8);
    living.alive = false;
    living.hp = 0;
    local
        .death(living.clone(), "Local after import".into())
        .await
        .unwrap();
    assert!(local.death(living.clone(), "Replay".into()).await.is_err());
    let after = local.export_account_save(result.account).await.unwrap();
    assert_eq!(
        after
            .deaths
            .last()
            .unwrap()
            .character
            .lifetime
            .level_assists,
        8
    );
    assert_eq!(after.claims, carried.claims);
    assert!(
        !after
            .deaths
            .last()
            .unwrap()
            .receipt
            .rows
            .iter()
            .any(|r| r.kind == death_awards::BonusKind::LegacyBuilder)
    );
    let gold = local.presentation(result.account).await.unwrap().credits;
    local.reward_first_bug(result.account).await.unwrap();
    let rewarded = local.presentation(result.account).await.unwrap().credits;
    assert_eq!(rewarded, gold + 50);
    local.reward_first_bug(result.account).await.unwrap();
    assert_eq!(
        local.presentation(result.account).await.unwrap().credits,
        rewarded
    );
    drop(local);
    let reopened = Store::open(&dest, &content).unwrap();
    assert_eq!(
        reopened
            .authenticate(result.login, "new-local-password".into())
            .await
            .unwrap(),
        2
    );
    drop(reopened);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn account_save_unknown_future_corrupt_collision_and_rollback() {
    let (root, content, source, save) = fixture().await;
    let local = Store::open(&root.join("local.db"), &content).unwrap();
    let account = local
        .register(
            "collision".into(),
            "backup-false@example.invalid".into(),
            "collision-password".into(),
        )
        .await
        .unwrap();
    let db = Connection::open(root.join("local.db")).unwrap();
    let before = dump(&db);
    assert!(
        local
            .import_account_save(save.clone(), "new-password".into(), false)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    local
        .call(move |db| {
            db.execute(
                "UPDATE credentials SET email='other@example.test' WHERE account=?1",
                [account],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let mut invalid = save.clone();
    invalid.fame_earned += 1;
    let before = dump(&db);
    assert!(
        local
            .import_account_save(invalid, "new-password".into(), false)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    let imported = local
        .import_account_save(save.clone(), "new-password".into(), false)
        .await
        .unwrap();
    let before = dump(&db);
    assert!(
        local
            .import_account_save(save.clone(), "new-password".into(), false)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    let mut wrong = save.clone();
    wrong.origin.namespace = "0".repeat(64);
    assert!(
        local
            .import_account_save(wrong, "new-password".into(), true)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    for path in ["outer", "character", "lifetime", "receipt"] {
        let mut value = serde_json::to_value(&save).unwrap();
        let target = match path {
            "outer" => &mut value,
            "character" => &mut value["characters"][0]["character"],
            "lifetime" => &mut value["characters"][0]["character"]["lifetime"],
            _ => &mut value["deaths"][0]["receipt"],
        };
        target["PRIVATE_UNKNOWN_CANARY"] = true.into();
        assert!(AccountSave::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut future = save.clone();
    future.format = 2;
    assert!(AccountSave::decode(&serde_json::to_vec(&future).unwrap()).is_err());
    assert!(AccountSave::decode(b"{").is_err());
    assert!(AccountSave::decode(&vec![0; MAX_BYTES as usize + 1]).is_err());
    let mut art = save.clone();
    art.pictures[0].png[20] ^= 1;
    assert!(
        local
            .import_account_save(art, "new-password".into(), true)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    let mut texture = save.clone();
    texture.characters[0].character.tex1 = 0xff000001;
    assert!(texture.validate().is_err());
    let mut duplicate = save.clone();
    duplicate.characters.push(duplicate.characters[0].clone());
    assert!(duplicate.validate().is_err());
    local
        .import_account_save(save, "replacement-password".into(), true)
        .await
        .unwrap();
    assert!(
        local
            .authenticate(imported.login.clone(), "new-password".into())
            .await
            .is_err()
    );
    assert!(
        local
            .authenticate(imported.login, "replacement-password".into())
            .await
            .is_ok()
    );
    drop(source);
    drop(local);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_replacement_peer_edges_consumed_levels_and_shared_history_refusals() {
    let (root, content, source, save) = fixture().await;
    let local = Store::open(&root.join("local.db"), &content).unwrap();
    let peer_account = local.list("local-peer".into()).await.unwrap().id;
    let mut peer = local
        .create(peer_account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let imported = local
        .import_account_save(save.clone(), "new-password".into(), false)
        .await
        .unwrap();
    let mut living = local.list_id(imported.account).await.unwrap().characters[0].clone();
    peer.gain_experience(&content, 50, &mut rand::rngs::StdRng::seed_from_u64(40))
        .unwrap();
    local
        .progress_with_assists(
            peer.clone(),
            vec![LevelAssist {
                reached_level: 2,
                giver: living.id,
            }],
        )
        .await
        .unwrap();
    living
        .gain_experience(&content, 150, &mut rand::rngs::StdRng::seed_from_u64(41))
        .unwrap();
    local
        .progress_with_assists(
            living.clone(),
            vec![LevelAssist {
                reached_level: 3,
                giver: peer.id,
            }],
        )
        .await
        .unwrap();
    assert_eq!(local.assist_count(peer.id).await.unwrap(), 1);
    for _ in 0..2 {
        local
            .import_account_save(save.clone(), "replacement-password".into(), true)
            .await
            .unwrap();
        assert_eq!(local.assist_count(peer.id).await.unwrap(), 1);
    }
    let new_living = local.list_id(imported.account).await.unwrap().characters[0].clone();
    assert!(
        local
            .progress_with_assists(
                peer.clone(),
                vec![LevelAssist {
                    reached_level: 2,
                    giver: new_living.id
                }]
            )
            .await
            .is_err()
    );
    peer.gain_experience(&content, 150, &mut rand::rngs::StdRng::seed_from_u64(42))
        .unwrap();
    local
        .progress_with_assists(
            peer,
            vec![LevelAssist {
                reached_level: 3,
                giver: new_living.id,
            }],
        )
        .await
        .unwrap();
    assert_eq!(local.assist_count(new_living.id).await.unwrap(), 8);
    let db = Connection::open(root.join("local.db")).unwrap();
    local.call(move |db|{
        db.execute("INSERT INTO discord_realm_close_tokens VALUES('new-local-entitlement',?1,'local-discord','local-actor',NULL)",[imported.account])?;Ok(())
    }).await.unwrap();
    let before = dump(&db);
    assert!(
        local
            .import_account_save(save.clone(), "replacement-password".into(), true)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    local
        .call(|db| {
            db.execute("DELETE FROM discord_realm_close_tokens", [])?;
            Ok(())
        })
        .await
        .unwrap();
    local
        .change_guild(imported.account, GuildAction::Create("Local Guild".into()))
        .await
        .unwrap();
    let before = dump(&db);
    assert!(
        local
            .import_account_save(save.clone(), "replacement-password".into(), true)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    // A former member can leave an aggregate shared liability even after membership changes.
    local
        .call(move |db| {
            db.execute(
                "UPDATE guild_members SET account=?2 WHERE account=?1",
                params![imported.account, peer_account],
            )?;
            db.execute("UPDATE guilds SET earned=?1", [save.fame_earned])?;
            Ok(())
        })
        .await
        .unwrap();
    let mut older = local.export_account_save(imported.account).await.unwrap();
    older.fame_earned = 0;
    older.fame_balance = 0;
    older.deaths.clear();
    older.claims.clear();
    older.characters.retain(|c| c.character.alive);
    let before = dump(&db);
    let error = local
        .import_account_save(older, "replacement-password".into(), true)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("shared history"));
    assert_eq!(dump(&db), before);
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_full_snapshot_restore_and_failed_backup_leave_destination_unchanged() {
    let (root, content, source, save) = fixture().await;
    let path = root.join("source/game.db");
    let before = dump(&Connection::open(&path).unwrap());
    let snapshot = root.join("full.db");
    crate::account_save::snapshot(&path, &snapshot, &content).unwrap();
    assert_eq!(dump(&Connection::open(&snapshot).unwrap()), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(crate::account_save::snapshot(&path, &snapshot, &content).is_err());
    assert_eq!(dump(&Connection::open(&path).unwrap()), before);
    assert!(
        crate::account_save::snapshot(&path, &root.join("missing/backup.db"), &content).is_err()
    );
    assert_eq!(dump(&Connection::open(&path).unwrap()), before);
    let dest = root.join("destination");
    std::fs::create_dir(&dest).unwrap();
    let local = Store::open(&dest.join("game.db"), &content).unwrap();
    local
        .import_account_save(save, "new-password".into(), false)
        .await
        .unwrap();
    let backup = root.join("pre-restore.db");
    let local_before = dump(&Connection::open(dest.join("game.db")).unwrap());
    crate::account_save::run(&[
        "--local-save".into(),
        "restore".into(),
        dest.to_str().unwrap().into(),
        "data".into(),
        snapshot.to_str().unwrap().into(),
        backup.to_str().unwrap().into(),
    ])
    .await
    .unwrap();
    assert_eq!(dump(&Connection::open(&backup).unwrap()), local_before);
    assert_eq!(
        dump(&Connection::open(dest.join("game.db")).unwrap()),
        before
    );
    assert_eq!(
        local
            .authenticate(
                "backup-false@example.invalid".into(),
                "backup-fixture-password".into()
            )
            .await
            .unwrap(),
        2
    );
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_unnamed_name_semantics_and_unrelated_display_collision() {
    let root = root();
    let content = Content::load(Path::new("data")).unwrap();
    let source = Store::open(&root.join("source.db"), &content).unwrap();
    let id = source
        .register(
            "unnamed".into(),
            "unnamed@example.test".into(),
            "source-password".into(),
        )
        .await
        .unwrap();
    source
        .create(id, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let save = source.export_account_save(id).await.unwrap();
    assert!(!save.name_chosen);
    let local = Store::open(&root.join("dest.db"), &content).unwrap();
    local.list("unrelated-guest".into()).await.unwrap();
    let result = local
        .import_account_save(save.clone(), "new-password".into(), false)
        .await
        .unwrap();
    let actual = local.export_account_save(result.account).await.unwrap();
    assert!(!actual.name_chosen);
    assert_eq!(actual.name, "Guest2");
    assert_eq!(actual.characters[0].character.name, "Guest2");
    let mut collision = save;
    collision.origin.namespace = "0".repeat(64);
    collision.name_chosen = true;
    collision.name = "Guest2".into();
    let db = Connection::open(root.join("dest.db")).unwrap();
    let before = dump(&db);
    assert!(
        local
            .import_account_save(collision, "new-password".into(), false)
            .await
            .is_err()
    );
    assert_eq!(dump(&db), before);
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_process_cuts_are_atomic_for_import_and_replacement() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    fn gate(root: &Path) -> ! {
        let mut ready = std::fs::File::create(root.join("ready")).unwrap();
        ready.write_all(b"ready").unwrap();
        ready.sync_all().unwrap();
        loop {
            std::thread::park();
        }
    }
    let content = Content::load(Path::new("data")).unwrap();
    if let Ok(directory) = std::env::var("ACCOUNT_SAVE_CUT_ROOT") {
        let directory = PathBuf::from(directory);
        let store = Store::open(&directory.join("game.db"), &content).unwrap();
        let cut = std::env::var("ACCOUNT_SAVE_CUT").unwrap();
        let boundary = directory.clone();
        store
            .call(move |db| {
                match cut.as_str() {
                    "first_write" => db.update_hook(Some(
                        move |_: rusqlite::hooks::Action, _: &str, _: &str, _: i64| gate(&boundary),
                    )),
                    "before_commit" => db.commit_hook(Some(move || gate(&boundary))),
                    "committed" => db.wal_hook(Some(|_, _| {
                        gate(Path::new(&std::env::var("ACCOUNT_SAVE_CUT_ROOT").unwrap()))
                    })),
                    _ => panic!("unknown cut"),
                }
                Ok(())
            })
            .await
            .unwrap();
        let save =
            AccountSave::decode(&std::fs::read(directory.join("save.json")).unwrap()).unwrap();
        store
            .import_account_save(
                save,
                "cut-password".into(),
                std::env::var("ACCOUNT_SAVE_MODE").unwrap() == "replace",
            )
            .await
            .unwrap();
        panic!("cut not reached");
    }
    let (root, content, source, save) = fixture().await;
    for mode in ["import", "replace"] {
        let baseline = root.join(format!("baseline-{mode}.db"));
        let local = Store::open(&baseline, &content).unwrap();
        local.list("unrelated".into()).await.unwrap();
        if mode == "replace" {
            local
                .import_account_save(save.clone(), "before-password".into(), false)
                .await
                .unwrap();
        }
        let before = dump(&Connection::open(&baseline).unwrap());
        for cut in ["first_write", "before_commit", "committed"] {
            let directory = root.join(format!("{mode}-{cut}"));
            std::fs::create_dir(&directory).unwrap();
            crate::account_save::snapshot(&baseline, &directory.join("game.db"), &content).unwrap();
            std::fs::write(
                directory.join("save.json"),
                serde_json::to_vec(&save).unwrap(),
            )
            .unwrap();
            let log = std::fs::File::create(directory.join("child.log")).unwrap();
            let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","store::account_save::tests::account_save_process_cuts_are_atomic_for_import_and_replacement","--nocapture"]).env("ACCOUNT_SAVE_CUT_ROOT",&directory).env("ACCOUNT_SAVE_CUT",cut).env("ACCOUNT_SAVE_MODE",mode).stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log)).spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(60);
            while !directory.join("ready").exists() {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "child exited {status}: {}",
                        std::fs::read_to_string(directory.join("child.log")).unwrap()
                    );
                }
                if Instant::now() > deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("cut deadline");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            let reopened = Store::open(&directory.join("game.db"), &content).unwrap();
            let db = Connection::open(directory.join("game.db")).unwrap();
            verify_database(&db, &content).unwrap();
            if cut != "committed" {
                assert_eq!(dump(&db), before, "{mode}/{cut}");
            } else {
                assert_eq!(
                    reopened
                        .authenticate("backup-false@example.invalid".into(), "cut-password".into())
                        .await
                        .unwrap(),
                    2
                );
                let actual = reopened.export_account_save(2).await.unwrap();
                assert_eq!(
                    (
                        actual.characters.len(),
                        actual.deaths.len(),
                        actual.realm_unused
                    ),
                    (save.characters.len(), save.deaths.len(), save.realm_unused)
                );
            }
            drop(reopened);
        }
        drop(local);
    }
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_schema21_snapshot_precedes_migration_and_preserves_old_credentials() {
    let root = root();
    let content = Content::load(Path::new("data")).unwrap();
    let path = root.join("game.db");
    let store = Store::open(&path, &content).unwrap();
    let account = store
        .register(
            "old-local".into(),
            "old@local.invalid".into(),
            "old-local-password".into(),
        )
        .await
        .unwrap();
    store
        .create(account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    drop(store);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE portable_history; DROP TABLE portable_consumed_levels; DROP TABLE portable_assists; DROP TABLE account_origins; DROP TABLE save_namespace; DROP TABLE discord_realm_close_tokens; ALTER TABLE collections DROP COLUMN source; PRAGMA user_version=21;").unwrap();
    let before = dump(&db);
    let backup = root.join("before-migration.db");
    crate::account_save::snapshot(&path, &backup, &content).unwrap();
    assert_eq!(dump(&db), before);
    assert_eq!(dump(&Connection::open(&backup).unwrap()), before);
    let migrated = Store::open(&path, &content).unwrap();
    assert_eq!(
        migrated
            .authenticate("old@local.invalid".into(), "old-local-password".into())
            .await
            .unwrap(),
        account
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        23
    );
    assert_eq!(
        Connection::open(&backup)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        21
    );
    drop(migrated);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_full_restore_process_cuts_preserve_old_or_complete_snapshot() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    if let Ok(directory) = std::env::var("ACCOUNT_SAVE_RESTORE_CUT_ROOT") {
        let args = vec![
            "--local-save".into(),
            "restore".into(),
            directory.clone(),
            "data".into(),
            format!("{directory}/replacement.db"),
            format!("{directory}/pre-restore.db"),
        ];
        crate::account_save::run(&args).await.unwrap();
        panic!("restore cut not reached");
    }
    let (root, content, source, _) = fixture().await;
    let original = root.join("original.db");
    let local = Store::open(&original, &content).unwrap();
    local
        .register(
            "original-local".into(),
            "original@local.invalid".into(),
            "original-password".into(),
        )
        .await
        .unwrap();
    let old = dump(&Connection::open(&original).unwrap());
    let new = dump(&Connection::open(root.join("source/game.db")).unwrap());
    for cut in ["before_commit", "committed"] {
        let directory = root.join(cut);
        std::fs::create_dir(&directory).unwrap();
        crate::account_save::snapshot(&original, &directory.join("game.db"), &content).unwrap();
        crate::account_save::snapshot(
            &root.join("source/game.db"),
            &directory.join("replacement.db"),
            &content,
        )
        .unwrap();
        let log = std::fs::File::create(directory.join("child.log")).unwrap();
        let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","store::account_save::tests::account_save_full_restore_process_cuts_preserve_old_or_complete_snapshot","--nocapture"]).env("ACCOUNT_SAVE_RESTORE_CUT_ROOT",&directory).env("ACCOUNT_SAVE_RESTORE_CUT",cut).stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log)).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while !directory.join("ready").exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "restore child exited {status}: {}",
                    std::fs::read_to_string(directory.join("child.log")).unwrap()
                );
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("restore cut deadline");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        let log = std::fs::read_to_string(directory.join("child.log")).unwrap();
        assert!(!log.contains("\"protocol\"") && !log.contains("\"restored\""));
        assert_eq!(
            dump(&Connection::open(directory.join("pre-restore.db")).unwrap()),
            old
        );
        let reopened = Store::open(&directory.join("game.db"), &content).unwrap();
        assert_eq!(
            dump(&Connection::open(directory.join("game.db")).unwrap()),
            if cut == "committed" { &new } else { &old }.clone()
        );
        drop(reopened);
    }
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_snapshot_required_schema_rejected_before_restore_without_repair() {
    let root = root();
    let content = Content::load(Path::new("data")).unwrap();
    let baseline = root.join("baseline.db");
    let source = Store::open(&baseline, &content).unwrap();
    source.list("snapshot-source".into()).await.unwrap();
    let state = root.join("destination");
    std::fs::create_dir(&state).unwrap();
    let target = Store::open(&state.join("game.db"), &content).unwrap();
    target.list("keep-destination".into()).await.unwrap();
    let source_before = dump(&Connection::open(&baseline).unwrap());
    let target_db = Connection::open(state.join("game.db")).unwrap();
    let before = dump(&target_db);
    for version in [21, 22] {
        let mut damages = vec![
            "DROP TRIGGER character_slot_insert",
            "DROP TRIGGER character_slot_update",
            "DROP TRIGGER character_slot_capacity",
            "DROP INDEX living_characters_by_account",
            "ALTER TABLE vaults RENAME COLUMN revision TO invalid_revision",
            "DROP TABLE trade_transactions",
            "DROP TABLE bot_minds",
            "DROP TABLE account_lists",
            "DROP TABLE update_feed",
            "DROP TABLE email_password_resets",
            "INSERT INTO collections(account,item,found_at) VALUES(1,2594,0)",
            "INSERT INTO collections(account,item,found_at) VALUES(1,2594,-1)",
            "INSERT INTO collections(account,item,found_at) VALUES(1,2594,1.5)",
            "INSERT INTO collections(account,item,found_at) VALUES(1,2594,'invalid-time')",
        ];
        if version == 21 {
            damages.push("CREATE TABLE account_origins(ignored TEXT)");
        }
        for (index, damage) in damages.iter().enumerate() {
            let bad = root.join(format!("bad-{version}-{index}.db"));
            crate::account_save::snapshot(&baseline, &bad, &content).unwrap();
            let db = Connection::open(&bad).unwrap();
            if version == 21 {
                db.execute_batch("DROP TABLE portable_history; DROP TABLE portable_consumed_levels; DROP TABLE portable_assists; DROP TABLE account_origins; DROP TABLE save_namespace; ALTER TABLE collections DROP COLUMN source; PRAGMA user_version=21;").unwrap();
            }
            db.execute_batch(damage).unwrap();
            let unchanged = dump(&db);
            drop(db);
            let bytes = std::fs::read(&bad).unwrap();
            let args = vec![
                "--local-save".into(),
                "verify-snapshot".into(),
                bad.to_str().unwrap().into(),
                "data".into(),
            ];
            assert!(
                crate::account_save::run(&args).await.is_err(),
                "accepted {version}: {damage}"
            );
            let prebackup = root.join(format!("must-not-create-{version}-{index}.db"));
            let args = vec![
                "--local-save".into(),
                "restore".into(),
                state.to_str().unwrap().into(),
                "data".into(),
                bad.to_str().unwrap().into(),
                prebackup.to_str().unwrap().into(),
            ];
            assert!(
                crate::account_save::run(&args).await.is_err(),
                "restored {version}: {damage}"
            );
            assert!(!prebackup.exists());
            assert_eq!(dump(&target_db), before);
            assert_eq!(dump(&Connection::open(&bad).unwrap()), unchanged);
            assert_eq!(std::fs::read(&bad).unwrap(), bytes);
        }
    }
    assert_eq!(dump(&Connection::open(&baseline).unwrap()), source_before);
    drop(target);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_migrated_legacy_paid_death_exports_without_lifetime_but_modern_stays_strict()
{
    let root = root();
    let content = Content::load(Path::new("data")).unwrap();
    let path = root.join("source.db");
    let source = Store::open(&path, &content).unwrap();
    let account = source
        .register(
            "legacy-person".into(),
            "legacy@example.test".into(),
            "legacy-password".into(),
        )
        .await
        .unwrap();
    let mut ch = source
        .create(account, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    ch.fame = 19;
    ch.alive = false;
    ch.hp = 0;
    source
        .death(ch.clone(), "Historical death".into())
        .await
        .unwrap();
    drop(source);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE portable_history; DROP TABLE portable_consumed_levels; DROP TABLE portable_assists; DROP TABLE account_origins; DROP TABLE save_namespace; DROP TABLE email_password_resets; DROP TABLE verified_emails; DROP TABLE email_verifications; DROP TABLE email_send_attempts; DROP TABLE legacy_claims; DROP TABLE level_credits; ALTER TABLE deaths DROP COLUMN receipt; UPDATE characters SET stats=json_remove(stats,'$.lifetime'); UPDATE deaths SET snapshot=json_remove(snapshot,'$.lifetime'); UPDATE account_fame SET balance=19,earned=19; ALTER TABLE collections DROP COLUMN source; PRAGMA user_version=16;").unwrap();
    let source = Store::open(&path, &content).unwrap();
    let before = dump(&db);
    let raw: String = db
        .query_row("SELECT snapshot FROM deaths", [], |r| r.get(0))
        .unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap()
            .get("lifetime")
            .is_none()
    );
    let save = source.export_account_save(account).await.unwrap();
    assert_eq!(dump(&db), before);
    assert_eq!(
        save.deaths[0].receipt.policy,
        death_awards::ReceiptPolicy::LegacyPaid
    );
    assert_eq!(save.deaths[0].receipt.total, 19);
    assert_eq!(
        save.deaths[0].character.lifetime.completeness,
        LifetimeCompleteness::KnownOnly
    );
    let local = Store::open(&root.join("local.db"), &content).unwrap();
    local.list("other-local".into()).await.unwrap();
    let result = local
        .import_account_save(save.clone(), "local-password".into(), false)
        .await
        .unwrap();
    let actual = local.export_account_save(result.account).await.unwrap();
    assert_eq!(actual.deaths[0].receipt, save.deaths[0].receipt);
    assert_eq!(
        actual.deaths[0].character.lifetime,
        save.deaths[0].character.lifetime
    );
    assert_eq!((actual.fame_balance, actual.fame_earned), (19, 19));
    db.execute(
        "UPDATE deaths SET receipt=json_set(receipt,'$.policy','equipment_only')",
        [],
    )
    .unwrap();
    let unchanged = dump(&db);
    assert!(source.export_account_save(account).await.is_err());
    assert_eq!(dump(&db), unchanged);
    db.execute("UPDATE deaths SET receipt=json_set(receipt,'$.policy','legacy_paid'),snapshot=json_set(snapshot,'$.unknown_archive_field',1)",[]).unwrap();
    let unchanged = dump(&db);
    assert!(source.export_account_save(account).await.is_err());
    assert_eq!(dump(&db), unchanged);
    drop(local);
    drop(source);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_replacement_retained_trade_participation_refused_but_unrelated_trade_retained()
 {
    let (root, content, source, save) = fixture().await;
    let source_before = dump(&Connection::open(root.join("source/game.db")).unwrap());
    let local = Store::open(&root.join("local.db"), &content).unwrap();
    let mut peers = Vec::new();
    for n in 0..2 {
        let id = local.list(format!("trade-peer-{n}")).await.unwrap().id;
        peers.push(
            local
                .create(id, content.get(WIZARD).unwrap())
                .await
                .unwrap(),
        );
    }
    let imported = local
        .import_account_save(save.clone(), "local-password".into(), false)
        .await
        .unwrap();
    let write = |ch: &Character| InventoryWrite {
        id: ch.id,
        account: ch.account,
        revision: ch.revision,
        items: ch.items.clone(),
    };
    local
        .exchange_trade("a".repeat(32), peers.iter().map(write).collect(), None)
        .await
        .unwrap();
    let db = Connection::open(root.join("local.db")).unwrap();
    let unrelated: String = db
        .query_row("SELECT writes FROM trade_transactions", [], |r| r.get(0))
        .unwrap();
    local
        .import_account_save(save.clone(), "local-password".into(), true)
        .await
        .unwrap();
    assert_eq!(
        db.query_row("SELECT writes FROM trade_transactions", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        unrelated
    );
    let left = local.load(peers[0].account, peers[0].id).await.unwrap();
    let right = local.list_id(imported.account).await.unwrap().characters[0].clone();
    local
        .exchange_trade("b".repeat(32), vec![write(&left), write(&right)], None)
        .await
        .unwrap();
    let before = dump(&db);
    let error = local
        .import_account_save(save, "replace-password".into(), true)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("retained local trade history"));
    assert_eq!(dump(&db), before);
    for raw in rows(
        &db,
        "SELECT writes FROM trade_transactions WHERE ?1>0",
        1,
        |r| r.get::<_, String>(0),
    )
    .unwrap()
    {
        for w in serde_json::from_str::<Vec<InventoryWrite>>(&raw).unwrap() {
            assert!(local.load(w.account, w.id).await.is_ok());
        }
    }
    drop(local);
    assert_eq!(
        dump(&Connection::open(root.join("source/game.db")).unwrap()),
        source_before
    );
    drop(source);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_schema21_preview_readonly_login_and_collision_checks() {
    let (root, content, source, mut save) = fixture().await;
    let source_before = dump(&Connection::open(root.join("source/game.db")).unwrap());
    let state = root.join("local");
    std::fs::create_dir(&state).unwrap();
    let path = state.join("game.db");
    let local = Store::open(&path, &content).unwrap();
    local
        .register(
            "preview-owner".into(),
            "other@local.invalid".into(),
            "old-password".into(),
        )
        .await
        .unwrap();
    drop(local);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE portable_history; DROP TABLE portable_consumed_levels; DROP TABLE portable_assists; DROP TABLE account_origins; DROP TABLE save_namespace; ALTER TABLE collections DROP COLUMN source; PRAGMA user_version=21;").unwrap();
    let file = root.join("save.json");
    std::fs::write(&file, serde_json::to_vec(&save).unwrap()).unwrap();
    let before = dump(&db);
    let args = vec![
        "--local-save".into(),
        "preview".into(),
        state.to_str().unwrap().into(),
        "data".into(),
        file.to_str().unwrap().into(),
    ];
    assert!(crate::account_save::run(&args).await.unwrap());
    assert_eq!(dump(&db), before);
    let plan = preview(&db, &save).unwrap();
    assert_eq!(plan.login, "backup-false@example.invalid");
    assert!(!plan.replaced);
    db.execute("UPDATE credentials SET email='backup-false@example.invalid'", [])
        .unwrap();
    let before = dump(&db);
    assert!(crate::account_save::run(&args).await.is_err());
    assert_eq!(dump(&db), before);
    db.execute("UPDATE credentials SET email='other@local.invalid'", [])
        .unwrap();
    save.name_chosen = true;
    save.name = "Guest1".into();
    std::fs::write(&file, serde_json::to_vec(&save).unwrap()).unwrap();
    let before = dump(&db);
    assert!(crate::account_save::run(&args).await.is_err());
    assert_eq!(dump(&db), before);
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        21
    );
    drop(db);
    assert_eq!(
        dump(&Connection::open(root.join("source/game.db")).unwrap()),
        source_before
    );
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_collection_timestamp_rejected_without_destination_mutation() {
    let (root, content, source, save) = fixture().await;
    let source_before = dump(&Connection::open(root.join("source/game.db")).unwrap());
    let local = Store::open(&root.join("local.db"), &content).unwrap();
    local.list("keep-local".into()).await.unwrap();
    let db = Connection::open(root.join("local.db")).unwrap();
    let before = dump(&db);
    for time in [0, -1] {
        let mut bad = save.clone();
        bad.collections = vec![(0xa22, time)];
        let bytes = serde_json::to_vec(&bad).unwrap();
        assert!(AccountSave::decode(&bytes).is_err());
        assert!(
            local
                .import_account_save(bad, "local-password".into(), false)
                .await
                .is_err()
        );
        assert_eq!(dump(&db), before);
    }
    let mut valid = save;
    valid.collections = vec![(0xa22, 1)];
    valid.collection_sources = vec![(0xa22, "Medusa".into())];
    for source in ["", "  ", "Medusa\n"] {
        let mut bad = valid.clone();
        bad.collection_sources[0].1 = source.into();
        assert!(AccountSave::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let imported = local
        .import_account_save(valid, "local-password".into(), false)
        .await
        .unwrap();
    assert_eq!(
        local
            .export_account_save(imported.account)
            .await
            .unwrap()
            .collections,
        vec![(0xa22, 1)]
    );
    assert_eq!(
        local
            .export_account_save(imported.account)
            .await
            .unwrap()
            .collection_sources,
        vec![(0xa22, "Medusa".into())]
    );
    drop(local);
    assert_eq!(
        dump(&Connection::open(root.join("source/game.db")).unwrap()),
        source_before
    );
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_cli_child() {
    let Ok(args) = std::env::var("ACCOUNT_SAVE_CLI_ARGS") else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&args).unwrap();
    if let Err(error) = crate::account_save::run(&args).await {
        eprintln!("{error:#}");
        std::process::exit(2);
    }
}
fn cli_process(
    args: &[&str],
    password: &[u8],
    env: &[(&str, &str)],
    stdout: std::process::Stdio,
) -> std::process::Output {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::account_save::tests::account_save_cli_child",
            "--nocapture",
        ])
        .env(
            "ACCOUNT_SAVE_CLI_ARGS",
            serde_json::to_string(&[&["--local-save"][..], args].concat()).unwrap(),
        )
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let written = child.stdin.take().unwrap().write_all(password);
    let output = child.wait_with_output().unwrap();
    if let Err(error) = written {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(!output.status.success());
    }
    output
}
fn cli(args: &[&str], password: &[u8]) -> (bool, serde_json::Value, String) {
    cli_env(args, password, &[])
}
fn cli_env(
    args: &[&str],
    password: &[u8],
    env: &[(&str, &str)],
) -> (bool, serde_json::Value, String) {
    let output = cli_process(args, password, env, std::process::Stdio::piped());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    for secret in [
        "private-cli-password",
        "replacement-cli-password",
        "backup-fixture-password",
        "$argon2",
    ] {
        assert!(!stdout.contains(secret) && !stderr.contains(secret));
    }
    let value = stdout
        .lines()
        .filter_map(|line| {
            line.find('{')
                .and_then(|i| serde_json::from_str(&line[i..]).ok())
        })
        .next()
        .unwrap_or(serde_json::Value::Null);
    if !output.status.success() {
        assert!(
            !stdout.contains("PRIVATE_FAILURE_CANARY")
                && !stderr.contains("PRIVATE_FAILURE_CANARY")
        );
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["protocol"], 1);
        let operation = args
            .first()
            .copied()
            .filter(|op| {
                matches!(
                    *op,
                    "import"
                        | "replace"
                        | "restore"
                        | "snapshot"
                        | "export"
                        | "preview"
                        | "verify-account"
                        | "verify-snapshot"
                )
            })
            .unwrap_or("invalid");
        assert_eq!(value["operation"], operation);
        assert!(matches!(
            value["outcome"].as_str(),
            Some("unchanged" | "unknown")
        ));
        let error = value["error"].as_str().unwrap();
        assert!(!error.is_empty() && error.len() <= 512);
    }
    (output.status.success(), value, stderr)
}
fn cli_ok(args: &[&str], password: &[u8]) -> serde_json::Value {
    let (success, value, error) = cli(args, password);
    assert!(success, "{error}");
    assert!(value.is_object());
    value
}
fn write_input(root: &Path, save: &AccountSave) -> PathBuf {
    let file = root.join("incoming.json");
    std::fs::write(&file, serde_json::to_vec(save).unwrap()).unwrap();
    file
}
fn schema21(db: &Connection) {
    db.execute_batch("DROP TABLE portable_history; DROP TABLE portable_consumed_levels; DROP TABLE portable_assists; DROP TABLE account_origins; DROP TABLE save_namespace; ALTER TABLE collections DROP COLUMN source; PRAGMA user_version=21;").unwrap();
}

#[tokio::test]
async fn account_save_cli_preview_metadata_expected_and_snapshot_counts_preserve_credentials() {
    let (root, content, source, save) = fixture().await;
    let source_before = dump(&Connection::open(root.join("source/game.db")).unwrap());
    let file = write_input(&root, &save);
    let state = root.join("local");
    let file = file.to_str().unwrap();
    let state = state.to_str().unwrap();
    let preview = cli_ok(&["preview", state, "data", file], b"");
    assert_eq!(preview.as_object().unwrap().len(), 8);
    assert_eq!(preview["exported_at"], save.exported_at);
    assert_eq!(preview["name"], save.name);
    assert_eq!(preview["character_count"], save.characters.len());
    assert_eq!(preview["login"], "backup-false@example.invalid");
    let expected = preview["expected"].as_str().unwrap();
    assert!(key(expected));
    let digest = format!("{:x}", Sha256::digest(std::fs::read(file).unwrap()));
    let canonical = serde_json::to_vec(&(
        "account-import-v1",
        &digest,
        &save.origin.namespace,
        save.origin.account,
        1,
        "backup-false@example.invalid",
        false,
    ))
    .unwrap();
    assert_eq!(expected, format!("{:x}", Sha256::digest(canonical)));
    assert_eq!(
        cli_ok(&["preview", state, "data", file], b"")["expected"],
        expected
    );
    assert!(!Path::new(state).join("game.db").exists());
    let backup = root.join("first.db");
    let (ok, _, error) = cli(
        &[
            "import",
            state,
            "data",
            file,
            backup.to_str().unwrap(),
            &"0".repeat(64),
        ],
        b"private-cli-password\n",
    );
    assert!(!ok && error.contains("re-preview"));
    assert!(!Path::new(state).join("game.db").exists() && !backup.exists());
    let imported = cli_ok(
        &[
            "import",
            state,
            "data",
            file,
            backup.to_str().unwrap(),
            expected,
        ],
        b"private-cli-password\n",
    );
    assert_eq!(imported["login"], preview["login"]);
    assert!(!backup.exists());
    let replacement = cli_ok(&["preview", state, "data", file], b"");
    assert_eq!(replacement["replaced"], true);
    let local = Store::open(&Path::new(state).join("game.db"), &content).unwrap();
    let peer = local
        .list("unrelated-after-confirmation".into())
        .await
        .unwrap()
        .id;
    let other = local
        .create(peer, content.get(WIZARD).unwrap())
        .await
        .unwrap();
    // Progress/peers are not fingerprinted: the confirmed target identity stays the same.
    assert_eq!(
        cli_ok(&["preview", state, "data", file], b"")["expected"],
        replacement["expected"]
    );
    let before = dump(&Connection::open(Path::new(state).join("game.db")).unwrap());
    let result = cli_ok(
        &[
            "replace",
            state,
            "data",
            file,
            backup.to_str().unwrap(),
            replacement["expected"].as_str().unwrap(),
        ],
        b"replacement-cli-password\n",
    );
    assert_eq!(result["login"], replacement["login"]);
    assert_eq!(dump(&Connection::open(&backup).unwrap()), before);
    let verified = cli_ok(&["verify-snapshot", backup.to_str().unwrap(), "data"], b"");
    assert_eq!(
        verified,
        serde_json::json!({"verified":true,"schema":23,"scope":"all-local-accounts","account_count":2,"character_count":save.characters.len()+1})
    );
    assert_eq!(local.load(peer, other.id).await.unwrap().items, other.items);
    assert_eq!(
        local
            .authenticate(
                "backup-false@example.invalid".into(),
                "replacement-cli-password".into()
            )
            .await
            .unwrap(),
        1
    );
    let historical = Store::open(&backup, &content).unwrap();
    assert_eq!(
        historical
            .authenticate(
                "backup-false@example.invalid".into(),
                "private-cli-password".into()
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        dump(&Connection::open(root.join("source/game.db")).unwrap()),
        source_before
    );
    drop(historical);
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_cli_stale_missing_malformed_and_input_changed_fail_before_backup_or_schema21_migration()
 {
    let (root, content, source, save) = fixture().await;
    let file = write_input(&root, &save);
    let input = std::fs::read(&file).unwrap();
    let state = root.join("local");
    std::fs::create_dir(&state).unwrap();
    let local = Store::open(&state.join("game.db"), &content).unwrap();
    local
        .register(
            "keep-owner".into(),
            "keep@example.invalid".into(),
            "private-cli-password".into(),
        )
        .await
        .unwrap();
    drop(local);
    let db = Connection::open(state.join("game.db")).unwrap();
    schema21(&db);
    let state = state.to_str().unwrap();
    let file = file.to_str().unwrap();
    let backup = root.join("before.db");
    let backup = backup.to_str().unwrap();
    let initial = dump(&db);
    let confirmed = cli_ok(&["preview", state, "data", file], b"");
    assert_eq!(dump(&db), initial);
    assert_eq!(confirmed["login"], "backup-false@example.invalid");
    let expected = confirmed["expected"].as_str().unwrap();
    for token in [
        None,
        Some(String::new()),
        Some("abc".to_string()),
        Some("A".repeat(64)),
        Some("0".repeat(64)),
    ] {
        let mut args = vec!["import", state, "data", file, backup];
        if let Some(token) = token.as_deref() {
            args.push(token)
        }
        let before = dump(&db);
        let (ok, _, error) = cli(&args, b"private-cli-password\n");
        assert!(!ok && error.contains("re-preview"), "{error}");
        assert_eq!(dump(&db), before);
        assert!(!Path::new(backup).exists());
    }
    std::fs::write(file, [input.as_slice(), b"\n"].concat()).unwrap();
    let (ok, _, error) = cli(
        &["import", state, "data", file, backup, expected],
        b"private-cli-password\n",
    );
    assert!(!ok && error.contains("re-preview"));
    assert_eq!(dump(&db), initial);
    assert!(!Path::new(backup).exists());
    std::fs::write(file, &input).unwrap();
    db.execute("UPDATE credentials SET email='backup-false@example.invalid'", [])
        .unwrap();
    let before = dump(&db);
    let (ok, _, error) = cli(
        &["import", state, "data", file, backup, expected],
        b"private-cli-password\n",
    );
    assert!(!ok && error.contains("re-preview"));
    assert_eq!(dump(&db), before);
    assert!(!Path::new(backup).exists());
    db.execute("UPDATE credentials SET email='keep@example.invalid'", [])
        .unwrap();
    db.execute(
        "INSERT INTO accounts(guid,name) VALUES('allocated-drift','Guest2')",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO account_fame VALUES(2,0,0)", [])
        .unwrap();
    vaults::provision(&db, 2).unwrap();
    verify_database(&db, &content).unwrap();
    let before = dump(&db);
    let (ok, _, error) = cli(
        &["import", state, "data", file, backup, expected],
        b"private-cli-password\n",
    );
    assert!(!ok && error.contains("re-preview"));
    assert_eq!(dump(&db), before);
    assert!(!Path::new(backup).exists());
    db.execute("DELETE FROM vaults WHERE account=2", [])
        .unwrap();
    db.execute("DELETE FROM account_fame WHERE account=2", [])
        .unwrap();
    db.execute("DELETE FROM accounts WHERE id=2", []).unwrap();
    assert_eq!(dump(&db), initial);
    assert_eq!(
        cli_ok(&["preview", state, "data", file], b"")["expected"],
        expected
    );
    let result = cli_ok(
        &["import", state, "data", file, backup, expected],
        b"private-cli-password\n",
    );
    assert_eq!(result["login"], confirmed["login"]);
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        23
    );
    let verified = cli_ok(&["verify-snapshot", backup, "data"], b"");
    assert_eq!(
        verified,
        serde_json::json!({"verified":true,"schema":21,"scope":"all-local-accounts","account_count":1,"character_count":0})
    );
    let local = Store::open(&Path::new(state).join("game.db"), &content).unwrap();
    assert_eq!(
        local
            .authenticate("keep@example.invalid".into(), "private-cli-password".into())
            .await
            .unwrap(),
        1
    );
    drop(local);
    drop(db);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_expected_transaction_rechecks_identity_and_cli_origin_replaced_drift_without_writes()
 {
    let (root, content, source, save) = fixture().await;
    let file = write_input(&root, &save);
    let digest = format!("{:x}", Sha256::digest(std::fs::read(&file).unwrap()));
    let state = root.join("local");
    std::fs::create_dir(&state).unwrap();
    let local = Store::open(&state.join("game.db"), &content).unwrap();
    let db = Connection::open(state.join("game.db")).unwrap();
    let mut plan = preview(&db, &save).unwrap();
    let token = expected(&save, &plan, &digest).unwrap();
    local.list("allocated-after-preview".into()).await.unwrap();
    let before = dump(&db);
    assert!(
        local
            .import_expected_account_save(
                save.clone(),
                "private-cli-password".into(),
                false,
                ImportExpectation {
                    input_sha256: digest.clone(),
                    expected: token
                }
            )
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("re-preview")
    );
    assert_eq!(dump(&db), before);
    plan = preview(&db, &save).unwrap();
    for field in ["account", "login", "replaced", "origin"] {
        let mut forged = ImportResult {
            account: plan.account,
            login: plan.login.clone(),
            replaced: plan.replaced,
            disclosure: DISCLOSURE,
        };
        let mut other = save.clone();
        match field {
            "account" => forged.account += 1,
            "login" => forged.login = "another@local.invalid".into(),
            "replaced" => forged.replaced = !forged.replaced,
            "origin" => other.origin.namespace = "f".repeat(64),
            _ => unreachable!(),
        };
        let token = expected(&other, &forged, &digest).unwrap();
        let before = dump(&db);
        assert!(
            local
                .import_expected_account_save(
                    save.clone(),
                    "private-cli-password".into(),
                    false,
                    ImportExpectation {
                        input_sha256: digest.clone(),
                        expected: token
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(dump(&db), before);
    }
    let token = expected(&save, &plan, &digest).unwrap();
    local
        .import_expected_account_save(
            save.clone(),
            "private-cli-password".into(),
            false,
            ImportExpectation {
                input_sha256: digest,
                expected: token,
            },
        )
        .await
        .unwrap();
    let state = state.to_str().unwrap();
    let file = file.to_str().unwrap();
    let confirmed = cli_ok(&["preview", state, "data", file], b"");
    assert_eq!(confirmed["replaced"], true);
    let backup = root.join("refused.db");
    let backup = backup.to_str().unwrap();
    db.execute("INSERT INTO portable_history VALUES(1,0,0,0)", [])
        .unwrap();
    db.execute("UPDATE account_origins SET account=1", [])
        .unwrap();
    verify_database(&db, &content).unwrap();
    for mapping_present in [true, false] {
        if !mapping_present {
            db.execute("DELETE FROM account_origins", []).unwrap();
        }
        let before = dump(&db);
        let (ok, _, error) = cli(
            &[
                "replace",
                state,
                "data",
                file,
                backup,
                confirmed["expected"].as_str().unwrap(),
            ],
            b"replacement-cli-password\n",
        );
        assert!(!ok && error.contains("re-preview"), "{error}");
        assert!(!Path::new(backup).exists());
        assert_eq!(dump(&db), before);
    }
    drop(db);
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_cli_failure_outcome_prewrite_retry_and_rolled_back_postwrite_unknown() {
    let (root, content, source, save) = fixture().await;
    let file = write_input(&root, &save);
    let file = file.to_str().unwrap();
    let state = root.join("local");
    std::fs::create_dir(&state).unwrap();
    let dbpath = state.join("game.db");
    let local = Store::open(&dbpath, &content).unwrap();
    local.list("keep-original".into()).await.unwrap();
    let state = state.to_str().unwrap();
    let backup = root.join("pre-import.db");
    let backup = backup.to_str().unwrap();
    let before = dump(&Connection::open(&dbpath).unwrap());
    let confirmed = cli_ok(&["preview", state, "data", file], b"");
    for failure_args in [
        vec!["import", state, "data", file, backup],
        vec!["import", state, "data", file, backup, &"0".repeat(64)],
    ] {
        let (ok, failure, _) = cli(&failure_args, b"private-cli-password\n");
        assert!(!ok);
        assert_eq!(failure["outcome"], "unchanged");
        assert!(!Path::new(backup).exists());
        assert_eq!(dump(&Connection::open(&dbpath).unwrap()), before);
    }
    let result = cli_ok(
        &[
            "import",
            state,
            "data",
            file,
            backup,
            confirmed["expected"].as_str().unwrap(),
        ],
        b"private-cli-password\n",
    );
    assert_eq!(result["login"], confirmed["login"]);
    let db = Connection::open(&dbpath).unwrap();
    db.execute_batch("CREATE TRIGGER fail_after_character_deletion BEFORE UPDATE ON accounts WHEN OLD.id=2 BEGIN SELECT RAISE(ABORT,'PRIVATE_FAILURE_CANARY'); END;").unwrap();
    let confirmed = cli_ok(&["preview", state, "data", file], b"");
    let before = dump(&db);
    let backup = root.join("pre-replace.db");
    let (ok, failure, _) = cli(
        &[
            "replace",
            state,
            "data",
            file,
            backup.to_str().unwrap(),
            confirmed["expected"].as_str().unwrap(),
        ],
        b"replacement-cli-password\n",
    );
    assert!(!ok);
    assert_eq!(failure["outcome"], "unknown");
    assert!(backup.is_file());
    assert_eq!(dump(&db), before);
    for operation in ["verify-account", "preview"] {
        let bad = root.join("PRIVATE_FAILURE_CANARY.json");
        std::fs::write(
            &bad,
            b"{\"PRIVATE_FAILURE_CANARY\":\"private-cli-password\"}",
        )
        .unwrap();
        let args = if operation == "preview" {
            vec![operation, state, "data", bad.to_str().unwrap()]
        } else {
            vec![operation, bad.to_str().unwrap(), "data"]
        };
        let (ok, failure, _) = cli(&args, b"");
        assert!(!ok);
        assert_eq!(failure["outcome"], "unchanged");
    }
    for args in [vec!["PRIVATE_FAILURE_CANARY"], vec!["restore"]] {
        let (ok, failure, _) = cli(&args, b"");
        assert!(!ok);
        assert_eq!(failure["outcome"], "unchanged");
    }
    drop(db);
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_cli_failure_output_artifacts_are_not_game_mutation_and_restore_is_unknown() {
    let (root, content, source, _) = fixture().await;
    let state = root.join("source");
    let before = dump(&Connection::open(state.join("game.db")).unwrap());
    let snapshot = root.join("published-but-not-acknowledged.db");
    let (ok, failure, _) = cli_env(
        &[
            "snapshot",
            state.to_str().unwrap(),
            "data",
            snapshot.to_str().unwrap(),
        ],
        b"",
        &[("ACCOUNT_SAVE_SNAPSHOT_FAIL_AFTER_PUBLISH", "1")],
    );
    assert!(!ok);
    assert_eq!(failure["outcome"], "unchanged");
    assert!(snapshot.is_file());
    assert_eq!(
        dump(&Connection::open(state.join("game.db")).unwrap()),
        before
    );
    let exported = root.join("written-but-not-acknowledged.json");
    let (ok, failure, _) = cli_env(
        &[
            "export",
            state.to_str().unwrap(),
            "data",
            "2",
            exported.to_str().unwrap(),
        ],
        b"",
        &[("ACCOUNT_SAVE_EXPORT_FAIL_AFTER_WRITE", "1")],
    );
    assert!(!ok);
    assert_eq!(failure["outcome"], "unchanged");
    assert!(exported.is_file());
    assert_eq!(
        dump(&Connection::open(state.join("game.db")).unwrap()),
        before
    );
    let target = root.join("target");
    std::fs::create_dir(&target).unwrap();
    let local = Store::open(&target.join("game.db"), &content).unwrap();
    local.list("different-progress".into()).await.unwrap();
    let target_before = dump(&Connection::open(target.join("game.db")).unwrap());
    let pre = root.join("pre-restore.db");
    let (ok, failure, _) = cli_env(
        &[
            "restore",
            target.to_str().unwrap(),
            "data",
            snapshot.to_str().unwrap(),
            pre.to_str().unwrap(),
        ],
        b"",
        &[("ACCOUNT_SAVE_RESTORE_FAIL_AFTER_COPY", "1")],
    );
    assert!(!ok);
    assert_eq!(failure["outcome"], "unknown");
    assert_eq!(dump(&Connection::open(&pre).unwrap()), target_before);
    assert_eq!(
        dump(&Connection::open(target.join("game.db")).unwrap()),
        before
    );
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_cli_truncated_or_broken_acknowledgement_is_nonzero_not_unchanged() {
    let (root, content, source, save) = fixture().await;
    let file = write_input(&root, &save);
    let state = root.join("unacknowledged");
    let confirmed = cli_ok(
        &[
            "preview",
            state.to_str().unwrap(),
            "data",
            file.to_str().unwrap(),
        ],
        b"",
    );
    let backup = root.join("unused.db");
    let output = cli_process(
        &[
            "import",
            state.to_str().unwrap(),
            "data",
            file.to_str().unwrap(),
            backup.to_str().unwrap(),
            confirmed["expected"].as_str().unwrap(),
        ],
        b"private-cli-password\n",
        &[("ACCOUNT_SAVE_TRUNCATE_ACK", "1")],
        std::process::Stdio::piped(),
    );
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let fragment = &text[text.find('{').unwrap()..];
    assert!(serde_json::from_str::<serde_json::Value>(fragment).is_err());
    assert!(!text.contains("\"unchanged\""));
    let local = Store::open(&state.join("game.db"), &content).unwrap();
    assert_eq!(
        local
            .authenticate(
                "backup-false@example.invalid".into(),
                "private-cli-password".into()
            )
            .await
            .unwrap(),
        1
    );
    #[cfg(target_os = "linux")]
    {
        let device = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let output = cli_process(
            &["verify-account", file.to_str().unwrap(), "data"],
            b"",
            &[],
            std::process::Stdio::from(device),
        );
        assert!(!output.status.success());
        assert!(
            !String::from_utf8(output.stderr)
                .unwrap()
                .contains("PRIVATE_FAILURE_CANARY")
        );
    }
    drop(local);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn account_save_import_uses_exported_email_and_refuses_missing_email_without_changes() {
    let (root, content, source, save) = fixture().await;
    assert_eq!(save.email.as_deref(), Some("backup-false@example.invalid"));
    let local = Store::open(&root.join("email.db"), &content).unwrap();
    let db = Connection::open(root.join("email.db")).unwrap();
    let before = dump(&db);
    let mut legacy = save.clone(); legacy.email = None;
    assert!(preview(&db, &legacy).err().unwrap().to_string().contains("no account email"));
    assert_eq!(dump(&db), before);
    let imported = local.import_account_save(save, "chosen-password".into(), false).await.unwrap();
    assert_eq!(imported.login, "backup-false@example.invalid");
    assert_eq!(local.authenticate(imported.login, "chosen-password".into()).await.unwrap(), imported.account);
    drop(db);drop(local);drop(source);std::fs::remove_dir_all(root).unwrap();
}
