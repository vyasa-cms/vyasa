//! The newsletter digest: publishing an entry emails confirmed
//! subscribers — one plain email per post, each carrying its own
//! unsubscribe link.
//!
//! Gated twice: the `newsletter_enabled` option defaults to off (a
//! publish must never surprise-email anyone), and without a `site_url`
//! there are no working links to send, so nothing goes out.

use vyasa_core::events::{self, Event};

use crate::state::AppState;

/// Watches the bus; every publish fans out at most once.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    let mut rx = events::subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(Event::Published(p)) => send_digest(&state, p.post_id).await,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    });
}

async fn send_digest(state: &AppState, post_id: i64) {
    let enabled = state
        .options
        .get("newsletter_enabled")
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let Ok(base) = state.options_service.site_url().await else {
        return;
    };
    if base.is_empty() {
        return;
    }
    let Ok(post) = state.posts.get(post_id).await else {
        return;
    };
    // Only entries a subscriber could actually open.
    if post.password_hash.is_some()
        || !crate::policy::publicly_openable(&state.plugin_surface, &state.pool, post.post_type)
            .await
    {
        return;
    }
    let subscribers = match state.audience.confirmed().await {
        Ok(list) => list,
        Err(err) => {
            tracing::warn!("newsletter: could not list subscribers: {err}");
            return;
        }
    };
    if subscribers.is_empty() {
        return;
    }
    let site = state
        .options_service
        .site_identity()
        .await
        .map(|i| i.title)
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| base.clone());
    let path = crate::permalinks::path(state, &post).await;
    let mailer = vyasa_core::notify::EmailService::new(state.pool.clone());
    let mut queued = 0usize;
    for sub in &subscribers {
        let ctx = serde_json::json!({
            "site": site,
            "post_title": post.title,
            "excerpt": post.excerpt.as_deref().unwrap_or("").trim(),
            "url": format!("{base}{path}"),
            "unsubscribe_url":
                format!("{base}/newsletter/unsubscribe?token={}", sub.token),
        });
        match mailer.queue(&sub.email, "newsletter_post", &ctx).await {
            Ok(_) => queued += 1,
            Err(err) => tracing::warn!("newsletter: queue to one address failed: {err}"),
        }
    }
    if queued > 0 {
        state.publisher_notify.notify_one();
        tracing::info!(post_id, queued, "newsletter: digest queued");
    }
}
