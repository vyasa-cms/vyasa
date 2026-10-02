//! `POST /form` — the no-JavaScript handler behind `signup-form`
//! sections.
//!
//! Same discipline as the comment form: a plain urlencoded body (the
//! site runs `script-src 'self'`), a size cap, a honeypot that answers
//! bots with the same page a person gets, and a themed-enough
//! confirmation page instead of a redirect into nothing.
//!
//! `mode=newsletter` starts double opt-in: the address is stored
//! pending and a confirm link goes out; nothing is "subscribed" until
//! the click. `mode=contact` (anything else) lands in the submissions
//! inbox.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use crate::state::AppState;

const MAX_BODY_BYTES: usize = 16 * 1024;
/// Longest accepted message; enough for a real note, hostile for spam.
const MAX_MESSAGE: usize = 4000;

/// A plausible-enough email: something@something.something. Real
/// validation is the confirm click.
fn email_ok(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && email.len() <= 254
        && !email.contains(char::is_whitespace)
}

/// The tiny confirmation page. Standalone on purpose: a themed render
/// would need the whole pipeline for one sentence.
fn thanks_page(site: &str, message: &str) -> Response {
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\">\
         <title>{}</title>\
         <style>body{{font-family:system-ui,sans-serif;display:grid;place-items:center;\
         min-height:100vh;margin:0;background:#fafaf9;color:#1c1917}}\
         main{{max-width:26rem;padding:2rem;text-align:center}}\
         a{{color:inherit}}</style></head><body><main>\
         <h1 style=\"font-size:1.2rem\">{}</h1>\
         <p><a href=\"/\">Back to {}</a></p>\
         </main></body></html>",
        esc(site),
        esc(message),
        esc(site)
    ))
    .into_response()
}

/// Handles one submission.
pub async fn submit(State(state): State<AppState>, body: String) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "form too large").into_response();
    }
    let fields = super::comment_form::parse_form(&body);
    let get = |name: &str| {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.trim().to_owned())
            .unwrap_or_default()
    };
    let site = state
        .options_service
        .site_identity()
        .await
        .map(|i| i.title)
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "this site".to_owned());

    // Honeypot: a filled `website` gets the success page and no row —
    // a bot that cannot tell it failed does not adapt.
    if !get("website").is_empty() {
        return thanks_page(&site, "Thanks!");
    }
    // A defined form: its own fields, its own message. Checked before the
    // email test, since a defined form may not have an email field at all.
    let form_slug = get("form");
    if !form_slug.is_empty() {
        if let Some(def) = crate::forms::by_slug(&state, &form_slug).await {
            let path: String = get("path").chars().take(200).collect();
            return match crate::forms::accept(&state, &def, &fields, &path).await {
                Ok(msg) => thanks_page(&site, &msg),
                Err(e) => thanks_page(&site, &format!("{e} — go back and try again.")),
            };
        }
    }
    let email = get("email");
    if !email_ok(&email) {
        return thanks_page(
            &site,
            "That email address doesn't look right — go back and try again.",
        );
    }
    let mode = get("mode");
    let name: String = get("name").chars().take(120).collect();

    if mode == "newsletter" {
        return subscribe(&state, &site, &email).await;
    }
    let form: String = {
        let f = get("form");
        if f.is_empty() {
            "contact".to_owned()
        } else {
            f.chars().take(60).collect()
        }
    };
    let message: String = get("message").chars().take(MAX_MESSAGE).collect();
    match state
        .audience
        .add_submission(&form, &name, &email, &message, "")
        .await
    {
        Ok(_) => thanks_page(&site, "Thanks — your message has been received."),
        Err(err) => {
            tracing::warn!("form submission failed: {err}");
            thanks_page(
                &site,
                "Something went wrong on our side — please try again.",
            )
        }
    }
}

/// Stores a pending subscriber and sends the confirm link. Without a
/// configured site URL there is no clickable link to send, so the form
/// says so instead of pretending.
async fn subscribe(state: &AppState, site: &str, email: &str) -> Response {
    use rand::Rng as _;
    let Ok(base) = state.options_service.site_url().await else {
        return thanks_page(site, "Subscriptions aren't set up yet.");
    };
    if base.is_empty() {
        return thanks_page(site, "Subscriptions aren't set up yet.");
    }
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(40)
        .map(char::from)
        .collect();
    let row = match state.audience.upsert_pending(email, &token).await {
        Ok(row) => row,
        Err(err) => {
            tracing::warn!("subscriber upsert failed: {err}");
            return thanks_page(site, "Something went wrong on our side — please try again.");
        }
    };
    if row.status == "confirmed" {
        return thanks_page(site, "You're already subscribed — nothing to do.");
    }
    let ctx = serde_json::json!({
        "site": site,
        "url": format!("{base}/newsletter/confirm?token={}", row.token),
    });
    let queued = vyasa_core::notify::EmailService::new(state.pool.clone())
        .queue(email, "newsletter_confirm", &ctx)
        .await;
    match queued {
        Ok(_) => {
            state.publisher_notify.notify_one();
            thanks_page(site, "Check your inbox to confirm your subscription.")
        }
        Err(err) => {
            tracing::warn!("confirm email failed: {err}");
            thanks_page(site, "Something went wrong on our side — please try again.")
        }
    }
}

/// `GET /newsletter/confirm?token=` — the double-opt-in click.
pub async fn confirm(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<TokenQuery>,
) -> Response {
    flip(&state, &q.token, "confirmed", "You're subscribed. Welcome!").await
}

/// `GET /newsletter/unsubscribe?token=` — the exit every email carries.
pub async fn unsubscribe(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<TokenQuery>,
) -> Response {
    flip(
        &state,
        &q.token,
        "unsubscribed",
        "You're unsubscribed — no more emails.",
    )
    .await
}

/// Token query for confirm/unsubscribe.
#[derive(serde::Deserialize)]
pub struct TokenQuery {
    /// The secret from the emailed link.
    #[serde(default)]
    pub token: String,
}

async fn flip(state: &AppState, token: &str, status: &str, done: &str) -> Response {
    let site = state
        .options_service
        .site_identity()
        .await
        .map(|i| i.title)
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "this site".to_owned());
    if token.is_empty() || token.len() > 100 {
        return thanks_page(&site, "That link is not valid.");
    }
    match state.audience.set_status_by_token(token, status).await {
        Ok(Some(_)) => thanks_page(&site, done),
        Ok(None) => thanks_page(&site, "That link is not valid or has expired."),
        Err(err) => {
            tracing::warn!("subscriber flip failed: {err}");
            thanks_page(
                &site,
                "Something went wrong on our side — please try again.",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::email_ok;

    #[test]
    fn email_shapes() {
        assert!(email_ok("a@b.co"));
        assert!(email_ok("first.last+tag@sub.domain.example"));
        assert!(!email_ok("nope"));
        assert!(!email_ok("a@nodot"));
        assert!(!email_ok("a b@x.co"));
        assert!(!email_ok("@x.co"));
    }
}
