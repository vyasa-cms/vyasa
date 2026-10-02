//! An account that has not confirmed its address never becomes a public
//! author (phase 98): content is not reassigned to it, an import does not
//! attribute posts to it, and GraphQL does not name it as an author.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

async fn unconfirmed(pool: &PgPool, id: i64, email: &str, username: &str) {
    UsersRepo::new(pool.clone())
        .insert_unconfirmed(
            &NewUser {
                id,
                email,
                username,
                display_name: "Pending Person",
                password_hash: None,
                role: Role::Subscriber,
                bio: "",
            },
            None,
        )
        .await
        .expect("unconfirmed account");
}

async fn published_post(pool: &PgPool, id: i64, author: i64, slug: &str) {
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, published_at)
         VALUES ($1, $2, 'post', 'published', $3, 'A title',
                 '{\"schema_version\":1,\"blocks\":[]}', '{}', now())",
    )
    .bind(id)
    .bind(author)
    .bind(slug)
    .execute(pool)
    .await
    .expect("post");
}

async fn author_of(pool: &PgPool, slug: &str) -> i64 {
    sqlx::query_scalar("SELECT author_id FROM posts WHERE slug = $1")
        .bind(slug)
        .fetch_one(pool)
        .await
        .expect("post author")
}

#[tokio::test]
async fn content_is_not_reassigned_to_an_unconfirmed_account() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let leaving = common::seed_user(pool, Role::Author).await;
    unconfirmed(pool, 94_001, "pending@example.com", "pending-a2b3c4").await;
    published_post(pool, 94_100, leaving.id, "kept").await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    let refused = http(
        ureq::delete(&format!(
            "{base}/api/v1/users/{}?reassign_to=94001",
            leaving.id
        ))
        .set("cookie", &cookie)
        .call(),
    );
    assert_eq!(refused.status(), 400);
    let body: Value = refused.into_json().unwrap();
    assert_eq!(body["code"], "validation_failed");
    assert_eq!(
        body["message"],
        "content can only be reassigned to an active, confirmed account"
    );
    assert_eq!(author_of(pool, "kept").await, leaving.id);
    assert!(UsersRepo::new(pool.clone()).get(leaving.id).await.is_ok());
}

#[tokio::test]
async fn an_archive_import_does_not_attribute_posts_to_an_unconfirmed_account() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    unconfirmed(pool, 94_001, "pending@example.com", "pending-a2b3c4").await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    // The archive's author has the address of the unconfirmed account.
    let archive = json!({
        "version": 1,
        "exported_at": "2026-09-30T00:00:00Z",
        "site": {},
        "roles": [],
        "users": [{
            "id": 7, "email": "PENDING@example.com", "username": "writer",
            "display_name": "Writer", "role": "author", "bio": ""
        }],
        "terms": [],
        "posts": [{
            "id": 70, "type": "post", "status": "published", "slug": "imported",
            "title": "Imported", "content": {"schema_version": 1, "blocks": []},
            "author_id": 7
        }],
        "comments": [],
        "media": [],
        "menus": []
    });
    let resp = http(
        ureq::post(&format!("{base}/api/v1/import"))
            .set("cookie", &cookie)
            .send_json(archive),
    );
    assert_eq!(resp.status(), 200);
    let report: Value = resp.into_json().unwrap();
    assert_eq!(report["posts"], 1, "{report}");
    assert_eq!(author_of(pool, "imported").await, admin.id);
    let warnings = report["warnings"].to_string();
    assert!(
        warnings.contains("PENDING@example.com") && warnings.contains("not confirmed"),
        "{report}"
    );
    // The account itself is as it was.
    let account = UsersRepo::new(pool.clone()).get(94_001).await.unwrap();
    assert!(account.email_verified_at.is_none());
    assert_eq!(account.role, Role::Subscriber);
}

#[tokio::test]
async fn graphql_does_not_name_an_unconfirmed_account_as_an_author() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    unconfirmed(pool, 94_001, "pending@example.com", "pending-a2b3c4").await;
    // However it came about, a published post by the unconfirmed account,
    // and one by a confirmed author.
    published_post(pool, 94_100, 94_001, "by-pending").await;
    published_post(pool, 94_101, admin.id, "by-admin").await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    let resp = http(ureq::post(&format!("{base}/api/graphql")).send_json(json!({
        "query": "{ posts(first: 10) { edges { node { slug author { username displayName } } } } }"
    })));
    assert_eq!(resp.status(), 200);
    let body: Value = resp.into_json().unwrap();
    assert!(body["errors"].is_null(), "{body}");
    let edges = body["data"]["posts"]["edges"].as_array().unwrap();
    let author = |slug: &str| {
        edges
            .iter()
            .find(|e| e["node"]["slug"] == slug)
            .map(|e| e["node"]["author"].clone())
            .expect(slug)
    };
    assert!(author("by-pending").is_null(), "{body}");
    assert!(!body.to_string().contains("pending-a2b3c4"));
    assert!(!body.to_string().contains("Pending Person"));
    assert!(author("by-admin")["username"].is_string(), "{body}");
}
