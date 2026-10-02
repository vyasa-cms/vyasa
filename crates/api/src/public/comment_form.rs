//! The visitor-facing comment endpoint.
//!
//! The rendered page carries a plain HTML form (no JavaScript, so it works
//! under the site's `script-src 'self'` policy and with scripting off).
//! That form cannot send JSON, so it posts urlencoded here; this module
//! decodes it, runs the same [`CommentService::submit`] the REST API uses —
//! same validation, same moderation, same AI screening — and then
//! redirects back to the post so a refresh does not resubmit.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use vyasa_db::content_models::CommentStatus;

use crate::middleware::MaybePrincipal;
use crate::state::AppState;

/// Longest accepted form body. Comments are short; the field caps in the
/// form are smaller still.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// What the visitor is told after posting, carried in `?comment=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Published straight away.
    Published,
    /// Held for moderation.
    Pending,
    /// Rejected (validation, or the honeypot caught a bot).
    Rejected,
}

impl Outcome {
    /// The value used in the redirect query.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Pending => "pending",
            Self::Rejected => "rejected",
        }
    }

    /// Parses the value back from a query string.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "published" => Some(Self::Published),
            "pending" => Some(Self::Pending),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    /// The message shown above the post.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Published => "Thanks — your comment is published.",
            Self::Pending => "Thanks — your comment is awaiting moderation.",
            Self::Rejected => {
                "Your comment could not be posted. Please check the fields and try again."
            }
        }
    }
}

/// `POST /comment` — accepts the rendered form.
///
/// Always redirects (303) back to the post, so the browser's back button
/// and a refresh behave. The outcome rides in `?comment=`.
pub async fn submit(
    State(state): State<AppState>,
    maybe: MaybePrincipal,
    headers: axum::http::HeaderMap,
    body: String,
) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return redirect("/", Outcome::Rejected, None);
    }
    let form = parse_form(&body);
    let field = |key: &str| form.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());

    let Some(post_id) = field("post_id").and_then(|v| v.parse::<i64>().ok()) else {
        return redirect("/", Outcome::Rejected, None);
    };
    // The rule REST and GraphQL follow: missing, not visible to the
    // visitor (a protected entry without its unlock cookie included) or
    // of a type they may not read — all the not-found page.
    let unlock = crate::public::routes::unlock_cookie(&headers, post_id);
    let Ok(post) = crate::policy::comment_target(&state, maybe.0.as_ref(), post_id, unlock).await
    else {
        return crate::public::errors::not_found();
    };
    let back = crate::permalinks::path(&state, &post).await;
    // The honeypot: a field hidden from people, irresistible to bots.
    // Silently accepted so a bot cannot tell it failed.
    if field("website").is_some_and(|v| !v.trim().is_empty()) {
        return redirect(&back, Outcome::Pending, None);
    }

    // `comment-body` runs here, where the WIT says it does: after the
    // visitor's text arrives, before it is stored. It had no call site.
    let body_text = crate::public::routes::filter_through_plugins(
        &state,
        vyasa_plugins::hooks::HookPoint::CommentBody,
        field("content").unwrap_or_default().to_owned(),
    )
    .await;
    let parent_id = field("parent_id").and_then(|v| v.parse::<i64>().ok());
    let author_user_id = maybe.0.as_ref().map(|p| p.user().id);
    let submitted = state
        .comments
        .submit(vyasa_core::comment::service::SubmitComment {
            post_id,
            author_user_id,
            author_name: field("author_name").unwrap_or_default().to_owned(),
            author_email: field("author_email").unwrap_or_default().to_owned(),
            content: body_text,
            parent_id,
        })
        .await;

    match submitted {
        Ok(row) => {
            let outcome = if row.status == CommentStatus::Approved {
                Outcome::Published
            } else {
                Outcome::Pending
            };
            // `submit` emits CommentAdded, which already invalidates the
            // cached page and queues AI screening; nothing to do here.
            let anchor = (outcome == Outcome::Published).then_some(row.id);
            redirect(&back, outcome, anchor)
        }
        Err(err) => {
            tracing::debug!("comment rejected: {err}");
            redirect(&back, Outcome::Rejected, None)
        }
    }
}

/// 303 back to `path`, carrying the outcome and an anchor.
fn redirect(path: &str, outcome: Outcome, comment_id: Option<i64>) -> Response {
    let anchor =
        comment_id.map_or_else(|| String::from("#comments"), |id| format!("#comment-{id}"));
    let location = format!("{path}?comment={}{anchor}", outcome.as_str());
    match header::HeaderValue::from_str(&location) {
        Ok(value) => (StatusCode::SEE_OTHER, [(header::LOCATION, value)]).into_response(),
        Err(_) => crate::public::errors::internal_error(),
    }
}

/// Decodes an `application/x-www-form-urlencoded` body.
pub(crate) fn parse_form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((urldecode(key), urldecode(value)))
        })
        .collect()
}

/// Percent-decoding with `+` for space, as browsers encode form bodies.
fn urldecode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_browser_form_body() {
        let form = parse_form(
            "post_id=7&author_name=Ada+Lovelace&author_email=ada%40example.com\
             &content=First%21+%26+best&website=",
        );
        let get = |k: &str| {
            form.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("post_id"), Some("7"));
        assert_eq!(get("author_name"), Some("Ada Lovelace"));
        assert_eq!(get("author_email"), Some("ada@example.com"));
        assert_eq!(get("content"), Some("First! & best"));
        assert_eq!(get("website"), Some(""));
    }

    #[test]
    fn survives_malformed_input() {
        assert!(parse_form("").is_empty());
        assert!(parse_form("novalue").is_empty());
        // A stray percent is kept rather than panicking.
        assert_eq!(parse_form("a=100%").first().unwrap().1, "100%");
        // Multi-byte characters survive the round trip.
        assert_eq!(parse_form("a=caf%C3%A9").first().unwrap().1, "café");
    }

    #[test]
    fn outcomes_round_trip_and_read_sensibly() {
        for outcome in [Outcome::Published, Outcome::Pending, Outcome::Rejected] {
            assert_eq!(Outcome::parse(outcome.as_str()), Some(outcome));
            assert!(!outcome.message().is_empty());
        }
        assert_eq!(Outcome::parse("nonsense"), None);
    }
}
