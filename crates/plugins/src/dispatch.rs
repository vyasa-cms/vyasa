//! Domain-event fan-out: core bus events → plugin actions.

use vyasa_core::events::Event;

use crate::hooks::{ActionExecutor, ActionKind, DegradedSink, HookPoint, HookRegistry};

/// Maps a core event to the hook whose registrations should hear about it,
/// the action kind to hand the plugin, and the JSON payload.
///
/// `None` means no plugin-visible action for this event.
#[must_use]
pub fn event_to_action(ev: &Event) -> Option<(HookPoint, ActionKind, String)> {
    let payload = |id: i64| serde_json::json!({ "id": id }).to_string();
    match ev {
        Event::Published(p) => Some((
            HookPoint::Content,
            ActionKind::PostPublished,
            payload(p.post_id),
        )),
        Event::Updated(p) => Some((
            HookPoint::Content,
            ActionKind::PostUpdated,
            payload(p.post_id),
        )),
        Event::Trashed(p) => Some((
            HookPoint::PageHtml,
            ActionKind::PostTrashed,
            payload(p.post_id),
        )),
        Event::CommentAdded(c) => Some((
            HookPoint::CommentBody,
            ActionKind::CommentAdded,
            payload(c.comment_id),
        )),
        _ => None,
    }
}

/// Spawns the bus subscriber that fans events out to registered actions.
///
/// Detached by design: the bus lives for the process lifetime.
pub fn spawn_event_dispatcher(
    registry: HookRegistry,
    executor: std::sync::Arc<dyn ActionExecutor>,
    sink: std::sync::Arc<dyn DegradedSink>,
) {
    let mut rx = vyasa_core::events::subscribe();
    tokio::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                // Lagged drops events but leaves the receiver usable, so
                // keep going: breaking here would silently stop plugin
                // dispatch for the rest of the process's life.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("plugin dispatcher lagged {n} events; actions were skipped");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if let Some((hook, kind, payload)) = event_to_action(&ev) {
                registry
                    .dispatch_action(hook, kind, payload, executor.as_ref(), sink.as_ref())
                    .await;
            }
        }
    });
}
