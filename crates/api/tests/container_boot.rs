//! What a container platform relies on at boot: health endpoints, the
//! first administrator from the environment, and a search index that
//! rebuilds itself.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use vyasa_testkit::{TestDb, TestServer};

fn get(url: &str) -> (u16, serde_json::Value) {
    match ureq::get(url).call() {
        Ok(r) | Err(ureq::Error::Status(_, r)) => {
            let status = r.status();
            (status, r.into_json().unwrap_or(serde_json::Value::Null))
        }
        Err(e) => panic!("transport {e}"),
    }
}

#[tokio::test]
async fn healthz_and_readyz_answer_on_a_fresh_install() {
    let db = TestDb::new().await;
    let server = TestServer::start(common::BIN, &db);
    let (s, body) = get(&format!("{}/healthz", server.base()));
    assert_eq!((s, body["status"].as_str()), (200, Some("ok")), "{body}");
    let (s, body) = get(&format!("{}/readyz", server.base()));
    assert_eq!((s, body["status"].as_str()), (200, Some("ready")), "{body}");
}

#[tokio::test]
async fn readyz_reports_pending_migrations() {
    let db = TestDb::new().await;
    let server = TestServer::start(common::BIN, &db);
    // A newer binary's migration missing here: forget the last applied
    // one so the migrator sees it pending.
    sqlx::query(
        "DELETE FROM _sqlx_migrations WHERE version = (SELECT max(version) FROM _sqlx_migrations)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let (s, body) = get(&format!("{}/readyz", server.base()));
    assert_eq!(s, 503, "{body}");
    assert_eq!(body["reason"], "migrations pending", "{body}");
}

// ---- the first administrator from the environment ----------------------

#[tokio::test]
async fn the_first_admin_can_come_from_the_environment() {
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_ADMIN_EMAIL", "ops@example.com")
        .env("VYASA_ADMIN_PASSWORD", "a-long-enough-password")
        .start();
    let cookie = common::login_cookie(server.base(), "ops@example.com", "a-long-enough-password");
    assert!(cookie.starts_with("vy_session="), "{cookie}");
    let (s, body) = get(&format!("{}/api/v1/setup/status", server.base()));
    assert_eq!(s, 200);
    assert_eq!(body["needs_admin"], false, "{body}");
}

#[tokio::test]
async fn admin_variables_are_ignored_when_users_exist() {
    let db = TestDb::new().await;
    let _existing = common::seed_user(db.pool(), vyasa_db::models::Role::Admin).await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_ADMIN_EMAIL", "ops@example.com")
        .env("VYASA_ADMIN_PASSWORD", "a-long-enough-password")
        .start();
    let r = ureq::post(&format!("{}/api/v1/auth/login", server.base())).send_json(
        serde_json::json!({ "email": "ops@example.com", "password": "a-long-enough-password" }),
    );
    assert!(
        matches!(r, Err(ureq::Error::Status(401, _))),
        "no second administrator was created"
    );
}

#[tokio::test]
async fn half_set_admin_variables_are_ignored_with_a_warning() {
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_ADMIN_EMAIL", "ops@example.com")
        .start();
    let (s, body) = get(&format!("{}/api/v1/setup/status", server.base()));
    assert_eq!(s, 200);
    assert_eq!(
        body["needs_admin"], true,
        "the setup-token path still applies: {body}"
    );
}

#[tokio::test]
async fn a_weak_admin_password_stops_the_boot() {
    let db = TestDb::new().await;
    let out = std::process::Command::new(common::BIN)
        .arg("serve")
        .env("VYASA_DATABASE_URL", db.url())
        .env("VYASA_BIND_ADDR", "127.0.0.1:0")
        .env("VYASA_ADMIN_EMAIL", "ops@example.com")
        .env("VYASA_ADMIN_PASSWORD", "short")
        .output()
        .expect("run the binary");
    assert!(!out.status.success(), "the boot must stop");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("VYASA_ADMIN_PASSWORD"), "{err}");
}
