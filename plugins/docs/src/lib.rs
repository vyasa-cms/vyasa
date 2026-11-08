//! `vyasa-docs`: documentation sites for Vyasa.
//!
//! Everything with logic is an ordinary Rust module with native unit tests
//! (`cargo test`); only `guest.rs`, compiled for `wasm32-wasip2`, touches the
//! host. The design is `docs/plugin-api.md` ("Content sync") and the README
//! beside this file.
pub mod blocks;
pub mod components;
pub mod frontmatter;
pub mod github;
pub mod nav;
pub mod plan;
pub mod render;
pub mod routes;
pub mod sections;
pub mod settings;
pub mod sync;
pub mod webhook;

// wit-bindgen generates a function with ten parameters; the lint is about
// code we wrote, not about generated bindings.
#[cfg(target_os = "wasi")]
#[allow(clippy::too_many_arguments)]
mod guest;
