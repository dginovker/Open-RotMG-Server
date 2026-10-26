use super::super::lifecycle_finalize_tests::*;
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discord_gold_is_private_and_updates_the_linked_native_players_existing_wallet_once() {
    let (dir, a) = tests::fixture();
    let account = a
        .store
        .register("gold".into(), "gold@example.org".into(), "password".into())
        .await
        .unwrap();
    let code = a.store.issue_discord_code(account).await.unwrap();
    a.store
        .redeem_discord_code(code, "1558662335076376578".into())
        .await
        .unwrap();
    let ch = a
        .store
        .create(account, a.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let (port, _server) = socket_server(&a).await;
    let mut socket = NativeSocket::connect(port, &a.origin).await;
    socket
        .send(socket_hello_password(
            &dir,
            "gold@example.org",
            "password",
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
    let id = Reader::new(&socket.receive(wire::CREATE_SUCCESS).await.body)
        .i32()
        .unwrap();
    let grant = |peer: &str, headers: HeaderMap, target: &str, amount| {
        let app = a.clone();
        let peer = peer.parse().unwrap();
        let discord_id = target.to_owned();
        async move {
            discord_gold(
                State(app),
                ConnectInfo(peer),
                headers,
                Json(GoldRequest {
                    discord_id,
                    amount,
                    request_id: "1558662335076376580".into(),
                    actor_id: "1558662335076376579".into(),
                }),
            )
            .await
        }
    };
    let mut headers = HeaderMap::new();
    assert_eq!(
        grant(
            "127.0.0.1:1234",
            headers.clone(),
            "1558662335076376578",
            100
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    headers.insert(
        "authorization",
        format!("Bearer {}", "a".repeat(64)).parse().unwrap(),
    );
    assert_eq!(
        grant(
            "192.0.2.1:1234",
            headers.clone(),
            "1558662335076376578",
            100
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    headers.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
    assert_eq!(
        grant(
            "127.0.0.1:1234",
            headers.clone(),
            "1558662335076376578",
            100
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    headers.remove("x-forwarded-for");
    for (target, amount) in [
        ("1558662335076376578", 0),
        ("1558662335076376578", -1),
        ("1558662335076376581", 100),
    ] {
        assert_eq!(
            grant("127.0.0.1:1234", headers.clone(), target, amount)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for _ in 0..2 {
        let response = grant(
            "127.0.0.1:1234",
            headers.clone(),
            "1558662335076376578",
            100,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"balance":300})
        );
    }
    assert_eq!(
        a.world
            .for_player(id)
            .call(move |w| Ok(w.players[&id].presentation.credits))
            .await
            .unwrap(),
        300
    );
    assert!(
        a.store
            .characters("gold@example.org".into(), "password".into())
            .await
            .unwrap()
            .xml("", 0.0)
            .contains("<Credits>300</Credits>")
    );
}
