//! The AI integrations over REST, without a provider: switches gate every
//! call, storage round-trips, and similarity ranks what it should.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

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

fn login(base: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "feat-admin@example.com", "password": "pw-secret-1"})),
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
    UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 84_001,
            email: "feat-admin@example.com",
            username: "featadmin",
            display_name: "Feature Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
}

fn doc(text: &str) -> Value {
    // Long enough that the assist kinds see "enough content" and reach the
    // registry, which is what these tests want to hit.
    let body =
        format!("{text}: a paragraph of ordinary prose long enough to be summarised sensibly.");
    json!({"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": body}}]})
}

#[tokio::test]
async fn switches_gate_every_call_and_related_posts_rank_by_similarity() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);

    // Three published posts.
    let mut ids = Vec::new();
    for title in ["Alpha", "Beta", "Gamma"] {
        let created = json_body(http(
            ureq::post(format!("{base}/api/v1/posts").as_str())
                .set("cookie", &cookie)
                .send_json(json!({"title": title, "content": doc(title), "status": "published"})),
        ));
        ids.push(created["id"].as_i64().unwrap());
    }

    // Everything is off by default: on-demand calls say so.
    let off = http(
        ureq::post(format!("{base}/api/v1/posts/{}/autofill", ids[0]).as_str())
            .set("cookie", &cookie)
            .call(),
    );
    assert_eq!(off.status(), 400);
    assert!(json_body(off)["message"]
        .as_str()
        .unwrap()
        .contains("turned off"));
    let related_off = http(
        ureq::get(format!("{base}/api/v1/posts/{}/related", ids[0]).as_str())
            .set("cookie", &cookie)
            .call(),
    );
    assert_eq!(related_off.status(), 400);

    // Option validation: booleans must be booleans, the mode is an enum.
    let bad = http(
        ureq::put(format!("{base}/api/v1/options").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"ai_alt_text": "yes"})),
    );
    assert_eq!(bad.status(), 400);
    let bad_mode = http(
        ureq::put(format!("{base}/api/v1/options").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"ai_comment_screening": "maybe"})),
    );
    assert_eq!(bad_mode.status(), 400);

    // Turn features on.
    let ok = http(
        ureq::put(format!("{base}/api/v1/options").as_str())
            .set("cookie", &cookie)
            .send_json(json!({
                "ai_alt_text": true, "ai_autofill": true, "ai_embeddings": true,
                "ai_related_posts": true, "ai_images": true, "ai_comment_screening": "flag"
            })),
    );
    assert!(ok.status() < 300, "options save: {}", ok.status());

    // On, but no model registered: a clear message, not a 500.
    let no_model = http(
        ureq::post(format!("{base}/api/v1/posts/{}/autofill", ids[0]).as_str())
            .set("cookie", &cookie)
            .call(),
    );
    assert_eq!(no_model.status(), 400);
    assert!(json_body(no_model)["message"]
        .as_str()
        .unwrap()
        .contains("Models page"));
    let no_image_model = http(
        ureq::post(format!("{base}/api/v1/ai/images").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"prompt": "a lighthouse"})),
    );
    assert_eq!(no_image_model.status(), 400);

    // Similarity: vectors written straight to the table (no provider
    // needed) — Alpha is close to Beta and far from Gamma.
    for (id, vector) in [
        (ids[0], vec![1.0f32, 0.0, 0.0]),
        (ids[1], vec![0.9f32, 0.1, 0.0]),
        (ids[2], vec![0.0f32, 0.0, 1.0]),
    ] {
        sqlx::query(
            "INSERT INTO post_embeddings (post_id, model, text_hash, dims, vector) \
             VALUES ($1, 'test', 'h', 3, $2)",
        )
        .bind(id)
        .bind(&vector)
        .execute(db.pool())
        .await
        .unwrap();
    }
    let related = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{}/related?limit=5", ids[0]).as_str())
            .set("cookie", &cookie)
            .call(),
    ));
    let titles: Vec<&str> = related["posts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["Beta", "Gamma"], "nearest first, never itself");

    // Unpublishing a post removes it from the neighbours.
    let unpublished = http(
        ureq::put(format!("{base}/api/v1/posts/{}", ids[1]).as_str())
            .set("cookie", &cookie)
            .send_json(json!({"status": "draft"})),
    );
    assert!(unpublished.status() < 300);
    let related = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{}/related", ids[0]).as_str())
            .set("cookie", &cookie)
            .call(),
    ));
    assert_eq!(related["posts"].as_array().unwrap().len(), 1);

    // Comments carry their verdict once one is stored.
    let comment = json_body(http(
        ureq::post(format!("{base}/api/v1/posts/{}/comments", ids[0]).as_str())
            .send_json(json!({"author_name": "Reader", "author_email": "r@example.com", "content": "Nice post"})),
    ));
    let cid = comment["id"].as_i64().unwrap();
    sqlx::query("UPDATE comments SET moderation = $2 WHERE id = $1")
        .bind(cid)
        .bind(json!({"flagged": false, "top": [], "model": "test", "action": "none"}))
        .execute(db.pool())
        .await
        .unwrap();
    // Totals describe the entire matching status, including when the page is empty.
    let page = http(
        ureq::get(format!("{base}/api/v1/comments?status=pending&limit=1&offset=9999").as_str())
            .set("cookie", &cookie)
            .call(),
    );
    let total = page
        .header("x-total-count")
        .expect("comment total")
        .parse::<i64>()
        .unwrap();
    assert!(total >= 1);
    assert!(json_body(page).as_array().unwrap().is_empty());
    let listed = json_body(http(
        ureq::get(format!("{base}/api/v1/comments?status=pending").as_str())
            .set("cookie", &cookie)
            .call(),
    ));
    let mine = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_i64() == Some(cid))
        .unwrap();
    assert_eq!(mine["moderation"]["flagged"], false);

    // Post meta survives an update and carries the SEO fields.
    let with_meta = json_body(http(
        ureq::put(format!("{base}/api/v1/posts/{}", ids[0]).as_str())
            .set("cookie", &cookie)
            .send_json(json!({"meta": {"seo_description": "A short description", "keep": 1}})),
    ));
    assert_eq!(with_meta["meta"]["seo_description"], "A short description");
    assert_eq!(with_meta["meta"]["keep"], 1);
}
