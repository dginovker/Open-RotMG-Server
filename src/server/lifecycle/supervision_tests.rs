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
            super::super::serve(
                server_app,
                "127.0.0.1:0".parse().unwrap(),
                "data".into(),
                false,
                None,
            )
            .await
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
        assert!(
            crate::supervision::Supervisor::recovered(dir.join("incident.json"), &app.store)
                .is_ok()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn realm_progress_starts_each_owned_save_before_waiting_for_other_accounts() {
    let (_, app) = fixture();
    let mut avatars = Vec::new();
    for name in ["save-first", "save-second", "save-third"] {
        avatars.push(avatar(&app, name).await);
    }
    let ids: Vec<_> = avatars.iter().map(|(_, id, _)| *id).collect();
    let expected = app
        .world
        .call(move |world| {
            let mut expected = std::collections::BTreeMap::new();
            for id in ids {
                let player = world.players.get_mut(&id).unwrap();
                player.character.lifetime.tiles += 1;
                expected.insert(id, player.character.lifetime.tiles);
            }
            Ok(expected)
        })
        .await
        .unwrap();
    let (entered, release) = app.store.lifecycle_test_pause_reply();
    let saving = tokio::spawn({
        let app = app.clone();
        async move { super::super::persistence_tick(&app).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    let submitted = tokio::time::timeout(Duration::from_secs(1), async {
        while app.store.lifecycle_pending_jobs() < avatars.len() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), saving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        submitted.is_ok(),
        "later accounts remain reserved without starting their saves while native input waits"
    );
    for (owner, id, before) in avatars {
        let saved = app.store.load(before.account, before.id).await.unwrap();
        assert_eq!(saved.lifetime.tiles, expected[&id]);
        assert_eq!(saved.items, before.items);
        assert!(saved.alive);
        assert!(!state(&app, before.account).unwrap().0);
        owner
            .close(&app, CloseReason::Transport)
            .unwrap()
            .unwrap()
            .wait()
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_shutdown_drain_records_incident_and_restart_admits_the_account() {
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
    release_sql.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), completion)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let store = crate::store::Store::open(&dir.join("game.db"), &app.content).unwrap();
    let restarted =
        crate::supervision::Supervisor::recovered(dir.join("incident.json"), &store).unwrap();
    assert!(restarted.ensure_running().is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_publication_still_saves_releases_and_drains() {
    // Live Nov 3: 15 bot re-entries failed at publication; their claims refused every deploy drain until restart.
    let (dir, app) = fixture();
    let (owner, player, character) = avatar(&app, "failed-publication").await;
    let account = character.account;
    let operation = reserve_player(&app, account, player, "autonomous_reentry")
        .unwrap()
        .unwrap();
    let error = complete(operation, |_| async {
        stage(Stage::Publication);
        Err::<(), _>(anyhow::anyhow!("invalid market memory bounds"))
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("invalid market memory bounds"));
    let finalized = owner.close(&app, CloseReason::Controller).unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(10), finalized.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(state(&app, account).is_none(), "the account is released");
    assert!(
        !app.world
            .call(move |w| Ok(w.players.contains_key(&player)))
            .await
            .unwrap(),
        "the failed avatar keeps a player slot"
    );
    drain::prepare(&app);
    app.shutdown.send_replace(true);
    tokio::time::timeout(Duration::from_secs(20), drain::terminate(&app))
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
