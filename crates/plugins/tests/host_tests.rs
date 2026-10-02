//! Host runtime tests. The `load_and_init` test requires the compiled
//! hello fixture (built from plugin-sdk/template); see FIXTURE_PATH docs.

#![allow(clippy::pedantic, clippy::unwrap_used)]

use vyasa_plugins::host::{sha256_hex, HostLimits, InvokeOutcome};
use vyasa_testkit::TestDb;

#[test]
fn sha256_is_stable() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn outcome_failed_carries_message() {
    match InvokeOutcome::Failed(String::from("fuel")) {
        InvokeOutcome::Failed(m) => assert_eq!(m, "fuel"),
        InvokeOutcome::Ok(_) => panic!("unexpected ok"),
    }
}

// --- Full runtime path (env + fixture gated) ---------------------------------

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/hello/hello-component.wasm"
);

/// # Panics
/// If the wasm fixture at [`FIXTURE`] is missing: build it as described in
/// `plugin-sdk/examples/hello/README.md`.
#[tokio::test]
async fn hello_loads_from_db_and_init_logs() {
    let bytes = std::fs::read(FIXTURE).unwrap_or_else(|e| {
        panic!(
            "fixture missing at {FIXTURE} ({e}); from plugin-sdk/template run `cargo build \
             --target wasm32-wasip2 && cp target/wasm32-wasip2/debug/vyasa_plugin_template.wasm \
             .` (see plugin-sdk/examples/hello/README.md)"
        )
    });

    let db = TestDb::new().await;
    let pool = db.pool().clone();

    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let sha = sha256_hex(&bytes);
    repo.create("hello", "0.1.0", &bytes, &sha, &serde_json::json!([]))
        .await
        .expect("create");
    repo.set_enabled(
        sqlx::query_scalar::<_, i64>("SELECT id FROM plugins LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("id"),
        true,
    )
    .await
    .expect("enable");

    let host = vyasa_plugins::host::WasmtimeHost::with_limits(
        repo.clone(),
        HostLimits {
            fuel_per_call: 10_000_000,
            ..Default::default()
        },
    )
    .expect("engine");
    let plugin_id = sqlx::query_scalar::<_, i64>("SELECT id FROM plugins LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();

    let env = vyasa_plugins::host::HostEnv::background(
        std::sync::Arc::new(vyasa_plugins::broker::Broker::new(
            repo.clone(),
            pool.clone(),
        )),
        pool.clone(),
    );

    // Disabled plugins are refused.
    repo.set_enabled(plugin_id, false).await.unwrap();
    assert!(
        host.load(plugin_id, &env).await.is_err(),
        "disabled must refuse load"
    );
    repo.set_enabled(plugin_id, true).await.unwrap();

    // init runs and accepts config; status becomes loaded.
    let result = host.call_init(plugin_id, r#"{"name":"hello"}"#, &env).await;
    match result {
        Ok(Ok(())) => {}
        other => panic!("init failed: {other:?}"),
    }
    pool.close().await;
}

// --- Broker: allow/deny/audit/quota matrix -----------------------------------

#[tokio::test]
async fn broker_matrix_no_caps_denies_and_audits() {
    use vyasa_plugins::broker::{Broker, Decision};
    use vyasa_plugins::capabilities::Capability;

    let db = TestDb::new().await;
    let pool = db.pool().clone();

    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    // Plugin with NO capabilities.
    let row = repo
        .create("nocaps", "0.1.0", b"{}", "deadbeef", &serde_json::json!([]))
        .await
        .expect("create");

    let broker = Broker::new(repo.clone(), pool.clone());

    // db read denied.
    assert_eq!(
        broker.check_db_read(row.id, Capability::DbReadPosts).await,
        Decision::Denied
    );
    // options read denied.
    assert_eq!(
        broker
            .check_db_read(row.id, Capability::DbReadOptions)
            .await,
        Decision::Denied
    );
    // fetch denied for any host.
    assert_eq!(
        broker.check_fetch(row.id, "https://a.example.com/x").await,
        Decision::Denied
    );
    // event emit denied.
    assert_eq!(broker.check_event_emit(row.id).await, Decision::Denied);

    // Audit rows written for every denial.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM plugin_audit WHERE plugin_id = $1 AND kind = 'deny'",
    )
    .bind(row.id)
    .fetch_one(&pool)
    .await
    .expect("audit count");
    assert_eq!(n, 4, "four denials audited");
    pool.close().await;
}

#[tokio::test]
async fn broker_grants_allow_and_quota_exhausts() {
    use vyasa_plugins::broker::{Broker, Decision};
    use vyasa_plugins::capabilities::Capability;

    let db = TestDb::new().await;
    let pool = db.pool().clone();

    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let row = repo
        .create(
            "fullcaps",
            "0.1.0",
            b"{}",
            "cafebabe",
            &serde_json::json!(["db:read:posts", "net:fetch:*.example.com", "event:emit"]),
        )
        .await
        .expect("create");

    let broker = Broker::new(repo.clone(), pool.clone());

    // Granted caps allowed; net pattern allows a/b.example.com, denies evil.com.
    assert_eq!(
        broker.check_db_read(row.id, Capability::DbReadPosts).await,
        Decision::Allowed
    );
    assert_eq!(
        broker.check_fetch(row.id, "https://a.example.com/x").await,
        Decision::Allowed
    );
    assert_eq!(
        broker.check_fetch(row.id, "https://b.example.com/y").await,
        Decision::Allowed
    );
    assert_eq!(
        broker.check_fetch(row.id, "https://evil.com/z").await,
        Decision::Denied
    );
    assert_eq!(broker.check_event_emit(row.id).await, Decision::Allowed);

    // Byte quota: record_bytes over budget returns false.
    assert!(broker.record_bytes(row.id, 100).await);
    assert!(!broker.record_bytes(row.id, 10u64 * 1024 * 1024).await);

    // Quota rows recorded as kind='quota' eventually; here we only assert
    // fetch audit entries exist (3 calls: 2 allowed + 1 denied).
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM plugin_audit WHERE plugin_id = $1 AND kind = 'fetch'",
    )
    .bind(row.id)
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(n, 2, "two allowed fetches audited");
    let denies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM plugin_audit WHERE plugin_id = $1 \
         AND kind = 'deny' AND capability LIKE 'net:fetch%'",
    )
    .bind(row.id)
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(denies, 1, "evil.com denial audited as net:fetch deny");
    pool.close().await;
}
