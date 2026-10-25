use super::*;

async fn request(app: &App, id: &str, command: Value) -> Value {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer protection-test".parse().unwrap());
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
async fn prepared_protection_uses_real_conditions_and_rejects_without_fencing() {
    let (dir, mut app) = crate::server::account_actions::tests::fixture();
    write_json(
        &dir.join("testing.json"),
        &json!({"schema":1,"provenance":"prepared"}),
    )
    .unwrap();
    app.testing = Testing::open(&dir).unwrap();
    app.control = Some("protection-test".into());
    app.world.clock.set_paused(true);
    let pos = app
        .world
        .in_world(-2)
        .call(|w| {
            let (x, y) = w.map.spawns[0];
            Ok(Pos { x, y })
        })
        .await
        .unwrap();
    let spawn = request(&app,"spawn",json!({"op":"spawn","spec":{"guid":"qa-protection","class":WIZARD,"level":8,"seed":12351,"equipment":[-1,-1,-1,-1],"backpack":[-1,-1,-1,-1,-1,-1,-1,-1],"world":-2,"position":pos,"controller":"direct"}})).await;
    assert_eq!(spawn["status"], "succeeded", "{spawn}");
    let actor = spawn["result"]["actor"].as_i64().unwrap();
    let initial = request(&app, "initial", json!({"op":"observe","actor":actor})).await;
    let player = initial["session"]["player"].as_i64().unwrap() as i32;
    let change = json!({"kind":"protection","condition":"invincible","duration_ms":1000});
    for (i, duration) in [0, 60001].into_iter().enumerate() {
        let mut invalid = change.clone();
        invalid["duration_ms"] = duration.into();
        assert_eq!(
            request(
                &app,
                &format!("invalid-{i}"),
                json!({"op":"cheat","actor":actor,"change":invalid})
            )
            .await["status"],
            "failed"
        );
        assert_eq!(
            request(
                &app,
                &format!("unchanged-{i}"),
                json!({"op":"observe","actor":actor})
            )
            .await,
            initial
        );
    }
    assert_eq!(
        request(
            &app,
            "foreign",
            json!({"op":"cheat","actor":99999,"change":change})
        )
        .await["status"],
        "failed"
    );
    let testing = app.testing.take();
    assert_eq!(
        request(
            &app,
            "ordinary",
            json!({"op":"cheat","actor":actor,"change":change})
        )
        .await["status"],
        "failed"
    );
    app.testing = testing;
    app.world.clock.set_paused(false);
    assert_eq!(
        request(
            &app,
            "running",
            json!({"op":"cheat","actor":actor,"change":change})
        )
        .await["status"],
        "failed"
    );
    app.world.clock.set_paused(true);
    for (name, index) in [
        ("invincible", crate::condition::INVINCIBLE),
        ("stasis", crate::condition::STASIS),
        ("invulnerable", crate::condition::INVULNERABLE),
    ] {
        let id = format!("apply-{name}");
        let command = json!({"op":"cheat","actor":actor,"change":{"kind":"protection","condition":name,"duration_ms":1000}});
        let applied = request(&app, &id, command.clone()).await;
        assert_eq!(applied["status"], "succeeded", "{applied}");
        assert_eq!(request(&app, &id, command).await, applied);
        let after = request(
            &app,
            &format!("after-{name}"),
            json!({"op":"observe","actor":actor}),
        )
        .await;
        assert_eq!(
            after["character"]["conditions"],
            crate::condition::bit(index)
        );
        assert_eq!(after["session"], initial["session"]);
        for key in [
            "base_stats",
            "stats",
            "items",
            "inventory_revision",
            "hp",
            "mp",
        ] {
            assert_eq!(after["character"][key], initial["character"][key], "{key}");
        }
        assert_eq!(
            request(
                &app,
                &format!("overlap-{name}"),
                json!({"op":"cheat","actor":actor,"change":change})
            )
            .await["status"],
            "failed"
        );
        assert_eq!(
            request(
                &app,
                &format!("no-fence-{name}"),
                json!({"op":"observe","actor":actor})
            )
            .await,
            after
        );
        app.world
            .for_player(player)
            .call(move |w| {
                w.now += 999;
                assert_eq!(
                    w.players[&player].conditions(w.now),
                    crate::condition::bit(index)
                );
                w.now += 1;
                assert_eq!(w.players[&player].conditions(w.now), 0);
                w.updates(player, true)
            })
            .await
            .unwrap();
        assert_eq!(
            request(
                &app,
                &format!("expired-{name}"),
                json!({"op":"observe","actor":actor})
            )
            .await["character"]["conditions"],
            0
        );
    }
    app.shutdown.send_replace(true);
    fs::remove_dir_all(dir).unwrap();
}
