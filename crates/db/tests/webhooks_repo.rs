//! The webhooks repository: subscription fan-out and the delivery log.
//!
//! `list_for_event` is the query that decides whether a webhook fires at
//! all, and it once contained a stray backslash from a Rust line
//! continuation — syntactically invalid SQL, so no webhook could ever have
//! fired. It is exercised here against a real database for that reason.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_db::repo::WebhooksRepo;
use vyasa_testkit::TestDb;

#[tokio::test]
async fn fan_out_selects_only_enabled_subscribers_of_that_event() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = WebhooksRepo::new(pool.clone());

    let published = repo
        .create(
            "https://a.test/hook",
            "secret-a",
            &serde_json::json!(["post.published", "post.updated"]),
        )
        .await
        .expect("create");
    let other_event = repo
        .create(
            "https://b.test/hook",
            "secret-b",
            &serde_json::json!(["comment.added"]),
        )
        .await
        .expect("create");
    let disabled = repo
        .create(
            "https://c.test/hook",
            "secret-c",
            &serde_json::json!(["post.published"]),
        )
        .await
        .expect("create");
    sqlx::query("UPDATE webhooks SET enabled = false WHERE id = $1")
        .bind(disabled.id)
        .execute(&pool)
        .await
        .expect("disable");

    let firing = repo.list_for_event("post.published").await.expect("fanout");
    let ids: Vec<i64> = firing.iter().map(|w| w.id).collect();
    assert_eq!(ids, vec![published.id], "only the enabled subscriber");

    // A second event on the same subscriber also matches.
    assert_eq!(
        repo.list_for_event("post.updated").await.unwrap().len(),
        1,
        "the events list is a set, not a single value"
    );
    assert_eq!(
        repo.list_for_event("comment.added").await.unwrap()[0].id,
        other_event.id
    );
    // An event nobody subscribed to fans out to nobody, rather than to
    // everybody.
    assert!(repo
        .list_for_event("post.trashed")
        .await
        .unwrap()
        .is_empty());
    pool.close().await;
}

#[tokio::test]
async fn a_secret_is_stored_and_returned_for_signing() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = WebhooksRepo::new(pool.clone());
    let row = repo
        .create("https://a.test/h", "shhh", &serde_json::json!(["ping"]))
        .await
        .expect("create");
    // The dispatcher signs each payload with this, so the fan-out query
    // has to carry it — a listing that dropped it would produce webhooks
    // nobody could verify.
    assert_eq!(repo.list_for_event("ping").await.unwrap()[0].secret, "shhh");
    assert_eq!(repo.get(row.id).await.unwrap().url, "https://a.test/h");
    assert!(repo.get(404).await.is_err());

    assert_eq!(repo.list().await.unwrap().len(), 1);
    repo.delete(row.id).await.expect("delete");
    assert!(repo.list().await.unwrap().is_empty());
    pool.close().await;
}

#[tokio::test]
async fn attempts_count_up_per_event_and_deliveries_read_back_newest_first() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = WebhooksRepo::new(pool.clone());
    let hook = repo
        .create(
            "https://a.test/h",
            "s",
            &serde_json::json!(["post.published", "comment.added"]),
        )
        .await
        .expect("create");

    repo.record_delivery(hook.id, "post.published", "failed", Some(500))
        .await
        .expect("first");
    repo.record_delivery(hook.id, "post.published", "failed", Some(502))
        .await
        .expect("second");
    repo.record_delivery(hook.id, "post.published", "success", Some(200))
        .await
        .expect("third");

    let rows = repo.deliveries(hook.id, 10).await.expect("deliveries");
    assert_eq!(rows.len(), 3);
    // Newest first, so an operator opening the page sees what just
    // happened rather than what happened first.
    assert_eq!(rows[0].status, "success");
    assert_eq!(rows[0].response_code, Some(200));
    // Retries of the same event accumulate, which is what a backoff reads.
    assert_eq!(rows[0].attempts, 3);

    // A different event starts its own count rather than inheriting one.
    repo.record_delivery(hook.id, "comment.added", "success", Some(200))
        .await
        .expect("other event");
    let other = repo
        .deliveries(hook.id, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.event == "comment.added")
        .expect("recorded");
    assert_eq!(other.attempts, 1);

    // The limit is honoured.
    assert_eq!(repo.deliveries(hook.id, 2).await.unwrap().len(), 2);
    pool.close().await;
}

#[tokio::test]
async fn deleting_a_webhook_takes_its_delivery_log_with_it() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = WebhooksRepo::new(pool.clone());
    let hook = repo
        .create("https://a.test/h", "s", &serde_json::json!(["ping"]))
        .await
        .expect("create");
    repo.record_delivery(hook.id, "ping", "success", Some(200))
        .await
        .expect("record");

    repo.delete(hook.id).await.expect("delete");
    let orphans: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE webhook_id = $1")
            .bind(hook.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(orphans, 0, "no delivery rows pointing at a deleted webhook");
    pool.close().await;
}

#[tokio::test]
async fn only_the_three_statuses_the_schema_knows_are_accepted() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = WebhooksRepo::new(pool.clone());
    let hook = repo
        .create("https://a.test/h", "s", &serde_json::json!(["ping"]))
        .await
        .expect("create");

    for good in ["success", "failed", "dead"] {
        repo.record_delivery(hook.id, "ping", good, Some(200))
            .await
            .unwrap_or_else(|e| panic!("{good}: {e}"));
    }
    // `record_delivery` takes a plain `&str`, so the column's constraint is
    // the only thing standing between a typo and a delivery log that
    // silently records nothing — both callers swallow the error so as not
    // to retry a delivery that actually succeeded.
    assert!(
        repo.record_delivery(hook.id, "ping", "delivered", Some(200))
            .await
            .is_err(),
        "an unknown status must be refused, not stored"
    );
    pool.close().await;
}
