//! The AI model registry: which providers are configured, which models are
//! registered for which kind of job, and which one of each kind is the
//! default that features reach for.
//!
//! Provider construction (HTTP clients) lives in `vyasa-ai`, which sits
//! above this crate; this module owns the data, the vocabulary and the
//! credential handling.

pub mod kinds;
pub mod secret;
pub mod service;

pub use kinds::{ModelKind, ProviderId, Recommended};
pub use secret::KeyVault;
pub use service::{AiModelsService, NewModelSpec, ProviderStatus, ResolvedModel};
