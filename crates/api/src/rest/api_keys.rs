//! API-key management for headless clients.
//!
//! Phase 08 built key authentication and phase 49 documents headless use,
//! but nothing could mint a key: the repository had `insert` and no route
//! reached it, so the only way to obtain one was to write a row by hand.
//!
//! The raw key is returned exactly once, at creation. Only its SHA-256 hash
//! is stored, so a database leak does not yield usable credentials and a
//! lost key has to be replaced rather than recovered.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use vyasa_common::AppError;
use vyasa_core::user::{cap_name, Capability};

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::CurrentUser;
use crate::state::AppState;

/// Request body for creating a key.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateKeyRequest {
    /// Human label, e.g. "Next.js frontend".
    pub name: String,
    /// Capability names to grant. Each must be one the owner already holds.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// A key as returned by the list endpoint — never includes the secret.
#[derive(Serialize, utoipa::ToSchema)]
pub struct KeyResponse {
    /// Snowflake id, as a string so JavaScript cannot round it.
    pub id: String,
    /// Human label.
    pub name: String,
    /// Granted capability names.
    pub capabilities: Vec<String>,
    /// Last successful authentication, if any.
    pub last_used_at: Option<String>,
    /// Creation time.
    pub created_at: String,
    /// Whether the key has been revoked.
    pub revoked: bool,
}

impl KeyResponse {
    fn from_row(row: &vyasa_db::models::ApiKeyRow) -> Self {
        Self {
            id: row.id.to_string(),
            name: row.name.clone(),
            capabilities: row
                .capabilities
                .as_array()
                .map(|caps| {
                    caps.iter()
                        .filter_map(|c| c.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            last_used_at: row.last_used_at.map(|t| t.to_rfc3339()),
            created_at: row.created_at.to_rfc3339(),
            revoked: row.revoked_at.is_some(),
        }
    }
}

/// Resolves a capability name, rejecting unknown ones.
fn capability_by_name(name: &str) -> Option<Capability> {
    Capability::ALL.into_iter().find(|c| cap_name(*c) == name)
}

/// `POST /api/v1/api-keys` — mint a key. The secret is shown once.
#[utoipa::path(
    post, path = "/api/v1/api-keys", tag = "api-keys",
    security(("session_cookie" = [])),
    request_body = CreateKeyRequest,
    responses(
        (status = 201, description = "Created; `key` is shown only here", body = serde_json::Value),
        (status = 400, description = "Unknown capability", body = ApiErrorBody),
        (status = 403, description = "Capability not held by the owner", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateKeyRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError(AppError::validation("name must not be empty")));
    }

    let mut granted = Vec::with_capacity(body.capabilities.len());
    for requested in &body.capabilities {
        let cap = capability_by_name(requested).ok_or_else(|| {
            let known: Vec<&str> = Capability::ALL.iter().map(|c| cap_name(*c)).collect();
            ApiError(AppError::validation(format!(
                "unknown capability \"{requested}\"; known capabilities are {}",
                known.join(", ")
            )))
        })?;
        // A key must never be able to do more than the person who made it.
        // Without this, an author could mint an admin key for themselves.
        if !vyasa_core::user::can(&user.user, cap) {
            return Err(ApiError(AppError::forbidden(format!(
                "you do not hold the {requested} capability, so a key cannot grant it"
            ))));
        }
        granted.push(serde_json::Value::String(cap_name(cap).to_owned()));
    }

    // 32 bytes of entropy, prefixed so a leaked key is recognisable in a log
    // or a repository scan.
    let raw = format!("vy_{}", {
        use rand::RngCore as _;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        hex::encode(buf)
    });
    let hash = hex::encode(Sha256::digest(raw.as_bytes()));

    let row = state
        .api_keys
        .insert(
            vyasa_common::next_id_i64(),
            user.user.id,
            name,
            &hash,
            &serde_json::Value::Array(granted),
        )
        .await
        .map_err(ApiError)?;

    let mut out = serde_json::to_value(KeyResponse::from_row(&row))
        .map_err(|e| ApiError(AppError::internal_msg(e.to_string())))?;
    out["key"] = serde_json::Value::String(raw);
    out["note"] = serde_json::Value::String(String::from(
        "This key is shown once. Store it now; it cannot be recovered.",
    ));
    Ok((StatusCode::CREATED, Json(out)))
}

/// `GET /api/v1/api-keys` — the caller's own keys, without secrets.
#[utoipa::path(
    get, path = "/api/v1/api-keys", tag = "api-keys",
    security(("session_cookie" = [])),
    responses((status = 200, description = "The caller's keys", body = Vec<KeyResponse>))
)]
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<KeyResponse>>> {
    let rows = state
        .api_keys
        .list_for_user(user.user.id)
        .await
        .map_err(ApiError)?;
    Ok(Json(rows.iter().map(KeyResponse::from_row).collect()))
}

/// `DELETE /api/v1/api-keys/{id}` — revoke a key.
#[utoipa::path(
    delete, path = "/api/v1/api-keys/{id}", tag = "api-keys",
    security(("session_cookie" = [])),
    params(("id" = String, Path, description = "Key id")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 404, description = "No such key for this user", body = ApiErrorBody),
    )
)]
pub async fn revoke(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    // Scoped to the caller's own keys: revoking by id alone would let any
    // authenticated user disable another's integration.
    let rows = state
        .api_keys
        .list_for_user(user.user.id)
        .await
        .map_err(ApiError)?;
    if !rows.iter().any(|r| r.id == id) {
        return Err(ApiError(AppError::not_found("api key", id)));
    }
    state.api_keys.revoke(id).await.map_err(ApiError)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::capability_by_name;
    use vyasa_core::user::{cap_name, Capability};

    #[test]
    fn every_capability_resolves_by_its_own_name() {
        for cap in Capability::ALL {
            assert_eq!(capability_by_name(cap_name(cap)), Some(cap));
        }
    }

    #[test]
    fn an_unknown_capability_is_rejected() {
        assert!(capability_by_name("do_anything").is_none());
        assert!(capability_by_name("").is_none());
    }
}
