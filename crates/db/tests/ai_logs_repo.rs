//! The AI usage log: a recorded call reads back on the usage page and
//! counts toward the month's spend and the per-purpose daily limits.
//!
//! `record` and `recent` once carried a stray backslash from a Rust line
//! continuation (`\\` where `\` was meant), invalid SQL, so no AI call was
//! ever logged and the budget caps never saw any spend. Exercised here
//! against a real database for that reason, as `webhooks_repo` is.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_db::repo::AiLogRepo;
use vyasa_testkit::TestDb;

#[tokio::test]
async fn recorded_calls_read_back_and_count() {
    let db = TestDb::new().await;
    let logs = AiLogRepo::new(db.pool().clone());
    logs.record(
        "anthropic",
        "claude-haiku-4-5",
        "theme-studio",
        120,
        30,
        0.25,
    )
    .await
    .expect("record");
    logs.record("openai", "gpt-image-1", "image-generate", 0, 0, 0.5)
        .await
        .expect("record");

    let recent = logs.recent(10).await.expect("recent");
    assert_eq!(recent.len(), 2);
    let studio = recent.iter().find(|r| r.purpose == "theme-studio").unwrap();
    assert_eq!(
        (
            studio.provider.as_str(),
            studio.model.as_str(),
            studio.prompt_tokens,
            studio.completion_tokens
        ),
        ("anthropic", "claude-haiku-4-5", 120, 30)
    );
    let spent = logs.month_spend_total().await.expect("month");
    assert!((spent - 0.75).abs() < 1e-9, "{spent}");
    let since = chrono::Utc::now() - chrono::Duration::hours(24);
    assert_eq!(
        logs.count_purpose_since("image-generate", since)
            .await
            .unwrap(),
        1
    );
}
