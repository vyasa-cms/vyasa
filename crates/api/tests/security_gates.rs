//! Capability gates on endpoints that used to accept any signed-in user
//! (or any `EditPosts` holder): analytics, AI assist, the media library,
//! forms and their submissions, per-entry SEO tools, and marketplace
//! consent.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Read;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

const ADMIN: i64 = 73_001;
const EDITOR: i64 = 73_002;
const AUTHOR: i64 = 73_003;
const CONTRIBUTOR: i64 = 73_004;
const SUBSCRIBER: i64 = 73_005;

async fn seed(pool: &PgPool) {
    let users = UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").expect("hash");
    for (id, name, role) in [
        (ADMIN, "admin", Role::Admin),
        (EDITOR, "editor", Role::Editor),
        (AUTHOR, "author", Role::Author),
        (CONTRIBUTOR, "contrib", Role::Contributor),
        (SUBSCRIBER, "sub", Role::Subscriber),
    ] {
        let email = format!("{name}@example.com");
        users
            .insert(&NewUser {
                id,
                email: &email,
                username: name,
                display_name: name,
                password_hash: Some(&hash),
                role,
                bio: "",
            })
            .await
            .expect("user");
    }
    // One upload each for the admin and the author.
    for (id, owner) in [(73_101_i64, ADMIN), (73_102, AUTHOR)] {
        sqlx::query(
            "INSERT INTO media (id, owner_id, file_name, mime, byte_size, path)
             VALUES ($1, $2, $3, 'image/png', 10, $3)",
        )
        .bind(id)
        .bind(owner)
        .bind(format!("f{id}.png"))
        .execute(pool)
        .await
        .expect("media");
    }
    // An entry by the admin that the author must not run SEO tools on.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta)
         VALUES (73201, $1, 'post', 'draft', 'admins-draft', 'Admin draft',
                 '{\"schema_version\":1,\"blocks\":[]}', '{}')",
    )
    .bind(ADMIN)
    .execute(pool)
    .await
    .expect("post");
}

fn http(result: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(err)) => panic!("transport failure: {err}"),
    }
}

fn json_body(response: ureq::Response) -> Value {
    let mut body = String::new();
    response
        .into_reader()
        .take(1_000_000)
        .read_to_string(&mut body)
        .expect("read body");
    serde_json::from_str(&body).unwrap_or(Value::String(body))
}

fn login(base: &str, name: &str) -> String {
    let response = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": format!("{name}@example.com"), "password": "pw-secret-1"})),
    );
    assert_eq!(response.status(), 200, "login for {name}");
    response
        .all("set-cookie")
        .iter()
        .filter_map(|c| c.split(';').next())
        .find(|kv| kv.starts_with("vy_session="))
        .expect("session cookie")
        .to_owned()
}

fn get(base: &str, cookie: &str, path: &str) -> ureq::Response {
    http(
        ureq::get(format!("{base}{path}").as_str())
            .set("cookie", cookie)
            .call(),
    )
}

fn post(base: &str, cookie: &str, path: &str, body: &Value) -> ureq::Response {
    http(
        ureq::post(format!("{base}{path}").as_str())
            .set("cookie", cookie)
            .send_json(body.clone()),
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn endpoints_answer_only_to_the_roles_that_own_them() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin = login(base, "admin");
    let editor = login(base, "editor");
    let author = login(base, "author");
    let contrib = login(base, "contrib");
    let sub = login(base, "sub");

    // --- analytics: editor and up ---------------------------------------
    for (who, cookie) in [("sub", &sub), ("author", &author)] {
        let r = get(base, cookie, "/api/v1/analytics/summary");
        assert_eq!(r.status(), 403, "analytics for {who}");
    }
    assert_eq!(
        get(base, &editor, "/api/v1/analytics/summary").status(),
        200
    );

    // --- AI assist spends budget: EditPosts ------------------------------
    let assist = json!({"content": {"schema_version": 1, "blocks": []}});
    let r = post(base, &sub, "/api/v1/ai/assist/title", &assist);
    assert_eq!(r.status(), 403, "assist for a subscriber");

    // --- media library ----------------------------------------------------
    for path in [
        "/api/v1/media",
        "/api/v1/media/stats",
        "/api/v1/media/73101",
        "/api/v1/media/73101/usage",
    ] {
        assert_eq!(get(base, &sub, path).status(), 403, "{path} for sub");
    }
    // The author sees only their own uploads, whatever owner_id asks for.
    let ids = |r: ureq::Response| -> Vec<String> {
        json_body(r)
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].to_string().trim_matches('"').to_owned())
            .collect()
    };
    assert_eq!(ids(get(base, &author, "/api/v1/media")), ["73102"]);
    assert_eq!(
        ids(get(
            base,
            &author,
            &format!("/api/v1/media?owner_id={ADMIN}")
        )),
        ["73102"]
    );
    assert_eq!(
        ids(get(
            base,
            &author,
            &format!("/api/v1/media?owner_id={AUTHOR}")
        )),
        ["73102"]
    );
    let mut all = ids(get(base, &editor, "/api/v1/media"));
    all.sort();
    assert_eq!(all, ["73101", "73102"]);
    assert_eq!(
        ids(get(
            base,
            &editor,
            &format!("/api/v1/media?owner_id={ADMIN}")
        )),
        ["73101"]
    );
    assert_eq!(
        get(base, &author, "/api/v1/media/73101/usage").status(),
        403
    );
    assert_eq!(
        get(base, &author, "/api/v1/media/73102/usage").status(),
        200
    );

    // Stats follow the same scoping as the list.
    let count = |r: ureq::Response| json_body(r)["count"].as_i64().unwrap();
    assert_eq!(count(get(base, &author, "/api/v1/media/stats")), 1);
    assert_eq!(
        count(get(
            base,
            &author,
            &format!("/api/v1/media/stats?owner_id={ADMIN}")
        )),
        1
    );
    assert_eq!(count(get(base, &editor, "/api/v1/media/stats")), 2);
    assert_eq!(
        count(get(
            base,
            &editor,
            &format!("/api/v1/media/stats?owner_id={ADMIN}")
        )),
        1
    );

    // --- forms: editor and up ----------------------------------------------
    let form = json!({"name": "Contact", "fields": [], "notify_email": "x@example.com"});
    for (who, cookie) in [("contrib", &contrib), ("author", &author)] {
        assert_eq!(
            get(base, cookie, "/api/v1/forms").status(),
            403,
            "forms list for {who}"
        );
        assert_eq!(
            post(base, cookie, "/api/v1/forms", &form).status(),
            403,
            "forms create for {who}"
        );
    }
    assert_eq!(get(base, &editor, "/api/v1/forms").status(), 200);

    // --- SEO tools on someone else's entry -----------------------------------
    let r = post(base, &author, "/api/v1/posts/73201/check-links", &json!({}));
    assert_eq!(r.status(), 403, "check-links on another's post");
    assert_eq!(
        get(base, &author, "/api/v1/posts/73201/seo-signals").status(),
        403
    );
    assert_eq!(
        get(base, &editor, "/api/v1/posts/73201/seo-signals").status(),
        200
    );

    // --- marketplace: a plugin install must state consent ---------------------
    let r = post(
        base,
        &admin,
        "/api/v1/registry/install",
        &json!({"kind": "plugin", "name": "anything"}),
    );
    assert_eq!(r.status(), 400);
    let body = json_body(r).to_string();
    assert!(body.contains("accept_capabilities"), "{body}");
}
