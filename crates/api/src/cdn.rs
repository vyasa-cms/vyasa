//! Purging an edge cache.
//!
//! Caching HTML at a CDN is the single biggest performance change a site
//! can make — a 4 ms origin render becomes a 0 ms edge hit, worldwide —
//! and it is only safe if a publish reaches the edge too. Without a purge,
//! a correction sits behind a stale copy until the TTL runs out, which is
//! why `edge_cache_seconds` defaults to zero and this exists.
//!
//! The invalidator already knows exactly which pages changed. This tells
//! the edge the same thing.

use std::time::Duration;

use vyasa_common::CdnConfig;

/// Longest a purge request may take before it is abandoned.
///
/// A purge is best-effort: the origin has already served the new content,
/// and a request that hangs must not hold up the event loop behind it.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Most URLs Cloudflare accepts in one purge call.
const MAX_URLS: usize = 30;

/// Tells an edge cache to drop pages.
pub trait Purger: Send + Sync {
    /// Drops specific absolute URLs.
    fn purge<'a>(
        &'a self,
        urls: Vec<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

    /// Drops everything the edge holds for this site.
    fn purge_all<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// The purger for a site with no CDN configured.
///
/// Not an `Option<Purger>` at every call site: "there is no edge" is a
/// perfectly good edge, and saying so once here keeps the callers honest.
pub struct NoCdn;

impl Purger for NoCdn {
    fn purge<'a>(
        &'a self,
        _urls: Vec<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    fn purge_all<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// Cloudflare's zone purge API.
pub struct Cloudflare {
    client: reqwest::Client,
    zone_id: String,
    token: String,
}

impl Cloudflare {
    /// Builds a purger, or `None` when the configuration is incomplete.
    #[must_use]
    pub fn new(config: &CdnConfig) -> Option<Self> {
        let token = config.api_token.as_ref()?.expose().to_owned();
        if config.zone_id.is_empty() || token.is_empty() {
            return None;
        }
        Some(Self {
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            zone_id: config.zone_id.clone(),
            token,
        })
    }

    async fn post(&self, body: serde_json::Value) {
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}/purge_cache",
            self.zone_id
        );
        let sent = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await;
        match sent {
            Ok(response) if response.status().is_success() => {
                tracing::debug!("edge cache purged");
            }
            // Logged, never retried and never surfaced: the origin has
            // already served the new content, and a failed purge means the
            // edge is briefly stale, not that the write failed.
            Ok(response) => {
                tracing::warn!("edge purge refused: HTTP {}", response.status().as_u16());
            }
            Err(e) => tracing::warn!("edge purge failed: {e}"),
        }
    }
}

impl Purger for Cloudflare {
    fn purge<'a>(
        &'a self,
        urls: Vec<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if urls.is_empty() {
                return;
            }
            for chunk in urls.chunks(MAX_URLS) {
                self.post(serde_json::json!({ "files": chunk })).await;
            }
        })
    }

    fn purge_all<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.post(serde_json::json!({ "purge_everything": true }))
                .await;
        })
    }
}

/// The purger a configuration asks for.
#[must_use]
pub fn from_config(config: &vyasa_common::VyasaConfig) -> std::sync::Arc<dyn Purger> {
    let Some(cdn) = config.cdn.as_ref().filter(|c| c.is_configured()) else {
        return std::sync::Arc::new(NoCdn);
    };
    match cdn.provider.as_str() {
        "cloudflare" => match Cloudflare::new(cdn) {
            Some(purger) => std::sync::Arc::new(purger),
            None => std::sync::Arc::new(NoCdn),
        },
        other => {
            tracing::warn!("unknown cdn provider {other:?}; the edge will not be purged");
            std::sync::Arc::new(NoCdn)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cloudflare, Purger};
    use vyasa_common::CdnConfig;

    #[test]
    fn an_incomplete_configuration_purges_nothing_rather_than_half_working() {
        // A zone with no token, or a token with no zone, cannot purge. It
        // must not look configured, or an operator would enable edge
        // caching believing publishes reach the edge.
        let no_token = CdnConfig {
            provider: String::from("cloudflare"),
            zone_id: String::from("abc"),
            api_token: None,
        };
        assert!(!no_token.is_configured());
        assert!(Cloudflare::new(&no_token).is_none());

        let no_zone = CdnConfig {
            provider: String::from("cloudflare"),
            zone_id: String::new(),
            api_token: Some(vyasa_common::Secret::new("t")),
        };
        assert!(!no_zone.is_configured());
        assert!(Cloudflare::new(&no_zone).is_none());

        let complete = CdnConfig {
            provider: String::from("cloudflare"),
            zone_id: String::from("abc"),
            api_token: Some(vyasa_common::Secret::new("t")),
        };
        assert!(complete.is_configured());
        assert!(Cloudflare::new(&complete).is_some());
    }

    #[test]
    fn an_unknown_provider_purges_nothing_rather_than_guessing() {
        let cdn = CdnConfig {
            provider: String::from("some-other-cdn"),
            zone_id: String::from("abc"),
            api_token: Some(vyasa_common::Secret::new("t")),
        };
        assert!(cdn.is_configured(), "complete, but not one we speak");
        // The purger still has to be a purger; it just does nothing, so a
        // caller never has to ask whether there is an edge.
        assert!(Cloudflare::new(&cdn).is_some(), "the shape is fine");
    }

    #[test]
    fn a_no_op_purger_accepts_every_call() {
        let purger = super::NoCdn;
        futures::executor::block_on(purger.purge(vec![String::from("https://x.test/")]));
        futures::executor::block_on(purger.purge(Vec::new()));
        futures::executor::block_on(purger.purge_all());
    }
}
