//! REST surface of the SEO helpers: redirects, an entry's signals, and
//! outbound link checks.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use vyasa_common::AppError;

use crate::error::{ApiError, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::seo::{self, LinkCheck, Redirect, SeoSignals};
use crate::state::AppState;

/// `GET /api/v1/redirects` — every redirect, newest first.
#[utoipa::path(get, path = "/api/v1/redirects", tag = "seo",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Redirects", body = [Redirect])))]
pub async fn list_redirects(State(state): State<AppState>) -> ApiResult<Json<Vec<Redirect>>> {
    Ok(Json(seo::list_redirects(&state.pool).await?))
}

/// A redirect to write.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct RedirectBody {
    /// Old site-relative path, starting with `/`.
    pub from_path: String,
    /// New site-relative path, starting with `/`.
    pub to_path: String,
}

/// `POST /api/v1/redirects` — write one by hand.
#[utoipa::path(post, path = "/api/v1/redirects", tag = "seo",
    security(("session_cookie" = [])), request_body = RedirectBody,
    responses((status = 204, description = "Written"), (status = 400, description = "Not a site path")))]
pub async fn add_redirect(
    State(state): State<AppState>,
    Json(body): Json<RedirectBody>,
) -> ApiResult<StatusCode> {
    for p in [&body.from_path, &body.to_path] {
        if !is_site_path(p) {
            return Err(ApiError(AppError::validation(
                "paths are site-relative and start with /",
            )));
        }
    }
    seo::add_redirect(&state.pool, &body.from_path, &body.to_path).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A path on this site: starts with `/`, and is not a network-path
/// reference a browser would take to another host. Browsers treat a
/// backslash in a URL as a slash, so `/\host` is refused like `//host`.
///
/// No control character or whitespace anywhere: browsers drop tab, CR and
/// LF from a URL before reading it, so `/<tab>/host` is `//host`.
fn is_site_path(p: &str) -> bool {
    p.starts_with('/')
        && !p.starts_with("//")
        && !p.starts_with("/\\")
        && !p.chars().any(|c| c.is_control() || c.is_whitespace())
        && p.len() <= 500
}

/// `DELETE /api/v1/redirects/{*from}` — remove one.
#[utoipa::path(delete, path = "/api/v1/redirects/{from}", tag = "seo",
    security(("session_cookie" = [])),
    params(("from" = String, Path, description = "The old path, without its leading slash")),
    responses((status = 204, description = "Removed"), (status = 404, description = "No such redirect")))]
pub async fn delete_redirect(
    State(state): State<AppState>,
    Path(from): Path<String>,
) -> ApiResult<StatusCode> {
    if !seo::delete_redirect(&state.pool, &format!("/{from}")).await? {
        return Err(ApiError(AppError::not_found("redirect", from)));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/posts/{id}/seo-signals` — inbound links, rivals, redirects.
#[utoipa::path(get, path = "/api/v1/posts/{id}/seo-signals", tag = "seo",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses((status = 200, description = "Signals", body = SeoSignals)))]
pub async fn signals(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<SeoSignals>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    let site_url = state.options_service.site_url().await.unwrap_or_default();
    Ok(Json(
        seo::signals(&state.pool, &post, Some(site_url.as_str())).await?,
    ))
}

/// `POST /api/v1/posts/{id}/check-links` — fetch every outbound link now.
#[utoipa::path(post, path = "/api/v1/posts/{id}/check-links", tag = "seo",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses((status = 200, description = "What is broken", body = LinkCheck)))]
pub async fn check_links(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<LinkCheck>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    Ok(Json(seo::check_links(&state.pool, &post).await?))
}

/// `GET /api/v1/posts/{id}/link-check` — the last check, if any.
#[utoipa::path(get, path = "/api/v1/posts/{id}/link-check", tag = "seo",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses((status = 200, description = "Last check", body = LinkCheck), (status = 404, description = "Never checked")))]
pub async fn last_check(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<LinkCheck>> {
    policy::post_for_edit(&state, &principal, id).await?;
    match seo::last_check(&state.pool, id).await? {
        Some(c) => Ok(Json(c)),
        None => Err(ApiError(AppError::not_found("link_check", id))),
    }
}

#[cfg(test)]
mod redirect_path_tests {
    use super::is_site_path;

    #[test]
    fn only_paths_on_this_site_are_redirect_targets() {
        assert!(is_site_path("/new-home"));
        assert!(is_site_path("/a/b?c=d"));
        for other_host in ["//evil.example", "https://evil.example", "evil", ""] {
            assert!(!is_site_path(other_host), "{other_host:?}");
        }
        // Browsers read a backslash after the first slash as a second
        // slash, so `/\host` is `//host`.
        for other_host in ["/\\evil.example", "/\\/evil.example", "\\\\evil.example"] {
            assert!(!is_site_path(other_host), "{other_host:?}");
        }
        // Browsers drop tab, CR and LF from a URL before reading it, so a
        // control character or whitespace anywhere could hide `//`.
        for hidden in [
            "/\t/evil.example",
            "/\t\\evil",
            "/\n/evil",
            "/\r//evil",
            "/ /evil",
            "/ok\u{0}",
            "/a b",
            "/\u{7f}/evil",
        ] {
            assert!(!is_site_path(hidden), "{hidden:?}");
        }
        assert!(!is_site_path(&format!("/{}", "a".repeat(500))));
    }
}
