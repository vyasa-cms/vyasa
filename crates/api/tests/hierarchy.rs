//! Hierarchical post types over HTTP: REST, GraphQL, public addresses, search
//! scopes, managed entries. Tasks 3 to 6, 10, 12 and 17 add tests here.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    dead_code
)]

mod common;

use serde_json::{json, Value};
use vyasa_db::models::Role;
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn text(r: ureq::Response) -> String {
    use std::io::Read as _;
    let mut s = String::new();
    r.into_reader()
        .take(8_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn json_of(r: ureq::Response) -> Value {
    let body = text(r);
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
}

/// A request with a session cookie and a JSON body; returns status and body.
fn call(method: &str, base: &str, cookie: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let req = ureq::request(method, &format!("{base}{path}")).set("cookie", cookie);
    let resp = http(match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    });
    let status = resp.status();
    let body = text(resp);
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body).unwrap_or(Value::String(body))
    };
    (status, value)
}

fn get_public(base: &str, path: &str) -> (u16, String) {
    let resp = http(ureq::get(&format!("{base}{path}")).call());
    (resp.status(), text(resp))
}

/// An id as text, whether the response carries it as a string or a number
/// (post responses have always carried numbers; the tree surfaces use strings).
fn sid(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn doc(words: &str) -> Value {
    json!({"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": words}}]})
}

/// Creates a type and asserts it was created.
fn create_type(base: &str, cookie: &str, slug: &str, singular: &str, plural: &str) {
    let (status, body) = call(
        "POST",
        base,
        cookie,
        "/api/v1/content-types",
        Some(json!({"slug": slug, "singular": singular, "plural": plural})),
    );
    assert_eq!(status, 201, "create type {slug}: {body}");
}

#[tokio::test]
async fn an_administrator_can_make_a_type_hierarchical_and_back() {
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = common::login_cookie(base, &admin.email, &admin.password);
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/content-types",
        Some(
            json!({"slug": "guide", "singular": "Guide", "plural": "Guides", "hierarchical": true}),
        ),
    );
    assert_eq!(status, 201, "{body}");
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    let guide = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["slug"] == "guide")
        .expect("listed");
    assert_eq!(guide["hierarchical"], true);
    assert_eq!(guide["base"], "guide");
    // A post type that is not hierarchical stays flat.
    create_type(base, &a, "note", "Note", "Notes");
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["slug"] == "note")
            .unwrap()["hierarchical"],
        false
    );
    // Turning it off with nested entries is refused; flat entries are fine.
    let (_, parent) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "guide", "title": "Parent", "status": "published", "content": doc("p")}),
        ),
    );
    let pid = sid(&parent["id"]);
    let (status, child) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "guide", "title": "Child", "status": "published", "parent_id": pid, "content": doc("c")}),
        ),
    );
    assert_eq!(status, 201, "{child}");
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/guide",
        Some(json!({"hierarchical": false})),
    );
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("parents"),
        "{body}"
    );
}
