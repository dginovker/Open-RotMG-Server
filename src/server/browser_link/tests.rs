use super::*;

async fn value(response: Response) -> serde_json::Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap()
}
fn headers(a: &App, cookie: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("origin", a.origin.parse().unwrap());
    if let Some(cookie) = cookie {
        h.insert("cookie", cookie.parse().unwrap());
    }
    h
}
async fn browser_tab(a: &App, old: Option<&str>) -> (String, String) {
    let tab = random();
    let response = super::super::chars(
        State(a.clone()),
        ConnectInfo("127.0.0.1:1".parse().unwrap()),
        headers(a, old),
        Query(HashMap::from([("browser".into(), tab.clone())])),
        Form(HashMap::from([
            (
                "guid".into(),
                format!("browser-{:016x}", rand::random::<u64>()),
            ),
            ("password".into(), String::new()),
        ])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = if let Some(cookie) = old {
        cookie.to_owned()
    } else {
        let header = response.headers()["set-cookie"].to_str().unwrap();
        assert!(
            header.contains("HttpOnly")
                && header.contains("SameSite=Lax")
                && header.contains("Path=/")
        );
        header.split(';').next().unwrap().to_owned()
    };
    (cookie, tab)
}
async fn native_login(a: &App, cookie: &str, tab: &str, email: &str, password: &str) -> Response {
    super::super::chars(
        State(a.clone()),
        ConnectInfo("127.0.0.1:1".parse().unwrap()),
        headers(a, Some(cookie)),
        Query(HashMap::from([("browser".into(), tab.into())])),
        Form(HashMap::from([
            ("guid".into(), email.into()),
            ("password".into(), password.into()),
        ])),
    )
    .await
}
fn configure(a: &mut App, provider: &str) {
    let mut oauth = OAuth::new("12345678901234567".into(), "fixture-secret".into()).unwrap();
    oauth.authorize = format!("{provider}/authorize");
    oauth.exchange = format!("{provider}/token");
    oauth.user = format!("{provider}/user");
    a.browser_links = Arc::new(Service {
        oauth: Some(oauth),
        ..Service::default()
    });
}
fn private_headers(a: &App) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        "authorization",
        format!("Bearer {}", a.discord_link_token.as_ref().unwrap())
            .parse()
            .unwrap(),
    );
    h
}
async fn private_request(a: &App, path: &str, body: serde_json::Value) -> serde_json::Value {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}{path}", listener.local_addr().unwrap());
    let router = super::super::routes(a, PathBuf::new());
    let serving = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let response = reqwest::Client::new()
        .post(url)
        .headers(private_headers(a))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json().await.unwrap();
    serving.abort();
    body
}
async fn link_status(a: &App, token: &str) -> serde_json::Value {
    private_request(
        a,
        "/internal/discord/link/status",
        serde_json::json!({"token":token}),
    )
    .await
}
async fn unlink_caller(a: &App, discord: &str) -> serde_json::Value {
    private_request(
        a,
        "/internal/discord/unlink",
        serde_json::json!({"discord_id":discord}),
    )
    .await
}
async fn grant(a: &App, discord: &str) -> String {
    let h = private_headers(a);
    let v = value(
        issue(
            State(a.clone()),
            ConnectInfo("127.0.0.1:1".parse().unwrap()),
            h,
            Json(DiscordRequest {
                discord_id: discord.into(),
            }),
        )
        .await,
    )
    .await;
    v["url"].as_str().unwrap().split('#').nth(1).unwrap().into()
}
async fn begin(a: &App, cookie: &str, token: &str) -> serde_json::Value {
    value(
        start(
            State(a.clone()),
            headers(a, Some(cookie)),
            Json(StartRequest {
                token: token.into(),
            }),
        )
        .await,
    )
    .await
}
async fn authorization_response(a: &App, cookie: &str, auth: &serde_json::Value) -> Response {
    let url = reqwest::Url::parse(auth["url"].as_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    callback(
        State(a.clone()),
        headers(a, Some(cookie)),
        Query(Callback {
            state: Some(state),
            code: Some("fixture-code".into()),
            iss: Some("https://discord.com".into()),
            error: None,
        }),
    )
    .await
}
async fn finish(a: &App, cookie: &str, auth: &serde_json::Value) -> serde_json::Value {
    let response = authorization_response(a, cookie, auth).await;
    let mut location =
        reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
    let fragment = location.fragment().unwrap().to_owned();
    let Some(receipt) = fragment.strip_prefix("result=") else {
        location.set_query(Some(&fragment));
        let error = location
            .query_pairs()
            .find(|(key, _)| key == "error")
            .unwrap()
            .1
            .into_owned();
        return serde_json::json!({"error":error});
    };
    value(
        complete(
            State(a.clone()),
            headers(a, Some(cookie)),
            Json(StartRequest {
                token: receipt.into(),
            }),
        )
        .await,
    )
    .await
}
async fn provider(discord: &str) -> (String, tokio::task::JoinHandle<()>) {
    let discord = discord.to_string();
    let router=Router::new().route("/token",post(|Form(form):Form<HashMap<String,String>>|async move {
        assert_eq!(form["client_secret"],"fixture-secret");assert_eq!(form["code"],"fixture-code");
        Json(serde_json::json!({"access_token":"fixture-access","token_type":"Bearer","scope":"identify"}))
    })).route("/user",get(move|headers:HeaderMap|{let id=discord.clone();async move{assert_eq!(headers["authorization"],"Bearer fixture-access");Json(serde_json::json!({"id":id}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (origin, task)
}

#[tokio::test]
async fn native_login_logout_switch_and_close_bind_only_verified_browser_accounts() {
    let (dir, a) = super::super::account_actions::tests::fixture();
    let first = a
        .store
        .register(
            "first-browser".into(),
            "first@example.test".into(),
            "test-password".into(),
        )
        .await
        .unwrap();
    let second = a
        .store
        .register(
            "second-browser".into(),
            "second@example.test".into(),
            "test-password".into(),
        )
        .await
        .unwrap();
    let (cookie, tab) = browser_tab(&a, None).await;
    assert!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        native_login(&a, &cookie, &tab, "first@example.test", "test-password")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .unwrap()
            .unwrap()
            .account,
        first
    );
    native_login(&a, &cookie, &tab, "wrong@example.test", "wrong-password").await;
    assert!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .unwrap()
            .is_none()
    );
    native_login(&a, &cookie, &tab, "second@example.test", "test-password").await;
    assert_eq!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .unwrap()
            .unwrap()
            .account,
        second
    );
    a.browser_links.registry.lock().unwrap().browsers.clear();
    let heartbeat = browser(
        State(a.clone()),
        ConnectInfo("127.0.0.1:1".parse().unwrap()),
        headers(&a, Some(&cookie)),
        Json(BrowserRequest { tab: tab.clone() }),
    )
    .await;
    assert_eq!(heartbeat.status(), StatusCode::OK);
    assert!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .unwrap()
            .is_none()
    );
    native_login(&a, &cookie, &tab, "second@example.test", "test-password").await;
    assert_eq!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .unwrap()
            .unwrap()
            .account,
        second
    );
    let (_, other) = browser_tab(&a, Some(&cookie)).await;
    native_login(&a, &cookie, &other, "first@example.test", "test-password").await;
    assert!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .is_err()
    );
    close(
        State(a.clone()),
        headers(&a, Some(&cookie)),
        Json(BrowserRequest { tab: other }),
    )
    .await;
    native_login(&a, &cookie, &tab, "signed-out-guest", "").await;
    assert!(
        a.browser_links
            .browser(&headers(&a, Some(&cookie)))
            .unwrap()
            .lock()
            .await
            .identity()
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn automatic_link_requires_both_identities_and_enables_recovery_without_another_prompt() {
    let (dir, mut a) = super::super::account_actions::tests::fixture();
    let (origin, provider) = provider("12345678901234567").await;
    configure(&mut a, &origin);
    a.store
        .register(
            "flow-browser".into(),
            "flow@example.test".into(),
            "test-password".into(),
        )
        .await
        .unwrap();
    let token = grant(&a, "12345678901234567").await;
    for _ in 0..25 {
        assert_eq!(link_status(&a, &token).await["state"], "pending");
    }
    let pending = a.browser_links.registry.lock().unwrap().pending[&hash(&token)].clone();
    pending.used.store(1, Ordering::SeqCst);
    assert_eq!(
        link_status(&a, &token).await["state"],
        "pending",
        "A claim is not committed success"
    );
    pending.used.store(0, Ordering::SeqCst);
    let (cookie, tab) = browser_tab(&a, None).await;
    assert_eq!(begin(&a, &cookie, &token).await["state"], "signed_out");
    native_login(&a, &cookie, &tab, "flow@example.test", "test-password").await;
    let auth = begin(&a, &cookie, &token).await;
    assert_eq!(auth["state"], "authorize");
    let response = authorization_response(&a, &cookie, &auth).await;
    let location = reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
    let receipt = location
        .fragment()
        .unwrap()
        .strip_prefix("result=")
        .unwrap();
    assert!(finish(&a, &cookie, &auth).await["error"].is_string());
    let confirmed = value(
        complete(
            State(a.clone()),
            headers(&a, Some(&cookie)),
            Json(StartRequest {
                token: receipt.into(),
            }),
        )
        .await,
    )
    .await;
    assert!(
        confirmed["name"].is_string(),
        "A replayed callback must not overwrite the successful completion receipt: {confirmed}"
    );
    a.store
        .issue_reset("12345678901234567".into(), None)
        .await
        .unwrap();
    assert!(
        begin(&a, &cookie, &token).await["error"]
            .as_str()
            .unwrap()
            .contains("already used")
    );
    assert!(finish(&a, &cookie, &auth).await["error"].is_string());
    assert_eq!(link_status(&a, &token).await["state"], "linked");
    let new_grant = grant(&a, "12345678901234567").await;
    let auth = begin(&a, &cookie, &new_grant).await;
    let response = authorization_response(&a, &cookie, &auth).await;
    let location = reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
    let receipt = location
        .fragment()
        .unwrap()
        .strip_prefix("result=")
        .unwrap()
        .to_owned();
    // The browser can retain its receipt after the expired grant registry was pruned.
    a.browser_links.registry.lock().unwrap().pending.clear();
    assert_eq!(
        unlink_caller(&a, "12345678901234567").await["unlinked"],
        true
    );
    assert!(
        value(
            complete(
                State(a.clone()),
                headers(&a, Some(&cookie)),
                Json(StartRequest { token: receipt })
            )
            .await
        )
        .await["error"]
            .is_string()
    );
    assert_eq!(link_status(&a, &token).await["state"], "expired");
    assert!(
        a.store
            .issue_reset("12345678901234567".into(), None)
            .await
            .is_err()
    );
    provider.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn forwarded_link_cannot_give_the_issuers_discord_recovery_of_a_victims_game() {
    let (dir, mut a) = super::super::account_actions::tests::fixture();
    let (origin, provider) = provider("12345678901234567").await;
    configure(&mut a, &origin);
    a.store
        .register(
            "victim-browser".into(),
            "victim@example.test".into(),
            "test-password".into(),
        )
        .await
        .unwrap();
    let token = grant(&a, "12345678901234568").await;
    let (cookie, tab) = browser_tab(&a, None).await;
    native_login(&a, &cookie, &tab, "victim@example.test", "test-password").await;
    let auth = begin(&a, &cookie, &token).await;
    assert!(finish(&a, &cookie, &auth).await["error"].is_string());
    assert!(
        a.store
            .linked_account("12345678901234568".into())
            .await
            .is_err()
    );
    assert!(
        a.store
            .issue_reset("12345678901234568".into(), None)
            .await
            .is_err()
    );
    provider.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn logout_reset_cookie_state_and_challenge_rotation_invalidate_approval() {
    let (dir, mut a) = super::super::account_actions::tests::fixture();
    let (origin, provider) = provider("12345678901234567").await;
    configure(&mut a, &origin);
    let account = a
        .store
        .register(
            "race-browser".into(),
            "race@example.test".into(),
            "test-password".into(),
        )
        .await
        .unwrap();
    let token = grant(&a, "12345678901234567").await;
    let (cookie, tab) = browser_tab(&a, None).await;
    native_login(&a, &cookie, &tab, "race@example.test", "test-password").await;
    let auth = begin(&a, &cookie, &token).await;
    let (other_cookie, _) = browser_tab(&a, None).await;
    assert!(finish(&a, &other_cookie, &auth).await["error"].is_string());
    native_login(&a, &cookie, &tab, "logout-guest", "").await;
    assert!(finish(&a, &cookie, &auth).await["error"].is_string());
    native_login(&a, &cookie, &tab, "race@example.test", "test-password").await;
    let auth = begin(&a, &cookie, &token).await;
    let replacement = grant(&a, "12345678901234567").await;
    assert!(finish(&a, &cookie, &auth).await["error"].is_string());
    let auth = begin(&a, &cookie, &replacement).await;
    let verifier = a
        .store
        .prepare_password("new-password".into())
        .await
        .unwrap();
    a.store.change_password(account, verifier).await.unwrap();
    assert!(finish(&a, &cookie, &auth).await["error"].is_string());
    assert!(
        a.store
            .linked_account("12345678901234567".into())
            .await
            .is_err()
    );
    provider.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn expired_grants_cross_origin_posts_and_forged_completion_are_rejected() {
    let (dir, mut a) = super::super::account_actions::tests::fixture();
    configure(&mut a, "http://unused.invalid");
    let token = grant(&a, "12345678901234567").await;
    let (cookie, _) = browser_tab(&a, None).await;
    let denied = browser(
        State(a.clone()),
        ConnectInfo("127.0.0.1:1".parse().unwrap()),
        HeaderMap::new(),
        Json(BrowserRequest { tab: random() }),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied = value(
        start(
            State(a.clone()),
            HeaderMap::new(),
            Json(StartRequest {
                token: token.clone(),
            }),
        )
        .await,
    )
    .await;
    assert!(denied["error"].is_string());
    let denied = value(
        complete(
            State(a.clone()),
            headers(&a, Some(&cookie)),
            Json(StartRequest {
                token: token.clone(),
            }),
        )
        .await,
    )
    .await;
    assert!(denied["error"].is_string());
    {
        let mut registry = a.browser_links.registry.lock().unwrap();
        Arc::get_mut(registry.pending.get_mut(&hash(&token)).unwrap())
            .unwrap()
            .expires = Instant::now();
    }
    assert!(begin(&a, &cookie, &token).await["error"].is_string());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn superseded_in_flight_oauth_cannot_link_the_previous_discord_identity() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let router = Router::new()
        .route("/token", post(|| async { Json(serde_json::json!({"access_token":"fixture-access","token_type":"Bearer","scope":"identify"})) }))
        .route("/user", get({
            let entered = entered.clone(); let release = release.clone();
            move || {
                let entered = entered.clone(); let release = release.clone(); let requests = requests.clone();
                async move {
                    let first = requests.fetch_add(1, Ordering::SeqCst) == 0;
                    if first { entered.notify_one(); release.notified().await; }
                    Json(serde_json::json!({"id":if first {"12345678901234567"} else {"12345678901234568"}}))
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let provider = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (dir, mut a) = super::super::account_actions::tests::fixture();
    configure(&mut a, &origin);
    a.store
        .register(
            "superseded-browser".into(),
            "superseded@example.test".into(),
            "test-password".into(),
        )
        .await
        .unwrap();
    let (cookie, tab) = browser_tab(&a, None).await;
    native_login(
        &a,
        &cookie,
        &tab,
        "superseded@example.test",
        "test-password",
    )
    .await;
    let old = begin(&a, &cookie, &grant(&a, "12345678901234567").await).await;
    let callback = tokio::spawn({
        let a = a.clone();
        let cookie = cookie.clone();
        async move { finish(&a, &cookie, &old).await }
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let new = begin(&a, &cookie, &grant(&a, "12345678901234568").await).await;
    release.notify_one();
    let old = tokio::time::timeout(Duration::from_secs(2), callback)
        .await
        .unwrap()
        .unwrap();
    assert!(
        old["error"].is_string(),
        "A superseded in-flight authorization must not link: {old}"
    );
    assert!(
        a.store
            .linked_account("12345678901234567".into())
            .await
            .is_err()
    );
    assert!(finish(&a, &cookie, &new).await["name"].is_string());
    a.store
        .issue_reset("12345678901234568".into(), None)
        .await
        .unwrap();
    provider.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn unlink_fences_a_delayed_oauth_callback_and_private_routes_reject_public_callers() {
    for linked in [false, true] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let router = Router::new()
        .route("/token", post(|| async { Json(serde_json::json!({"access_token":"fixture-access","token_type":"Bearer","scope":"identify"})) }))
        .route("/user", get({
            let entered=entered.clone(); let release=release.clone();
            move || { let entered=entered.clone(); let release=release.clone(); async move {
                entered.notify_one(); release.notified().await;
                Json(serde_json::json!({"id":"12345678901234567"}))
            }}
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let provider = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let (dir, mut a) = super::super::account_actions::tests::fixture();
        configure(&mut a, &origin);
        let account = a
            .store
            .register(
                "unlink-race".into(),
                "unlink@example.test".into(),
                "test-password".into(),
            )
            .await
            .unwrap();
        if linked {
            a.store
                .link_browser_discord(
                    a.store.browser_identity(account).await.unwrap(),
                    "12345678901234568".into(),
                )
                .await
                .unwrap();
        }
        let (cookie, tab) = browser_tab(&a, None).await;
        native_login(&a, &cookie, &tab, "unlink@example.test", "test-password").await;
        let token = grant(&a, "12345678901234567").await;
        let auth = begin(&a, &cookie, &token).await;
        let callback = tokio::spawn({
            let a = a.clone();
            let cookie = cookie.clone();
            async move { finish(&a, &cookie, &auth).await }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        assert_eq!(
            unlink_caller(
                &a,
                if linked {
                    "12345678901234568"
                } else {
                    "12345678901234567"
                }
            )
            .await["unlinked"],
            linked
        );
        release.notify_one();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), callback)
                .await
                .unwrap()
                .unwrap()["error"]
                .is_string()
        );
        assert!(
            a.store
                .linked_account("12345678901234567".into())
                .await
                .is_err()
        );
        for response in [
            status(
                State(a.clone()),
                ConnectInfo("127.0.0.1:1".parse().unwrap()),
                HeaderMap::new(),
                Json(StartRequest {
                    token: token.clone(),
                }),
            )
            .await,
            unlink(
                State(a.clone()),
                ConnectInfo("127.0.0.1:1".parse().unwrap()),
                HeaderMap::new(),
                Json(DiscordRequest {
                    discord_id: "12345678901234567".into(),
                }),
            )
            .await,
            unlink(
                State(a.clone()),
                ConnectInfo("192.0.2.1:1".parse().unwrap()),
                private_headers(&a),
                Json(DiscordRequest {
                    discord_id: "12345678901234567".into(),
                }),
            )
            .await,
        ] {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        provider.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
