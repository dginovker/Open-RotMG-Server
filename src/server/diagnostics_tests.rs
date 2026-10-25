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
async fn website_reports_require_live_tab_origin_bounded_image_and_rate() {
    let (_dir, a) = account_actions::tests::fixture();
    let key = "a".repeat(32);
    let browser = a.diagnostics.browser(&key).unwrap();
    let (sender, mut requests) = mpsc::channel(1);
    browser.lock().unwrap().reports = Some(sender);
    assert_ne!(
        submit(
            State(a.clone()),
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
            Path("f".repeat(32)),
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
    assert_ne!(
        submit(State(a.clone()), Path(key), headers(&a), Json(input()))
            .await
            .status(),
        StatusCode::OK
    );
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
