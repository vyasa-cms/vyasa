//! HookRegistry: priority order, chaining, failure isolation.

#![allow(clippy::pedantic, clippy::unwrap_used)]

use std::sync::Mutex;

use vyasa_plugins::hooks::{DegradedSink, FilterExecutor, HookPoint, HookRegistry, Registration};

#[derive(Default)]
struct RecordingSink(pub Mutex<Vec<(i64, String)>>);

impl DegradedSink for RecordingSink {
    fn mark_degraded(&self, plugin_id: i64, reason: &str) {
        self.0.lock().unwrap().push((plugin_id, reason.to_owned()));
    }
}

/// Appends `[plugin:<id>]` around the content; failing plugin panics
/// instead of returning.
struct ChainExecutor {
    fail_ids: Vec<i64>,
    sink: Mutex<Vec<String>>,
}

impl FilterExecutor for ChainExecutor {
    fn invoke(
        &self,
        plugin_id: i64,
        _hook: HookPoint,
        content: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> {
        if self.fail_ids.contains(&plugin_id) {
            return Box::pin(std::future::ready(Err(String::from("boom"))));
        }
        self.sink.lock().unwrap().push(content.clone());
        Box::pin(std::future::ready(Ok(format!("[p{plugin_id}]{content}"))))
    }
}

fn reg(plugin_id: i64, priority: i32) -> Registration {
    Registration {
        plugin_id,
        hook: HookPoint::PageHtml,
        priority,
    }
}

#[tokio::test]
async fn priority_order_is_stable() {
    let reg_map = HookRegistry::new();
    reg_map.register(reg(30, 5)).await;
    reg_map.register(reg(10, 1)).await;
    reg_map.register(reg(20, 1)).await; // tie -> lower id first
    reg_map.register(reg(40, 9)).await;
    assert_eq!(
        reg_map.ordered(HookPoint::PageHtml).await,
        vec![10, 20, 30, 40]
    );
}

#[tokio::test]
async fn filters_chain_in_priority_order() {
    let reg_map = HookRegistry::new();
    reg_map.register(reg(1, 1)).await;
    reg_map.register(reg(2, 2)).await;
    reg_map.register(reg(3, 3)).await;
    let ex = ChainExecutor {
        fail_ids: vec![],
        sink: Mutex::new(Vec::new()),
    };
    let out = reg_map
        .dispatch_filter(
            HookPoint::PageHtml,
            String::from("X"),
            &ex,
            &RecordingSink::default(),
        )
        .await;
    assert_eq!(out, "[p3][p2][p1]X");
}

#[tokio::test]
async fn failing_plugin_is_skipped_and_marked_degraded() {
    let reg_map = HookRegistry::new();
    reg_map.register(reg(1, 1)).await;
    reg_map.register(reg(2, 2)).await; // fails
    reg_map.register(reg(3, 3)).await;
    let sink = RecordingSink::default();
    let ex = ChainExecutor {
        fail_ids: vec![2],
        sink: Mutex::new(Vec::new()),
    };
    let out = reg_map
        .dispatch_filter(HookPoint::PageHtml, String::from("Y"), &ex, &sink)
        .await;
    assert_eq!(out, "[p3][p1]Y", "chain continues past failure");
    assert_eq!(sink.0.lock().unwrap().as_slice(), [(2, "boom".to_owned())]);
}

#[tokio::test]
async fn empty_registry_returns_content_unchanged() {
    let reg_map = HookRegistry::new();
    let ex = ChainExecutor {
        fail_ids: vec![],
        sink: Mutex::new(Vec::new()),
    };
    let out = reg_map
        .dispatch_filter(
            HookPoint::SeoTitle,
            String::from("same"),
            &ex,
            &RecordingSink::default(),
        )
        .await;
    assert_eq!(out, "same");
}

/// Records every action it is handed; fails for ids in `fail_ids`.
struct RecordingActions {
    fail_ids: Vec<i64>,
    seen: Mutex<Vec<(i64, &'static str, String)>>,
}

impl vyasa_plugins::hooks::ActionExecutor for RecordingActions {
    fn invoke_action(
        &self,
        plugin_id: i64,
        kind: vyasa_plugins::hooks::ActionKind,
        payload: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        if self.fail_ids.contains(&plugin_id) {
            return Box::pin(std::future::ready(Err(String::from("trap"))));
        }
        self.seen
            .lock()
            .unwrap()
            .push((plugin_id, kind.as_str(), payload));
        Box::pin(std::future::ready(Ok(())))
    }
}

#[tokio::test]
async fn actions_reach_every_registered_plugin_with_the_event_kind() {
    let reg_map = HookRegistry::new();
    reg_map.register(reg(1, 1)).await;
    reg_map.register(reg(2, 2)).await;
    let ex = RecordingActions {
        fail_ids: vec![],
        seen: Mutex::new(Vec::new()),
    };
    reg_map
        .dispatch_action(
            HookPoint::PageHtml,
            vyasa_plugins::hooks::ActionKind::PostPublished,
            String::from(r#"{"id":7}"#),
            &ex,
            &RecordingSink::default(),
        )
        .await;
    assert_eq!(
        ex.seen.lock().unwrap().as_slice(),
        [
            (1, "post-published", r#"{"id":7}"#.to_owned()),
            (2, "post-published", r#"{"id":7}"#.to_owned()),
        ],
        "both plugins run, and each is told which event fired"
    );
}

#[tokio::test]
async fn one_failing_action_does_not_stop_the_others() {
    let reg_map = HookRegistry::new();
    reg_map.register(reg(1, 1)).await;
    reg_map.register(reg(2, 2)).await; // traps
    reg_map.register(reg(3, 3)).await;
    let sink = RecordingSink::default();
    let ex = RecordingActions {
        fail_ids: vec![2],
        seen: Mutex::new(Vec::new()),
    };
    reg_map
        .dispatch_action(
            HookPoint::PageHtml,
            vyasa_plugins::hooks::ActionKind::CommentAdded,
            String::from("{}"),
            &ex,
            &sink,
        )
        .await;
    let ran: Vec<i64> = ex.seen.lock().unwrap().iter().map(|(id, ..)| *id).collect();
    assert_eq!(ran, vec![1, 3]);
    assert_eq!(sink.0.lock().unwrap().as_slice(), [(2, "trap".to_owned())]);
}

#[test]
fn every_action_kind_has_a_wit_matching_name() {
    use vyasa_plugins::hooks::ActionKind::{CommentAdded, PostPublished, PostTrashed, PostUpdated};
    // These strings are the WIT `event-kind` cases; drift here means the
    // host and the guest disagree about what happened.
    assert_eq!(PostPublished.as_str(), "post-published");
    assert_eq!(PostUpdated.as_str(), "post-updated");
    assert_eq!(PostTrashed.as_str(), "post-trashed");
    assert_eq!(CommentAdded.as_str(), "comment-added");
}
