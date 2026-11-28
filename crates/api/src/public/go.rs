//! `/go/<name>`: the outbound-click redirect table.
//!
//! A link on the site (GitHub, PyPI, the install command's docs page) points at `/go/<name>`; the server counts the
//! click, then redirects. Counting happens here, on the server, so no script has to run in the visitor's browser.
//! A link marked `conversion` also counts as a conversion. Requests that opted out (`DNT`, `Sec-GPC`, the internal
//! header, a signed-in session) and crawlers are redirected and not counted.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Redirect, Response};
use vyasa_db::repo::audience::dim;

use crate::state::AppState;

/// True for a name a link can have: 1 to 64 of `a-z`, `0-9`, `-`, `_`.
#[must_use]
pub fn valid_link_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// True for a redirect target the table may hold: an absolute `http` or `https` URL with a host.
#[must_use]
pub fn safe_target(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// `GET /go/{name}` — count the click and redirect; an unknown name is the site's 404.
pub async fn go(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let link = match state.audience.link(&name).await {
        Ok(Some(link)) if safe_target(&link.url) => link,
        _ => return crate::public::errors::not_found_themed(&state).await,
    };
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    if !crate::analytics::opted_out(&headers) && !crate::analytics::is_bot(ua) {
        let audience = state.audience.clone();
        let (name, conversion) = (link.name.clone(), link.conversion);
        tokio::spawn(async move {
            let counted = audience.count(dim::OUTBOUND, &name, "", 1).await;
            let converted = if conversion {
                audience.count(dim::CONVERSION, &name, "", 1).await
            } else {
                Ok(())
            };
            if let Err(err) = counted.and(converted) {
                tracing::debug!("click not recorded: {err}");
            }
        });
    }
    let mut response = Redirect::temporary(&link.url).into_response();
    response.headers_mut().insert(
        "x-robots-tag",
        axum::http::HeaderValue::from_static("noindex"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::{safe_target, valid_link_name};

    #[test]
    fn names_are_short_and_plain() {
        assert!(valid_link_name("github"));
        assert!(valid_link_name("install-docs_2"));
        assert!(!valid_link_name(""));
        assert!(!valid_link_name("Has Space"));
        assert!(!valid_link_name("a/b"));
        assert!(!valid_link_name(&"a".repeat(65)));
    }

    #[test]
    fn only_web_urls_are_targets() {
        assert!(safe_target("https://github.com/vyasa-cms/vyasa"));
        assert!(safe_target("http://example.com/x?y=1"));
        assert!(!safe_target("javascript:alert(1)"));
        assert!(!safe_target("/relative"));
        assert!(!safe_target("ftp://example.com"));
        assert!(!safe_target("https://"));
    }
}
