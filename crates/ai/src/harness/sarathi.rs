//! The Sarathi arm of [`super::run`]: one agent run on Sarathi's driver,
//! read back as the same [`AgentOutcome`] Vyasa's loop gives.
//!
//! Vyasa keeps owning the model registry, the keys and the month cap: the
//! registry's resolved model becomes a [`SarathiModel`], and its provider
//! kind picks Sarathi's caller (Anthropic's Messages API, or the OpenAI
//! chat format for every other provider). Sarathi calls the model with
//! native tool use, so a run measures Sarathi as it is meant to run, not
//! on Vyasa's JSON step dialect.
//!
//! What the rest of Vyasa sees does not change with the switch:
//! - every model call writes one `ai_logs` row through the [`AiUsageSink`]
//!   (a failed call a failure row), so budget caps and the usage page hold;
//! - the steps (thought, tool, input, observation) are read back from the
//!   session log, observations as the model saw them;
//! - observations are cut at the length Vyasa's loop cuts them, so the
//!   switch does not quietly change what a run costs.
//!
//! On a surface with a `look` tool (Theme Studio) the run is behind
//! Sarathi's completion gate: the model states its criteria with a `plan`
//! tool, and a finish without a render taken after the last edit is sent
//! back to the model (the run ends as ran out after Sarathi's limit).
//!
//! The session lives in memory for the one request, as Vyasa's loop does:
//! nothing persists until a person accepts the proposal.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use sarathi_core::driver::{DenyAsks, Driver, DriverConfig, DriverError, ModelCaller};
use sarathi_core::gate::EvidenceKinds;
use sarathi_core::toolbox::{SharedGate, ToolboxTools, PLAN_TOOL};
use sarathi_core::{Limits, ModelProfile};
use sarathi_log::{Body, Log, MemoryStore};
use sarathi_protocol::{BlockKind, Event, Op, Outcome, RawJson, ResponseView};
use sarathi_provider::{Chain, Client, Slot, Target};
use sarathi_tools::{Hooks, Mode, Policy};
use serde_json::Value;
use vyasa_common::AppError;
use vyasa_core::ai_models::{ProviderId, ResolvedModel};

use super::SarathiToolbox;
use crate::agent::{AgentOutcome, AgentSpec, AgentStep, Toolbox, MAX_STEPS_CEILING};
use crate::provider::Attachment;
use crate::usage::AiUsageSink;

/// The answer length a step gets when the registry row sets none: what
/// Vyasa's own providers send.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// The provider name an error carries when the run fails.
const ERROR_SOURCE: &str = "sarathi";

/// The wire format Sarathi speaks to a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// Anthropic's Messages API.
    Anthropic,
    /// The OpenAI chat format (OpenAI and every compatible API).
    OpenAi,
}

/// The model a Sarathi run calls: the registry's primary model, resolved.
///
/// Sarathi's caller serves one model per session (a failover to another
/// model would be a protocol switch), so a run uses the chain's first
/// model only; the month cap was checked when the chain was built.
#[derive(Clone)]
pub struct SarathiModel {
    /// The provider's name for `ai_logs` (`anthropic`, `openrouter`, …).
    pub provider: &'static str,
    /// How Sarathi talks to it.
    pub wire: Wire,
    /// The provider's model id.
    pub model: String,
    /// The API's base URL as the registry stores it.
    pub base_url: String,
    /// The API key.
    pub api_key: String,
    /// The longest answer a step may give.
    pub max_tokens: u32,
    /// Whether the model is offered native tool calls. Without them it
    /// answers one JSON step at a time, with the tools described in the
    /// system prompt. Registry models use native tools.
    pub native_tools: bool,
}

/// Never prints the key.
impl std::fmt::Debug for SarathiModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SarathiModel")
            .field("provider", &self.provider)
            .field("wire", &self.wire)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("native_tools", &self.native_tools)
            .finish_non_exhaustive()
    }
}

impl SarathiModel {
    /// The run's model from a resolved registry row.
    #[must_use]
    pub fn from_resolved(m: &ResolvedModel) -> Self {
        let wire = match m.provider {
            ProviderId::Anthropic => Wire::Anthropic,
            ProviderId::OpenAi
            | ProviderId::OpenRouter
            | ProviderId::Gemini
            | ProviderId::Mistral
            | ProviderId::Custom => Wire::OpenAi,
        };
        let max_tokens = m.row.settings["max_tokens"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(DEFAULT_MAX_TOKENS);
        Self {
            provider: m.provider.as_str(),
            wire,
            model: m.row.model.clone(),
            base_url: m.base_url.clone(),
            api_key: m.api_key.clone(),
            max_tokens,
            native_tools: true,
        }
    }

    fn profile(&self) -> ModelProfile {
        if self.native_tools {
            ModelProfile::native()
        } else {
            // No response format: Vyasa's loop asks these models for JSON
            // in the prompt too.
            ModelProfile::json_object(false)
        }
    }

    /// Sarathi's HTTP target. The registry stores Anthropic's base with its
    /// `/v1` (Vyasa's client appends `/messages`); Sarathi appends the whole
    /// `/v1/messages`, so the version segment is taken off. OpenAI-format
    /// bases keep theirs (Sarathi appends `/chat/completions`).
    fn target(&self) -> Target {
        match self.wire {
            Wire::Anthropic => {
                let base = self.base_url.trim_end_matches('/');
                let root = base.strip_suffix("/v1").unwrap_or(base);
                Target::anthropic(self.api_key.clone()).with_base_url(root)
            }
            Wire::OpenAi => Target::openai_compatible(self.base_url.clone(), self.api_key.clone()),
        }
    }

    fn chain(&self) -> Result<Chain, AppError> {
        let client = Client::new(self.target()).map_err(|e| external(&e.to_string()))?;
        Chain::new(vec![Slot::new(self.provider, client, self.model.clone())])
            .map_err(|e| external(&e.to_string()))
    }
}

/// Runs one agent on Sarathi against `model`.
///
/// # Errors
/// [`AppError::External`] when the model could not be reached or the run
/// failed; the detail is the last error Sarathi reported.
pub async fn run(
    model: &SarathiModel,
    sink: &dyn AiUsageSink,
    spec: &AgentSpec<'_>,
    toolbox: &mut dyn Toolbox,
) -> Result<AgentOutcome, AppError> {
    run_wrapped(model, Unwrapped, sink, spec, toolbox).await
}

/// Puts a layer around the caller a run builds, whichever wire the model
/// speaks: an evaluation observes or injects at the model seam this way.
pub trait WrapCaller {
    /// The wrapped caller.
    type Caller<C: ModelCaller>: ModelCaller;

    /// Wraps `caller`.
    fn wrap<C: ModelCaller>(self, caller: C) -> Self::Caller<C>;
}

/// The caller as built.
struct Unwrapped;

impl WrapCaller for Unwrapped {
    type Caller<C: ModelCaller> = C;

    fn wrap<C: ModelCaller>(self, caller: C) -> C {
        caller
    }
}

/// [`run`] with `wrap` around the model caller.
///
/// # Errors
/// As [`run`].
pub async fn run_wrapped<W: WrapCaller>(
    model: &SarathiModel,
    wrap: W,
    sink: &dyn AiUsageSink,
    spec: &AgentSpec<'_>,
    toolbox: &mut dyn Toolbox,
) -> Result<AgentOutcome, AppError> {
    let chain = model.chain()?;
    let profile = model.profile();
    match model.wire {
        Wire::Anthropic => {
            let caller = sarathi_core::anthropic::AnthropicCaller::new(chain, model.max_tokens)
                .map_err(|e| external(&e.detail))?;
            let caller = wrap.wrap(caller);
            run_with(caller, profile, model.provider, sink, spec, toolbox, true).await
        }
        Wire::OpenAi => {
            let caller = sarathi_core::openai::OpenAiCaller::new(chain, model.max_tokens)
                .map_err(|e| external(&e.detail))?;
            let caller = wrap.wrap(caller);
            run_with(caller, profile, model.provider, sink, spec, toolbox, false).await
        }
    }
}

/// Runs one agent on Sarathi through `caller`, reading its steps as
/// `profile` says. `images` says whether the
/// caller's protocol carries image blocks; when it does not, attachments
/// are left out with a warning (the model is told nothing it cannot see).
///
/// # Errors
/// As [`run`].
pub async fn run_with<C: ModelCaller>(
    caller: C,
    profile: ModelProfile,
    provider: &'static str,
    sink: &dyn AiUsageSink,
    spec: &AgentSpec<'_>,
    toolbox: &mut dyn Toolbox,
    images: bool,
) -> Result<AgentOutcome, AppError> {
    let tools = ToolboxTools::new(
        SarathiToolbox::new(toolbox),
        // Every tool runs, as on Vyasa's loop: the run works a sandbox and
        // nothing persists until a person accepts the proposal.
        Policy::new(Mode::Bypass),
        Hooks::new(),
    );
    let (tools, gate) = gated(tools, spec.purpose)?;
    let defs = tools.defs().map_err(|e| external(&e.to_string()))?;
    let steps = u32::try_from(spec.max_steps.clamp(1, MAX_STEPS_CEILING)).unwrap_or(1);
    let mut config =
        DriverConfig::new(profile, defs, spec.system).with_limits(Limits::with_max_steps(steps));
    if let Some(gate) = gate {
        config = config.with_completion(Arc::new(gate));
    }
    let (mut driver, _handle) = Driver::new(config, MemoryStore::new(), caller, tools, DenyAsks)
        .await
        .map_err(|e| driver_error(&e))?;

    let attachments: &[Attachment] = if images {
        &spec.attachments
    } else {
        if !spec.attachments.is_empty() {
            tracing::warn!(
                purpose = spec.purpose,
                count = spec.attachments.len(),
                "this model's wire format carries no images on Sarathi; attachments left out"
            );
        }
        &[]
    };
    let op = Op::UserMessage {
        content: user_content(spec.task, attachments)?,
    };

    let mut last_error: Option<String> = None;
    let mut model_name = String::from(spec.model);
    // The answers' views by the seq of the assistant message they became;
    // their ranges point into the response bodies the log keeps.
    let mut views: HashMap<u64, ResponseView> = HashMap::new();
    let end = driver
        .run_turn(op, &mut |event| match event {
            Event::ModelCall { model, .. } => model_name = model.into(),
            Event::AssistantMessage { seq, view } => {
                views.insert(seq.0, view);
            }
            Event::Usage { usage, .. } => {
                let prompt = usage
                    .input_tokens
                    .saturating_add(usage.cache_creation_input_tokens)
                    .saturating_add(usage.cache_read_input_tokens);
                sink.log_success(
                    provider,
                    &model_name,
                    spec.purpose,
                    clamp_u32(prompt),
                    clamp_u32(usage.output_tokens),
                );
                sink.log_cache(
                    provider,
                    &model_name,
                    spec.purpose,
                    clamp_u32(usage.cache_creation_input_tokens),
                    clamp_u32(usage.cache_read_input_tokens),
                );
            }
            Event::Error { kind, detail } => {
                sink.log_failure(provider, &model_name, spec.purpose, &detail);
                last_error = Some(format!("{}: {detail}", kind.as_str()));
            }
            _ => {}
        })
        .await
        .map_err(|e| driver_error(&e))?;

    // A refusal is an answer (its reply says so); an interrupt cannot
    // happen (no one holds the handle), so anything else is a failure.
    if !matches!(
        end.outcome,
        Outcome::Finished | Outcome::RanOut | Outcome::Refused
    ) {
        return Err(external(
            last_error
                .as_deref()
                .unwrap_or("the Sarathi run ended without an answer"),
        ));
    }
    let steps = read_steps(driver.log(), &views);
    Ok(AgentOutcome {
        reply: end.reply.map(String::from).unwrap_or_default(),
        steps,
        ran_out: end.outcome == Outcome::RanOut,
    })
}

/// The surface's tools behind Sarathi's completion gate, when one of them
/// supplies evidence (the studio's `look`): the model gets a `plan` tool to
/// state what must hold, and a finish is accepted only with evidence for
/// every criterion gathered after the last edit. A finish refused more
/// than Sarathi's limit ends the run as ran out, as a run out of steps does.
///
/// A surface with no evidence tool runs without a gate (nothing could
/// satisfy it); one that has its own `plan` tool does too, with a warning,
/// since the gate's tool would shadow it.
fn gated<T: sarathi_tools::Toolbox>(
    tools: ToolboxTools<T>,
    purpose: &str,
) -> Result<(ToolboxTools<T>, Option<SharedGate>), AppError> {
    let kinds: Vec<String> = tools
        .specs()
        .iter()
        .filter_map(|s| s.evidence.clone())
        .collect();
    if kinds.is_empty() {
        return Ok((tools, None));
    }
    if tools.specs().iter().any(|s| s.name == PLAN_TOOL) {
        tracing::warn!(
            purpose,
            "a surface tool is named `{PLAN_TOOL}`; the Sarathi run has no completion gate"
        );
        return Ok((tools, None));
    }
    // The name was checked above, so `with_gate` does not refuse here.
    let (tools, gate) = tools
        .with_gate(EvidenceKinds::new(kinds))
        .map_err(|e| external(&e.to_string()))?;
    Ok((tools, Some(gate)))
}

/// The first user message: the task, then the attachments as image blocks
/// (Anthropic's form, which Sarathi's Anthropic caller sends unchanged).
fn user_content(task: &str, attachments: &[Attachment]) -> Result<RawJson, AppError> {
    if attachments.is_empty() {
        return Ok(RawJson::string(task));
    }
    let mut blocks: Vec<Value> = attachments
        .iter()
        .map(|a| {
            serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": a.mime, "data": a.data_b64},
            })
        })
        .collect();
    blocks.push(serde_json::json!({"type": "text", "text": task}));
    RawJson::render(&blocks).map_err(|e| AppError::internal_msg(e.to_string()))
}

/// The run's steps, read back from the session log: one per tool call,
/// with the text the model wrote alongside it as the thought and the
/// result it was sent back as the observation.
fn read_steps(log: &Log, views: &HashMap<u64, ResponseView>) -> Vec<AgentStep> {
    let mut responses: HashMap<u64, &RawJson> = HashMap::new();
    let mut results: HashMap<&str, String> = HashMap::new();
    for entry in log.entries() {
        match &entry.body {
            Body::ModelResponse { body, .. } => {
                responses.insert(entry.seq.0, body);
            }
            Body::ToolResult { call, block, .. } => {
                results.insert(call, result_text(block));
            }
            _ => {}
        }
    }
    let mut steps = Vec::new();
    for entry in log.entries() {
        let Body::AssistantMessage { from, calls } = &entry.body else {
            continue;
        };
        let (Some(body), Some(view)) = (responses.get(&from.0), views.get(&entry.seq.0)) else {
            continue;
        };
        let raw = body.as_str();
        let cut = |r: &Range<u32>| raw.get(r.start as usize..r.end as usize).unwrap_or("");
        let thought: Vec<String> = view
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Text)
            .filter_map(|b| text_of(cut(&b.range)))
            .collect();
        let thought = thought.join("\n");
        if view.tool_uses().next().is_none() {
            // A JSON step: the answer's text is the step, and the log names
            // the call it asked for.
            let step = json_step(&thought);
            for call in calls {
                steps.push(AgentStep {
                    thought: step["thought"].as_str().unwrap_or_default().to_owned(),
                    tool: call.name.to_string(),
                    input: step
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    observation: results.get(&*call.id).cloned().unwrap_or_default(),
                });
            }
            continue;
        }
        for (id, name, input) in view.tool_uses() {
            steps.push(AgentStep {
                thought: thought.clone(),
                tool: name.to_owned(),
                input: input_of(cut(&input)),
                observation: results.get(id).cloned().unwrap_or_default(),
            });
        }
    }
    steps
}

/// The JSON step in an answer's text, a Markdown code fence around it
/// removed; null when it is not one.
fn json_step(text: &str) -> Value {
    let text = text.trim();
    let body = text
        .strip_prefix("```")
        .map(|rest| rest.strip_prefix("json").unwrap_or(rest))
        .map_or(text, |rest| rest.strip_suffix("```").unwrap_or(rest));
    serde_json::from_str(body.trim()).unwrap_or(Value::Null)
}

/// A text block's text: a JSON string (the OpenAI form) or an object with
/// a `text` member (Anthropic's).
fn text_of(block: &str) -> Option<String> {
    match serde_json::from_str::<Value>(block).ok()? {
        Value::String(s) => Some(s),
        v => v["text"].as_str().map(str::to_owned),
    }
}

/// A tool call's input: a JSON object (Anthropic), or a string holding one
/// (OpenAI's `arguments`). A string that is not JSON stays a string.
fn input_of(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::String(s)) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        Ok(v) => v,
        Err(_) => Value::String(raw.to_owned()),
    }
}

/// What a result block told the model: its `content`, a string or text
/// blocks (both wire formats name it `content`), or under JSON steps the
/// text block of an observation.
fn result_text(block: &RawJson) -> String {
    let Ok(v) = serde_json::from_str::<Value>(block.as_str()) else {
        return block.as_str().to_owned();
    };
    match &v["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => match v["text"].as_str() {
            Some(text) => text
                .strip_prefix("observation:\n")
                .unwrap_or(text)
                .to_owned(),
            None => block.as_str().to_owned(),
        },
    }
}

fn clamp_u32(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn external(detail: &str) -> AppError {
    AppError::external(ERROR_SOURCE, detail.to_owned())
}

fn driver_error(e: &DriverError) -> AppError {
    external(&e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::agent::ToolDef;
    use sarathi_core::driver::{BoxFuture, CallFailure, Received};
    use sarathi_log::RequestParts;
    use sarathi_protocol::{CallId, ErrorKind};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    /// A model that answers from a script, in either wire format.
    struct Scripted {
        wire: Wire,
        replies: Mutex<VecDeque<Option<String>>>,
        bodies: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn new(wire: Wire, replies: &[String]) -> Self {
            Self {
                wire,
                replies: Mutex::new(replies.iter().cloned().map(Some).collect()),
                bodies: Mutex::new(Vec::new()),
            }
        }

        /// A model whose one call fails as an outage.
        fn unreachable(wire: Wire) -> Self {
            Self {
                wire,
                replies: Mutex::new(VecDeque::from([None])),
                bodies: Mutex::new(Vec::new()),
            }
        }

        fn answer(&self, body: &RawJson) -> Result<Received, CallFailure> {
            match self.wire {
                Wire::Anthropic => sarathi_core::anthropic::answer(body),
                Wire::OpenAi => sarathi_core::openai::answer(body),
            }
        }
    }

    impl ModelCaller for &Scripted {
        type Prepared = ();

        fn protocol(&self) -> &str {
            match self.wire {
                Wire::Anthropic => sarathi_core::anthropic::PROTOCOL,
                Wire::OpenAi => sarathi_core::openai::PROTOCOL,
            }
        }

        fn params(&self) -> RawJson {
            let text = match self.wire {
                Wire::Anthropic => r#"{"model":"scripted","max_tokens":1024}"#,
                Wire::OpenAi => {
                    r#"{"model":"scripted","max_tokens":1024,"token_field":"max_tokens"}"#
                }
            };
            RawJson::from_text(text).unwrap()
        }

        fn render_tools(
            &self,
            tools: &[sarathi_core::ToolDef],
        ) -> Result<Option<RawJson>, CallFailure> {
            match self.wire {
                Wire::Anthropic => sarathi_core::anthropic::render_tools(tools),
                Wire::OpenAi => sarathi_core::openai::render_tools(tools),
            }
        }

        fn result_block(&self, call: &CallId, content: &RawJson, is_error: bool) -> RawJson {
            match self.wire {
                Wire::Anthropic => sarathi_core::anthropic::result_block(call, content, is_error),
                Wire::OpenAi => sarathi_core::openai::result_block(call, content, is_error),
            }
        }

        fn prepare(&self, parts: &RequestParts<'_>) -> Result<(Vec<u8>, ()), CallFailure> {
            let body = match self.wire {
                Wire::Anthropic => sarathi_core::anthropic::request(parts)
                    .unwrap()
                    .body(sarathi_provider::Endpoint::Messages),
                Wire::OpenAi => sarathi_core::openai::request(parts).unwrap().body(),
            };
            self.bodies
                .lock()
                .unwrap()
                .push(String::from_utf8(body.clone()).unwrap());
            Ok((body, ()))
        }

        fn send(&self, (): ()) -> BoxFuture<'_, Result<Received, CallFailure>> {
            let reply = self.replies.lock().unwrap().pop_front();
            let out = match reply {
                Some(Some(b)) => self.answer(&RawJson::from_text(&b).unwrap()),
                Some(None) => Err(CallFailure::new(ErrorKind::Upstream, "scripted outage")),
                None => Err(CallFailure::new(ErrorKind::Upstream, "script exhausted")),
            };
            Box::pin(std::future::ready(out))
        }

        fn read(&self, body: &RawJson, content: Range<u32>) -> Result<Received, CallFailure> {
            let mut r = self.answer(body)?;
            r.content = content;
            Ok(r)
        }
    }

    /// An answer that calls `calls` (`(id, tool, input)`) after `text`.
    fn anthropic(text: Option<&str>, calls: &[(&str, &str, Value)]) -> String {
        let mut blocks = Vec::new();
        if let Some(t) = text {
            blocks.push(json!({"type": "text", "text": t}));
        }
        for (id, name, input) in calls {
            blocks.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
        }
        let stop = if calls.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        };
        json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "scripted",
               "content": blocks, "stop_reason": stop,
               "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 2}})
        .to_string()
    }

    fn openai(text: Option<&str>, calls: &[(&str, &str, Value)]) -> String {
        let tool_calls: Vec<Value> = calls
            .iter()
            .map(|(id, name, input)| {
                json!({"id": id, "type": "function",
                       "function": {"name": name, "arguments": input.to_string()}})
            })
            .collect();
        let mut message = json!({"role": "assistant", "content": text});
        if !tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(tool_calls);
        }
        let finish = if calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        json!({"id": "c1", "model": "scripted",
               "choices": [{"index": 0, "message": message, "finish_reason": finish}],
               "usage": {"prompt_tokens": 10, "completion_tokens": 5}})
        .to_string()
    }

    /// `(provider, model, purpose, prompt tokens, completion tokens)`.
    type UsageRow = (String, String, String, u32, u32);

    #[derive(Default)]
    struct Recorder {
        rows: Mutex<Vec<UsageRow>>,
        failures: Mutex<Vec<String>>,
    }

    impl AiUsageSink for Recorder {
        fn log_success(&self, provider: &str, model: &str, purpose: &str, pt: u32, ct: u32) {
            self.rows.lock().unwrap().push((
                provider.to_owned(),
                model.to_owned(),
                purpose.to_owned(),
                pt,
                ct,
            ));
        }

        fn log_failure(&self, _provider: &str, _model: &str, _purpose: &str, error: &str) {
            self.failures.lock().unwrap().push(error.to_owned());
        }
    }

    /// Edits count, `look` renders, `long` answers past the observation cap,
    /// anything else fails as an observation. `look` is offered only on a
    /// [`Page::studio`], which puts the run behind the completion gate.
    #[derive(Default)]
    struct Page {
        edits: Vec<Value>,
        look: bool,
    }

    impl Page {
        fn studio() -> Self {
            Self {
                look: true,
                ..Self::default()
            }
        }
    }

    /// A `plan` call stating one criterion a `look` shows.
    fn plan(id: &str) -> (&str, &'static str, Value) {
        (
            id,
            PLAN_TOOL,
            json!({"criteria": [{"id": "red", "text": "The title is red.", "kind": LOOK}]}),
        )
    }

    const LOOK: &str = super::super::LOOK_EVIDENCE;

    impl Toolbox for Page {
        fn tools(&self) -> Vec<ToolDef> {
            ["edit", "look", "long"]
                .into_iter()
                .filter(|name| self.look || *name != "look")
                .map(|name| ToolDef {
                    name,
                    description: "d",
                    input_schema: json!({"type": "object"}),
                })
                .collect()
        }

        fn call<'a>(
            &'a mut self,
            name: &str,
            input: &Value,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            let out = match name {
                "edit" => {
                    self.edits.push(input.clone());
                    Ok("edited".to_owned())
                }
                "look" => Ok("rendered".to_owned()),
                "long" => Ok("x".repeat(7_000)),
                other => Err(format!("no tool {other}")),
            };
            Box::pin(std::future::ready(out))
        }
    }

    fn spec(max_steps: usize, attachments: Vec<Attachment>) -> AgentSpec<'static> {
        AgentSpec {
            purpose: "theme-studio",
            model: "scripted",
            system: "You edit pages.",
            task: "Make the title red.",
            max_steps,
            attachments,
        }
    }

    async fn go(
        model: &Scripted,
        sink: &Recorder,
        spec: &AgentSpec<'_>,
        page: &mut Page,
    ) -> Result<AgentOutcome, AppError> {
        let images = model.wire == Wire::Anthropic;
        let profile = ModelProfile::native();
        run_with(model, profile, "anthropic", sink, spec, page, images).await
    }

    #[tokio::test]
    async fn a_run_maps_back_to_steps_reply_and_one_usage_row_per_call() {
        for (wire, answer) in [
            (
                Wire::Anthropic,
                anthropic as fn(Option<&str>, &[(&str, &str, Value)]) -> String,
            ),
            (Wire::OpenAi, openai),
        ] {
            let model = Scripted::new(
                wire,
                &[
                    answer(
                        Some("Edit, then check."),
                        &[plan("c0"), ("c1", "edit", json!({"title": "red"}))],
                    ),
                    answer(None, &[("c2", "look", json!({}))]),
                    answer(None, &[("c3", "frobnicate", json!({}))]),
                    answer(Some("All set."), &[]),
                ],
            );
            let sink = Recorder::default();
            let mut page = Page::studio();
            let out = go(&model, &sink, &spec(12, Vec::new()), &mut page)
                .await
                .expect("runs");
            assert_eq!(out.reply, "All set.", "{wire:?}");
            assert!(!out.ran_out);
            assert_eq!(page.edits, vec![json!({"title": "red"})], "{wire:?}");
            let steps: Vec<_> = out
                .steps
                .iter()
                .map(|s| (s.thought.as_str(), s.tool.as_str(), &s.input))
                .collect();
            assert_eq!(
                steps,
                vec![
                    ("Edit, then check.", PLAN_TOOL, &plan("c0").2),
                    ("Edit, then check.", "edit", &json!({"title": "red"})),
                    ("", "look", &json!({})),
                    ("", "frobnicate", &json!({})),
                ],
                "{wire:?}"
            );
            assert!(out.steps[0].observation.starts_with("Plan taken"));
            assert_eq!(out.steps[1].observation, "edited");
            assert_eq!(out.steps[2].observation, "rendered");
            // A tool the surface does not offer is refused before it runs.
            assert!(
                out.steps[3].observation.contains("frobnicate"),
                "{wire:?}: {}",
                out.steps[3].observation
            );
            let rows = sink.rows.lock().unwrap().clone();
            assert_eq!(rows.len(), 4, "{wire:?}");
            let prompt = if wire == Wire::Anthropic { 12 } else { 10 };
            for row in rows {
                assert_eq!(
                    row,
                    (
                        "anthropic".to_owned(),
                        "scripted".to_owned(),
                        "theme-studio".to_owned(),
                        prompt,
                        5
                    )
                );
            }
            assert!(sink.failures.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn a_studio_finish_needs_a_plan_and_a_look_after_the_last_edit() {
        let model = Scripted::new(
            Wire::Anthropic,
            &[
                anthropic(None, &[("c1", "edit", json!({"title": "red"}))]),
                // Refused: no plan yet.
                anthropic(Some("Done."), &[]),
                anthropic(None, &[plan("c2"), ("c3", "look", json!({}))]),
                anthropic(Some("Checked, done."), &[]),
            ],
        );
        let sink = Recorder::default();
        let mut page = Page::studio();
        let out = go(&model, &sink, &spec(12, Vec::new()), &mut page)
            .await
            .expect("runs");
        assert_eq!(out.reply, "Checked, done.");
        assert!(!out.ran_out);
        let tools: Vec<_> = out.steps.iter().map(|s| s.tool.as_str()).collect();
        assert_eq!(tools, ["edit", PLAN_TOOL, "look"]);
        let bodies = model.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 4);
        // The gate's tool is offered from the first call; the refusal reaches
        // the model as a message before the third.
        assert!(bodies[0].contains(r#""name":"plan""#), "{}", bodies[0]);
        let refusal = "Completion refused: state a plan first";
        assert!(!bodies[1].contains(refusal));
        assert!(bodies[2].contains(refusal), "{}", bodies[2]);
        assert_eq!(sink.rows.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn a_surface_without_look_runs_ungated() {
        let model = Scripted::new(
            Wire::Anthropic,
            &[
                anthropic(None, &[("c1", "edit", json!({}))]),
                anthropic(Some("Done."), &[]),
            ],
        );
        let mut page = Page::default();
        let out = go(
            &model,
            &Recorder::default(),
            &spec(12, Vec::new()),
            &mut page,
        )
        .await
        .expect("runs");
        assert_eq!(out.reply, "Done.");
        assert!(!out.ran_out);
        let first = model.bodies.lock().unwrap()[0].clone();
        assert!(!first.contains(r#""name":"plan""#), "{first}");
    }

    #[tokio::test]
    async fn the_step_budget_ends_the_run_as_ran_out() {
        let replies: Vec<_> = (0..4)
            .map(|i| anthropic(None, &[(&format!("c{i}"), "edit", json!({"n": i}))]))
            .collect();
        let model = Scripted::new(Wire::Anthropic, &replies);
        let sink = Recorder::default();
        let mut page = Page::default();
        let out = go(&model, &sink, &spec(2, Vec::new()), &mut page)
            .await
            .expect("runs");
        assert!(out.ran_out);
        assert_eq!(out.steps.len(), 2);
        assert_eq!(page.edits.len(), 2);
    }

    #[tokio::test]
    async fn a_model_that_cannot_be_reached_is_an_error_and_a_failure_row() {
        let model = Scripted::unreachable(Wire::Anthropic);
        let sink = Recorder::default();
        let mut page = Page::default();
        let err = go(&model, &sink, &spec(12, Vec::new()), &mut page)
            .await
            .expect_err("fails");
        assert!(matches!(err, AppError::External { .. }), "{err:?}");
        assert!(err.to_string().contains("scripted outage"), "{err}");
        assert_eq!(sink.failures.lock().unwrap().len(), 1);
        assert!(sink.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn observations_are_cut_as_on_vyasas_loop() {
        let model = Scripted::new(
            Wire::Anthropic,
            &[
                anthropic(None, &[("c1", "long", json!({}))]),
                anthropic(Some("ok"), &[]),
            ],
        );
        let mut page = Page::default();
        let out = go(
            &model,
            &Recorder::default(),
            &spec(12, Vec::new()),
            &mut page,
        )
        .await
        .expect("runs");
        assert_eq!(
            out.steps[0].observation,
            crate::agent::truncate(&"x".repeat(7_000))
        );
        assert!(out.steps[0].observation.len() < 7_000);
    }

    #[tokio::test]
    async fn attachments_go_as_image_blocks_where_the_wire_carries_them() {
        let image = Attachment {
            mime: "image/png".to_owned(),
            data_b64: "aGk=".to_owned(),
        };
        for (wire, answer) in [
            (
                Wire::Anthropic,
                anthropic as fn(Option<&str>, &[(&str, &str, Value)]) -> String,
            ),
            (Wire::OpenAi, openai),
        ] {
            let model = Scripted::new(wire, &[answer(Some("Seen."), &[])]);
            let mut page = Page::default();
            go(
                &model,
                &Recorder::default(),
                &spec(12, vec![image.clone()]),
                &mut page,
            )
            .await
            .expect("runs");
            let first = model.bodies.lock().unwrap()[0].clone();
            assert!(first.contains("Make the title red."), "{wire:?}: {first}");
            assert_eq!(
                first.contains("aGk="),
                wire == Wire::Anthropic,
                "{wire:?}: {first}"
            );
        }
    }

    #[test]
    fn registry_rows_map_to_sarathi_targets() {
        let model = |wire, base: &str| SarathiModel {
            provider: "p",
            wire,
            model: "m".to_owned(),
            base_url: base.to_owned(),
            api_key: "k".to_owned(),
            max_tokens: 1,
            native_tools: true,
        };
        let base = |t: Target| match t {
            Target::Provider { kind, base_url, .. } => (kind.as_str(), base_url),
            Target::Gateway { .. } => unreachable!("no gateway targets"),
        };
        assert_eq!(
            base(model(Wire::Anthropic, "https://api.anthropic.com/v1").target()),
            ("anthropic", "https://api.anthropic.com".to_owned())
        );
        assert_eq!(
            base(model(Wire::Anthropic, "https://proxy.test/").target()),
            ("anthropic", "https://proxy.test".to_owned())
        );
        assert_eq!(
            base(model(Wire::OpenAi, "https://openrouter.ai/api/v1").target()),
            ("openai", "https://openrouter.ai/api/v1".to_owned())
        );
        let debug = format!("{:?}", model(Wire::OpenAi, "u"));
        assert!(!debug.contains("\"k\""), "{debug}");
    }

    #[test]
    fn result_and_block_texts_read_both_wire_formats() {
        let raw = |s: &str| RawJson::from_text(s).unwrap();
        // A JSON step's observation is a text block of its own.
        assert_eq!(
            result_text(&raw(r#"{"type":"text","text":"observation:\nApplied"}"#)),
            "Applied"
        );
        assert_eq!(
            result_text(&raw(
                r#"{"type":"tool_result","tool_use_id":"a","content":"hi"}"#
            )),
            "hi"
        );
        assert_eq!(
            result_text(&raw(
                r#"{"type":"tool_result","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}"#
            )),
            "a\nb"
        );
        assert_eq!(
            result_text(&raw(r#"{"role":"tool","tool_call_id":"a","content":"hi"}"#)),
            "hi"
        );
        assert_eq!(result_text(&raw(r#"{"x":1}"#)), r#"{"x":1}"#);
        assert_eq!(text_of(r#""plain""#).as_deref(), Some("plain"));
        assert_eq!(
            text_of(r#"{"type":"text","text":"t"}"#).as_deref(),
            Some("t")
        );
        assert_eq!(text_of("nope"), None);
        assert_eq!(input_of(r#""{\"a\":1}""#), json!({"a": 1}));
        assert_eq!(input_of(r#""not json""#), json!("not json"));
        assert_eq!(input_of(r#"{"a":1}"#), json!({"a": 1}));
        assert_eq!(input_of("{bad"), json!("{bad"));
    }
}
