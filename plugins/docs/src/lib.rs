//! `vyasa-docs`: documentation sites for Vyasa.
//!
//! Everything with logic is an ordinary Rust module with native unit tests
//! (`cargo test`); only `guest.rs`, compiled for `wasm32-wasip2`, touches the
//! host. The design is `docs/plugin-api.md` ("Content sync") and the README
//! beside this file.
