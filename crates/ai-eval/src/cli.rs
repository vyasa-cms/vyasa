//! The command line: one `sarathi-eval` pass, a dry run unless `--live`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use sarathi_eval::brief::{parse_brief_set, Brief};
use sarathi_eval::config::{RunConfig, SlotConfig};
use sarathi_eval::measure::{CostTolerance, Judging};
use sarathi_eval::pass::{Pass, PassKind, Summary};
use sarathi_eval::runner::{Acceptance, Harness as EvalHarness, HarnessError, HarnessOutput};
use sarathi_eval::structural::Structural;
use vyasa_ai::harness::Harness;
use vyasa_core::ai_models::ProviderId;

use crate::harness::{arm_label, check_briefs, Arm, Target};

/// How to call the program.
pub const USAGE: &str = "\
usage: ai-eval <smoke | tune ROUND | measure> --config FILE --briefs FILE
               --root DIR --date YYYY-MM-DD --artifacts DIR
  --root DIR                 the pass records and reports; every record there counts towards the caps
  --artifacts DIR            each run's proposal and steps, outside --root
  --slot NAME                tune: the slot (default strong)
  --cost-tolerance T         measure, required: old-spread, or a fraction such as 0.05
  --deviations N             measure: deviations from the plan, recorded in the report
  --base-url PROVIDER=URL    anthropic, openai or gemini somewhere else
  --live                     call the providers. Without it nothing is called: the plan is printed
Keys: VYASA_AI_ANTHROPIC_KEY, VYASA_AI_OPENAI_KEY, VYASA_AI_GEMINI_KEY, or the same name with _FILE
holding the path of a file with the key.";

/// The two briefs of the smoke run.
const SMOKE_BRIEFS: &[&str] = &["plain-02", "adv-03"];
/// The slot the kill line is decided on, and the default tuning slot.
const STRONG: &str = "strong";

/// Which pass to make.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The small run that prices the measurement.
    Smoke,
    /// A tuning round.
    Tune {
        /// From 1.
        round: u32,
    },
    /// The measurement.
    Measure {
        /// The cost side of the kill line.
        cost: Cost,
    },
}

/// The cost tolerance, as stated on the command line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cost {
    /// The old arm's own spread.
    OldSpread,
    /// A fixed fraction.
    Fraction(f64),
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The pass.
    pub command: Command,
    /// The run configuration file.
    pub config: PathBuf,
    /// The brief set file.
    pub briefs: PathBuf,
    /// The pass root.
    pub root: PathBuf,
    /// The pass date.
    pub date: String,
    /// Where run outputs go.
    pub artifacts: PathBuf,
    /// The tuning slot.
    pub slot: String,
    /// Deviations from the plan.
    pub deviations: u32,
    /// Whether to call the providers.
    pub live: bool,
    /// Base URLs by provider name.
    pub base_urls: BTreeMap<String, String>,
}

/// The measurement's cost tolerance: it has no default.
fn cost_tolerance(text: Option<String>) -> Result<Cost, String> {
    let text = text
        .ok_or("measure needs --cost-tolerance (old-spread or a fraction), fixed before the run")?;
    if text == "old-spread" {
        return Ok(Cost::OldSpread);
    }
    text.parse::<f64>()
        .ok()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map(Cost::Fraction)
        .ok_or_else(|| format!("`{text}` is not a cost tolerance"))
}

/// The pass root counts every file in it, so run outputs go elsewhere.
fn check_apart(root: &Path, artifacts: &Path) -> Result<(), String> {
    if [root, artifacts]
        .iter()
        .any(|p| p.components().any(|c| c == Component::ParentDir))
    {
        return Err("--root and --artifacts may not contain `..`".into());
    }
    let absolute = |p: &Path| std::path::absolute(p).map_err(|e| format!("{}: {e}", p.display()));
    if absolute(artifacts)?.starts_with(absolute(root)?) {
        return Err("--artifacts must be outside --root".into());
    }
    Ok(())
}

/// Parses the arguments after the program name.
///
/// # Errors
/// What is wrong with them.
pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut args = args.iter();
    let name = args.next().ok_or("no command")?;
    let round = if name == "tune" {
        let text = args.next().ok_or("tune takes a round number")?;
        Some(
            text.parse::<u32>()
                .map_err(|_| format!("`{text}` is not a round number"))?,
        )
    } else {
        None
    };
    let mut values: BTreeMap<&str, &String> = BTreeMap::new();
    let mut base_urls = BTreeMap::new();
    let mut live = false;
    while let Some(flag) = args.next() {
        if flag == "--live" {
            live = true;
            continue;
        }
        let known = [
            "--config",
            "--briefs",
            "--root",
            "--date",
            "--artifacts",
            "--slot",
            "--cost-tolerance",
            "--deviations",
            "--base-url",
        ];
        if !known.contains(&flag.as_str()) {
            return Err(format!("unknown argument `{flag}`"));
        }
        let value = args.next().ok_or_else(|| format!("{flag} takes a value"))?;
        if flag == "--base-url" {
            let (provider, url) = value
                .split_once('=')
                .ok_or("--base-url takes PROVIDER=URL")?;
            if !["anthropic", "openai", "gemini"].contains(&provider) {
                return Err(format!("unknown provider `{provider}`"));
            }
            base_urls.insert(provider.to_owned(), url.to_owned());
        } else if values.insert(flag, value).is_some() {
            return Err(format!("{flag} is given twice"));
        }
    }
    let mut take = |flag: &str| values.remove(flag).cloned();
    let mut need = |flag: &str| take(flag).ok_or_else(|| format!("{flag} is required"));
    let config = PathBuf::from(need("--config")?);
    let briefs = PathBuf::from(need("--briefs")?);
    let root = PathBuf::from(need("--root")?);
    let date = need("--date")?;
    let artifacts = PathBuf::from(need("--artifacts")?);
    let command = match (name.as_str(), round) {
        ("smoke", _) => Command::Smoke,
        ("tune", Some(round)) => Command::Tune { round },
        ("measure", _) => Command::Measure {
            cost: cost_tolerance(take("--cost-tolerance"))?,
        },
        (other, _) => return Err(format!("unknown command `{other}`")),
    };
    let slot = take("--slot");
    if slot.is_some() && !matches!(command, Command::Tune { .. }) {
        return Err("--slot is for tune".into());
    }
    let deviations = take("--deviations")
        .map(|t| {
            t.parse::<u32>()
                .map_err(|_| format!("`{t}` is not a number of deviations"))
        })
        .transpose()?
        .unwrap_or(0);
    // What is left was not for this command.
    if let Some(flag) = values.keys().next() {
        return Err(format!("{flag} is not for `{name}`"));
    }
    check_apart(&root, &artifacts)?;
    Ok(Options {
        command,
        config,
        briefs,
        root,
        date,
        artifacts,
        slot: slot.unwrap_or_else(|| STRONG.to_owned()),
        deviations,
        live,
        base_urls,
    })
}

/// The structural check, saying why it refused a run.
struct Explained(Structural);

impl Acceptance for Explained {
    fn handles(&self, check: &str) -> bool {
        self.0.handles(check)
    }

    fn accepts(&self, brief: &Brief, out: &HarnessOutput) -> bool {
        match self.0.check(out) {
            Ok(()) => true,
            Err(why) => {
                eprintln!("refused: brief {}: {why}", brief.id);
                false
            }
        }
    }
}

/// Stands where no arm was built; the factory below always finds one.
struct Unbuilt;

impl EvalHarness for Unbuilt {
    fn run(&self, _: &Brief, _: &str, _: u32) -> Result<HarnessOutput, HarnessError> {
        Err(HarnessError::new("no arm was built for this slot"))
    }
}

/// The provider of a model id.
fn provider_of(model: &str) -> Result<ProviderId, String> {
    [
        ("claude-", ProviderId::Anthropic),
        ("gpt-", ProviderId::OpenAi),
        ("gemini-", ProviderId::Gemini),
    ]
    .into_iter()
    .find(|(prefix, _)| model.starts_with(prefix))
    .map(|(_, provider)| provider)
    .ok_or_else(|| format!("no provider is known for model `{model}`"))
}

/// The key of `provider`, from the environment variable Vyasa reads or the
/// file its `_FILE` form names. Never part of an error.
fn key_of(provider: ProviderId, env: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let name = provider.env_key();
    let file = format!("{name}_FILE");
    let key = if let Some(key) = env(name).filter(|k| !k.trim().is_empty()) {
        key
    } else {
        let path = env(&file).ok_or_else(|| format!("set {name} or {file}"))?;
        std::fs::read_to_string(&path).map_err(|e| format!("{file}: {path}: {e}"))?
    };
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err(format!("{file} names an empty file"));
    }
    Ok(key)
}

fn dollars(micro: u64) -> String {
    format!("${}.{:06}", micro / 1_000_000, micro % 1_000_000)
}

fn print_summary(summary: &Summary, out: &mut dyn Write) -> std::io::Result<()> {
    writeln!(out, "pass directory: {}", summary.dir.display())?;
    for s in &summary.slots {
        let projected = s
            .projected_micro
            .map_or_else(String::new, |p| format!(", projected {}", dollars(p)));
        writeln!(
            out,
            "  {} ({}): {} of {} runs on disk, spent {} of {}{projected}",
            s.slot,
            s.model,
            s.runs_done,
            s.runs_planned,
            dollars(s.spent_micro),
            dollars(s.cap_micro),
        )?;
    }
    Ok(())
}

/// The arms of the pass's slots, with their keys: everything that can be
/// refused before money is spent.
fn build_arms(
    options: &Options,
    config: &RunConfig,
    summary: &Summary,
    pass: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<(String, Arm)>, String> {
    let mut arms = Vec::new();
    for slot in &summary.slots {
        let Some(SlotConfig {
            native_tools,
            model,
            ..
        }) = config.slot(&slot.slot)
        else {
            return Err(format!("slot `{}` is not configured", slot.slot));
        };
        let provider = provider_of(model)?;
        let target = Target {
            provider,
            model: model.clone(),
            base_url: options
                .base_urls
                .get(provider.as_str())
                .cloned()
                .unwrap_or_else(|| provider.default_base_url().to_owned()),
            api_key: key_of(provider, env)?,
            native_tools: *native_tools,
        };
        for name in &config.harnesses {
            let harness = [Harness::Vyasa, Harness::Sarathi]
                .into_iter()
                .find(|h| arm_label(*h) == name)
                .ok_or_else(|| format!("unknown harness `{name}`: `old` or `new`"))?;
            arms.push((
                name.clone(),
                Arm {
                    harness,
                    target: target.clone(),
                    slot: slot.slot.clone(),
                    artifacts: options.artifacts.join(pass),
                },
            ));
        }
    }
    Ok(arms)
}

/// Makes the pass `options` describe. `env` reads an environment variable.
///
/// # Errors
/// Why the pass was refused or stopped.
pub fn execute(
    options: &Options,
    env: &dyn Fn(&str) -> Option<String>,
    out: &mut dyn Write,
) -> Result<(), String> {
    let read =
        |path: &Path| std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
    let config = RunConfig::from_json(&read(&options.config)?).map_err(|e| e.to_string())?;
    let briefs = parse_brief_set(&read(&options.briefs)?).map_err(|e| e.to_string())?;
    check_briefs(&briefs)?;
    let (kind, pass_name) = match &options.command {
        Command::Smoke => (
            PassKind::Smoke {
                briefs: SMOKE_BRIEFS,
            },
            "smoke".to_owned(),
        ),
        Command::Tune { round } => (
            PassKind::Tune {
                round: *round,
                slot: &options.slot,
            },
            format!("tune-{round}"),
        ),
        Command::Measure { cost } => (
            PassKind::Measure {
                judging: Judging {
                    old: arm_label(Harness::Vyasa),
                    new: arm_label(Harness::Sarathi),
                    strong_slot: STRONG,
                    cost: match cost {
                        Cost::OldSpread => CostTolerance::OldSpread,
                        Cost::Fraction(f) => CostTolerance::Fraction(*f),
                    },
                },
            },
            "measure".to_owned(),
        ),
    };
    let check = Explained(Structural::default());
    let pass = Pass::prepare(&config, &briefs, kind, &check, &options.root, &options.date)
        .map_err(|e| e.to_string())?;
    let io = |e: std::io::Error| format!("output: {e}");
    print_summary(pass.summary(), out).map_err(io)?;
    if !options.live {
        writeln!(
            out,
            "dry run: nothing was called. Pass --live to make the runs."
        )
        .map_err(io)?;
        return Ok(());
    }
    let arms = build_arms(options, &config, pass.summary(), &pass_name, env)?;
    let unbuilt = Unbuilt;
    let dir = pass.summary().dir.join(&options.date);
    let (run, _report) = pass
        .run(
            |name, slot| {
                arms.iter()
                    .find(|(n, arm)| n == name && arm.slot == slot.slot)
                    .map_or(&unbuilt as &dyn EvalHarness, |(_, arm)| arm)
            },
            options.deviations,
        )
        .map_err(|e| e.to_string())?;
    if !run.stopped.is_empty() {
        writeln!(out, "stopped at the cap: {:?}", run.stopped).map_err(io)?;
    }
    writeln!(out, "report: {}", dir.join("report.md").display()).map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn each_slot_model_names_its_provider() {
        assert_eq!(provider_of("claude-sonnet-5-5"), Ok(ProviderId::Anthropic));
        assert_eq!(provider_of("gpt-5-mini"), Ok(ProviderId::OpenAi));
        assert_eq!(provider_of("gemini-2.5-flash"), Ok(ProviderId::Gemini));
        assert!(provider_of("llama-3").is_err());
    }

    #[test]
    fn a_key_comes_from_the_variable_or_its_file() {
        let path = std::env::temp_dir().join(format!("ai-eval-key-{}", std::process::id()));
        std::fs::write(&path, "from-file\n").unwrap();
        let file = path.display().to_string();
        let set = |name: &str| (name == "VYASA_AI_OPENAI_KEY").then(|| "direct".to_owned());
        assert_eq!(key_of(ProviderId::OpenAi, &set), Ok("direct".to_owned()));
        let named = |name: &str| (name == "VYASA_AI_OPENAI_KEY_FILE").then(|| file.clone());
        assert_eq!(
            key_of(ProviderId::OpenAi, &named),
            Ok("from-file".to_owned())
        );
        std::fs::write(&path, "  \n").unwrap();
        assert!(key_of(ProviderId::OpenAi, &named).is_err(), "an empty file");
        std::fs::remove_file(&path).unwrap();
        let why = key_of(ProviderId::OpenAi, &named).unwrap_err();
        assert!(why.contains("VYASA_AI_OPENAI_KEY_FILE"), "{why}");
        assert!(key_of(ProviderId::Gemini, &|_| None).is_err());
    }
}
