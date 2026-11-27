//! One arm of the comparison as a `sarathi-eval` harness: one run of one
//! brief, set up as the Theme Studio job sets it up.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sarathi_eval::brief::Brief;
use sarathi_eval::runner::{Harness as EvalHarness, HarnessError, HarnessOutput};
use serde_json::json;
use vyasa_ai::agent::{AgentOutcome, AgentSpec};
use vyasa_ai::harness::{self, Harness, SarathiModel, Wire};
use vyasa_ai::theme_studio::{
    request_allows_script, system_prompt, user_prompt, AssistInput, StudioToolbox,
};
use vyasa_ai::{CircuitBreaker, FailoverProvider, ModelSlot, ProviderSlot};
use vyasa_core::ai_models::ProviderId;
use vyasa_themes::{builtin_registry, registry_schema, DraftState};

use crate::probe::{Probe, ProbeProvider, ProbeWrap, MALFORMED_STEPS};
use crate::site::{count_ops, proposal, OfflineEyes, Starter};
use crate::usage::RunSink;

/// The step budget the Theme Studio job gives a run.
const MAX_STEPS: usize = 12;
/// The answer length Vyasa gives a Sarathi step when the registry row sets
/// none.
const SARATHI_MAX_TOKENS: u32 = 4096;
/// The Theme Studio purpose, as `ai_logs` records it.
const PURPOSE: &str = "theme-studio";

/// The model of one slot and how to reach it.
#[derive(Clone)]
pub struct Target {
    /// The provider.
    pub provider: ProviderId,
    /// The model id.
    pub model: String,
    /// The API's base URL, in the form Vyasa's registry stores.
    pub base_url: String,
    /// The API key.
    pub api_key: String,
    /// Whether the new arm offers native tools; the old arm always uses
    /// JSON steps.
    pub native_tools: bool,
}

/// Never prints the key.
impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("native_tools", &self.native_tools)
            .finish_non_exhaustive()
    }
}

/// One harness on one slot.
#[derive(Debug)]
pub struct Arm {
    /// Vyasa's loop, or Sarathi.
    pub harness: Harness,
    /// The slot's model.
    pub target: Target,
    /// The slot's name, for the saved outputs.
    pub slot: String,
    /// Where each run's output is saved for a person to read.
    pub artifacts: PathBuf,
}

/// Refuses a brief set this crate cannot run as written: an unknown fault or
/// kind of site. Checked before any run is paid for.
///
/// # Errors
/// Naming the first such brief.
pub fn check_briefs(briefs: &[Brief]) -> Result<(), String> {
    for b in briefs {
        Starter::for_site(&b.site_context).map_err(|e| format!("brief `{}`: {e}", b.id))?;
        if let Some(fault) = b.fault.as_deref().filter(|f| *f != MALFORMED_STEPS) {
            return Err(format!("brief `{}`: unknown fault `{fault}`", b.id));
        }
    }
    Ok(())
}

struct Run {
    outcome: Result<AgentOutcome, String>,
    toolbox: StudioToolbox<OfflineEyes>,
    before: DraftState,
}

impl Arm {
    fn run_once(&self, brief: &Brief, sink: &RunSink, probe: &Arc<Probe>) -> Result<Run, String> {
        let before = Starter::for_site(&brief.site_context)?.draft()?;
        let vocabulary = registry_schema();
        let system = system_prompt(&vocabulary);
        let task = user_prompt(
            &before,
            &AssistInput {
                vocabulary: &vocabulary,
                site: &brief.site_context,
                brand: "",
                history: &[],
                message: &brief.text,
                attachments: Vec::new(),
                model: &self.target.model,
            },
        );
        let mut toolbox = StudioToolbox::new(
            &before,
            builtin_registry(),
            Vec::new(),
            Some(OfflineEyes {
                context: brief.site_context.clone(),
            }),
        )
        .allow_script(request_allows_script(&brief.text));
        let spec = AgentSpec {
            purpose: PURPOSE,
            model: &self.target.model,
            system: &system,
            task: &task,
            max_steps: MAX_STEPS,
            attachments: Vec::new(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("runtime: {e}"))?;
        let outcome = runtime.block_on(self.drive(&spec, sink, probe, &mut toolbox));
        Ok(Run {
            outcome,
            toolbox,
            before,
        })
    }

    /// The run on this arm's harness.
    async fn drive(
        &self,
        spec: &AgentSpec<'_>,
        sink: &RunSink,
        probe: &Arc<Probe>,
        toolbox: &mut StudioToolbox<OfflineEyes>,
    ) -> Result<AgentOutcome, String> {
        let t = &self.target;
        let outcome = match self.harness {
            Harness::Vyasa => {
                let provider = ProbeProvider {
                    inner: chain(t)?,
                    probe: Arc::clone(probe),
                };
                harness::run(Harness::Vyasa, &provider, None, sink, spec, toolbox).await
            }
            Harness::Sarathi => {
                // Straight to Sarathi: this arm has no way back to Vyasa's
                // loop.
                let wire = match t.provider {
                    ProviderId::Anthropic => Wire::Anthropic,
                    _ => Wire::OpenAi,
                };
                let model = SarathiModel {
                    provider: t.provider.as_str(),
                    wire,
                    model: t.model.clone(),
                    base_url: t.base_url.clone(),
                    api_key: t.api_key.clone(),
                    max_tokens: SARATHI_MAX_TOKENS,
                    native_tools: t.native_tools,
                };
                let wrap = ProbeWrap {
                    probe: Arc::clone(probe),
                    wire,
                    native_tools: t.native_tools,
                };
                harness::sarathi::run_wrapped(&model, wrap, sink, spec, toolbox).await
            }
        };
        outcome.map_err(|e| e.to_string())
    }

    /// Writes what the run produced, for the labelling sheet. A write that
    /// fails is reported and does not lose a run that was paid for.
    fn save(&self, brief: &Brief, run: u32, document: &serde_json::Value) {
        let arm = format!("{}.{}", arm_label(self.harness), self.slot);
        let dir = self.artifacts.join(arm);
        let path = dir.join(format!("{}-{run}.json", brief.id));
        let written = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, format!("{document:#}\n")));
        if let Err(e) = written {
            eprintln!("could not save {}: {e}", path.display());
        }
    }
}

/// The name a run configuration gives a harness.
#[must_use]
pub fn arm_label(harness: Harness) -> &'static str {
    match harness {
        Harness::Vyasa => "old",
        Harness::Sarathi => "new",
    }
}

/// The old arm's provider, built as the server builds its chain: one slot,
/// no settings.
fn chain(t: &Target) -> Result<FailoverProvider, String> {
    let provider =
        vyasa_ai::catalog::chat_provider(t.provider, &t.api_key, &t.base_url, &json!({}))
            .map_err(|e| e.to_string())?;
    let breaker = CircuitBreaker::new(3, Duration::from_secs(60));
    FailoverProvider::new(vec![ModelSlot {
        slot: ProviderSlot { provider, breaker },
        model: t.model.clone(),
    }])
    .map_err(|e| e.to_string())
}

impl EvalHarness for Arm {
    fn run(&self, brief: &Brief, model: &str, run: u32) -> Result<HarnessOutput, HarnessError> {
        if model != self.target.model {
            return Err(HarnessError::new(format!(
                "this arm runs `{}`, not `{model}`",
                self.target.model
            )));
        }
        let sink = RunSink::default();
        let probe = Probe::new(brief.fault.as_deref() == Some(MALFORMED_STEPS));
        let started = Instant::now();
        let done = self
            .run_once(brief, &sink, &probe)
            .map_err(HarnessError::new)?;
        let wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let after = &done.toolbox.sandbox;
        let judged = proposal(&done.before, after);
        let steps = done.outcome.as_ref().map(|o| o.steps.as_slice());
        let (valid_ops, total_ops) = count_ops(steps.unwrap_or_default());
        let mut out = HarnessOutput {
            artifact: judged
                .to_artifact()
                .map_err(|e| HarnessError::new(format!("the proposal does not serialise: {e}")))?,
            valid_ops,
            total_ops,
            usage: sink.usage(),
            steps: sink.calls(),
            wall_ms,
        };
        let failure = match &done.outcome {
            Err(e) => Some(e.clone()),
            Ok(_) if probe.unbilled() => {
                Some("a model reply carried no usage: the run's cost is unknown".to_owned())
            }
            Ok(_) => None,
        };
        let outcome = done.outcome.as_ref().ok();
        self.save(
            brief,
            run,
            &json!({
                "brief": brief.id,
                "model": model,
                "run": run,
                "error": failure,
                "reply": outcome.map(|o| &o.reply),
                "ran_out": outcome.map(|o| o.ran_out),
                "steps": outcome.map(|o| &o.steps),
                "changes": done.toolbox.changes,
                "menus": done.toolbox.menus,
                "tokens": after.tokens_json(),
                "layout": after.layout_json(),
                "templates": after.templates_json(),
                "assets": after.assets_json(),
                "proposal": judged,
            }),
        );
        match failure {
            Some(message) => {
                out.artifact = String::new();
                Err(HarnessError::after(message, out))
            }
            None => Ok(out),
        }
    }
}
