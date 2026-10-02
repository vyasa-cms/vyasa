//! `/api/v1/export` and `/api/v1/import`.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiResult;
use crate::export::{self, Archive, ImportReport};
use crate::middleware::Principal;
use crate::state::AppState;

/// Which shape to download.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct ExportQuery {
    /// `json` (round-trips into Vyasa) or `wxr` (WordPress importer).
    pub format: Option<String>,
}

/// `GET /api/v1/export?format=json|wxr` — the whole site's content.
#[utoipa::path(get, path = "/api/v1/export", tag = "content", params(ExportQuery),
    security(("session_cookie" = [])),
    responses((status = 200, description = "The archive, as a download")))]
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ExportQuery>,
) -> ApiResult<(HeaderMap, String)> {
    let site_url = match state.options_service.site_url().await {
        Ok(u) if !u.is_empty() => u,
        _ => crate::feeds::origin_from_headers(&headers),
    };
    let archive = export::build(&state, &site_url).await?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M");
    let mut out = HeaderMap::new();
    let (body, name, mime) = if q.format.as_deref() == Some("wxr") {
        (
            export::to_wxr(&archive, &site_url),
            format!("vyasa-export-{stamp}.xml"),
            "application/xml; charset=utf-8",
        )
    } else {
        (
            serde_json::to_string_pretty(&archive).unwrap_or_default(),
            format!("vyasa-export-{stamp}.json"),
            "application/json; charset=utf-8",
        )
    };
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        out.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok((out, body))
}

/// Import options.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct ImportQuery {
    /// Also apply the archive's site options (title, tagline, …).
    pub options: Option<bool>,
}

/// `POST /api/v1/import` — load a Vyasa JSON archive. Re-running adds
/// nothing: rows are matched by email, slug or import key.
///
/// A full administrator only (`policy::full_administrator`): an archive
/// brings in entries, terms, comments, menus, accounts and, when asked,
/// site options, which is every capability at once.
///
/// Beneath that, roles are created and given only as far as the caller could do so by
/// hand. A user whose role (built-in or custom) holds a capability the
/// caller lacks arrives as a subscriber; custom roles are created, and
/// handed to the users the archive names, only with `manage_users` and
/// when the caller holds every capability in them. What is left out is
/// listed in the report's warnings.
#[utoipa::path(post, path = "/api/v1/import", tag = "content", params(ImportQuery),
    security(("session_cookie" = [])),
    request_body(content = serde_json::Value, description = "A Vyasa JSON archive"),
    responses((status = 200, description = "What was created", body = ImportReport),
              (status = 403, description = "Not a full administrator")))]
pub async fn post(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<ImportQuery>,
    Json(archive): Json<Archive>,
) -> ApiResult<Json<ImportReport>> {
    crate::policy::full_administrator(&principal, "import a site archive")?;
    let report = export::import(
        &state,
        &archive,
        q.options.unwrap_or(false),
        Some(&principal),
    )
    .await?;
    if report.posts > 0 {
        let _ = crate::search_indexer::reindex_all(&state).await;
    }
    // New entries, types and fields change what rendered pages list.
    state
        .surface_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(Json(report))
}
