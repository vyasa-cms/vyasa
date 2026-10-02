//! Term taxonomy endpoints.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_db::content_models::Taxonomy;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

/// Response shape for a term.
#[derive(Serialize, utoipa::ToSchema)]
pub struct TermResponse {
    /// Term id.
    pub id: i64,
    /// Taxonomy (`category` or `tag`).
    pub taxonomy: String,
    /// Human name.
    pub name: String,
    /// Slug.
    pub slug: String,
    /// Parent id.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Extension metadata.
    pub meta: serde_json::Value,
    /// Number of posts assigned (when `post_counts=true`).
    pub post_count: Option<i64>,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<vyasa_db::repo::TermWithCount> for TermResponse {
    fn from(wc: vyasa_db::repo::TermWithCount) -> Self {
        Self {
            id: wc.term.id,
            taxonomy: wc.term.taxonomy.as_str().to_string(),
            name: wc.term.name,
            slug: wc.term.slug,
            parent_id: wc.term.parent_id,
            meta: wc.term.meta,
            post_count: Some(wc.post_count),
            created_at: wc.term.created_at,
        }
    }
}

impl From<vyasa_db::content_models::TermRow> for TermResponse {
    fn from(row: vyasa_db::content_models::TermRow) -> Self {
        Self {
            id: row.id,
            taxonomy: row.taxonomy.as_str().to_string(),
            name: row.name,
            slug: row.slug,
            parent_id: row.parent_id,
            meta: row.meta,
            post_count: None,
            created_at: row.created_at,
        }
    }
}

/// Request for creating a term.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateTermRequest {
    /// Taxonomy (`category` or `tag`).
    pub taxonomy: String,
    /// Human name.
    pub name: String,
    /// Slug (derived from name when absent).
    pub slug: Option<String>,
    /// Parent id.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Extension metadata.
    pub meta: Option<serde_json::Value>,
}

/// Request for updating a term.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateTermRequest {
    /// New name.
    pub name: Option<String>,
    /// New slug.
    pub slug: Option<String>,
    /// New parent (`null` clears).
    #[allow(clippy::option_option)]
    #[serde(default, deserialize_with = "crate::flexible_id::double_option")]
    pub parent_id: Option<Option<i64>>,
    /// New meta.
    pub meta: Option<serde_json::Value>,
}

/// Query for listing terms.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct ListTermsQuery {
    /// Filter by taxonomy.
    pub taxonomy: Option<String>,
    /// Include post counts.
    pub post_counts: Option<bool>,
}

/// Query for deleting a term with reassignment.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct DeleteTermQuery {
    /// Reassign posts/children to this term before deleting.
    pub reassign_to: Option<i64>,
}

/// Request for setting a post's terms (replace-set).
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SetPostTermsRequest {
    /// Term ids to assign (replace-set).
    #[serde(default, deserialize_with = "crate::flexible_id::vec")]
    pub term_ids: Vec<i64>,
}

/// `POST /api/v1/terms` — create a term.
///
/// # Errors
///
/// 400 on validation, 403 without `manage_categories`, 409 on slug conflict.
#[utoipa::path(
    post, path = "/api/v1/terms",
    tag = "terms",
    security(("session_cookie" = []), ("api_key" = [])),
    request_body = CreateTermRequest,
    responses(
        (status = 201, description = "Term created", body = TermResponse),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 409, description = "Slug taken", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateTermRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<TermResponse>)> {
    let taxonomy = Taxonomy::parse(&body.taxonomy).map_err(ApiError)?;
    let term = state
        .terms
        .create(
            taxonomy,
            &body.name,
            body.slug.as_deref(),
            body.parent_id,
            body.meta,
        )
        .await?;
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::TERM_CREATED,
        serde_json::json!({
            "term_id": term.id,
            "taxonomy": term.taxonomy.as_str(),
            "slug": term.slug,
        }),
    );
    Ok((
        axum::http::StatusCode::CREATED,
        Json(TermResponse::from(term)),
    ))
}

/// `GET /api/v1/terms` — list terms.
///
/// # Errors
///
/// 401 when unauthenticated.
#[utoipa::path(
    get, path = "/api/v1/terms",
    tag = "terms",
    security(("session_cookie" = []), ("api_key" = [])),
    params(ListTermsQuery),
    responses(
        (status = 200, description = "Term list", body = [TermResponse]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    _principal: Principal,
    Query(query): Query<ListTermsQuery>,
) -> ApiResult<Json<Vec<TermResponse>>> {
    let taxonomy = match query.taxonomy {
        Some(ref s) if !s.is_empty() => Some(Taxonomy::parse(s).map_err(ApiError)?),
        _ => None,
    };
    let with_counts = query.post_counts.unwrap_or(false);
    let terms = state.terms.list(taxonomy, with_counts).await?;
    Ok(Json(terms.into_iter().map(TermResponse::from).collect()))
}

/// `GET /api/v1/terms/{id}` — fetch a term.
///
/// # Errors
///
/// 404 when missing.
#[utoipa::path(
    get, path = "/api/v1/terms/{id}",
    tag = "terms",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Term id")),
    responses(
        (status = 200, description = "Term", body = TermResponse),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn get(
    State(state): State<AppState>,
    _principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<TermResponse>> {
    let term = state.terms.get(id).await?;
    Ok(Json(TermResponse::from(term)))
}

/// `PUT /api/v1/terms/{id}` — update a term.
///
/// # Errors
///
/// 400/403/404/409 as appropriate.
#[utoipa::path(
    put, path = "/api/v1/terms/{id}",
    tag = "terms",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Term id")),
    request_body = UpdateTermRequest,
    responses(
        (status = 200, description = "Updated term", body = TermResponse),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
        (status = 409, description = "Slug taken", body = ApiErrorBody),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTermRequest>,
) -> ApiResult<Json<TermResponse>> {
    let term = state
        .terms
        .update(
            id,
            body.name.as_deref(),
            body.slug.as_deref(),
            body.parent_id,
            body.meta,
        )
        .await?;
    Ok(Json(TermResponse::from(term)))
}

/// `DELETE /api/v1/terms/{id}` — delete a term.
///
/// # Errors
///
/// 403/404 as appropriate.
#[utoipa::path(
    delete, path = "/api/v1/terms/{id}",
    tag = "terms",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("id" = i64, Path, description = "Term id"),
        DeleteTermQuery,
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn delete_term(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<DeleteTermQuery>,
) -> ApiResult<axum::http::StatusCode> {
    state.terms.delete(id, query.reassign_to).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /api/v1/terms/merge` — merge one term into another.
///
/// # Errors
///
/// 400/403/404 as appropriate.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct MergeTermsRequest {
    /// Source term id.
    pub from: i64,
    /// Target term id.
    pub into: i64,
}

#[utoipa::path(
    post, path = "/api/v1/terms/merge",
    tag = "terms",
    security(("session_cookie" = []), ("api_key" = [])),
    request_body = MergeTermsRequest,
    responses(
        (status = 204, description = "Merged"),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn merge(
    State(state): State<AppState>,
    Json(body): Json<MergeTermsRequest>,
) -> ApiResult<axum::http::StatusCode> {
    state.terms.merge(body.from, body.into).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /api/v1/posts/{id}/terms` — replace a post's terms.
///
/// # Errors
///
/// 400/403/404 as appropriate.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/terms",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    request_body = SetPostTermsRequest,
    responses(
        (status = 204, description = "Terms set"),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn set_post_terms(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<SetPostTermsRequest>,
) -> ApiResult<axum::http::StatusCode> {
    // Post authors can set their own terms if they can edit posts; others need EditOthers.
    policy::post_for_edit(&state, &principal, id).await?;
    state.terms.set_post_terms(id, &body.term_ids, None).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /api/v1/posts/{id}/terms` — list a post's terms.
///
/// # Errors
///
/// 401/403/404 as appropriate.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/terms",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 200, description = "Term list", body = [TermResponse]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Entry not visible", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn list_post_terms(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<TermResponse>>> {
    // An entry's terms are readable by whoever may read the entry.
    policy::post_for_view(&state, &principal, id, None).await?;
    let term_ids = state.terms.list_for_post(id).await?;
    let mut terms = Vec::new();
    for term_id in term_ids {
        if let Ok(term) = state.terms.get(term_id).await {
            terms.push(TermResponse::from(term));
        }
    }
    Ok(Json(terms))
}
