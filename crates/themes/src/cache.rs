//! Public-site render cache: moka-backed, event-invalidated, ETag-aware.
//!
//! Keys embed the active theme version, so installing a new theme version
//! never serves stale HTML; entries are weighed by their byte size and the
//! total capacity comes from configuration. Invalidation is driven by
//! domain events through [`CacheService::apply_domain_event`].

use vyasa_core::events::Event;

/// A fully rendered public page plus its derived validators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPage {
    /// Rendered document body.
    pub html: String,
    /// Strong ETag derived from the body content hash.
    pub etag: String,
}

impl CachedPage {
    /// Builds a page and computes its ETag.
    #[must_use]
    pub fn new(html: impl Into<String>) -> Self {
        let html = html.into();
        Self {
            etag: etag_for(&html),
            html,
        }
    }

    /// Approximate memory footprint used as the cache weight.
    #[must_use]
    pub fn weight(&self) -> u64 {
        (self.html.len() + self.etag.len()) as u64
    }
}

/// FNV-1a 64-bit content hash rendered as a quoted strong ETag.
#[must_use]
pub fn etag_for(body: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in body.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("\"{hash:016x}\"")
}

/// Outcome of checking an `If-None-Match` header against the current ETag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalGet {
    /// Client cache matches; serve `304 Not Modified`.
    NotModified,
    /// Serve the full representation with this ETag.
    Fresh(String),
}

/// Evaluates an `If-None-Match` header value against the current ETag.
///
/// Handles `*`, exact matches, and weak comparisons (`W/"..."` prefixes are
/// compared weakly, which is correct for 304 semantics).
#[must_use]
pub fn evaluate_if_none_match(if_none_match: Option<&str>, current: &str) -> ConditionalGet {
    let Some(header) = if_none_match else {
        return ConditionalGet::Fresh(current.to_owned());
    };
    let header = header.trim();
    if header == "*" {
        return ConditionalGet::NotModified;
    }
    let normalize = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
    let wanted = normalize(current);
    for candidate in header.split(',') {
        if normalize(candidate) == wanted {
            return ConditionalGet::NotModified;
        }
    }
    ConditionalGet::Fresh(current.to_owned())
}

/// Key namespaces used by the public site.
pub mod keys {
    /// Home / blog index: `t{v}:home`.
    #[must_use]
    pub fn home(theme_version: &str) -> String {
        format!("t{theme_version}:home")
    }

    /// Single entry incl. revision: `t{v}:single:{slug}:r{rev}`.
    #[must_use]
    pub fn single(theme_version: &str, slug: &str, revision: i64) -> String {
        format!("t{theme_version}:single:{slug}:r{revision}")
    }

    /// Archive listing: `t{v}:archive:{kind}:{term}:{page}`.
    ///
    /// Global listings pass the empty term (e.g. date archives).
    #[must_use]
    pub fn archive(theme_version: &str, kind: &str, term: &str, page: u32) -> String {
        format!("t{theme_version}:archive:{kind}:{term}:{page}")
    }

    /// One page of search results for one query.
    #[must_use]
    pub fn search(theme_version: &str, term: &str, page: u32) -> String {
        format!("t{theme_version}:search:{page}:{term}")
    }

    /// RSS/Atom feeds: `t{v}:feed`.
    #[must_use]
    pub fn feed(theme_version: &str) -> String {
        format!("t{theme_version}:feed")
    }

    /// Shared prefix of every key for a theme version.
    #[must_use]
    pub fn version_prefix(theme_version: &str) -> String {
        format!("t{theme_version}:")
    }
}

/// Hit/miss counters snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheMetrics {
    /// Requests served from cache.
    pub hits: u64,
    /// Requests that had to render.
    pub misses: u64,
}

/// The render cache service.
#[derive(Clone)]
pub struct CacheService {
    inner: moka::sync::Cache<String, std::sync::Arc<CachedPage>>,
    hits: std::sync::Arc<core::sync::atomic::AtomicU64>,
    misses: std::sync::Arc<core::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for CacheService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheService")
            .field("metrics", &self.snapshot())
            .finish()
    }
}

impl CacheService {
    /// Creates a cache bounded by total rendered-bytes weight and TTL.
    #[must_use]
    pub fn new(max_total_bytes: u64, ttl_seconds: u64) -> Self {
        Self {
            inner: moka::sync::Cache::builder()
                .weigher(|_k: &String, v: &std::sync::Arc<CachedPage>| {
                    u32::try_from(v.weight()).unwrap_or(u32::MAX)
                })
                .max_capacity(max_total_bytes)
                .time_to_live(std::time::Duration::from_secs(ttl_seconds))
                .build(),
            hits: std::sync::Arc::new(core::sync::atomic::AtomicU64::new(0)),
            misses: std::sync::Arc::new(core::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Current hit/miss counters.
    #[must_use]
    pub fn snapshot(&self) -> CacheMetrics {
        CacheMetrics {
            hits: self.hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.misses.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Fetches a cached page, counting hit/miss metrics.
    #[allow(clippy::must_use_candidate)] // metrics are the side effect
    pub fn get(&self, key: &str) -> Option<std::sync::Arc<CachedPage>> {
        use std::sync::atomic::Ordering;
        let hit = self.inner.get(key);
        if hit.is_some() {
            let _ = self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            let _ = self.misses.fetch_add(1, Ordering::Relaxed);
        }
        hit
    }

    /// Inserts a rendered page.
    pub fn insert(&self, key: &str, page: CachedPage) {
        self.inner.insert(key.to_owned(), std::sync::Arc::new(page));
    }

    /// Removes one key.
    pub fn invalidate(&self, key: &str) {
        self.inner.invalidate(key);
    }

    /// Removes every key starting with `prefix`; returns how many entries
    /// were removed.
    #[allow(clippy::must_use_candidate)] // count is informational
    pub fn invalidate_prefix(&self, prefix: &str) -> usize {
        let victims: Vec<String> = self
            .inner
            .iter()
            .filter(|(k, _)| k.as_str().starts_with(prefix))
            .map(|(k, _)| (*k).clone())
            .collect();
        for k in &victims {
            self.inner.invalidate(k);
        }
        victims.len()
    }

    /// Drops every entry (used for theme/options changes).
    pub fn invalidate_all(&self) {
        self.inner.invalidate_all();
    }

    /// Drives pending maintenance (eviction/TTL) — mainly for tests.
    pub fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks();
    }

    /// Applies the invalidation policy for a core domain event.
    ///
    /// Returns the key names/prefixes that were purged (test observability).
    #[allow(clippy::must_use_candidate)] // return value is informational
    pub fn apply_domain_event(&self, event: &Event, theme_version: &str) -> Vec<String> {
        if matches!(event, Event::MenuChanged | Event::OptionChanged(_)) {
            self.invalidate_options();
            return vec!["all".into()];
        }
        // Comments are rendered as a dynamic block inside single pages (phase
        // 25); new/approved comments must invalidate their parent single.
        if matches!(event, Event::CommentAdded(_) | Event::CommentApproved(_)) {
            let n = self.invalidate_prefix(&format!("t{theme_version}:single:"));
            return if n == 0 {
                Vec::new()
            } else {
                vec![format!("t{theme_version}:single:")]
            };
        }
        let mut purges: Vec<(String, bool)> = vec![
            // Exact key:
            (keys::home(theme_version), true),
            // Prefixes:
            (format!("t{theme_version}:archive:"), false),
            (format!("t{theme_version}:search:"), false),
            (keys::feed(theme_version), true),
        ];
        // Mutations can also alter rendered singles; without an id→slug map
        // here we conservatively drop all of them (revision-bearing keys
        // make this cheap for pure content edits once wired to revisions).
        if !matches!(event, Event::Published(_)) {
            purges.push((format!("t{theme_version}:single:"), false));
        }
        for (target, exact) in &purges {
            if *exact {
                self.invalidate(target);
            } else {
                self.invalidate_prefix(target);
            }
        }
        purges.into_iter().map(|(t, _)| t).collect()
    }

    /// Purges listing caches after taxonomy changes (emitted by later
    /// phases when terms are mutated).
    #[allow(clippy::must_use_candidate)] // count is informational
    pub fn invalidate_terms(&self, theme_version: &str) -> usize {
        let n = self.invalidate_prefix(&format!("t{theme_version}:archive:"))
            + self.invalidate_prefix(&keys::home(theme_version));
        self.invalidate_prefix(&keys::feed(theme_version));
        n
    }

    /// Full purge on theme install/change.
    pub fn invalidate_theme(&self) {
        self.invalidate_all();
    }

    /// Full purge when site options that influence rendering change.
    pub fn invalidate_options(&self) {
        self.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_is_stable_and_quoted() {
        let a = etag_for("hello");
        let b = etag_for("hello");
        assert_eq!(a, b);
        assert!(a.starts_with('"') && a.ends_with('"'));
        assert_ne!(etag_for("hellp"), a);
    }

    #[test]
    fn conditional_get_matches_rfc_semantics() {
        let current = "\"abc123\"";
        assert_eq!(
            evaluate_if_none_match(Some("\"abc123\""), current),
            ConditionalGet::NotModified
        );
        assert_eq!(
            evaluate_if_none_match(Some("W/\"abc123\", \"other\""), current),
            ConditionalGet::NotModified
        );
        assert_eq!(
            evaluate_if_none_match(Some("*"), current),
            ConditionalGet::NotModified
        );
        assert_eq!(
            evaluate_if_none_match(None, current),
            ConditionalGet::Fresh(current.to_owned())
        );
        assert_eq!(
            evaluate_if_none_match(Some("\"zzz\""), current),
            ConditionalGet::Fresh(current.to_owned())
        );
    }
}
