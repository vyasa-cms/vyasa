//! `now-ms` and `call-budget-ms`.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};
use vyasa_plugins::broker::Broker;
use vyasa_plugins::host::vyasa::plugin::host::Host as _;
use vyasa_plugins::host::{AiWindow, HostCtx};

fn ctx(deadline: Option<Instant>) -> HostCtx {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .unwrap();
    HostCtx {
        plugin_name: String::from("t"),
        plugin_id: 1,
        limits: None,
        wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
        table: wasmtime_wasi::ResourceTable::new(),
        broker: std::sync::Arc::new(Broker::new(
            vyasa_db::repo::PluginsRepo::new(pool.clone()),
            pool.clone(),
        )),
        dest: pool,
        ai: None,
        fetch: None,
        viewer: None,
        ai_window: AiWindow::Unbounded,
        call_deadline: deadline,
    }
}

#[tokio::test]
async fn now_ms_is_the_unix_clock() {
    let mut c = ctx(None);
    let a = c.now_ms().await;
    assert!(a > 1_700_000_000_000, "{a}");
    let b = c.now_ms().await;
    assert!(b >= a);
}

#[tokio::test]
async fn the_budget_counts_down_and_stops_at_zero() {
    let mut c = ctx(Some(Instant::now() + Duration::from_millis(1500)));
    let first = c.call_budget_ms().await;
    assert!((1000..=1500).contains(&first), "{first}");
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(c.call_budget_ms().await < first);
    let mut gone = ctx(Some(Instant::now() - Duration::from_secs(1)));
    assert_eq!(gone.call_budget_ms().await, 0);
    assert_eq!(ctx(None).call_budget_ms().await, 0);
}
