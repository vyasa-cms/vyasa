//! Theme Studio on the Sarathi harness, through the server: a registered
//! Anthropic model (a local stub of the Messages API), `ai_harness` set
//! over REST, a chat message queued as a job and answered on the draft.
//!
//! This is the path a site takes, not `harness::run` called directly: the
//! option is read per request, the model and its key come from the
//! registry, the job worker runs the agent, `look` renders through the
//! server, the reply and proposal land on the draft, and every model call
//! is a usage row. A finish before a fresh `look` is sent back; switching
//! the option back to `vyasa` runs Vyasa's own loop, without a `plan` tool.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vyasa_db::{models::Role, repo::NewUser, repo::UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

type Script = Arc<Mutex<std::vec::IntoIter<String>>>;

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn json_body(r: ureq::Response) -> Value {
    r.into_json().unwrap()
}

fn login(base: &str, email: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": email, "password": "pw-secret-1"})),
    );
    assert_eq!(resp.status(), 200, "login {email}");
    let token = resp
        .all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .unwrap()
        .to_string();
    format!("vy_session={token}")
}

async fn seed(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 86_001,
            email: "sarathi-admin@example.com",
            username: "sarathiadmin",
            display_name: "Sarathi Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
}

fn get(base: &str, cookie: &str, path: &str) -> ureq::Response {
    http(
        ureq::get(format!("{base}{path}").as_str())
            .set("Cookie", cookie)
            .call(),
    )
}

fn send(method: &str, base: &str, cookie: &str, path: &str, body: Value) -> ureq::Response {
    http(
        ureq::request(method, format!("{base}{path}").as_str())
            .set("Cookie", cookie)
            .send_json(body),
    )
}

/// A Messages API stub: each call gets the next answer of the current
/// script; every request body is kept, in order. `count_tokens` answers
/// without using up the script.
async fn stub() -> (String, Script, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let script: Script = Arc::new(Mutex::new(Vec::new().into_iter()));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let (answers, seen) = (script.clone(), bodies.clone());
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (answers, seen) = (answers.clone(), seen.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 16_384];
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
                        let body = buf[end + 4..end + 4 + len].to_vec();
                        break (path, body);
                    }
                };
                let (status, answer) = if path.ends_with("/count_tokens") {
                    ("200 OK", r#"{"input_tokens":100}"#.to_owned())
                } else {
                    seen.lock()
                        .unwrap()
                        .push(serde_json::from_slice(&body).unwrap_or(Value::Null));
                    match answers.lock().unwrap().next() {
                        Some(a) => ("200 OK", a),
                        // Over-calling shows up as a failed run, not a hang.
                        None => (
                            "400 Bad Request",
                            r#"{"type":"error","error":{"type":"invalid_request_error","message":"unscripted call"}}"#
                                .to_owned(),
                        ),
                    }
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (url, script, bodies)
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
        "id": "msg_01", "type": "message", "role": "assistant", "model": MODEL,
        "content": content, "stop_reason": stop,
        "usage": {"input_tokens": 12, "output_tokens": 5}
    })
    .to_string()
}

const MODEL: &str = "claude-haiku-4-5";

fn plan_call(id: &str) -> (&str, &'static str, Value) {
    (
        id,
        "plan",
        json!({"criteria": [{"id": "red", "text": "the primary colour is red", "kind": "look"}]}),
    )
}

fn red_ops() -> Value {
    json!([{"op": "patch_tokens", "patch": {"colors": {"primary": {"light": "#b91c1c", "dark": "#fca5a5"}}}}])
}

fn edit_call(id: &str) -> (&str, &'static str, Value) {
    (id, "edit", json!({"ops": red_ops()}))
}

fn look_call(id: &str) -> (&str, &'static str, Value) {
    (id, "look", json!({"path": "/"}))
}

/// One step of Vyasa's own loop: its forced `emit_result` with a step.
fn vyasa_step(tool: &str, input: &Value, reply: &str) -> String {
    answer(
        None,
        &[(
            "t1",
            "emit_result",
            json!({"thought": "step", "tool": tool, "input": input, "reply": reply}),
        )],
    )
}

fn load(script: &Script, answers: Vec<String>) {
    *script.lock().unwrap() = answers.into_iter();
}

/// Asks the draft's assistant and waits for the job to settle; returns the
/// draft's final status and its newest message.
fn ask(base: &str, admin: &str, draft: i64, text: &str) -> (String, Value) {
    let queued = send(
        "POST",
        base,
        admin,
        &format!("/api/v1/themes/drafts/{draft}/chat"),
        json!({"message": text}),
    );
    assert!(
        queued.status() < 300,
        "chat refused: {} {}",
        queued.status(),
        queued.into_string().unwrap()
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let d = json_body(get(base, admin, &format!("/api/v1/themes/drafts/{draft}")));
        let status = d["status"].as_str().unwrap_or_default().to_owned();
        if status == "ready" || status == "failed" {
            let messages = json_body(get(
                base,
                admin,
                &format!("/api/v1/themes/drafts/{draft}/messages"),
            ));
            let last = messages.as_array().unwrap().last().cloned().unwrap();
            if last["role"] == "assistant" {
                return (status, last);
            }
        }
        assert!(Instant::now() < deadline, "the run did not settle: {d}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Waits for `want` theme-studio usage rows (the sink writes them from a
/// spawned task); on a timeout, fails with every row there is.
async fn usage_rows(pool: &PgPool, want: i64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM ai_logs WHERE purpose = 'theme-studio'")
                .fetch_one(pool)
                .await
                .unwrap();
        if n == want {
            return;
        }
        if Instant::now() > deadline {
            let rows: Vec<(String, String, String, i32, i32)> = sqlx::query_as(
                "SELECT provider, model, purpose, prompt_tokens, completion_tokens FROM ai_logs",
            )
            .fetch_all(pool)
            .await
            .unwrap();
            panic!("{n} theme-studio usage rows, want {want}; all rows: {rows:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn theme_studio_runs_on_sarathi_through_the_server() {
    let (url, script, bodies) = stub().await;
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SECRET_KEY", "test-secret-for-sealing-keys")
        .start();
    let base = server.base();
    let admin = login(base, "sarathi-admin@example.com");

    // The model comes from the registry: Anthropic's key and base, as an
    // operator enters them, pointed at the stub.
    let saved = send(
        "PUT",
        base,
        &admin,
        "/api/v1/ai/providers/anthropic",
        json!({"api_key": "sk-ant-test-0000", "base_url": format!("{url}/v1")}),
    );
    assert_eq!(saved.status(), 200, "{}", saved.into_string().unwrap());
    let model = send(
        "POST",
        base,
        &admin,
        "/api/v1/ai/models",
        json!({"provider": "anthropic", "kind": "text", "model": MODEL}),
    );
    assert!(model.status() < 300, "{}", model.into_string().unwrap());
    let harness = send(
        "PUT",
        base,
        &admin,
        "/api/v1/options/ai_harness",
        json!("sarathi"),
    );
    assert!(harness.status() < 300, "{}", harness.into_string().unwrap());

    let draft = json_body(send(
        "POST",
        base,
        &admin,
        "/api/v1/themes/drafts",
        json!({}),
    ))["id"]
        .as_i64()
        .unwrap();

    // 1. Plan and edit, then a finish with no look since the edit: sent
    //    back. A look, then the finish is accepted and the edit is offered.
    load(
        &script,
        vec![
            answer(None, &[plan_call("p1"), edit_call("e1")]),
            answer(Some("Done: the primary colour is red."), &[]),
            answer(None, &[look_call("l1")]),
            answer(Some("The primary colour is red now."), &[]),
        ],
    );
    let (status, reply) = ask(base, &admin, draft, "make the primary colour red");
    let calls: Vec<Value> = bodies.lock().unwrap().drain(..).collect();
    assert_eq!(status, "ready", "{reply}");
    assert_eq!(reply["text"], "The primary colour is red now.", "{reply}");
    assert!(
        reply["proposal"].is_object(),
        "the edit is offered: {reply}"
    );
    assert_eq!(calls.len(), 4, "plan+edit, refused finish, look, finish");
    let tools = tool_names(&calls[0]);
    for t in ["plan", "edit", "look"] {
        assert!(tools.iter().any(|n| n == t), "{t} offered: {tools:?}");
    }
    assert_eq!(calls[0]["model"], MODEL, "the registry's model is called");
    // The refusal went back to the model as a tool-less user turn before
    // the look: the third request carries it.
    let refusal = calls[2]["messages"].to_string();
    assert!(
        refusal.contains("look"),
        "the refusal names the missing evidence: {refusal}"
    );
    usage_rows(db.pool(), 4).await; // one per call

    // 2. A model that never looks: its finish is sent back each time, the
    //    fourth refusal ends the run, and no finish is accepted on its word.
    load(
        &script,
        vec![
            answer(None, &[plan_call("p2"), edit_call("e2")]),
            answer(Some("Done."), &[]),
            answer(Some("Done, really."), &[]),
            answer(Some("Done, I promise."), &[]),
            answer(Some("Done, final answer."), &[]),
        ],
    );
    let (status, reply) = ask(base, &admin, draft, "make the primary colour red again");
    let calls: Vec<Value> = bodies.lock().unwrap().drain(..).collect();
    assert_eq!(calls.len(), 5, "plan+edit and four refused finishes");
    // The run stopped, and says so: the last unproven "Done" is not the
    // answer. What the steps applied is still offered, as on Vyasa's loop.
    assert_eq!(status, "ready", "{reply}");
    assert_eq!(
        reply["text"],
        vyasa_ai::harness::sarathi::RAN_OUT_REPLY,
        "an unproven finish was passed on: {reply}"
    );
    usage_rows(db.pool(), 9).await;

    // 3. Back to `vyasa`: Vyasa's own loop answers the same request, with
    //    no `plan` tool and no gate.
    let harness = send(
        "PUT",
        base,
        &admin,
        "/api/v1/options/ai_harness",
        json!("vyasa"),
    );
    assert!(harness.status() < 300);
    load(
        &script,
        vec![
            vyasa_step("edit", &json!({"ops": red_ops()}), ""),
            vyasa_step("done", &json!({}), "Red it is."),
        ],
    );
    let (status, reply) = ask(base, &admin, draft, "make the primary colour red once more");
    let calls: Vec<Value> = bodies.lock().unwrap().drain(..).collect();
    assert_eq!(status, "ready", "{reply}");
    assert_eq!(reply["text"], "Red it is.", "{reply}");
    assert_eq!(calls.len(), 2);
    assert_eq!(tool_names(&calls[0]), ["emit_result"]);
    assert!(!calls[0].to_string().contains("\"plan\""));
    usage_rows(db.pool(), 11).await;
}

/// Vyasa's own loop through the same path: the usage rows a site's
/// billing page reads are written on this harness too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn theme_studio_on_vyasa_writes_usage_rows() {
    let (url, script, bodies) = stub().await;
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SECRET_KEY", "test-secret-for-sealing-keys")
        .start();
    let base = server.base();
    let admin = login(base, "sarathi-admin@example.com");
    let saved = send(
        "PUT",
        base,
        &admin,
        "/api/v1/ai/providers/anthropic",
        json!({"api_key": "sk-ant-test-0000", "base_url": format!("{url}/v1")}),
    );
    assert_eq!(saved.status(), 200);
    let model = send(
        "POST",
        base,
        &admin,
        "/api/v1/ai/models",
        json!({"provider": "anthropic", "kind": "text", "model": MODEL}),
    );
    assert!(model.status() < 300);
    let draft = json_body(send(
        "POST",
        base,
        &admin,
        "/api/v1/themes/drafts",
        json!({}),
    ))["id"]
        .as_i64()
        .unwrap();
    load(
        &script,
        vec![
            vyasa_step("edit", &json!({"ops": red_ops()}), ""),
            vyasa_step("done", &json!({}), "Red it is."),
        ],
    );
    let (status, reply) = ask(base, &admin, draft, "make the primary colour red");
    assert_eq!(status, "ready", "{reply}");
    assert_eq!(bodies.lock().unwrap().len(), 2);
    usage_rows(db.pool(), 2).await;
}
