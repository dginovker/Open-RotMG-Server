use super::hiscores::{json_error, private_denial};
use super::*;
use crate::store::UpdateSnapshot;

fn response(result: Result<crate::store::UpdateFeed>) -> Response {
    match result {
        Ok(feed) => ([("cache-control", "no-store")], Json(feed)).into_response(),
        Err(error) => {
            tracing::error!(%error, "update feed storage failed");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Update feed storage failed",
            )
        }
    }
}
pub(super) async fn feed(State(a): State<App>) -> Response {
    response(a.store.updates().await)
}
pub(super) async fn sync(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(snapshot): Json<UpdateSnapshot>,
) -> Response {
    if let Some(denial) = private_denial(&a, peer, &headers) {
        return denial;
    }
    let count = snapshot.posts.len();
    match a.store.sync_updates(snapshot).await {
        Ok(feed) => {
            tracing::info!(
                posts = count,
                synced_at = feed.synced_at,
                "update feed synced"
            );
            response(Ok(feed))
        }
        Err(error) => {
            tracing::warn!(%error, "update feed sync rejected");
            json_error(StatusCode::BAD_REQUEST, error)
        }
    }
}
