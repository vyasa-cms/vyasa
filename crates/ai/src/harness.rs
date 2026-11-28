//! Which agent harness runs a surface's agent: Vyasa's own loop
//! ([`crate::agent::run_agent`]) or Sarathi, an embeddable harness pulled
//! in as a git dependency pinned to a commit.
//!
//! The operator picks with the `ai_harness` option (`"vyasa"`, the
//! default, or `"sarathi"`), read per request, so both can run side by
//! side on one site without a restart. Callers go through [`run`] and get
//! the same [`AgentOutcome`] from either, so the proposal, the step list
//! the studio shows and the `ai_logs` rows do not change with the switch.
//!
//! A Sarathi run calls the registry's primary model ([`SarathiModel`]);
//! the run itself is in [`sarathi`]. When the caller has no registry model
//! to give (the environment-key fallback), a run asked for on Sarathi runs
//! on Vyasa's loop and says so in a warning.

pub mod sarathi;

use serde_json::Value;
use vyasa_common::AppError;

pub use self::sarathi::{SarathiModel, Wire};
use crate::agent::{run_agent, truncate, AgentOutcome, AgentSpec, Toolbox};
use crate::provider::LlmProvider;
use crate::usage::AiUsageSink;

/// The option that picks the harness.
pub const OPTION_KEY: &str = "ai_harness";

/// The agent harness a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Harness {
    /// Vyasa's own loop: a JSON step schema over structured output.
    #[default]
    Vyasa,
    /// Sarathi.
    Sarathi,
}

impl Harness {
    /// The option value for this harness.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Vyasa => "vyasa",
            Harness::Sarathi => "sarathi",
        }
    }

    /// The harness an option value names. A missing value is
    /// [`Harness::Vyasa`]; an unknown one is too, with a warning, so a
    /// typo in the option never stops the assistant.
    #[must_use]
    pub fn from_option(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str) {
            None => Harness::Vyasa,
            Some(s) => Self::parse(s).unwrap_or_else(|| {
                tracing::warn!(value = s, "unknown {OPTION_KEY}; using the vyasa harness");
                Harness::Vyasa
            }),
        }
    }

    /// The harness a string names, or `None`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "vyasa" => Some(Harness::Vyasa),
            "sarathi" => Some(Harness::Sarathi),
            _ => None,
        }
    }
}

/// Runs one agent on `harness`: the one entry point a surface calls.
///
/// `provider` is Vyasa's failover chain, for Vyasa's loop; `sarathi` is the
/// chain's primary model, for Sarathi (callers resolve it only when the
/// harness is Sarathi). Sarathi without a model runs on Vyasa's loop.
///
/// # Errors
/// What the harness returns: [`AppError`] when the provider chain or the
/// budget refuses a step, or the Sarathi run fails.
pub async fn run<P: LlmProvider>(
    harness: Harness,
    provider: &P,
    sarathi: Option<&SarathiModel>,
    sink: &dyn AiUsageSink,
    spec: &AgentSpec<'_>,
    toolbox: &mut dyn Toolbox,
) -> Result<AgentOutcome, AppError> {
    match (harness, sarathi) {
        (Harness::Sarathi, Some(model)) => sarathi::run(model, sink, spec, toolbox).await,
        (Harness::Sarathi, None) => {
            tracing::warn!(
                purpose = spec.purpose,
                "{OPTION_KEY} is \"sarathi\" but no registry model is set up; using the vyasa harness"
            );
            run_agent(provider, sink, spec, toolbox).await
        }
        (Harness::Vyasa, _) => run_agent(provider, sink, spec, toolbox).await,
    }
}

/// What a tool's output proves to a completion check: `look` renders the
/// page, which is the evidence a visual change was checked.
pub const LOOK_EVIDENCE: &str = "look";

/// Tools that change nothing, by name, across Vyasa's surfaces.
const READ_ONLY_TOOLS: &[&str] = &["inspect", "look", "menus", "content"];

/// Tools that edit the sandbox draft, by name, across Vyasa's surfaces.
const EDIT_TOOLS: &[&str] = &["edit", "edit_menu"];

/// A surface's [`Toolbox`] offered to Sarathi.
///
/// The tools and their calls pass through; an `Err` stays an observation
/// the model reads (a tool error, not a run failure), and either is cut at
/// the length Vyasa's loop cuts observations at. Each
/// tool also gets what Sarathi's permission layer and completion gate
/// need: read-only or edit by name, and `look` tagged as evidence.
pub struct SarathiToolbox<'a> {
    inner: &'a mut dyn Toolbox,
}

impl<'a> SarathiToolbox<'a> {
    /// Wraps a surface's toolbox for one run.
    pub fn new(inner: &'a mut dyn Toolbox) -> Self {
        Self { inner }
    }
}

impl sarathi_tools::Toolbox for SarathiToolbox<'_> {
    fn tools(&self) -> Vec<sarathi_tools::ToolSpec> {
        self.inner
            .tools()
            .into_iter()
            .map(|t| {
                let mut spec = sarathi_tools::ToolSpec::new(t.name, t.description, t.input_schema);
                if READ_ONLY_TOOLS.contains(&t.name) {
                    spec = spec.read_only();
                }
                if EDIT_TOOLS.contains(&t.name) {
                    spec = spec.edits();
                }
                if t.name == "look" {
                    spec = spec.evidence(LOOK_EVIDENCE);
                }
                spec
            })
            .collect()
    }

    fn call<'b>(
        &'b mut self,
        name: &str,
        input: &Value,
    ) -> sarathi_tools::BoxFuture<'b, Result<String, String>> {
        let call = self.inner.call(name, input);
        Box::pin(async move {
            match call.await {
                Ok(out) => Ok(truncate(&out)),
                Err(e) => Err(truncate(&e)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::agent::ToolDef;
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    #[test]
    fn option_values() {
        assert_eq!(Harness::from_option(None), Harness::Vyasa);
        assert_eq!(Harness::from_option(Some(&Value::Null)), Harness::Vyasa);
        assert_eq!(Harness::from_option(Some(&json!("vyasa"))), Harness::Vyasa);
        assert_eq!(
            Harness::from_option(Some(&json!("sarathi"))),
            Harness::Sarathi
        );
        assert_eq!(
            Harness::from_option(Some(&json!("Sarathi"))),
            Harness::Vyasa
        );
        assert_eq!(Harness::from_option(Some(&json!(true))), Harness::Vyasa);
        for h in [Harness::Vyasa, Harness::Sarathi] {
            assert_eq!(Harness::parse(h.as_str()), Some(h));
        }
        assert_eq!(Harness::default(), Harness::Vyasa);
    }

    /// A studio-shaped toolbox: one edit, one look, one tool Sarathi has no
    /// flags for, and a call that fails as an observation.
    struct Studio {
        edits: usize,
    }

    impl Toolbox for Studio {
        fn tools(&self) -> Vec<ToolDef> {
            ["edit", "look", "inspect", "frobnicate"]
                .into_iter()
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
                    self.edits += 1;
                    Ok(format!("edited {input}"))
                }
                "look" => Ok("rendered".to_owned()),
                other => Err(format!("no tool {other}")),
            };
            Box::pin(std::future::ready(out))
        }
    }

    #[tokio::test]
    async fn sarathi_without_a_registry_model_runs_on_vyasas_loop() {
        use crate::agent::tests::{spec, EchoBox, NullSink, Scripted};
        let script = [
            r#"{"thought":"try","tool":"echo","input":{"msg":"hi"},"reply":""}"#,
            r#"{"thought":"done","tool":"done","input":{},"reply":"All set."}"#,
        ];
        let mut outcomes = Vec::new();
        for harness in [Harness::Vyasa, Harness::Sarathi] {
            let mut toolbox = EchoBox(Vec::new());
            let out = run(
                harness,
                &Scripted::new(&script),
                None,
                &NullSink,
                &spec(),
                &mut toolbox,
            )
            .await
            .expect("runs");
            assert_eq!(toolbox.0.len(), 1, "the tool really ran");
            outcomes.push((out.reply, out.steps.len(), out.ran_out));
        }
        assert_eq!(outcomes[0], outcomes[1]);
        assert_eq!(outcomes[0], ("All set.".to_owned(), 1, false));
    }

    #[tokio::test]
    async fn sarathi_sees_the_surface_tools_with_flags() {
        use sarathi_tools::Toolbox as _;
        let mut studio = Studio { edits: 0 };
        let mut tb = SarathiToolbox::new(&mut studio);
        let specs = tb.tools();
        let by = |n: &str| specs.iter().find(|s| s.name == n).cloned().unwrap();
        assert_eq!(specs.len(), 4);
        assert!(by("edit").edits && !by("edit").read_only);
        assert!(by("look").read_only);
        assert_eq!(by("look").evidence.as_deref(), Some(LOOK_EVIDENCE));
        assert!(by("inspect").read_only && by("inspect").evidence.is_none());
        let unknown = by("frobnicate");
        assert!(!unknown.read_only && !unknown.edits && unknown.evidence.is_none());
        assert_eq!(by("edit").input_schema, json!({"type": "object"}));

        assert_eq!(
            tb.call("edit", &json!({"a": 1})).await,
            Ok(r#"edited {"a":1}"#.to_owned())
        );
        // A failing call is an observation for the model, not a run error.
        let failed: sarathi_tools::ToolResult = tb.call("frobnicate", &json!({})).await.into();
        assert!(failed.is_error);
        assert_eq!(failed.content, "no tool frobnicate");
        assert_eq!(studio.edits, 1);
    }
}
