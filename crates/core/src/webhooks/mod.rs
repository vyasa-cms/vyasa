//! Outbound webhooks: HMAC-signed delivery over the job queue.

pub mod service;

pub use service::{WebhookEvent, WebhookService, SIGNATURE_HEADER};
