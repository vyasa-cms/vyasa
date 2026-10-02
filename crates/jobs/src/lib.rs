//! Background work for Vyasa: tokio-cron-scheduler based recurring jobs
//! and a Postgres-backed queue for media processing, emails, webhooks,
//! reindexing, and scheduled publishing.

#![allow(missing_docs)]

pub mod net_guard;
pub mod publisher;
pub mod queue;
pub mod registration;
pub mod runner;
pub mod workers;

pub use publisher::{publish_due_once, spawn as spawn_publisher, POLL_INTERVAL};
pub use registration::{purge_unconfirmed_once, PURGE_INTERVAL};
pub use runner::{HandlerFuture, JobHandler};
