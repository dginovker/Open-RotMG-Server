use super::*;
use std::time::Duration;

fn fixture() -> (std::path::PathBuf, App) {
    let (dir, mut app) = super::super::account_actions::tests::fixture();
    let directory = crate::world::Directory::test_with_seed(
        std::path::Path::new("data"),
        app.content.clone(),
        16,
        12351,
    )
    .unwrap();
    app.world = crate::world::Handle::supervised(
        directory,
        crate::supervision::Supervisor::recovered(dir.join("incident.json"), &app.store).unwrap(),
    );
    app.world.clock.set_paused(true);
    (dir, app)
}

async fn avatar(app: &App, name: &str) -> (Owner, i32, crate::store::Character) {
    let account = app.store.list(name.into()).await.unwrap().id;
    let character = app
        .store
        .create(account, app.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let snapshot = character.clone();
    let owner = app.claim_session(account).unwrap();
    let binding = binding(app, account).unwrap();
    let player = app
        .world
        .call(move |world| {
            let player = world.enter(character, None)?;
            binding(player)?;
            Ok(player)
        })
        .await
        .unwrap();
    (owner, player, snapshot)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fatal_world_and_persistence_workers_stop_real_server_and_preserve_incident() {
    for component in ["world", "persistence"] {
        let (dir, app) = fixture();
        if component == "persistence" {
            let account = app.store.list("orphan-progress".into()).await.unwrap().id;
            let character = app
                .store
                .create(account, app.content.get(WIZARD).unwrap())
                .await
                .unwrap();
            app.world
                .call(move |world| {
                    let id = world.enter(character, None)?;
                    world.players.get_mut(&id).unwrap().character.lifetime.tiles += 1;
                    Ok(())
                })
                .await
                .unwrap();
        }
        let server_app = app.clone();
        let server = tokio::spawn(async move {
            super::super::serve(server_app, "127.0.0.1:0".parse().unwrap(), "data".into()).await
        });
        if component == "world" {
            assert!(
                app.world
                    .directory::<()>(|_| panic!("injected world worker panic"))
                    .await
                    .is_err()
            );
        }
        let error = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("critical {component} worker")),
            "{error:#}"
        );
        assert!(*app.shutdown.borrow());
        let incident = app.world.supervisor.latest().unwrap().unwrap();
        assert_eq!(incident.component, component);
        assert_eq!(
            incident.failure,
            if component == "world" {
                "panicked"
            } else {
                "failed"
            }
        );
        assert!(incident.fences_preserved);
        assert!(incident.fenced_accounts.is_empty());
        assert!(
            crate::supervision::Supervisor::recovered(dir.join("incident.json"), &app.store)
                .is_ok()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_shutdown_drain_releases_fences_only_after_recovery() {
    let (dir, app) = fixture();
    let (_owner, player, mut character) = avatar(&app, "held-shutdown").await;
    let account = character.account;
    let operation = reserve_player(&app, account, player, "held_shutdown_save")
        .unwrap()
        .unwrap();
    character.lifetime.tiles += 1;
    let (committed, release_sql) = app.store.lifecycle_test_pause_reply();
    let completion = tokio::spawn(complete(operation, move |app| async move {
        stage(Stage::Store);
        app.store.progress(character).await?;
        stage(Stage::Publication);
        Ok(())
    }));
    tokio::time::timeout(Duration::from_secs(5), committed)
        .await
        .unwrap()
        .unwrap();
    drain::prepare(&app);
    app.shutdown.send_replace(true);
    let error = tokio::time::timeout(Duration::from_secs(20), drain::terminate(&app))
        .await
        .unwrap()
        .unwrap_err();
    let original = error.to_string();
    assert!(original.contains("shutdown drain failed"), "{original}");
    assert!(original.contains("drain deadline exceeded"), "{original}");
    assert!(!completion.is_finished());
    assert!(state(&app, account).unwrap().0);
    let error = super::super::failed_shutdown(&app, error)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), original);
    let incident = app.world.supervisor.latest().unwrap().unwrap();
    assert_eq!(incident.component, "shutdown");
    assert_eq!(incident.failure, "failed");
    assert!(incident.fences_preserved);
    assert_eq!(incident.fenced_accounts, vec![account]);
    assert!(app.world.supervisor.check_account(account).is_err());
    release_sql.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), completion)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(app.world.supervisor.check_account(account).is_err());
    let store = crate::store::Store::open(&dir.join("game.db"), &app.content).unwrap();
    let restarted =
        crate::supervision::Supervisor::recovered(dir.join("incident.json"), &store).unwrap();
    assert!(restarted.check_account(account).is_ok());
    assert!(
        restarted
            .latest()
            .unwrap()
            .unwrap()
            .fenced_accounts
            .is_empty()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_shutdown_checkpoint_failure_retains_original_error_but_allows_recovery() {
    let (dir, app) = fixture();
    app.world
        .supervisor
        .failure("shutdown", anyhow::anyhow!("original drain failure"));
    std::fs::create_dir(dir.join("incident.json.new")).unwrap();
    let error = super::super::failed_shutdown(&app, anyhow::anyhow!("original drain failure"))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("account-fence checkpoint failed")
    );
    assert_eq!(error.root_cause().to_string(), "original drain failure");
    assert!(
        crate::supervision::Supervisor::recovered(dir.join("incident.json"), &app.store).is_ok()
    );
    assert!(
        !app.world
            .supervisor
            .latest()
            .unwrap()
            .unwrap()
            .fences_preserved
    );
    app.shutdown.send_replace(true);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorded_drain_and_critical_checkpoint_failures_both_survive_select() {
    let (dir, app) = fixture();
    let failure = Mutex::new(Some(anyhow::anyhow!("original drain failure")));
    app.world.supervisor.failure(
        "fixture-critical",
        anyhow::anyhow!("concurrent worker failure"),
    );
    let incident = app.world.supervisor.latest().unwrap().unwrap();
    let blocked = dir.join("incident.json.new");
    std::fs::create_dir(&blocked).unwrap();
    let checkpoint_error = std::fs::File::create(&blocked).unwrap_err().to_string();
    let (_stopped, stop) = tokio::sync::watch::channel(false);
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        super::super::supervise_serving(&app, std::future::pending(), &failure, stop),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.root_cause().to_string(), "original drain failure");
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("critical fixture-critical worker failed"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains(&incident.id), "{diagnostic}");
    assert!(
        diagnostic.contains("account-fence checkpoint failed"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains(&checkpoint_error), "{diagnostic}");
    assert!(failure.lock().unwrap().is_none());
    assert!(*app.shutdown.borrow());
    let saved = app.world.supervisor.latest().unwrap().unwrap();
    assert_eq!(saved.id, incident.id);
    assert_eq!(saved.component, "fixture-critical");
    assert!(!saved.fences_preserved);
    assert!(
        crate::supervision::Supervisor::recovered(dir.join("incident.json"), &app.store).is_ok()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fatal_fame_fences_stop_ownership_without_rejecting_accepted_sql() {
    let (dir, app) = fixture();
    let (_pending_owner, pending_player, mut pending) = avatar(&app, "pending").await;
    let (failed_owner, _, failed) = avatar(&app, "failed").await;
    let (closing_owner, closing_player, closing) = avatar(&app, "closing").await;
    let (_giver_owner, giver, giver_character) = avatar(&app, "giver").await;
    let (_recipient_owner, recipient, recipient_character) = avatar(&app, "recipient").await;
    let giver_character_id = giver_character.id;
    app.world
        .call(move |world| {
            world
                .players
                .get_mut(&recipient)
                .unwrap()
                .queue_experience(50, Some(giver_character_id), true)?;
            world.stop_controller(giver)?;
            assert!(world.players[&giver].departing);
            assert_eq!(
                world.credit_recipients(giver_character_id),
                vec![(recipient, recipient_character.account)]
            );
            Ok(())
        })
        .await
        .unwrap();
    failed_owner.failed("injected uncertain owner failure".into());
    let operation = reserve_player(&app, pending.account, pending_player, "accepted_sql")
        .unwrap()
        .unwrap();
    pending.lifetime.tiles += 1;
    let pending_account = pending.account;
    let pending_character = pending.id;
    let pending_revision = pending.revision;
    let (committed, release_sql) = app.store.lifecycle_test_pause_reply();
    let completion = tokio::spawn(complete(operation, move |app| async move {
        stage(Stage::Store);
        app.store.progress(pending).await?;
        stage(Stage::Publication);
        Ok(())
    }));
    tokio::time::timeout(Duration::from_secs(5), committed)
        .await
        .unwrap()
        .unwrap();
    let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
    let stats: String = db
        .query_row(
            "SELECT stats FROM characters WHERE id=?",
            [pending_character],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<crate::store::Character>(&stats)
            .unwrap()
            .lifetime
            .tiles,
        1
    );
    let (finalizing, release_world) = app.world.lifecycle_test_pause_job(1, true);
    let finalized = closing_owner
        .close(&app, CloseReason::Controller)
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), finalizing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state(&app, closing.account),
        Some((false, Some(closing_player), false))
    );
    app.world.supervisor.spawn("fixture-critical", async {
        anyhow::bail!("injected critical worker failure")
    });
    let mut incident = tokio::time::timeout(Duration::from_secs(5), app.world.supervisor.failed())
        .await
        .unwrap();
    assert!(!incident.fences_preserved);
    preserve_uncertain(&app, &mut incident).unwrap();
    let mut expected = vec![pending_account, failed.account, closing.account];
    expected.sort_unstable();
    assert_eq!(
        app.world
            .supervisor
            .latest()
            .unwrap()
            .unwrap()
            .fenced_accounts,
        expected
    );
    for account in expected {
        assert!(app.world.supervisor.check_account(account).is_err());
    }
    assert!(app.world.supervisor.ensure_running().is_err());
    let error = app.claim(recipient_character.account).err().unwrap();
    assert!(error.to_string().contains("critical worker stopped"));
    let error = reserve_player(&app, recipient_character.account, recipient, "new_progress")
        .err()
        .unwrap();
    assert!(error.to_string().contains("critical worker stopped"));
    let error = reserve_credit_recipient(
        &app,
        (giver_character.account, giver),
        (recipient_character.account, recipient),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("critical worker stopped"));
    assert!(!state(&app, recipient_character.account).unwrap().0);
    release_sql.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), completion)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let saved = app
        .store
        .load(pending_account, pending_character)
        .await
        .unwrap();
    assert_eq!(saved.revision, pending_revision);
    assert_eq!(saved.lifetime.tiles, 1);
    assert!(saved.alive);
    assert_eq!(
        db.query_row("SELECT count(*) FROM deaths", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    release_world.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), finalized.wait())
            .await
            .unwrap()
            .is_err()
    );
    app.shutdown.send_replace(true);
    let store = crate::store::Store::open(&dir.join("game.db"), &app.content).unwrap();
    let restarted =
        crate::supervision::Supervisor::recovered(dir.join("incident.json"), &store).unwrap();
    for account in [pending_account, failed.account, closing.account] {
        assert!(restarted.check_account(account).is_ok());
    }
    std::fs::remove_dir_all(dir).unwrap();
}
