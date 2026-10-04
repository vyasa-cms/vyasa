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
//! The marketplace signature over the exact bytes is the trust anchor.
//! Plugins additionally carry their author's in-package signature,
//! checked against the `author_key` the listing names (or the marketplace
//! keys when it names none), so a listing cannot be re-pointed at someone
//! else's package.

pub mod host;
pub mod index;
pub mod install;

use crate::state::AppState;

/// The marketplace this install reads, from the compiled-in source and
/// the operator's configuration.
#[must_use]
pub fn source(state: &AppState) -> crate::official::Source {
    crate::official::marketplace(&state.config)
}
