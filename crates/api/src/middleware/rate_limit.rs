//! In-process per-IP rate limiting (single-node; Redis noted for multi-instance).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Sliding-window counter per IP per bucket.
#[derive(Default)]
struct Window {
    count: u32,
    window_start: Option<Instant>,
}
/// Rate limiter with named buckets and configurable limits.
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<(String, String), Window>>>,
    /// When expired windows were last swept out.
    last_sweep: Mutex<Option<Instant>>,
    limits: HashMap<String, (u32, Duration)>,
}

impl RateLimiter {
    /// Creates a limiter from `(bucket, max_requests, window)` triples.
    #[must_use]
    pub fn new(limits: Vec<(&str, u32, u64)>) -> Self {
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
            last_sweep: Mutex::new(None),
            limits: limits
                .into_iter()
                .map(|(name, max, secs)| (name.to_owned(), (max, Duration::from_secs(secs))))
                .collect(),
        }
    }

    /// Checks whether `ip` is allowed for `bucket`; returns false when
    /// rate exceeded.
    #[must_use]
    pub fn check(&self, bucket: &str, ip: &str) -> bool {
        let Some(&(max, window)) = self.limits.get(bucket) else {
            return true;
        };
        let mut map = self
            .buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (bucket.to_owned(), ip.to_owned());
        let now = Instant::now();
        // Opportunistic sweep: keys include addresses a stranger types,
        // so without it the map only ever grows. At most once per
        // interval, so a map that stays large is not scanned per request.
        if map.len() >= SWEEP_ABOVE && self.sweep_due(now) {
            let limits = &self.limits;
            map.retain(|(name, _), w| {
                let window = limits.get(name).map_or(Duration::ZERO, |l| l.1);
                w.window_start
                    .is_some_and(|start| now.duration_since(start) <= window)
            });
        }
        let entry = map.entry(key).or_insert_with(|| Window {
            count: 0,
            window_start: Some(now),
        });
        let start = entry.window_start.unwrap_or(now);
        if now.duration_since(start) > window {
            entry.count = 0;
            entry.window_start = Some(now);
        }
        if entry.count >= max {
            return false;
        }
        entry.count += 1;
        true
    }
}

#[cfg(test)]
impl RateLimiter {
    /// Every `(bucket, key)` the limiter holds a window for.
    pub(crate) fn keys(&self) -> Vec<(String, String)> {
        self.buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}

/// Windows kept before expired ones are swept out.
const SWEEP_ABOVE: usize = 10_000;

/// Registrations one client address may make an hour (phase 98). Checked
/// by the handler, which knows a request is a well-formed registration.
pub const REGISTER: &str = "register";

/// Requests to send a confirmation again one client address may make an
/// hour.
pub const REGISTER_RESEND: &str = "register-resend";

/// Confirmation mails one email address may be sent an hour, whichever
/// route and client address ask. Keyed by the address ([`address_key`]),
/// not the client. A password-reset request for an unconfirmed account
/// counts here too.
pub const CONFIRMATION_MAIL: &str = "confirmation-mail";

/// Password-reset requests (`/auth/forgot`) one email address may make an
/// hour, whatever the account and whoever asks. Keyed by
/// [`address_key`].
pub const RESET_MAIL: &str = "reset-mail";

/// The key an email address is counted under by every per-address bucket:
/// [`address_digest`] in hex, so 64 bytes whatever was typed. `None` for
/// anything that is not an address by registration's rule
/// ([`vyasa_core::user::normalized_email`]: at most 254 bytes, an `@`
/// between a local part and a domain, no whitespace), applied to the
/// folded address ([`fold_address`]) so every spelling the database finds
/// the account by is judged alike. A caller answers such a request exactly
/// as any other and counts nothing: no mail can go to it, and counting it
/// would have the limiter hold, for an hour, whatever a client typed.
#[must_use]
pub fn address_key(email: &str) -> Option<String> {
    let folded = fold_address(email);
    vyasa_core::user::normalized_email(&folded).ok()?;
    Some(hex::encode(digest(&folded)))
}

/// The SHA-256 of an address as the database compares it
/// ([`fold_address`]): every spelling that finds the account is one
/// digest. Not checked: the sign-in lockout counts every attempt by it,
/// whatever was typed.
#[must_use]
pub fn address_digest(email: &str) -> [u8; 32] {
    digest(&fold_address(email))
}

/// An address as `citext` compares it: surrounding whitespace dropped and
/// each character lowercased as PostgreSQL's `lower()` does, one character
/// for one. Rust's full mapping turns 'İ' (U+0130) into two characters
/// ("i" and a combining dot); the database makes it 'i', so the first is
/// taken. The Kelvin sign (U+212A) is 'k' either way.
/// One definition, shared with registration's length rule
/// ([`vyasa_core::user::fold_email`]).
#[must_use]
pub fn fold_address(email: &str) -> String {
    vyasa_core::user::fold_email(email)
}

fn digest(folded: &str) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(folded.as_bytes()).into()
}

/// The least time between two sweeps.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

impl RateLimiter {
    /// Whether a sweep may run now; records it when so.
    fn sweep_due(&self, now: Instant) -> bool {
        let mut last = self
            .last_sweep
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|at| now.duration_since(at) < SWEEP_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// Buckets, chosen so a brute-force attempt on the login endpoint is limited
/// far more tightly than ordinary reads.
///
/// `(bucket, max requests, window seconds)`. The last four are not picked
/// by [`bucket_for`]: handlers check them where the key is known.
pub const DEFAULT_LIMITS: [(&str, u32, u64); 8] = [
    // Credential endpoints: enough for a person who mistypes, not enough to
    // grind through a password list.
    ("auth", 10, 60),
    // Public lead/newsletter forms: a person submits once, maybe twice.
    ("form", 5, 60),
    // Anything that changes state.
    ("write", 60, 60),
    // Everything else, as a backstop against scraping.
    ("global", 300, 60),
    (REGISTER, 5, 3600),
    (REGISTER_RESEND, 5, 3600),
    (CONFIRMATION_MAIL, 3, 3600),
    (RESET_MAIL, 3, 3600),
];

/// Picks the bucket for a request. Most specific wins.
fn bucket_for(method: &Method, path: &str) -> &'static str {
    if path.starts_with("/api/v1/auth/login")
        || path.starts_with("/api/v1/users") && method == Method::POST
    {
        "auth"
    } else if path == "/form" && method == Method::POST {
        "form"
    } else if matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        "write"
    } else {
        "global"
    }
}

/// Enforces per-IP limits, rejecting with 429 once a bucket is exhausted.
///
/// The client address comes from [`crate::client_ip`]: the socket peer,
/// unless that peer is a configured trusted proxy. A request that reaches
/// the port directly could otherwise write any address it liked into a
/// forwarding header and take a fresh bucket per request.
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip());
    let ip = crate::client_ip::for_request(&state, peer, req.headers());
    let bucket = bucket_for(req.method(), req.uri().path());
    if !state.rate_limiter.check(bucket, &ip) {
        return too_many_requests();
    }
    next.run(req).await
}

/// Returns 429 Too Many Requests with Retry-After.
#[must_use]
pub fn too_many_requests() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, "60")],
        "rate limited",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_rejects() {
        let limiter = RateLimiter::new(vec![("auth", 3, 60)]);
        for attempt in 1..=3 {
            assert!(
                limiter.check("auth", "1.2.3.4"),
                "attempt {attempt} allowed"
            );
        }
        assert!(!limiter.check("auth", "1.2.3.4"), "fourth is rejected");
    }

    #[test]
    fn limits_are_per_ip_and_per_bucket() {
        let limiter = RateLimiter::new(vec![("auth", 1, 60), ("write", 1, 60)]);
        assert!(limiter.check("auth", "1.1.1.1"));
        assert!(!limiter.check("auth", "1.1.1.1"));
        // A different address is unaffected...
        assert!(limiter.check("auth", "2.2.2.2"));
        // ...and so is a different bucket for the same address.
        assert!(limiter.check("write", "1.1.1.1"));
    }

    #[test]
    fn expired_windows_are_swept_once_the_map_is_large() {
        let limiter = RateLimiter::new(vec![("short", 1, 0), ("long", 1, 3600)]);
        assert!(limiter.check("long", "kept"));
        for n in 0..SWEEP_ABOVE {
            let _ = limiter.check("short", &n.to_string());
        }
        std::thread::sleep(Duration::from_millis(5));
        // Past the threshold: the expired short windows go, the live one
        // stays and still counts.
        let _ = limiter.check("short", "trigger");
        let len = limiter.buckets.lock().unwrap().len();
        assert!(len <= 3, "{len}");
        assert!(!limiter.check("long", "kept"));
        // Refilled past the threshold at once: no second sweep until the
        // interval has passed, so a full map is not scanned on every check.
        for n in 0..SWEEP_ABOVE {
            let _ = limiter.check("short", &format!("again-{n}"));
        }
        std::thread::sleep(Duration::from_millis(5));
        let _ = limiter.check("short", "trigger-again");
        let len = limiter.buckets.lock().unwrap().len();
        assert!(len > SWEEP_ABOVE, "{len}");
    }

    #[test]
    fn an_address_is_counted_by_its_digest_and_nothing_else_is_counted() {
        let key = address_key("Ada@Example.test").expect("an address");
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        // Every spelling that finds the account is one key.
        assert_eq!(address_key("  ada@example.TEST ").as_ref(), Some(&key));
        assert_ne!(address_key("bob@example.test").as_ref(), Some(&key));
        let long = format!("{}@example.test", "a".repeat(250));
        for bad in [
            "",
            "   ",
            "no-at-sign",
            "@example.test",
            "a@",
            "two words@x.test",
            &long,
        ] {
            assert!(address_key(bad).is_none(), "{bad:?}");
        }
        // At the limit is still an address.
        let at_limit = format!("{}@x.test", "a".repeat(254 - "@x.test".len()));
        assert_eq!(address_key(&at_limit).map(|k| k.len()), Some(64));
    }

    #[test]
    fn addresses_fold_as_the_database_compares_them() {
        // citext compares by PostgreSQL's lower(), one character for one:
        // 'İ' is 'i' there, where Rust's full mapping gives "i\u{307}".
        let info = address_key("info@example.test").unwrap();
        assert_eq!(address_key("\u{130}nfo@example.test"), Some(info.clone()));
        assert_eq!(address_key("\u{130}NFO@EXAMPLE.TEST"), Some(info));
        assert_eq!(
            address_digest("\u{130}nfo@example.test"),
            address_digest("info@example.test")
        );
        // The Kelvin sign is 'k'.
        assert_eq!(
            address_key("\u{212A}ate@example.test"),
            address_key("kate@example.test")
        );
        // The length rule is the folded address's: a Kelvin spelling of an
        // address at the limit is that address (and finds its account),
        // and one character more is past it however it is spelt.
        let at_limit = format!("{}@x.test", "k".repeat(254 - "@x.test".len()));
        let kelvin = at_limit.replace('k', "\u{212A}");
        assert!(kelvin.len() > 254);
        assert!(address_key(&at_limit).is_some());
        assert_eq!(address_key(&kelvin), address_key(&at_limit));
        let past = format!("k{at_limit}");
        assert!(address_key(&past).is_none());
        assert!(address_key(&past.replace('k', "\u{212A}")).is_none());
    }

    /// An address registration accepts always has a bucket: the rule is
    /// the folded address's in both places. 'Ⱥ' (U+023A) grows by a byte
    /// when folded, so a spelling that fits as typed can be past the limit
    /// folded — and registration refuses it rather than accepting an
    /// account that could never be sent a link.
    #[test]
    fn an_address_registration_accepts_always_gets_a_bucket() {
        let domain = "@x.test";
        for n in 100..=130 {
            let raw = format!("{}{domain}", "\u{23A}".repeat(n));
            let accepted = vyasa_core::user::normalized_email(&raw).is_ok();
            if accepted {
                assert!(address_key(&raw).is_some(), "accepted but no bucket: {n}");
            }
        }
        let fits = format!("{}{domain}", "\u{23A}".repeat(82));
        assert!(fold_address(&fits).len() <= 254);
        assert!(vyasa_core::user::normalized_email(&fits).is_ok());
        assert!(address_key(&fits).is_some());
        let grows = format!("{}{domain}", "\u{23A}".repeat(123));
        assert!(grows.len() <= 254 && fold_address(&grows).len() > 254);
        assert!(vyasa_core::user::normalized_email(&grows).is_err());
        assert!(address_key(&grows).is_none());
    }

    /// The fold is checked against the database it stands in for.
    #[tokio::test]
    async fn the_fold_is_what_the_database_does() {
        let db = vyasa_testkit::TestDb::new().await;
        for raw in [
            "\u{130}nfo@Example.TEST",
            "\u{212A}ate@example.test",
            "\u{c5}SA@\u{d6}RE.TEST",
            "  Ada@Example.Test ",
        ] {
            let lowered: String = sqlx::query_scalar("SELECT lower(btrim($1))")
                .bind(raw)
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(fold_address(raw), lowered, "{raw:?}");
            let same: bool = sqlx::query_scalar("SELECT $1::citext = $2::citext")
                .bind(raw.trim())
                .bind(fold_address(raw))
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert!(same, "{raw:?}");
        }
    }

    #[test]
    fn an_unknown_bucket_is_never_limited() {
        let limiter = RateLimiter::new(vec![("auth", 1, 60)]);
        for _ in 0..100 {
            assert!(limiter.check("not-configured", "1.1.1.1"));
        }
    }

    #[test]
    fn login_is_bucketed_more_tightly_than_reads() {
        assert_eq!(bucket_for(&Method::POST, "/api/v1/auth/login"), "auth");
        assert_eq!(bucket_for(&Method::POST, "/api/v1/posts"), "write");
        assert_eq!(bucket_for(&Method::DELETE, "/api/v1/posts/1"), "write");
        assert_eq!(bucket_for(&Method::GET, "/api/v1/posts"), "global");
        assert_eq!(bucket_for(&Method::GET, "/"), "global");
    }

    #[test]
    fn the_login_bucket_is_strictest() {
        let by_name = |name: &str| {
            DEFAULT_LIMITS
                .iter()
                .find(|(b, _, _)| *b == name)
                .map(|(_, max, _)| *max)
                .expect("bucket present")
        };
        assert!(by_name("auth") < by_name("write"));
        assert!(by_name("write") < by_name("global"));
    }
}
