//! A page composing itself, end to end: written through the API, served to
//! a visitor.
//!
//! The unit tests pin the renderer's seam; this pins the whole chain — the
//! REST body, validation against the active theme, storage, revisions, and
//! the HTML a browser receives.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::{models::Role, repo::NewUser, repo::UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn read(r: ureq::Response) -> String {
    use std::io::Read as _;
    let mut s = String::new();
    r.into_reader()
        .take(4_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn json_of(r: ureq::Response) -> Value {
    serde_json::from_str(&read(r)).unwrap()
}

fn login(base: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "compose@example.com", "password": "pw-secret-1"})),
    );
    assert_eq!(resp.status(), 200, "login");
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
            id: 87_001,
            email: "compose@example.com",
            username: "composer",
            display_name: "Ada Lovelace",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
}

/// Hero, the author's own words, then a dark call-to-action band.
fn campaign() -> Value {
    json!([
        {
            "id": "lead",
            "kind": "hero",
            "settings": {
                "headline": "Ship your first site today",
                "subhead": "No template wrangling."
            }
        },
        { "id": "words", "kind": "content" },
        {
            "id": "closing",
            "kind": "band",
            "children": [{
                "id": "signup",
                "kind": "cta-band",
                "scope": { "bg": "$text", "text": "$bg" },
                "settings": { "headline": "Ready?", "label": "Start free", "url": "/signup" }
            }]
        }
    ])
}

fn create_page(base: &str, cookie: &str, layout: &Value) -> Value {
    json_of(http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("Cookie", cookie)
            .send_json(json!({
                "type": "page",
                "status": "published",
                "title": "Launch",
                "slug": "launch",
                "content": {
                    "schema_version": 1,
                    "blocks": [{"kind": "paragraph", "attrs": {"text": "Written by hand."}}]
                },
                "layout": layout,
            })),
    ))
}

#[tokio::test]
async fn a_composed_page_reaches_the_visitor() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    let created = create_page(base, &cookie, &campaign());
    assert_eq!(
        created["layout"].as_array().map(Vec::len),
        Some(3),
        "the tree came back on the response: {created}"
    );

    let html = read(http(ureq::get(format!("{base}/launch").as_str()).call()));
    // Order is asserted inside <main>: the excerpt also appears in the head
    // as an `og:description`, which would otherwise read as "prose first".
    let body = html
        .split_once("<main")
        .map(|(_, rest)| rest)
        .expect("main element");

    assert!(
        body.contains("Ship your first site today"),
        "hero rendered:\n{html}"
    );
    let hero = body.find("Ship your first site").expect("hero");
    let words = body.find("Written by hand.").expect("the author's prose");
    let cta = body.find("Start free").expect("call to action");
    assert!(
        hero < words && words < cta,
        "prose sits where the tree put it:\n{html}"
    );
    // The scope compiled against the live theme's palette rather than being
    // dropped on the floor.
    assert!(
        html.contains(".vy-scope-signup"),
        "the scope compiled to a rule, not just a class:\n{html}"
    );
}

#[tokio::test]
async fn clearing_the_tree_puts_the_page_back_on_its_template() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    let created = create_page(base, &cookie, &campaign());
    // Snowflake ids are i64; serde_json may hand them back either way.
    let id = created["id"].as_str().map_or_else(
        || created["id"].as_i64().expect("id present").to_string(),
        str::to_owned,
    );

    let cleared = json_of(http(
        ureq::put(format!("{base}/api/v1/posts/{id}").as_str())
            .set("Cookie", &cookie)
            .send_json(json!({"layout": []})),
    ));
    assert_eq!(cleared["layout"].as_array().map(Vec::len), Some(0));

    let html = read(http(ureq::get(format!("{base}/launch").as_str()).call()));
    let body = html
        .split_once("<main")
        .map(|(_, rest)| rest)
        .expect("main element");
    assert!(
        !body.contains("Ship your first site today"),
        "the hero is gone:\n{html}"
    );
    assert!(
        body.contains("Written by hand."),
        "the writing survives the layout being cleared:\n{html}"
    );
    assert!(
        body.contains("vy-page"),
        "and the theme's page template is laying it out again:\n{html}"
    );
}

#[tokio::test]
async fn a_tree_the_theme_cannot_render_is_refused() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);

    for (name, layout) in [
        (
            "unknown kind",
            json!([{"id": "x", "kind": "no-such-section"}]),
        ),
        (
            "children on a leaf",
            json!([{"id": "x", "kind": "hero", "children": [{"id": "y", "kind": "cta-band"}]}]),
        ),
        (
            "duplicate ids",
            json!([{"id": "x", "kind": "cta-band"}, {"id": "x", "kind": "cta-band"}]),
        ),
        (
            "a scope role that does not exist",
            json!([{"id": "x", "kind": "cta-band", "scope": {"bg": "$nonesuch"}}]),
        ),
    ] {
        let resp = http(
            ureq::post(format!("{base}/api/v1/posts").as_str())
                .set("Cookie", &cookie)
                .send_json(json!({
                    "type": "page",
                    "title": "Bad",
                    "content": {"schema_version": 1, "blocks": []},
                    "layout": layout,
                })),
        );
        assert_eq!(resp.status(), 400, "{name} should be refused");
        let body = read(resp);
        assert!(
            body.contains("layout["),
            "{name}: the error points at the field: {body}"
        );
    }
}

#[tokio::test]
async fn a_page_can_be_seen_before_it_is_saved() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    // Saved with no sections at all: what comes back must be the tree in
    // the request, not the one in the database.
    let created = create_page(base, &cookie, &json!([]));
    let id = created["id"].as_str().map_or_else(
        || created["id"].as_i64().expect("id present").to_string(),
        str::to_owned,
    );

    let html = read(http(
        ureq::post(format!("{base}/api/v1/posts/{id}/render").as_str())
            .set("Cookie", &cookie)
            .send_json(json!({
                "sections": campaign(),
                "content": {
                    "schema_version": 1,
                    "blocks": [{"kind": "paragraph", "attrs": {"text": "Not saved yet."}}]
                },
            })),
    ));

    let body = html
        .split_once("<main")
        .map(|(_, rest)| rest)
        .expect("main element");
    assert!(
        body.contains("Ship your first site today"),
        "the unsaved hero rendered:\n{html}"
    );
    assert!(
        body.contains("Not saved yet."),
        "the unsaved writing rendered:\n{html}"
    );

    // And the database is untouched: previewing is not saving.
    let after = json_of(http(
        ureq::get(format!("{base}/api/v1/posts/{id}").as_str())
            .set("Cookie", &cookie)
            .call(),
    ));
    assert_eq!(after["layout"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn rendering_refuses_a_tree_the_theme_cannot_render() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    let created = create_page(base, &cookie, &json!([]));
    let id = created["id"].as_str().map_or_else(
        || created["id"].as_i64().expect("id present").to_string(),
        str::to_owned,
    );

    // The editor is a client like any other. Without validation here an
    // unknown kind would fail deep in the renderer, with a message about
    // templates rather than about the section.
    let resp = http(
        ureq::post(format!("{base}/api/v1/posts/{id}/render").as_str())
            .set("Cookie", &cookie)
            .send_json(json!({"sections": [{"id": "x", "kind": "no-such-section"}]})),
    );
    assert_eq!(resp.status(), 400);
    assert!(read(resp).contains("layout[0].kind"));
}

#[tokio::test]
async fn only_pages_compose() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);

    // A composed body replaces the whole template body, and `single` is
    // where the byline, date and taxonomy links live — none of them section
    // kinds yet, so a composed post would lose them irrecoverably.
    let resp = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("Cookie", &cookie)
            .send_json(json!({
                "type": "post",
                "title": "An entry",
                "content": {"schema_version": 1, "blocks": []},
                "layout": campaign(),
            })),
    );
    assert_eq!(resp.status(), 400);
    assert!(read(resp).contains("only pages compose"));

    // An empty tree is not composing, so it is fine on anything.
    let ok = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("Cookie", &cookie)
            .send_json(json!({
                "type": "post",
                "title": "An entry",
                "content": {"schema_version": 1, "blocks": []},
                "layout": [],
            })),
    );
    assert_eq!(ok.status(), 201);
}
