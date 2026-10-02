//! Mocked end-to-end: PromptSpec -> validated JSON with retries; usage
//! rows recorded; breaker opens on consecutive failures.

#![allow(clippy::pedantic, clippy::unwrap_used)]

use std::sync::Mutex;

use schemars::JsonSchema;
use serde::Deserialize;
use vyasa_ai::{
    run_structured, AiUsageSink, Completion, LlmProvider, PromptSpec, ProviderError, Usage,
};

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct ThemeResult {
    name: String,
    primary_color: String,
}

fn spec() -> PromptSpec {
    PromptSpec {
        attachments: Vec::new(),
        purpose: "theme-gen".into(),
        model: "claude-sonnet-4-5".into(),
        system: "You generate themes.".into(),
        user: "Make a blue theme.".into(),
        output_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "primary_color": {"type": "string"}
            },
            "required": ["name", "primary_color"]
        }),
    }
}

#[derive(Default, Clone)]
struct RecordingSink {
    successes: std::sync::Arc<Mutex<Vec<(String, u32, u32)>>>,
    failures: std::sync::Arc<Mutex<Vec<String>>>,
}

impl AiUsageSink for RecordingSink {
    fn log_success(
        &self,
        provider: &str,
        model: &str,
        _purpose: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) {
        self.successes.lock().unwrap().push((
            format!("{provider}:{model}"),
            prompt_tokens,
            completion_tokens,
        ));
    }

    fn log_failure(&self, provider: &str, model: &str, _purpose: &str, error: &str) {
        self.failures
            .lock()
            .unwrap()
            .push(format!("{provider}:{model}: {error}"));
    }
}

/// Scripted provider returning canned responses in order.
struct ScriptedProvider {
    responses: Mutex<Vec<Result<String, ProviderError>>>,
    usage: Usage,
}

impl LlmProvider for ScriptedProvider {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn complete(
        &self,
        _spec: &PromptSpec,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Completion, ProviderError>> + Send>,
    > {
        let mut queue = self.responses.lock().unwrap();
        let response = if queue.is_empty() {
            Err(ProviderError::Transport(String::from("exhausted")))
        } else {
            queue.remove(0)
        };
        Box::pin(std::future::ready(response.map(|text| Completion {
            text,
            usage: self.usage,
            provider: None,
            model: None,
        })))
    }
}

fn good_json() -> String {
    serde_json::json!({"name": "ocean", "primary_color": "#1e3a8a"}).to_string()
}

#[tokio::test]
async fn happy_path_validates_and_logs() {
    let sink = RecordingSink::default();
    let provider = ScriptedProvider {
        responses: Mutex::new(vec![Ok(good_json())]),
        usage: Usage {
            prompt_tokens: 100,
            completion_tokens: 20,
        },
    };
    let result: ThemeResult = run_structured(&provider, &sink, &spec())
        .await
        .expect("valid");
    assert_eq!(result.name, "ocean");
    assert_eq!(result.primary_color, "#1e3a8a");
    let logged = sink.successes.lock().unwrap();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].0, "scripted:claude-sonnet-4-5");
    assert_eq!(logged[0].1, 100);
    assert_eq!(logged[0].2, 20);
}

#[tokio::test]
async fn invalid_json_retries_then_succeeds() {
    let sink = RecordingSink::default();
    let provider = ScriptedProvider {
        responses: Mutex::new(vec![Ok(String::from("not json at all")), Ok(good_json())]),
        usage: Usage {
            prompt_tokens: 50,
            completion_tokens: 10,
        },
    };
    let result: ThemeResult = run_structured(&provider, &sink, &spec())
        .await
        .expect("second attempt wins");
    assert_eq!(result.name, "ocean");
    assert_eq!(
        sink.failures.lock().unwrap().len(),
        1,
        "first attempt audited as failure"
    );
    assert_eq!(sink.successes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn schema_mismatch_fails_all_attempts() {
    let sink = RecordingSink::default();
    let provider = ScriptedProvider {
        responses: Mutex::new(vec![
            Ok(String::from("{\"wrong\": true}")),
            Ok(String::from("{\"also\": \"wrong\"}")),
            Ok(String::from("{\"still\": \"wrong\"}")),
        ]),
        usage: Usage::default(),
    };
    let result: Result<ThemeResult, _> = run_structured(&provider, &sink, &spec()).await;
    assert!(result.is_err(), "all three attempts invalid");
    assert_eq!(sink.failures.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn transport_error_bubbles_immediately() {
    let sink = RecordingSink::default();
    let provider = ScriptedProvider {
        responses: Mutex::new(vec![Err(ProviderError::Timeout(String::from("60s")))]),
        usage: Usage::default(),
    };
    let result: Result<ThemeResult, _> = run_structured(&provider, &sink, &spec()).await;
    assert!(result.is_err());
    assert_eq!(sink.failures.lock().unwrap().len(), 1);
}

// --- circuit breaker ----------------------------------------------------------

#[tokio::test]
async fn breaker_opens_and_cools_down() {
    use std::time::Duration;
    use vyasa_ai::CircuitBreaker;

    let breaker = CircuitBreaker::new(3, Duration::from_millis(30));
    assert!(breaker.allows());
    breaker.record_failure();
    breaker.record_failure();
    assert!(breaker.allows(), "below threshold still closed");
    breaker.record_failure();
    assert!(!breaker.allows(), "open after threshold");
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(breaker.allows(), "cooldown elapsed -> probe allowed");
    breaker.record_success();
    breaker.record_failure();
    assert!(
        breaker.allows(),
        "success resets the consecutive count before the new failure"
    );
}
