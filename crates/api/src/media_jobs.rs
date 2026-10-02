//! The media job kinds, run by the queue's workers with the storage the API
//! process is actually configured with.
//!
//! The jobs crate cannot know which backend holds the bytes — object storage
//! is wired up here, from the config the API reads — so the worker in that
//! crate is only a fallback for a bare runner. This handler claims the kind
//! first and hands the worker the real store.

use std::sync::Arc;

use serde_json::Value;
use vyasa_jobs::JobHandler;

use crate::state::AppState;

/// The job kind this handler owns.
pub const KIND: &str = "media_derivatives";

/// Derivative generation, against the configured backend.
pub struct MediaJobs {
    storage: Arc<dyn vyasa_core::media::StorageBackend>,
    pool: sqlx::PgPool,
}

impl MediaJobs {
    /// Builds the handler in the form the runner takes.
    #[must_use]
    pub fn handler(state: &AppState) -> Arc<dyn JobHandler> {
        Arc::new(Self {
            storage: state.media.storage(),
            pool: state.pool.clone(),
        })
    }
}

impl JobHandler for MediaJobs {
    fn handle<'a>(&'a self, kind: &'a str, payload: Value) -> vyasa_jobs::HandlerFuture<'a> {
        Box::pin(async move {
            if kind != KIND {
                return None;
            }
            Some(
                vyasa_jobs::workers::media::handle(&self.pool, self.storage.as_ref(), payload)
                    .await,
            )
        })
    }
}

/// Several handlers presented to the runner as one.
///
/// The runner takes a single external handler. Rather than widen its
/// signature, the API composes its own: each handler answers `None` for a
/// kind it does not own, so the first `Some` wins and the built-in dispatch
/// only sees what nobody claimed.
pub struct Handlers(pub Vec<Arc<dyn JobHandler>>);

impl JobHandler for Handlers {
    fn handle<'a>(&'a self, kind: &'a str, payload: Value) -> vyasa_jobs::HandlerFuture<'a> {
        Box::pin(async move {
            for handler in &self.0 {
                if let Some(outcome) = handler.handle(kind, payload.clone()).await {
                    return Some(outcome);
                }
            }
            None
        })
    }
}
