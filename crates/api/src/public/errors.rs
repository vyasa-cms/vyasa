//! Themed error pages for the public site.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

/// A 404 body, marked never to be stored.
///
/// A 404 carries no validator and no freshness of its own, which leaves a
/// browser free to cache it heuristically. That turns a routing mistake
/// into a lasting one: the URL starts working, and the operator still gets
/// "not found" from their own disk cache with nothing on the server to
/// point at. Whether a page exists is not a fact worth remembering.
fn uncached(body: Html<String>) -> Response {
    (
        StatusCode::NOT_FOUND,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        body,
    )
        .into_response()
}

/// Renders a 404 without the site's theme.
///
/// Kept for the paths that have no [`AppState`] to hand; prefer
/// [`not_found_themed`], which looks like the rest of the site.
pub fn not_found() -> Response {
    match crate::public::routes::render_standalone("not-found.html", "404 — Not Found") {
        Ok(html) => uncached(Html(html)),
        Err(_) => uncached(Html("404 Not Found".to_owned())),
    }
}

/// Renders the 404 through the active theme, falling back to the plain
/// one when no theme is active or the theme cannot render.
pub async fn not_found_themed(state: &crate::state::AppState) -> Response {
    match crate::public::routes::themed_not_found(state).await {
        Some(html) => uncached(Html(html)),
        None => not_found(),
    }
}

/// Fallback handler with state, so an unknown URL gets the themed page.
///
/// The admin is deliberately not special-cased here. It used to be, which
/// read as though it covered /admin/*; it did not, because the public
/// site's own /{type}/{slug} route matched those URLs first and the
/// fallback never ran. The admin now owns /admin and /admin/{*path} as
/// real routes, which is also how it gets its no-cache headers.
pub async fn themed_fallback(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
) -> Response {
    not_found_themed(&state).await
}

/// Turns a 404 for a GET into the 301 an old address was given, when
/// one was. Runs over the whole public router, so it covers the typed
/// routes as well as the fallback.
pub async fn redirect_on_not_found(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let is_get =
        request.method() == axum::http::Method::GET || request.method() == axum::http::Method::HEAD;
    let headers = request.headers().clone();
    let uri = request.uri().clone();
    let path = request.uri().path().to_owned();
    // Keep previously generated default links useful after a setting change.
    if is_get {
        if let Some(slug) = path.strip_prefix("/post/").filter(|s| !s.contains('/')) {
            if let Ok(post) = state
                .posts
                .get_by_slug(vyasa_db::content_models::PostType::Post, slug)
                .await
            {
                let to = crate::permalinks::path(&state, &post).await;
                if post.status == vyasa_db::content_models::PostStatus::Published && to != path {
                    return (
                        StatusCode::MOVED_PERMANENTLY,
                        [(axum::http::header::LOCATION, to)],
                    )
                        .into_response();
                }
            }
        }
    }
    let response = next.run(request).await;
    if !is_get || response.status() != StatusCode::NOT_FOUND {
        return response;
    }
    let pattern = crate::permalinks::pattern(&state).await;
    let markdown = path.strip_suffix(".md").is_some();
    let bare = path.strip_suffix(".md").unwrap_or(&path);
    if let Some(slug) = pattern.slug_from(bare) {
        if let Ok(post) = state
            .posts
            .get_by_slug(vyasa_db::content_models::PostType::Post, slug)
            .await
        {
            if pattern.path_for(&post) == bare
                && post.status == vyasa_db::content_models::PostStatus::Published
            {
                if markdown {
                    return super::routes::markdown_mirror(
                        &state,
                        post.post_type,
                        &post.slug,
                        &headers,
                    )
                    .await;
                }
                let params = axum::extract::Query::<super::routes::EntryParams>::try_from_uri(&uri);
                let Ok(axum::extract::Query(params)) = params else {
                    return StatusCode::BAD_REQUEST.into_response();
                };
                return super::routes::single_impl(
                    state,
                    post.post_type,
                    post.slug,
                    headers,
                    params,
                )
                .await;
            }
        }
    }
    if let Ok(Some(to)) = crate::seo::redirect_for(&state.pool, &path).await {
        if let Ok(value) = axum::http::HeaderValue::from_str(&to) {
            return (
                StatusCode::MOVED_PERMANENTLY,
                [(axum::http::header::LOCATION, value)],
            )
                .into_response();
        }
    }
    response
}

/// Internal error page used by handlers that fail late.
pub fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "500 Internal Server Error",
    )
        .into_response()
}
