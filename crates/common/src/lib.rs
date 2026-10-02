//! Shared foundation for Vyasa: error taxonomy, configuration,
//! telemetry setup, and ID/slug generation.
//!
//! This crate has no dependencies on other Vyasa crates and contains no
//! business logic — it is the base of the dependency graph.

pub mod config;
pub mod error;
pub mod ids;
pub mod slug;
pub mod telemetry;

pub use config::{
    AiConfig, CdnConfig, DbConfig, JobsConfig, LogConfig, LogFormat, Secret, SmtpConfig,
    StorageConfig, VyasaConfig,
};
pub use error::{AppError, BoxError};
pub use ids::{next_id, next_id_i64, IdGen, WorkerId};
pub use slug::{slug_candidates, slugify, unique_slug};
