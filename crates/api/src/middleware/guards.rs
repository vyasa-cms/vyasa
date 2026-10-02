//! Request guards: body size limits and timeouts.
//!
//! Both wrap the whole `/api` surface; limits are generous enough for
//! media metadata but small enough to shed abuse.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Default request body cap: 2 MiB.
pub const DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;

/// The one route that legitimately carries more than that.
///
/// The media limit is the file cap plus room for the multipart envelope
/// and the alt and caption fields that ride along. It must be applied in
/// two places: here, or a declared `Content-Length` of 3 MB is refused
/// before the router ever sees it, and on the route itself, or axum's
/// multipart extractor stops reading at its own 2 MiB default. For as long
/// as only one of the two was set, every ordinary phone photo failed to
/// upload and the documented 413 never fired.
pub const MEDIA_UPLOAD_LIMIT: usize = vyasa_core::media::MAX_BYTES + 64 * 1024;

/// The body cap for this request.
fn limit_for(req: &Request<Body>) -> usize {
    if req.method() == axum::http::Method::POST && req.uri().path() == "/api/v1/media" {
        MEDIA_UPLOAD_LIMIT
    } else {
        DEFAULT_BODY_LIMIT
    }
}

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Enforces a request body size limit.
///
/// # Errors
///
/// Returns 413 when the declared/observed body exceeds the limit.
pub async fn body_limit(req: Request<Body>, next: Next) -> Response {
    if let Some(length) = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
    {
        if length > limit_for(&req) {
            return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
        }
    }
    next.run(req).await
}

/// Bounds total request handling time.
///
/// Never fails; a timeout produces 504 directly.
pub async fn timeout(req: Request<Body>, next: Next) -> Response {
    match tokio::time::timeout(DEFAULT_TIMEOUT, next.run(req)).await {
        Ok(response) => response,
        Err(_elapsed) => (StatusCode::GATEWAY_TIMEOUT, "request timed out").into_response(),
    }
}
