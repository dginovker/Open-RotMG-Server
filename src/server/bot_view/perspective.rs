use super::*;
use axum::extract::Query;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct Source {
    source: i32,
}

fn approved(headers: &HeaderMap, origin: &str) -> bool {
    match headers.get("origin") {
        Some(value) => value.to_str().ok() == Some(origin),
        // Same-origin browser GETs omit Origin; cross-site reads must not enumerate the native world.
        None => {
            headers
                .get("sec-fetch-site")
                .and_then(|value| value.to_str().ok())
                == Some("same-origin")
        }
    }
}

pub(in crate::server) async fn get(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((name, target)): Path<(String, i32)>,
    Query(Source { source }): Query<Source>,
) -> Response {
    let result = resolve(&a, peer, headers, &name, source, target).await;
    let (status, body) = match result {
        Ok(bot) => (
            StatusCode::OK,
            serde_json::json!({"bot":bot,"player":target,"source":source}),
        ),
        Err((status, error)) => (status, serde_json::json!({"error":error})),
    };
    (status, [("cache-control", "no-store")], Json(body)).into_response()
}

async fn resolve(
    a: &App,
    peer: SocketAddr,
    mut headers: HeaderMap,
    name: &str,
    source: i32,
    target: i32,
) -> Result<&'static str, (StatusCode, String)> {
    let origin = a.bot_view_origin.as_deref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "native bot viewing is not configured".into(),
    ))?;
    if !approved(&headers, origin) {
        return Err((
            StatusCode::FORBIDDEN,
            "native bot view origin is not approved".into(),
        ));
    }
    if source <= 0 || target <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "source and target must be positive player IDs".into(),
        ));
    }
    headers.remove("origin");
    let ip = request_ip(a, &headers, peer)
        .map_err(|error| (StatusCode::FORBIDDEN, error.to_string()))?;
    if !a.cctv.allow_perspective(ip) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "too many perspective inspections; retry shortly".into(),
        ));
    }
    let missing = || {
        (
            StatusCode::NOT_FOUND,
            "player is not a visible active bot in this perspective".into(),
        )
    };
    let binding = a
        .cctv
        .perspective(name, source, target)
        .ok_or_else(missing)?;
    let visible = tokio::time::timeout(
        Duration::from_secs(2),
        a.world
            .perspective_visible((source, binding.1), (target, binding.2)),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "perspective inspection timed out".into(),
        )
    })?
    .map_err(|error| {
        tracing::warn!(bot=name, %error, "perspective inspection failed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "perspective inspection failed".into(),
        )
    })?;
    if !visible || a.cctv.perspective(name, source, target) != Some(binding) {
        return Err(missing());
    }
    Ok(binding.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn perspective_http_failures_are_explicit_and_not_cacheable() {
        let (directory, mut app) = crate::server::test_app();
        let peer = SocketAddr::from(([127, 0, 0, 1], 40001));
        let mut headers = HeaderMap::new();
        let refused = |result: Result<&'static str, (StatusCode, String)>| result.unwrap_err().0;
        assert_eq!(
            refused(resolve(&app, peer, headers.clone(), "Missing", 1, 2).await),
            StatusCode::FORBIDDEN
        );
        headers.insert("origin", "http://evil.example".parse().unwrap());
        assert_eq!(
            refused(resolve(&app, peer, headers.clone(), "Missing", 1, 2).await),
            StatusCode::FORBIDDEN
        );
        headers.insert(
            "origin",
            app.bot_view_origin.as_ref().unwrap().parse().unwrap(),
        );
        assert_eq!(
            refused(resolve(&app, peer, headers.clone(), "Missing", 0, 2).await),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            refused(resolve(&app, peer, headers.clone(), "Missing", 1, 2).await),
            StatusCode::NOT_FOUND
        );
        headers.remove("origin");
        headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
        let response = get(
            State(app.clone()),
            ConnectInfo(peer),
            headers.clone(),
            Path(("Missing".into(), 2)),
            Query(Source { source: 1 }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
        app.bot_view_origin = None;
        assert_eq!(
            refused(resolve(&app, peer, headers, "Missing", 1, 2).await),
            StatusCode::SERVICE_UNAVAILABLE
        );
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn perspective_origin_is_exact_and_missing_origin_requires_same_origin_browser_metadata() {
        let origin = "https://game.example";
        let mut headers = HeaderMap::new();
        assert!(!approved(&headers, origin));
        headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(approved(&headers, origin));
        for site in ["same-site", "cross-site", "none"] {
            headers.insert("sec-fetch-site", site.parse().unwrap());
            assert!(!approved(&headers, origin));
        }
        headers.insert("origin", origin.parse().unwrap());
        assert!(approved(&headers, origin));
        for value in [
            "null",
            "https://evil.example",
            "https://game.example/",
            "http://game.example",
        ] {
            headers.insert("origin", value.parse().unwrap());
            assert!(!approved(&headers, origin));
        }
    }
}
