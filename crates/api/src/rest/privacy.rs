//! `/api/v1/privacy/*`: personal data by email, and a starter policy page.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiResult;
use crate::middleware::Principal;
use crate::privacy::{self, Erasure, PersonalData};
use crate::state::AppState;

/// Whose data.
#[derive(Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct EmailQuery {
    pub email: String,
}

/// `GET /api/v1/privacy/export?email=` — everything held, as a download.
/// It names the account's entries, drafts included, so the address's
/// account, when there is one, must hold no capability the caller lacks.
#[utoipa::path(get, path = "/api/v1/privacy/export", tag = "operations", params(EmailQuery),
    security(("session_cookie" = [])),
    responses((status = 200, description = "The document", body = PersonalData),
              (status = 403, description = "The address's account holds a capability the caller lacks", body = crate::error::ApiErrorBody)))]
pub async fn export(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<EmailQuery>,
) -> ApiResult<(HeaderMap, Json<PersonalData>)> {
    crate::policy::personal_data_within_reach(&state, &principal, &q.email).await?;
    let data = privacy::export(&state, &q.email).await?;
    let mut h = HeaderMap::new();
    let safe: String = data
        .email
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if let Ok(v) = HeaderValue::from_str(&format!(
        "attachment; filename=\"personal-data-{safe}.json\""
    )) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    crate::audit::record(
        &state,
        principal.user(),
        "privacy.export",
        format!("email:{}", data.email),
        serde_json::json!({}),
    );
    Ok((h, Json(data)))
}

/// `POST /api/v1/privacy/erase` — anonymise and delete. The address's
/// account, when there is one, is deleted too, so it must hold no
/// capability the caller lacks.
#[utoipa::path(post, path = "/api/v1/privacy/erase", tag = "operations", request_body = EmailQuery,
    security(("session_cookie" = [])),
    responses((status = 200, description = "What was done", body = Erasure),
              (status = 403, description = "The address's account holds a capability the caller lacks", body = crate::error::ApiErrorBody)))]
pub async fn erase(
    State(state): State<AppState>,
    principal: Principal,
    Json(q): Json<EmailQuery>,
) -> ApiResult<Json<Erasure>> {
    crate::policy::demo_erasure(&state, &q.email)?;
    crate::policy::personal_data_within_reach(&state, &principal, &q.email).await?;
    let done = privacy::erase(&state, &q.email).await?;
    crate::audit::record(
        &state,
        principal.user(),
        "privacy.erase",
        format!("email:{}", q.email.trim().to_lowercase()),
        serde_json::json!({ "comments": done.comments_anonymised, "submissions": done.submissions_deleted, "account": done.account_deleted }),
    );
    Ok(Json(done))
}

/// `POST /api/v1/privacy/policy-page` — creates a draft "Privacy policy"
/// page from a template that describes what this site does, for the
/// editor to finish. Answers the existing page if one is there.
#[utoipa::path(post, path = "/api/v1/privacy/policy-page", tag = "operations",
    security(("session_cookie" = [])),
    responses((status = 201, description = "The page", body = serde_json::Value),
              (status = 200, description = "Already exists", body = serde_json::Value)))]
pub async fn policy_page(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    if let Ok(existing) = state
        .posts
        .get_by_slug(vyasa_db::content_models::PostType::Page, "privacy-policy")
        .await
    {
        return Ok((
            StatusCode::OK,
            Json(serde_json::json!({ "id": existing.id, "slug": existing.slug, "created": false })),
        ));
    }
    let site = state
        .options_service
        .site_identity()
        .await
        .ok()
        .map(|i| i.title)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| String::from("this site"));
    let newsletter = state
        .options
        .get("newsletter_enabled")
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let blocks = privacy::policy_blocks(&site, newsletter, true);
    let doc: vyasa_core::BlockDocument =
        serde_json::from_value(serde_json::json!({ "schema_version": 1, "blocks": blocks }))
            .map_err(|e| vyasa_common::AppError::internal_msg(e.to_string()))?;
    let page = state
        .posts
        .create(vyasa_core::post::CreatePost {
            post_type: vyasa_db::content_models::PostType::Page,
            status: vyasa_db::content_models::PostStatus::Draft,
            title: String::from("Privacy policy"),
            slug: Some(String::from("privacy-policy")),
            content: doc,
            excerpt: None,
            author_id: principal.user().id,
            parent_id: None,
            scheduled_for: None,
            password: None,
            term_ids: None,
            layout: None,
        })
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": page.id, "slug": page.slug, "created": true })),
    ))
}
