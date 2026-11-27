//! The whole caller offline: both arms against a local stub of the Messages
//! API, through `sarathi-eval`'s passes. No provider is called.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sarathi_eval::brief::{write_brief_set, Brief, Split};
use sarathi_eval::runner::{Harness as _, HarnessOutput};
use sarathi_eval::structural::Structural;
use serde_json::{json, Value};
use vyasa_ai::agent::AgentStep;
use vyasa_ai::harness::Harness;
use vyasa_ai::theme_studio::StudioEyes;
use vyasa_ai_eval::cli::{execute, parse, Command, Cost};
use vyasa_ai_eval::harness::{check_briefs, Arm, Target};
use vyasa_ai_eval::site::{count_ops, proposal, OfflineEyes, Starter};
use vyasa_core::ai_models::ProviderId;

const LAW: &str = "A three-partner law firm: practice areas, team, news, contact.";
const SAAS: &str = "A B2B analytics product: landing page with hero, features, pricing.";

/// What the stub was sent, in order.
type Seen = Arc<Mutex<Vec<String>>>;

/// A Messages API that plays an agent making one edit and looking at it, for
/// either arm, deciding from what the request already holds. `usage` is what
/// each reply reports.
fn stub(usage: Value) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let mut sock = sock.unwrap();
            let log = Arc::clone(&log);
            let usage = usage.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let (head, body) = loop {
                    let k = sock.read(&mut tmp).unwrap();
                    if k == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..k]);
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    if buf.len() >= end + 4 + len {
                        let body =
                            String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                        break (head, body);
                    }
                };
                let path = head.split(' ').nth(1).unwrap_or_default();
                let answer = if path.ends_with("/count_tokens") {
                    json!({"input_tokens": 100})
                } else {
                    log.lock().unwrap().push(body.clone());
                    reply(&body, &usage)
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                sock.write_all(response.as_bytes()).unwrap();
            });
        }
    });
    (url, seen)
}

fn is_old_arm(body: &str) -> bool {
    body.contains("emit_result")
}

fn reply(body: &str, usage: &Value) -> Value {
    let edit = json!({"ops": [{"op": "patch_tokens", "patch": {"radius_px": 3}}]});
    let look = json!({"path": "/"});
    let content = if is_old_arm(body) {
        let step = |tool: &str, input: &Value, reply: &str| {
            json!([{"type": "tool_use", "id": "t", "name": "emit_result",
                    "input": {"thought": "next", "tool": tool, "input": input, "reply": reply}}])
        };
        if !body.contains("called: edit(") {
            step("edit", &edit, "")
        } else if !body.contains("called: look(") {
            step("look", &look, "")
        } else {
            step("done", &json!({}), "Rounded the corners.")
        }
    } else {
        let call = |id: &str, name: &str, input: &Value| json!([{"type": "tool_use", "id": id, "name": name, "input": input}]);
        if !body.contains("toolu_plan") {
            let plan = json!({"criteria": [{"id": "r", "text": "rounder", "kind": "look"}]});
            call("toolu_plan", "plan", &plan)
        } else if !body.contains("toolu_edit") {
            call("toolu_edit", "edit", &edit)
        } else if !body.contains("toolu_look") {
            call("toolu_look", "look", &look)
        } else {
            json!([{"type": "text", "text": "Rounded the corners."}])
        }
    };
    let stop = if content[0]["type"] == "text" {
        "end_turn"
    } else {
        "tool_use"
    };
    json!({"id": "msg", "type": "message", "role": "assistant", "model": "claude-test",
           "content": content, "stop_reason": stop, "usage": usage})
}

fn usage() -> Value {
    json!({"input_tokens": 100, "output_tokens": 20})
}

fn brief(id: &str, context: &str, fault: Option<&str>) -> Brief {
    Brief {
        id: id.into(),
        split: Split::Measurement,
        site_context: context.into(),
        text: "Make the corners rounder.".into(),
        allowed_models: vec!["strong".into()],
        acceptance: "structural".into(),
        fault: fault.map(Into::into),
    }
}

fn arm(harness: Harness, url: &str, artifacts: &Path) -> Arm {
    Arm {
        harness,
        target: Target {
            provider: ProviderId::Anthropic,
            model: "claude-test".into(),
            base_url: url.into(),
            api_key: "test-key".into(),
            native_tools: true,
        },
        slot: "strong".into(),
        artifacts: artifacts.into(),
    }
}

/// A fresh directory under the system temp directory.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ai-eval-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes the files of a pass and returns the arguments that name them.
fn files(dir: &Path, command: &[&str], url: &str) -> Vec<String> {
    let config = json!({
        "version": 1, "runs": 1, "harnesses": ["old", "new"], "total_cap_usd": 5,
        "slots": [{"slot": "strong", "model": "claude-test", "native_tools": true, "cap_usd": 5,
                   "price": {"input": 1_000_000, "output": 2_000_000, "cached_input": 100_000}}]
    });
    std::fs::write(dir.join("run.json"), config.to_string()).unwrap();
    let briefs = [brief("plain-02", LAW, None), brief("adv-03", SAAS, None)];
    std::fs::write(dir.join("briefs.jsonl"), write_brief_set(&briefs).unwrap()).unwrap();
    let path = |name: &str| dir.join(name).display().to_string();
    let mut args: Vec<String> = command.iter().map(|s| (*s).to_owned()).collect();
    for (flag, value) in [
        ("--config", path("run.json")),
        ("--briefs", path("briefs.jsonl")),
        ("--root", path("root")),
        ("--artifacts", path("artifacts")),
        ("--date", "2026-01-01".to_owned()),
        ("--base-url", format!("anthropic={url}")),
    ] {
        args.push(flag.to_owned());
        args.push(value);
    }
    args
}

fn key(name: &str) -> Option<String> {
    (name == "VYASA_AI_ANTHROPIC_KEY").then(|| "test-key".to_owned())
}

#[test]
fn a_live_smoke_pass_runs_both_arms_and_accepts_what_they_did() {
    let (url, seen) = stub(usage());
    let dir = scratch("smoke");
    let mut args = files(&dir, &["smoke"], &url);
    args.push("--live".to_owned());
    let mut out = Vec::new();
    execute(&parse(&args).unwrap(), &key, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("report: "), "{out}");
    assert!(!out.contains("test-key"));

    let seen = seen.lock().unwrap();
    // Two briefs: three calls each on Vyasa's loop, four behind Sarathi's gate.
    assert_eq!(seen.iter().filter(|b| is_old_arm(b)).count(), 6);
    assert_eq!(seen.iter().filter(|b| !is_old_arm(b)).count(), 8);
    for (arm, calls, input) in [("old.strong", 3, 300), ("new.strong", 4, 400)] {
        let record = dir.join(format!(
            "root/smoke/2026-01-01/measure/{arm}/plain-02/0.json"
        ));
        let record: Value = serde_json::from_str(
            &std::fs::read_to_string(&record)
                .unwrap_or_else(|e| panic!("{}: {e}", record.display())),
        )
        .unwrap();
        assert_eq!(record["accepted"], true, "{record}");
        assert_eq!(record["steps"], calls, "{record}");
        assert_eq!(record["usage"]["input_tokens"], input, "{record}");
        assert_eq!(record["valid_ops"], 1, "{record}");
        let saved = dir.join(format!("artifacts/smoke/{arm}/plain-02-0.json"));
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(saved).unwrap()).unwrap();
        assert_eq!(saved["tokens"]["radius_px"].as_f64(), Some(3.0));
        assert!(saved["steps"].as_array().unwrap().len() >= 2);
        assert_eq!(saved["proposal"]["errors"], json!([]));
    }
    drop(seen);

    // The smoke run priced the arms, so the measurement may start; it
    // counts the smoke run's spend and judges the two arms.
    let mut measure = files(&dir, &["measure"], &url);
    measure.extend(["--cost-tolerance", "old-spread", "--live"].map(str::to_owned));
    let mut out = Vec::new();
    execute(&parse(&measure).unwrap(), &key, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(!out.contains("spent $0.000000"), "{out}");
    let report = dir.join("root/measure/2026-01-01/report.md");
    let report =
        std::fs::read_to_string(&report).unwrap_or_else(|e| panic!("{}: {e}", report.display()));
    assert!(!report.contains("not evaluated"), "{report}");
    assert!(dir
        .join("artifacts/measure/new.strong/adv-03-0.json")
        .exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dry_run_calls_nothing_and_needs_no_key() {
    let (url, seen) = stub(usage());
    let dir = scratch("dry");
    let args = files(&dir, &["smoke"], &url);
    let mut out = Vec::new();
    execute(&parse(&args).unwrap(), &|_| None, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("0 of 4 runs on disk"), "{out}");
    assert!(out.contains("dry run"), "{out}");
    assert!(seen.lock().unwrap().is_empty());
    assert!(!dir.join("root").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_live_pass_without_a_key_is_refused_before_any_call() {
    let (url, seen) = stub(usage());
    let dir = scratch("nokey");
    let mut args = files(&dir, &["smoke"], &url);
    args.push("--live".to_owned());
    let why = execute(&parse(&args).unwrap(), &|_| None, &mut Vec::new()).unwrap_err();
    assert!(why.contains("VYASA_AI_ANTHROPIC_KEY"), "{why}");
    assert!(seen.lock().unwrap().is_empty());
    assert!(!dir.join("root").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_malformed_steps_fault_is_injected_once_into_both_arms() {
    for harness in [Harness::Vyasa, Harness::Sarathi] {
        let (url, seen) = stub(usage());
        let dir = scratch(&format!("fault-{harness:?}"));
        let faulty = brief("adv-04", LAW, Some("malformed_steps"));
        let out = arm(harness, &url, &dir)
            .run(&faulty, "claude-test", 0)
            .unwrap();
        let seen = seen.lock().unwrap();
        // The injected answer is not sent, billed or counted; the model
        // reads that its step was unusable and then does the work.
        let (calls, told) = match harness {
            Harness::Vyasa => (3, "previous response was invalid"),
            Harness::Sarathi => (4, "apply_theme"),
        };
        assert!(seen[0].contains(told), "{}", seen[0]);
        assert_eq!(seen.len(), calls, "{harness:?}");
        assert_eq!(out.steps as usize, calls, "{harness:?}");
        assert_eq!(out.valid_ops, 1, "{harness:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_reply_without_usage_fails_the_run_instead_of_counting_it_free() {
    for harness in [Harness::Vyasa, Harness::Sarathi] {
        let (url, _) = stub(json!({"input_tokens": 0, "output_tokens": 0}));
        let dir = scratch(&format!("unbilled-{harness:?}"));
        let why = arm(harness, &url, &dir)
            .run(&brief("plain-02", LAW, None), "claude-test", 0)
            .unwrap_err();
        assert!(why.message.contains("no usage"), "{}", why.message);
        assert!(why.partial.artifact.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_failed_run_keeps_what_it_spent() {
    // Nothing listens on the old arm's URL after the listener is dropped.
    let url = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/v1", listener.local_addr().unwrap())
    };
    let dir = scratch("down");
    for harness in [Harness::Vyasa, Harness::Sarathi] {
        let why = arm(harness, &url, &dir)
            .run(&brief("plain-02", LAW, None), "claude-test", 0)
            .unwrap_err();
        assert_eq!(why.partial.steps, 0, "{}", why.message);
        assert!(!why.message.contains("test-key"));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_starter_passes_the_structural_check_before_any_edit() {
    for starter in Starter::ALL {
        let draft = starter.draft().unwrap();
        let out = HarnessOutput {
            artifact: proposal(&draft, &draft).to_artifact().unwrap(),
            valid_ops: 1,
            ..HarnessOutput::default()
        };
        let checked = Structural::default().check(&out);
        assert!(checked.is_ok(), "{starter:?}: {checked:?}");
    }
}

#[test]
fn a_removed_home_section_and_unreadable_text_are_seen() {
    let before = Starter::Blog.draft().unwrap();
    let mut after = before.clone();
    after.layout.index.pop();
    after.tokens.colors.text = after.tokens.colors.bg.clone();
    let p = proposal(&before, &after);
    assert!(p.sections.len() < p.required_sections.len(), "{p:?}");
    assert!(p.contrast.iter().any(|c| c.ratio < 1.01), "{p:?}");
}

#[test]
fn a_site_context_names_its_starter_and_an_unknown_one_is_refused() {
    assert_eq!(Starter::for_site(LAW), Ok(Starter::Blog));
    assert_eq!(Starter::for_site(SAAS), Ok(Starter::Technology));
    assert!(Starter::for_site("A zoo").is_err());
    let mut bad = brief("x", "A zoo", None);
    assert!(check_briefs(&[bad.clone()]).is_err());
    bad.site_context = LAW.into();
    assert!(check_briefs(&[bad.clone()]).is_ok());
    bad.fault = Some("fire".into());
    assert!(check_briefs(&[bad]).is_err());
}

#[test]
fn ops_are_counted_from_what_the_toolbox_answered() {
    let step = |tool: &str, input: Value, observation: &str| AgentStep {
        thought: String::new(),
        tool: tool.into(),
        input,
        observation: observation.into(),
    };
    let steps = [
        step("edit", json!({"ops": [{}, {}]}), "Applied:\nrounder"),
        step("edit", json!({"ops": [{}]}), "The tool refused: no"),
        step("edit", json!({}), "edit takes ops"),
        step("edit_menu", json!({}), "Staged: created menu"),
        step("edit_menu", json!({}), "edit_menu refused"),
        step("look", json!({}), "Applied"),
    ];
    assert_eq!(count_ops(&steps), (3, 6));
}

#[tokio::test]
async fn the_eyes_render_the_draft_and_tell_what_the_brief_says() {
    let draft = Starter::Blog.draft().unwrap();
    let mut eyes = OfflineEyes {
        context: LAW.into(),
    };
    for path in [
        "/",
        "/post/a",
        "/category/a",
        "/search?s=a",
        "/about",
        "/a/b/c",
    ] {
        let seen = eyes.look(&draft, &[], path).await.unwrap();
        assert!(seen.starts_with("sections in order:"), "{path}: {seen}");
    }
    assert_eq!(eyes.content().await.unwrap(), LAW);
}

#[test]
fn the_measurement_states_its_cost_tolerance_first() {
    let dir = scratch("cli");
    let measure = files(&dir, &["measure"], "http://127.0.0.1:1/v1");
    let why = parse(&measure).unwrap_err();
    assert!(why.contains("--cost-tolerance"), "{why}");
    for (text, cost) in [
        ("old-spread", Cost::OldSpread),
        ("0.05", Cost::Fraction(0.05)),
    ] {
        let mut args = measure.clone();
        args.extend(["--cost-tolerance".to_owned(), text.to_owned()]);
        let options = parse(&args).unwrap();
        assert_eq!(options.command, Command::Measure { cost });
        assert!(!options.live, "a pass is dry unless asked");
    }
    for bad in ["-0.1", "NaN", "inf", "much"] {
        let mut args = measure.clone();
        args.extend(["--cost-tolerance".to_owned(), bad.to_owned()]);
        assert!(parse(&args).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_outputs_may_not_go_into_the_pass_root_and_flags_are_checked() {
    let dir = scratch("flags");
    let good = files(&dir, &["tune", "1"], "http://127.0.0.1:1/v1");
    assert_eq!(parse(&good).unwrap().command, Command::Tune { round: 1 });
    assert_eq!(parse(&good).unwrap().slot, "strong");
    let with = |flag: &str, value: &str| {
        let mut args = good.clone();
        if let Some(at) = args.iter().position(|a| a == flag) {
            args[at + 1] = value.to_owned();
        } else {
            args.extend([flag.to_owned(), value.to_owned()]);
        }
        parse(&args)
    };
    let inside = dir.join("root/outputs").display().to_string();
    assert!(with("--artifacts", &inside).is_err());
    let sneaky = dir.join("x/../root/outputs").display().to_string();
    assert!(with("--artifacts", &sneaky).is_err());
    assert!(with("--cost-tolerance", "0.1").is_err(), "not for tune");
    assert!(with("--base-url", "elsewhere=http://x").is_err());
    assert!(with("--colour", "red").is_err());
    assert!(parse(&["smoke".to_owned()]).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
