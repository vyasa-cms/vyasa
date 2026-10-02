//! OpenAI-compatible Chat Completions client.
//!
//! Serves OpenAI itself and OpenRouter, which speaks the same wire format
//! from a different base URL. The one behavioural difference is
//! `response_format`: OpenAI honours `json_object`, while OpenRouter passes
//! it through to vendors that may reject it, so the OpenRouter flavour
//! relies on the system prompt (the runner validates and repairs anyway).

use serde_json::{json, Value};

use super::provider::{Completion, LlmProvider, PromptSpec, ProviderError, Usage};
use crate::anthropic::DEFAULT_TIMEOUT;

/// Output ceiling for structured generation.
///
/// Large enough for a full theme layout plan; the provider's own default is
/// not, on at least one host.
const MAX_STRUCTURED_TOKENS: u32 = 8192;

/// OpenAI-compatible client bound to one API key.
#[derive(Clone)]
pub struct OpenAiProvider {
    key: String,
    base_url: String,
    name: &'static str,
    json_mode: bool,
    inner: reqwest::Client,
    /// From the model's registry settings; the built-in ceiling otherwise.
    max_tokens: Option<u32>,
    /// From the model's registry settings; the provider default otherwise.
    temperature: Option<f32>,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("json_mode", &self.json_mode)
            .field("key", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl OpenAiProvider {
    /// Builds a provider; empty keys are rejected.
    ///
    /// # Errors
    /// [`AppError`]-shaped validation when the key is blank.
    pub fn new(key: &str) -> Result<Self, String> {
        Self::build(key, "openai", "https://api.openai.com/v1", true)
    }

    /// An OpenRouter client: same protocol, different door.
    ///
    /// # Errors
    /// Validation when the key is blank.
    pub fn openrouter(key: &str) -> Result<Self, String> {
        Self::build(key, "openrouter", "https://openrouter.ai/api/v1", false)
    }

    /// Any other server that speaks this protocol: Gemini's and Mistral's
    /// compatible endpoints, Ollama, LM Studio, vLLM. The key may be empty
    /// for a local server; `name` is what audit rows record.
    ///
    /// # Errors
    /// Client construction failure.
    pub fn compatible(key: &str, name: &'static str, base_url: &str) -> Result<Self, String> {
        let key = if key.trim().is_empty() { "no-key" } else { key };
        Self::build(key, name, base_url, false)
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

    fn build(
        key: &str,
        name: &'static str,
        base_url: &str,
        json_mode: bool,
    ) -> Result<Self, String> {
        if key.trim().is_empty() {
            return Err(format!("{name} API key is empty"));
        }
        let inner = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self {
            key: key.to_owned(),
            base_url: base_url.to_owned(),
            max_tokens: None,
            temperature: None,
            name,
            json_mode,
            inner,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .inner
            .request(method, format!("{}/{}", self.base_url, path))
            .header("authorization", format!("Bearer {}", self.key));
        if self.name == "openrouter" {
            // OpenRouter asks callers to identify themselves; both optional.
            req = req
                .header("HTTP-Referer", "https://vyasa.site")
                .header("X-Title", "Vyasa");
        }
        req
    }

    /// Model ids the account can use (`GET /models`).
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    pub async fn list_models(&self) -> Result<Vec<Value>, ProviderError> {
        let response = self
            .request(reqwest::Method::GET, "models")
            .send()
            .await
            .map_err(|e| transport(&e))?;
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
        let body = response
            .text()
            .await
            .map_err(|e| ProviderError::Transport(format!("read: {e}")))?;
        let payload = parse_padded_json(&body)?;
        Ok(payload["data"].as_array().cloned().unwrap_or_default())
    }

    /// Embeds one string; returns the vector's dimensionality.
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    pub async fn embed_probe(&self, model: &str, input: &str) -> Result<usize, ProviderError> {
        let vectors = self.embed(model, &[input.to_owned()]).await?;
        Ok(vectors.first().map_or(0, Vec::len))
    }

    /// Embeds each input, in order.
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    // Providers return doubles on the wire; vectors are stored and compared
    // as f32, and the lost precision is far below what similarity needs.
    #[allow(clippy::cast_possible_truncation)]
    pub async fn embed(
        &self,
        model: &str,
        inputs: &[String],
    ) -> Result<Vec<Vec<f32>>, ProviderError> {
        let payload = self
            .post_json("embeddings", &json!({ "model": model, "input": inputs }))
            .await?;
        let mut rows: Vec<(usize, Vec<f32>)> = payload["data"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let index =
                            usize::try_from(item["index"].as_u64().unwrap_or(0)).unwrap_or(0);
                        let vector = item["embedding"]
                            .as_array()
                            .map(|v| {
                                v.iter()
                                    .filter_map(Value::as_f64)
                                    .map(|f| f as f32)
                                    .collect()
                            })
                            .unwrap_or_default();
                        (index, vector)
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by_key(|(i, _)| *i);
        Ok(rows.into_iter().map(|(_, v)| v).collect())
    }

    /// Classifies text with a moderation model.
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    pub async fn moderate(&self, model: &str, input: &str) -> Result<Moderation, ProviderError> {
        let payload = self
            .post_json("moderations", &json!({ "model": model, "input": input }))
            .await?;
        let result = &payload["results"][0];
        let mut scores: Vec<(String, f64)> = result["category_scores"]
            .as_object()
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f)))
                    .collect()
            })
            .unwrap_or_default();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(Moderation {
            flagged: result["flagged"].as_bool().unwrap_or(false),
            scores,
        })
    }

    /// Generates one image and returns its bytes (PNG for OpenAI models).
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems, or an unexpected shape.
    pub async fn generate_image(
        &self,
        model: &str,
        prompt: &str,
        size: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        use base64::Engine as _;
        let b64 = if self.name == "openrouter" {
            // OpenRouter returns images through chat completions.
            let payload = self
                .post_json(
                    "chat/completions",
                    &json!({
                        "model": model,
                        "messages": [{"role": "user", "content": prompt}],
                        "modalities": ["image", "text"],
                    }),
                )
                .await?;
            let url = payload["choices"][0]["message"]["images"][0]["image_url"]["url"]
                .as_str()
                .ok_or_else(|| ProviderError::Transport("no image in response".into()))?;
            url.split_once(',')
                .map(|(_, data)| data.to_owned())
                .ok_or_else(|| ProviderError::Transport("image is not a data URL".into()))?
        } else {
            let mut body = json!({ "model": model, "prompt": prompt, "n": 1, "size": size });
            if model.starts_with("dall-e") {
                body["response_format"] = json!("b64_json");
            }
            let payload = self.post_json("images/generations", &body).await?;
            payload["data"][0]["b64_json"]
                .as_str()
                .ok_or_else(|| ProviderError::Transport("no image data in response".into()))?
                .to_owned()
        };
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| ProviderError::Transport(format!("image decode: {e}")))
    }

    /// Transcribes an audio file to text.
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    pub async fn transcribe(
        &self,
        model: &str,
        bytes: Vec<u8>,
        file_name: &str,
        mime: &str,
    ) -> Result<String, ProviderError> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name.to_owned())
            .mime_str(mime)
            .map_err(|e| ProviderError::Transport(format!("mime: {e}")))?;
        let form = reqwest::multipart::Form::new()
            .text("model", model.to_owned())
            .text("response_format", "text")
            .part("file", part);
        let response = self
            .request(reqwest::Method::POST, "audio/transcriptions")
            .multipart(form)
            .send()
            .await
            .map_err(|e| transport(&e))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Status(
                status,
                text.chars().take(500).collect(),
            ));
        }
        Ok(text.trim().to_owned())
    }

    /// Reads text aloud; returns MP3 bytes.
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    pub async fn speak(
        &self,
        model: &str,
        input: &str,
        voice: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        let response = self
            .request(reqwest::Method::POST, "audio/speech")
            .json(&json!({ "model": model, "input": input, "voice": voice, "response_format": "mp3" }))
            .send()
            .await
            .map_err(|e| transport(&e))?;
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
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ProviderError::Transport(e.to_string()))
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value, ProviderError> {
        let response = self
            .request(reqwest::Method::POST, path)
            .json(body)
            .send()
            .await
            .map_err(|e| transport(&e))?;
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
        let body = response
            .text()
            .await
            .map_err(|e| ProviderError::Transport(format!("read: {e}")))?;
        parse_padded_json(&body)
    }

    /// Overrides the API base URL (tests).
    #[must_use]
    pub fn with_base_url(mut self, url: &str) -> Self {
        url.clone_into(&mut self.base_url);
        self
    }

    /// One chat completion.
    ///
    /// # Errors
    /// [`ProviderError`] on transport/status problems.
    pub async fn complete(&self, spec: &PromptSpec) -> Result<Completion, ProviderError> {
        let user_content: Value = if spec.attachments.is_empty() {
            Value::String(spec.user.clone())
        } else {
            let mut parts: Vec<Value> = spec
                .attachments
                .iter()
                .map(|a| {
                    json!({"type": "image_url", "image_url": {
                        "url": format!("data:{};base64,{}", a.mime, a.data_b64)}})
                })
                .collect();
            parts.push(json!({"type": "text", "text": spec.user}));
            Value::Array(parts)
        };
        let mut body = json!({
            "model": spec.model,
            "messages": [
                {"role": "system", "content": format!("{}\nRespond with exactly one JSON object conforming to this JSON Schema:\n{}", spec.system, spec.output_schema)},
                {"role": "user", "content": user_content},
            ],
            // Structured replies are whole documents, not sentences: a
            // composed page layout — hero, a grid of cards, a scoped band —
            // runs to several thousand tokens of JSON. Without an explicit
            // ceiling the provider default applies, and on some hosts that is
            // small enough to truncate the object mid-array, which surfaces
            // as "invalid JSON: expected `,` or `]`" rather than as a limit.
            "max_tokens": self.max_tokens.unwrap_or(MAX_STRUCTURED_TOKENS),
        });
        if let Some(t) = self.temperature {
            body["temperature"] = json!(t);
        }
        if self.json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }
        let response = self
            .request(reqwest::Method::POST, "chat/completions")
            .json(&body)
            .send()
            .await
            .map_err(|e| transport(&e))?;
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
        let body = response
            .text()
            .await
            .map_err(|e| ProviderError::Transport(format!("read: {e}")))?;
        let payload = parse_padded_json(&body)?;
        Ok(Completion {
            text: payload["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            usage: Usage {
                prompt_tokens: payload["usage"]["prompt_tokens"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or_else(missing_usage),
                completion_tokens: payload["usage"]["completion_tokens"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or_else(missing_usage),
            },
            provider: None,
            model: None,
        })
    }
}

/// Parses a chat-completions body that may carry keep-alive padding.
///
/// OpenRouter holds the connection open on a slow request by streaming
/// SSE-style comment lines — `: OPENROUTER PROCESSING` — ahead of the real
/// payload, even when the call is not a streaming one. `response.json()` then
/// fails with "error decoding response body", which reads like a network
/// fault but is not: the JSON is there, after the padding.
///
/// A quick probe never waits long enough to see this, which is why a model
/// could pass the admin panel's test and still fail every real generation.
fn parse_padded_json(body: &str) -> Result<Value, ProviderError> {
    let cleaned: String = body
        .lines()
        .filter(|line| {
            let t = line.trim_start();
            // SSE comments start with ':'. Keep genuine JSON, which starts
            // with '{' or '[' — a bare ':' can never begin a document.
            !t.is_empty() && !t.starts_with(':')
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Some hosts prefix each chunk with `data: ` even outside a stream.
    let cleaned = cleaned
        .strip_prefix("data: ")
        .map_or(cleaned.as_str(), |rest| rest)
        .trim();

    serde_json::from_str(cleaned).map_err(|e| {
        let excerpt: String = body.chars().take(300).collect();
        ProviderError::Transport(format!("decode: {e}; body began: {excerpt}"))
    })
}

/// A moderation verdict: the provider's flag plus per-category scores,
/// highest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Moderation {
    /// The provider's own decision.
    pub flagged: bool,
    /// `(category, score)` descending.
    pub scores: Vec<(String, f64)>,
}

fn transport(e: &reqwest::Error) -> ProviderError {
    if e.is_timeout() {
        ProviderError::Timeout(e.to_string())
    } else {
        ProviderError::Transport(e.to_string())
    }
}

impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn complete(
        &self,
        spec: &PromptSpec,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Completion, ProviderError>> + Send>,
    > {
        let this = self.clone();
        let spec = spec.clone();
        Box::pin(async move { this.complete(&spec).await })
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

#[cfg(test)]
mod padded_json_tests {
    use super::parse_padded_json;

    #[test]
    fn plain_json_is_unchanged() {
        let v =
            parse_padded_json(r#"{"choices":[{"message":{"content":"hi"}}]}"#).expect("plain json");
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
    }

    #[test]
    fn openrouter_keep_alive_padding_is_stripped() {
        // What a slow OpenRouter request actually returns: comment lines to
        // hold the connection open, then the payload.
        let body = ": OPENROUTER PROCESSING\n: OPENROUTER PROCESSING\n\n{\"id\":\"x\",\"choices\":[{\"message\":{\"content\":\"{}\"}}]}";
        let v = parse_padded_json(body).expect("padded json");
        assert_eq!(v["id"], "x");
    }

    #[test]
    fn a_data_prefixed_chunk_is_unwrapped() {
        let v = parse_padded_json("data: {\"id\":\"y\"}").expect("data prefix");
        assert_eq!(v["id"], "y");
    }

    #[test]
    fn a_colon_inside_the_json_is_not_mistaken_for_a_comment() {
        let v = parse_padded_json("{\n  \"a\": 1,\n  \"b\": \"c: d\"\n}").expect("json");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "c: d");
    }

    #[test]
    fn genuinely_broken_bodies_still_fail_and_say_what_arrived() {
        let err = parse_padded_json("<html>gateway timeout</html>").expect_err("must fail");
        // The excerpt is what turns an opaque decode error into a diagnosis,
        // so it still has to be there -- it just belongs in the log now, not
        // in the sentence an author reads.
        let detail = err.detail();
        assert!(detail.contains("decode"), "{detail}");
        assert!(detail.contains("gateway timeout"), "{detail}");

        // And it must not come back out the other way: a body arriving as
        // HTML, JSON or anything else is not something to put on screen.
        let shown = err.to_string();
        assert!(!shown.contains("gateway timeout"), "{shown}");
        assert!(!shown.contains("<html>"), "{shown}");
    }
}
