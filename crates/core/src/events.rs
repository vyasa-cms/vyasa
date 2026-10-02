//! Domain events — in-process broadcast channel for post lifecycle.
//!
//! Plugin hooks (phase 33) and other subscribers (webhooks, search) listen
//! on the same channel. The channel is bounded (100) and uses
//! `tokio::sync::broadcast`; lagging receivers get a `Lagged` error and
//! should re-sync via DB if needed.

use tokio::sync::broadcast;

/// A post was first published (draft/scheduled → published, or scheduled
/// auto-published by the worker).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostPublished {
    /// Post id.
    pub post_id: i64,
    /// Author id.
    pub author_id: i64,
}

/// A post was updated (any mutable field, including status changes that
/// are not terminal publish/trash/restore).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostUpdated {
    /// Post id.
    pub post_id: i64,
}

/// A post was moved to trash.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostTrashed {
    /// Post id.
    pub post_id: i64,
}

/// A trashed post was restored to draft.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostRestored {
    /// Post id.
    pub post_id: i64,
}

/// A post was hard-deleted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostDeleted {
    /// Post id.
    pub post_id: i64,
}

/// A comment was added.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommentAdded {
    /// Comment id.
    pub comment_id: i64,
    /// Post id.
    pub post_id: i64,
}

/// A site option changed (phase 27).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OptionChanged {
    /// The option key that was written.
    pub key: String,
}

/// All domain events.
///
/// Serializable because the cross-instance bridge relays them through
/// Postgres `NOTIFY`; the shape is an internal wire format between
/// instances of the same build, not a public contract.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Event {
    /// See [`PostPublished`].
    Published(PostPublished),
    /// See [`PostUpdated`].
    Updated(PostUpdated),
    /// See [`PostTrashed`].
    Trashed(PostTrashed),
    /// See [`PostRestored`].
    Restored(PostRestored),
    /// See [`PostDeleted`].
    Deleted(PostDeleted),
    /// See [`CommentAdded`].
    CommentAdded(CommentAdded),
    /// A comment a moderator has just made public. Distinct from
    /// [`Event::CommentAdded`]: re-using that for approval re-sent the
    /// author's email, re-fired the webhook and re-ran AI screening on a
    /// comment that had already been screened.
    CommentApproved(CommentAdded),
    /// See [`OptionChanged`].
    OptionChanged(OptionChanged),
    /// Navigation changed; every rendered page may contain it.
    MenuChanged,
}

static CHANNEL: std::sync::OnceLock<broadcast::Sender<Event>> = std::sync::OnceLock::new();

fn sender() -> &'static broadcast::Sender<Event> {
    CHANNEL.get_or_init(|| broadcast::channel(100).0)
}

/// Where an event came from.
///
/// The bus is per process. A second server behind a load balancer never
/// saw the first's events, so publishing a post left half the traffic
/// serving a stale page indefinitely — the reason this could only ever run
/// on one machine. A bridge fixes that by relaying events between
/// instances, and needs to tell a relayed event from a local one or the
/// two bridges would echo forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Raised by this process.
    Local,
    /// Relayed from another instance.
    Remote,
}

/// A hook that forwards locally-raised events to other instances.
type Relay = Box<dyn Fn(&Event) + Send + Sync>;

static RELAY: std::sync::OnceLock<Relay> = std::sync::OnceLock::new();

/// Installs the outbound half of a cross-instance bridge.
///
/// Called once at boot. Only [`Origin::Local`] events are handed to it, so
/// a relayed event is never relayed onward.
///
/// # Errors
/// Returns the relay unchanged when one is already installed.
pub fn set_relay(relay: Relay) -> Result<(), Relay> {
    RELAY.set(relay)
}

/// Emits an event that arrived from another instance.
///
/// Delivered to local subscribers exactly like a local one, but never
/// relayed back out.
pub fn emit_remote(event: Event) {
    let _ = sender().send(event);
}

/// Emits an option-changed event.
pub fn emit_option_changed(key: &str) {
    emit(Event::OptionChanged(OptionChanged {
        key: key.to_owned(),
    }));
}

/// Emits an event (best-effort; lagged receivers are not retried).
///
/// Also handed to the cross-instance relay when one is installed, so a
/// sibling server's caches learn about it too.
pub fn emit(event: Event) {
    if let Some(relay) = RELAY.get() {
        relay(&event);
    }
    let _ = sender().send(event);
}

/// Subscribes to the event stream.
#[must_use]
pub fn subscribe() -> broadcast::Receiver<Event> {
    sender().subscribe()
}

/// Emits a published event.
pub fn emit_published(post_id: i64, author_id: i64) {
    emit(Event::Published(PostPublished { post_id, author_id }));
}

/// Emits an updated event.
pub fn emit_updated(post_id: i64) {
    emit(Event::Updated(PostUpdated { post_id }));
}

/// Emits a trashed event.
pub fn emit_trashed(post_id: i64) {
    emit(Event::Trashed(PostTrashed { post_id }));
}

/// Emits a restored event.
pub fn emit_restored(post_id: i64) {
    emit(Event::Restored(PostRestored { post_id }));
}

/// Emits a deleted event.
pub fn emit_deleted(post_id: i64) {
    emit(Event::Deleted(PostDeleted { post_id }));
}

/// Emits a comment-added event.
pub fn emit_comment_added(comment_id: i64, post_id: i64) {
    emit(Event::CommentAdded(CommentAdded {
        comment_id,
        post_id,
    }));
}

/// Emits [`Event::CommentApproved`].
pub fn emit_comment_approved(comment_id: i64, post_id: i64) {
    emit(Event::CommentApproved(CommentAdded {
        comment_id,
        post_id,
    }));
}

#[cfg(test)]
mod tests {
    use super::{emit_published, subscribe, Event};

    #[test]
    fn publish_and_receive() {
        let mut rx = subscribe();
        emit_published(42, 7);
        let event = rx.try_recv().expect("event");
        assert_eq!(
            event,
            Event::Published(super::PostPublished {
                post_id: 42,
                author_id: 7
            })
        );
    }
}
