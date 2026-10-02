//! Full-text search over HTTP, plus index maintenance.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use vyasa_db::content_models::PostStatus;

use crate::error::{ApiErrorBody, ApiResult};
use crate::state::AppState;

/// Query parameters for `GET /api/v1/search`.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct SearchParams {
    /// The search terms.
    pub q: String,
    /// Maximum hits (1..=50, default 10).
    pub limit: Option<usize>,
}

/// `GET /api/v1/search?q=…` — full-text search over published posts.
///
/// Unauthenticated, and published-only: this is the same content the public
/// site serves, so gating it would only push headless clients into scraping
/// the HTML.
///
/// Every hit is re-read through the repository before being returned, so a
/// post that has been unpublished since it was indexed cannot leak through a
/// stale index entry.
#[utoipa::path(
    get, path = "/api/v1/search",
    tag = "search",
    params(SearchParams),
    responses(
        (status = 200, description = "Hits, most relevant first", body = serde_json::Value),
    )
)]
pub async fn search(
    State(state): State<AppState>,
    crate::middleware::principal::MaybePrincipal(principal): crate::middleware::principal::MaybePrincipal,
    Query(params): Query<SearchParams>,
) -> ApiResult<Json<serde_json::Value>> {
    let term = params.q.trim();
    let limit = params.limit.unwrap_or(10).clamp(1, 50);
    if term.is_empty() {
        return Ok(Json(serde_json::json!({ "query": "", "hits": [] })));
    }

    let Some(index) = state.search_index.as_ref() else {
        // Say so rather than silently returning nothing: an empty result
        // and an unavailable index mean very different things to a client.
        return Ok(Json(serde_json::json!({
            "query": term,
            "hits": [],
            "note": "search index unavailable",
        })));
    };

    let mut found: Vec<(i64, vyasa_search::SearchHit)> = index
        .search(term, None, limit, 0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|hit| i64::try_from(hit.id).ok().map(|id| (id, hit)))
        .collect();
    // With an embedding model registered and semantic search on, the
    // full-text candidates are re-ordered by meaning.
    if crate::ai_features::AiSettings::load(&state)
        .await
        .semantic_search
    {
        let order = crate::ai_features::semantic_rerank(
            &state,
            principal.as_ref(),
            term,
            found.iter().map(|(id, _)| *id).collect(),
        )
        .await;
        found.sort_by_key(|(id, _)| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    }
    let mut hits = Vec::new();
    for (id, hit) in found {
        let Ok(row) = state.posts.get(id).await else {
            continue;
        };
        if row.status != PostStatus::Published || row.password_hash.is_some() {
            continue;
        }
        // A type made non-public may still be in the index until its
        // reindex finishes; never show it to someone who does not edit.
        if !crate::policy::may_read_type(&state, principal.as_ref(), row.post_type).await {
            continue;
        }
        hits.push(serde_json::json!({
            // A string, because a snowflake does not survive JavaScript.
            "id": row.id.to_string(),
            "slug": row.slug,
            "title": row.title,
            "url": crate::permalinks::path(&state, &row).await,
            "excerpt": row.excerpt,
            // Contains <mark> around the matched terms; escaped by the
            // index's snippet generator before the markup is inserted.
            "snippet": hit.snippet,
        }));
    }
    Ok(Json(serde_json::json!({ "query": term, "hits": hits })))
}

/// `POST /api/v1/search/reindex` — rebuild the full-text index.
///
/// Tantivy permits one writer, and the server holds it, so the equivalent CLI
/// command only works with the server stopped. This runs the rebuild inside
/// the process that owns the writer, which is what makes it usable without
/// downtime.
#[utoipa::path(
    post, path = "/api/v1/search/reindex",
    tag = "search",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Rebuilt", body = serde_json::Value),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn reindex(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let indexed = crate::search_indexer::reindex_all(&state).await?;
    Ok(Json(serde_json::json!({ "indexed": indexed })))
}
