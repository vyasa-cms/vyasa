//! Priority-ordered hook registry + dispatch.
//!
//! Filters chain strings through every registered plugin (stable priority
//! order); actions fire-and-forget. A failing plugin is skipped, marked
//! degraded via the [`DegradedSink`], and never breaks the request.

use std::collections::BTreeMap;
use std::sync::Arc;

/// Hook points wired into real flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HookPoint {
    /// Post content before block rendering.
    Content,
    /// Fully rendered page HTML before caching.
    PageHtml,
    /// Feed item XML fragment.
    FeedItem,
    /// Comment body after sanitization, before storage.
    CommentBody,
    /// Rendered `<title>` text.
    SeoTitle,
    /// Login credentials accepted, before a session is issued: a plugin
    /// may demand a second factor here.
    AuthChallenge,
}

impl HookPoint {
    /// Stable string name used in telemetry.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::PageHtml => "page-html",
            Self::FeedItem => "feed-item",
            Self::CommentBody => "comment-body",
            Self::SeoTitle => "seo-title",
            Self::AuthChallenge => "auth-challenge",
        }
    }
}

/// One registration: plugin participates in `hook` at `priority`.
#[derive(Debug, Clone)]
pub struct Registration {
    /// Plugin row id.
    pub plugin_id: i64,
    /// Hook point.
    pub hook: HookPoint,
    /// Ascending priority (lower runs first); ties break by plugin id.
    pub priority: i32,
}

/// Sink for degraded plugins (implemented by api to update DB status).
pub trait DegradedSink: Send + Sync {
    /// Marks the plugin degraded after a hook failure.
    fn mark_degraded(&self, plugin_id: i64, reason: &str);
}

/// Async filter executor implemented by the api over the wasmtime host.
pub trait FilterExecutor: Send + Sync {
    /// Runs one plugin's filter for this hook point.
    fn invoke(
        &self,
        plugin_id: i64,
        hook: HookPoint,
        content: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;
}

/// The domain event that triggered an action.
///
/// Distinct from [`HookPoint`]: a hook point says *where in the render*
/// a plugin participates, while an action kind says *what happened*. A
/// plugin's `action` export is keyed by the latter, so the dispatcher has
/// to carry it rather than reconstruct it from the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// A post moved into `published`.
    PostPublished,
    /// A published post was edited.
    PostUpdated,
    /// A post was moved to trash.
    PostTrashed,
    /// A comment was created.
    CommentAdded,
}

impl ActionKind {
    /// Stable wire name, matching the WIT `event-kind` cases.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PostPublished => "post-published",
            Self::PostUpdated => "post-updated",
            Self::PostTrashed => "post-trashed",
            Self::CommentAdded => "comment-added",
        }
    }
}

/// Async action executor implemented by the api over the wasmtime host.
///
/// Separate from [`FilterExecutor`] because actions return nothing and are
/// keyed by [`ActionKind`]; folding both into one trait forced the
/// dispatcher to discard the event kind.
pub trait ActionExecutor: Send + Sync {
    /// Fires one plugin's action for this event.
    fn invoke_action(
        &self,
        plugin_id: i64,
        kind: ActionKind,
        payload: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;
}

/// Registry of registrations across all hooks.
#[derive(Default, Clone)]
pub struct HookRegistry {
    regs: Arc<tokio::sync::RwLock<BTreeMap<HookPoint, Vec<Registration>>>>,
}

impl HookRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a plugin for a hook point.
    pub async fn register(&self, reg: Registration) {
        let mut map = self.regs.write().await;
        let list = map.entry(reg.hook).or_default();
        list.push(reg);
        // Stable ordering: priority asc, then plugin id asc.
        list.sort_by_key(|r| (r.priority, r.plugin_id));
    }

    /// Removes every registration a plugin holds.
    ///
    /// Without this a disabled plugin stayed registered on every hook, so
    /// each page render called it, failed to load it, and marked it
    /// degraded — turning a plugin off made the admin report that it had
    /// crashed.
    pub async fn forget(&self, plugin_id: i64) {
        let mut map = self.regs.write().await;
        for list in map.values_mut() {
            list.retain(|r| r.plugin_id != plugin_id);
        }
    }

    /// Ordered plugin ids participating in `hook`.
    pub async fn ordered(&self, hook: HookPoint) -> Vec<i64> {
        self.regs
            .read()
            .await
            .get(&hook)
            .map(|v| v.iter().map(|r| r.plugin_id).collect())
            .unwrap_or_default()
    }

    /// Whether any plugin listens on `hook`.
    pub async fn is_empty(&self, hook: HookPoint) -> bool {
        self.ordered(hook).await.is_empty()
    }

    /// Chains `content` through every registered plugin in order.
    ///
    /// Filter errors skip that plugin (marked degraded) without aborting
    /// the chain; the combined deadline lives with the caller-provided
    /// executor.
    pub async fn dispatch_filter(
        &self,
        hook: HookPoint,
        mut content: String,
        executor: &dyn FilterExecutor,
        sink: &dyn DegradedSink,
    ) -> String {
        for plugin_id in self.ordered(hook).await {
            match executor.invoke(plugin_id, hook, content.clone()).await {
                Ok(next) => content = next,
                Err(reason) => {
                    tracing::warn!(plugin_id, hook = hook.as_str(), "filter failed: {reason}");
                    sink.mark_degraded(plugin_id, &reason);
                }
            }
        }
        content
    }

    /// Fires an action on every plugin registered at `hook`; failures are
    /// logged + degraded, never surfaced.
    ///
    /// `hook` selects the audience, `kind` tells each plugin what happened.
    pub async fn dispatch_action(
        &self,
        hook: HookPoint,
        kind: ActionKind,
        payload: String,
        executor: &dyn ActionExecutor,
        sink: &dyn DegradedSink,
    ) {
        // Sequential per plugin: each invocation runs to completion, and a
        // failure degrades that plugin without aborting the rest.
        for id in self.ordered(hook).await {
            if let Err(reason) = executor.invoke_action(id, kind, payload.clone()).await {
                sink.mark_degraded(id, &reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HookPoint, HookRegistry, Registration};

    #[tokio::test]
    async fn registrations_run_in_priority_order_and_can_be_withdrawn() {
        let registry = HookRegistry::new();
        for (plugin_id, priority) in [(7, 50), (3, 10), (5, 10)] {
            registry
                .register(Registration {
                    plugin_id,
                    hook: HookPoint::PageHtml,
                    priority,
                })
                .await;
        }
        // Priority ascending, ties broken by id, so the order a visitor's
        // page is filtered in does not depend on install order.
        assert_eq!(registry.ordered(HookPoint::PageHtml).await, vec![3, 5, 7]);
        assert!(registry.is_empty(HookPoint::Content).await);

        registry.forget(5).await;
        assert_eq!(registry.ordered(HookPoint::PageHtml).await, vec![3, 7]);
        registry.forget(3).await;
        registry.forget(7).await;
        assert!(
            registry.is_empty(HookPoint::PageHtml).await,
            "a withdrawn plugin is not called again"
        );
    }

    #[tokio::test]
    async fn forgetting_one_plugin_leaves_the_others_alone() {
        let registry = HookRegistry::new();
        for hook in [HookPoint::PageHtml, HookPoint::Content] {
            for plugin_id in [1, 2] {
                registry
                    .register(Registration {
                        plugin_id,
                        hook,
                        priority: 0,
                    })
                    .await;
            }
        }
        registry.forget(1).await;
        assert_eq!(registry.ordered(HookPoint::PageHtml).await, vec![2]);
        assert_eq!(registry.ordered(HookPoint::Content).await, vec![2]);
    }
}
