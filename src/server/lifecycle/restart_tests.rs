use super::super::lifecycle_finalize_tests::*;
use super::*;

#[tokio::test]
async fn restart_does_not_hide_incident_write_errors() {
    let (dir, app) = super::super::account_actions::tests::fixture();
    let path = dir.join("incident.json");
    let supervisor = crate::supervision::Supervisor::recovered(path.clone(), &app.store).unwrap();
    supervisor.failure("fixture", anyhow::anyhow!("injected worker failure"));
    let mut incident = supervisor.latest().unwrap().unwrap();
    supervisor.preserve_accounts(&mut incident, &[1]).unwrap();
    let before = std::fs::read(&path).unwrap();
    std::fs::create_dir(path.with_extension("json.new")).unwrap();
    assert!(crate::supervision::Supervisor::recovered(path.clone(), &app.store).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::remove_dir(path.with_extension("json.new")).unwrap();
    assert!(
        crate::supervision::Supervisor::recovered(path.clone(), &app.store)
            .unwrap()
            .check_account(1)
            .is_ok()
    );
    std::fs::create_dir(path.with_extension("json.new")).unwrap();
    let fresh = crate::supervision::Supervisor::recovered(path, &app.store).unwrap();
    fresh.failure(
        "fixture-write-error",
        anyhow::anyhow!("injected worker failure"),
    );
    assert!(fresh.ensure_running().is_err());
    let failure = tokio::time::timeout(std::time::Duration::from_secs(1), fresh.failed())
        .await
        .unwrap();
    assert_eq!(failure.component, "fixture-write-error");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn restart_releases_accounts_even_without_a_shutdown_checkpoint() {
    let _logs =
        tracing::subscriber::set_default(tracing_subscriber::fmt().with_test_writer().finish());
    for checkpoint in [true, false] {
        let (dir, mut app) = super::super::account_actions::tests::fixture();
        let account = app
            .store
            .register(
                "restart".into(),
                "restart@example.test".into(),
                "test-password".into(),
            )
            .await
            .unwrap();
        let ch = app
            .store
            .create(account, app.content.get(WIZARD).unwrap())
            .await
            .unwrap();
        let path = dir.join("incident.json");
        let supervisor =
            crate::supervision::Supervisor::recovered(path.clone(), &app.store).unwrap();
        supervisor.failure("shutdown", anyhow::anyhow!("injected lost save reply"));
        let mut incident = supervisor.latest().unwrap().unwrap();
        if checkpoint {
            supervisor
                .preserve_accounts(&mut incident, &[account])
                .unwrap();
        }
        assert!(supervisor.check_account(account).is_err());
        let store = crate::store::Store::open(&dir.join("game.db"), &app.content).unwrap();
        let restarted = crate::supervision::Supervisor::recovered(path, &store).unwrap();
        restarted
            .check_account(account)
            .expect("recovered account is still locked out");
        let saved = restarted.latest().unwrap().unwrap();
        assert!(saved.fenced_accounts.is_empty());
        assert_eq!(saved.id, incident.id);
        assert_eq!(saved.fences_preserved, checkpoint);
        app.store = store;
        app.world = crate::world::Handle::supervised(
            crate::world::Directory::test_with_seed(
                std::path::Path::new("data"),
                app.content.clone(),
                16,
                12351,
            )
            .unwrap(),
            restarted,
        );
        app.world.clock.set_paused(true);
        let (port, server) = socket_server(&app).await;
        let mut socket = NativeSocket::connect(port, &app.origin).await;
        socket
            .send(socket_hello_password(
                &dir,
                "restart@example.test",
                "test-password",
                -2,
                None,
            ))
            .await;
        socket.receive(wire::MAP_INFO).await;
        socket
            .send(Packet {
                id: wire::LOAD,
                body: ch.id.to_be_bytes().to_vec(),
            })
            .await;
        socket.receive(wire::CREATE_SUCCESS).await;
        println!("restart checkpoint={checkpoint}: native login and character load succeeded");
        assert!(
            supervisor.check_account(account).is_err(),
            "old process admitted ownership"
        );
        app.shutdown.send_replace(true);
        server.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
