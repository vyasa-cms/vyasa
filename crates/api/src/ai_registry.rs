//! The bridge from the model registry (`vyasa_core::ai_models`) to real
//! clients (`vyasa_ai`): builds the failover chain a feature calls.
//!
//! Resolution order for the text kind, which is the one the product uses
//! today: registered models (default first, then the other enabled ones),
//! and if nothing is registered, the pre-registry environment key
//! (`VYASA_AI_ANTHROPIC_KEY` + `VYASA_AI_MODEL`) so an existing deployment
//! keeps working until someone opens the Models page.

use std::sync::Arc;
use std::time::Duration;

use vyasa_ai::{CircuitBreaker, FailoverProvider, ModelSlot, ProviderSlot};
use vyasa_common::AppError;
use vyasa_core::ai_models::{ModelKind, ProviderId};

use crate::state::AppState;

/// Breaker settings for registry slots: three consecutive infrastructure
/// failures open the slot for a minute.
const BREAKER_THRESHOLD: u32 = 3;
const BREAKER_COOLDOWN: Duration = Duration::from_secs(60);

/// The month's total spending cap, if the site has one.
///
/// From the `ai_month_cap_usd` site option, else the environment the
/// deployment was started with. It used to be read from the environment
/// only, and checked on one endpoint only: alt text, autofill, comment
/// screening and the theme studio all spent past it.
pub async fn month_cap_usd(state: &AppState) -> Option<f64> {
    let from_option = state
        .options
        .get_or_default("ai_month_cap_usd", serde_json::Value::Null)
        .await
        .ok()
        .and_then(|v| v.as_f64());
    from_option.or_else(|| {
        std::env::var("VYASA_AI_MONTH_CAP_USD")
            .ok()
            .and_then(|v| v.parse().ok())
    })
}

/// Refuses when the month's spend has reached the cap.
///
/// Every feature obtains its provider through [`chain`], so this is the
/// one place the check has to live to cover all of them.
async fn enforce_month_cap(state: &AppState) -> Result<(), AppError> {
    let Some(cap) = month_cap_usd(state).await else {
        return Ok(());
    };
    let spent = vyasa_db::repo::AiLogRepo::new(state.pool.clone())
        .month_spend_total()
        .await?;
    if spent >= cap {
        return Err(AppError::external(
            "ai-budget",
            format!(
                "This month's AI budget of ${cap:.2} is spent. Raise it under Settings or wait for next month."
            ),
        ));
    }
    Ok(())
}

/// The one breaker for this model, created on first sight.
fn breaker_for(state: &AppState, key: &str) -> CircuitBreaker {
    state
        .breakers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(key.to_owned())
        .or_insert_with(|| CircuitBreaker::new(BREAKER_THRESHOLD, BREAKER_COOLDOWN))
        .clone()
}

/// The chain for a kind of job.
///
/// # Errors
/// [`AppError::Validation`] when no model can be used, with a message that
/// says what to do about it.
pub async fn chain(state: &AppState, kind: ModelKind) -> Result<FailoverProvider, AppError> {
    enforce_month_cap(state).await?;
    let resolved = state.ai_models.chain(kind).await?;
    let mut slots = Vec::with_capacity(resolved.len());
    for model in resolved {
        let provider = vyasa_ai::catalog::chat_provider(
            model.provider,
            &model.api_key,
            &model.base_url,
            &model.row.settings,
        )?;
        let breaker = breaker_for(
            state,
            &format!("{}/{}", model.provider.as_str(), model.row.model),
        );
        slots.push(ModelSlot {
            slot: ProviderSlot { provider, breaker },
            model: model.row.model,
        });
    }

    if slots.is_empty() && kind == ModelKind::Text {
        // Pre-registry fallback: the environment key, if any.
        if let Ok((key, base_url)) = state.ai_models.credentials(ProviderId::Anthropic).await {
            let provider = vyasa_ai::catalog::chat_provider(
                ProviderId::Anthropic,
                &key,
                &base_url,
                &serde_json::json!({}),
            )?;
            let model = std::env::var("VYASA_AI_MODEL")
                .unwrap_or_else(|_| String::from("claude-haiku-4-5"));
            let breaker = breaker_for(state, &format!("anthropic/{model}"));
            slots.push(ModelSlot {
                slot: ProviderSlot { provider, breaker },
                model,
            });
        }
    }

    FailoverProvider::new(slots).map_err(|_| {
        AppError::validation(format!(
            "no {} model is set up: register one on the Models page",
            kind.label().to_lowercase()
        ))
    })
}

/// The text chain, which every current feature uses.
///
/// # Errors
/// See [`chain`].
pub async fn text(state: &AppState) -> Result<Arc<FailoverProvider>, AppError> {
    chain(state, ModelKind::Text).await.map(Arc::new)
}

/// The default (first usable) model of a kind with its credentials, for the
/// kinds that are not chat completions.
///
/// # Errors
/// [`AppError::Validation`] when nothing is registered.
pub async fn first(
    state: &AppState,
    kind: ModelKind,
) -> Result<vyasa_core::ai_models::ResolvedModel, AppError> {
    state
        .ai_models
        .chain(kind)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            AppError::validation(format!(
                "no {} model is set up: register one on the Models page",
                kind.label().to_lowercase()
            ))
        })
}

/// The usage sink: writes `ai_logs` rows priced from the registry's
/// per-model costs, falling back to the built-in table.
pub struct Sink {
    pool: sqlx::PgPool,
    costs: std::collections::HashMap<String, vyasa_ai::ModelCost>,
}

/// Builds the sink, reading the registry's costs once.
pub async fn sink(state: &AppState) -> Sink {
    let mut costs = vyasa_ai::default_cost_table();
    if let Ok(models) = state.ai_models.models().await {
        for m in models {
            if let (Some(input), Some(output)) = (m.input_cost_per_mtok, m.output_cost_per_mtok) {
                costs.insert(
                    m.model.clone(),
                    vyasa_ai::ModelCost {
                        input_per_mtok: input,
                        output_per_mtok: output,
                    },
                );
            }
        }
    }
    Sink {
        pool: state.pool.clone(),
        costs,
    }
}

impl vyasa_ai::AiUsageSink for Sink {
    fn log_success(&self, provider_name: &str, model: &str, purpose: &str, pt: u32, ct: u32) {
        let cost = vyasa_ai::compute_cost(
            &self.costs,
            model,
            vyasa_ai::Usage {
                prompt_tokens: pt,
                completion_tokens: ct,
            },
        );
        let repo = vyasa_db::repo::AiLogRepo::new(self.pool.clone());
        let provider_name = provider_name.to_owned();
        let model = model.to_owned();
        let purpose = purpose.to_owned();
        tokio::spawn(async move {
            let _ = repo
                .record(
                    &provider_name,
                    &model,
                    &purpose,
                    i32::try_from(pt).unwrap_or(i32::MAX),
                    i32::try_from(ct).unwrap_or(i32::MAX),
                    cost,
                )
                .await;
        });
    }

    fn log_failure(&self, provider_name: &str, model: &str, purpose: &str, error: &str) {
        tracing::warn!(
            provider = provider_name,
            model,
            purpose,
            "AI call failed: {error}"
        );
    }
}

/// Logs a call that has no token accounting (images, audio) so the usage
/// page still shows it happened; the cost is the registry's input rate per
/// call when one is set, else zero.
pub async fn log_flat(
    state: &AppState,
    model: &vyasa_core::ai_models::ResolvedModel,
    purpose: &str,
) {
    let cost = model.row.input_cost_per_mtok.unwrap_or(0.0);
    let repo = vyasa_db::repo::AiLogRepo::new(state.pool.clone());
    let _ = repo
        .record(
            model.provider.as_str(),
            &model.row.model,
            purpose,
            0,
            0,
            cost,
        )
        .await;
}

/// The text model as plugins see it: a plain reply, logged under the
/// plugin's name so the usage page shows who spent what.
pub struct PluginAi {
    state: AppState,
}

impl PluginAi {
    /// Builds the backend.
    #[must_use]
    pub fn new(state: AppState) -> Arc<Self> {
        Arc::new(Self { state })
    }
}

impl vyasa_plugins::host::AiBackend for PluginAi {
    fn complete<'a>(
        &'a self,
        plugin_name: &'a str,
        system: String,
        user: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            #[derive(serde::Deserialize, schemars::JsonSchema)]
            struct Reply {
                text: String,
            }
            let provider = text(&self.state).await.map_err(|e| e.to_string())?;
            let spec = vyasa_ai::PromptSpec {
                purpose: format!("plugin:{plugin_name}"),
                model: provider.primary_model().to_owned(),
                system,
                user,
                output_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                    "additionalProperties": false
                }),
                attachments: Vec::new(),
            };
            let sink_ = sink(&self.state).await;
            let out: Reply = vyasa_ai::run_structured(provider.as_ref(), &sink_, &spec)
                .await
                .map_err(|e| e.to_string())?;
            Ok(out.text)
        })
    }
}
