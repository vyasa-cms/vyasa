//! Public-site render cache wiring: metrics + conditional GET support.
//!
//! The actual routes land in phase 26; this module exposes the service so
//! it can be tested and later injected into [`crate::state::AppState`].
#![allow(dead_code)] // consumed by public routes in phase 26

use vyasa_themes::cache::{evaluate_if_none_match, CacheMetrics, CacheService, ConditionalGet};

/// The namespace cache keys are scoped by.
///
/// Both the render path and the invalidator must derive this identically; when
/// they disagreed, purges targeted a namespace nothing read and the site
/// served stale pages. Keep this the only definition.
#[must_use]
pub fn theme_namespace(name: &str, version: i32) -> String {
    format!("{name}-v{version}")
}

/// Render-cache facade for the public site.
#[derive(Clone)]
pub struct RenderCache {
    inner: CacheService,
}

impl std::fmt::Debug for RenderCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderCache")
            .field("metrics", &self.metrics())
            .finish()
    }
}

impl RenderCache {
    /// Builds a service bounded by total rendered bytes with a TTL.
    #[must_use]
    pub fn new(max_total_bytes: u64, ttl_seconds: u64) -> Self {
        Self {
            inner: CacheService::new(max_total_bytes, ttl_seconds),
        }
    }

    /// Hit/miss counters.
    #[must_use]
    pub fn metrics(&self) -> CacheMetrics {
        self.inner.snapshot()
    }

    /// Cached page for a key, if present.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<std::sync::Arc<vyasa_themes::cache::CachedPage>> {
        self.inner.get(key)
    }

    /// Stores a rendered page.
    pub fn insert(&self, key: &str, page: vyasa_themes::cache::CachedPage) {
        self.inner.insert(key, page);
    }

    /// Applies a core domain event's invalidation policy.
    ///
    /// # Errors
    /// Never fails today; the signature keeps room for async purges.
    pub fn apply_domain_event(
        &self,
        event: &vyasa_core::events::Event,
        theme_version: &str,
    ) -> Vec<String> {
        self.inner.apply_domain_event(event, theme_version)
    }

    /// Full purge (theme install/change).
    pub fn invalidate_theme(&self) {
        self.inner.invalidate_theme();
    }

    /// Full purge (render-relevant options change).
    pub fn invalidate_options(&self) {
        self.inner.invalidate_options();
    }

    /// Listing purge after taxonomy changes.
    pub fn invalidate_terms(&self, theme_version: &str) -> usize {
        self.inner.invalidate_terms(theme_version)
    }

    /// Entry-page purge after a comment's visibility changes.
    ///
    /// Approving a comment in the admin used to have no effect on the
    /// site until the cache expired on its own: moderation emits no
    /// domain event, so nothing told the cache the page had changed.
    pub fn invalidate_entries(&self, theme_version: &str) -> usize {
        self.inner
            .invalidate_prefix(&format!("t{theme_version}:single:"))
    }
}

/// Decides between `304 Not Modified` and a fresh response for a cached
/// page based on the request's `If-None-Match` header.
#[must_use]
pub fn respond_cached(
    page: &vyasa_themes::cache::CachedPage,
    if_none_match: Option<&str>,
) -> ConditionalGet {
    evaluate_if_none_match(if_none_match, &page.etag)
}
