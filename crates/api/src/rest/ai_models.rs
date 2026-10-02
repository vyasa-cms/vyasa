//! The Models page: providers, their keys, and the registry of models per
//! kind of job. ManageOptions throughout — this is site configuration.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use vyasa_common::AppError;
use vyasa_core::ai_models::{ModelKind, ProviderId};
use vyasa_db::repo::{AiModelRow, AiModelUpdate};

use crate::error::{ApiError, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

fn model_json(row: &AiModelRow) -> Value {
    json!({
        "id": row.id,
        "provider": row.provider,
        "model": row.model,
        "kind": row.kind,
        "label": row.label,
        "enabled": row.enabled,
        "is_default": row.is_default,
        "settings": row.settings,
        "input_cost_per_mtok": row.input_cost_per_mtok,
        "output_cost_per_mtok": row.output_cost_per_mtok,
        "last_probe_ok": row.last_probe_ok,
        "last_probe_detail": row.last_probe_detail,
        "last_probe_at": row.last_probe_at,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    })
}

/// `GET /api/v1/ai/models` — the whole registry: providers, models and the
/// vocabulary of kinds, in one response so the page renders from one fetch.
#[utoipa::path(
    get, path = "/api/v1/ai/models", tag = "ai",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Providers, models and kinds", body = serde_json::Value))
)]
pub async fn overview(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let providers = state.ai_models.providers().await?;
    let models = state.ai_models.models().await?;
    let logs = vyasa_db::repo::AiLogRepo::new(state.pool.clone());
    let total = logs.month_spend_total().await.unwrap_or(0.0);
    let by_purpose = logs.month_spend_by_purpose().await.unwrap_or_default();
    let by_model = state
        .ai_models
        .month_spend_by_model()
        .await
        .unwrap_or_default();
    let cap = crate::ai_registry::month_cap_usd(&state).await;
    let disabled_providers: std::collections::HashSet<&str> = providers
        .iter()
        .filter(|p| !p.enabled)
        .map(|p| p.provider.as_str())
        .collect();
    let models_json: Vec<Value> = models
        .iter()
        .map(|row| {
            let mut v = model_json(row);
            let key = format!("{}/{}", row.provider, row.model);
            let tripped = state
                .breakers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&key)
                .is_some_and(|b| !b.allows());
            v["breaker_open"] = json!(tripped);
            v["provider_enabled"] = json!(!disabled_providers.contains(row.provider.as_str()));
            let spend = by_model
                .iter()
                .find(|(p, m, _, _)| p == &row.provider && m == &row.model);
            v["month_calls"] = json!(spend.map_or(0, |s| s.2));
            v["month_cost_usd"] = json!(spend.map_or(0.0, |s| s.3));
            v
        })
        .collect();
    Ok(Json(json!({
        "encrypting_keys": state.ai_models.encrypting(),
        "providers": providers,
        "models": models_json,
        "spend": {
            "total_usd": total,
            "cap_usd": cap,
            "by_purpose": by_purpose.into_iter().collect::<std::collections::BTreeMap<String, f64>>(),
        },
        "kinds": ModelKind::ALL.iter().map(|k| json!({
            "kind": k.as_str(),
            "label": k.label(),
            "used_by": k.used_by(),
        })).collect::<Vec<_>>(),
    })))
}

/// The fallback order for one kind.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct OrderBody {
    /// `text`, `vision`, …
    pub kind: String,
    /// Model ids as strings (64-bit ids do not survive JavaScript numbers),
    /// first to last, after the default.
    pub ids: Vec<String>,
}

/// `PUT /api/v1/ai/models/order` — set the fallback order within a kind.
#[utoipa::path(
    put, path = "/api/v1/ai/models/order", tag = "ai",
    security(("session_cookie" = [])),
    request_body = OrderBody,
    responses((status = 204, description = "Ordered"))
)]
pub async fn order(
    State(state): State<AppState>,
    Json(body): Json<OrderBody>,
) -> ApiResult<axum::http::StatusCode> {
    let kind = ModelKind::parse(&body.kind).map_err(ApiError)?;
    let ids: Vec<i64> = body
        .ids
        .iter()
        .map(|s| {
            s.parse::<i64>()
                .map_err(|_| ApiError(AppError::validation(format!("bad id {s:?}"))))
        })
        .collect::<Result<_, _>>()?;
    state.ai_models.reorder(kind, &ids).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Provider settings body.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ProviderBody {
    /// New API key; omit to keep the stored one, send "" to clear it.
    pub api_key: Option<String>,
    /// Base URL override; empty for the provider default.
    #[serde(default)]
    pub base_url: String,
    /// Whether the provider may be used.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// What the refusal says a provider write takes a full administrator to
/// do. A changed `base_url` with the key left out keeps the stored key and
/// sends it to the new host, so writing either is as sensitive as
/// redirecting the site's mail.
pub const AI_PROVIDER_WRITE: &str = "change an AI provider's address or key";

/// `PUT /api/v1/ai/providers/{provider}` — save a provider's key and
/// settings. Takes a full administrator ([`AI_PROVIDER_WRITE`]).
#[utoipa::path(
    put, path = "/api/v1/ai/providers/{provider}", tag = "ai",
    security(("session_cookie" = [])),
    params(("provider" = String, Path, description = "anthropic | openai | openrouter")),
    request_body = ProviderBody,
    responses((status = 200, description = "Provider status", body = serde_json::Value))
)]
pub async fn save_provider(
    State(state): State<AppState>,
    principal: Principal,
    Path(provider): Path<String>,
    Json(body): Json<ProviderBody>,
) -> ApiResult<Json<Value>> {
    policy::full_administrator(&principal, AI_PROVIDER_WRITE)?;
    let id = ProviderId::parse(&provider).map_err(ApiError)?;
    let status = state
        .ai_models
        .save_provider(id, body.api_key.as_deref(), &body.base_url, body.enabled)
        .await?;
    crate::audit::record(
        &state,
        principal.user(),
        "ai.provider.update",
        id.as_str(),
        json!({ "key_changed": body.api_key.is_some(), "enabled": body.enabled }),
    );
    Ok(Json(serde_json::to_value(status).unwrap_or(Value::Null)))
}

/// `DELETE /api/v1/ai/providers/{provider}` — forget a provider and its models.
#[utoipa::path(
    delete, path = "/api/v1/ai/providers/{provider}", tag = "ai",
    security(("session_cookie" = [])),
    params(("provider" = String, Path, description = "anthropic | openai | openrouter")),
    responses((status = 204, description = "Removed"))
)]
pub async fn delete_provider(
    State(state): State<AppState>,
    principal: Principal,
    Path(provider): Path<String>,
) -> ApiResult<StatusCode> {
    // Its key goes with it: the same rule as writing one.
    policy::full_administrator(&principal, "remove an AI provider and its key")?;
    let id = ProviderId::parse(&provider).map_err(ApiError)?;
    vyasa_db::repo::AiModelsRepo::new(state.pool.clone())
        .delete_provider(id.as_str())
        .await?;
    crate::audit::record(
        &state,
        principal.user(),
        "ai.provider.delete",
        id.as_str(),
        json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Catalogue query.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct CatalogQuery {
    /// Substring filter on the model id or name.
    #[serde(default)]
    pub q: String,
    /// Keep only models fit for this kind.
    pub kind: Option<String>,
}

/// `GET /api/v1/ai/providers/{provider}/catalog` — what the key can use.
#[utoipa::path(
    get, path = "/api/v1/ai/providers/{provider}/catalog", tag = "ai",
    security(("session_cookie" = [])),
    params(("provider" = String, Path, description = "anthropic | openai | openrouter"), CatalogQuery),
    responses((status = 200, description = "Catalogue entries", body = serde_json::Value))
)]
pub async fn catalog(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<CatalogQuery>,
) -> ApiResult<Json<Value>> {
    let id = ProviderId::parse(&provider).map_err(ApiError)?;
    let (key, base_url) = state.ai_models.credentials(id).await?;
    let mut entries = vyasa_ai::catalog::list(id, &key, &base_url).await?;
    let needle = query.q.trim().to_ascii_lowercase();
    if !needle.is_empty() {
        entries.retain(|e| {
            e.id.to_ascii_lowercase().contains(&needle)
                || e.name.to_ascii_lowercase().contains(&needle)
        });
    }
    if let Some(kind) = query.kind.as_deref() {
        let kind = ModelKind::parse(kind).map_err(ApiError)?;
        entries.retain(|e| e.kinds.contains(&kind.as_str()));
    }
    entries.truncate(200);
    Ok(Json(serde_json::to_value(entries).unwrap_or(Value::Null)))
}

/// `POST /api/v1/ai/providers/{provider}/test` — can the key list models?
#[utoipa::path(
    post, path = "/api/v1/ai/providers/{provider}/test", tag = "ai",
    security(("session_cookie" = [])),
    params(("provider" = String, Path, description = "anthropic | openai | openrouter")),
    responses((status = 200, description = "Outcome", body = serde_json::Value))
)]
pub async fn test_provider(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = ProviderId::parse(&provider).map_err(ApiError)?;
    let (key, base_url) = state.ai_models.credentials(id).await?;
    let entries = vyasa_ai::catalog::list(id, &key, &base_url).await?;
    Ok(Json(json!({
        "ok": true,
        "detail": format!("Key accepted; {} models available", entries.len()),
    })))
}

/// New-model body.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct NewModelBody {
    /// Provider.
    pub provider: String,
    /// Kind of job.
    pub kind: String,
    /// Provider model id.
    pub model: String,
    /// Display name (defaults to the id).
    #[serde(default)]
    pub label: String,
    /// Per-model settings.
    #[serde(default = "empty_object")]
    pub settings: Value,
    /// USD per million input tokens.
    pub input_cost_per_mtok: Option<f64>,
    /// USD per million output tokens.
    pub output_cost_per_mtok: Option<f64>,
    /// Make it the default of its kind right away.
    #[serde(default)]
    pub make_default: bool,
}

fn empty_object() -> Value {
    json!({})
}

/// `POST /api/v1/ai/models` — register a model.
#[utoipa::path(
    post, path = "/api/v1/ai/models", tag = "ai",
    security(("session_cookie" = [])),
    request_body = NewModelBody,
    responses((status = 201, description = "Registered", body = serde_json::Value),
              (status = 409, description = "Already registered"))
)]
pub async fn register(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<NewModelBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let provider = ProviderId::parse(&body.provider).map_err(ApiError)?;
    let kind = ModelKind::parse(&body.kind).map_err(ApiError)?;
    let mut row = state
        .ai_models
        .register(vyasa_core::ai_models::NewModelSpec {
            provider,
            kind,
            model: &body.model,
            label: &body.label,
            settings: body.settings,
            input_cost_per_mtok: body.input_cost_per_mtok,
            output_cost_per_mtok: body.output_cost_per_mtok,
        })
        .await?;
    if body.make_default && !row.is_default {
        row = state.ai_models.set_default(row.id).await?;
    }
    crate::audit::record(
        &state,
        principal.user(),
        "ai.model.register",
        row.id.to_string(),
        json!({ "provider": row.provider, "model": row.model, "kind": row.kind }),
    );
    Ok((StatusCode::CREATED, Json(model_json(&row))))
}

/// A cost field in an edit: absent keeps the stored value, `null` clears
/// it, a number sets it.
#[derive(Debug, Clone, Copy, Default, utoipa::ToSchema)]
pub enum CostField {
    /// Field not sent.
    #[default]
    Keep,
    /// Explicit `null`.
    Clear,
    /// A value.
    Set(f64),
}

impl<'de> Deserialize<'de> for CostField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<f64>::deserialize(deserializer)? {
            None => Self::Clear,
            Some(v) => Self::Set(v),
        })
    }
}

impl CostField {
    // The repository's update type distinguishes "keep" from "clear" the
    // same three-way way; this is the boundary where the enum becomes it.
    #[allow(clippy::option_option)]
    fn into_update(self) -> Option<Option<f64>> {
        match self {
            Self::Keep => None,
            Self::Clear => Some(None),
            Self::Set(v) => Some(Some(v)),
        }
    }
}

/// Edit body; every field optional.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct EditModelBody {
    /// Display name.
    pub label: Option<String>,
    /// Enabled flag.
    pub enabled: Option<bool>,
    /// Per-model settings.
    pub settings: Option<Value>,
    /// USD per million input tokens; `null` clears, absent keeps.
    #[serde(default)]
    pub input_cost_per_mtok: CostField,
    /// USD per million output tokens; `null` clears, absent keeps.
    #[serde(default)]
    pub output_cost_per_mtok: CostField,
}

/// `PUT /api/v1/ai/models/{id}` — edit a model.
#[utoipa::path(
    put, path = "/api/v1/ai/models/{id}", tag = "ai",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Model id")),
    request_body = EditModelBody,
    responses((status = 200, description = "Updated", body = serde_json::Value))
)]
pub async fn edit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<EditModelBody>,
) -> ApiResult<Json<Value>> {
    let row = state
        .ai_models
        .update(
            id,
            &AiModelUpdate {
                label: body.label.as_deref(),
                enabled: body.enabled,
                settings: body.settings,
                input_cost_per_mtok: body.input_cost_per_mtok.into_update(),
                output_cost_per_mtok: body.output_cost_per_mtok.into_update(),
            },
        )
        .await?;
    Ok(Json(model_json(&row)))
}

/// `POST /api/v1/ai/models/{id}/default` — make it the default of its kind.
#[utoipa::path(
    post, path = "/api/v1/ai/models/{id}/default", tag = "ai",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Model id")),
    responses((status = 200, description = "Updated", body = serde_json::Value))
)]
pub async fn make_default(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let row = state.ai_models.set_default(id).await?;
    crate::audit::record(
        &state,
        principal.user(),
        "ai.model.default",
        row.id.to_string(),
        json!({ "kind": row.kind, "model": row.model }),
    );
    Ok(Json(model_json(&row)))
}

/// `POST /api/v1/ai/models/{id}/test` — a real, cheap call to the model.
#[utoipa::path(
    post, path = "/api/v1/ai/models/{id}/test", tag = "ai",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Model id")),
    responses((status = 200, description = "Outcome", body = serde_json::Value))
)]
pub async fn test_model(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let resolved = state.ai_models.resolve(id).await?;
    let kind = ModelKind::parse(&resolved.row.kind).map_err(ApiError)?;
    let outcome = vyasa_ai::catalog::probe(
        kind,
        resolved.provider,
        &resolved.api_key,
        &resolved.base_url,
        &resolved.row.model,
    )
    .await;
    let (ok, detail) = match &outcome {
        Ok(detail) => (true, detail.clone()),
        Err(err) => (false, err.to_string()),
    };
    state.ai_models.record_probe(id, ok, &detail).await?;
    Ok(Json(json!({ "ok": ok, "detail": detail })))
}

/// `DELETE /api/v1/ai/models/{id}` — remove a model.
#[utoipa::path(
    delete, path = "/api/v1/ai/models/{id}", tag = "ai",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Model id")),
    responses((status = 204, description = "Removed"), (status = 404, description = "Not found"))
)]
pub async fn remove(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    if !state.ai_models.remove(id).await? {
        return Err(ApiError(AppError::not_found("ai_model", id)));
    }
    crate::audit::record(
        &state,
        principal.user(),
        "ai.model.delete",
        id.to_string(),
        json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}
