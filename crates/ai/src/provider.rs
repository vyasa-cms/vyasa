//! LLM provider abstraction: prompt spec in, structured JSON out.

use std::collections::HashMap;
use std::hash::BuildHasher;

use serde::{Deserialize, Serialize};
use std::fmt;

/// An inline attachment (an image, for vision models), base64-encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// MIME type, e.g. `image/jpeg`.
    pub mime: String,
    /// Base64 payload.
    pub data_b64: String,
}

/// A complete request to an LLM provider.
#[derive(Debug, Clone, Default)]
pub struct PromptSpec {
    /// Stable identifier for audit rows (`theme-gen`, `editor-assist`, …).
    pub purpose: String,
    /// Model identifier (e.g. `claude-sonnet-4-5`).
    pub model: String,
    /// System prompt.
    pub system: String,
    /// User turn.
    pub user: String,
    /// JSON Schema the response must validate against. The provider is
    /// instructed to emit exactly one JSON object conforming to it.
    pub output_schema: serde_json::Value,
    /// Images to show a vision model alongside the user turn.
    pub attachments: Vec<Attachment>,
}

/// Token usage reported by the provider for one completion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Input tokens.
    pub prompt_tokens: u32,
    /// Output tokens.
    pub completion_tokens: u32,
}

/// Raw text completion from a provider plus usage.
#[derive(Debug, Clone, Default)]
pub struct Completion {
    /// The assistant's text (expected to be one JSON object).
    pub text: String,
    /// Token accounting.
    pub usage: Usage,
    /// Which provider actually answered, when a failover chain was used and
    /// it may differ from the caller's first choice. `None` = the provider
    /// that was called.
    pub provider: Option<&'static str>,
    /// Which model actually answered (see `provider`).
    pub model: Option<String>,
}

/// What actually went wrong, whatever wording the provider chose for it.
///
/// Providers describe the same handful of situations in wildly different
/// ways — OpenRouter returns several hundred characters of nested JSON for
/// a rate limit — so the wording is classified once, here, and everything
/// downstream works from the classification rather than from prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// Too many requests, on our key or on the pool behind it.
    RateLimited,
    /// The key is missing, wrong, or not entitled to this model.
    Auth,
    /// The account has no credit left.
    Credits,
    /// The provider does not serve a model by that name.
    UnknownModel,
    /// The provider refused the request itself.
    Rejected,
    /// The provider is up but could not answer right now.
    Overloaded,
    /// Nothing came back in time.
    Timeout,
    /// The provider could not be reached at all.
    Network,
    /// Something came back, but not something we can use.
    Unusable,
}

impl Cause {
    /// What to tell the person looking at the screen.
    ///
    /// One or two short sentences: what happened, and what they can do. The
    /// provider's own body is deliberately never quoted — it arrives as
    /// JSON, runs to several hundred characters, and was being cut off
    /// mid-word in every place it was shown.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::RateLimited => {
                "Rate limited by the provider. Try again shortly, or add your own key for it."
            }
            Self::Auth => "The provider rejected the API key. Check it under AI models.",
            Self::Credits => "The provider account is out of credit.",
            Self::UnknownModel => {
                "The provider does not offer this model. Choose another under AI models."
            }
            Self::Rejected => "The provider refused the request.",
            Self::Overloaded => "The provider is busy. Try again shortly.",
            Self::Timeout => "The provider did not answer in time. Try again.",
            Self::Network => "Could not reach the provider.",
            Self::Unusable => "The provider's answer could not be read.",
        }
    }

    /// How much a human needs to intervene; lower comes first.
    ///
    /// When a failover chain fails for several different reasons, the one
    /// worth reporting is the one that will not fix itself: a wrong key is
    /// more useful to hear about than a rate limit that clears in a minute.
    const fn rank(self) -> u8 {
        match self {
            Self::Auth => 0,
            Self::Credits => 1,
            Self::UnknownModel => 2,
            Self::Rejected => 3,
            Self::RateLimited => 4,
            Self::Overloaded => 5,
            Self::Timeout => 6,
            Self::Network => 7,
            Self::Unusable => 8,
        }
    }

    /// The one worth reporting out of everything a chain hit.
    #[must_use]
    pub fn most_actionable(causes: impl IntoIterator<Item = Self>) -> Option<Self> {
        causes.into_iter().min_by_key(|c| c.rank())
    }
}

/// Provider-agnostic transport failure.
#[derive(Debug)]
pub enum ProviderError {
    /// HTTP/serialization problem; message is redacted-safe.
    Transport(String),
    /// Non-success status from the API with body excerpt.
    Status(u16, String),
    /// Timeout elapsed.
    Timeout(String),
    /// Every model in a failover chain failed. Per-attempt detail is logged
    /// where it happens; carrying it here only puts it back on screen.
    AllFailed {
        /// The cause worth reporting out of everything the chain hit.
        cause: Cause,
        /// How many models were tried.
        tried: usize,
    },
}

impl ProviderError {
    /// What went wrong, independent of how the provider worded it.
    #[must_use]
    pub fn cause(&self) -> Cause {
        match self {
            Self::AllFailed { cause, .. } => *cause,
            Self::Timeout(_) => Cause::Timeout,
            Self::Transport(m) => {
                // A body that arrived but would not parse is a different
                // problem from never reaching the provider, and only one of
                // them is worth retrying.
                let m = m.to_ascii_lowercase();
                if m.contains("decode") || m.contains("decoding") || m.contains("no image") {
                    Cause::Unusable
                } else {
                    Cause::Network
                }
            }
            Self::Status(code, _) => match *code {
                401 | 403 => Cause::Auth,
                402 => Cause::Credits,
                404 => Cause::UnknownModel,
                408 => Cause::Timeout,
                429 => Cause::RateLimited,
                500..=599 => Cause::Overloaded,
                _ => Cause::Rejected,
            },
        }
    }

    /// The raw provider wording, for logs only.
    ///
    /// Kept off [`fmt::Display`] on purpose: everything that formats an
    /// error ends up showing it to somebody, and this is the text that was
    /// arriving as a wall of truncated JSON.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Transport(m) => format!("transport: {m}"),
            Self::Status(code, m) => format!("status {code}: {m}"),
            Self::Timeout(m) => format!("timeout: {m}"),
            Self::AllFailed { cause, tried } => {
                format!("all {tried} models failed ({cause:?})")
            }
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllFailed { cause, tried } if *tried > 1 => {
                write!(
                    f,
                    "No model could answer ({tried} tried). {}",
                    cause.message()
                )
            }
            other => f.write_str(other.cause().message()),
        }
    }
}

/// Async provider surface. Implementations must never log the API key and
/// must redact prompts from error messages.
pub trait LlmProvider: Send + Sync {
    /// Human-readable provider name for ai_logs.
    fn name(&self) -> &'static str;

    /// Completes one [`PromptSpec`].
    fn complete(
        &self,
        spec: &PromptSpec,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Completion, ProviderError>> + Send>,
    >;
}

/// Cost table entry: USD per million tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ModelCost {
    /// USD per 1M input tokens.
    pub input_per_mtok: f64,
    /// USD per 1M output tokens.
    pub output_per_mtok: f64,
}

/// Computes the cost of one completion from the table; unknown models
/// price at zero (visible in logs as 0.0 rather than failing).
#[must_use]
pub fn compute_cost<S: BuildHasher>(
    table: &HashMap<String, ModelCost, S>,
    model: &str,
    usage: Usage,
) -> f64 {
    let Some(cost) = table.get(model) else {
        return 0.0;
    };
    let input = f64::from(usage.prompt_tokens) / 1_000_000.0 * cost.input_per_mtok;
    let output = f64::from(usage.completion_tokens) / 1_000_000.0 * cost.output_per_mtok;
    input + output
}

/// Turns a provider failure into the application error.
///
/// The provider's own wording goes to the log; the classified sentence goes
/// to the caller. Both matter, to different people: an operator wants the
/// JSON body, and an author wants to know whether to wait a minute or fix a
/// key. Sending one string to both was how a rate limit ended up on screen
/// as three hundred characters of nested JSON, cut off mid-word.
#[must_use]
pub fn external_error(provider: &'static str, e: &ProviderError) -> vyasa_common::AppError {
    tracing::warn!(
        provider,
        cause = ?e.cause(),
        detail = %e.detail(),
        "provider call failed"
    );
    vyasa_common::AppError::external(provider, e.to_string())
}
