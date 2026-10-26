use super::*;

fn input() -> WebReportInput {
    serde_json::from_value(serde_json::json!({"message":"First line\nReference: issue479", "screenshot":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGNgAAIAAAUAAarVyFEAAAAASUVORK5CYII="})).unwrap()
}
fn headers(a: &App) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("origin", a.origin.parse().unwrap());
    headers
}
#[tokio::test]
async fn website_reports_keep_live_session_origin_bounded_image_and_rate() {
    let (_dir, a) = account_actions::tests::fixture();
    let key = "a".repeat(32);
    let browser = a.diagnostics.browser(&key).unwrap();
    let (sender, mut requests) = mpsc::channel(1);
    browser.lock().unwrap().reports = Some(sender);
    assert_ne!(
        submit(
            State(a.clone()),
            ConnectInfo("127.0.0.1:18000".parse().unwrap()),
            Path(key.clone()),
            HeaderMap::new(),
            Json(input())
        )
        .await
        .status(),
        StatusCode::OK
    );
    let mut bad = serde_json::json!({"message":"test","screenshot":"not a png"});
    assert_ne!(
        submit(
            State(a.clone()),
            ConnectInfo("127.0.0.1:18000".parse().unwrap()),
            Path(key.clone()),
            headers(&a),
            Json(serde_json::from_value(bad.clone()).unwrap())
        )
        .await
        .status(),
        StatusCode::OK
    );
    bad["account"] = serde_json::json!(99);
    assert!(serde_json::from_value::<WebReportInput>(bad).is_err());
    assert_ne!(
        submit(
            State(a.clone()),
            ConnectInfo("127.0.0.1:18000".parse().unwrap()),
            Path("invalid".into()),
            headers(&a),
            Json(input())
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(requests.try_recv().is_err());
    let response = submit(
        State(a.clone()),
        ConnectInfo("127.0.0.1:18000".parse().unwrap()),
        Path(key.clone()),
        headers(&a),
        Json(input()),
    );
    let consume = async {
        let request = requests.recv().await.unwrap();
        from_session(&a, &Session::default(), request).await;
    };
    let (response, ()) = tokio::join!(response, consume);
    assert_ne!(
        response.status(),
        StatusCode::OK,
        "unauthenticated game task cannot save a report"
    );
    assert_ne!(
        submit(
            State(a.clone()),
            ConnectInfo("127.0.0.1:18000".parse().unwrap()),
            Path(key.clone()),
            headers(&a),
            Json(input())
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(
        requests.try_recv().is_err(),
        "rate-limited requests must not reach the game task"
    );
    let mut oversized = input();
    oversized.screenshot = "a".repeat(1_400_001);
    assert!(screenshot_png(&oversized.screenshot).is_err());
    let mut png = screenshot_png(&input().screenshot).unwrap();
    png[16..20].copy_from_slice(&5000_u32.to_be_bytes());
    use base64::Engine;
    assert!(screenshot_png(&base64::engine::general_purpose::STANDARD.encode(png)).is_err());
    drop(browser);
}

#[tokio::test]
async fn website_screenshot_is_durable_public_png_without_leaking_queue_bytes() {
    let (dir, a) = account_actions::tests::fixture();
    let id = "b".repeat(24);
    let input = input();
    let png = screenshot_png(&input.screenshot).unwrap();
    let report = json!({"id":id,"account_id":1,"created_at_unix_ms":1,"context":{"player_id":1},"delivery":{"status":"pending","notes_through":0},"notes":[{"text_untrusted":"already queued old /bug-info note"}],"screenshot":input.screenshot});
    a.diagnostics.save(&report).unwrap();
    let pending = a.diagnostics.next().unwrap().unwrap();
    assert!(pending.get("screenshot").is_none());
    assert_eq!(pending["delivery"]["note_batch"]["through"], 1);
    let reopened = Diagnostics::open(dir.join("bug-reports")).unwrap();
    assert_eq!(reopened.next().unwrap().unwrap()["id"], id);
    let response = screenshot(State(a.clone()), Path(id.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .as_ref(),
        png
    );
    assert_eq!(
        screenshot(State(a.clone()), Path("../../game".into()))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        screenshot(State(a), Path("c".repeat(24))).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn reports_keep_the_reporters_dropped_connection() {
    let (_dir, a) = account_actions::tests::fixture();
    let account = a.store.list("dropped".into()).await.unwrap().id;
    let transport = json!({"reason":"Output","input_queue_packets":64,"output_queue_packets":0});
    let connection = ConnectionDiagnostics::default();
    disconnect(&a, 7, Some(account), &connection, transport.clone()).await;
    let ch = a
        .store
        .create(account, a.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let presentation = a.store.presentation(account).await.unwrap();
    let player = a
        .world
        .call(move |w| w.enter_at(ch, None, None, presentation))
        .await
        .unwrap();
    let id = report(
        &a,
        account,
        player,
        "dropped near Skull Shrine",
        &connection,
        None,
    )
    .await
    .unwrap();
    let saved: Value = serde_json::from_slice(
        &std::fs::read(a.diagnostics.root.join(format!("{id}.json"))).unwrap(),
    )
    .unwrap();
    let drop = &saved["diagnostics"]["previous_drop"];
    assert_eq!(drop["transport"], transport);
    assert!(drop["diagnostics"]["connection"].is_object());
    assert!(a.diagnostics.previous_drop(account + 1).is_null());
}

#[tokio::test]
async fn anonymous_reports_publish_without_game_identity_or_rewards_and_connected_rewards_stay_once_only()
 {
    async fn json_body(response: Response) -> Value {
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap()
    }
    let (dir, a) = account_actions::tests::fixture();
    let peer = ConnectInfo("127.0.0.1:18000".parse().unwrap());
    let response = submit(
        State(a.clone()),
        peer,
        Path("a".repeat(32)),
        headers(&a),
        Json(input()),
    )
    .await;
    let id = json_body(response).await["id"].as_str().unwrap().to_owned();
    let pending = a.diagnostics.next().unwrap().unwrap();
    assert_eq!(pending["id"], id);
    assert_eq!(
        pending["reporter_name"],
        "Website visitor (no character connected)"
    );
    assert_eq!(pending["account_id"], Value::Null);
    assert_eq!(pending["context"], Value::Null);
    assert_eq!(
        pending["diagnostics"]["world_context_status"],
        "unavailable"
    );
    assert_eq!(
        pending["diagnostics"]["connection"]["connection_id"],
        Value::Null
    );
    let waiting = status(State(a.clone()), Path(id.clone())).await;
    assert_eq!(waiting.headers()["cache-control"], "no-store");
    assert_eq!(json_body(waiting).await, json!({"url":null}));
    let limited = submit(
        State(a.clone()),
        peer,
        Path("b".repeat(32)),
        headers(&a),
        Json(input()),
    )
    .await;
    assert_eq!(
        limited.status(),
        StatusCode::BAD_REQUEST,
        "rotating tab keys must not bypass anonymous throttling"
    );
    let mut authorization = HeaderMap::new();
    authorization.insert(
        "authorization",
        format!("Bearer {}", a.discord_link_token.as_ref().unwrap())
            .parse()
            .unwrap(),
    );
    let issue = |id: &str| Receipt {
        target: "issue_number".into(),
        id: id.into(),
        through: None,
    };
    assert_ne!(
        receipt(
            State(a.clone()),
            peer,
            Path(id.clone()),
            HeaderMap::new(),
            Json(issue("528"))
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        receipt(
            State(a.clone()),
            peer,
            Path(id.clone()),
            authorization.clone(),
            Json(issue("528"))
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        json_body(status(State(a.clone()), Path(id.clone())).await).await,
        json!({"url":"https://github.com/OpenRotMG/Open-RotMG-Server/issues/528"})
    );
    let db = rusqlite::Connection::open(dir.join("game.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM gold_rewards", [], |r| r
            .get::<_, i32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        Diagnostics::open(dir.join("bug-reports"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()["delivery"]["issue_number"],
        "528"
    );
    let account = a.store.list("connected-report".into()).await.unwrap();
    let character = a
        .store
        .create(account.id, a.content.get(crate::content::WIZARD).unwrap())
        .await
        .unwrap();
    let player = a
        .world
        .call(move |w| w.enter(character, None))
        .await
        .unwrap();
    let connected = report(
        &a,
        account.id,
        player,
        "Connected report",
        &ConnectionDiagnostics::default(),
        None,
    )
    .await
    .unwrap();
    let gold = a.store.gold(account.id).await.unwrap().balance;
    for _ in 0..2 {
        assert_eq!(
            receipt(
                State(a.clone()),
                peer,
                Path(connected.clone()),
                authorization.clone(),
                Json(issue("529"))
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(a.store.gold(account.id).await.unwrap().balance, gold + 50);
    assert_eq!(
        status(State(a.clone()), Path("../game".into()))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(State(a), Path("f".repeat(24))).await.status(),
        StatusCode::NOT_FOUND
    );
}
