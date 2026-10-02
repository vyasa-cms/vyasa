//! Authentication middleware: session cookie → current user extractor.

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

use crate::state::AppState;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "vy_session";

/// The authenticated user attached to a request.
#[derive(Clone, Debug)]
pub struct CurrentUser {
    /// Session token (needed for logout).
    pub token: String,
    /// Authenticated user row.
    pub user: vyasa_db::models::UserRow,
}

impl CurrentUser {
    /// Extracts the session token from the request cookies.
    #[must_use]
    pub fn token_from_headers(parts: &Parts) -> Option<String> {
        let cookies = parts.headers.get(axum::http::header::COOKIE)?;
        let cookies = cookies.to_str().ok()?;
        for pair in cookies.split(';') {
            let pair = pair.trim();
            if let Some(value) = pair.strip_prefix(SESSION_COOKIE) {
                let value = value.strip_prefix('=')?;
                if value.is_empty() {
                    return None;
                }
                return Some(value.to_string());
            }
        }
        None
    }
}

/// Extractor requiring a valid session; rejects with 401 otherwise.
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Already resolved by the route guard (`crate::authz`): the handler
        // acts as the principal the guard checked, never as a second one.
        // A session-only endpoint does not accept an API key, so a key the
        // guard let through is rejected here rather than falling back to
        // whatever session cookie the request also carries.
        match parts.extensions.get::<crate::middleware::Principal>() {
            Some(crate::middleware::Principal::Session(current)) => return Ok(current.clone()),
            Some(crate::middleware::Principal::ApiKey(_)) => {
                return Err(vyasa_common::AppError::auth("authentication required").into());
            }
            None => {}
        }
        let token = CurrentUser::token_from_headers(parts)
            .ok_or_else(|| vyasa_common::AppError::auth("authentication required"))?;
        #[cfg(test)]
        lookups::record(&token);
        let session = state
            .auth
            .resolve(&token)
            .await
            .map_err(|_| vyasa_common::AppError::auth("authentication required"))?;
        Ok(CurrentUser {
            token: session.token,
            user: session.user,
        })
    }
}

/// Middleware that attaches a `x-request-id` header and logs the request.
///
/// # Errors
///
/// Never fails; exists to satisfy axum's middleware signature.
pub async fn request_context(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // Reuse the caller's id when there is one, so a trace started at the
    // proxy or in a client stays a single thread through our logs; generate
    // one otherwise. Correlating a user's report with a log line is
    // impossible without this.
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        // Bound it: an unbounded client-supplied value ends up in every log
        // line for that request.
        .filter(|v| !v.is_empty() && v.len() <= 128 && v.is_ascii())
        .map_or_else(
            || format!("{:016x}", vyasa_common::next_id_i64()),
            str::to_owned,
        );

    let span = tracing::info_span!("request", request_id = %request_id, %method, %path);
    // Scoped rather than held across the await: entering a span guard over an
    // await point attaches it to whatever else the runtime schedules on this
    // thread.
    {
        let _enter = span.enter();
        tracing::debug!("request");
    }

    let mut response = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    if let Ok(id) = HeaderValue::from_str(&format!("{method} {path}")) {
        response.headers_mut().insert("x-vyasa-route", id);
    }
    response
}

/// Counts session lookups per token, so a test can pin "one lookup per
/// request" without being disturbed by tests running beside it.
#[cfg(test)]
pub(crate) mod lookups {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, PoisonError};

    static COUNTS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

    pub(super) fn record(token: &str) {
        let mut counts = COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
        *counts.entry(token.to_owned()).or_default() += 1;
    }

    /// How many times the extractor has looked `token`'s session up.
    pub(crate) fn count(token: &str) -> usize {
        let counts = COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
        counts.get(token).copied().unwrap_or(0)
    }
}
