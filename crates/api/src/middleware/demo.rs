//! Demo mode at the edge: refuse the routes a public sandbox must not run
//! (before their bodies are read), keep search engines away, and mark
//! public pages as a demo. What is refused is decided in `policy`.

use axum::body::Body;
use axum::extract::{MatchedPath, State};
use axum::http::{header, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::state::AppState;

/// The banner put at the top of every public page of a demo.
fn banner(email: &str, password: &str) -> String {
    format!(
        "<div data-vyasa-demo-banner style=\"position:sticky;top:0;z-index:2147483647;\
         background:#1f2937;color:#f9fafb;font:14px/1.4 system-ui,sans-serif;\
         padding:8px 12px;text-align:center\">\
         This is the Vyasa demo. It resets every hour. \
         <a href=\"/admin\" style=\"color:#93c5fd\">Sign in</a> as \
         <strong>{}</strong> / <strong>{}</strong>.</div>",
        html_escape(email),
        html_escape(password)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Whether `path` is the public site: everything outside the `/admin` and
/// `/api` segments.
fn is_public_page(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    first != "admin" && first != "api"
}

/// `html` with `banner` right after its `<body …>` tag. Comments and
/// scripts are skipped, so a `<body>` written inside one is not taken for
/// the real tag; a page without a body tag is returned unchanged.
fn inject_banner(html: &str, banner: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &lower[i..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => i += end + 3,
                None => break,
            }
        } else if rest.starts_with("<script") {
            match rest.find("</script") {
                Some(end) => i += end + 8,
                None => break,
            }
        } else if rest.starts_with("<body")
            && rest[5..]
                .chars()
                .next()
                .is_some_and(|c| c == '>' || c.is_ascii_whitespace())
        {
            return match rest.find('>') {
                Some(end) => {
                    let at = i + end + 1;
                    format!("{}{banner}{}", &html[..at], &html[at..])
                }
                None => html.to_owned(),
            };
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    html.to_owned()
}

/// Largest page the banner is added to; anything bigger passes untouched.
const MAX_BANNER_PAGE: usize = 8 * 1024 * 1024;

/// The demo layer. A no-op unless demo mode is on.
pub async fn demo(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    if !state.config.demo.enabled {
        return next.run(req).await;
    }
    if let Some(path) = req.extensions().get::<MatchedPath>() {
        if let Err(e) = crate::policy::demo_route(&state, req.method().as_str(), path.as_str()) {
            return ApiError(e).into_response();
        }
        let declared = req
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if let Err(e) =
            crate::policy::demo_upload(&state, req.method().as_str(), path.as_str(), declared)
        {
            return ApiError(e).into_response();
        }
    }
    let public_page = is_public_page(req.uri().path());
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(
        header::HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex, nofollow"),
    );
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let declared = resp
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if !(public_page && is_html)
        || resp.headers().contains_key(header::CONTENT_ENCODING)
        || declared.is_some_and(|n| n > MAX_BANNER_PAGE)
    {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BANNER_PAGE).await else {
        // A streamed page that turned out too big: say so rather than
        // sending a truncated document under its old length.
        return ApiError(vyasa_common::AppError::internal_msg(
            "page too large for the demo banner",
        ))
        .into_response();
    };
    // Only UTF-8 pages are rewritten; anything else passes as it came.
    let Ok(html) = std::str::from_utf8(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let out = inject_banner(
        html,
        &banner(&state.config.demo.email, &state.config.demo.password),
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    // The validator described the page without the banner.
    parts.headers.remove(header::ETAG);
    Response::from_parts(parts, Body::from(out))
}

#[cfg(test)]
mod tests {
    use super::{inject_banner, is_public_page};

    #[test]
    fn the_banner_goes_right_after_the_body_tag_whatever_its_case() {
        assert_eq!(
            inject_banner("<html><BODY class=x><p>hi", "[B]"),
            "<html><BODY class=x>[B]<p>hi"
        );
        assert_eq!(inject_banner("<body><p>hi", "[B]"), "<body>[B]<p>hi");
    }

    #[test]
    fn a_body_tag_inside_a_comment_or_script_is_not_the_body() {
        let html = "<head><!-- <body> --><script>var s='<body>'</script></head><body>x";
        assert_eq!(
            inject_banner(html, "[B]"),
            "<head><!-- <body> --><script>var s='<body>'</script></head><body>[B]x"
        );
    }

    #[test]
    fn a_page_without_a_body_tag_is_left_alone() {
        assert_eq!(inject_banner("<p>fragment</p>", "[B]"), "<p>fragment</p>");
    }

    #[test]
    fn only_the_admin_and_api_segments_count_as_not_public() {
        assert!(!is_public_page("/admin"));
        assert!(!is_public_page("/admin/posts"));
        assert!(!is_public_page("/api/v1/posts"));
        assert!(is_public_page("/administrator-tips"));
        assert!(is_public_page("/apiary"));
        assert!(is_public_page("/"));
    }
}
