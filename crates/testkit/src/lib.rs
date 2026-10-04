//! Test fixtures for the Vyasa workspace.
//!
//! [`TestDb`] gives each test its own Postgres database, cloned from a
//! template migrated once per migration set. [`TestServer`] runs the
//! `vyasa` binary against one. Both clean up on drop.
//!
//! Tests need `VYASA_TEST_DATABASE_URL`; without it they fail rather than
//! skip, so a green run always means the database tests ran.

#![allow(clippy::missing_panics_doc, clippy::expect_used)]

mod db;
mod server;

pub mod fixtures;
pub use db::{template_name, validate_admin_url, with_database, TestDb};
pub use server::{TestServer, TestServerBuilder};
