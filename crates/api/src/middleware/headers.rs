//! Security headers appended to every response.

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// Appends standard security headers.
pub async fn security_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    insert(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    insert(
        h,
        header::REFERRER_POLICY,
        "strict-origin-when-cross-origin",
    );
    // SAMEORIGIN, not DENY: the admin's theme studio previews a draft theme by
    // framing this same origin. Third-party framing stays blocked, so the
    // clickjacking protection is unchanged.
    insert(h, header::X_FRAME_OPTIONS, "SAMEORIGIN");
    // HSTS only makes sense over HTTPS but harmless to include.
    insert(
        h,
        header::STRICT_TRANSPORT_SECURITY,
        "max-age=31536000; includeSubDomains",
    );
    // Minimal CSP: same-origin scripts and styles, no inline script or eval.
    //
    // `frame-ancestors 'self'` mirrors the X-Frame-Options relaxation above.
    // Inline scripts stay forbidden — the admin loads its pre-paint theme
    // setup from /assets/theme-init.js rather than an inline block, so no
    // 'unsafe-inline' or per-script hash is needed here.
    //
    // A handler that set a stricter policy of its own keeps it: a bundled
    // SVG is served under one that runs no script at all, and the site
    // default must not loosen it on the way out.
    if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        insert(
            h,
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data: https:; frame-ancestors 'self'",
        );
    }
    resp
}

fn insert(map: &mut axum::http::HeaderMap, name: header::HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        map.insert(name, v);
    }
}
