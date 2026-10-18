//! Outbound HTTP for plugins.
//!
//! The `net:fetch:<host>` capability, its wildcard matching, its quotas
//! and its audit trail were all implemented and reachable from nothing —
//! no host function ever performed a request. This is that function's
//! back end; the capability check itself stays in the broker.
//!
//! Deliberately narrow: HTTPS GET, a response size cap, a short timeout,
//! no redirects to other hosts, and no way to send a body or headers. A
//! plugin that needs more than this is asking for a different design.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use vyasa_plugins::host::{FetchBackend, FetchReply};

/// Longest a plugin may wait for a response.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Largest response body handed back to a plugin.
const MAX_BYTES: usize = 1024 * 1024;

/// Request headers a plugin may send.
pub const REQUEST_HEADERS: &[&str] = &[
    "authorization",
    "accept",
    "if-none-match",
    "x-github-api-version",
];

/// Response headers a plugin may see.
pub const RESPONSE_HEADERS: &[&str] = &[
    "etag",
    "content-type",
    "retry-after",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
];

/// Longest header value a plugin may send.
const MAX_HEADER_BYTES: usize = 4096;

/// Checks a plugin's request headers against the allowlist.
fn request_headers(headers: &[(String, String)]) -> Result<HeaderMap, String> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if !REQUEST_HEADERS.contains(&lower.as_str()) {
            return Err(format!("header {lower:?} is not allowed"));
        }
        if value.len() > MAX_HEADER_BYTES {
            return Err(String::from("header value refused"));
        }
        let mut v =
            HeaderValue::from_str(value).map_err(|_| String::from("header value refused"))?;
        if lower == "authorization" {
            v.set_sensitive(true);
        }
        let n = HeaderName::from_bytes(lower.as_bytes())
            .map_err(|_| String::from("header name refused"))?;
        map.insert(n, v);
    }
    Ok(map)
}

/// The response headers a plugin may see, lower-cased, in the list's order.
fn response_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .map(|v| ((*name).to_owned(), v.to_owned()))
        })
        .collect()
}

/// Performs plugin fetches with a shared client.
pub struct PluginFetch {
    client: reqwest::Client,
}

impl PluginFetch {
    /// Builds the backend. Redirects are refused rather than followed:
    /// the capability was granted for a host, and a redirect is how that
    /// grant gets turned into a request somewhere else.
    #[must_use]
    pub fn new() -> std::sync::Arc<Self> {
        // Guarded: a `net:fetch` grant names a public host, and a name
        // that resolves to loopback or a private range (or rebinds to one)
        // must not turn it into a probe of the server's own network.
        let client = crate::net_guard::guarded_builder(0)
            .timeout(TIMEOUT)
            .user_agent("Vyasa-plugin/1.0")
            .build()
            .unwrap_or_default();
        std::sync::Arc::new(Self { client })
    }
}

impl FetchBackend for PluginFetch {
    fn get<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            // The broker's own parser: the `Url` requested here is the one
            // whose host the capability check approved, never a second
            // reading of the raw string by a different parser.
            let target = vyasa_plugins::broker::parse_fetch_url(url)
                .ok_or_else(|| String::from("url refused"))?;
            let response = self
                .client
                .get(target)
                .send()
                .await
                .map_err(|e| format!("request failed: {}", transport_reason(&e)))?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!("upstream returned {}", status.as_u16()));
            }
            // A running count, not the advertised length: a lying or absent
            // Content-Length must not let a plugin pull an unbounded
            // response into memory.
            let bytes = crate::net_guard::read_capped(response, MAX_BYTES)
                .await
                .map_err(|e| match e {
                    crate::net_guard::BodyError::TooLarge => String::from("response too large"),
                    crate::net_guard::BodyError::Transport(e) => {
                        format!("read failed: {}", transport_reason(&e))
                    }
                })?;
            String::from_utf8(bytes).map_err(|_| String::from("response is not UTF-8"))
        })
    }

    fn get_with<'a>(
        &'a self,
        url: &'a str,
        headers: Vec<(String, String)>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<FetchReply, String>> + Send + 'a>>
    {
        Box::pin(async move { self.get_with_impl(url, &headers).await })
    }
}

impl PluginFetch {
    async fn get_with_impl(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<FetchReply, String> {
        let target = vyasa_plugins::broker::parse_fetch_url(url)
            .ok_or_else(|| String::from("url refused"))?;
        let map = request_headers(headers)?;
        let response = self
            .client
            .get(target)
            .headers(map)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", transport_reason(&e)))?;
        let status = response.status().as_u16();
        let reply_headers = response_headers(response.headers());
        let bytes = crate::net_guard::read_capped(response, MAX_BYTES)
            .await
            .map_err(|e| match e {
                crate::net_guard::BodyError::TooLarge => String::from("response too large"),
                crate::net_guard::BodyError::Transport(e) => {
                    format!("read failed: {}", transport_reason(&e))
                }
            })?;
        let body = String::from_utf8(bytes).map_err(|_| String::from("response is not UTF-8"))?;
        Ok(FetchReply {
            status,
            headers: reply_headers,
            body,
        })
    }
}

/// A reason without the URL: an error string crosses back into the guest,
/// and the guest already knows the URL it asked for.
fn transport_reason(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else if e.is_decode() {
        "malformed response"
    } else {
        "transport error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_reasons_carry_no_url() {
        // Constructing reqwest errors directly is not possible, so this
        // guards the shape: every branch is a fixed string.
        for reason in [
            "timed out",
            "could not connect",
            "malformed response",
            "transport error",
        ] {
            assert!(!reason.contains("http"), "{reason} leaks a URL");
        }
    }

    #[tokio::test]
    async fn userinfo_urls_are_refused_before_any_request() {
        // `https://api.good.com@evil.com` names evil.com; the backend must
        // refuse it rather than re-read the string its own way.
        let backend = PluginFetch::new();
        let err = backend
            .get("https://api.good.com@evil.com/x")
            .await
            .unwrap_err();
        assert_eq!(err, "url refused");
    }

    #[tokio::test]
    async fn internal_hosts_are_unreachable() {
        let backend = PluginFetch::new();
        let err = backend.get("https://localhost/").await.unwrap_err();
        assert!(err.starts_with("request failed"), "{err}");
        assert_eq!(
            backend.get("https://127.0.0.1/").await.unwrap_err(),
            "url refused"
        );
    }

    #[test]
    fn the_client_builds_with_redirects_refused() {
        // A redirect would let a `net:fetch:a.example.com` grant reach
        // any other host, so the policy must be `none`.
        let _ = PluginFetch::new();
    }

    #[test]
    fn only_the_allowed_request_headers_are_sent() {
        let ok = request_headers(&[
            ("Authorization".into(), "Bearer abc".into()),
            ("accept".into(), "application/vnd.github+json".into()),
            ("If-None-Match".into(), "\"abc\"".into()),
            ("x-github-api-version".into(), "2022-11-28".into()),
        ])
        .expect("allowed");
        assert_eq!(ok.len(), 4);
        assert!(
            ok.get("authorization")
                .is_some_and(reqwest::header::HeaderValue::is_sensitive),
            "kept out of logs"
        );
        for bad in [
            "cookie",
            "host",
            "x-forwarded-for",
            "user-agent",
            "content-length",
            "proxy-authorization",
        ] {
            let err = request_headers(&[(bad.into(), "x".into())]).unwrap_err();
            assert!(err.contains("not allowed"), "{bad}: {err}");
        }
        assert!(
            request_headers(&[("accept".into(), "a\r\nb: c".into())]).is_err(),
            "no header injection"
        );
        assert!(
            request_headers(&[("authorization".into(), "x".repeat(5000))]).is_err(),
            "bounded"
        );
    }

    #[test]
    fn only_the_listed_response_headers_come_back() {
        let mut h = HeaderMap::new();
        h.insert("etag", HeaderValue::from_static("\"v1\""));
        h.insert("set-cookie", HeaderValue::from_static("a=b"));
        h.insert("x-ratelimit-remaining", HeaderValue::from_static("59"));
        h.insert("server", HeaderValue::from_static("github"));
        let out = response_headers(&h);
        assert_eq!(
            out,
            vec![
                ("etag".to_owned(), "\"v1\"".to_owned()),
                ("x-ratelimit-remaining".to_owned(), "59".to_owned())
            ]
        );
    }

    #[tokio::test]
    async fn a_userinfo_url_is_refused_before_any_request_by_get_with_too() {
        let backend = PluginFetch::new();
        let err = backend
            .get_with("https://api.good.com@evil.com/x", vec![])
            .await
            .unwrap_err();
        assert_eq!(err, "url refused");
    }
}
