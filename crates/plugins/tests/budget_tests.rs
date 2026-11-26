//! The extended task budget: a longer call, only with the capability.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_plugins::broker::{Broker, Decision, Quotas};
use vyasa_plugins::capabilities::Capability;
use vyasa_plugins::host::{HostLimits, TaskBudget};
use vyasa_testkit::TestDb;

#[test]
fn the_extended_limits_are_a_minute_and_twenty_billion_fuel() {
    let (fuel, ticks) = HostLimits::default().for_budget(TaskBudget::Extended);
    assert_eq!((fuel, ticks), (20_000_000_000, 1200));
    let (fuel, ticks) = HostLimits::default().for_budget(TaskBudget::Ordinary);
    assert_eq!((fuel, ticks), (1_000_000_000, 40));
}

#[test]
fn the_extended_quotas_are_larger_in_the_four_places_a_sync_needs() {
    let q = Quotas::extended();
    let d = Quotas::default();
    assert!(
        q.fetches_per_min > d.fetches_per_min
            && q.bytes_per_min > d.bytes_per_min
            && q.writes_per_min > d.writes_per_min
            && q.db_reads_per_min > d.db_reads_per_min
    );
    assert_eq!(
        q.ai_calls_per_min, d.ai_calls_per_min,
        "money is not part of a longer budget"
    );
}

#[tokio::test]
async fn an_extended_task_gets_a_longer_budget_only_with_the_capability() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let with = repo
        .create(
            "with-cap",
            "0.1.0",
            b"{}",
            "aa",
            &serde_json::json!(["tasks:extended", "net:fetch:api.github.com"]),
        )
        .await
        .unwrap();
    let without = repo
        .create(
            "without",
            "0.1.0",
            b"{}",
            "bb",
            &serde_json::json!(["net:fetch:api.github.com"]),
        )
        .await
        .unwrap();
    let broker = Broker::new(repo, pool);
    assert_eq!(
        broker
            .check_capability(with.id, &Capability::TasksExtended)
            .await,
        Decision::Allowed
    );
    assert_eq!(
        broker
            .check_capability(without.id, &Capability::TasksExtended)
            .await,
        Decision::Denied
    );
    // An extended task's call gets the larger fetch quota; an ordinary one the ordinary quota.
    for i in 0..100 {
        assert_eq!(
            broker
                .check_fetch(with.id, "https://api.github.com/x", true)
                .await,
            Decision::Allowed,
            "fetch {i}"
        );
    }
    let mut allowed = 0;
    for _ in 0..200 {
        if broker
            .check_fetch(without.id, "https://api.github.com/x", false)
            .await
            == Decision::Allowed
        {
            allowed += 1;
        }
    }
    assert_eq!(allowed, Quotas::default().fetches_per_min as usize);
}

/// A render made while the plugin's sync is running counts on the ordinary
/// budget: it neither gets the task's larger limits nor spends them.
#[tokio::test]
async fn an_ordinary_call_keeps_the_ordinary_fetch_quota_while_a_task_fetches() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let p = repo
        .create(
            "scoped",
            "0.1.0",
            b"{}",
            "aa",
            &serde_json::json!(["tasks:extended", "net:fetch:api.github.com"]),
        )
        .await
        .unwrap();
    let broker = Broker::new(repo, pool);
    let count = |task: bool| {
        let broker = broker.clone();
        async move {
            let mut allowed = 0;
            for _ in 0..400 {
                if broker
                    .check_fetch(p.id, "https://api.github.com/x", task)
                    .await
                    == Decision::Allowed
                {
                    allowed += 1;
                }
            }
            allowed
        }
    };
    // Wait for the start of a minute so both budgets are counted in one window.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    if secs % 60 > 50 {
        tokio::time::sleep(std::time::Duration::from_secs(61 - secs % 60)).await;
    }
    assert_eq!(
        count(true).await,
        Quotas::extended().fetches_per_min as usize
    );
    assert_eq!(
        count(false).await,
        Quotas::default().fetches_per_min as usize
    );
    // Each budget stays spent on its own.
    assert_eq!(count(true).await, 0);
}
