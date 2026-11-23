//! Cookieless analytics: the server counts its own pages.
//!
//! No beacon script, no cookies, no stored IPs — a request that leaves
//! with an HTML 200 on a public path increments day counters, and those
//! aggregates are all that ever exists:
//!
//! * a path × referrer-host view counter, bots counted on their own line;
//! * device class, browser family and `utm_*` tags as day counters;
//! * entry pages, 404s with their referrers and internal search terms;
//! * a unique-visitor count keyed by `sha256(day secret, client, agent,
//!   host)`. The secret is random, lives in memory for one UTC day and is
//!   never written anywhere; the day's hashes are deleted when the next
//!   day's first visit arrives. A restart mid-day starts a new secret, so a
//!   visitor can count twice that day, never twice across days.
//!
//! Not counted at all: requests carrying `X-Vyasa-Internal: 1` (link
//! checks, staging QA), `DNT: 1` or `Sec-GPC: 1`, and anyone holding a
//! session cookie (a signed-in author or admin). Bots are filtered by the
//! user agents they announce; the ones that lie are noise this feature can
//! afford.

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use chrono::NaiveDate;
use sha2::{Digest, Sha256};
use vyasa_db::repo::audience::dim;

use crate::state::AppState;

/// Substrings that mark a self-declared crawler.
const BOT_MARKS: &[&str] = &[
    "bot",
    "crawl",
    "spider",
    "slurp",
    "preview",
    "headless",
    "curl",
    "wget",
    "python-requests",
    "monitor",
    "lighthouse",
    "pingdom",
    "facebookexternalhit",
];

/// True for a user agent that announces automation (or announces
/// nothing, which no browser does).
pub(crate) fn is_bot(user_agent: Option<&str>) -> bool {
    match user_agent {
        None => true,
        Some(ua) => {
            let lower = ua.to_ascii_lowercase();
            lower.is_empty() || BOT_MARKS.iter().any(|m| lower.contains(m))
        }
    }
}

/// A path worth counting: public HTML territory only.
fn countable_path(path: &str) -> bool {
    !(path.starts_with("/admin")
        || path.starts_with("/api")
        || path.starts_with("/preview")
        || path.starts_with("/metrics")
        || path == "/favicon.ico")
}

/// The authority (host and port, no userinfo) of an `http(s)` URL, or `None`.
fn authority_of(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or(auth);
    (!auth.is_empty()).then_some(auth)
}

/// True when the `Referer` names this same host (port included, case-insensitive).
fn same_site_referrer(referer: Option<&str>, own_host: &str) -> bool {
    !own_host.is_empty()
        && referer
            .and_then(authority_of)
            .is_some_and(|h| h.eq_ignore_ascii_case(own_host))
}

/// The referring *host*, and only when it is a different site — the
/// visitor's journey inside this site is not the analytics' business.
fn referrer_host(referer: Option<&str>, own_host: &str) -> String {
    let Some(auth) = referer.and_then(authority_of) else {
        return String::new();
    };
    if same_site_referrer(referer, own_host) {
        return String::new();
    }
    let host = auth.rsplit_once(':').map_or(auth, |(h, port)| {
        if port.bytes().all(|b| b.is_ascii_digit()) {
            h
        } else {
            auth
        }
    });
    host.chars().take(100).collect()
}

/// Why a request is not counted at all (not even as a bot).
pub(crate) fn opted_out(headers: &HeaderMap) -> bool {
    let is_one = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim() == "1")
    };
    is_one("x-vyasa-internal")
        || is_one("dnt")
        || is_one("sec-gpc")
        || headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|c| {
                c.split(';').any(|kv| {
                    kv.trim()
                        .strip_prefix(crate::middleware::SESSION_COOKIE)
                        .is_some_and(|rest| rest.starts_with('='))
                })
            })
}

/// `desktop`, `mobile` or `tablet`, from the announced user agent.
fn device_class(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    if ua.contains("ipad")
        || ua.contains("tablet")
        || (ua.contains("android") && !ua.contains("mobile"))
    {
        "tablet"
    } else if ua.contains("mobi") || ua.contains("iphone") || ua.contains("android") {
        "mobile"
    } else {
        "desktop"
    }
}

/// The browser family, from the announced user agent. Order matters: Edge, Opera and Chrome all say "Chrome".
fn browser_family(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    if ua.contains("edg/") || ua.contains("edga/") || ua.contains("edgios/") {
        "Edge"
    } else if ua.contains("opr/") || ua.contains("opera") {
        "Opera"
    } else if ua.contains("firefox") || ua.contains("fxios") {
        "Firefox"
    } else if ua.contains("chrome") || ua.contains("crios") {
        "Chrome"
    } else if ua.contains("safari") {
        "Safari"
    } else {
        "Other"
    }
}

/// A tag value kept for the day counters: trimmed, lower-cased, at most 60 characters.
fn tag_value(raw: &str) -> Option<String> {
    let v: String = raw
        .trim()
        .chars()
        .take(60)
        .collect::<String>()
        .to_lowercase();
    (!v.is_empty()).then_some(v)
}

/// The `utm_source`, `utm_medium` and `utm_campaign` values of a query string, as `(dimension, value)`.
fn utm_tags(query: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        let d = match k.as_ref() {
            "utm_source" => dim::UTM_SOURCE,
            "utm_medium" => dim::UTM_MEDIUM,
            "utm_campaign" => dim::UTM_CAMPAIGN,
            _ => continue,
        };
        // One value per dimension per request: a query string cannot fan out into many writes.
        if out.iter().all(|(od, _)| *od != d) {
            if let Some(v) = tag_value(&v) {
                out.push((d, v));
            }
        }
    }
    out
}

/// The internal search term (`/search?s=...`), tidied for counting.
fn search_term(path: &str, query: &str) -> Option<String> {
    if path != "/search" {
        return None;
    }
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "s")
        .and_then(|(_, v)| tag_value(&v))
}

/// The day's visitor secret: random, in memory only, replaced when the UTC day changes.
static DAY_SECRET: Mutex<Option<(NaiveDate, [u8; 32])>> = Mutex::new(None);

/// `sha256(day secret, client, agent, host)` for `day`. The secret of an earlier day is dropped on the way, so
/// yesterday's hashes can no longer be recomputed by anyone.
fn visitor_hash(day: NaiveDate, client: &str, user_agent: &str, host: &str) -> [u8; 32] {
    let secret = {
        let mut guard = DAY_SECRET
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *guard {
            Some((d, s)) if d == day => s,
            _ => {
                let s: [u8; 32] = rand::random();
                *guard = Some((day, s));
                s
            }
        }
    };
    let mut h = Sha256::new();
    h.update(secret);
    for part in [client, user_agent, host] {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part.as_bytes());
    }
    h.finalize().into()
}

/// Everything read from the request before the handler consumes it.
struct Facts {
    is_get: bool,
    path: String,
    query: String,
    ua: Option<String>,
    referer: Option<String>,
    host: String,
    skip: bool,
    client: String,
}

fn facts(state: &AppState, req: &Request) -> Facts {
    let header = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let peer: Option<IpAddr> = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let client = crate::client_ip::for_request(state, peer, req.headers());
    Facts {
        is_get: req.method() == axum::http::Method::GET,
        path: req.uri().path().chars().take(200).collect(),
        query: req
            .uri()
            .query()
            .unwrap_or_default()
            .chars()
            .take(500)
            .collect(),
        ua: header("user-agent"),
        referer: header("referer"),
        // HTTP/2 carries the host in `:authority`, not in a `Host` header.
        host: header("host")
            .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()))
            .unwrap_or_default(),
        skip: opted_out(req.headers()),
        client,
    }
}

/// Counters older than this many days are deleted.
const KEEP_DAYS: i32 = 400;

/// Spawns the purge: at startup and every ten minutes it deletes visitor hashes of earlier UTC days and day counters
/// past [`KEEP_DAYS`], so no identifier outlives its day even when no visit arrives to trigger it.
pub fn spawn_purge(audience: vyasa_db::repo::AudienceRepo) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(600));
        loop {
            tick.tick().await;
            if let Err(err) = audience
                .purge(chrono::Utc::now().date_naive(), KEEP_DAYS)
                .await
            {
                tracing::warn!("audience purge failed: {err}");
            }
        }
    })
}

/// Counts the response on its way out. Never blocks it: the writes are
/// spawned, and a failed write is a lost view, not a lost page.
pub async fn middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Scoped: the request's body is !Sync, and keeping a borrow of it across the await below would make this future
    // !Send — which axum's layer bound rejects. `Facts` owns what it read.
    let f = facts(&state, &req);
    let response = next.run(req).await;

    if f.skip || !f.is_get || !countable_path(&f.path) {
        return response;
    }
    let status = response.status();
    let html = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    if !html
        || !(status == axum::http::StatusCode::OK || status == axum::http::StatusCode::NOT_FOUND)
    {
        return response;
    }
    let audience = state.audience.clone();
    tokio::spawn(async move {
        if let Err(err) = record(&audience, &f, status == axum::http::StatusCode::OK).await {
            tracing::debug!("view not recorded: {err}");
        }
    });
    response
}

/// The counters one counted response moves.
async fn record(
    audience: &vyasa_db::repo::AudienceRepo,
    f: &Facts,
    ok: bool,
) -> Result<(), vyasa_common::AppError> {
    // One UTC day for the secret, the visitor row and every counter of this view.
    let day = chrono::Utc::now().date_naive();
    if is_bot(f.ua.as_deref()) {
        return audience.count_on(day, dim::BOT, "", "", 1).await;
    }
    let from_host = referrer_host(f.referer.as_deref(), &f.host);
    if !ok {
        return audience.count(dim::NOT_FOUND, &f.path, &from_host, 1).await;
    }
    let ua = f.ua.as_deref().unwrap_or_default();
    audience.record_view(&f.path, &from_host).await?;
    audience.count(dim::DEVICE, device_class(ua), "", 1).await?;
    audience
        .count(dim::BROWSER, browser_family(ua), "", 1)
        .await?;
    for (d, v) in utm_tags(&f.query) {
        audience.count(d, &v, "", 1).await?;
    }
    // An entry page is the first page of a visit: nothing on this site sent the visitor.
    if !same_site_referrer(f.referer.as_deref(), &f.host) {
        audience.count(dim::ENTRY, &f.path, "", 1).await?;
    }
    if let Some(term) = search_term(&f.path, &f.query) {
        audience.count(dim::SEARCH, &term, "", 1).await?;
    }
    let hash = visitor_hash(day, &f.client, ua, &f.host);
    if audience.first_visit_on(day, &hash).await? {
        audience.count(dim::VISITOR, "", "", 1).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{countable_path, is_bot, referrer_host};

    #[test]
    fn bots_and_blank_agents_do_not_count() {
        assert!(is_bot(None));
        assert!(is_bot(Some("Googlebot/2.1")));
        assert!(is_bot(Some("Mozilla/5.0 (compatible; bingbot/2.0)")));
        assert!(is_bot(Some("curl/8.5")));
        assert!(!is_bot(Some(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/126 Safari/537.36"
        )));
    }

    #[test]
    fn only_public_pages_count() {
        assert!(countable_path("/"));
        assert!(countable_path("/post/hello"));
        assert!(!countable_path("/admin/posts"));
        assert!(!countable_path("/api/v1/posts"));
        assert!(!countable_path("/favicon.ico"));
    }

    #[test]
    fn referrers_keep_the_host_and_drop_self_traffic() {
        assert_eq!(
            referrer_host(
                Some("https://news.ycombinator.com/item?id=1"),
                "blog.example"
            ),
            "news.ycombinator.com"
        );
        assert_eq!(
            referrer_host(Some("https://blog.example/post/a"), "blog.example"),
            ""
        );
        assert_eq!(referrer_host(None, "blog.example"), "");
        assert_eq!(referrer_host(Some("garbage"), "x"), "");
    }

    #[test]
    fn opt_outs_and_internal_traffic_are_not_counted() {
        use axum::http::HeaderMap;
        let h = |pairs: &[(&'static str, &str)]| {
            let mut m = HeaderMap::new();
            for (k, v) in pairs {
                m.append(*k, v.parse().unwrap());
            }
            m
        };
        assert!(!super::opted_out(&h(&[("user-agent", "Mozilla/5.0")])));
        assert!(super::opted_out(&h(&[("dnt", "1")])));
        assert!(super::opted_out(&h(&[("sec-gpc", "1")])));
        assert!(super::opted_out(&h(&[("x-vyasa-internal", "1")])));
        assert!(!super::opted_out(&h(&[("dnt", "0")])));
        let cookie = format!("a=b; {}=token", crate::middleware::SESSION_COOKIE);
        assert!(super::opted_out(&h(&[("cookie", cookie.as_str())])));
        assert!(!super::opted_out(&h(&[("cookie", "other=1")])));
    }

    #[test]
    fn devices_and_browsers_are_classed() {
        let iphone =
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit Mobile/15E148 Safari/604.1";
        let ipad = "Mozilla/5.0 (iPad; CPU OS 17_0) AppleWebKit Safari/604.1";
        let desk = "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/126.0";
        let edge = "Mozilla/5.0 (Windows NT 10.0) Chrome/126 Safari/537.36 Edg/126";
        assert_eq!(super::device_class(iphone), "mobile");
        assert_eq!(super::device_class(ipad), "tablet");
        assert_eq!(super::device_class(desk), "desktop");
        assert_eq!(super::browser_family(desk), "Firefox");
        assert_eq!(super::browser_family(edge), "Edge");
        assert_eq!(super::browser_family(iphone), "Safari");
        assert_eq!(super::browser_family("something"), "Other");
    }

    #[test]
    fn campaign_tags_and_search_terms_are_read_and_tidied() {
        let tags =
            super::utm_tags("utm_source=Newsletter&utm_medium=email&x=1&utm_campaign=%20Launch%20");
        assert_eq!(
            tags,
            vec![
                ("utm_source", "newsletter".to_owned()),
                ("utm_medium", "email".to_owned()),
                ("utm_campaign", "launch".to_owned()),
            ]
        );
        assert!(super::utm_tags("utm_source=").is_empty());
        assert_eq!(
            super::search_term("/search", "s=Rust+CMS"),
            Some("rust cms".to_owned())
        );
        assert_eq!(super::search_term("/search", "s="), None);
        assert_eq!(super::search_term("/post/a", "s=x"), None);
    }

    #[test]
    fn a_visitor_hash_is_stable_within_a_day_and_new_on_the_next() {
        let d1 = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let d2 = chrono::NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let a = super::visitor_hash(d1, "203.0.113.7", "UA", "site");
        assert_eq!(a, super::visitor_hash(d1, "203.0.113.7", "UA", "site"));
        assert_ne!(a, super::visitor_hash(d1, "203.0.113.8", "UA", "site"));
        assert_ne!(a, super::visitor_hash(d1, "203.0.113.7", "UA2", "site"));
        // The boundaries are part of the hash: moving a character between fields changes it.
        assert_ne!(
            super::visitor_hash(d1, "ab", "c", "site"),
            super::visitor_hash(d1, "a", "bc", "site")
        );
        assert_ne!(a, super::visitor_hash(d2, "203.0.113.7", "UA", "site"));
    }

    #[test]
    fn a_host_with_a_port_matches_its_own_referrer() {
        assert!(super::same_site_referrer(
            Some("http://localhost:3000/a"),
            "localhost:3000"
        ));
        assert_eq!(
            super::referrer_host(Some("http://localhost:3000/a"), "localhost:3000"),
            ""
        );
        assert_eq!(
            super::referrer_host(Some("https://news.example:8443/a"), "blog.example"),
            "news.example"
        );
        // No Host at all: nothing is "same site", and nothing panics.
        assert!(!super::same_site_referrer(Some("https://x.example/"), ""));
    }

    #[test]
    fn one_campaign_value_per_dimension() {
        let tags = super::utm_tags("utm_source=a&utm_source=b&utm_source=c");
        assert_eq!(tags, vec![("utm_source", "a".to_owned())]);
    }

    #[test]
    fn a_same_site_referrer_is_not_an_entry() {
        assert!(super::same_site_referrer(
            Some("https://blog.example/a"),
            "blog.example"
        ));
        assert!(!super::same_site_referrer(
            Some("https://news.example/a"),
            "blog.example"
        ));
        assert!(!super::same_site_referrer(None, "blog.example"));
    }
}
