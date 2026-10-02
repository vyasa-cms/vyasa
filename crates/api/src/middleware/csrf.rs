//! CSRF protection: SameSite=Strict cookies + Origin header check.
//!
//! Session cookies use SameSite=Lax (needed for normal navigation). For
//! unsafe methods (POST/PUT/DELETE/PATCH) on cookie-authed requests we
//! verify the Origin header matches the configured base URL. API-key
//! authenticated requests are exempt (they don't rely on ambient cookies).

use axum::http::{header, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Checks Origin header on unsafe methods when session cookie is present.
pub async fn csrf_check(req: Request<axum::body::Body>, next: Next) -> Response {
    let method = req.method().clone();
    let is_unsafe = matches!(
        method,
        axum::http::Method::POST
            | axum::http::Method::PUT
            | axum::http::Method::DELETE
            | axum::http::Method::PATCH
    );
    if !is_unsafe {
        return next.run(req).await;
    }
    // Only check requests that carry a session cookie (api_key callers are
    // exempt because they authenticate explicitly).
    let has_cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.contains(crate::middleware::SESSION_COOKIE));
    if !has_cookie {
        return next.run(req).await;
    }
    // Verify Origin matches Host (same-origin).
    let origin_ok = match (
        req.headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok()),
        req.headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok()),
    ) {
        (Some(origin), Some(host)) => same_origin(origin, host),
        _ => true, // non-browser clients without Origin are allowed
    };
    if !origin_ok {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "CSRF check failed: cross-origin request",
        )
            .into_response();
    }
    next.run(req).await
}

/// Whether `origin` names exactly the host this request arrived at.
///
/// `origin.contains(host)` was the check before. `https://evil-wp.example`
/// contains `wp.example`, and so does `https://wp.example.attacker.net`:
/// any page on a sibling or a look-alike domain could post to the admin
/// with the victim's cookie. An `Origin` is `scheme://authority`; the
/// authority must be the whole host, compared case-insensitively.
fn same_origin(origin: &str, host: &str) -> bool {
    let authority = origin
        .split_once("://")
        .map_or(origin, |(_, rest)| rest)
        .trim_end_matches('/');
    authority.eq_ignore_ascii_case(host)
}

#[cfg(test)]
mod origin_tests {
    use super::same_origin;

    #[test]
    fn only_the_exact_host_is_same_origin() {
        assert!(same_origin("https://wp.example.com", "wp.example.com"));
        assert!(same_origin("HTTPS://WP.Example.com", "wp.example.com"));
        assert!(same_origin(
            "https://wp.example.com:8443",
            "wp.example.com:8443"
        ));
        // The three shapes the substring check let through.
        assert!(!same_origin(
            "https://evil-wp.example.com",
            "wp.example.com"
        ));
        assert!(!same_origin(
            "https://wp.example.com.attacker.net",
            "wp.example.com"
        ));
        assert!(!same_origin(
            "https://attacker.net/?x=wp.example.com",
            "wp.example.com"
        ));
        // A port on one side only is a different origin.
        assert!(!same_origin(
            "https://wp.example.com:8443",
            "wp.example.com"
        ));
    }
}
