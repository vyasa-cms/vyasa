//! The studio agent with a scripted model: its edits pass the same
//! validation as a person's, invalid ops come back as observations it
//! reads and repairs, and a question changes nothing.
//!
//! These began life as the single-shot assistant's tests; the contract
//! they pin — same-ops validation, diagnostics-driven repair, no silent
//! writes — survived the move to the agent loop unchanged.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use vyasa_ai::agent::{run_agent, AgentSpec};
use vyasa_ai::theme_studio::{system_prompt, user_prompt, AssistInput, StudioEyes, StudioToolbox};
use vyasa_ai::{AiUsageSink, Completion, LlmProvider, PromptSpec, Usage};
use vyasa_themes::DraftState;

struct Scripted {
    replies: Mutex<Vec<String>>,
    seen: Mutex<Vec<PromptSpec>>,
}

impl Scripted {
    fn new(replies: &[&str]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().rev().map(|s| (*s).to_owned()).collect()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl LlmProvider for Scripted {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn complete(
        &self,
        spec: &PromptSpec,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Completion, vyasa_ai::ProviderError>> + Send>,
    > {
        self.seen.lock().unwrap().push(spec.clone());
        let text = self.replies.lock().unwrap().pop().unwrap_or_default();
        Box::pin(std::future::ready(Ok(Completion {
            text,
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 10,
            },
            provider: None,
            model: None,
        })))
    }
}

struct NullSink;
impl AiUsageSink for NullSink {
    fn log_success(&self, _p: &str, _m: &str, _u: &str, _pt: u32, _ct: u32) {}
    fn log_failure(&self, _p: &str, _m: &str, _u: &str, _e: &str) {}
}

/// Blind toolbox: the pure half only, which is all these tests need.
struct NoEyes;
impl StudioEyes for NoEyes {
    fn look<'a>(
        &'a mut self,
        _sandbox: &'a DraftState,
        _menus: &'a [vyasa_core::menu::MenuDraft],
        _path: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async { Err("no eyes in tests".to_owned()) })
    }
    fn content<'a>(
        &'a mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async { Err("no eyes in tests".to_owned()) })
    }
}

fn blog() -> DraftState {
    let tokens =
        serde_json::from_str(include_str!("../../../themes-starter/blog/tokens.json")).unwrap();
    let layout =
        serde_json::from_str(include_str!("../../../themes-starter/blog/layout.json")).unwrap();
    DraftState::from_json(&tokens, &layout, None).unwrap()
}

fn toolbox(state: &DraftState) -> StudioToolbox<NoEyes> {
    StudioToolbox::new(state, vyasa_themes::builtin_registry(), Vec::new(), None)
}

async fn run(
    provider: &Scripted,
    state: &DraftState,
    message: &str,
) -> (vyasa_ai::agent::AgentOutcome, StudioToolbox<NoEyes>) {
    let vocab = vyasa_themes::registry_schema();
    let input = AssistInput {
        vocabulary: &vocab,
        site: "Vyasa — a fresh CMS in Rust",
        brand: "",
        history: &[],
        message,
        attachments: Vec::new(),
        model: "scripted-1",
    };
    let system = system_prompt(&vocab);
    let task = user_prompt(state, &input);
    let mut tools = toolbox(state);
    let out = run_agent(
        provider,
        &NullSink,
        &AgentSpec {
            purpose: "theme-studio",
            model: "scripted-1",
            system: &system,
            task: &task,
            max_steps: 6,
            attachments: Vec::new(),
        },
        &mut tools,
    )
    .await
    .expect("agent runs");
    (out, tools)
}

#[tokio::test]
async fn valid_ops_become_a_new_state_with_a_reply() {
    let provider = Scripted::new(&[
        r##"{"thought": "one edit does it", "tool": "edit", "input": {"ops": [
          {"op": "patch_tokens", "patch": {"colors": {"primary": {"light": "#1d4ed8", "dark": "#93c5fd"}}}},
          {"op": "set_layout", "template": "index", "blocks": [
            {"id": "header", "kind": "header"},
            {"id": "search", "kind": "search-box", "settings": {"placeholder": "Find a post"}},
            {"id": "posts", "kind": "latest-posts", "settings": {"count": 6}},
            {"id": "footer", "kind": "footer"}
          ]}
        ]}}"##,
        r#"{"thought": "done", "tool": "done", "input": {}, "reply": "Made the accent a deep blue and gave the home page a search box."}"#,
    ]);
    let state = blog();
    let (out, tools) = run(&provider, &state, "Blue accent, add search").await;

    assert_eq!(tools.sandbox.tokens.colors.primary.light.0, "#1d4ed8");
    assert_eq!(tools.sandbox.layout.index.len(), 4);
    assert_eq!(
        tools.sandbox.layout.single, state.layout.single,
        "untouched template kept"
    );
    assert_eq!(tools.changes.len(), 2);
    assert!(out.reply.starts_with("Made the accent"));
    assert_eq!(out.steps.len(), 1);
    assert!(out.steps[0].observation.contains("Applied"));

    // The model saw the current documents, the vocabulary, and the tools.
    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].purpose, "theme-studio");
    assert!(seen[0].system.contains("- search-box — "));
    assert!(seen[0].system.contains("TOOLS"), "protocol appended");
    assert!(
        seen[0].user.contains(
            blog().tokens_json()["colors"]["primary"]["light"]
                .as_str()
                .unwrap()
        ),
        "current primary colour shown"
    );
    assert!(seen[0].user.contains("Request:\nBlue accent, add search"));
}

#[tokio::test]
async fn invalid_ops_come_back_as_diagnostics_the_model_repairs() {
    let provider = Scripted::new(&[
        r#"{"thought": "try", "tool": "edit", "input": {"ops": [{"op": "set_layout", "template": "index", "blocks": [{"id": "c", "kind": "carousel"}]}]}}"#,
        r##"{"thought": "repair", "tool": "edit", "input": {"ops": [{"op": "set_layout", "template": "index", "blocks": [{"id": "posts", "kind": "latest-posts"}]}]}}"##,
        r#"{"thought": "good", "tool": "done", "input": {}, "reply": "Used a supported block instead."}"#,
    ]);
    let state = blog();
    let (out, tools) = run(&provider, &state, "add a carousel").await;

    assert_eq!(tools.sandbox.layout.index[0].kind, "latest-posts");
    // The refusal reached the model verbatim enough to act on.
    assert!(out.steps[0].observation.contains("refused"));
    assert!(out.steps[0]
        .observation
        .contains("unknown kind \"carousel\""));
    // And the transcript carried it into the next step's prompt.
    let seen = provider.seen.lock().unwrap();
    assert!(seen[1].user.contains("unknown kind \"carousel\""));
}

#[tokio::test]
async fn a_question_changes_nothing() {
    let provider = Scripted::new(&[
        r#"{"thought": "no change needed", "tool": "done", "input": {}, "reply": "The body font is the system UI stack."}"#,
    ]);
    let state = blog();
    let (out, tools) = run(&provider, &state, "what font is the body?").await;

    assert!(tools.changes.is_empty());
    assert_eq!(tools.sandbox, state);
    assert_eq!(out.reply, "The body font is the system UI stack.");
    assert!(out.steps.is_empty());
}

#[tokio::test]
async fn css_and_unknown_ops_never_get_through() {
    // A model that keeps handing back CSS gets the same "no such op"
    // observation each time and ends at the step cap with the sandbox
    // untouched — refusals are observations, not writes.
    let bad = r#"{"thought": "css", "tool": "edit", "input": {"ops": [{"op": "write_css", "css": "body{}"}]}}"#;
    let provider = Scripted::new(&[bad, bad, bad, bad, bad, bad]);
    let state = blog();
    let (out, tools) = run(&provider, &state, "add css").await;

    assert!(out.ran_out, "never finished cleanly");
    assert!(tools.changes.is_empty());
    assert_eq!(tools.sandbox, state, "nothing landed");
    assert!(out.steps[0].observation.contains("write_css"));
    assert!(out.steps[0].observation.contains("ops[0]"));
}

#[tokio::test]
async fn a_staged_menu_lands_in_the_toolbox_not_the_database() {
    let provider = Scripted::new(&[
        r#"{"thought": "what exists?", "tool": "menus", "input": {}}"#,
        r#"{"thought": "stage nav", "tool": "edit_menu", "input": {"slug": "main", "items": [
          {"label": "Home", "url": "/"},
          {"label": "Docs", "url": "/docs", "children": [{"label": "API", "url": "/api-docs"}]}
        ]}}"#,
        r#"{"thought": "done", "tool": "done", "input": {}, "reply": "Built the main menu."}"#,
    ]);
    let state = blog();
    let (out, tools) = run(&provider, &state, "give the site a nav").await;

    // The empty site said so, the staged menu is in the sandbox, and the
    // change is described for the proposal card.
    assert!(out.steps[0].observation.contains("No menus exist yet"));
    assert!(out.steps[1]
        .observation
        .contains("Staged: created menu \"main\" with 3 links"));
    assert_eq!(tools.menus.len(), 1);
    assert_eq!(tools.menus[0].name, "Main", "name derived from the slug");
    assert_eq!(tools.menus[0].items[1].children[0].label, "API");
    assert!(tools.changed_menus.contains("main"));
    assert!(tools.changes.iter().any(|c| c.contains("menu \"main\"")));
    // The theme documents were never touched — menus are site data.
    assert_eq!(tools.sandbox, state);
}

#[tokio::test]
async fn menu_rules_come_back_as_observations() {
    let provider = Scripted::new(&[
        r#"{"thought": "try", "tool": "edit_menu", "input": {"slug": "main", "items": [{"label": "Evil", "url": "javascript:alert(1)"}]}}"#,
        r#"{"thought": "give up", "tool": "done", "input": {}, "reply": "That URL is not allowed."}"#,
    ]);
    let state = blog();
    let (out, tools) = run(&provider, &state, "add a link").await;

    assert!(out.steps[0].observation.contains("edit_menu refused"));
    assert!(out.steps[0].observation.contains("javascript:alert(1)"));
    assert!(tools.menus.is_empty(), "nothing staged");
    assert!(tools.changes.is_empty(), "no proposal from a refusal");
}

#[tokio::test]
async fn a_layout_pointing_at_a_ghost_menu_gets_a_note() {
    let provider = Scripted::new(&[
        r#"{"thought": "add nav", "tool": "edit", "input": {"ops": [{"op": "set_layout", "template": "index", "blocks": [
          {"id": "nav", "kind": "menu", "settings": {"slug": "primary"}},
          {"id": "posts", "kind": "latest-posts"}
        ]}]}}"#,
        r#"{"thought": "oh, stage it", "tool": "edit_menu", "input": {"slug": "primary", "items": [{"label": "Home", "url": "/"}]}}"#,
        r#"{"thought": "done", "tool": "done", "input": {}, "reply": "Nav placed and menu staged."}"#,
    ]);
    let state = blog();
    let (out, _tools) = run(&provider, &state, "add a nav").await;

    // The edit that references a menu nobody has warns (the starter's
    // other templates render "main", so that ghost is named too); once
    // "primary" is staged it drops out of the note.
    let first = &out.steps[0].observation;
    assert!(first.contains("no such menu exists"), "{first}");
    assert!(first.contains("primary"), "{first}");
    let second = &out.steps[1].observation;
    assert!(!second.contains("primary but"), "{second}");
    assert!(
        second.contains("main"),
        "the other templates' ghost still noted: {second}"
    );
}

#[tokio::test]
async fn inspect_reads_the_sandbox_not_the_original() {
    let provider = Scripted::new(&[
        r##"{"thought": "recolor", "tool": "edit", "input": {"ops": [{"op": "patch_tokens", "patch": {"colors": {"primary": {"light": "#123456"}}}}]}}"##,
        r#"{"thought": "check", "tool": "inspect", "input": {"what": "tokens"}}"#,
        r#"{"thought": "confirmed", "tool": "done", "input": {}, "reply": "Recoloured."}"#,
    ]);
    let state = blog();
    let (out, _tools) = run(&provider, &state, "new accent").await;

    assert!(
        out.steps[1].observation.contains("#123456"),
        "inspect sees the edit that just happened:\n{}",
        out.steps[1].observation
    );
}

#[tokio::test]
async fn assistant_inspects_and_edits_assets_without_losing_the_other_half() {
    use vyasa_ai::agent::Toolbox;
    let mut state = blog();
    state.assets.css = ".vy { letter-spacing: -.01em; }".into();
    state.assets.js = "window.original=true;".into();
    let mut tools = toolbox(&state);
    let inspected = tools
        .call("inspect", &serde_json::json!({"what":"assets"}))
        .await
        .unwrap();
    assert!(inspected.contains("window.original=true"));
    let changed = tools
        .call(
            "edit",
            &serde_json::json!({"ops":[{
                "op":"set_assets", "css":".vy-hero { padding: 4rem 0; }", "js":state.assets.js
            }]}),
        )
        .await;
    assert!(changed.is_ok(), "{changed:?}");
    assert!(tools.sandbox.assets.css.contains("4rem"));
    assert_eq!(tools.sandbox.assets.js, "window.original=true;");
}

#[tokio::test]
async fn large_assets_can_be_read_without_truncating_the_source() {
    use vyasa_ai::agent::Toolbox;
    let mut state = blog();
    state.assets.css = "/* café */\n".repeat(700);
    let mut tools = toolbox(&state);
    let mut offset = 0;
    let mut restored = String::new();
    loop {
        let response = tools
            .call(
                "inspect",
                &serde_json::json!({"what": "assets", "asset": "css", "offset": offset}),
            )
            .await
            .unwrap();
        assert!(
            response.chars().count() < 6000,
            "fits the agent observation limit"
        );
        let chunk: serde_json::Value = serde_json::from_str(&response).unwrap();
        restored.push_str(chunk["text"].as_str().unwrap());
        let Some(next) = chunk["next_offset"].as_u64() else {
            break;
        };
        offset = next;
    }
    assert_eq!(restored, state.assets.css);
}
