use super::*;

#[tokio::test]
async fn startup_rejects_integrity_and_foreign_key_failures() {
    let content = Content::load(Path::new("data")).unwrap();
    for (damage, expected) in [
        (
            "PRAGMA ignore_check_constraints=ON; UPDATE accounts SET credits=-1",
            "integrity_check",
        ),
        (
            "PRAGMA foreign_keys=OFF; INSERT INTO discord_codes VALUES(999,'nonsecret-test-marker',1,2)",
            "foreign_key_check",
        ),
    ] {
        let dir = std::env::temp_dir().join(format!("startup-check-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("game.db");
        let store = Store::open(&path, &content).unwrap();
        store.list("startup-check".into()).await.unwrap();
        drop(store);
        Connection::open(&path)
            .unwrap()
            .execute_batch(damage)
            .unwrap();
        let error = Store::open(&path, &content)
            .err()
            .expect("damaged store admitted");
        assert!(format!("{error:#}").contains(expected), "{error:#}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
