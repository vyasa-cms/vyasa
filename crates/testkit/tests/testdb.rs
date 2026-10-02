//! The testkit's own guarantees. These need VYASA_TEST_DATABASE_URL.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use sqlx::{Connection, PgConnection};
use vyasa_testkit::{template_name, validate_admin_url, with_database, TestDb};

async fn admin() -> PgConnection {
    let url = validate_admin_url(std::env::var("VYASA_TEST_DATABASE_URL").ok()).unwrap();
    PgConnection::connect(&url).await.unwrap()
}

async fn exists(name: &str) -> bool {
    let mut conn = admin().await;
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(name)
        .fetch_one(&mut conn)
        .await
        .unwrap()
}

#[test]
fn missing_or_empty_url_is_an_error_naming_the_fix() {
    for raw in [None, Some(String::new())] {
        let err = validate_admin_url(raw).unwrap_err();
        assert!(err.contains("VYASA_TEST_DATABASE_URL"), "{err}");
        assert!(err.contains("scripts/test-with-db.sh"), "{err}");
    }
}

#[test]
fn the_production_database_name_is_refused() {
    let err = validate_admin_url(Some("postgres://u:p@localhost:5432/vyasa".into())).unwrap_err();
    assert!(err.contains("production"), "{err}");
    assert!(validate_admin_url(Some("postgres://u:p@localhost:5432/vyasa_test".into())).is_ok());
}

#[test]
fn a_url_with_no_database_path_that_defaults_to_the_production_name_is_refused() {
    // No explicit database and no PGDATABASE in this process's env: Postgres
    // defaults the database to the connecting user's name, so `vyasa` here
    // resolves to the production database just as surely as an explicit path.
    let err = validate_admin_url(Some("postgres://vyasa:vyasa@localhost:5432".into())).unwrap_err();
    assert!(err.contains("production"), "{err}");
    assert!(
        validate_admin_url(Some("postgres://other:pw@localhost:5432/vyasa_test".into())).is_ok()
    );
}

#[test]
fn a_dbname_query_parameter_is_refused() {
    // Postgres (and sqlx) apply `dbname` after the URL's own path, so
    // `with_database` rewriting the path would silently be overridden by
    // this, reconnecting every clone to the admin database instead of its
    // own.
    let err = validate_admin_url(Some(
        "postgres://u:p@localhost:5432/vyasa_test?dbname=vyasa_test".into(),
    ))
    .unwrap_err();
    assert!(err.contains("dbname"), "{err}");
}

#[test]
fn with_database_keeps_query() {
    let url = with_database(
        "postgres://u:p@h:5432/vyasa_test?sslmode=disable",
        "vyasa_t_1_ab",
    );
    assert_eq!(url, "postgres://u:p@h:5432/vyasa_t_1_ab?sslmode=disable");
}

#[test]
fn template_name_tracks_migrations() {
    let name = template_name();
    assert!(name.starts_with("vyasa_tmpl_"), "{name}");
    assert_eq!(name.len(), "vyasa_tmpl_".len() + 16);
    assert_eq!(name, template_name(), "stable within a build");
}

#[tokio::test]
async fn each_test_db_is_isolated_and_migrated() {
    let a = TestDb::new().await;
    let b = TestDb::new().await;
    assert_ne!(a.name(), b.name());
    sqlx::query("CREATE TABLE only_in_a (x int)")
        .execute(a.pool())
        .await
        .unwrap();
    let in_b: bool = sqlx::query_scalar("SELECT to_regclass('only_in_a') IS NOT NULL")
        .fetch_one(b.pool())
        .await
        .unwrap();
    assert!(!in_b);
    let users: bool = sqlx::query_scalar("SELECT to_regclass('users') IS NOT NULL")
        .fetch_one(b.pool())
        .await
        .unwrap();
    assert!(users, "clone carries the migrated schema");
}

#[tokio::test]
async fn the_database_is_gone_after_drop() {
    let db = TestDb::new().await;
    let name = db.name().to_owned();
    assert!(exists(&name).await);
    drop(db);
    assert!(!exists(&name).await);
}

#[tokio::test]
async fn drop_runs_on_panic() {
    let name = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen = name.clone();
    let handle = tokio::spawn(async move {
        let db = TestDb::new().await;
        *seen.lock().unwrap() = db.name().to_owned();
        panic!("simulated test failure");
    });
    assert!(handle.await.is_err());
    let name = name.lock().unwrap().clone();
    assert!(!name.is_empty());
    assert!(!exists(&name).await, "{name} left behind after a panic");
}

#[test]
fn concurrent_first_use_in_one_process_shares_one_template() {
    // Four OS threads, each with its own runtime, race `TestDb::new()` on
    // its first call in this process. They all share the process-global
    // `TEMPLATE` `OnceCell`, so this proves that cell serialises the race
    // within one process; it does not exercise the cross-process
    // advisory-lock path (that needs separate processes, not threads,
    // and a cold cache no single test binary can produce for itself).
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        let db = TestDb::new().await;
                        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations")
                            .fetch_one(db.pool())
                            .await
                            .unwrap()
                    })
            })
        })
        .collect();
    let counts: Vec<i64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(
        counts.iter().all(|c| *c == counts[0] && *c > 40),
        "{counts:?}"
    );

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let template = template_name();
            assert!(exists(&template).await, "{template} missing after the race");
            let building = format!("{template}_b");
            assert!(
                !exists(&building).await,
                "{building} (the in-progress build name) left behind after the race"
            );
        });
}
