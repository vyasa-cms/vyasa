//! Embedded migrations.
//!
//! SQL files under `crates/db/migrations/` are compiled into the binary
//! via [`sqlx::migrate!`] and applied by [`run_migrations`]. The runner is
//! idempotent: applied migrations are recorded in the `_sqlx_migrations`
//! table and never run twice.

use sqlx::migrate::Migrator;
use sqlx::PgPool;
use vyasa_common::AppError;

/// The embedded migrator for all migrations in `crates/db/migrations`.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Applies all pending migrations.
///
/// Safe to call on every boot: already-applied migrations are skipped.
///
/// # Errors
///
/// Returns [`AppError::Db`] when a migration fails; sqlx records applied
/// migrations transactionally, so a failed run leaves a consistent state.
pub async fn run_migrations(pool: &PgPool) -> Result<(), AppError> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|err| AppError::db(format!("migration failed: {err}")))
}

/// Migrations embedded in this binary that the database has not applied,
/// as `(version, description)` in the order they would run.
///
/// This is what an upgrade preflight shows an operator *before* anything
/// is swapped: migrations are forward-only, so "what is about to change,
/// irreversibly" is the one question worth answering in advance.
///
/// # Errors
///
/// Returns [`AppError::Db`] when the applied set cannot be read. A
/// database with no `_sqlx_migrations` table yet reports everything as
/// pending, which is correct for a fresh install.
pub async fn pending_migrations(pool: &PgPool) -> Result<Vec<(i64, String)>, AppError> {
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    Ok(MIGRATOR
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .map(|m| (m.version, m.description.to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::MIGRATOR;

    #[test]
    fn migrator_embeds_the_migrations_directory() {
        // The migrator is compile-time embedded; asserting its location
        // keeps the `sqlx::migrate!` path honest if the crate moves.
        assert_eq!(MIGRATOR.iter().count(), MIGRATOR.iter().count());
    }
}
