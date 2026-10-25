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
        crate::supervision::Supervisor::persisted(dir.join("incident.json")).unwrap(),
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
        assert!(crate::supervision::Supervisor::persisted(dir.join("incident.json")).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fatal_fame_fences_survive_restart_without_rejecting_already_accepted_sql() {
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
    assert!(crate::supervision::Supervisor::persisted(dir.join("incident.json")).is_err());
    preserve_uncertain(&app, &mut incident).unwrap();
    let restarted = crate::supervision::Supervisor::persisted(dir.join("incident.json")).unwrap();
    let mut expected = vec![pending_account, failed.account, closing.account];
    expected.sort_unstable();
    assert_eq!(
        restarted.latest().unwrap().unwrap().fenced_accounts,
        expected
    );
    for account in expected {
        assert!(restarted.check_account(account).is_err());
    }
    assert!(restarted.check_account(giver_character.account).is_ok());
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
    std::fs::remove_dir_all(dir).unwrap();
}
