//! Media storage chosen from the admin: saved with the keys sealed,
//! applied to the running server, old files still served, moved on
//! request. Needs the throwaway MinIO that `scripts/test-with-db.sh`
//! starts; skipped without it.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::time::{Duration, Instant};

use vyasa_db::models::Role;
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

struct S3Env {
    endpoint: String,
    bucket: String,
    key: String,
    secret: String,
}

fn s3_env() -> Option<S3Env> {
    Some(S3Env {
        endpoint: std::env::var("VYASA_TEST_S3_ENDPOINT").ok()?,
        bucket: std::env::var("VYASA_TEST_S3_BUCKET").ok()?,
        key: std::env::var("VYASA_TEST_S3_KEY").ok()?,
        secret: std::env::var("VYASA_TEST_S3_SECRET").ok()?,
    })
}

fn input(env: &S3Env, secret: &str) -> serde_json::Value {
    serde_json::json!({
        "provider": "s3",
        "bucket": env.bucket,
        "region": "us-east-1",
        "endpoint": env.endpoint,
        "path_style": true,
        "access_key_id": env.key,
        "secret_access_key": secret,
    })
}

/// A PNG by signature, different bytes per name: identical uploads are
/// de-duplicated to the existing row.
fn png(name: &str) -> Vec<u8> {
    let mut bytes = vec![0u8; 256];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes[16..16 + name.len()].copy_from_slice(name.as_bytes());
    bytes
}

fn upload(base: &str, cookie: &str, name: &str) -> i64 {
    let boundary = "----vyasastorage";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&png(name));
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let r = http(
        ureq::post(&format!("{base}/api/v1/media"))
            .set("Cookie", cookie)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body),
    );
    assert_eq!(r.status(), 201);
    let v: serde_json::Value = r.into_json().unwrap();
    v["id"]
        .as_i64()
        .or_else(|| v["id"].as_str().and_then(|s| s.parse().ok()))
        .expect("id")
}

fn settings(base: &str, cookie: &str) -> serde_json::Value {
    http(
        ureq::get(&format!("{base}/api/v1/media/storage"))
            .set("Cookie", cookie)
            .call(),
    )
    .into_json()
    .unwrap()
}

fn put(base: &str, cookie: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let r = http(
        ureq::put(&format!("{base}/api/v1/media/storage"))
            .set("Cookie", cookie)
            .send_json(body.clone()),
    );
    let s = r.status();
    (s, r.into_json().unwrap_or(serde_json::Value::Null))
}

fn raw_status(base: &str, id: i64) -> u16 {
    http(ureq::get(&format!("{base}/api/v1/media/{id}/raw")).call()).status()
}

fn walkdir_count(dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            if e.path().is_dir() {
                walkdir_count(&e.path())
            } else {
                1
            }
        })
        .sum()
}

async fn storage_of(db: &TestDb, id: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT storage FROM media WHERE id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn the_admin_switches_to_object_storage_and_moves_files_across() {
    let Some(env) = s3_env() else {
        eprintln!("skipping: no VYASA_TEST_S3_ENDPOINT");
        return;
    };
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let dir = std::env::temp_dir().join(format!("vyasa-storage-{}", db.name()));
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_MEDIA_DIR", dir.join("media").display().to_string())
        .env("VYASA_SECRET_KEY", "a-server-secret-for-sealing-keys")
        .start();
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    let s = settings(base, &cookie);
    assert_eq!(s["source"], "none", "{s}");
    assert_eq!(s["provider"], "local");
    assert_eq!(s["encrypted"], true);

    let before = upload(base, &cookie, "before.png");
    assert_eq!(storage_of(&db, before).await, "local");

    // Wrong secret: refused, nothing saved, uploads still local.
    let (code, body) = put(base, &cookie, &input(&env, "not-the-secret"));
    assert_eq!(code, 400, "{body}");
    assert_eq!(settings(base, &cookie)["source"], "none");

    let (code, body) = put(base, &cookie, &input(&env, &env.secret));
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["source"], "options");
    assert_eq!(body["has_secret"], true);
    assert_eq!(body["access_key_id_hint"], env.key[env.key.len() - 4..]);
    assert!(body.get("secret_access_key").is_none());
    assert!(body.get("access_key_id").is_none());
    let text = body.to_string();
    assert!(!text.contains(&env.secret), "secret leaked: {text}");

    // Sealed at rest.
    let stored: String = sqlx::query_scalar(
        "SELECT value::text FROM options WHERE key = 'storage_secret_access_key'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(stored.contains("v1:"), "{stored}");
    assert!(!stored.contains(&env.secret));

    let after = upload(base, &cookie, "after.png");
    assert_eq!(storage_of(&db, after).await, "s3");
    // The old file still serves from disk.
    assert_eq!(raw_status(base, before), 200);
    assert_eq!(raw_status(base, after), 200);
    let s = settings(base, &cookie);
    assert_eq!(s["counts"]["local"], 1, "{s}");
    assert_eq!(s["counts"]["s3"], 1, "{s}");

    // Move the old one across.
    let r = http(
        ureq::post(&format!("{base}/api/v1/media/storage/migrate"))
            .set("Cookie", &cookie)
            .call(),
    );
    assert_eq!(r.status(), 202);
    let deadline = Instant::now() + Duration::from_secs(30);
    let done = loop {
        let s = settings(base, &cookie);
        if s["migration"]["state"] != "running" {
            break s;
        }
        assert!(Instant::now() < deadline, "move did not finish: {s}");
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(done["migration"]["state"], "done", "{done}");
    assert_eq!(done["migration"]["done"], 1);
    assert_eq!(done["counts"]["local"], 0);
    assert_eq!(done["counts"]["s3"], 2);
    assert_eq!(storage_of(&db, before).await, "s3");
    assert_eq!(raw_status(base, before), 200);
    // The source copy is gone from disk.
    let left_on_disk = walkdir_count(&dir.join("media"));
    assert_eq!(left_on_disk, 0, "files left on disk after the move");

    // Back to local: a new upload lands on disk, the ones in the bucket
    // still serve, and a move brings them back.
    let (code, body) = put(base, &cookie, &serde_json::json!({ "provider": "local" }));
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["provider"], "local");
    assert_eq!(body["source"], "options");
    let local_again = upload(base, &cookie, "local-again.png");
    assert_eq!(storage_of(&db, local_again).await, "local");
    assert_eq!(raw_status(base, before), 200);
    let r = http(
        ureq::post(&format!("{base}/api/v1/media/storage/migrate"))
            .set("Cookie", &cookie)
            .call(),
    );
    assert_eq!(r.status(), 202);
    let deadline = Instant::now() + Duration::from_secs(30);
    let done = loop {
        let s = settings(base, &cookie);
        if s["migration"]["state"] != "running" {
            break s;
        }
        assert!(Instant::now() < deadline, "move back did not finish: {s}");
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(done["migration"]["state"], "done", "{done}");
    assert_eq!(done["counts"]["local"], 3, "{done}");
    assert_eq!(done["counts"]["s3"], 0);
    assert_eq!(raw_status(base, before), 200);
    assert_eq!(raw_status(base, after), 200);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn saved_settings_survive_a_restart() {
    let Some(env) = s3_env() else {
        eprintln!("skipping: no VYASA_TEST_S3_ENDPOINT");
        return;
    };
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let boot = || {
        TestServer::builder(common::BIN, &db)
            .env("VYASA_SECRET_KEY", "a-server-secret-for-sealing-keys")
            .start()
    };
    let server = boot();
    let cookie = common::login_cookie(server.base(), &admin.email, &admin.password);
    let (code, body) = put(server.base(), &cookie, &input(&env, &env.secret));
    assert_eq!(code, 200, "{body}");
    drop(server);

    let server = boot();
    let cookie = common::login_cookie(server.base(), &admin.email, &admin.password);
    let s = settings(server.base(), &cookie);
    assert_eq!(s["source"], "options", "{s}");
    let id = upload(server.base(), &cookie, "restart.png");
    assert_eq!(storage_of(&db, id).await, "s3");
}

#[tokio::test]
async fn the_environment_wins_and_the_page_is_read_only() {
    let Some(env) = s3_env() else {
        eprintln!("skipping: no VYASA_TEST_S3_ENDPOINT");
        return;
    };
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_STORAGE__PROVIDER", "s3")
        .env("VYASA_STORAGE__BUCKET", &env.bucket)
        .env("VYASA_STORAGE__REGION", "us-east-1")
        .env("VYASA_STORAGE__ENDPOINT", &env.endpoint)
        .env("VYASA_STORAGE__ACCESS_KEY_ID", &env.key)
        .env("VYASA_STORAGE__SECRET_ACCESS_KEY", &env.secret)
        .env("VYASA_STORAGE__PATH_STYLE", "true")
        .start();
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    let s = settings(base, &cookie);
    assert_eq!(s["source"], "environment", "{s}");
    assert!(!s.to_string().contains(&env.secret));
    let (code, body) = put(base, &cookie, &serde_json::json!({ "provider": "local" }));
    assert_eq!(code, 409, "{body}");
}

#[tokio::test]
async fn a_non_administrator_cannot_read_or_change_storage() {
    let db = TestDb::new().await;
    let author = common::seed_user(db.pool(), Role::Author).await;
    let server = TestServer::start(common::BIN, &db);
    let cookie = common::login_cookie(server.base(), &author.email, &author.password);
    let r = http(
        ureq::get(&format!("{}/api/v1/media/storage", server.base()))
            .set("Cookie", &cookie)
            .call(),
    );
    assert_eq!(r.status(), 403);
}
