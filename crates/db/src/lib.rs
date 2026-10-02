//! Persistence layer for Vyasa: connection pool management, embedded
//! migrations, and health checks over PostgreSQL (sqlx).
//! Repos expose data access only; domain rules live in `vyasa-core`.

pub mod content_models;
pub mod health;
pub mod migrate;
pub mod models;
pub mod pool;
pub mod repo;

pub use health::{health, HealthReport};
pub use migrate::{pending_migrations, run_migrations, MIGRATOR};
pub use pool::{connect, connect_options, pool};
