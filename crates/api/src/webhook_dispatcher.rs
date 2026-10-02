//! Turns domain events into queued webhook deliveries.
//!
//! `WebhookService::fan_out` has always been able to resolve subscribers and
//! sign a payload, and the queue has always been able to retry a failing
//! job, but nothing connected the two: no code path called `fan_out`, so a
//! subscription only ever fired from the manual `/test` endpoint.
//!
//! Delivery is queued rather than performed inline so a slow or dead
//! receiver cannot hold up the request that published the post, and so
//! retries survive a restart.

use vyasa_core::events::{self, Event};
use vyasa_core::webhooks::service::{WebhookEvent, WebhookService};

use crate::state::AppState;

/// Maps a domain event to its public webhook name and payload.
///
/// `None` means the event is not part of the v1 webhook contract.
fn subscribable(event: &Event) -> Option<(WebhookEvent, serde_json::Value)> {
    match event {
        Event::Published(p) => Some((
            WebhookEvent::PostPublished,
            serde_json::json!({ "post_id": p.post_id }),
        )),
        Event::Updated(p) => Some((
            WebhookEvent::PostUpdated,
            serde_json::json!({ "post_id": p.post_id }),
        )),
        Event::Trashed(p) => Some((
            WebhookEvent::PostTrashed,
            serde_json::json!({ "post_id": p.post_id }),
        )),
        Event::CommentAdded(c) => Some((
            WebhookEvent::CommentAdded,
            serde_json::json!({ "comment_id": c.comment_id, "post_id": c.post_id }),
        )),
        // `theme.changed` has no bus event: activation is an admin action,
        // not a content lifecycle transition, so it is fanned out directly
        // from the activate handler via `deliver_now`.
        _ => None,
    }
}

/// Subscribes to the domain event bus and enqueues signed deliveries for
/// the life of the process.
pub fn spawn(state: &AppState) {
    let service = WebhookService::new(vyasa_db::repo::WebhooksRepo::new(state.pool.clone()));
    let pool = state.pool.clone();
    let notify = state.publisher_notify.clone();
    let metrics = std::sync::Arc::clone(&state.metrics);
    let mut rx = events::subscribe();

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                // Lagged loses deliveries. There is nothing honest to do
                // about it after the fact — the events are gone — so say so
                // loudly and keep serving the ones that remain.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("webhook dispatcher lagged {n} events; deliveries were lost");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            let Some((kind, data)) = subscribable(&event) else {
                continue;
            };
            match enqueue_all(&pool, &service, kind, data).await {
                Ok(queued) => {
                    for _ in 0..queued {
                        metrics.inc_webhook_delivery();
                    }
                }
                Err(err) => tracing::warn!("webhook fan-out failed: {err}"),
            }
            // Wake a worker rather than waiting for the poll interval.
            notify.notify_one();
        }
    });
}

async fn enqueue_all(
    pool: &sqlx::PgPool,
    service: &WebhookService,
    kind: WebhookEvent,
    data: serde_json::Value,
) -> Result<usize, vyasa_common::AppError> {
    let name = kind.as_str();
    let mut queued = 0usize;
    for (webhook_id, url, signature, body) in service.fan_out(name, data).await? {
        let payload = serde_json::json!({
            "webhook_id": webhook_id,
            "event": name,
            "url": url,
            "signature": signature,
            "body": body,
        });
        vyasa_jobs::queue::enqueue(pool, "webhook_deliver", payload, None).await?;
        queued += 1;
    }
    Ok(queued)
}

/// Fans one event out immediately, outside the bus.
///
/// For events that are not post lifecycle transitions — theme activation is
/// the only one today — and so have no place on the domain event bus.
///
/// # Errors
/// Propagates repository and queue failures.
pub async fn deliver_now(
    state: &AppState,
    kind: WebhookEvent,
    data: serde_json::Value,
) -> Result<(), vyasa_common::AppError> {
    let service = WebhookService::new(vyasa_db::repo::WebhooksRepo::new(state.pool.clone()));
    let queued = enqueue_all(&state.pool, &service, kind, data).await?;
    for _ in 0..queued {
        state.metrics.inc_webhook_delivery();
    }
    state.publisher_notify.notify_one();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::subscribable;
    use vyasa_core::events::Event;

    #[test]
    fn published_maps_to_the_public_event_name() {
        let ev = Event::Published(vyasa_core::events::PostPublished {
            post_id: 42,
            author_id: 1,
        });
        let (kind, data) = subscribable(&ev).expect("published is subscribable");
        assert_eq!(kind.as_str(), "post.published");
        assert_eq!(data["post_id"], 42);
    }

    #[test]
    fn unsubscribable_events_are_skipped_rather_than_sent_as_something_else() {
        // Every arm must be a deliberate mapping; a catch-all that invented
        // an event name would send subscribers something that never happened.
        let ev = Event::Restored(vyasa_core::events::PostRestored { post_id: 1 });
        assert!(subscribable(&ev).is_none());
    }
}
