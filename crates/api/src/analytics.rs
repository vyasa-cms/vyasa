//! Cookieless analytics: the server counts its own pages.
//!
//! No beacon script, no cookies, no fingerprints, no IPs — a request
//! that leaves with an HTML 200 on a public path increments one
//! day × path × referrer-host counter, and that aggregate is all that
//! ever exists. Bots are filtered by the user agents they announce;
//! the ones that lie are noise this feature can afford.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

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
fn is_bot(user_agent: Option<&str>) -> bool {
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

/// The referring *host*, and only when it is a different site — the
/// visitor's journey inside this site is not the analytics' business.
fn referrer_host(referer: Option<&str>, own_host: &str) -> String {
    let Some(r) = referer else {
        return String::new();
    };
    let host = r
        .strip_prefix("https://")
        .or_else(|| r.strip_prefix("http://"))
        .unwrap_or("")
        .split(['/', ':', '?'])
        .next()
        .unwrap_or("");
    if host.is_empty() || host.eq_ignore_ascii_case(own_host) {
        String::new()
    } else {
        host.chars().take(100).collect()
    }
}

/// Counts the response on its way out. Never blocks it: the write is
/// spawned, and a failed write is a lost view, not a lost page.
pub async fn middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let count = req.method() == axum::http::Method::GET && countable_path(req.uri().path());
    let path: String = req.uri().path().chars().take(200).collect();
    // Scoped: the closure borrows the request (whose body is !Sync), and
    // letting it live across the await below would make this future
    // !Send — which axum's layer bound rejects.
    let (ua, referer, host) = {
        let header = |name: &str| {
            req.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        (
            header("user-agent"),
            header("referer"),
            header("host").unwrap_or_default(),
        )
    };

    let response = next.run(req).await;

    if count
        && response.status() == axum::http::StatusCode::OK
        && response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("text/html"))
        && !is_bot(ua.as_deref())
    {
        let from_host = referrer_host(referer.as_deref(), &host);
        let audience = state.audience.clone();
        tokio::spawn(async move {
            if let Err(err) = audience.record_view(&path, &from_host).await {
                tracing::debug!("view not recorded: {err}");
            }
        });
    }
    response
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
}
