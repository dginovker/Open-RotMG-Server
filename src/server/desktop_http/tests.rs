use super::*;
use crate::server::{account_actions, lifecycle_finalize_tests::NativeSocket};
use axum::http::StatusCode;
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const GENERATION: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const MODIFIED: &str = "Sat, 01 Jan 2000 00:00:00 GMT";
const FUTURE: &str = "Thu, 01 Jan 2099 00:00:00 GMT";
const FILES: &[(&str, &str)] = &[
    ("index.html", "text/html"),
    ("launcher.js", "javascript"),
    ("draw.html", "text/html"),
    ("native-texture-editor.swf", "application/x-shockwave-flash"),
    ("vendor/ruffle.js", "javascript"),
    ("vendor/ruffle_bg.wasm", "application/wasm"),
];
fn write(path: &Path, body: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(946684800)),
        )
        .unwrap();
}
fn fixture() -> (PathBuf, App) {
    let (dir, mut app) = account_actions::tests::fixture();
    for (name, _) in FILES {
        write(
            &dir.join("client").join(name),
            format!("synthetic cache fixture {name}: full selected body").as_bytes(),
        );
    }
    for name in ["spectator.js", "spectator.html", "spectator.swf"] {
        write(&dir.join("client").join(name), b"synthetic CCTV fixture");
    }
    app.sfx = dir.join("sfx");
    write(
        &app.sfx.join("fixture.ogg"),
        b"synthetic audio full selected body",
    );
    app.swf = std::sync::Arc::new(b"synthetic server-patched SWF fixture".to_vec());
    (dir, app)
}
async fn start(
    a: &App,
    dir: &Path,
    desktop: bool,
    generation: Option<&str>,
) -> (String, u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = crate::server::routes(a, dir.join("client"), desktop, generation);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://127.0.0.1:{port}"), port, server)
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
fn assets(dir: &Path, app: &App) -> Vec<(String, Vec<u8>, String)> {
    let mut assets = FILES
        .iter()
        .map(|(name, mime)| {
            (
                name.to_string(),
                std::fs::read(dir.join("client").join(name)).unwrap(),
                mime.to_string(),
            )
        })
        .collect::<Vec<_>>();
    assets.extend([
        (
            "client.swf".into(),
            app.swf.as_ref().clone(),
            "application/x-shockwave-flash".into(),
        ),
        (
            "sfx/fixture.ogg".into(),
            std::fs::read(app.sfx.join("fixture.ogg")).unwrap(),
            "audio/ogg".into(),
        ),
    ]);
    assets
}

#[tokio::test]
async fn desktop_namespace_serves_selected_game_assets_and_preserves_root_services() {
    let (dir, app) = fixture();
    let account = app
        .store
        .register(
            "cache-guid".into(),
            "cache@example.invalid".into(),
            "synthetic-cache-password".into(),
        )
        .await
        .unwrap();
    assert!(account > 0);
    let (url, port, server) = start(&app, &dir, true, Some(GENERATION)).await;
    let client = client();
    let prefix = format!("/_runtime/{GENERATION}/");
    for path in ["/".to_string(), prefix.trim_end_matches('/').to_string()] {
        let response = client.get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(response.headers()["location"], prefix);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let response = client.get(format!("{url}{prefix}")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap(),
        std::fs::read(dir.join("client/index.html")).unwrap()
    );
    for (path, body, mime) in assets(&dir, &app) {
        let response = client
            .get(format!("{url}{prefix}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains(&mime),
            "{path}"
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.bytes().await.unwrap(), body);
        let root = client.get(format!("{url}/{path}")).send().await.unwrap();
        assert_eq!(root.status(), StatusCode::NOT_FOUND, "unqualified {path}");
        assert_eq!(root.headers()["cache-control"], "no-store");
    }
    for path in [
        format!("/_runtime/{}/launcher.js", "f".repeat(64)),
        format!("{prefix}missing.js"),
        format!("{prefix}vendor/missing.wasm"),
    ] {
        let response = client.get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let response = client
        .get(format!("{url}{prefix}launcher.js"))
        .header("range", "bytes=0-2")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let health: serde_json::Value = client
        .get(format!("{url}/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["process"], std::process::id());
    let verified = client
        .post(format!("{url}/legacy-api/account/verify"))
        .header("origin", &app.origin)
        .form(&[
            ("guid", "cache@example.invalid"),
            ("password", "synthetic-cache-password"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(verified.status(), StatusCode::OK);
    assert_eq!(verified.text().await.unwrap(), "<Account/>");
    let socket = NativeSocket::connect(port, &app.origin).await;
    drop(socket);
    assert_eq!(
        client
            .get(format!("{url}/api/bots/spectator.js"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    server.abort();
}

#[tokio::test]
async fn desktop_get_head_strip_stale_future_conditions_and_ranges_with_or_without_namespace() {
    let (dir, app) = fixture();
    for generation in [None, Some(GENERATION)] {
        let (url, _, server) = start(&app, &dir, true, generation).await;
        let prefix = entry(generation);
        for (path, body, mime) in assets(&dir, &app) {
            for method in [reqwest::Method::GET, reqwest::Method::HEAD] {
                for modified in [MODIFIED, FUTURE] {
                    let response = client()
                        .request(method.clone(), format!("{url}{prefix}{path}"))
                        .header("if-modified-since", modified)
                        .header("if-none-match", "*")
                        .header("if-match", "\"wrong-etag\"")
                        .header("if-unmodified-since", "Thu, 01 Jan 1970 00:00:00 GMT")
                        .header("if-range", MODIFIED)
                        .header("range", "bytes=1-3")
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), StatusCode::OK, "{path}");
                    assert_eq!(response.headers()["cache-control"], "no-store");
                    assert!(!response.headers().contains_key("content-range"));
                    assert!(
                        response.headers()["content-type"]
                            .to_str()
                            .unwrap()
                            .contains(&mime)
                    );
                    assert_eq!(
                        response.headers()["content-length"]
                            .to_str()
                            .unwrap()
                            .parse::<usize>()
                            .unwrap(),
                        body.len()
                    );
                    let bytes = response.bytes().await.unwrap();
                    assert_eq!(
                        bytes.as_ref(),
                        if method == reqwest::Method::HEAD {
                            b"".as_slice()
                        } else {
                            body.as_slice()
                        }
                    );
                }
            }
        }
        let response = client()
            .get(format!("{url}{prefix}launcher.js"))
            .header("range", "bytes=9999999-")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.bytes().await.unwrap(),
            std::fs::read(dir.join("client/launcher.js")).unwrap()
        );
        server.abort();
    }
}

#[tokio::test]
async fn web_root_validators_ranges_errors_and_cctv_static_semantics_are_unchanged() {
    let (dir, app) = fixture();
    let (url, _, server) = start(&app, &dir, false, None).await;
    for path in ["/launcher.js", "/api/bots/spectator.js"] {
        let response = client().get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["last-modified"], MODIFIED);
        assert!(!response.headers().contains_key("cache-control"));
        for modified in [MODIFIED, FUTURE] {
            let response = client()
                .get(format!("{url}{path}"))
                .header("if-modified-since", modified)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
            assert!(response.bytes().await.unwrap().is_empty());
        }
    }
    for method in [reqwest::Method::GET, reqwest::Method::HEAD] {
        let response = client()
            .request(method.clone(), format!("{url}/launcher.js"))
            .header("range", "bytes=1-3")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert!(response.headers().contains_key("content-range"));
        let bytes = response.bytes().await.unwrap();
        assert_eq!(
            bytes.as_ref(),
            if method == reqwest::Method::HEAD {
                b""
            } else {
                &b"synthetic"[1..4]
            }
        );
    }
    assert_eq!(
        client()
            .get(format!("{url}/launcher.js"))
            .header("range", "bytes=9999999-")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    assert_eq!(
        client()
            .post(format!("{url}/launcher.js"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        client()
            .get(format!("{url}/missing.js"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    server.abort();
}

#[tokio::test]
async fn synthetic_changed_body_uses_new_namespace_despite_identical_file_mtime() {
    let (dir, app) = fixture();
    let path = dir.join("client/launcher.js");
    write(&path, b"synthetic generation A body");
    let (url, _, server) = start(&app, &dir, true, Some(GENERATION)).await;
    let response = client()
        .get(format!("{url}{}launcher.js", entry(Some(GENERATION))))
        .send()
        .await
        .unwrap();
    let modified = response.headers()["last-modified"].clone();
    assert_eq!(
        response.bytes().await.unwrap(),
        b"synthetic generation A body".as_slice()
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    write(&path, b"synthetic generation B genuinely different body");
    let next = "b".repeat(64);
    let (url, _, server) = start(&app, &dir, true, Some(&next)).await;
    let old = client()
        .get(format!("{url}{}launcher.js", entry(Some(GENERATION))))
        .send()
        .await
        .unwrap();
    assert_eq!(old.status(), StatusCode::NOT_FOUND);
    let response = client()
        .get(format!("{url}{}launcher.js", entry(Some(&next))))
        .header("if-modified-since", modified)
        .header("range", "bytes=0-1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["last-modified"], MODIFIED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.bytes().await.unwrap(),
        b"synthetic generation B genuinely different body".as_slice()
    );
    server.abort();
}
