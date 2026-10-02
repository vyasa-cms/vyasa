//! End-to-end RBAC tests: role-based access over the users REST API
//! against the real binary.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Read;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

/// Seeds an admin, an author, and a subscriber; returns their ids.
async fn seed(pool: &PgPool) -> (i64, i64, i64) {
    let users = UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").expect("hash");

    /// Builds a [`NewUser`] row with a shared password hash.
    #[allow(clippy::items_after_statements)]
    fn mk<'a>(
        id: i64,
        email: &'a str,
        username: &'a str,
        role: Role,
        hash: &'a str,
    ) -> NewUser<'a> {
        NewUser {
            id,
            email,
            username,
            display_name: username,
            password_hash: Some(hash),
            role,
            bio: "",
        }
    }
    users
        .insert(&mk(
            71_001,
            "admin@example.com",
            "admin",
            Role::Admin,
            &hash,
        ))
        .await
        .expect("admin");
    users
        .insert(&mk(
            71_002,
            "author@example.com",
            "author",
            Role::Author,
            &hash,
        ))
        .await
        .expect("author");
    users
        .insert(&mk(
            71_003,
            "sub@example.com",
            "sub",
            Role::Subscriber,
            &hash,
        ))
        .await
        .expect("sub");
    (71_001, 71_002, 71_003)
}

fn login_token(base: &str, email: &str) -> String {
    let response = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": email, "password": "pw-secret-1"})),
    );
    assert_eq!(response.status(), 200, "login for {email}");
    response
        .all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .expect("cookie value")
        .to_string()
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
    serde_json::from_str(&body).expect("parse json body")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn rbac_enforcement_matrix() {
    let db = TestDb::new().await;
    let (admin_id, author_id, sub_id) = seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    let admin = login_token(base, "admin@example.com");
    let author = login_token(base, "author@example.com");
    let sub = login_token(base, "sub@example.com");
    let authed = |token: &str| format!("vy_session={token}");

    // --- caps endpoint reflects role -----------------------------------
    let caps = json_body(http(
        ureq::get(format!("{base}/api/v1/auth/me/caps").as_str())
            .set("cookie", &authed(&sub))
            .call(),
    ));
    assert_eq!(caps, json!(["view_admin"]), "subscriber caps: {caps}");
    let caps = json_body(http(
        ureq::get(format!("{base}/api/v1/auth/me/caps").as_str())
            .set("cookie", &authed(&author))
            .call(),
    ));
    assert!(caps
        .as_array()
        .is_some_and(|c| c.contains(&json!("upload_media"))));

    // --- users list: admin yes, author no, subscriber no ---------------
    assert_eq!(
        http(
            ureq::get(format!("{base}/api/v1/users").as_str())
                .set("cookie", &authed(&admin))
                .call()
        )
        .status(),
        200
    );
    for token in [&author, &sub] {
        let response = http(
            ureq::get(format!("{base}/api/v1/users").as_str())
                .set("cookie", &authed(token))
                .call(),
        );
        assert_eq!(response.status(), 403, "list must be forbidden");
        assert_eq!(json_body(response)["code"], "forbidden");
    }

    // --- create user: admin yes, subscriber no --------------------------
    let created = http(
        ureq::post(format!("{base}/api/v1/users").as_str())
            .set("cookie", &authed(&admin))
            .send_json(json!({
                "email": "new-user@example.com",
                "password": "a-long-password",
                "role": "contributor"
            })),
    );
    assert_eq!(created.status(), 201);
    let new_user = json_body(created);
    assert_eq!(new_user["role"], "contributor");
    assert_eq!(new_user["username"], "new-user");

    let forbidden = http(
        ureq::post(format!("{base}/api/v1/users").as_str())
            .set("cookie", &authed(&sub))
            .send_json(json!({
                "email": "hacker@example.com",
                "password": "a-long-password",
                "role": "admin"
            })),
    );
    assert_eq!(forbidden.status(), 403);

    // --- self-profile vs others: self readable, others need cap ---------
    assert_eq!(
        http(
            ureq::get(format!("{base}/api/v1/users/{sub_id}").as_str())
                .set("cookie", &authed(&sub))
                .call()
        )
        .status(),
        200
    );
    assert_eq!(
        http(
            ureq::get(format!("{base}/api/v1/users/{admin_id}").as_str())
                .set("cookie", &authed(&sub))
                .call()
        )
        .status(),
        403
    );

    // --- profile update: self-service works for subscriber --------------
    let updated = http(
        ureq::put(format!("{base}/api/v1/users/me").as_str())
            .set("cookie", &authed(&sub))
            .send_json(json!({"bio": "hello from subscriber"})),
    );
    assert_eq!(updated.status(), 204);
    let me = json_body(http(
        ureq::get(format!("{base}/api/v1/auth/me").as_str())
            .set("cookie", &authed(&sub))
            .call(),
    ));
    // /auth/me does not carry bio; read via self GET instead.
    let me_full = json_body(http(
        ureq::get(format!("{base}/api/v1/users/{sub_id}").as_str())
            .set("cookie", &authed(&sub))
            .call(),
    ));
    assert_eq!(me_full["bio"], "hello from subscriber");
    let _ = me;

    // --- role change: admin only; last-admin protection -----------------
    assert_eq!(
        http(
            ureq::put(format!("{base}/api/v1/users/{author_id}/role").as_str())
                .set("cookie", &authed(&author))
                .send_json(json!({"role": "admin"}))
        )
        .status(),
        403
    );
    assert_eq!(
        http(
            ureq::put(format!("{base}/api/v1/users/{author_id}/role").as_str())
                .set("cookie", &authed(&admin))
                .send_json(json!({"role": "editor"}))
        )
        .status(),
        204
    );
    // Author is now editor; admin demotes themselves → blocked (last admin).
    let demote = http(
        ureq::put(format!("{base}/api/v1/users/{admin_id}/role").as_str())
            .set("cookie", &authed(&admin))
            .send_json(json!({"role": "subscriber"})),
    );
    assert_eq!(demote.status(), 400, "last admin must not be demotable");
    assert_eq!(json_body(demote)["code"], "validation_failed");

    // --- delete: admin; last admin protected -----------------------------
    let del = http(
        ureq::delete(format!("{base}/api/v1/users/{admin_id}").as_str())
            .set("cookie", &authed(&admin))
            .call(),
    );
    assert_eq!(del.status(), 400, "last admin must not be deletable");
    let del = http(
        ureq::delete(format!("{base}/api/v1/users/{sub_id}").as_str())
            .set("cookie", &authed(&author)) // author is editor now: no manage_users
            .call(),
    );
    assert_eq!(del.status(), 403);
    let del = http(
        ureq::delete(format!("{base}/api/v1/users/{sub_id}").as_str())
            .set("cookie", &authed(&admin))
            .call(),
    );
    assert_eq!(del.status(), 204);
}
