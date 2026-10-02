//! Ordered failover across providers with per-provider breakers.

use vyasa_common::AppError;

use super::provider::{Completion, LlmProvider, PromptSpec};

/// One provider in the ordered chain.
pub struct ProviderSlot {
    /// The provider.
    pub provider: std::sync::Arc<dyn LlmProvider>,
    /// Consecutive-failure breaker guarding this slot.
    pub breaker: super::circuit::CircuitBreaker,
}

/// Tries providers in order; the first success wins. A provider whose
/// breaker is open is skipped; a failed attempt records on its breaker and
/// moves to the next slot.
///
/// # Errors
/// Returns [`AppError::External`] naming every attempted provider when all
/// fail (or all breakers are open).
pub async fn complete_with_failover(
    slots: &[ProviderSlot],
    spec: &PromptSpec,
) -> Result<(&'static str, Completion), AppError> {
    let mut attempts: Vec<String> = Vec::new();
    let mut causes: Vec<crate::provider::Cause> = Vec::new();
    for slot in slots {
        if !slot.breaker.allows() {
            attempts.push(format!("{}: circuit open", slot.provider.name()));
            causes.push(crate::provider::Cause::Overloaded);
            continue;
        }
        match slot.provider.complete(spec).await {
            Ok(completion) => {
                slot.breaker.record_success();
                return Ok((slot.provider.name(), completion));
            }
            Err(e) => {
                // Only infrastructure failures trip the breaker; invalid
                // content retries are the runner's job. 4xx are caller
                // errors, not outages.
                let transient = !matches!(e, crate::provider::ProviderError::Status(400..=499, _));
                if transient {
                    slot.breaker.record_failure();
                }
                causes.push(e.cause());
                attempts.push(format!("{}: {}", slot.provider.name(), e.detail()));
            }
        }
    }
    tracing::warn!(attempts = %attempts.join(" | "), "every provider failed");
    let tried = attempts.len();
    Err(AppError::external(
        "ai",
        crate::provider::ProviderError::AllFailed {
            cause: crate::provider::Cause::most_actionable(causes)
                .unwrap_or(crate::provider::Cause::Network),
            tried,
        }
        .to_string(),
    ))
}

/// A provider in a chain together with the model to ask it for. The
/// registry produces these: the default model of a kind first, then the
/// other enabled models, possibly at other providers.
pub struct ModelSlot {
    /// Provider and breaker.
    pub slot: ProviderSlot,
    /// The model id to use at this provider.
    pub model: String,
}

/// An [`LlmProvider`] over an ordered chain of `(provider, model)` pairs.
///
/// Lets the structured runner and the theme generator — written against a
/// single provider — use the registry's failover chain unchanged. Each
/// slot gets the spec with *its* model substituted, and the completion
/// records which provider and model actually answered so usage logs stay
/// truthful when the first choice was down.
pub struct FailoverProvider {
    slots: std::sync::Arc<Vec<ModelSlot>>,
    name: &'static str,
}

impl FailoverProvider {
    /// Builds the chain. The first slot names the provider for callers that
    /// need one before a call is made.
    ///
    /// # Errors
    /// [`AppError::Validation`] when the chain is empty.
    pub fn new(slots: Vec<ModelSlot>) -> Result<Self, AppError> {
        let Some(first) = slots.first() else {
            return Err(AppError::validation(
                "no model is registered for this job: add one on the Models page",
            ));
        };
        let name = first.slot.provider.name();
        Ok(Self {
            slots: std::sync::Arc::new(slots),
            name,
        })
    }

    /// The model the first slot will use — what callers should put in a
    /// [`PromptSpec`] so logs and cost tables line up.
    #[must_use]
    pub fn primary_model(&self) -> &str {
        self.slots.first().map_or("", |s| s.model.as_str())
    }
}

/// Tries each slot in order with its own model.
async fn run_chain(
    slots: &[ModelSlot],
    spec: &PromptSpec,
) -> Result<Completion, crate::provider::ProviderError> {
    let mut attempts: Vec<String> = Vec::new();
    let mut causes: Vec<crate::provider::Cause> = Vec::new();
    for entry in slots {
        let slot = &entry.slot;
        let label = format!("{}/{}", slot.provider.name(), entry.model);
        if !slot.breaker.allows() {
            attempts.push(format!("{label}: circuit open"));
            causes.push(crate::provider::Cause::Overloaded);
            continue;
        }
        let mut own = spec.clone();
        own.model.clone_from(&entry.model);
        match slot.provider.complete(&own).await {
            Ok(mut completion) => {
                slot.breaker.record_success();
                completion.provider = Some(slot.provider.name());
                completion.model = Some(entry.model.clone());
                return Ok(completion);
            }
            Err(e) => {
                let transient = !matches!(e, crate::provider::ProviderError::Status(400..=499, _));
                if transient {
                    slot.breaker.record_failure();
                }
                causes.push(e.cause());
                attempts.push(format!("{label}: {}", e.detail()));
            }
        }
    }
    // The provider wording is worth keeping, but in the log rather than on
    // the caller's screen: joined together it ran to a paragraph of nested
    // JSON that every surface then truncated mid-word.
    tracing::warn!(
        attempts = %attempts.join(" | "),
        "every model in the chain failed"
    );
    let tried = attempts.len();
    Err(crate::provider::ProviderError::AllFailed {
        cause: crate::provider::Cause::most_actionable(causes)
            .unwrap_or(crate::provider::Cause::Network),
        tried,
    })
}

impl LlmProvider for FailoverProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn complete(
        &self,
        spec: &PromptSpec,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Completion, crate::provider::ProviderError>>
                + Send,
        >,
    > {
        let slots = std::sync::Arc::clone(&self.slots);
        let spec = spec.clone();
        Box::pin(async move { run_chain(&slots, &spec).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use crate::provider::{Completion, ProviderError, Usage};

    struct Scripted {
        name: &'static str,
        remaining_failures: AtomicU32,
        error_kind: &'static str,
    }

    impl Scripted {
        fn new(name: &'static str, fails: u32, kind: &'static str) -> Self {
            Self {
                name,
                remaining_failures: AtomicU32::new(fails),
                error_kind: kind,
            }
        }
    }

    impl LlmProvider for Scripted {
        fn name(&self) -> &'static str {
            self.name
        }

        fn complete(
            &self,
            _spec: &PromptSpec,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Completion, crate::provider::ProviderError>>
                    + Send,
            >,
        > {
            let remaining = self.remaining_failures.fetch_sub(1, Ordering::Relaxed);
            let kind = self.error_kind;
            Box::pin(ready(if remaining > 0 {
                Err(match kind {
                    "status" => ProviderError::Status(503, String::from("upstream")),
                    _ => ProviderError::Transport(String::from("net")),
                })
            } else {
                Ok(Completion {
                    text: String::from("{}"),
                    usage: Usage {
                        prompt_tokens: 1,
                        completion_tokens: 1,
                    },
                    provider: None,
                    model: None,
                })
            }))
        }
    }

    fn spec() -> PromptSpec {
        PromptSpec {
            attachments: Vec::new(),
            purpose: String::from("t"),
            model: String::from("m"),
            system: String::new(),
            user: String::from("u"),
            output_schema: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn primary_500_falls_over_to_secondary() {
        let slots = vec![
            ProviderSlot {
                provider: Arc::new(Scripted::new("anthropic", 1, "status")) as _,
                breaker: crate::CircuitBreaker::new(5, Duration::from_secs(60)),
            },
            ProviderSlot {
                provider: Arc::new(Scripted::new("openai", 0, "status")) as _,
                breaker: crate::CircuitBreaker::new(5, Duration::from_secs(60)),
            },
        ];
        let (used, completion) = complete_with_failover(&slots, &spec())
            .await
            .expect("secondary serves");
        assert_eq!(used, "openai");
        assert_eq!(completion.text, "{}");
        assert!(
            slots[0].breaker.allows(),
            "single failure must not open a threshold-5 breaker"
        );
    }

    #[tokio::test]
    async fn all_fail_reports_how_many_and_why_but_not_the_raw_bodies() {
        let slots = vec![
            ProviderSlot {
                provider: Arc::new(Scripted::new("anthropic", 2, "transport")) as _,
                breaker: crate::CircuitBreaker::new(50, Duration::from_secs(60)),
            },
            ProviderSlot {
                provider: Arc::new(Scripted::new("openai", 2, "transport")) as _,
                breaker: crate::CircuitBreaker::new(50, Duration::from_secs(60)),
            },
        ];
        let Err(err) = complete_with_failover(&slots, &spec()).await else {
            panic!("expected total failure");
        };
        // Which providers were tried, and what each said, goes to the log.
        // What comes back is what an author can act on: how many were tried
        // and the one cause worth reporting.
        let msg = err.to_string();
        assert!(msg.contains("2 tried"), "{msg}");
        assert!(msg.contains("Could not reach the provider"), "{msg}");
        // The provider bodies must not ride along. This is the regression
        // that put three hundred characters of nested JSON on screen.
        assert!(!msg.contains("transport"), "{msg}");
    }

    #[tokio::test]
    async fn open_circuit_is_skipped() {
        let breaker = crate::CircuitBreaker::new(1, Duration::from_secs(60));
        breaker.record_failure(); // opens immediately
        let slots = vec![
            ProviderSlot {
                provider: Arc::new(Scripted::new("anthropic", 0, "transport")) as _,
                breaker,
            },
            ProviderSlot {
                provider: Arc::new(Scripted::new("openai", 0, "transport")) as _,
                breaker: crate::CircuitBreaker::new(5, Duration::from_secs(60)),
            },
        ];
        let (used, _) = complete_with_failover(&slots, &spec())
            .await
            .expect("serves");
        assert_eq!(used, "openai", "open-circuit provider skipped");
    }
}
