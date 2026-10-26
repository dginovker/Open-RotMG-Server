use super::*;
use sha2::{Digest, Sha256};

pub(super) fn json_error(status: StatusCode, message: impl ToString) -> Response {
    (
        status,
        Json(serde_json::json!({"error":message.to_string()})),
    )
        .into_response()
}
pub(super) fn json_result(result: Result<serde_json::Value>) -> Response {
    let body = match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            tracing::warn!(%error, "game API request rejected");
            json_error(StatusCode::BAD_REQUEST, error)
        }
    };
    let mut response = body;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
pub(super) async fn hiscores(State(a): State<App>) -> Response {
    let result = async {
        let living = a
            .world
            .directory(|d| Ok(d.competitive_characters()))
            .await?;
        let mut scores = a.store.hiscores(living).await?;
        scores["collectible_items"] = serde_json::json!(crate::loot::collectibles(&a.content));
        Ok(scores)
    }
    .await;
    json_result(result)
}
pub(super) async fn graveyard(
    State(a): State<App>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    json_result(
        async {
            let since = query.get("since").map_or(Ok(0), |s| s.parse::<i64>())?;
            a.store.graveyard(since).await
        }
        .await,
    )
}
pub(super) async fn collection(
    State(a): State<App>,
    axum::extract::Path(account): axum::extract::Path<i32>,
) -> Response {
    json_result(a.store.collection(account).await)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinkRequest {
    code: String,
    discord_id: String,
}
pub(super) fn authorize(a: &App, peer: SocketAddr, headers: &HeaderMap) -> Result<()> {
    ensure!(
        peer.ip().is_loopback()
            && ![
                "x-forwarded-for",
                "x-forwarded-host",
                "x-forwarded-proto",
                "forwarded",
                "x-real-ip",
                "origin"
            ]
            .iter()
            .any(|key| headers.contains_key(*key)),
        "link API is private and loopback-only"
    );
    let expected = a
        .discord_link_token
        .as_ref()
        .context("Discord linking is not configured")?;
    let supplied = headers
        .get("authorization")
        .context("link authorization required")?
        .to_str()?
        .strip_prefix("Bearer ")
        .context("link authorization required")?;
    let left = Sha256::digest(expected.as_bytes());
    let right = Sha256::digest(supplied.as_bytes());
    ensure!(
        left.iter()
            .zip(right.iter())
            .fold(0u8, |n, (a, b)| n | (a ^ b))
            == 0,
        "invalid link authorization"
    );
    Ok(())
}
pub(super) fn private_denial(a: &App, peer: SocketAddr, headers: &HeaderMap) -> Option<Response> {
    if authorize(a, peer, headers).is_err() {
        return Some(json_error(
            StatusCode::FORBIDDEN,
            "account API authorization denied",
        ));
    }
    if !rate_allow(
        &mut a.requests.lock().expect("request registry poisoned"),
        peer.ip(),
        std::time::Instant::now(),
    ) {
        return Some(json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "account attempt limit; wait one minute",
        ));
    }
    None
}
pub(super) async fn discord_link(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LinkRequest>,
) -> Response {
    if let Some(response) = private_denial(&a, peer, &headers) {
        return response;
    }
    json_result(
        async {
            let (account, name) = a
                .store
                .redeem_discord_code(request.code, request.discord_id)
                .await?;
            super::gold::sync(&a, account).await?;
            Ok(serde_json::json!({"name":name}))
        }
        .await,
    )
}
