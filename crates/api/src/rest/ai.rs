//! AI generate endpoint (ManageOptions capability; internal building block
//! for phases 39/41).

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use vyasa_ai::PromptSpec;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Generate request: purpose-scoped passthrough.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct AiGenerateRequest {
    /// Purpose tag recorded in ai_logs.
    pub purpose: String,
    /// Model identifier (defaults to the config's default model).
    pub model: Option<String>,
    /// System prompt.
    pub system: String,
    /// User prompt.
    pub user: String,
    /// JSON Schema the response must satisfy.
    pub output_schema: serde_json::Value,
}

// The api-side sink writes ai_logs rows with costs from the table.
struct DbSink<'a> {
    state: &'a AppState,
}
impl vyasa_ai::AiUsageSink for DbSink<'_> {
    fn log_success(
        &self,
        provider_name: &str,
        model: &str,
        purpose: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) {
        let cost = vyasa_ai::compute_cost(
            &vyasa_ai::default_cost_table(),
            model,
            vyasa_ai::Usage {
                prompt_tokens,
                completion_tokens,
            },
        );
        let repo = vyasa_db::repo::AiLogRepo::new(self.state.pool.clone());
        let provider_name = provider_name.to_owned();
        let model = model.to_owned();
        let purpose = purpose.to_owned();
        tokio::spawn(async move {
            if let Err(e) = repo
                .record(
                    &provider_name,
                    &model,
                    &purpose,
                    i32::try_from(prompt_tokens).unwrap_or(i32::MAX),
                    i32::try_from(completion_tokens).unwrap_or(i32::MAX),
                    cost,
                )
                .await
            {
                tracing::warn!("ai_log write failed: {e}");
            }
        });
    }

    fn log_spend(
        &self,
        provider_name: &str,
        model: &str,
        purpose: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) {
        // Billed the same whether or not the reply was usable.
        self.log_success(
            provider_name,
            model,
            purpose,
            prompt_tokens,
            completion_tokens,
        );
    }

    fn log_failure(&self, provider_name: &str, model: &str, _purpose: &str, error: &str) {
        tracing::warn!(
            provider = provider_name,
            model,
            "ai attempt failed: {error}"
        );
    }
}
/// `POST /api/v1/ai/generate` — validated structured generation.
///
/// Requires ManageOptions; keys come from config only and are never echoed.
pub async fn generate(
    State(state): State<AppState>,
    Json(body): Json<AiGenerateRequest>,
) -> ApiResult<Json<Value>> {
    let provider = crate::ai_registry::text(&state).await?;
    let spec = PromptSpec {
        attachments: Vec::new(),
        purpose: body.purpose,
        // An explicit model still wins; otherwise the registry's default.
        model: body
            .model
            .unwrap_or_else(|| provider.primary_model().to_owned()),
        system: body.system,
        user: body.user,
        output_schema: body.output_schema,
    };

    // Spend guard (phase 38): UTC-month caps; race tolerance of one call.
    // The same cap the registry enforces for every feature; this endpoint
    // adds the per-purpose view on top.
    let cap_total: Option<f64> = crate::ai_registry::month_cap_usd(&state).await;
    if let Some(cap) = cap_total {
        let repo = vyasa_db::repo::AiLogRepo::new(state.pool.clone());
        let spent = repo.month_spend_by_purpose().await.map_err(ApiError)?;
        let purpose_spend = spent
            .iter()
            .find(|(p, _)| *p == spec.purpose)
            .map_or(0.0, |(_, v)| *v);
        let total = repo.month_spend_total().await.map_err(ApiError)?;
        match vyasa_ai::evaluate_budget(
            &vyasa_ai::BudgetCaps {
                total_month_usd: Some(cap),
                per_purpose_month_usd: std::collections::HashMap::new(),
            },
            total,
            &spec.purpose,
            purpose_spend,
        ) {
            vyasa_ai::BudgetDecision::Allowed => {}
            decision => return Err(ApiError(vyasa_ai::budget_error(decision, &spec.purpose))),
        }
    }

    let value: Value =
        vyasa_ai::run_structured(provider.as_ref(), &DbSink { state: &state }, &spec)
            .await
            .map_err(ApiError)?;
    Ok(Json(value))
}

/// `GET /api/v1/ai/usage` — month-to-date totals by purpose and total.
#[utoipa::path(
    get, path = "/api/v1/ai/usage", tag = "ai",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Month spend", body = serde_json::Value))
)]
pub async fn usage(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let repo = vyasa_db::repo::AiLogRepo::new(state.pool.clone());
    let by_purpose = repo.month_spend_by_purpose().await.map_err(ApiError)?;
    let total = repo.month_spend_total().await.map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "month": "utc-calendar",
        "total_usd": total,
        "by_purpose": by_purpose.into_iter().collect::<std::collections::BTreeMap<String, f64>>(),
    })))
}
