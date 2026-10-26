use super::*;

async fn request(app: &App, id: &str, command: Value) -> Value {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer persona-test".parse().unwrap());
    let response = super::command(
        State(app.clone()),
        headers,
        Json(json!({"id":id,"command":command})),
    )
    .await;
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn prepared_persona_requires_test_pause_and_preserves_actual_registry_identity_without_reset()
{
    let (dir, mut app) = crate::server::account_actions::tests::fixture();
    app.control = Some("persona-test".into());
    let pos = app
        .world
        .in_world(-2)
        .call(|w| {
            let (x, y) = w.map.spawns[0];
            Ok(Pos { x, y })
        })
        .await
        .unwrap();
    let command = json!({"op":"persona","profile":"Acrobat","world":-2,"position":pos});
    assert!(
        crate::bots::start_test_persona(&app, "Acrobat", -2, pos)
            .await
            .is_err()
    );
    write_json(
        &dir.join("testing.json"),
        &json!({"schema":1,"provenance":"prepared"}),
    )
    .unwrap();
    app.testing = Testing::open(&dir).unwrap();
    app.world.clock.set_paused(false);
    assert_eq!(
        request(&app, "running", command.clone()).await["status"],
        "failed"
    );
    app.world.clock.set_paused(true);
    let mut invalid = command.clone();
    invalid["profile"] = "NotAnActualPersona".into();
    assert_eq!(request(&app, "unknown", invalid).await["status"], "failed");
    let created = request(&app, "create", command.clone()).await;
    assert_eq!(created["status"], "succeeded", "{created}");
    let account = created["result"]["account"].as_i64().unwrap() as i32;
    let actor = created["result"]["actor"].as_i64().unwrap() as i32;
    assert_eq!(
        app.store
            .shore_account("persona:v1:Acrobat".into())
            .await
            .unwrap(),
        Some(account)
    );
    let registration = app.testing.as_ref().unwrap().registration(actor).unwrap();
    assert_eq!(
        registration.guid,
        app.store.account_guid(account).await.unwrap()
    );
    assert_eq!(registration.profile.as_deref(), Some("Acrobat"));
    let before = app.store.load(account, actor).await.unwrap();
    assert_eq!(
        request(&app, "duplicate", command).await["status"],
        "failed"
    );
    assert_eq!(app.store.load(account, actor).await.unwrap(), before);
    app.shutdown.send(true).unwrap();
}
