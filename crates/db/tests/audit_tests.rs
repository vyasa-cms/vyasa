//! Audit log: writes land, reads come back newest-first, and the log
//! survives the deletion of the account that acted.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_db::repo::AuditRepo;
use vyasa_testkit::TestDb;

#[tokio::test]
async fn entries_come_back_newest_first() {
    let db = TestDb::new().await;
    let repo = AuditRepo::new(db.pool().clone());

    for action in ["user.delete", "theme.activate", "options.write"] {
        repo.record(
            Some(1),
            "Ada",
            action,
            "target:1",
            serde_json::json!({}),
            "10.0.0.1",
        )
        .await
        .expect("record");
    }

    let rows = repo.recent(10).await.expect("recent");
    assert_eq!(rows.len(), 3);
    // Newest first is what an operator investigating an incident wants.
    assert_eq!(rows[0].action, "options.write");
    assert_eq!(rows[2].action, "user.delete");
    assert_eq!(rows[0].actor_name, "Ada");
}

#[tokio::test]
async fn the_limit_is_clamped_rather_than_trusted() {
    let db = TestDb::new().await;
    let repo = AuditRepo::new(db.pool().clone());
    repo.record(Some(1), "Ada", "x.y", "t", serde_json::json!({}), "")
        .await
        .expect("record");

    // A negative or absurd limit must not become a SQL error or an
    // unbounded read.
    assert_eq!(repo.recent(-5).await.expect("clamped low").len(), 1);
    assert_eq!(repo.recent(10_000).await.expect("clamped high").len(), 1);
}

#[tokio::test]
async fn the_log_survives_deletion_of_the_actor() {
    let db = TestDb::new().await;
    let repo = AuditRepo::new(db.pool().clone());

    // actor_id is deliberately not a foreign key: an audit row explaining a
    // user deletion must not vanish when that user is deleted.
    repo.record(
        Some(999_999),
        "Departed Admin",
        "user.delete",
        "user:42",
        serde_json::json!({}),
        "",
    )
    .await
    .expect("record against a user that does not exist");

    let rows = repo.recent(10).await.expect("recent");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_name, "Departed Admin");
    assert_eq!(rows[0].actor_id, Some(999_999));
}

#[tokio::test]
async fn detail_is_stored_as_structured_json() {
    let db = TestDb::new().await;
    let repo = AuditRepo::new(db.pool().clone());
    repo.record(
        Some(1),
        "Ada",
        "options.write",
        "options:2",
        serde_json::json!({ "keys": ["site_title", "site_url"] }),
        "",
    )
    .await
    .expect("record");

    let rows = repo.recent(1).await.expect("recent");
    assert_eq!(rows[0].detail["keys"][1], "site_url");
}
