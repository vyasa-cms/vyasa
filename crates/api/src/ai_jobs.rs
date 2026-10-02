//! The AI job kinds, run by the queue's workers through the runner's
//! external handler. Each job is one feature call from `ai_features`;
//! failures propagate so the queue retries with backoff and dead-letters.

use std::sync::Arc;

use serde_json::Value;
use vyasa_jobs::JobHandler;

use crate::ai_features::{self, AiSettings, Screening};
use crate::state::AppState;

/// Job kinds this handler owns.
pub const KINDS: &[&str] = &[
    "ai_alt_text",
    "ai_moderate_comment",
    "ai_autofill_post",
    "ai_embed_post",
    "ai_transcribe_media",
    "ai_read_aloud",
    crate::theme_assistant::JOB_KIND,
];

/// Queues a job and wakes the workers.
pub async fn enqueue(state: &AppState, kind: &str, payload: Value) {
    match vyasa_jobs::queue::enqueue(&state.pool, kind, payload, None).await {
        Ok(_) => state.publisher_notify.notify_one(),
        Err(err) => tracing::warn!(kind, "could not queue AI job: {err}"),
    }
}

/// The handler handed to the job runner.
pub struct AiJobs {
    state: AppState,
}

impl AiJobs {
    /// Builds the handler in the form the runner takes.
    #[must_use]
    pub fn handler(state: AppState) -> Arc<dyn JobHandler> {
        Arc::new(Self { state })
    }

    async fn run(&self, kind: &str, payload: Value) -> Result<(), String> {
        let id = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_i64)
                .ok_or_else(|| format!("missing {key}"))
        };
        let state = &self.state;
        let outcome = match kind {
            "ai_alt_text" => ai_features::alt_text(state, id("media_id")?, false)
                .await
                .map(|_| ()),
            "ai_moderate_comment" => {
                let mode = AiSettings::load(state).await.comment_screening;
                if mode == Screening::Off {
                    return Ok(());
                }
                ai_features::moderate_comment(state, id("comment_id")?, mode)
                    .await
                    .map(|_| ())
            }
            "ai_autofill_post" => {
                if !AiSettings::load(state).await.autofill {
                    return Ok(());
                }
                ai_features::autofill_post(state, id("post_id")?)
                    .await
                    .map(|_| ())
            }
            "ai_embed_post" => {
                if !AiSettings::load(state).await.embeddings {
                    return Ok(());
                }
                ai_features::embed_post(state, id("post_id")?)
                    .await
                    .map(|_| ())
            }
            "ai_transcribe_media" => ai_features::transcribe_media(state, id("media_id")?)
                .await
                .map(|_| ()),
            "ai_read_aloud" => ai_features::read_aloud(state, id("post_id")?)
                .await
                .map(|_| ()),
            crate::theme_assistant::JOB_KIND => {
                crate::theme_assistant::run(
                    state,
                    id("draft_id")?,
                    id("message_id")?,
                    &crate::theme_assistant::payload_ids(&payload),
                )
                .await
            }
            other => return Err(format!("not an AI job: {other}")),
        };
        outcome.map_err(|e| e.to_string())
    }
}

impl JobHandler for AiJobs {
    fn handle<'a>(&'a self, kind: &'a str, payload: Value) -> vyasa_jobs::HandlerFuture<'a> {
        Box::pin(async move {
            if !KINDS.contains(&kind) {
                return None;
            }
            Some(self.run(kind, payload).await)
        })
    }
}
