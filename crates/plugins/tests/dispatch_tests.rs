//! Domain-event fan-out: which core events reach a plugin, as what, and
//! what happens when one of them fails.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use vyasa_core::events::{
    CommentAdded, Event, OptionChanged, PostDeleted, PostPublished, PostRestored, PostTrashed,
    PostUpdated,
};
use vyasa_plugins::dispatch::{event_to_action, spawn_event_dispatcher};
use vyasa_plugins::hooks::{
    ActionExecutor, ActionKind, DegradedSink, HookPoint, HookRegistry, Registration,
};

#[test]
fn every_plugin_visible_event_maps_to_an_action_and_the_rest_map_to_none() {
    let cases = [
        (
            Event::Published(PostPublished {
                post_id: 7,
                author_id: 1,
            }),
            Some((HookPoint::Content, ActionKind::PostPublished)),
        ),
        (
            Event::Updated(PostUpdated { post_id: 7 }),
            Some((HookPoint::Content, ActionKind::PostUpdated)),
        ),
        (
            Event::Trashed(PostTrashed { post_id: 7 }),
            Some((HookPoint::PageHtml, ActionKind::PostTrashed)),
        ),
        (
            Event::CommentAdded(CommentAdded {
                comment_id: 7,
                post_id: 9,
            }),
            Some((HookPoint::CommentBody, ActionKind::CommentAdded)),
        ),
        // Not part of the v1 action contract: a plugin has no case for
        // them, so handing one over would be an event it cannot name.
        (Event::Restored(PostRestored { post_id: 7 }), None),
        (Event::Deleted(PostDeleted { post_id: 7 }), None),
        (
            Event::OptionChanged(OptionChanged {
                key: String::from("site_title"),
            }),
            None,
        ),
    ];

    for (event, expected) in cases {
        match (event_to_action(&event), expected) {
            (Some((hook, kind, payload)), Some((want_hook, want_kind))) => {
                assert_eq!(hook, want_hook, "{event:?}");
                assert_eq!(kind, want_kind, "{event:?}");
                // Only the id crosses the boundary: a plugin is told what
                // happened and to what, and looks the rest up itself under
                // whatever capabilities it holds.
                assert_eq!(payload, r#"{"id":7}"#, "{event:?}");
            }
            (None, None) => {}
            (got, want) => panic!("{event:?}: got {got:?}, wanted {want:?}"),
        }
    }
}

/// Records every invocation, and fails for the plugin ids it was told to.
struct Recorder {
    seen: Arc<Mutex<Vec<(i64, ActionKind)>>>,
    failing: Vec<i64>,
}

impl ActionExecutor for Recorder {
    fn invoke_action(
        &self,
        plugin_id: i64,
        kind: ActionKind,
        _payload: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        self.seen.lock().unwrap().push((plugin_id, kind));
        let failed = self.failing.contains(&plugin_id);
        Box::pin(async move {
            if failed {
                Err(String::from("boom"))
            } else {
                Ok(())
            }
        })
    }
}

#[derive(Default)]
struct Degraded(Arc<Mutex<Vec<i64>>>);

impl DegradedSink for Degraded {
    fn mark_degraded(&self, plugin_id: i64, _reason: &str) {
        self.0.lock().unwrap().push(plugin_id);
    }
}

#[tokio::test]
async fn one_failing_plugin_is_degraded_and_the_others_still_run() {
    let registry = HookRegistry::new();
    for (plugin_id, priority) in [(1, 10), (2, 20), (3, 30)] {
        registry
            .register(Registration {
                plugin_id,
                hook: HookPoint::Content,
                priority,
            })
            .await;
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let executor = Recorder {
        seen: Arc::clone(&seen),
        failing: vec![2],
    };
    let degraded = Degraded::default();
    let marked = Arc::clone(&degraded.0);

    registry
        .dispatch_action(
            HookPoint::Content,
            ActionKind::PostPublished,
            String::from(r#"{"id":1}"#),
            &executor,
            &degraded,
        )
        .await;

    // Every plugin was called, in priority order, and the failure of the
    // middle one neither stopped the chain nor was surfaced to the caller.
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            (1, ActionKind::PostPublished),
            (2, ActionKind::PostPublished),
            (3, ActionKind::PostPublished),
        ]
    );
    assert_eq!(*marked.lock().unwrap(), vec![2], "only the one that failed");
}

#[tokio::test]
async fn the_dispatcher_keeps_running_after_an_event_it_ignores() {
    // A dropped subscriber or an unhandled event kind must not end the
    // loop: doing so once stopped plugin dispatch for the life of the
    // process, and nothing said so.
    let registry = HookRegistry::new();
    registry
        .register(Registration {
            plugin_id: 1,
            hook: HookPoint::Content,
            priority: 0,
        })
        .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let degraded = Arc::new(Degraded::default());
    spawn_event_dispatcher(
        registry,
        Arc::new(Recorder {
            seen: Arc::clone(&seen),
            failing: Vec::new(),
        }),
        degraded,
    );
    // Let the subscriber attach before anything is emitted.
    tokio::task::yield_now().await;

    // An event with no plugin action, then one with.
    vyasa_core::events::emit(Event::Restored(PostRestored { post_id: 1 }));
    vyasa_core::events::emit(Event::Published(PostPublished {
        post_id: 42,
        author_id: 1,
    }));

    for _ in 0..100 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        *seen.lock().unwrap(),
        vec![(1, ActionKind::PostPublished)],
        "the ignored event did not stop the loop"
    );
}
