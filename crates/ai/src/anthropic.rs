//! Anthropic Messages API client (tool-use forced JSON mode).

use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};

use vyasa_common::AppError;

use super::provider::{Completion, LlmProvider, PromptSpec, ProviderError, Usage};

/// Default request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Anthropic client bound to one API key.
pub struct AnthropicProvider {
    key: String,
    client: Client,
    base_url: String,
    /// From the model's registry settings; 4096 otherwise.
    max_tokens: Option<u32>,
    /// From the model's registry settings; the provider default otherwise.
    temperature: Option<f32>,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the key.
        f.debug_struct("AnthropicProvider")
            .field("key", &"[redacted]")
            .finish()
    }
}

impl AnthropicProvider {
    /// Builds a provider; returns [`AppError::Validation`] when the key is
    /// empty.
    ///
    /// # Errors
    /// See above.
    pub fn new(key: &str) -> Result<Self, AppError> {
        if key.trim().is_empty() {
            return Err(AppError::validation(
                "ANTHROPIC API key is empty (set VYASA_AI_ANTHROPIC_KEY)",
            ));
        }
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| AppError::internal_msg(format!("http client: {e}")))?;
        Ok(Self {
            key: key.to_owned(),
            client,
            base_url: String::from("https://api.anthropic.com/v1"),
            max_tokens: None,
            temperature: None,
        })
    }

    /// Applies a model's registry settings: `max_tokens`, `temperature`.
    #[must_use]
    pub fn with_settings(mut self, settings: &Value) -> Self {
        self.max_tokens = settings["max_tokens"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok());
        #[allow(clippy::cast_possible_truncation)]
        {
            self.temperature = settings["temperature"].as_f64().map(|t| t as f32);
        }
        self
    }

    /// Overrides the API base URL (tests point this at the mock server).
    #[must_use]
    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = url.into();
        self
    }

    /// Model ids the key can use (`GET /models`).
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    pub async fn list_models(&self) -> Result<Vec<Value>, ProviderError> {
        let response = self
            .client
            .get(format!("{}/models?limit=100", self.base_url))
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        if status != 200 {
            let excerpt: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect();
            return Err(ProviderError::Status(status, excerpt));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("decode: {e}")))?;
        Ok(payload["data"].as_array().cloned().unwrap_or_default())
    }

    /// Issues one messages call and extracts text + usage.
    ///
    /// # Errors
    /// [`ProviderError`] variants for transport/status problems.
    pub async fn complete(&self, spec: &PromptSpec) -> Result<Completion, ProviderError> {
        // A vision request carries its images as content blocks before the
        // text; a plain request keeps the simpler string form.
        let user_content: Value = if spec.attachments.is_empty() {
            Value::String(spec.user.clone())
        } else {
            let mut blocks: Vec<Value> = spec
                .attachments
                .iter()
                .map(|a| {
                    json!({"type": "image", "source": {
                        "type": "base64", "media_type": a.mime, "data": a.data_b64}})
                })
                .collect();
            blocks.push(json!({"type": "text", "text": spec.user}));
            Value::Array(blocks)
        };
        let mut body = json!({
            "model": spec.model,
            "max_tokens": self.max_tokens.unwrap_or(4096),
            "system": spec.system,
            "messages": [{ "role": "user", "content": user_content }],
            "tools": [{
                "name": "emit_result",
                "description":
                    "Emit the final structured result conforming to the requested schema",
                "input_schema": spec.output_schema,
            }],
            "tool_choice": { "type": "tool", "name": "emit_result" },
        });
        if let Some(t) = self.temperature {
            body["temperature"] = json!(t);
        }

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ProviderError::Timeout(e.to_string())
                } else {
                    ProviderError::Transport(e.to_string())
                }
            })?;

        let status = response.status().as_u16();
        if status != 200 {
            let excerpt = response.text().await.unwrap_or_default();
            let excerpt = excerpt.chars().take(500).collect::<String>();
            return Err(ProviderError::Status(status, excerpt));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("decode: {e}")))?;

        // Extract concatenated text blocks.
        let text = payload["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<String>()
            })
            .unwrap_or_default();

        // When tool_choice forces the tool, arguments arrive as a
        // tool_use block's input; prefer it over plain text.
        let tool_input = payload["content"]
            .as_array()
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find_map(|b| (b["type"] == "tool_use").then(|| b["input"].clone()))
            })
            .unwrap_or(Value::Null);

        let final_text = if tool_input.is_null() {
            text
        } else {
            tool_input.to_string()
        };

        Ok(Completion {
            text: final_text,
            usage: Usage {
                prompt_tokens: payload["usage"]["input_tokens"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or_else(missing_usage),
                completion_tokens: payload["usage"]["output_tokens"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or_else(missing_usage),
            },
            provider: None,
            model: None,
        })
    }
}

impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn complete(
        &self,
        spec: &PromptSpec,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Completion, ProviderError>> + Send>,
    > {
        // Clone the borrowed spec into the boxed future: `self` lives as
        // long as the provider, but the borrow of `spec` cannot.
        let this = std::sync::Arc::new(self.clone_shim());
        let spec = spec.clone();
        Box::pin(async move { this.complete_inner(&spec).await })
    }
}

impl AnthropicProvider {
    /// Arc-friendly inner completion used by the trait adapter.
    async fn complete_inner(&self, spec: &PromptSpec) -> Result<Completion, ProviderError> {
        self.complete(spec).await
    }

    /// Cheap clone shim (key + client are cheap handles).
    fn clone_shim(&self) -> AnthropicProvider {
        Self {
            key: self.key.clone(),
            client: self.client.clone(),
            base_url: self.base_url.clone(),
            max_tokens: self.max_tokens,
            temperature: self.temperature,
        }
    }
}

/// What a reply with no usage block is billed as.
///
/// It used to be `u32::MAX` on both counters — a provider that omitted the
/// block (some proxies and free tiers do) was recorded as four billion
/// tokens each way, which put an absurd cost in the logs and could trip a
/// monthly budget on one call. Unknown is zero with a note in the log,
/// which is what it actually is: unbilled, not infinite.
fn missing_usage() -> u32 {
    tracing::warn!("provider reply carried no usage; recording zero tokens");
    0
}
