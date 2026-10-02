//! Webhook fan-out service: event → subscribed webhooks → signed payloads.

use vyasa_common::AppError;

/// Header carrying `t=<unix>,v1=<hex hmac>`.
pub const SIGNATURE_HEADER: &str = "X-Vyasa-Signature";

/// Events a webhook can subscribe to (v1 fixed list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookEvent {
    /// Post published.
    PostPublished,
    /// Post updated.
    PostUpdated,
    /// Post trashed.
    PostTrashed,
    /// Comment added.
    CommentAdded,
    /// Theme changed.
    ThemeChanged,
}

impl WebhookEvent {
    /// Every event a subscription may name, in wire form.
    ///
    /// Exposed so the REST layer can reject a subscription to an event that
    /// will never fire — silently accepting one leaves an integration that
    /// looks configured and never delivers.
    pub const ALL: [Self; 5] = [
        Self::PostPublished,
        Self::PostUpdated,
        Self::PostTrashed,
        Self::CommentAdded,
        Self::ThemeChanged,
    ];

    /// Parses a wire name, or `None` when no such event exists.
    ///
    /// Deliberately not `FromStr`: an unknown event is an ordinary,
    /// expected outcome here rather than a parse error, and `Option` says
    /// that more plainly than a `Result` with a synthetic error type.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == name)
    }

    /// Canonical wire name used in payloads + subscriptions.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PostPublished => "post.published",
            Self::PostUpdated => "post.updated",
            Self::PostTrashed => "post.trashed",
            Self::CommentAdded => "comment.added",
            Self::ThemeChanged => "theme.changed",
        }
    }
}
use vyasa_db::repo::WebhooksRepo;

/// Builds the versioned payload envelope: `{v:1, event, occurred_at, data}`.
#[must_use]
#[allow(clippy::needless_pass_by_value)] // envelope consumes caller data
pub fn envelope(event: &str, occurred_at_unix: u64, data: serde_json::Value) -> String {
    serde_json::json!({
        "v": 1,
        "event": event,
        "occurred_at": occurred_at_unix,
        "data": data,
    })
    .to_string()
}

/// Signs `body` for delivery at unix `timestamp`:
/// header value = `t=<unix>,v1=<hex hmac-sha256(secret, "<t>.<body>")>`.
#[must_use]
pub fn sign(secret: &[u8], timestamp: u64, body: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if secret.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(secret));
    } else {
        k[..secret.len()].copy_from_slice(secret);
    }
    let inner_input: Vec<u8> = k
        .iter()
        .map(|b| b ^ 0x36)
        .chain(format!("{timestamp}.{body}").bytes())
        .collect();
    let inner_hash = Sha256::digest(&inner_input);
    let outer_input: Vec<u8> = k
        .iter()
        .map(|b| b ^ 0x5c)
        .chain(inner_hash.iter().copied())
        .collect();
    let mac = Sha256::digest(&outer_input);
    let mut hex = String::with_capacity(mac.len() * 2);
    for b in mac {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    format!("t={timestamp},v1={hex}")
}

/// Verifies a signature header against the expected value. Rejects
/// timestamps older than 5 minutes (replay protection).
///
/// # Errors
/// [`AppError::Auth`] on stale timestamp or hash mismatch.
#[allow(dead_code)] // receiver-side helper; used in docs/tests
pub fn verify_signature(
    secret: &[u8],
    header_value: &str,
    body: &str,
    now_unix: u64,
) -> Result<(), AppError> {
    let mut t: Option<u64> = None;
    let mut v1: Option<String> = None;
    for part in header_value.split(',') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        match k {
            "t" => t = v.parse().ok(),
            "v1" => v1 = Some(v.to_owned()),
            _ => {}
        }
    }
    let Some(timestamp) = t else {
        return Err(AppError::auth("signature missing timestamp"));
    };
    if now_unix.saturating_sub(timestamp) > 300 {
        return Err(AppError::auth("signature timestamp outside replay window"));
    }
    let expected = sign(secret, timestamp, body);
    let Some(v1) = v1 else {
        return Err(AppError::auth("signature missing v1 hash"));
    };
    // Constant-time compare of just the v1 hashes.
    let Some((_t, expected_hash)) = expected.rsplit_once(",v1=") else {
        return Err(AppError::auth("malformed expected signature"));
    };
    let a = expected_hash.as_bytes();
    let b = v1.as_bytes();
    let diff = if a.len() == b.len() {
        a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
    } else {
        1
    };
    if diff == 0 {
        Ok(())
    } else {
        Err(AppError::auth("webhook signature mismatch"))
    }
}

/// The webhook fan-out service over [`WebhooksRepo`].
#[derive(Clone)]
pub struct WebhookService {
    repo: WebhooksRepo,
}

impl std::fmt::Debug for WebhookService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookService")
    }
}

impl WebhookService {
    /// Wraps a webhooks repository.
    #[must_use]
    pub fn new(repo: WebhooksRepo) -> Self {
        Self { repo }
    }

    /// Finds all enabled webhooks subscribed to `event`, returning
    /// `(id, url, secret, body)` tuples ready for delivery jobs.
    ///
    /// # Errors
    /// Propagates repo errors.
    pub async fn fan_out(
        &self,
        event: &str,
        data: serde_json::Value,
    ) -> Result<Vec<(i64, String, String, String)>, AppError> {
        let hooks = self.repo.list_for_event(event).await?;
        let now = chrono::Utc::now().timestamp().unsigned_abs();
        let body = envelope(event, now, data);
        Ok(hooks
            .into_iter()
            .map(|h| {
                let sig = sign(h.secret.as_bytes(), now, &body);
                (h.id, h.url.clone(), sig, body.clone())
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::pedantic, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let secret = b"whsec_test123";
        let body = r#"{"v":1,"event":"post.published"}"#;
        let now: u64 = 1_700_000_000;
        let sig = sign(secret, now, body);
        assert!(sig.starts_with(&format!("t={now},v1=")));
        assert!(verify_signature(secret, &sig, body, now).is_ok());
    }

    #[test]
    fn stale_timestamp_rejected() {
        let secret = b"k";
        let body = "x";
        let old = 1_000_000_000; // far past
        let sig = sign(secret, old, body);
        assert!(verify_signature(secret, &sig, body, 2_000_000_000).is_err());
    }

    #[test]
    fn tampered_body_rejected() {
        let secret = b"secret";
        let sig = sign(secret, 1_700_000_000, r#"{"a":1}"#);
        assert!(verify_signature(secret, &sig, r#"{"a":2}"#, 1_700_000_000).is_err());
    }

    #[test]
    fn every_event_name_round_trips() {
        for event in WebhookEvent::ALL {
            assert_eq!(WebhookEvent::parse(event.as_str()), Some(event));
        }
    }

    #[test]
    fn an_unknown_event_name_is_rejected() {
        // The admin UI once offered "comment.created", which the server
        // never emits; such a subscription would never have fired.
        assert!(WebhookEvent::parse("comment.created").is_none());
    }

    #[test]
    fn envelope_shape() {
        let env = envelope("post.published", 42, serde_json::json!({"id": 7}));
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["event"], "post.published");
        assert_eq!(v["occurred_at"], 42);
        assert_eq!(v["data"]["id"], 7);
    }
}
