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
    }
    let public_page =
        !req.uri().path().starts_with("/admin") && !req.uri().path().starts_with("/api");
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
    if !(public_page && is_html) || resp.headers().contains_key(header::CONTENT_ENCODING) {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BANNER_PAGE).await else {
        return Response::from_parts(parts, Body::empty());
    };
    let html = String::from_utf8_lossy(&bytes);
    let banner = banner(&state.config.demo.email, &state.config.demo.password);
    let out = match html.find("<body") {
        Some(start) => match html[start..].find('>') {
            Some(end) => {
                let at = start + end + 1;
                format!("{}{banner}{}", &html[..at], &html[at..])
            }
            None => html.into_owned(),
        },
        None => format!("{banner}{html}"),
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(out))
}
