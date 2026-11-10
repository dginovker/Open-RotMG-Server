use super::App;
use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, Method, header},
    middleware::Next,
    response::{Redirect, Response},
    routing::get,
};

pub(super) fn entry(generation: Option<&str>) -> String {
    match generation {
        Some(generation) => format!("/_runtime/{generation}/"),
        None => "/".into(),
    }
}
pub(super) fn namespace(assets: Router<App>, generation: Option<&str>) -> Router<App> {
    let Some(_) = generation else { return assets };
    let entry = entry(generation);
    let target = entry.clone();
    let redirect = get(move || {
        let target = target.clone();
        async move { Redirect::temporary(&target) }
    });
    Router::new()
        .route("/", redirect.clone())
        .route(entry.trim_end_matches('/'), redirect)
        .nest(&entry, assets)
}
pub(super) async fn fresh(mut request: Request, next: Next) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD) {
        // A no-store header on a 304 cannot replace an already cached body.
        for name in [
            header::IF_MODIFIED_SINCE,
            header::IF_NONE_MATCH,
            header::IF_MATCH,
            header::IF_UNMODIFIED_SINCE,
            header::IF_RANGE,
            header::RANGE,
        ] {
            request.headers_mut().remove(name);
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests;
