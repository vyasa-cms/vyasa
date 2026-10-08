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
