//! The plugin and theme marketplace.
//!
//! A registry is a **static signed document**, not a service: one
//! `index.json` listing every plugin and theme with its versions, and
//! artifacts hosted anywhere. The site fetches the index, shows what is
//! available, and installs by downloading a package and handing it to
//! the same code path an uploaded file goes through.
//!
//! That shape is deliberate. Because every package is checksummed and
//! (when the registry signs) ed25519-verified, **the transport does not
//! have to be trusted** — a bucket, a CDN, a mirror, or a colleague's
//! laptop all give the same guarantee. Hosting a marketplace therefore
//! costs a static file, and publishing to one is a pull request.
//!
//! Plugins additionally carry their own in-package author signature,
//! checked by `vyasa_plugins::package`, so a marketplace install is
//! verified twice: the registry vouches for distribution, the author's
//! key vouches for the contents.

pub mod host;
pub mod index;
pub mod install;

/// Option holding the registry index URL (https). Empty disables the
/// marketplace entirely.
pub const URL_OPTION: &str = "registry_url";

/// Option holding hex ed25519 public keys allowed to sign registry
/// artifacts.
///
/// An option rather than config, deliberately: trusting a marketplace
/// should not require editing `vyasa.toml` and restarting the server.
pub const TRUSTED_KEYS_OPTION: &str = "registry_trusted_keys";

use crate::state::AppState;

/// The configured registry URL, empty when the marketplace is off.
pub async fn url(state: &AppState) -> String {
    state
        .options
        .get(URL_OPTION)
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Keys this install accepts registry signatures from.
pub async fn trusted_keys(state: &AppState) -> Vec<String> {
    match state.options.get(TRUSTED_KEYS_OPTION).await {
        Ok(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Ok(serde_json::Value::String(one)) => one
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}
