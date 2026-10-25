use super::hiscores::{json_error, json_result, private_denial};
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscordRequest {
    discord_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoverRequest {
    discord_id: String,
    code: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResetRequest {
    token: String,
    password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnnounceRequest {
    text: String,
}
pub(super) async fn discord_announce(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<AnnounceRequest>,
) -> Response {
    if let Some(response) = private_denial(&a, peer, &headers) {
        return response;
    }
    json_result(
        async {
            let text = request.text.trim();
            ensure!(!text.is_empty(), "Announcement text is empty.");
            ensure!(
                text.encode_utf16().count() <= 200,
                "Announcements are limited to 200 characters."
            );
            ensure!(
                !text.chars().any(char::is_control),
                "Announcement text contains control characters."
            );
            let deliver = text.to_owned();
            let players = a.world.directory(move |d| Ok(d.announce(&deliver))).await?;
            tracing::info!(players, text, "announcement sent");
            Ok(serde_json::json!({"players":players}))
        }
        .await,
    )
}
pub(super) async fn discord_kick(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<DiscordRequest>,
) -> Response {
    if let Some(response) = private_denial(&a, peer, &headers) {
        return response;
    }
    json_result(
        async {
            let account = a.store.linked_account(request.discord_id).await?;
            let disconnected = a.kick(account).await?;
            tracing::info!(account, disconnected, "self-disconnect request completed");
            Ok(serde_json::json!({"disconnected":disconnected}))
        }
        .await,
    )
}
pub(super) async fn discord_recover(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<RecoverRequest>,
) -> Response {
    if let Some(response) = private_denial(&a, peer, &headers) {
        return response;
    }
    let _authentication = a.authentication.read().await;
    json_result(
        a.store
            .issue_reset(request.discord_id, request.code)
            .await
            .map(|token| serde_json::json!({"url":format!("{}/recover/#{token}",a.origin)})),
    )
}
pub(super) async fn reset_password(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<ResetRequest>,
) -> Response {
    if headers.get("origin").and_then(|h| h.to_str().ok()) != Some(a.origin.as_str()) {
        return json_error(StatusCode::FORBIDDEN, "reset request origin denied");
    }
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => {
            return json_error(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many reset attempts; wait one minute",
            );
        }
        Err(e) => return json_result(Err(e)),
    }
    json_result(
        async {
            a.store.reset_account(request.token.clone()).await?;
            let verifier = a.store.prepare_password(request.password).await?;
            // Fence authentication through its ownership claim, not just the credential lookup.
            let authentication = a.authentication.clone().write_owned().await;
            let account = a.store.reset_account(request.token.clone()).await?;
            a.kick(account).await?;
            ensure!(
                !a.owners
                    .lock()
                    .expect("ownership registry poisoned")
                    .contains_key(&account),
                "account ownership was not released"
            );
            let _claim = a.claim(account)?;
            super::lifecycle::account(&a, vec![account], "password_reset", move |a| async move {
                let _authentication = authentication;
                super::lifecycle::stage(Stage::Store);
                a.store.finish_reset(request.token, verifier).await
            })
            .await?;
            Ok(serde_json::json!({"reset":true}))
        }
        .await,
    )
}

#[cfg(test)]
pub(crate) mod tests;
