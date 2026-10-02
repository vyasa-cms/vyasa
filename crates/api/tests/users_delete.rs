//! Deleting an account over HTTP: `DELETE /users/{id}` hands the
//! account's content to `reassign_to`.
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

fn json_body(r: ureq::Response) -> Value {
    r.into_json().expect("a JSON body")
}

/// Creates a draft as `cookie` and returns its id.
fn create_post(base: &str, cookie: &str, title: &str) -> i64 {
    let resp = http(
        ureq::post(&format!("{base}/api/v1/posts"))
            .set("cookie", cookie)
            .send_json(json!({
                "title": title,
                "content": { "schema_version": 1, "blocks": [] },
            })),
    );
    assert_eq!(resp.status(), 201, "create {title}");
    json_body(resp)["id"].as_i64().expect("post id")
}

fn delete_user(base: &str, cookie: &str, id: i64, query: &str) -> ureq::Response {
    http(
        ureq::delete(&format!("{base}/api/v1/users/{id}{query}"))
            .set("cookie", cookie)
            .call(),
    )
}

async fn user_exists(pool: &PgPool, id: i64) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("exists")
}

async fn author_of(pool: &PgPool, post: i64) -> i64 {
    sqlx::query_scalar("SELECT author_id FROM posts WHERE id = $1")
        .bind(post)
        .fetch_one(pool)
        .await
        .expect("post author")
}

#[tokio::test]
async fn deleting_a_user_who_owns_content_takes_a_reassignment_target() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let author = common::seed_user(pool, Role::Author).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    let author_cookie = common::login_cookie(base, &author.email, &author.password);
    let post = create_post(base, &author_cookie, "Owned");

    // No target: refused, and the refusal says what they own.
    let refused = delete_user(base, &admin_cookie, author.id, "");
    assert_eq!(refused.status(), 409);
    let body = json_body(refused);
    assert_eq!(body["code"], "conflict");
    let message = body["message"].as_str().expect("message");
    assert!(message.contains("1 post(s)"), "{message}");
    assert!(message.contains("0 media item(s)"), "{message}");
    assert!(user_exists(pool, author.id).await);

    // To themselves: not a target.
    let to_self = delete_user(
        base,
        &admin_cookie,
        author.id,
        &format!("?reassign_to={}", author.id),
    );
    assert_eq!(to_self.status(), 400);
    assert_eq!(json_body(to_self)["code"], "validation_failed");
    assert!(user_exists(pool, author.id).await);

    // To nobody we know.
    let unknown = delete_user(
        base,
        &admin_cookie,
        author.id,
        &format!("?reassign_to={MISSING}"),
    );
    assert_eq!(unknown.status(), 404);
    assert_eq!(json_body(unknown)["code"], "user_not_found");
    assert!(user_exists(pool, author.id).await);
    assert_eq!(author_of(pool, post).await, author.id);

    // To the admin: gone, and the post is the admin's.
    let done = delete_user(
        base,
        &admin_cookie,
        author.id,
        &format!("?reassign_to={}", admin.id),
    );
    assert_eq!(done.status(), 204);
    assert!(!user_exists(pool, author.id).await);
    assert_eq!(author_of(pool, post).await, admin.id);

    // The audit entry names the heir.
    let mut detail: Option<Value> = None;
    for _ in 0..50 {
        detail = sqlx::query_scalar("SELECT detail FROM audit_log WHERE action = 'user.delete'")
            .fetch_optional(pool)
            .await
            .expect("audit");
        if detail.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        detail.expect("a user.delete audit entry")["reassign_to"],
        admin.id
    );
}

#[tokio::test]
async fn deleting_a_user_without_content_needs_no_target() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let subscriber = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);

    assert_eq!(
        delete_user(base, &admin_cookie, subscriber.id, "").status(),
        204
    );
    assert!(!user_exists(pool, subscriber.id).await);
    // A user who is not there.
    assert_eq!(delete_user(base, &admin_cookie, MISSING, "").status(), 404);
}

#[tokio::test]
async fn an_admin_cannot_delete_their_own_account() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let other = common::seed_user(pool, Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    for query in [String::new(), format!("?reassign_to={}", other.id)] {
        let refused = delete_user(base, &cookie, admin.id, &query);
        assert_eq!(refused.status(), 400);
        let body = json_body(refused);
        assert_eq!(body["code"], "validation_failed");
        assert_eq!(body["message"], "you cannot delete your own account");
        assert!(user_exists(pool, admin.id).await);
    }
    // Another admin's account is theirs to delete.
    assert_eq!(delete_user(base, &cookie, other.id, "").status(), 204);
}
