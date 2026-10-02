//! The text-model gate: `ai:text` is granted per plugin and budgeted per
//! minute, and the render-path gates answer quietly.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_plugins::broker::{Broker, Decision, Quotas};
use vyasa_plugins::capabilities::Capability;
use vyasa_testkit::TestDb;

struct Env {
    _db: TestDb,
    pool: sqlx::PgPool,
    broker: Broker,
    ai: i64,
    plain: i64,
}

async fn setup() -> Env {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let ai = repo
        .create(
            "asker",
            "0.1.0",
            b"{}",
            "aa",
            &serde_json::json!(["ai:text", "html:page"]),
        )
        .await
        .expect("create");
    let plain = repo
        .create(
            "plain",
            "0.1.0",
            b"{}",
            "bb",
            &serde_json::json!(["log:write"]),
        )
        .await
        .expect("create plain");
    Env {
        broker: Broker::new(repo, pool.clone()),
        _db: db,
        pool,
        ai: ai.id,
        plain: plain.id,
    }
}

async fn quota_rows(env: &Env, plugin_id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM plugin_audit
         WHERE plugin_id = $1 AND kind = 'quota' AND capability = 'ai:text'",
    )
    .bind(plugin_id)
    .fetch_one(&env.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn ai_calls_are_budgeted_per_plugin_per_minute() {
    let env = setup().await;
    let budget = Quotas::default().ai_calls_per_min;
    assert!(budget > 0 && budget <= 60, "a budget, not a formality");

    // A minute boundary mid-loop would reset the window and hand out a
    // second budget; retry once so the test is not flaky at :59.
    let mut granted = 0;
    for _ in 0..2 {
        let fresh = Broker::new(
            vyasa_db::repo::PluginsRepo::new(env.pool.clone()),
            env.pool.clone(),
        );
        granted = 0;
        for _ in 0..budget + 5 {
            if fresh.check_ai(env.ai).await == Decision::Allowed {
                granted += 1;
            }
        }
        if granted == budget {
            break;
        }
    }
    assert_eq!(granted, budget, "exactly the budget, then refusals");
    assert!(quota_rows(&env, env.ai).await >= 5, "each refusal audited");

    // The budget is the plugin's own: a plugin without the grant is
    // refused for the grant, never for someone else's spending.
    assert_eq!(env.broker.check_ai(env.plain).await, Decision::Denied);
    assert_eq!(quota_rows(&env, env.plain).await, 0);
    env.pool.close().await;
}

#[tokio::test]
async fn holds_answers_without_auditing() {
    let env = setup().await;
    assert!(env.broker.holds(env.ai, &Capability::HtmlPage).await);
    for _ in 0..3 {
        assert!(!env.broker.holds(env.plain, &Capability::HtmlPage).await);
        assert!(!env.broker.holds(env.plain, &Capability::AssetsScript).await);
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM plugin_audit WHERE plugin_id = $1")
        .bind(env.plain)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "a render-path gate must not write a row per page");
    env.pool.close().await;
}

mod ai_complete {
    use super::*;
    use vyasa_plugins::host::vyasa::plugin::host::Host as _;
    use vyasa_plugins::host::{AiBackend, AiWindow, HostCtx};

    /// A model that answers after `delay`.
    struct Slow(std::time::Duration);

    impl AiBackend for Slow {
        fn complete<'a>(
            &'a self,
            _plugin_name: &'a str,
            _system: String,
            _user: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
        {
            let delay = self.0;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Ok(String::from("late"))
            })
        }
    }

    fn ctx(broker: Broker, pool: sqlx::PgPool, plugin_id: i64, window: AiWindow) -> HostCtx {
        HostCtx {
            plugin_name: String::from("asker"),
            plugin_id,
            limits: None,
            wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
            table: wasmtime_wasi::ResourceTable::new(),
            broker: std::sync::Arc::new(broker),
            dest: pool,
            ai: Some(std::sync::Arc::new(Slow(std::time::Duration::from_secs(5)))),
            fetch: None,
            viewer: None,
            ai_window: window,
        }
    }

    fn lazy_pool() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap()
    }

    #[tokio::test]
    async fn page_html_may_not_call_the_model_at_all() {
        let pool = lazy_pool();
        let broker = Broker::new(vyasa_db::repo::PluginsRepo::new(pool.clone()), pool.clone());
        let mut c = ctx(broker, pool, 1, AiWindow::Forbidden);
        let err = c
            .ai_complete(String::from("s"), String::from("u"))
            .await
            .unwrap_err();
        assert!(err.contains("page-html"), "{err}");
    }

    #[tokio::test]
    async fn the_system_prompt_is_capped_too() {
        let pool = lazy_pool();
        let broker = Broker::new(vyasa_db::repo::PluginsRepo::new(pool.clone()), pool.clone());
        let mut c = ctx(broker, pool, 1, AiWindow::Unbounded);
        let long = "x".repeat(vyasa_plugins::host::MAX_AI_SYSTEM_CHARS + 1);
        let err = c.ai_complete(long, String::from("u")).await.unwrap_err();
        assert_eq!(err, "system prompt too long");
        let long = "x".repeat(vyasa_plugins::host::MAX_AI_PROMPT_CHARS + 1);
        let err = c.ai_complete(String::from("s"), long).await.unwrap_err();
        assert_eq!(err, "prompt too long");
    }

    #[tokio::test]
    async fn a_render_path_call_cannot_hold_the_page() {
        let env = setup().await;
        let mut c = ctx(
            env.broker.clone(),
            env.pool.clone(),
            env.ai,
            AiWindow::Render,
        );
        let started = std::time::Instant::now();
        let err = c
            .ai_complete(String::from("s"), String::from("u"))
            .await
            .unwrap_err();
        assert_eq!(err, "ai-complete timed out");
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        env.pool.close().await;
    }
}
