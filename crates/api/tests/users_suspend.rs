//! Suspending accounts over HTTP: it never leaves the site without an
//! active administrator, however the requests interleave.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_testkit::{TestDb, TestServer};

const MISSING: i64 = 9_999_999;

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn suspend(base: &str, cookie: &str, id: i64) -> ureq::Response {
    http(
        ureq::post(&format!("{base}/api/v1/users/{id}/suspend"))
            .set("cookie", cookie)
            .send_json(json!({ "suspended": true })),
    )
}

async fn active_admins(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin' AND suspended_at IS NULL")
        .fetch_one(pool)
        .await
        .expect("admin count")
}

#[tokio::test]
async fn suspension_answers_as_it_did() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let second = common::seed_user(pool, Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    let second_cookie = common::login_cookie(base, &second.email, &second.password);

    assert_eq!(suspend(base, &admin_cookie, MISSING).status(), 404);
    // With another admin left, an admin can be suspended; their sessions end.
    assert_eq!(suspend(base, &admin_cookie, second.id).status(), 204);
    assert_eq!(active_admins(pool).await, 1);
    let me = http(
        ureq::get(&format!("{base}/api/v1/auth/me"))
            .set("cookie", &second_cookie)
            .call(),
    );
    assert_eq!(me.status(), 401);
    // Suspending someone already suspended is still fine.
    assert_eq!(suspend(base, &admin_cookie, second.id).status(), 204);

    // The last administrator is the caller: refused as before.
    let last = suspend(base, &admin_cookie, admin.id);
    assert_eq!(last.status(), 400);
    let body: Value = last.into_json().expect("a JSON body");
    assert_eq!(body["code"], "validation_failed");
    assert_eq!(active_admins(pool).await, 1);
}

/// Two administrators suspending each other at the same moment: one of
/// the requests is refused (or arrives signed out); never both applied.
#[tokio::test]
async fn two_admins_suspending_each_other_leave_one() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let a = common::seed_user(pool, Role::Admin).await;
    let b = common::seed_user(pool, Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base().to_owned();

    let a_cookie = common::login_cookie(&base, &a.email, &a.password);
    let b_cookie = common::login_cookie(&base, &b.email, &b.password);
    let (base_a, base_b) = (base.clone(), base.clone());
    let (a_id, b_id) = (a.id, b.id);
    let first = tokio::task::spawn_blocking(move || suspend(&base_a, &a_cookie, b_id).status());
    let second = tokio::task::spawn_blocking(move || suspend(&base_b, &b_cookie, a_id).status());
    let statuses = [first.await.expect("join"), second.await.expect("join")];
    assert_eq!(active_admins(pool).await, 1, "{statuses:?}");
    assert!(statuses.contains(&204), "{statuses:?}");
}
