//! A layer at each arm's model seam. It notes a reply that carries no
//! usage (spend that would go uncounted), and injects the one fault a brief
//! can ask for: a malformed first step, without calling the model.

use std::future::{ready, Future};
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sarathi_core::driver::{BoxFuture, CallFailure, ModelCaller, Received};
use sarathi_core::ToolDef;
use sarathi_log::RequestParts;
use sarathi_protocol::{CallId, RawJson};
use serde_json::json;
use vyasa_ai::harness::{Wire, WrapCaller};
use vyasa_ai::{Completion, LlmProvider, PromptSpec, ProviderError};

/// The brief fault this crate injects.
pub const MALFORMED_STEPS: &str = "malformed_steps";

/// The tool a malformed native step calls; no surface has it.
const NO_SUCH_TOOL: &str = "apply_theme";

/// What the old loop reads as its malformed step: not the JSON object it
/// asked for.
const MALFORMED_TEXT: &str = "{\"thought\": \"edit the colours\", \"tool\": ";

/// One run's state at the model seam.
#[derive(Debug, Default)]
pub struct Probe {
    inject: AtomicBool,
    unbilled: AtomicBool,
}

impl Probe {
    /// A run's probe; `malformed` makes the first model call answer with a
    /// malformed step.
    #[must_use]
    pub fn new(malformed: bool) -> Arc<Self> {
        Arc::new(Self {
            inject: AtomicBool::new(malformed),
            unbilled: AtomicBool::new(false),
        })
    }

    /// Whether a real reply carried no usage.
    #[must_use]
    pub fn unbilled(&self) -> bool {
        self.unbilled.load(Ordering::SeqCst)
    }

    fn take_fault(&self) -> bool {
        self.inject.swap(false, Ordering::SeqCst)
    }

    fn saw_tokens(&self, tokens: u64) {
        if tokens == 0 {
            self.unbilled.store(true, Ordering::SeqCst);
        }
    }
}

/// The old arm's provider with the probe around it.
pub struct ProbeProvider<P> {
    /// The provider that is called.
    pub inner: P,
    /// The run's probe.
    pub probe: Arc<Probe>,
}

impl<P: LlmProvider> LlmProvider for ProbeProvider<P> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn complete(
        &self,
        spec: &PromptSpec,
    ) -> Pin<Box<dyn Future<Output = Result<Completion, ProviderError>> + Send>> {
        if self.probe.take_fault() {
            return Box::pin(ready(Ok(Completion {
                text: MALFORMED_TEXT.to_owned(),
                ..Completion::default()
            })));
        }
        let call = self.inner.complete(spec);
        let probe = Arc::clone(&self.probe);
        Box::pin(async move {
            let reply = call.await?;
            probe.saw_tokens(
                u64::from(reply.usage.prompt_tokens) + u64::from(reply.usage.completion_tokens),
            );
            Ok(reply)
        })
    }
}

/// Puts the probe around the new arm's caller.
pub struct ProbeWrap {
    /// The run's probe.
    pub probe: Arc<Probe>,
    /// The wire the run speaks.
    pub wire: Wire,
    /// Whether the model calls tools natively; otherwise it answers in JSON
    /// steps.
    pub native_tools: bool,
}

impl WrapCaller for ProbeWrap {
    type Caller<C: ModelCaller> = ProbeCaller<C>;

    fn wrap<C: ModelCaller>(self, inner: C) -> ProbeCaller<C> {
        ProbeCaller { inner, with: self }
    }
}

/// The new arm's caller with the probe around it.
pub struct ProbeCaller<C> {
    inner: C,
    with: ProbeWrap,
}

impl<C> ProbeCaller<C> {
    /// A malformed step in the session's form, with no usage: under native
    /// tools a call to a tool that does not exist, under JSON steps text
    /// that is not the step object.
    fn malformed(&self) -> Result<Received, CallFailure> {
        let call_id = "injected_1";
        let body = match self.with.wire {
            Wire::Anthropic => {
                let content = if self.with.native_tools {
                    json!([{"type": "tool_use", "id": call_id, "name": NO_SUCH_TOOL, "input": {}}])
                } else {
                    json!([{"type": "text", "text": MALFORMED_TEXT}])
                };
                json!({
                    "id": "injected", "type": "message", "role": "assistant", "model": "injected",
                    "content": content,
                    "stop_reason": if self.with.native_tools { "tool_use" } else { "end_turn" },
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                })
            }
            Wire::OpenAi => {
                let (message, finish) = if self.with.native_tools {
                    (
                        json!({"role": "assistant", "content": null, "tool_calls": [{
                            "id": call_id, "type": "function",
                            "function": {"name": NO_SUCH_TOOL, "arguments": "{}"}}]}),
                        "tool_calls",
                    )
                } else {
                    (
                        json!({"role": "assistant", "content": MALFORMED_TEXT}),
                        "stop",
                    )
                };
                json!({"id": "injected", "object": "chat.completion", "model": "injected",
                       "choices": [{"index": 0, "message": message, "finish_reason": finish}]})
            }
        };
        let body = RawJson::render(&body)
            .map_err(|e| CallFailure::new(sarathi_protocol::ErrorKind::Malformed, e.to_string()))?;
        match self.with.wire {
            Wire::Anthropic => sarathi_core::anthropic::answer(&body),
            Wire::OpenAi => sarathi_core::openai::answer(&body),
        }
    }
}

impl<C: ModelCaller> ModelCaller for ProbeCaller<C> {
    type Prepared = C::Prepared;

    fn protocol(&self) -> &str {
        self.inner.protocol()
    }

    fn params(&self) -> RawJson {
        self.inner.params()
    }

    fn render_tools(&self, tools: &[ToolDef]) -> Result<Option<RawJson>, CallFailure> {
        self.inner.render_tools(tools)
    }

    fn result_block(&self, call: &CallId, content: &RawJson, is_error: bool) -> RawJson {
        self.inner.result_block(call, content, is_error)
    }

    fn prepare(&self, parts: &RequestParts<'_>) -> Result<(Vec<u8>, Self::Prepared), CallFailure> {
        self.inner.prepare(parts)
    }

    fn send(&self, request: Self::Prepared) -> BoxFuture<'_, Result<Received, CallFailure>> {
        if self.with.probe.take_fault() {
            return Box::pin(ready(self.malformed()));
        }
        let call = self.inner.send(request);
        Box::pin(async move {
            let reply = call.await?;
            let u = &reply.view.usage;
            self.with.probe.saw_tokens(
                u.input_tokens
                    .saturating_add(u.output_tokens)
                    .saturating_add(u.cache_creation_input_tokens)
                    .saturating_add(u.cache_read_input_tokens),
            );
            Ok(reply)
        })
    }

    fn read(&self, body: &RawJson, content: Range<u32>) -> Result<Received, CallFailure> {
        self.inner.read(body, content)
    }

    fn count_tokens(
        &self,
        parts: &RequestParts<'_>,
    ) -> Option<BoxFuture<'_, Result<u64, CallFailure>>> {
        self.inner.count_tokens(parts)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn the_injected_step_is_a_readable_unbilled_answer_on_every_wire() {
        for wire in [Wire::Anthropic, Wire::OpenAi] {
            for native_tools in [true, false] {
                let caller = ProbeCaller {
                    inner: (),
                    with: ProbeWrap {
                        probe: Probe::new(true),
                        wire,
                        native_tools,
                    },
                };
                let answer = caller.malformed().unwrap();
                assert_eq!(answer.view.usage.input_tokens, 0);
                assert_eq!(answer.view.usage.output_tokens, 0);
                let tools: Vec<&str> = answer.view.tool_uses().map(|(_, name, _)| name).collect();
                if native_tools {
                    assert_eq!(tools, [NO_SUCH_TOOL], "{wire:?}");
                } else {
                    assert!(tools.is_empty(), "{wire:?}");
                }
            }
        }
    }

    #[test]
    fn the_fault_is_injected_once() {
        let probe = Probe::new(true);
        assert!(probe.take_fault());
        assert!(!probe.take_fault());
        assert!(!Probe::new(false).take_fault());
    }
}
