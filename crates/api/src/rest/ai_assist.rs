//! Editor assist endpoints: POST /api/v1/ai/assist/{kind}.

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use vyasa_ai::assist::AssistKind;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Assist request: the post's block document, existing vocabulary, and —
/// for the text kinds — the passage the author selected.
#[allow(dead_code)] // deserialized from JSON body; fields read via doc
#[derive(Deserialize, utoipa::ToSchema)]
pub struct AssistRequest {
    /// Block document (schema_version + blocks) from the editor.
    pub content: serde_json::Value,
    /// Existing tag names (for the tags kind).
    #[serde(default)]
    pub vocabulary: Vec<String>,
    /// Selected text, for `rewrite`, `shorten`, `expand` and `fix`. Ignored
    /// by the other kinds.
    #[serde(default)]
    pub selection: Option<String>,
    /// Second-factor / user-submitted code passthrough (unused).
    #[serde(default)]
    pub extra_code: Option<String>,
}

/// ai_logs sink over the shared pool.
struct DbSink<'a> {
    state: &'a AppState,
}

impl vyasa_ai::AiUsageSink for DbSink<'_> {
    fn log_success(&self, provider_name: &str, model: &str, purpose: &str, pt: u32, ct: u32) {
        let cost = vyasa_ai::compute_cost(
            &vyasa_ai::default_cost_table(),
            model,
            vyasa_ai::Usage {
                prompt_tokens: pt,
                completion_tokens: ct,
            },
        );
        let repo = vyasa_db::repo::AiLogRepo::new(self.state.pool.clone());
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
        tracing::warn!(provider = provider_name, model, "assist failed: {error}");
    }
}

fn not_enough() -> ApiError {
    ApiError(vyasa_common::AppError::validation("not_enough_content"))
}

/// `POST /api/v1/ai/assist/{kind}` — suggestions are copy-into-form only.
pub async fn suggest(
    State(state): State<AppState>,
    axum::extract::Path(kind): axum::extract::Path<String>,
    Json(body): Json<AssistRequest>,
) -> ApiResult<Json<Value>> {
    // The route asks for `edit_posts`: assist spends the site's AI budget
    // on a draft; a subscriber's login has no draft to improve and no
    // business spending it.
    let parsed_kind = AssistKind::parse(&kind).map_err(ApiError)?;
    let doc: vyasa_core::BlockDocument = serde_json::from_value(body.content)
        .map_err(|e| ApiError(vyasa_common::AppError::validation(format!("content: {e}"))))?;
    let text = vyasa_ai::assist::extract_text(&doc.blocks, 4000);
    // A selection can be rewritten inside a two-line draft; only the kinds
    // that read the whole document need there to be a whole document.
    let needs_document = !parsed_kind.is_text() || parsed_kind == AssistKind::Continue;
    if needs_document && !vyasa_ai::assist::enough_content(&text) {
        return Err(not_enough());
    }

    let provider = crate::ai_registry::text(&state).await?;

    if parsed_kind.is_text() {
        // The passage is the selection, or for `continue` the document's
        // tail. `enough_content` was already checked above, so `continue`
        // never runs on an empty post.
        let passage = if parsed_kind == AssistKind::Continue {
            vyasa_ai::assist::tail(&text, 2500)
        } else {
            let selection = body.selection.unwrap_or_default();
            if !vyasa_ai::assist::enough_passage(&selection) {
                return Err(ApiError(vyasa_common::AppError::validation(
                    "selection_too_short",
                )));
            }
            selection.trim().to_owned()
        };
        let spec = parsed_kind.text_prompt_spec(&passage, &text);
        let out: TextResult =
            vyasa_ai::run_structured(provider.as_ref(), &DbSink { state: &state }, &spec)
                .await
                .map_err(ApiError)?;
        return Ok(Json(json!({ "text": out.text.trim() })));
    }

    let spec = parsed_kind.prompt_spec(&text, &body.vocabulary);

    match parsed_kind {
        AssistKind::Title | AssistKind::Excerpt => {
            let out: Suggestions =
                vyasa_ai::run_structured(provider.as_ref(), &DbSink { state: &state }, &spec)
                    .await
                    .map_err(ApiError)?;
            Ok(Json(json!({ "suggestions": out.suggestions })))
        }
        AssistKind::Seo => {
            let out: SeoSuggestion =
                vyasa_ai::run_structured(provider.as_ref(), &DbSink { state: &state }, &spec)
                    .await
                    .map_err(ApiError)?;
            Ok(Json(serde_json::to_value(out).map_err(|e| {
                ApiError(vyasa_common::AppError::internal_msg(e.to_string()))
            })?))
        }
        AssistKind::Tags => {
            let out: TagSuggestions =
                vyasa_ai::run_structured(provider.as_ref(), &DbSink { state: &state }, &spec)
                    .await
                    .map_err(ApiError)?;
            Ok(Json(serde_json::to_value(out).map_err(|e| {
                ApiError(vyasa_common::AppError::internal_msg(e.to_string()))
            })?))
        }
        AssistKind::Rewrite
        | AssistKind::Shorten
        | AssistKind::Expand
        | AssistKind::Fix
        | AssistKind::Continue => unreachable!("text kinds returned above"),
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TextResult {
    text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Suggestions {
    suggestions: Vec<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
struct SeoSuggestion {
    meta_title: String,
    meta_description: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
struct TagSuggestions {
    tags: Vec<String>,
    new_tags: Vec<String>,
}
