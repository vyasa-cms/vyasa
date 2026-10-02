//! Serving generated documents that something polls on a timer.
//!
//! Feeds, the sitemap and robots.txt are not read by people at their own
//! pace; they are fetched on a schedule by software that never stops. A
//! reader polls a feed every few minutes for as long as anyone is
//! subscribed, and a crawler re-fetches a sitemap for as long as the site
//! exists.
//!
//! All three were served with no `Cache-Control` and no validator, so every
//! one of those polls transferred the whole document, no shared cache would
//! hold a copy, and a reader had no way to ask "has this changed?" — the
//! question the whole protocol exists to answer cheaply.

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use vyasa_themes::cache::{evaluate_if_none_match, ConditionalGet};

/// Responds with `body` plus an ETag and a lifetime, or `304` when the
/// caller already holds this exact document.
///
/// The ETag is over the rendered bytes, so it changes when and only when
/// the document does — including when a change nothing else notices, like
/// a plugin rewriting a feed item, alters the output.
pub fn cacheable(
    headers: &HeaderMap,
    content_type: &'static str,
    max_age_secs: u32,
    body: String,
) -> Response {
    // One definition of what an ETag looks like across the whole server;
    // media raw bytes are served with the same shape.
    let etag = vyasa_core::media::MediaService::etag_for(body.as_bytes());
    let cache_control = format!("public, max-age={max_age_secs}");
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());

    if matches!(
        evaluate_if_none_match(if_none_match, &etag),
        ConditionalGet::NotModified
    ) {
        // A 304 carries no body, but must repeat the validator and the
        // lifetime or the next request has nothing to revalidate against.
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, etag), (header::CACHE_CONTROL, cache_control)],
        )
            .into_response();
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, cache_control),
        ],
        body,
    )
        .into_response()
}

/// How long a feed may be served without asking again.
///
/// Readers poll far more often than sites publish. Five minutes is short
/// enough that nobody notices a delay in a new post and long enough that a
/// reader checking every minute stops costing a render each time.
pub const FEED_MAX_AGE: u32 = 300;

/// How long a sitemap or robots.txt may be held.
///
/// Crawlers already decide their own re-crawl pace; this only stops a
/// burst of requests from re-rendering the same document.
pub const CRAWLER_MAX_AGE: u32 = 3600;
