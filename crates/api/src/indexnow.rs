//! IndexNow: tell search engines about a change the minute it happens.
//!
//! Publishing or updating an entry enqueues one ping to
//! `api.indexnow.org`, which fans out to every participating engine
//! (Bing, Yandex, Seznam, Naver…). The site proves ownership with a key
//! served at `/indexnow.txt` (advertised via `keyLocation`, so the file
//! name never has to *be* the key).
//!
//! Guard: pings only happen when `site_url` is configured and public —
//! a dev box on localhost must never announce `http://localhost:3000`
//! to the world's crawlers.

use vyasa_core::events::{self, Event};

use crate::state::AppState;

/// Option key holding the site's IndexNow key.
pub const KEY_OPTION: &str = "indexnow_key";

/// A `site_url` worth announcing: absolute, http(s), and not an address
/// that only means something on this machine.
fn announceable(site_url: &str) -> bool {
    let host = site_url
        .strip_prefix("https://")
        .or_else(|| site_url.strip_prefix("http://"))
        .unwrap_or("")
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    !host.is_empty()
        && host != "localhost"
        && !host.starts_with("127.")
        && !host.starts_with("192.168.")
        && !host.starts_with("10.")
        && host.contains('.')
}

/// The key, minting one on first use. 32 alphanumerics, stored as an
/// option so `/indexnow.txt` and every ping agree forever.
async fn ensure_key(state: &AppState) -> Option<String> {
    use rand::Rng as _;
    if let Ok(v) = state.options.get(KEY_OPTION).await {
        if let Some(key) = v.as_str().filter(|k| !k.is_empty()) {
            return Some(key.to_owned());
        }
    }
    let key: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(|b| char::from(b.to_ascii_lowercase()))
        .collect();
    match state
        .options
        .set(KEY_OPTION, &serde_json::Value::String(key.clone()))
        .await
    {
        Ok(()) => Some(key),
        Err(err) => {
            tracing::warn!("could not store indexnow key: {err}");
            None
        }
    }
}

/// Watches the event bus and enqueues one ping per publish/update.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    let mut rx = events::subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let post_id = match &event {
                        Event::Published(p) => p.post_id,
                        Event::Updated(u) => u.post_id,
                        _ => continue,
                    };
                    enqueue_ping(&state, post_id).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn enqueue_ping(state: &AppState, post_id: i64) {
    let Ok(site_url) = state.options_service.site_url().await else {
        return;
    };
    if !announceable(&site_url) {
        return;
    }
    let Ok(post) = state.posts.get(post_id).await else {
        return;
    };
    // Only URLs a crawler can fetch: published, public type, no gate.
    if !matches!(post.status, vyasa_db::content_models::PostStatus::Published)
        || post.password_hash.is_some()
        || !crate::policy::publicly_openable(&state.plugin_surface, &state.pool, post.post_type)
            .await
    {
        return;
    }
    let Some(key) = ensure_key(state).await else {
        return;
    };
    let path = crate::permalinks::path(state, &post).await;
    let payload = serde_json::json!({
        "url": format!("{site_url}{path}"),
        "key": key,
        "key_location": format!("{site_url}/indexnow.txt"),
    });
    if let Err(err) = vyasa_jobs::queue::enqueue(&state.pool, "indexnow_ping", payload, None).await
    {
        tracing::warn!("indexnow enqueue failed: {err}");
        return;
    }
    state.publisher_notify.notify_one();
}

#[cfg(test)]
mod tests {
    use super::announceable;

    #[test]
    fn only_public_addresses_are_announced() {
        assert!(announceable("https://blog.example.com"));
        assert!(announceable("https://blog.example/sub"));
        assert!(!announceable("http://localhost:3000"));
        assert!(!announceable("http://127.0.0.1:3000"));
        assert!(!announceable("https://192.168.1.4"));
        assert!(!announceable("https://10.0.0.9"));
        assert!(!announceable(""));
        assert!(!announceable("https://intranet"));
    }
}
