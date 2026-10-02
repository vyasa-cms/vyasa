//! Turns domain events into AI jobs, according to the operator's switches:
//! a published or updated post is embedded (and, on publish, gets its empty
//! fields filled and its audio version), a new comment is screened.

use serde_json::json;
use vyasa_core::events::{self, Event};

use crate::ai_features::{AiSettings, Screening};
use crate::ai_jobs::enqueue;
use crate::state::AppState;

pub fn spawn(state: &AppState) {
    let state = state.clone();
    let mut rx = events::subscribe();
    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("AI event subscriber lagged {n} events");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            let settings = AiSettings::load(&state).await;
            match event {
                Event::Published(e) => {
                    if settings.embeddings {
                        enqueue(&state, "ai_embed_post", json!({ "post_id": e.post_id })).await;
                    }
                    if settings.autofill {
                        enqueue(&state, "ai_autofill_post", json!({ "post_id": e.post_id })).await;
                    }
                    if settings.read_aloud {
                        enqueue(&state, "ai_read_aloud", json!({ "post_id": e.post_id })).await;
                    }
                }
                Event::Updated(e) => {
                    if settings.embeddings {
                        enqueue(&state, "ai_embed_post", json!({ "post_id": e.post_id })).await;
                    }
                }
                Event::Trashed(e) => {
                    if settings.embeddings {
                        enqueue(&state, "ai_embed_post", json!({ "post_id": e.post_id })).await;
                    }
                }
                Event::Deleted(e) => {
                    if settings.embeddings {
                        enqueue(&state, "ai_embed_post", json!({ "post_id": e.post_id })).await;
                    }
                }
                Event::CommentAdded(e) if settings.comment_screening != Screening::Off => {
                    enqueue(
                        &state,
                        "ai_moderate_comment",
                        json!({ "comment_id": e.comment_id }),
                    )
                    .await;
                }
                _ => {}
            }
        }
    });
}
