//! Security headers appended to every response.

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// The Content-Security-Policy value, built once at startup.
pub fn csp_value(cloudflare_web_analytics: bool) -> HeaderValue {
    let extra = if cloudflare_web_analytics {
        " https://static.cloudflareinsights.com"
    } else {
        ""
    };
    let policy = format!(
        "default-src 'self'; script-src 'self'{extra}; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: https:; frame-ancestors 'self'"
    );
    HeaderValue::from_str(&policy)
        .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'"))
}

/// Appends standard security headers.
pub async fn security_headers(
    State(csp): State<HeaderValue>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
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
        h.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    resp
}

fn insert(map: &mut axum::http::HeaderMap, name: header::HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        map.insert(name, v);
    }
}

#[cfg(test)]
mod tests {
    use super::{csp_value, security_headers};
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{header, HeaderValue};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    const SVG_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

    async fn csp_of(on: bool, path: &str) -> String {
        let app = Router::new()
            .route("/plain", get(|| async { "ok" }))
            .route(
                "/svg",
                get(|| async { ([(header::CONTENT_SECURITY_POLICY, SVG_CSP)], "svg") }),
            )
            .layer(axum::middleware::from_fn_with_state(
                csp_value(on),
                security_headers,
            ));
        let resp = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        resp.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn script_src(csp: &str) -> Vec<&str> {
        csp.split(';')
            .map(str::trim)
            .find(|d| d.starts_with("script-src "))
            .unwrap_or_default()
            .split_whitespace()
            .skip(1)
            .collect()
    }

    #[tokio::test]
    async fn setting_off_keeps_script_src_self() {
        let csp = csp_of(false, "/plain").await;
        assert_eq!(script_src(&csp), ["'self'"]);
        assert_eq!(
            csp,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data: https:; frame-ancestors 'self'"
        );
    }

    #[tokio::test]
    async fn setting_on_adds_only_the_beacon_origin() {
        let csp = csp_of(true, "/plain").await;
        assert_eq!(
            script_src(&csp),
            ["'self'", "https://static.cloudflareinsights.com"]
        );
        assert!(!csp.contains("unsafe-eval"));
        assert!(csp.contains("style-src 'self' 'unsafe-inline'"));
    }

    #[tokio::test]
    async fn a_handlers_own_policy_survives_in_both_modes() {
        for on in [false, true] {
            assert_eq!(csp_of(on, "/svg").await, SVG_CSP);
        }
    }

    #[test]
    fn the_value_is_a_valid_header() {
        let _: HeaderValue = csp_value(true);
    }
}
