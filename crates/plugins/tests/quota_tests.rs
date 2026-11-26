//! Per-minute quotas: an extended task's reads apart from ordinary calls,
//! refusals that say "over quota" apart from "not granted", and the
//! operator's per-plugin overrides.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_common::plugin_quota::parse_plugin_quotas;
use vyasa_plugins::broker::{Broker, Gate, Quotas};
use vyasa_plugins::capabilities::Capability;
use vyasa_plugins::host::vyasa::plugin::host::Host as _;
use vyasa_plugins::host::{AiWindow, HostCtx, TaskBudget};
use vyasa_testkit::TestDb;

/// The broker counts per wall-clock minute; a count that crosses into the
/// next minute would see its budget renewed. Start well inside one.
async fn inside_one_minute() {
    loop {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            % 60;
        if secs < 50 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// One call's host context for `plugin_id`, under `budget`.
fn call(
    broker: &std::sync::Arc<Broker>,
    pool: &sqlx::PgPool,
    plugin_id: i64,
    budget: TaskBudget,
) -> HostCtx {
    HostCtx {
        plugin_name: String::from("t"),
        plugin_id,
        limits: None,
        wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
        table: wasmtime_wasi::ResourceTable::new(),
        broker: std::sync::Arc::clone(broker),
        dest: pool.clone(),
        ai: None,
        fetch: None,
        viewer: None,
        ai_window: AiWindow::Unbounded,
        call_deadline: None,
        budget,
    }
}

async fn plugin(repo: &vyasa_db::repo::PluginsRepo, name: &str, caps: serde_json::Value) -> i64 {
    repo.create(name, "0.1.0", b"{}", name, &caps)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn page_renders_cannot_spend_an_extended_tasks_reads() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let id = plugin(&repo, "docs", serde_json::json!(["kv:read"])).await;
    let broker = Broker::new(repo, pool);
    let ordinary = Quotas::default().db_reads_per_min;
    inside_one_minute().await;
    // Ordinary calls (page renders) spend their whole minute.
    for i in 0..ordinary {
        assert_eq!(
            broker.read_gate(id, Capability::KvRead, false).await,
            Gate::Allowed,
            "read {i}"
        );
    }
    assert_eq!(
        broker.read_gate(id, Capability::KvRead, false).await,
        Gate::OverQuota,
        "the ordinary budget is spent, and says so"
    );
    // The task still has its own, larger budget.
    for i in 0..Quotas::extended().db_reads_per_min {
        assert_eq!(
            broker.read_gate(id, Capability::KvRead, true).await,
            Gate::Allowed,
            "task read {i}"
        );
    }
    assert_eq!(
        broker.read_gate(id, Capability::KvRead, true).await,
        Gate::OverQuota
    );
}

#[tokio::test]
async fn a_refusal_names_a_missing_grant_apart_from_a_spent_quota() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let writer = plugin(&repo, "writer", serde_json::json!(["kv:read", "kv:write"])).await;
    let reader = plugin(&repo, "reader", serde_json::json!(["kv:read"])).await;
    let overrides = parse_plugin_quotas("*:writes=0").unwrap();
    let broker = Broker::new(repo, pool).with_overrides(overrides);
    // Granted, but no writes left this minute.
    assert_eq!(
        broker
            .private_write_gate(writer, &Capability::KvWrite)
            .await,
        Gate::OverQuota
    );
    // Not granted: said as such even with no writes left.
    assert_eq!(
        broker
            .private_write_gate(reader, &Capability::KvWrite)
            .await,
        Gate::NotGranted
    );
    assert_eq!(
        broker
            .read_gate(reader, Capability::DbReadComments, false)
            .await,
        Gate::NotGranted
    );
}

/// Every host read (`get-option`, `get-post`, `query-posts`, `kv-get`,
/// `kv-list`) counts on its own call's budget: an ordinary call made while
/// the plugin's extended task is running spends the ordinary budget, not
/// the task's, and a spent budget reads `quota-exceeded`.
#[tokio::test]
async fn host_reads_count_on_their_own_calls_budget() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let id = plugin(
        &repo,
        "docs",
        serde_json::json!(["db:read:options", "db:read:posts", "kv:read"]),
    )
    .await;
    let overrides = parse_plugin_quotas("docs:reads=3,task_reads=4").unwrap();
    let broker = std::sync::Arc::new(Broker::new(repo, pool.clone()).with_overrides(overrides));
    inside_one_minute().await;
    // The task is running: its scope is alive for the whole test.
    let _scope = broker.extended_scope(id);

    let mut render = call(&broker, &pool, id, TaskBudget::Ordinary);
    assert!(render.get_option("site_title".into()).await.is_ok());
    assert!(render.query_posts(None, 1).await.is_ok());
    assert!(render.kv_get("k".into()).await.is_ok());
    assert_eq!(
        render.get_option("site_title".into()).await,
        Err("quota-exceeded".to_owned()),
        "a render's spent budget says so, on get-option too"
    );
    assert_eq!(
        render.get_post(1).await.map(|_| ()),
        Err("quota-exceeded".to_owned())
    );

    // The task's four reads are all still there.
    let mut task = call(&broker, &pool, id, TaskBudget::Extended);
    assert!(task.kv_get("k".into()).await.is_ok());
    assert!(task.kv_list(String::new(), 10).await.is_ok());
    assert!(task.get_option("site_title".into()).await.is_ok());
    assert!(task.query_posts(None, 1).await.is_ok());
    assert_eq!(
        task.kv_get("k".into()).await,
        Err("quota-exceeded".to_owned())
    );
}

#[tokio::test]
async fn the_operator_raises_or_lowers_one_plugins_limits_by_name() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let caps = serde_json::json!(["kv:read", "kv:write"]);
    let docs = plugin(&repo, "vyasa-docs", caps.clone()).await;
    let other = plugin(&repo, "other", caps).await;
    let overrides =
        parse_plugin_quotas("*:reads=3,writes=2;vyasa-docs:reads=5,task_reads=7").unwrap();
    let broker = Broker::new(repo, pool).with_overrides(overrides);

    let allowed = |id: i64, task: bool| {
        let broker = broker.clone();
        async move {
            let mut n = 0;
            while broker.read_gate(id, Capability::KvRead, task).await == Gate::Allowed {
                n += 1;
                assert!(n < 10_000, "no limit applied");
            }
            n
        }
    };
    inside_one_minute().await;
    assert_eq!(allowed(docs, false).await, 5, "the named entry wins over *");
    assert_eq!(
        allowed(docs, true).await,
        7,
        "task_ sets the extended budget"
    );
    assert_eq!(allowed(other, false).await, 3, "* applies to every plugin");
    assert_eq!(
        allowed(other, true).await,
        Quotas::extended().db_reads_per_min,
        "a budget with no override keeps its default"
    );
    // `*:writes=2` reaches the vyasa-docs plugin too.
    for _ in 0..2 {
        assert_eq!(
            broker.private_write_gate(docs, &Capability::KvWrite).await,
            Gate::Allowed
        );
    }
    assert_eq!(
        broker.private_write_gate(docs, &Capability::KvWrite).await,
        Gate::OverQuota
    );
}
