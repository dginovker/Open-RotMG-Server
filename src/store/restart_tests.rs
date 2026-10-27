use super::*;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn gate(path: &Path) -> ! {
    let mut ready = std::fs::File::create(path.join("ready")).unwrap();
    ready.write_all(b"ready").unwrap();
    ready.sync_all().unwrap();
    loop {
        std::thread::park();
    }
}
fn write(ch: &Character) -> InventoryWrite {
    InventoryWrite {
        id: ch.id,
        account: ch.account,
        revision: ch.revision,
        items: ch.items.clone(),
    }
}
async fn mutate(store: &Store, content: &Content, operation: &str) -> Result<()> {
    let mut left = store.load(1, 1).await?;
    match operation {
        "trade" => {
            let mut right = store.load(2, 2).await?;
            std::mem::swap(&mut left.items[4], &mut right.items[4]);
            store
                .exchange_trade("a".repeat(32), vec![write(&left), write(&right)], None)
                .await?;
        }
        "pickup" => {
            left.items[5] = 0xa97;
            store
                .exchange_collected(vec![write(&left)], vec![], Some(0xa97))
                .await?;
        }
        "drop" => {
            left.items[4] = -1;
            store.exchange(vec![write(&left)], vec![]).await?;
        }
        "death" => {
            left.alive = false;
            left.hp = 0;
            store.death(left, "restart fixture".into()).await?;
        }
        "bot_entry" => {
            let ch = Character::fresh(content.get(WIZARD)?)?;
            store
                .persona_character(
                    "persona:v1:Crashbot".into(),
                    vec![ch],
                    vec![[-1; 8]],
                    0,
                    None,
                )
                .await?;
        }
        _ => panic!("unknown crash operation"),
    }
    Ok(())
}
fn snapshot(path: &Path) -> Vec<String> {
    let db = Connection::open(path).unwrap();
    [
        "SELECT json_group_array(json_array(id,name,credits,character_slots,vault_mask)) FROM (SELECT * FROM accounts ORDER BY id)",
        "SELECT json_group_array(json_array(id,account,alive,revision,items,json_extract(stats,'$.fame'))) FROM (SELECT * FROM characters ORDER BY id)",
        "SELECT json_group_array(json_array(account,chest,revision,items)) FROM (SELECT * FROM vaults ORDER BY account,chest)",
        "SELECT json_group_array(json_array(character,snapshot,receipt)) FROM (SELECT * FROM deaths ORDER BY character)",
        "SELECT json_group_array(json_array(account,balance,earned)) FROM (SELECT * FROM account_fame ORDER BY account)",
        "SELECT json_group_array(json_array(id,owners,writes,revisions)) FROM trade_transactions",
        "SELECT json_group_array(json_array(account,item)) FROM collections",
        "SELECT json_group_array(json_array(account,class,character)) FROM legacy_claims",
        "SELECT json_group_array(json_array(name,account)) FROM shore_bots",
        "SELECT json_group_array(json_array(account,reward)) FROM gold_rewards",
    ].into_iter().map(|sql| db.query_row(sql, [], |r| r.get(0)).unwrap()).collect()
}

#[tokio::test]
async fn killed_mutations_recover_without_duplicates_or_account_lockouts() {
    let content = Content::load(Path::new("data")).unwrap();
    if let Ok(path) = std::env::var("ROTMG_RESTART_CASE") {
        let path = std::path::PathBuf::from(path);
        let store = Store::open(&path.join("game.db"), &content).unwrap();
        let supervisor =
            crate::supervision::Supervisor::recovered(path.join("incident.json"), &store).unwrap();
        supervisor.failure("fixture", anyhow::anyhow!("injected uncertain completion"));
        let mut incident = supervisor.latest().unwrap().unwrap();
        supervisor
            .preserve_accounts(&mut incident, &[1, 2])
            .unwrap();
        let cut = std::env::var("ROTMG_RESTART_CUT").unwrap();
        let boundary = path.clone();
        store
            .call(move |db| {
                match cut.as_str() {
                    "first_write" => db.update_hook(Some(
                        move |_: rusqlite::hooks::Action, _: &str, _: &str, _: i64| gate(&boundary),
                    )),
                    "before_commit" => db.commit_hook(Some(move || gate(&boundary))),
                    "committed" => db.wal_hook(Some(|_, _| {
                        gate(Path::new(&std::env::var("ROTMG_RESTART_CASE").unwrap()))
                    })),
                    _ => panic!("unknown crash boundary"),
                }
                Ok(())
            })
            .await
            .unwrap();
        mutate(
            &store,
            &content,
            &std::env::var("ROTMG_RESTART_OP").unwrap(),
        )
        .await
        .unwrap();
        panic!("mutation returned without reaching its crash boundary");
    }
    let root = std::env::var_os("QA_RESULTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("restart-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&root).unwrap();
    let template = root.join("template.db");
    let store = Store::open(&template, &content).unwrap();
    for guid in ["left", "right"] {
        let account = store
            .register(
                guid.into(),
                format!("{guid}@example.test"),
                "test-password".into(),
            )
            .await
            .unwrap();
        store.compete(account).await.unwrap();
        let mut ch = store
            .create(account, content.get(WIZARD).unwrap())
            .await
            .unwrap();
        ch.fame = 7;
        store.progress(ch.clone()).await.unwrap();
        ch.items[4] = if account == 1 { 0xa4f } else { 0xa97 };
        store.exchange(vec![write(&ch)], vec![]).await.unwrap();
    }
    drop(store);
    let initial: i32 = Connection::open(&template)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM characters,json_each(items) WHERE value>=0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for operation in ["trade", "pickup", "drop", "death", "bot_entry"] {
        let expected_path = root.join(format!("{operation}-expected.db"));
        Connection::open(&template)
            .unwrap()
            .execute("VACUUM INTO ?", [expected_path.to_str().unwrap()])
            .unwrap();
        let expected_store = Store::open(&expected_path, &content).unwrap();
        mutate(&expected_store, &content, operation).await.unwrap();
        let db = Connection::open(&expected_path).unwrap();
        let held: i32 = db
            .query_row(
                "SELECT count(*) FROM characters,json_each(items) WHERE value>=0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let base = Character::fresh(content.get(WIZARD).unwrap())
            .unwrap()
            .items
            .iter()
            .filter(|&&item| item >= 0)
            .count() as i32;
        assert_eq!(
            held,
            match operation {
                "trade" => initial,
                "pickup" => initial + 1,
                "drop" => initial - 1,
                "death" => initial / 2,
                "bot_entry" => initial + base,
                _ => unreachable!(),
            },
            "{operation}: item conservation"
        );
        if operation == "trade" {
            assert_eq!(expected_store.load(1, 1).await.unwrap().items[4], 0xa97);
            assert_eq!(expected_store.load(2, 2).await.unwrap().items[4], 0xa4f);
        }
        if operation == "pickup" {
            assert_eq!(
                db.query_row("SELECT credits FROM accounts WHERE id=1", [], |r| r
                    .get::<_, i32>(0))
                    .unwrap(),
                200
            );
        }
        if operation == "death" {
            assert_eq!(
                db.query_row(
                    "SELECT balance FROM account_fame WHERE account=1",
                    [],
                    |r| r.get::<_, i32>(0)
                )
                .unwrap(),
                29
            );
            assert!(expected_store.load(1, 1).await.is_err());
        }
        let expected = snapshot(&expected_path);
        drop(expected_store);
        for cut in ["first_write", "before_commit", "committed"] {
            let dir = root.join(format!("{operation}-{cut}"));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("game.db");
            Connection::open(&template)
                .unwrap()
                .execute("VACUUM INTO ?", [path.to_str().unwrap()])
                .unwrap();
            let before = snapshot(&path);
            let original = Store::open(&path, &content).unwrap();
            let stale = write(&original.load(1, 1).await.unwrap());
            drop(original);
            let output = std::fs::File::create(dir.join("worker.log")).unwrap();
            let mut worker = Worker(Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "store::restart_tests::killed_mutations_recover_without_duplicates_or_account_lockouts", "--nocapture"])
                .env("ROTMG_RESTART_CASE", &dir).env("ROTMG_RESTART_OP", operation).env("ROTMG_RESTART_CUT", cut)
                .stdout(Stdio::from(output.try_clone().unwrap())).stderr(Stdio::from(output)).spawn().unwrap());
            let started = Instant::now();
            while !dir.join("ready").exists() {
                assert!(
                    worker.0.try_wait().unwrap().is_none(),
                    "{operation}/{cut}: child exited; see {}",
                    dir.display()
                );
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "{operation}/{cut}: gate deadline; see {}",
                    dir.display()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            worker.0.kill().unwrap();
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
            let recovered = Store::open(&path, &content).unwrap();
            assert_eq!(
                snapshot(&path),
                if cut == "committed" {
                    &expected
                } else {
                    &before
                }
                .clone(),
                "{operation}/{cut}"
            );
            let supervisor =
                crate::supervision::Supervisor::recovered(dir.join("incident.json"), &recovered)
                    .unwrap();
            for account in [1, 2] {
                supervisor.check_account(account).unwrap();
            }
            if cut != "committed" {
                mutate(&recovered, &content, operation).await.unwrap();
            }
            if operation == "bot_entry" {
                mutate(&recovered, &content, operation).await.unwrap();
            } else {
                assert!(
                    recovered.exchange(vec![stale], vec![]).await.is_err(),
                    "{operation}/{cut}: stale inventory replay accepted"
                );
            }
            assert_eq!(
                snapshot(&path),
                expected,
                "{operation}/{cut}: recovered state or bot re-entry duplicated holdings"
            );
            println!(
                "SIGKILL {operation}/{cut}: atomic state, committed holdings preserved, accounts admitted"
            );
        }
    }
}
