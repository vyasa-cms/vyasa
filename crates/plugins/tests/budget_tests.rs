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
fn the_extended_quotas_are_larger_in_the_three_places_a_sync_needs() {
    let q = Quotas::extended();
    let d = Quotas::default();
    assert!(
        q.fetches_per_min > d.fetches_per_min
            && q.bytes_per_min > d.bytes_per_min
            && q.writes_per_min > d.writes_per_min
    );
    assert_eq!(q.db_reads_per_min, d.db_reads_per_min);
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
    // Within the scope the larger fetch quota applies; outside it, the ordinary one.
    {
        let _scope = broker.extended_scope(with.id);
        for i in 0..100 {
            assert_eq!(
                broker
                    .check_fetch(with.id, "https://api.github.com/x")
                    .await,
                Decision::Allowed,
                "fetch {i}"
            );
        }
    }
    let mut allowed = 0;
    for _ in 0..200 {
        if broker
            .check_fetch(without.id, "https://api.github.com/x")
            .await
            == Decision::Allowed
        {
            allowed += 1;
        }
    }
    assert_eq!(allowed, Quotas::default().fetches_per_min as usize);
}

#[tokio::test]
async fn a_scope_ends_when_it_is_dropped() {
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
    let scope = broker.extended_scope(p.id);
    drop(scope);
    // The window may already hold requests; assert the *limit*, not the count: the 61st in one window is denied.
    let mut allowed = 0;
    for _ in 0..200 {
        if broker.check_fetch(p.id, "https://api.github.com/x").await == Decision::Allowed {
            allowed += 1;
        }
    }
    assert_eq!(allowed, Quotas::default().fetches_per_min as usize);
}
