//! Editing locks: the second person sees the first, and can take over.
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

fn json_body(r: ureq::Response) -> Value {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader()
        .take(1_000_000)
        .read_to_string(&mut s)
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

fn login(base: &str, email: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": email, "password": "pw-secret-1"})),
    );
    assert_eq!(resp.status(), 200);
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
    let users = UsersRepo::new(pool.clone());
    for (id, email, username, name) in [
        (83_001, "ada@example.com", "ada", "Ada"),
        (83_002, "bob@example.com", "bob", "Bob"),
    ] {
        users
            .insert(&NewUser {
                id,
                email,
                username,
                display_name: name,
                password_hash: Some(&hash),
                role: Role::Admin,
                bio: "",
            })
            .await
            .expect("user");
    }
}

#[tokio::test]
async fn the_second_editor_sees_the_first_and_can_take_over() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let ada = login(base, "ada@example.com");
    let bob = login(base, "bob@example.com");
    let created = json_body(http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ada)
            .send_json(json!({"title": "Shared", "content": {"schema_version": 1, "blocks": []}})),
    ));
    let id = created["id"].as_i64().unwrap();
    let lock = |cookie: &str, force: bool| {
        json_body(http(
            ureq::post(
                format!(
                    "{base}/api/v1/posts/{id}/lock{}",
                    if force { "?force=true" } else { "" }
                )
                .as_str(),
            )
            .set("cookie", cookie)
            .call(),
        ))
    };
    assert_eq!(lock(&ada, false)["mine"], true);
    let seen_by_bob = lock(&bob, false);
    assert_eq!(seen_by_bob["mine"], false);
    assert_eq!(seen_by_bob["holder_name"], "Ada");
    assert!(seen_by_bob["seen_ago_secs"].as_i64().unwrap() < 90);
    // Bob takes over; Ada's next heartbeat finds him.
    assert_eq!(lock(&bob, true)["mine"], true);
    assert_eq!(lock(&ada, false)["holder_name"], "Bob");
    // Bob leaves; Ada has it again.
    let gone = http(
        ureq::delete(format!("{base}/api/v1/posts/{id}/lock").as_str())
            .set("cookie", &bob)
            .call(),
    );
    assert_eq!(gone.status(), 204);
    assert_eq!(lock(&ada, false)["mine"], true);
}
