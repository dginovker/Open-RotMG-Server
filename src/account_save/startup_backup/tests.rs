use super::*;
use rusqlite::types::Value;
use std::path::PathBuf;
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("startup-backup-{}", rand::random::<u64>()));
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("state")).unwrap();
    std::fs::create_dir(root.join("backups")).unwrap();
    root
}
fn dump(path: &Path) -> Vec<(String, Vec<Vec<Value>>)> {
    let db = readonly(path).unwrap();
    let tables = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    std::iter::once("sqlite_schema".to_string())
        .chain(tables)
        .map(|name| {
            let mut q = db
                .prepare(&format!("SELECT * FROM {name} ORDER BY rowid"))
                .unwrap();
            let n = q.column_count();
            let rows = q
                .query_map([], |r| {
                    (0..n)
                        .map(|i| r.get(i))
                        .collect::<rusqlite::Result<Vec<Value>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}
fn schema21(path: &Path) {
    Connection::open(path).unwrap().execute_batch("DROP TABLE portable_history; DROP TABLE portable_consumed_levels; DROP TABLE portable_assists; DROP TABLE account_origins; DROP TABLE save_namespace; PRAGMA user_version=21;").unwrap();
}
fn ack(path: &Path) -> serde_json::Value {
    let bytes = std::fs::read(format!("{}.ack.json", path.display())).unwrap();
    assert!(bytes.len() <= 16 * 1024);
    let a: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(a.as_object().unwrap().len(), 8);
    assert_eq!(a["protocol"], 1);
    assert_eq!(a["operation"], "pre-migration");
    assert_eq!(a["process"], std::process::id());
    assert_eq!(a["backup"], path.to_str().unwrap());
    assert_eq!(a["target_schema"], 22);
    a
}
#[tokio::test]
async fn startup_backup_schema21_and_22_are_verified_unconditional_and_keep_credentials() {
    let content = Content::load(Path::new("data")).unwrap();
    for version in [21, 22] {
        let root = root();
        let state = root.join("state");
        let dbpath = state.join("game.db");
        let store = Store::open(&dbpath, &content).unwrap();
        let id = store
            .register(
                "startup-owner".into(),
                "startup@example.invalid".into(),
                "private-startup-password".into(),
            )
            .await
            .unwrap();
        store
            .create(id, content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        drop(store);
        if version == 21 {
            schema21(&dbpath);
        }
        let before = dump(&dbpath);
        let backup = root.join("backups/first.db");
        let lock = StateLock::acquire(&state).unwrap();
        pre_migration_backup(&state, &backup, &content, &lock).unwrap();
        let a = ack(&backup);
        assert_eq!(a["outcome"], "verified-backup");
        assert_eq!(a["source_schema"], version);
        assert_eq!(a["sha256"], hash_file(&backup).unwrap());
        assert_eq!(dump(&dbpath), before);
        assert_eq!(dump(&backup), before);
        let store = Store::open(&dbpath, &content).unwrap();
        assert_eq!(
            store
                .authenticate(
                    "startup@example.invalid".into(),
                    "private-startup-password".into()
                )
                .await
                .unwrap(),
            id
        );
        assert_eq!(
            readonly(&dbpath)
                .unwrap()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            22
        );
        let next = root.join("backups/next.db");
        pre_migration_backup(&state, &next, &content, &lock).unwrap();
        assert_eq!(ack(&next)["source_schema"], 22);
        assert!(StateLock::acquire(&state).is_err());
        drop(store);
        drop(lock);
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn startup_backup_genuine_absence_and_resolved_outside_state_boundary() {
    let root = root();
    let state = root.join("state");
    let lock = StateLock::acquire(&state).unwrap();
    let content = Content::load(Path::new("data")).unwrap();
    let backup = root.join("backups/fresh.db");
    pre_migration_backup(&state, &backup, &content, &lock).unwrap();
    let a = ack(&backup);
    assert_eq!(a["outcome"], "no-game-db");
    assert!(a["sha256"].is_null() && a["source_schema"].is_null());
    assert!(!backup.exists() && !state.join("game.db").exists());
    for bad in [
        state.clone(),
        state.join("inside.db"),
        state.join("nested/inside.db"),
    ] {
        assert!(pre_migration_backup(&state, &bad, &content, &lock).is_err());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&state, root.join("alias")).unwrap();
        assert!(
            pre_migration_backup(&state, &root.join("alias/inside.db"), &content, &lock).is_err()
        );
    }
    #[cfg(windows)]
    {
        assert!(same_directory(Path::new(r"C:\State"), Path::new(r"c:\STATE")).unwrap());
    }
    drop(lock);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn startup_backup_refuses_bad_sources_or_existing_destinations_without_creating_ack() {
    let content = Content::load(Path::new("data")).unwrap();
    for damage in [
        "corrupt",
        "unsupported",
        "foreign",
        "orphan",
        "source-symlink",
        "backup-existing",
        "ack-existing",
        "backup-symlink",
        "ack-symlink",
    ] {
        if !cfg!(unix) && damage.contains("symlink") {
            continue;
        }
        let root = root();
        let state = root.join("state");
        let path = state.join("game.db");
        let backup = root.join("backups/file.db");
        let ack = PathBuf::from(format!("{}.ack.json", backup.display()));
        if damage == "orphan" {
            std::fs::write(state.join("game.db-wal"), b"uncheckpointed-progress").unwrap();
        } else {
            let baseline = root.join("baseline.db");
            let store = Store::open(&baseline, &content).unwrap();
            snapshot(&baseline, &path, &content).unwrap();
            drop(store);
            match damage {
                "corrupt" => std::fs::write(&path, b"not SQLite").unwrap(),
                "unsupported" => Connection::open(&path)
                    .unwrap()
                    .execute_batch("PRAGMA user_version=23;")
                    .unwrap(),
                "foreign" => {
                    std::fs::remove_file(&path).unwrap();
                    Connection::open(&path)
                        .unwrap()
                        .execute_batch("CREATE TABLE foreign_data(x); PRAGMA user_version=22;")
                        .unwrap();
                }
                "backup-existing" => std::fs::write(&backup, b"held-backup").unwrap(),
                "ack-existing" => std::fs::write(&ack, b"held-ack").unwrap(),
                #[cfg(unix)]
                "source-symlink" => {
                    std::fs::rename(&path, state.join("held.db")).unwrap();
                    std::os::unix::fs::symlink(state.join("held.db"), &path).unwrap();
                }
                #[cfg(unix)]
                "backup-symlink" => {
                    std::os::unix::fs::symlink(root.join("absent"), &backup).unwrap()
                }
                #[cfg(unix)]
                "ack-symlink" => std::os::unix::fs::symlink(root.join("absent"), &ack).unwrap(),
                _ => {}
            }
        }
        let before = std::fs::read(&path)
            .ok()
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        let lock = StateLock::acquire(&state).unwrap();
        assert!(
            pre_migration_backup(&state, &backup, &content, &lock).is_err(),
            "{damage}"
        );
        assert_eq!(
            std::fs::read(&path)
                .ok()
                .map(|bytes| format!("{:x}", Sha256::digest(bytes))),
            before,
            "{damage}"
        );
        if damage != "ack-existing" && damage != "ack-symlink" {
            assert!(!ack.exists());
        }
        drop(lock);
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[tokio::test]
async fn startup_backup_publication_and_flush_failures_abort_main_before_migration_or_listening() {
    if let Ok(root) = std::env::var("STARTUP_BACKUP_TEST_ROOT") {
        let error = crate::serve(
            vec![
                "--desktop".into(),
                "--config".into(),
                format!("{root}/config.json"),
            ],
            tokio::runtime::Handle::current(),
        )
        .await
        .unwrap_err();
        let expected = std::env::var("STARTUP_BACKUP_EXPECTED_ERROR").unwrap();
        assert!(format!("{error:#}").contains(&expected), "{error:#}");
        return;
    }
    for failure in ["publication", "snapshot", "acknowledgement"] {
        let root = root();
        let state = root.join("state");
        let path = state.join("game.db");
        let content = Content::load(Path::new("data")).unwrap();
        let store = Store::open(&root.join("baseline.db"), &content).unwrap();
        snapshot(&root.join("baseline.db"), &path, &content).unwrap();
        drop(store);
        schema21(&path);
        let before = dump(&path);
        let client = root.join("client");
        std::fs::create_dir_all(client.join("vendor")).unwrap();
        for name in ["index.html", "vendor/ruffle.js", "client.swf"] {
            std::fs::write(client.join(name), b"fixture").unwrap();
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bind = listener.local_addr().unwrap();
        drop(listener);
        let backup = root.join("backups/required.db");
        let config = serde_json::json!({"bind":bind.to_string(),"origin":format!("http://{bind}"),"data":std::fs::canonicalize("data").unwrap(),"client":client,"state":state,"pre_migration_backup":backup,"max_players":85,"behind_proxy":false,"shore_bots":false,"bot_llm":false,"realm_seed":42});
        std::fs::write(
            root.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "account_save::startup_backup::tests::startup_backup_publication_and_flush_failures_abort_main_before_migration_or_listening", "--nocapture"])
            .env("STARTUP_BACKUP_TEST_ROOT", &root);
        if failure == "publication" {
            command
                .env("STARTUP_BACKUP_FAIL_ACK", "1")
                .env("STARTUP_BACKUP_EXPECTED_ERROR", "acknowledgement");
        } else {
            command.env("ACCOUNT_SAVE_FAIL_STAGING_FLUSH", failure).env(
                "STARTUP_BACKUP_EXPECTED_ERROR",
                "injected staging flush failure",
            );
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(dump(&path), before);
        assert_eq!(
            readonly(&path)
                .unwrap()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            21
        );
        assert_eq!(backup.is_file(), failure != "snapshot");
        assert_eq!(
            PathBuf::from(format!("{}.ack.json", backup.display())).is_dir(),
            failure == "publication"
        );
        assert!(std::fs::read_dir(root.join("backups")).unwrap().all(|p| {
            !p.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("partial-")
        }));
        assert!(std::net::TcpStream::connect(bind).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn windows_startup_backup_durable_publication_consumes_staging_and_never_replaces() {
    let root = root();
    let staging = root.join("backups/stage");
    let destination = root.join("backups/final");
    let mut file = private_create(&staging).unwrap();
    file.write_all(b"verified bytes").unwrap();
    sync_staging(&file, "snapshot").unwrap();
    drop(file);
    assert!(publish_distinct(&staging, &destination).unwrap());
    assert!(!staging.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), b"verified bytes");
    std::fs::write(&staging, b"must not replace").unwrap();
    assert!(publish_distinct(&staging, &destination).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"verified bytes");
    assert_eq!(std::fs::read(&staging).unwrap(), b"must not replace");
    let ack = root.join("backups/final.ack.json");
    publish_ack(&ack, b"{\"protocol\":1}").unwrap();
    assert_eq!(std::fs::read(&ack).unwrap(), b"{\"protocol\":1}");
    assert!(publish_ack(&ack, b"must not replace").is_err());
    assert_eq!(std::fs::read(&ack).unwrap(), b"{\"protocol\":1}");
    assert!(std::fs::read_dir(root.join("backups")).unwrap().all(|p| {
        !p.unwrap()
            .file_name()
            .to_string_lossy()
            .contains("partial-")
    }));
    std::fs::remove_dir_all(root).unwrap();
}
