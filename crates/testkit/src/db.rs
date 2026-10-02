//! A Postgres database per test, cloned from a migrated template.

use std::fmt::Write as _;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use tokio::sync::OnceCell;

/// The same SQL files the server runs, embedded here so the testkit need
/// not depend on `vyasa-db`.
static MIGRATOR: Migrator = sqlx::migrate!("../db/migrations");
static TEMPLATE: OnceCell<String> = OnceCell::const_new();

const ENV: &str = "VYASA_TEST_DATABASE_URL";
const TEST_PREFIX: &str = "vyasa_t_";
const TEMPLATE_PREFIX: &str = "vyasa_tmpl_";
/// Test databases older than this are leftovers of a crashed run.
const STALE_SECS: u64 = 60 * 60;
/// Advisory-lock key serialising template builds across processes.
const TEMPLATE_LOCK: i64 = 0x76_79_61_73_61_74_6b; // "vyasatk"
const HINT: &str = "run `scripts/test-with-db.sh`, or set VYASA_TEST_DATABASE_URL to a \
Postgres where this user may create databases (never the production `vyasa` database)";

/// A database of its own for one test, dropped when the value is.
pub struct TestDb {
    name: String,
    url: String,
    admin_url: String,
    pool: PgPool,
}

impl TestDb {
    /// Clones the migrated template into a new database and connects.
    pub async fn new() -> Self {
        let admin_url =
            validate_admin_url(std::env::var(ENV).ok()).unwrap_or_else(|e| panic!("{e}"));
        let template = TEMPLATE
            .get_or_init(|| build_template(admin_url.clone()))
            .await
            .clone();
        let name = format!("{TEST_PREFIX}{}_{:016x}", now_secs(), rand::random::<u64>());
        let mut admin = connect_admin(&admin_url).await;
        sqlx::query(&format!(
            r#"CREATE DATABASE "{}" TEMPLATE "{}""#,
            guarded(&name),
            guarded(&template)
        ))
        .execute(&mut admin)
        .await
        .unwrap_or_else(|e| panic!("creating test database {name}: {e}"));
        let _ = admin.close().await;
        let url = with_database(&admin_url, &name);
        // Small on purpose: see `TestServer::builder` for the suite's
        // connection budget against Postgres's limit.
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .unwrap_or_else(|e| panic!("connecting to {name}: {e}"));
        Self {
            name,
            url,
            admin_url,
            pool,
        }
    }

    /// Connection URL for this database (for a server subprocess).
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A pool on this database.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The database's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // The pool's sockets belong to the test's runtime, which may be
        // mid-shutdown; `WITH (FORCE)` ends those sessions server-side, so
        // the drop runs on a runtime of its own and never waits on them.
        let (admin_url, name) = (self.admin_url.clone(), self.name.clone());
        let joined = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(drop_database(&admin_url, &name));
        })
        .join();
        if joined.is_err() {
            eprintln!(
                "vyasa-testkit: dropping {} panicked; a later run sweeps it",
                self.name
            );
        }
    }
}

/// Checks the admin URL: present, non-empty, resolves to a database other
/// than the production `vyasa`, and carries no `dbname` query parameter.
///
/// # Errors
/// A message naming the variable and the fix.
#[doc(hidden)]
pub fn validate_admin_url(raw: Option<String>) -> Result<String, String> {
    let url = raw
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| format!("{ENV} is not set: {HINT}"))?;
    let options = PgConnectOptions::from_str(&url)
        .map_err(|e| format!("{ENV} is not a Postgres URL ({e}): {HINT}"))?;
    // With no explicit database in the URL, Postgres falls back to
    // PGDATABASE and then to the connecting user's name; a bare
    // `postgres://vyasa:vyasa@host:5432` would otherwise silently pass
    // this check and then land on a database named `vyasa`, so the check
    // has to resolve the name the same way before comparing.
    let effective_database = options
        .get_database()
        .map(str::to_owned)
        .or_else(|| std::env::var("PGDATABASE").ok())
        .unwrap_or_else(|| options.get_username().to_owned());
    if effective_database == "vyasa" {
        return Err(format!(
            "{ENV} resolves to the database `vyasa` (from the URL's path, PGDATABASE, or by \
             defaulting to the connection user's name), which is the production database's \
             name; tests create and drop databases on that server. {HINT}"
        ));
    }
    // sqlx applies a `dbname` query parameter after the URL's own path, so
    // `with_database`'s rewritten path would be silently overridden by one,
    // reconnecting every cloned database back to this admin database and
    // defeating per-test isolation.
    let parsed =
        url::Url::parse(&url).map_err(|e| format!("{ENV} is not a Postgres URL ({e}): {HINT}"))?;
    if parsed.query_pairs().any(|(k, _)| k == "dbname") {
        return Err(format!(
            "{ENV} sets a `dbname` query parameter, which Postgres reads after the URL's own \
             database path and would override every per-test database this crate creates; \
             put the database in the URL's path instead and drop `dbname` from the query \
             string. {HINT}"
        ));
    }
    Ok(url)
}

/// `url` with its database replaced by `name`, query string kept.
#[doc(hidden)]
#[must_use]
pub fn with_database(url: &str, name: &str) -> String {
    let mut parsed = url::Url::parse(url).expect("validated Postgres URL");
    parsed.set_path(&format!("/{name}"));
    parsed.to_string()
}

/// The template for the migrations compiled into this build.
#[doc(hidden)]
#[must_use]
pub fn template_name() -> String {
    let mut hash = Sha256::new();
    for m in MIGRATOR.iter() {
        hash.update(m.version.to_le_bytes());
        hash.update(&*m.checksum);
    }
    let digest = hash.finalize();
    let hex = digest.iter().take(8).fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    format!("{TEMPLATE_PREFIX}{hex}")
}

/// Builds the template once across all processes, then sweeps leftovers.
async fn build_template(admin_url: String) -> String {
    let name = template_name();
    let mut admin = connect_admin(&admin_url).await;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(TEMPLATE_LOCK)
        .execute(&mut admin)
        .await
        .expect("template lock");
    if !database_exists(&mut admin, &name).await {
        // Built under another name and renamed when complete, so a crash
        // mid-migration never leaves a template that looks finished.
        let building = format!("{name}_b");
        exec(
            &mut admin,
            &format!(
                r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#,
                guarded(&building)
            ),
        )
        .await;
        exec(
            &mut admin,
            &format!(r#"CREATE DATABASE "{}""#, guarded(&building)),
        )
        .await;
        let pool = PgPool::connect(&with_database(&admin_url, &building))
            .await
            .unwrap_or_else(|e| panic!("connecting to {building}: {e}"));
        MIGRATOR.run(&pool).await.unwrap_or_else(|e| {
            panic!("building the test template failed; a migration is broken: {e}")
        });
        pool.close().await;
        exec(
            &mut admin,
            &format!(
                r#"ALTER DATABASE "{}" RENAME TO "{}""#,
                guarded(&building),
                guarded(&name)
            ),
        )
        .await;
        exec(
            &mut admin,
            &format!(
                r#"ALTER DATABASE "{}" WITH ALLOW_CONNECTIONS false"#,
                guarded(&name)
            ),
        )
        .await;
    }
    sweep(&mut admin, &name).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(TEMPLATE_LOCK)
        .execute(&mut admin)
        .await;
    let _ = admin.close().await;
    name
}

/// Drops test databases from crashed runs and templates of older
/// migration sets.
async fn sweep(admin: &mut PgConnection, current_template: &str) {
    let names: Vec<String> = match sqlx::query_scalar("SELECT datname FROM pg_database")
        .fetch_all(&mut *admin)
        .await
    {
        Ok(names) => names,
        Err(e) => {
            eprintln!("vyasa-testkit: listing databases to sweep failed, skipping: {e}");
            return;
        }
    };
    let now = now_secs();
    for name in names {
        let stale_test = name
            .strip_prefix(TEST_PREFIX)
            .and_then(|rest| rest.split('_').next())
            .and_then(|secs| secs.parse::<u64>().ok())
            .is_some_and(|secs| now.saturating_sub(secs) > STALE_SECS);
        let old_template = name.starts_with(TEMPLATE_PREFIX) && name != current_template;
        if stale_test || old_template {
            let sql = format!(
                r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#,
                guarded(&name)
            );
            if let Err(e) = sqlx::query(&sql).execute(&mut *admin).await {
                eprintln!("vyasa-testkit: sweeping {name}: {e}");
            }
        }
    }
}

async fn drop_database(admin_url: &str, name: &str) {
    let Ok(mut admin) = PgConnection::connect(admin_url).await else {
        eprintln!("vyasa-testkit: cannot reach Postgres to drop {name}; a later run sweeps it");
        return;
    };
    let sql = format!(
        r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#,
        guarded(name)
    );
    if let Err(e) = sqlx::query(&sql).execute(&mut admin).await {
        eprintln!("vyasa-testkit: dropping {name}: {e}; a later run sweeps it");
    }
    let _ = admin.close().await;
}

async fn database_exists(admin: &mut PgConnection, name: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(name)
        .fetch_one(admin)
        .await
        .expect("query pg_database")
}

async fn exec(admin: &mut PgConnection, sql: &str) {
    sqlx::query(sql)
        .execute(admin)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn connect_admin(url: &str) -> PgConnection {
    PgConnection::connect(url)
        .await
        .unwrap_or_else(|e| panic!("cannot reach test Postgres at {}: {e}. {HINT}", masked(url)))
}

/// Every name the testkit creates or drops passes through here.
fn guarded(name: &str) -> &str {
    assert!(
        (name.starts_with(TEST_PREFIX) || name.starts_with(TEMPLATE_PREFIX))
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "vyasa-testkit refuses to touch database {name:?}"
    );
    name
}

fn masked(url: &str) -> String {
    url::Url::parse(url).map_or_else(
        |_| "<unparseable URL>".to_owned(),
        |mut u| {
            if u.password().is_some() {
                let _ = u.set_password(Some("***"));
            }
            u.to_string()
        },
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
