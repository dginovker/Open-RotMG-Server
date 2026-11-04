use super::*;
use crate::server::lifecycle_finalize_tests::{NativeSocket, socket_hello_password};
use rusqlite::{Connection, types::Value};

const EMAIL: &str = "export-owner-canary@example.invalid";
const PASSWORD: &str = "export-password-canary-7381";
const GUID: &str = "export-guid-canary-8421";
const PEER_PASSWORD: &str = "peer-password-canary-8821";

async fn serve(a: &App) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = super::super::routes(a, PathBuf::from("../client"), false, None);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (url, server)
}
async fn fixture() -> (PathBuf, App, i32, crate::store::Character) {
    let (dir, app) = account_actions::tests::fixture();
    let account = app
        .store
        .register(GUID.into(), EMAIL.into(), PASSWORD.into())
        .await
        .unwrap();
    let ch = app
        .store
        .create(account, app.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    (dir, app, account, ch)
}
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
async fn export(url: &str) -> reqwest::Response {
    client()
        .post(format!("{url}/api/account/export"))
        .json(&serde_json::json!({"email":EMAIL,"password":PASSWORD}))
        .send()
        .await
        .unwrap()
}
async fn failure(response: reqwest::Response, status: StatusCode) -> String {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["content-type"], "application/json");
    assert!(!response.headers().contains_key("content-disposition"));
    let bytes = response.bytes().await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 1);
    let error = value["error"].as_str().unwrap().to_string();
    for secret in [
        EMAIL,
        PASSWORD,
        PEER_PASSWORD,
        GUID,
        "$argon2",
        "SELECT ",
        "INSERT ",
        "password_hash",
    ] {
        assert!(!String::from_utf8_lossy(&bytes).contains(secret));
    }
    error
}
fn dump(db: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
    let mut result = vec![(
        "version".into(),
        vec![vec![Value::Integer(
            db.query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
        )]],
    )];
    for table in std::iter::once("sqlite_schema".to_string()).chain(db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()) {
        let mut q = db.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
        let count = q.column_count();
        result.push((table,q.query_map([],|r|(0..count).map(|i|r.get(i)).collect::<rusqlite::Result<Vec<Value>>>()).unwrap().collect::<rusqlite::Result<_>>().unwrap()));
    }
    result
}
async fn saved(response: reqwest::Response) -> crate::store::account_save::AccountSave {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["content-type"], "application/json");
    let disposition = response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_string();
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() as u64 <= crate::store::account_save::MAX_BYTES);
    for secret in [
        EMAIL,
        PASSWORD,
        PEER_PASSWORD,
        GUID,
        "$argon2",
        "discord-canary",
        "recovery-canary",
        "session-canary",
    ] {
        assert!(!String::from_utf8_lossy(&bytes).contains(secret));
    }
    let save = crate::store::account_save::AccountSave::decode(&bytes).unwrap();
    assert_eq!(
        disposition,
        format!(
            "attachment; filename=\"openrotmg-account-{}.json\"",
            save.exported_at
        )
    );
    save
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_export_http_strict_auth_privacy_readonly_and_account_isolation() {
    let (dir, a, account, ch) = fixture().await;
    let peer = a
        .store
        .register(
            "peer-export-guid".into(),
            "peer-export@example.invalid".into(),
            PEER_PASSWORD.into(),
        )
        .await
        .unwrap();
    let peerch = a
        .store
        .create(peer, a.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let guest = a
        .store
        .list("unregistered-export-guest".into())
        .await
        .unwrap();
    let db = Connection::open(dir.join("game.db")).unwrap();
    db.execute(
        "INSERT INTO discord_links(account,discord_id) VALUES(?1,'discord-canary')",
        [account],
    )
    .unwrap();
    db.execute("INSERT INTO recovery_codes(account,hash,issued_at,expires_at) VALUES(?1,'recovery-canary',1,2147483647)",[account]).unwrap();
    let before = dump(&db);
    let main_before = std::fs::read(dir.join("game.db")).unwrap();
    let wal_before = std::fs::read(dir.join("game.db-wal")).unwrap();
    let (url, server) = serve(&a).await;
    for body in [
        serde_json::json!({}),
        serde_json::json!({"email":EMAIL}),
        serde_json::json!({"email":EMAIL,"password":PASSWORD,"account":peer}),
        serde_json::json!({"email":EMAIL,"password":42}),
    ] {
        failure(
            client()
                .post(format!("{url}/api/account/export"))
                .json(&body)
                .send()
                .await
                .unwrap(),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    for (email, password) in [
        (EMAIL, "wrong-password"),
        ("absent@example.invalid", PASSWORD),
        ("peer-export@example.invalid", PASSWORD),
        (EMAIL, PEER_PASSWORD),
        (GUID, ""),
        ("unregistered-export-guest", ""),
        (EMAIL, ""),
    ] {
        failure(
            client()
                .post(format!("{url}/api/account/export"))
                .json(&serde_json::json!({"email":email,"password":password}))
                .send()
                .await
                .unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    failure(
        client()
            .get(format!("{url}/api/account/export"))
            .send()
            .await
            .unwrap(),
        StatusCode::METHOD_NOT_ALLOWED,
    )
    .await;
    failure(
        client()
            .post(format!("{url}/api/account/export?account={peer}"))
            .json(&serde_json::json!({"email":EMAIL,"password":PASSWORD}))
            .send()
            .await
            .unwrap(),
        StatusCode::BAD_REQUEST,
    )
    .await;
    failure(
        client()
            .post(format!("{url}/api/account/export"))
            .header("origin", "https://unapproved.invalid")
            .json(&serde_json::json!({"email":EMAIL,"password":PASSWORD}))
            .send()
            .await
            .unwrap(),
        StatusCode::FORBIDDEN,
    )
    .await;
    failure(
        client()
            .post(format!("{url}/api/account/export"))
            .header("content-type", "application/json")
            .body("{".to_string())
            .send()
            .await
            .unwrap(),
        StatusCode::BAD_REQUEST,
    )
    .await;
    failure(
        client()
            .post(format!("{url}/api/account/export"))
            .header("content-type", "application/json")
            .body(" ".repeat(2049))
            .send()
            .await
            .unwrap(),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    failure(
        client()
            .post(format!("{url}/api/account/export"))
            .body("email=private&password=private")
            .send()
            .await
            .unwrap(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
    )
    .await;
    let own = saved(export(&url).await).await;
    assert_eq!(own.origin.account, account);
    assert_eq!(own.characters.len(), 1);
    assert_eq!(own.characters[0].character.id, ch.id);
    let other = saved(
        client()
            .post(format!("{url}/api/account/export"))
            .header("origin", &a.origin)
            .json(&serde_json::json!({"email":"peer-export@example.invalid","password":PEER_PASSWORD}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(other.origin.account, peer);
    assert_eq!(other.characters[0].character.id, peerch.id);
    assert_eq!(dump(&db), before);
    assert_eq!(std::fs::read(dir.join("game.db")).unwrap(), main_before);
    assert_eq!(std::fs::read(dir.join("game.db-wal")).unwrap(), wal_before);
    assert!(!a.owners.lock().unwrap().contains_key(&guest.id));
    assert_eq!(
        client()
            .post(format!("{url}/api/account/import"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    server.abort();
}
#[tokio::test]
async fn account_export_http_proxy_rate_admission_and_internal_failures_are_private() {
    let (dir, mut a, account, _) = fixture().await;
    a.behind_proxy = true;
    let (url, server) = serve(&a).await;
    failure(export(&url).await, StatusCode::BAD_REQUEST).await;
    let ip = "192.0.2.7";
    for _ in 0..20 {
        failure(
            client()
                .post(format!("{url}/api/account/export"))
                .header("x-forwarded-for", ip)
                .json(&serde_json::json!({"email":EMAIL,"password":"bad-password"}))
                .send()
                .await
                .unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    failure(
        client()
            .post(format!("{url}/api/account/export"))
            .header("x-forwarded-for", ip)
            .json(&serde_json::json!({"email":EMAIL,"password":PASSWORD}))
            .send()
            .await
            .unwrap(),
        StatusCode::TOO_MANY_REQUESTS,
    )
    .await;
    let db = Connection::open(dir.join("game.db")).unwrap();
    db.execute("UPDATE collections SET found_at=0", []).unwrap();
    db.execute(
        "INSERT INTO collections(account,item,found_at) VALUES(?1,2711,0)",
        [account],
    )
    .unwrap();
    failure(
        client()
            .post(format!("{url}/api/account/export"))
            .header("x-forwarded-for", "192.0.2.8")
            .json(&serde_json::json!({"email":EMAIL,"password":PASSWORD}))
            .send()
            .await
            .unwrap(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    a.shutdown.send_replace(true);
    failure(export(&url).await, StatusCode::SERVICE_UNAVAILABLE).await;
    server.abort();
}
async fn socket_load_registered(
    a: &App,
    dir: &std::path::Path,
    url: &str,
    email: &str,
    ch: i32,
) -> (NativeSocket, i32) {
    let port = url.rsplit(':').next().unwrap().parse().unwrap();
    let mut socket = NativeSocket::connect(port, &a.origin).await;
    socket
        .send(socket_hello_password(dir, email, PASSWORD, -2, None))
        .await;
    socket.receive(wire::MAP_INFO).await;
    socket
        .send(Packet {
            id: wire::LOAD,
            body: ch.to_be_bytes().to_vec(),
        })
        .await;
    let packet = socket.receive(wire::CREATE_SUCCESS).await;
    let id = Reader::new(&packet.body).i32().unwrap();
    lifecycle::account(
        a,
        vec![
            a.store
                .authenticate(email.into(), PASSWORD.into())
                .await
                .unwrap(),
        ],
        "account_export_test_loaded",
        |_| async { Ok(()) },
    )
    .await
    .unwrap();
    (socket, id)
}
async fn peer_chat(socket: &mut NativeSocket) {
    let text = "export-peer-still-playing";
    let mut body = Writer::new();
    body.utf(text);
    socket
        .send(Packet {
            id: wire::PLAYER_TEXT,
            body: body.0,
        })
        .await;
    for _ in 0..16 {
        if socket
            .receive(wire::TEXT)
            .await
            .body
            .windows(text.len())
            .any(|b| b == text.as_bytes())
        {
            return;
        }
    }
    panic!("peer chat was not observed");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_export_http_native_finalization_inflight_leave_and_other_player_continue() {
    let (dir, a, account, ch) = fixture().await;
    let peer = a
        .store
        .register(
            "playing-peer".into(),
            "playing-peer@example.invalid".into(),
            PASSWORD.into(),
        )
        .await
        .unwrap();
    let peerch = a
        .store
        .create(peer, a.content.get(WIZARD).unwrap())
        .await
        .unwrap();
    let (url, server) = serve(&a).await;
    let (socket, id) = socket_load_registered(&a, &dir, &url, EMAIL, ch.id).await;
    let (mut other, _) =
        socket_load_registered(&a, &dir, &url, "playing-peer@example.invalid", peerch.id).await;
    failure(export(&url).await, StatusCode::CONFLICT).await;
    let changed = a
        .world
        .for_player(id)
        .call(move |w| {
            let p = w.players.get_mut(&id).unwrap();
            p.character.exp = 37;
            Ok(p.character.clone())
        })
        .await
        .unwrap();
    assert_eq!(a.store.load(account, ch.id).await.unwrap().exp, 0);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let app = a.clone();
    let work = tokio::spawn(async move {
        lifecycle::account(
            &app,
            vec![account],
            "account_export_inflight_progress",
            move |app| async move {
                entered.send(()).unwrap();
                wait.await.unwrap();
                app.store.progress(changed).await
            },
        )
        .await
        .unwrap()
    });
    ready.await.unwrap();
    drop(socket);
    failure(export(&url).await, StatusCode::CONFLICT).await;
    peer_chat(&mut other).await;
    release.send(()).unwrap();
    work.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while a.owners.lock().unwrap().contains_key(&account) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before = dump(&Connection::open(dir.join("game.db")).unwrap());
    let (auth_entered, auth_release) = a.store.lifecycle_test_pause_reply();
    let target = url.clone();
    let download = tokio::spawn(async move { export(&target).await });
    auth_entered.await.unwrap();
    let (snapshot_entered, snapshot_release) = a.store.lifecycle_test_pause_reply();
    auth_release.send(()).unwrap();
    snapshot_entered.await.unwrap();
    peer_chat(&mut other).await;
    assert!(a.owners.lock().unwrap().contains_key(&account));
    snapshot_release.send(()).unwrap();
    let save = saved(download.await.unwrap()).await;
    assert_eq!(save.characters[0].character.exp, 37);
    assert!(lifecycle::drain::open(&a));
    assert!(a.owners.lock().unwrap().contains_key(&peer));
    assert_eq!(
        dump(&Connection::open(dir.join("game.db")).unwrap()),
        before
    );
    drop(other);
    server.abort();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_export_http_snapshot_holds_authentication_and_claim_against_reset_reconnect_and_namechange()
 {
    let (dir, a, account, ch) = fixture().await;
    let discord = "100000000000000001";
    let code = a.store.issue_discord_code(account).await.unwrap();
    a.store
        .redeem_discord_code(code, discord.into())
        .await
        .unwrap();
    let consent = a.store.issue_recovery_code(account).await.unwrap();
    let token = a
        .store
        .issue_reset(discord.into(), Some(consent))
        .await
        .unwrap();
    let (url, server) = serve(&a).await;
    let (auth_entered, auth_release) = a.store.lifecycle_test_pause_reply();
    let target = url.clone();
    let download = tokio::spawn(async move { export(&target).await });
    auth_entered.await.unwrap();
    assert!(a.authentication.try_write().is_err());
    let (snapshot_entered, snapshot_release) = a.store.lifecycle_test_pause_reply();
    auth_release.send(()).unwrap();
    snapshot_entered.await.unwrap();
    assert!(matches!(a.claim_session(account), Err(error) if error.is::<lifecycle::AccountBusy>()));
    let target = url.clone();
    let origin = a.origin.clone();
    let recovery_canary = token.clone();
    let reset = tokio::spawn(async move {
        client()
            .post(format!("{target}/api/account/recover"))
            .header("origin", origin)
            .json(&serde_json::json!({"token":token,"password":"new-export-password"}))
            .send()
            .await
            .unwrap()
    });
    let target = url.clone();
    let rename = tokio::spawn(async move {
        client()
            .post(format!("{target}/legacy-api/account/setName"))
            .form(&[("guid", EMAIL), ("password", PASSWORD), ("name", "Newname")])
            .send()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!reset.is_finished());
    assert!(!rename.is_finished());
    assert!(a.owners.lock().unwrap().contains_key(&account));
    snapshot_release.send(()).unwrap();
    let save = saved(download.await.unwrap()).await;
    assert!(
        !serde_json::to_string(&save)
            .unwrap()
            .contains(&recovery_canary)
    );
    assert_eq!(save.characters[0].character.id, ch.id);
    assert_eq!(
        reset
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()["reset"],
        true
    );
    let _ = rename.await.unwrap();
    failure(export(&url).await, StatusCode::UNAUTHORIZED).await;
    assert_eq!(
        a.store
            .authenticate(EMAIL.into(), "new-export-password".into())
            .await
            .unwrap(),
        account
    );
    assert!(!a.owners.lock().unwrap().contains_key(&account));
    server.abort();
    drop(dir);
}
#[test]
fn account_export_serialization_is_bounded_at_32_mib() {
    let mut writer = Bounded(vec![0; crate::store::account_save::MAX_BYTES as usize - 1]);
    assert_eq!(writer.write(&[1]).unwrap(), 1);
    assert!(writer.write(&[2]).is_err());
    assert_eq!(writer.0.len() as u64, crate::store::account_save::MAX_BYTES);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_export_http_failed_finalization_stays_owned_and_never_exports_stale_progress() {
    let (dir, a, account, ch) = fixture().await;
    a.world.clock.set_paused(true);
    let (url, server) = serve(&a).await;
    let (socket, id) = socket_load_registered(&a, &dir, &url, EMAIL, ch.id).await;
    a.world
        .for_player(id)
        .call(move |w| {
            w.players.get_mut(&id).unwrap().character.exp = 53;
            Ok(())
        })
        .await
        .unwrap();
    let db = Connection::open(dir.join("game.db")).unwrap();
    db.execute_batch("CREATE TRIGGER account_export_failed_save BEFORE UPDATE OF stats ON characters BEGIN SELECT RAISE(ABORT,'injected finalization failure'); END;").unwrap();
    let finalization = lifecycle::finalize(&a, id, CloseReason::Transport).unwrap();
    assert!(finalization.wait().await.is_err());
    drop(socket);
    assert!(a.owners.lock().unwrap().contains_key(&account));
    let before = dump(&db);
    failure(export(&url).await, StatusCode::CONFLICT).await;
    assert_eq!(a.store.load(account, ch.id).await.unwrap().exp, 0);
    assert_eq!(dump(&db), before);
    assert!(a.claim_session(account).is_err());
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_export_http_hash_admission_is_429_not_internal_or_unbounded_work() {
    let (_dir, a, _, _) = fixture().await;
    let (url, server) = serve(&a).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(11));
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let gate = barrier.clone();
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            gate.wait().await;
            export(&url).await
        }));
    }
    barrier.wait().await;
    let mut limited = 0;
    for task in tasks {
        let response = task.await.unwrap();
        match response.status() {
            StatusCode::OK => {
                saved(response).await;
            }
            StatusCode::CONFLICT => {
                failure(response, StatusCode::CONFLICT).await;
            }
            StatusCode::TOO_MANY_REQUESTS => {
                limited += 1;
                failure(response, StatusCode::TOO_MANY_REQUESTS).await;
            }
            status => panic!("unexpected export status {status}"),
        }
    }
    assert!(limited > 0);
    assert!(a.owners.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_export_http_deadline_retains_snapshot_guards_until_supervised_completion() {
    let (dir, a, account, _) = fixture().await;
    let (url, server) = serve(&a).await;
    let (auth_entered, auth_release) = a.store.lifecycle_test_pause_reply();
    let target = url.clone();
    let download = tokio::spawn(async move { export(&target).await });
    auth_entered.await.unwrap();
    let (snapshot_entered, snapshot_release) = a.store.lifecycle_test_pause_reply();
    auth_release.send(()).unwrap();
    snapshot_entered.await.unwrap();
    failure(download.await.unwrap(), StatusCode::GATEWAY_TIMEOUT).await;
    assert!(a.authentication.try_write().is_err());
    assert!(matches!(a.claim_session(account),Err(error) if error.is::<lifecycle::AccountBusy>()));
    let port = url.rsplit(':').next().unwrap().parse().unwrap();
    let mut socket = NativeSocket::connect(port, &a.origin).await;
    socket
        .send(socket_hello_password(&dir, EMAIL, PASSWORD, -2, None))
        .await;
    let reconnect = tokio::spawn(async move {
        socket.receive(wire::MAP_INFO).await;
        socket
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!reconnect.is_finished());
    snapshot_release.send(()).unwrap();
    let socket = reconnect.await.unwrap();
    failure(export(&url).await, StatusCode::CONFLICT).await;
    drop(socket);
    tokio::time::timeout(Duration::from_secs(5), async {
        while a.owners.lock().unwrap().contains_key(&account) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(a.authentication.try_write().is_ok());
    saved(export(&url).await).await;
    server.abort();
}
