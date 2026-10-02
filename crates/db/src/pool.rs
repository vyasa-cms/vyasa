//! Connection pool construction.
//!
//! Pools are built from [`VyasaConfig`] (or a raw URL for tests and
//! tooling) with the sizes and timeouts configured in
//! [`vyasa_common::DbConfig`].

use std::str::FromStr;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use vyasa_common::{AppError, DbConfig, VyasaConfig};

/// Builds [`PgConnectOptions`] from a `postgres://` URL.
///
/// # Errors
///
/// Returns [`AppError::Validation`] when the URL cannot be parsed.
pub fn connect_options(url: &str) -> Result<PgConnectOptions, AppError> {
    PgConnectOptions::from_str(url)
        .map_err(|err| AppError::validation(format!("invalid database_url: {err}")))
}

/// Connects a pool to `url` using the sizes/timeouts from `db`.
///
/// # Errors
///
/// Returns [`AppError::Db`] when the connection cannot be established
/// within the configured timeout.
pub async fn connect(url: &str, db: &DbConfig) -> Result<PgPool, AppError> {
    let options = connect_options(url)?;
    // `acquire_timeout` bounds the whole acquire path, including
    // establishing new physical connections.
    PgPoolOptions::new()
        .max_connections(db.max_connections)
        .acquire_timeout(Duration::from_secs(db.acquire_timeout_secs))
        .connect_with(options)
        .await
        .map_err(|err| {
            // The sqlx error can embed the DSN; log only its kind + message.
            AppError::db(format!("failed to connect to database: {err}"))
        })
}

/// Connects a pool from the application configuration.
///
/// # Errors
///
/// Returns [`AppError::Db`] when the connection cannot be established.
pub async fn pool(config: &VyasaConfig) -> Result<PgPool, AppError> {
    connect(config.database_url.expose(), &config.db).await
}
