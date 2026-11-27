//! The Sarathi harness end to end: `harness::run` with `ai_harness =
//! sarathi` drives a real HTTP model (a local stub of the Anthropic
//! Messages API) over the studio's own toolbox.
//!
//! These pin the completion gate as a site sees it: the gate is on only
//! when the surface offers `look`, its evidence kinds come from the
//! toolbox, a finish without evidence gathered after the last edit is sent
//! back, three refusals end the run as out of steps, and a surface with
//! its own `plan` tool runs ungated.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vyasa_ai::agent::{AgentOutcome, AgentSpec, ToolDef, Toolbox};
use vyasa_ai::harness::{self, Harness, SarathiModel, Wire};
use vyasa_ai::theme_studio::{StudioEyes, StudioToolbox};
use vyasa_ai::{AiUsageSink, Completion, LlmProvider, PromptSpec, ProviderError};
use vyasa_themes::DraftState;

/// A server that answers each request with the next of `answers` and keeps
/// every request body, in order.
async fn serve(answers: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = bodies.clone();
    let answers = Arc::new(Mutex::new(answers.into_iter()));
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let seen = seen.clone();
            let answers = answers.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let (path, body) = loop {
                    let k = sock.read(&mut tmp).await.unwrap();
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
                        let path = head.split(' ').nth(1).unwrap().to_owned();
                        let body =
                            String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                        break (path, body);
                    }
                };
                let answer = if path.ends_with("/count_tokens") {
                    r#"{"input_tokens":100}"#.to_owned()
                } else {
                    seen.lock().unwrap().push(body);
                    answers
                        .lock()
                        .unwrap()
                        .next()
                        .expect("the model was called more often than scripted")
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
            });
        }
    });
    (url, bodies)
}

/// A Messages answer: optional text, then tool calls `(id, name, input)`.
fn answer(text: Option<&str>, calls: &[(&str, &str, Value)]) -> String {
    let mut content: Vec<Value> = text
        .map(|t| json!({"type": "text", "text": t}))
        .into_iter()
        .collect();
    for (id, name, input) in calls {
        content.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
    }
    let stop = if calls.is_empty() {
        "end_turn"
    } else {
        "tool_use"
    };
    json!({
        "id": "msg_01", "type": "message", "role": "assistant", "model": "m-1",
        "content": content, "stop_reason": stop,
        "usage": {"input_tokens": 12, "output_tokens": 5}
    })
    .to_string()
}

fn finish(text: &str) -> String {
    answer(Some(text), &[])
}

/// A plan with one criterion checked by `look`.
fn plan(id: &str) -> (&str, &'static str, Value) {
    (
        id,
        "plan",
        json!({"criteria": [{"id": "red", "text": "the title is red", "kind": "look"}]}),
    )
}

fn edit(id: &str) -> (&str, &'static str, Value) {
    (
        id,
        "edit",
        json!({"ops": [{"op": "patch_tokens", "patch": {"colors": {"primary": {"light": "#b91c1c", "dark": "#fca5a5"}}}}]}),
    )
}

fn look(id: &str) -> (&str, &'static str, Value) {
    (id, "look", json!({"path": "/"}))
}

fn model(url: &str) -> SarathiModel {
    SarathiModel {
        provider: "anthropic",
        wire: Wire::Anthropic,
        model: "m-1".to_owned(),
        // As the registry stores Anthropic's base, with its `/v1`.
        base_url: format!("{url}/v1"),
        api_key: "k".to_owned(),
        max_tokens: 1024,
    }
}

/// Vyasa's own loop must not run when Sarathi has a model.
struct Unused;
impl LlmProvider for Unused {
    fn name(&self) -> &'static str {
        "unused"
    }
    fn complete(
        &self,
        _spec: &PromptSpec,
    ) -> Pin<Box<dyn Future<Output = Result<Completion, ProviderError>> + Send>> {
        panic!("the vyasa harness ran");
    }
}

#[derive(Default)]
struct Rows(Mutex<Vec<String>>);
impl AiUsageSink for Rows {
    fn log_success(&self, _p: &str, _m: &str, u: &str, _pt: u32, _ct: u32) {
        self.0.lock().unwrap().push(format!("ok {u}"));
    }
    fn log_failure(&self, _p: &str, _m: &str, u: &str, e: &str) {
        self.0.lock().unwrap().push(format!("failed {u}: {e}"));
    }
}

/// Eyes that render every page.
struct Eyes;
impl StudioEyes for Eyes {
    fn look<'a>(
        &'a mut self,
        _sandbox: &'a DraftState,
        _menus: &'a [vyasa_core::menu::MenuDraft],
        _path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async { Ok("rendered: the title is red".to_owned()) })
    }
    fn content<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async { Ok("no posts".to_owned()) })
    }
}

fn blog() -> DraftState {
    let tokens =
        serde_json::from_str(include_str!("../../../themes-starter/blog/tokens.json")).unwrap();
    let layout =
        serde_json::from_str(include_str!("../../../themes-starter/blog/layout.json")).unwrap();
    DraftState::from_json(&tokens, &layout, None).unwrap()
}

fn spec(max_steps: usize) -> AgentSpec<'static> {
    AgentSpec {
        purpose: "theme-studio",
        model: "m-1",
        system: "You edit themes.",
        task: "Make the title red.",
        max_steps,
        attachments: Vec::new(),
    }
}

async fn run_on(
    answers: Vec<String>,
    toolbox: &mut dyn Toolbox,
    max_steps: usize,
) -> (AgentOutcome, Vec<String>, Vec<String>) {
    let (url, bodies) = serve(answers).await;
    let sink = Rows::default();
    let out = harness::run(
        Harness::Sarathi,
        &Unused,
        Some(&model(&url)),
        &sink,
        &spec(max_steps),
        toolbox,
    )
    .await
    .expect("the run ends");
    let bodies = bodies.lock().unwrap().clone();
    let rows = sink.0.lock().unwrap().clone();
    (out, bodies, rows)
}

async fn studio_run(answers: Vec<String>, max_steps: usize) -> (AgentOutcome, Vec<String>) {
    let state = blog();
    let mut tools = StudioToolbox::new(
        &state,
        vyasa_themes::builtin_registry(),
        Vec::new(),
        Some(Eyes),
    );
    let (out, bodies, _) = run_on(answers, &mut tools, max_steps).await;
    (out, bodies)
}

/// The tool definitions of one request body.
fn tools_of(body: &str) -> Vec<Value> {
    let v: Value = serde_json::from_str(body).unwrap();
    v["tools"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn the_studio_gate_takes_its_evidence_kind_from_look() {
    let (out, bodies) = studio_run(
        vec![
            answer(None, &[edit("c1")]),
            answer(None, &[plan("c2"), look("c3")]),
            finish("Red now."),
        ],
        12,
    )
    .await;
    assert_eq!(out.reply, "Red now.");
    assert!(!out.ran_out);
    let tools = tools_of(&bodies[0]);
    let plan_tool = tools
        .iter()
        .find(|t| t["name"] == "plan")
        .expect("the gate's plan tool is offered from the first call");
    let description = plan_tool["description"].as_str().unwrap();
    assert!(description.contains("(kinds: look)"), "{description}");
    // The studio's own tools are offered beside it.
    for name in ["edit", "look"] {
        assert!(tools.iter().any(|t| t["name"] == name), "{name} missing");
    }
}

#[tokio::test]
async fn a_finish_without_a_plan_is_sent_back() {
    let (out, bodies) = studio_run(
        vec![
            answer(None, &[edit("c1")]),
            finish("Done."),
            answer(None, &[plan("c2"), look("c3")]),
            finish("Checked, done."),
        ],
        12,
    )
    .await;
    assert_eq!(out.reply, "Checked, done.");
    assert!(!out.ran_out);
    assert_eq!(bodies.len(), 4);
    let refusal = "Completion refused: state a plan first";
    assert!(!bodies[1].contains(refusal));
    assert!(bodies[2].contains(refusal), "{}", bodies[2]);
}

#[tokio::test]
async fn evidence_from_before_the_last_edit_does_not_count() {
    let (out, bodies) = studio_run(
        vec![
            answer(None, &[plan("c1"), edit("c2"), look("c3")]),
            // A second edit after the look: the look is stale now.
            answer(None, &[edit("c4")]),
            finish("Done."),
            answer(None, &[look("c5")]),
            finish("Looked again, done."),
        ],
        12,
    )
    .await;
    assert_eq!(out.reply, "Looked again, done.");
    assert!(!out.ran_out);
    assert_eq!(bodies.len(), 5);
    assert!(
        bodies[3].contains("evidence is older than your last edit"),
        "{}",
        bodies[3]
    );
    let tools: Vec<_> = out.steps.iter().map(|s| s.tool.as_str()).collect();
    assert_eq!(tools, ["plan", "edit", "look", "edit", "look"]);
}

#[tokio::test]
async fn evidence_after_the_last_edit_is_accepted_at_once() {
    let (out, bodies) = studio_run(
        vec![
            answer(None, &[plan("c1"), edit("c2"), look("c3")]),
            finish("Done."),
        ],
        12,
    )
    .await;
    assert_eq!(out.reply, "Done.");
    assert!(!out.ran_out);
    assert_eq!(bodies.len(), 2);
    assert!(!bodies[1].contains("Completion refused"));
}

#[tokio::test]
async fn a_finish_refused_after_three_refusals_ends_the_run_as_out_of_steps() {
    let mut answers = vec![answer(None, &[edit("c1")])];
    answers.extend((0..4).map(|i| finish(&format!("Done {i}."))));
    let (out, bodies) = studio_run(answers, 12).await;
    assert!(out.ran_out, "{out:?}");
    // The last refused claim is not passed on as the answer.
    assert_eq!(out.reply, harness::sarathi::RAN_OUT_REPLY);
    // The edit, then four finishes: three refusals go back to the model, the
    // fourth ends the run (Sarathi's MAX_COMPLETION_REFUSALS is 3).
    assert_eq!(bodies.len(), 5);
    assert!(!bodies[1].contains("Completion refused"));
    assert!(bodies[2..].iter().all(|b| b.contains("Completion refused")));
}

/// A surface with `look` that also offers its own `plan` tool.
struct OwnPlan {
    planned: usize,
}
impl Toolbox for OwnPlan {
    fn tools(&self) -> Vec<ToolDef> {
        ["plan", "edit", "look"]
            .into_iter()
            .map(|name| ToolDef {
                name,
                description: "the surface's own tool",
                input_schema: json!({"type": "object"}),
            })
            .collect()
    }
    fn call<'a>(
        &'a mut self,
        name: &str,
        _input: &Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        if name == "plan" {
            self.planned += 1;
        }
        let out = Ok(format!("{name} ran"));
        Box::pin(std::future::ready(out))
    }
}

#[tokio::test]
async fn a_surface_with_its_own_plan_tool_runs_ungated() {
    let mut tools = OwnPlan { planned: 0 };
    let (out, bodies, rows) = run_on(
        vec![
            answer(
                None,
                &[
                    ("c1", "plan", json!({"steps": 2})),
                    ("c2", "edit", json!({})),
                ],
            ),
            finish("Done."),
        ],
        &mut tools,
        12,
    )
    .await;
    // Accepted with no look after the edit: there is no gate.
    assert_eq!(out.reply, "Done.");
    assert!(!out.ran_out);
    assert_eq!(bodies.len(), 2);
    assert!(!bodies[1].contains("Completion refused"));
    // The surface's `plan` ran, not the gate's.
    assert_eq!(tools.planned, 1);
    assert_eq!(out.steps[0].observation, "plan ran");
    let offered = tools_of(&bodies[0]);
    let plans: Vec<_> = offered.iter().filter(|t| t["name"] == "plan").collect();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0]["description"], "the surface's own tool");
    assert_eq!(rows, ["ok theme-studio", "ok theme-studio"]);
}

/// A surface without `look` has nothing to gather evidence with.
struct Blind;
impl Toolbox for Blind {
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "edit",
            description: "edits",
            input_schema: json!({"type": "object"}),
        }]
    }
    fn call<'a>(
        &'a mut self,
        _name: &str,
        _input: &Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(std::future::ready(Ok("edited".to_owned())))
    }
}

#[tokio::test]
async fn a_surface_without_look_offers_no_plan_and_finishes_freely() {
    let (out, bodies, _) = run_on(
        vec![answer(None, &[("c1", "edit", json!({}))]), finish("Done.")],
        &mut Blind,
        12,
    )
    .await;
    assert_eq!(out.reply, "Done.");
    assert!(!out.ran_out);
    assert!(tools_of(&bodies[0]).iter().all(|t| t["name"] != "plan"));
}
