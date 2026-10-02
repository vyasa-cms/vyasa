//! The agent harness: plan, act through tools, look, iterate.
//!
//! The single-shot assistants answer once and hope. This loop lets a
//! model work the way a person does in the studio — make a change, look
//! at the page, fix what the look revealed — while keeping every
//! invariant that made the single shot safe:
//!
//! * **Tools are the validated operations.** A toolbox wraps a surface's
//!   own ops (studio ops, page sections); there is no free-form write.
//! * **Sandbox in, proposal out.** A toolbox owns an in-memory copy of
//!   the state it edits; the caller turns the end state into the same
//!   proposal a human accepts. The loop itself persists nothing.
//! * **Every step is accounted.** Each turn is a `run_structured` call,
//!   so failover, budget caps and `ai_logs` rows apply per step.
//!
//! The protocol is a schema, not native tool-calling: each step the
//! model returns `{thought, tool, input, reply}` and `tool: "done"`
//! finishes. That dialect runs on every model the registry can hold —
//! including the free fallbacks — because structured output is the one
//! thing all of them already speak here.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use vyasa_common::AppError;

use crate::provider::{Attachment, LlmProvider, PromptSpec};
use crate::usage::AiUsageSink;

/// Hard ceiling on steps, whatever the caller asks for.
pub const MAX_STEPS_CEILING: usize = 24;

/// An observation longer than this is truncated before it enters the
/// transcript; the model is told it was.
const MAX_OBSERVATION_CHARS: usize = 6_000;

/// Beyond this many characters of transcript, the oldest observations are
/// collapsed to a stub so the window never grows without bound.
const MAX_TRANSCRIPT_CHARS: usize = 40_000;

/// Consecutive unusable replies (bad JSON, unknown tool with no recovery)
/// before the run gives up rather than burning budget.
const MAX_CONSECUTIVE_FAILURES: usize = 3;

/// One tool as the model sees it.
#[derive(Debug, Clone)]
pub struct ToolDef {
    /// Short verb the model calls (`edit`, `look`, `inspect`).
    pub name: &'static str,
    /// One paragraph: what it does, when to reach for it.
    pub description: &'static str,
    /// JSON Schema for `input`.
    pub input_schema: serde_json::Value,
}

/// A surface's tools plus the sandbox they act on.
///
/// One object rather than one trait-object per tool, because tools on a
/// surface share mutable state (the sandbox draft, the accumulated
/// warnings) and a single owner needs no locks.
pub trait Toolbox: Send {
    /// The tools this surface offers, rendered into the system prompt.
    fn tools(&self) -> Vec<ToolDef>;

    /// Runs one tool. `Err` is an *observation* (the model reads it and
    /// recovers), not a run failure — reserve panics and I/O errors for
    /// things the model cannot act on.
    fn call<'a>(
        &'a mut self,
        name: &str,
        input: &serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

/// What one step recorded, for the UI and the audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStep {
    /// The model's stated reasoning for this action.
    pub thought: String,
    /// Tool called (`done` for the finishing step).
    pub tool: String,
    /// Input handed to the tool.
    pub input: serde_json::Value,
    /// What came back, post-truncation — exactly what the model saw.
    pub observation: String,
}

/// The finished run.
#[derive(Debug, Clone)]
pub struct AgentOutcome {
    /// The model's closing reply to the person.
    pub reply: String,
    /// Every step taken, in order.
    pub steps: Vec<AgentStep>,
    /// True when the loop stopped at the step cap rather than by choice.
    pub ran_out: bool,
}

/// One run's fixed inputs.
pub struct AgentSpec<'a> {
    /// `ai_logs` purpose for every step of this run.
    pub purpose: &'a str,
    /// Model identifier for the audit rows.
    pub model: &'a str,
    /// The surface's own system prompt (composition rules, vocabulary…).
    /// The harness appends the protocol and the tool list.
    pub system: &'a str,
    /// The task, in the caller's words, with whatever context it carries.
    pub task: &'a str,
    /// Step budget for this run, clamped to [`MAX_STEPS_CEILING`].
    pub max_steps: usize,
    /// Images shown alongside the first step only — reference material,
    /// not something worth paying for on every turn.
    pub attachments: Vec<Attachment>,
}

/// The shape every step's reply must take.
#[derive(Debug, Deserialize, JsonSchema)]
struct StepReply {
    /// Why this action, in one or two sentences.
    thought: String,
    /// A tool name from the list, or "done".
    tool: String,
    /// The tool's input; ignored for "done".
    #[serde(default)]
    input: serde_json::Value,
    /// The closing reply to the person; only read for "done".
    #[serde(default)]
    reply: String,
}

/// The step schema, in the same hand-written style the designers use.
fn step_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "thought": {"type": "string"},
            "tool": {"type": "string"},
            "input": {"type": "object"},
            "reply": {"type": "string"}
        },
        "required": ["thought", "tool"],
        "additionalProperties": false
    })
}

/// The protocol appended to every agent's system prompt.
fn protocol(tools: &[ToolDef]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "\n\nYou work in steps. Each reply is ONE action as JSON: \
         {\"thought\", \"tool\", \"input\", \"reply\"}.\n\
         - thought: why this action, briefly.\n\
         - tool: one tool name from the list below, or \"done\".\n\
         - input: that tool's input object (omit or {} for \"done\").\n\
         - reply: only with \"done\" — your closing message to the person, \
         plain prose, naming what you changed and anything you chose not \
         to do.\n\
         Look at the page after meaningful edits; fix what the look shows \
         before finishing. Finish as soon as the task is honestly done — \
         steps cost real money.\n\nTOOLS\n",
    );
    for t in tools {
        let _ = write!(
            out,
            "- {} — {}\n  input schema: {}\n",
            t.name, t.description, t.input_schema
        );
    }
    out
}

fn truncate(observation: &str) -> String {
    if observation.chars().count() <= MAX_OBSERVATION_CHARS {
        return observation.to_owned();
    }
    let kept: String = observation.chars().take(MAX_OBSERVATION_CHARS).collect();
    format!("{kept}\n[truncated — the observation continued]")
}

/// Renders the transcript for the next step's user turn, collapsing the
/// oldest observations when the whole thing outgrows the window.
fn transcript(task: &str, steps: &[AgentStep]) -> String {
    let mut rendered: Vec<String> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "[step {}] thought: {}\ncalled: {}({})\nobservation:\n{}\n",
                i + 1,
                s.thought,
                s.tool,
                s.input,
                s.observation
            )
        })
        .collect();
    let mut total: usize = task.len() + rendered.iter().map(String::len).sum::<usize>();
    let mut collapse = 0;
    while total > MAX_TRANSCRIPT_CHARS && collapse < rendered.len().saturating_sub(2) {
        let stub = format!(
            "[step {}] {} — observation elided to keep the window small\n",
            collapse + 1,
            steps[collapse].tool
        );
        total -= rendered[collapse].len();
        total += stub.len();
        rendered[collapse] = stub;
        collapse += 1;
    }
    let mut out = format!("TASK\n{task}\n\n");
    for r in &rendered {
        out.push_str(r);
        out.push('\n');
    }
    out.push_str("Reply with your next action as JSON.");
    out
}

/// Runs one agent to completion.
///
/// # Errors
/// Provider/config errors ([`AppError`]) — a model that cannot be
/// reached, a budget already spent. A model that *misbehaves* (bad JSON,
/// unknown tools) is handled inside the loop and only becomes an error
/// after [`MAX_CONSECUTIVE_FAILURES`] in a row.
pub async fn run_agent<P: LlmProvider>(
    provider: &P,
    sink: &dyn AiUsageSink,
    spec: &AgentSpec<'_>,
    toolbox: &mut dyn Toolbox,
) -> Result<AgentOutcome, AppError> {
    let tools = toolbox.tools();
    let system = format!("{}{}", spec.system, protocol(&tools));
    let max_steps = spec.max_steps.clamp(1, MAX_STEPS_CEILING);

    let mut steps: Vec<AgentStep> = Vec::new();
    let mut failures = 0usize;

    for step_no in 0..max_steps {
        let prompt = PromptSpec {
            purpose: spec.purpose.to_owned(),
            model: spec.model.to_owned(),
            system: system.clone(),
            user: transcript(spec.task, &steps),
            output_schema: step_schema(),
            attachments: if step_no == 0 {
                spec.attachments.clone()
            } else {
                Vec::new()
            },
        };

        let reply: StepReply = match crate::runner::run_structured(provider, sink, &prompt).await {
            Ok(r) => {
                failures = 0;
                r
            }
            Err(e) => {
                // The runner already retried transport-level trouble; what
                // reaches here after its attempts is a model that will not
                // speak the schema. Give it the error to read, a bounded
                // number of times.
                failures += 1;
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    return Err(e);
                }
                steps.push(AgentStep {
                    thought: String::new(),
                    tool: "invalid".to_owned(),
                    input: serde_json::Value::Null,
                    observation: format!(
                        "That reply was unusable ({e}). Answer with exactly one JSON object \
                         matching the schema."
                    ),
                });
                continue;
            }
        };

        if reply.tool == "done" {
            return Ok(AgentOutcome {
                reply: if reply.reply.trim().is_empty() {
                    "Done.".to_owned()
                } else {
                    reply.reply.trim().to_owned()
                },
                steps,
                ran_out: false,
            });
        }

        let known = tools.iter().any(|t| t.name == reply.tool);
        let observation = if known {
            match toolbox.call(&reply.tool, &reply.input).await {
                Ok(o) => {
                    failures = 0;
                    truncate(&o)
                }
                Err(e) => {
                    failures = 0; // a readable tool error is progress, not failure
                    truncate(&format!("The tool refused: {e}"))
                }
            }
        } else {
            failures += 1;
            if failures >= MAX_CONSECUTIVE_FAILURES {
                return Err(AppError::external(
                    "ai-agent",
                    format!(
                        "the model kept calling unknown tools (last: {:?})",
                        reply.tool
                    ),
                ));
            }
            format!(
                "\"{}\" is not a tool. The tools are: {} — or \"done\".",
                reply.tool,
                tools.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
            )
        };

        steps.push(AgentStep {
            thought: reply.thought,
            tool: reply.tool,
            input: reply.input,
            observation,
        });
    }

    // Out of steps: the work done so far still counts — the sandbox holds
    // it — but the person deserves to know the agent stopped on the cap.
    Ok(AgentOutcome {
        reply: format!(
            "I ran out of steps before finishing cleanly ({max_steps} used). \
             What is applied so far is what the steps show; ask again to continue."
        ),
        steps,
        ran_out: true,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::sync::Mutex;

    /// Replays queued replies; the harness must treat it like any model.
    struct Scripted(Mutex<Vec<String>>);

    impl Scripted {
        fn new(replies: &[&str]) -> Self {
            Self(Mutex::new(
                replies.iter().rev().map(|s| (*s).to_owned()).collect(),
            ))
        }
    }

    impl LlmProvider for Scripted {
        fn name(&self) -> &'static str {
            "scripted"
        }
        fn complete(
            &self,
            _spec: &PromptSpec,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            crate::provider::Completion,
                            crate::provider::ProviderError,
                        >,
                    > + Send,
            >,
        > {
            let next = self.0.lock().expect("lock").pop();
            Box::pin(async move {
                match next {
                    Some(text) => Ok(crate::provider::Completion {
                        text,
                        usage: crate::provider::Usage {
                            prompt_tokens: 1,
                            completion_tokens: 1,
                        },
                        provider: None,
                        model: None,
                    }),
                    None => Err(crate::provider::ProviderError::Transport(
                        "script exhausted".into(),
                    )),
                }
            })
        }
    }

    struct NullSink;
    impl AiUsageSink for NullSink {
        fn log_success(&self, _: &str, _: &str, _: &str, _: u32, _: u32) {}
        fn log_failure(&self, _: &str, _: &str, _: &str, _: &str) {}
    }

    /// Counts calls; `echo` repeats its input back.
    struct EchoBox(Vec<(String, serde_json::Value)>);
    impl Toolbox for EchoBox {
        fn tools(&self) -> Vec<ToolDef> {
            vec![ToolDef {
                name: "echo",
                description: "repeats the input back",
                input_schema: serde_json::json!({"type": "object"}),
            }]
        }
        fn call<'a>(
            &'a mut self,
            name: &str,
            input: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
        {
            self.0.push((name.to_owned(), input.clone()));
            let out = format!("echoed: {input}");
            Box::pin(async move { Ok(out) })
        }
    }

    fn spec() -> AgentSpec<'static> {
        AgentSpec {
            purpose: "test-agent",
            model: "scripted",
            system: "You are a test.",
            task: "Say hello via echo, then finish.",
            max_steps: 5,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn acts_then_finishes_and_records_the_steps() {
        let provider = Scripted::new(&[
            r#"{"thought":"try the tool","tool":"echo","input":{"msg":"hi"},"reply":""}"#,
            r#"{"thought":"that worked","tool":"done","input":{},"reply":"All set."}"#,
        ]);
        let mut toolbox = EchoBox(Vec::new());
        let out = run_agent(&provider, &NullSink, &spec(), &mut toolbox)
            .await
            .expect("runs");
        assert_eq!(out.reply, "All set.");
        assert!(!out.ran_out);
        assert_eq!(out.steps.len(), 1);
        assert_eq!(out.steps[0].tool, "echo");
        assert!(out.steps[0].observation.contains("echoed"));
        assert_eq!(toolbox.0.len(), 1, "the tool really ran");
    }

    #[tokio::test]
    async fn an_unknown_tool_becomes_a_correction_the_model_reads() {
        let provider = Scripted::new(&[
            r#"{"thought":"guess","tool":"paint","input":{},"reply":""}"#,
            r#"{"thought":"ok","tool":"done","input":{},"reply":"Fine."}"#,
        ]);
        let mut toolbox = EchoBox(Vec::new());
        let out = run_agent(&provider, &NullSink, &spec(), &mut toolbox)
            .await
            .expect("recovers");
        assert_eq!(out.steps.len(), 1);
        assert!(out.steps[0].observation.contains("not a tool"));
        assert!(toolbox.0.is_empty(), "nothing ran");
        assert_eq!(out.reply, "Fine.");
    }

    #[tokio::test]
    async fn the_step_cap_ends_the_run_honestly() {
        let step = r#"{"thought":"again","tool":"echo","input":{},"reply":""}"#;
        let provider = Scripted::new(&[step, step, step, step, step]);
        let mut toolbox = EchoBox(Vec::new());
        let mut s = spec();
        s.max_steps = 3;
        let out = run_agent(&provider, &NullSink, &s, &mut toolbox)
            .await
            .expect("caps");
        assert!(out.ran_out);
        assert_eq!(out.steps.len(), 3);
        assert!(out.reply.contains("ran out of steps"));
    }

    #[tokio::test]
    async fn a_readable_tool_refusal_is_an_observation_not_a_crash() {
        struct Refuses;
        impl Toolbox for Refuses {
            fn tools(&self) -> Vec<ToolDef> {
                vec![ToolDef {
                    name: "edit",
                    description: "always refuses",
                    input_schema: serde_json::json!({"type": "object"}),
                }]
            }
            fn call<'a>(
                &'a mut self,
                _name: &str,
                _input: &serde_json::Value,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>,
            > {
                Box::pin(async { Err("that id already exists".to_owned()) })
            }
        }
        let provider = Scripted::new(&[
            r#"{"thought":"try","tool":"edit","input":{},"reply":""}"#,
            r#"{"thought":"adjust","tool":"done","input":{},"reply":"Adjusted."}"#,
        ]);
        let mut toolbox = Refuses;
        let out = run_agent(&provider, &NullSink, &spec(), &mut toolbox)
            .await
            .expect("continues");
        assert!(out.steps[0].observation.contains("refused"));
        assert!(out.steps[0].observation.contains("already exists"));
    }

    #[test]
    fn long_observations_are_cut_and_say_so() {
        let long = "x".repeat(MAX_OBSERVATION_CHARS + 100);
        let cut = truncate(&long);
        assert!(cut.chars().count() < long.chars().count());
        assert!(cut.ends_with("[truncated — the observation continued]"));
    }

    #[test]
    fn old_observations_collapse_when_the_transcript_outgrows_the_window() {
        let steps: Vec<AgentStep> = (0..12)
            .map(|i| AgentStep {
                thought: format!("t{i}"),
                tool: "look".into(),
                input: serde_json::Value::Null,
                observation: "y".repeat(5_000),
            })
            .collect();
        let t = transcript("task", &steps);
        assert!(t.len() < MAX_TRANSCRIPT_CHARS + 10_000);
        assert!(t.contains("elided"));
        // The newest steps keep their full observations.
        assert!(t.contains(&format!("[step {}] thought: t11", 12)));
    }
}
