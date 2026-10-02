//! Structured-output runner: the ONLY way feature code calls LLMs.
//!
//! Pipeline: provider completion → JSON parse → schemars/serde validation
//! → retry (max 2) with validation-error feedback → cost-logged result.

use vyasa_common::AppError;

use super::provider::{LlmProvider, PromptSpec};
use super::usage::AiUsageSink;

/// Maximum attempts including the first (1 initial + 2 repairs).
pub const MAX_ATTEMPTS: u32 = 3;

/// Runs `spec` against `provider`, validating the response as `T`.
///
/// Invalid output triggers up to [`MAX_ATTEMPTS`] - 1 repair prompts whose
/// user turn includes the validation errors. Every attempt is logged to
/// the usage sink; the winning attempt's cost is what lands in ai_logs.
///
/// # Errors
/// [`AppError::External`] when all attempts fail validation or the
/// provider errors repeatedly. Transport failures bubble immediately
/// (retry only covers *invalid content*, not outages — the circuit breaker
/// handles those).
#[allow(clippy::too_many_arguments)]
pub async fn run_structured<T, P>(
    provider: &P,
    sink: &dyn AiUsageSink,
    spec: &PromptSpec,
) -> Result<T, AppError>
where
    T: serde::de::DeserializeOwned + schemars::JsonSchema,
    P: LlmProvider,
{
    let mut spec = spec.clone();
    let mut last_error = String::new();

    for attempt in 1..=MAX_ATTEMPTS {
        let completion = match provider.complete(&spec).await {
            Ok(c) => c,
            Err(e) => {
                sink.log_failure(provider.name(), &spec.model, &spec.purpose, &e.detail());
                return Err(crate::provider::external_error(provider.name(), &e));
            }
        };

        // Deserialization on `T` enforces the schema contract: unknown or
        // missing fields fail here and feed the repair prompt.
        let parsed: Result<T, String> = serde_json::from_str(json_payload(&completion.text))
            .map_err(|e| format!("invalid JSON: {e}"));

        match parsed {
            Ok(value) => {
                sink.log_success(
                    completion.provider.unwrap_or_else(|| provider.name()),
                    completion.model.as_deref().unwrap_or(&spec.model),
                    &spec.purpose,
                    completion.usage.prompt_tokens,
                    completion.usage.completion_tokens,
                );
                return Ok(value);
            }
            Err(reason) => {
                last_error.clone_from(&reason);
                sink.log_failure(provider.name(), &spec.model, &spec.purpose, &reason);
                sink.log_spend(
                    provider.name(),
                    &spec.model,
                    &spec.purpose,
                    completion.usage.prompt_tokens,
                    completion.usage.completion_tokens,
                );
                if attempt < MAX_ATTEMPTS {
                    // Repair prompt: include the previous output and errors.
                    spec.user = format!(
                        "{}\n\nYour previous response was invalid: {reason}\n\
                         Respond again with exactly one JSON object conforming \
                         to the schema.",
                        spec.user
                    );
                }
            }
        }
    }
    // "structured output failed after 3 attempts: invalid JSON: expected
    // `,` at line 1 column 812" tells an author nothing they can act on.
    // The parser's complaint is for the log; the choice of another model is
    // the only thing they can actually do about it.
    tracing::warn!(
        provider = provider.name(),
        model = %spec.model,
        purpose = %spec.purpose,
        attempts = MAX_ATTEMPTS,
        last_error = %last_error,
        "structured output never validated"
    );
    Err(AppError::external(
        provider.name(),
        "The model kept replying with something unusable. Try again, or \
         choose another model under AI models.",
    ))
}

/// The JSON object inside a model reply.
///
/// Models asked for "exactly one JSON object" routinely wrap it in a fenced
/// code block, and some prepend a sentence. Both are trivially recoverable,
/// and refusing them costs a whole retry — so trim to the outermost braces
/// rather than failing on the packaging.
///
/// Returns the input trimmed when no object is found, so a genuine syntax
/// error still surfaces as one.
fn json_payload(raw: &str) -> &str {
    let text = raw.trim();
    let body = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .map_or(text, |rest| rest.trim_start())
        .trim_end()
        .strip_suffix("```")
        .map_or_else(|| text.trim_end(), str::trim_end);

    let body = body.trim();
    match (body.find('{'), body.rfind('}')) {
        (Some(start), Some(end)) if end > start => &body[start..=end],
        _ => body,
    }
}

#[cfg(test)]
mod payload_tests {
    use super::json_payload;

    #[test]
    fn a_bare_object_is_returned_as_is() {
        assert_eq!(json_payload(r#"{"a":1}"#), r#"{"a":1}"#);
    }

    #[test]
    fn a_fenced_block_is_unwrapped() {
        assert_eq!(json_payload("```json\n{\"a\":1}\n```"), r#"{"a":1}"#);
        assert_eq!(json_payload("```\n{\"a\":1}\n```"), r#"{"a":1}"#);
    }

    #[test]
    fn surrounding_prose_is_discarded() {
        assert_eq!(
            json_payload("Here you go:\n{\"a\":1}\nHope that helps."),
            r#"{"a":1}"#
        );
    }

    #[test]
    fn nested_braces_keep_the_whole_object() {
        let raw = r#"{"ops":[{"op":"set_layout","blocks":[{"id":"a"}]}]}"#;
        assert_eq!(json_payload(raw), raw);
    }

    #[test]
    fn a_reply_with_no_object_is_left_alone_so_the_error_is_real() {
        assert_eq!(json_payload("I cannot do that"), "I cannot do that");
    }
}
