//! Migration 0052 removes the options that used to choose the marketplace
//! and update channel; they are compiled into the binary now.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_testkit::TestDb;

#[tokio::test]
async fn retired_options_are_deleted_by_the_migration() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    for key in [
        "registry_url",
        "registry_trusted_keys",
        "update_channel_url",
        "update_trusted_keys",
        "site_title",
    ] {
        sqlx::query(
            "INSERT INTO options (key, value) VALUES ($1, '\"x\"') ON CONFLICT (key) DO NOTHING",
        )
        .bind(key)
        .execute(&pool)
        .await
        .expect("insert");
    }
    // The migration already ran on the template; apply its SQL again, the
    // way a database upgraded from rc.2 would see it run once.
    sqlx::raw_sql(include_str!(
        "../migrations/0052_retire_marketplace_options.sql"
    ))
    .execute(&pool)
    .await
    .expect("migration sql");
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM options WHERE key IN \
         ('registry_url', 'registry_trusted_keys', 'update_channel_url', 'update_trusted_keys')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left, 0);
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM options WHERE key = 'site_title'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, 1, "other options are untouched");
}
