//! Requests served by plugin code.
//!
//! A plugin declares `[{ method, path }]` from `register-routes`; this is
//! what makes those declarations do something. Before, a declared route
//! answered with a JSON echo of its own name — the endpoint existed, the
//! plugin was never asked.
//!
//! Everything crossing the boundary is narrowed in both directions: the
//! guest never sees a cookie or an `Authorization` header, and its reply
//! can only set headers from a fixed list with a content type from a fixed
//! list. A plugin serves a response; it does not get to steer the browser.

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use vyasa_plugins::host::vyasa::plugin::host;
use vyasa_plugins::host::{HostEnv, V2Outcome};

use crate::middleware::MaybePrincipal;
use crate::plugin_surface::RouteDecl;
use crate::state::AppState;

/// Largest request body handed to a plugin.
const MAX_REQUEST_BODY: usize = 256 * 1024;
/// Largest response body accepted back.
const MAX_RESPONSE_BODY: usize = 1024 * 1024;

/// Request headers a plugin may see.
///
/// An allowlist, not a denylist: the question is what a plugin needs, and
/// the answer is content negotiation and nothing else. `cookie` and
/// `authorization` carry the visitor's session, which is precisely what a
/// sandbox exists to keep away from third-party code.
const FORWARDED_HEADERS: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "user-agent",
    "x-requested-with",
];

/// Content types a plugin route may answer with.
const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "text/plain",
    "text/html",
    "text/css",
    "application/json",
    "application/xml",
    "text/xml",
    "image/svg+xml",
];

/// Response headers a plugin may set.
const ALLOWED_RESPONSE_HEADERS: &[&str] = &["content-type", "cache-control", "location"];

/// `ANY /api/v1/plugin/{name}/{*rest}` — hand the request to the plugin
/// that declared the route.
pub async fn plugin_route(
    State(state): State<AppState>,
    Path((name, rest)): Path<(String, String)>,
    method: Method,
    RawQuery(query): RawQuery,
    MaybePrincipal(principal): MaybePrincipal,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(owner) = match_route(&state, &name, &method, &rest).await else {
        return super::routes::missing(&state).await;
    };
    if body.len() > MAX_REQUEST_BODY {
        return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
    }
    let Ok(body) = String::from_utf8(body.to_vec()) else {
        return (StatusCode::BAD_REQUEST, "request body must be UTF-8").into_response();
    };
    let request = host::HttpRequest {
        method: method.as_str().to_owned(),
        path: rest,
        // The request's own query string, not a header a caller could set.
        query: query.unwrap_or_default(),
        headers: forwarded(&headers),
        body,
    };
    // The session cookie never crosses into the guest, but who it belongs
    // to does — through `current-viewer`, and only for a plugin holding
    // `viewer:read`. That is what lets a plugin serve a members' area
    // without ever being able to impersonate the member.
    let env = HostEnv::background(std::sync::Arc::clone(&state.broker), state.pool.clone())
        .for_viewer(principal.as_ref().map(crate::middleware::Principal::viewer));
    match state
        .plugin_host
        .call_handle_request(owner, &request, &env)
        .await
    {
        V2Outcome::Ok(response) => build_response(&response),
        // The route was declared by a plugin that implements the base
        // world only: the declaration is real, the handler is not.
        V2Outcome::Unsupported => (
            StatusCode::NOT_IMPLEMENTED,
            "this plugin declares the route but does not implement handle-request",
        )
            .into_response(),
        V2Outcome::Failed(reason) => {
            tracing::warn!(plugin_id = owner, "plugin route failed: {reason}");
            state.metrics.inc_plugin_failure();
            (StatusCode::BAD_GATEWAY, "the plugin could not serve this").into_response()
        }
    }
}

/// The plugin that declared `method name/rest`, if any.
async fn match_route(state: &AppState, name: &str, method: &Method, rest: &str) -> Option<i64> {
    let routes = state.plugin_surface.routes().await;
    owner_of(&routes, name, method.as_str(), rest)
}

/// Pure matcher over declared routes.
///
/// Split out from the handler so the rules — exact paths, `*` prefixes,
/// method equality, mount-point isolation — can be tested without a
/// database and a running server behind them.
///
/// A declared path may end in `*`, which matches the rest of the path;
/// that is how a plugin serves a tree rather than one endpoint. Matching
/// is first-declared-wins, in registration order.
fn owner_of(routes: &[RouteDecl], name: &str, method: &str, rest: &str) -> Option<i64> {
    routes
        .iter()
        .find(|r| {
            r.plugin_name == name
                && r.method == method
                && match r.path.strip_suffix('*') {
                    // `a/*` matches `a/b` and `a`; a bare `*` matches the
                    // plugin's whole mount point.
                    Some(prefix) => rest.starts_with(prefix.trim_end_matches('/')),
                    None => r.path == rest,
                }
        })
        .map(|r| r.plugin_id)
}

/// The content hash in an asset file name, when the name is well formed.
///
/// Split out for the same reason: `..%2F..%2Fetc%2Fpasswd` reaching this
/// must produce `None`, and that is worth a test that does not need a
/// server.
fn asset_hash(file: &str) -> Option<&str> {
    let (hash, ext) = file.rsplit_once('.')?;
    let ok = matches!(ext, "css" | "js")
        && !hash.is_empty()
        && hash.chars().all(|c| c.is_ascii_hexdigit());
    ok.then_some(hash)
}

fn forwarded(headers: &HeaderMap) -> Vec<(String, String)> {
    FORWARDED_HEADERS
        .iter()
        .filter_map(|name| {
            let value = headers.get(*name)?.to_str().ok()?;
            Some(((*name).to_owned(), value.chars().take(1024).collect()))
        })
        .collect()
}

/// Turns a plugin's reply into a response, dropping anything it was not
/// entitled to set.
fn build_response(response: &host::HttpResponse) -> Response {
    if response.body.len() > MAX_RESPONSE_BODY {
        return (
            StatusCode::BAD_GATEWAY,
            "the plugin's response was too large",
        )
            .into_response();
    }
    // A nonsense status becomes 502 rather than 200: silently turning a
    // plugin's mistake into a success is how a broken endpoint gets cached
    // as if it worked.
    //
    // Bounded to the range HTTP actually defines, not to what the type
    // accepts — `StatusCode::from_u16` is happy with anything up to 999,
    // so checking only that would have let a plugin answer `999` and had
    // the response pass straight through to a browser.
    let status = match response.status {
        100..=599 => StatusCode::from_u16(response.status).unwrap_or(StatusCode::BAD_GATEWAY),
        _ => {
            return (
                StatusCode::BAD_GATEWAY,
                "the plugin returned an invalid status",
            )
                .into_response()
        }
    };
    let mut out = HeaderMap::new();
    for (raw_name, raw_value) in &response.headers {
        let lower = raw_name.to_ascii_lowercase();
        if !ALLOWED_RESPONSE_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        if lower == "content-type" && !content_type_allowed(raw_value) {
            continue;
        }
        // A plugin redirecting the visitor to another origin is a
        // credential-phishing primitive, so `location` stays same-site.
        if lower == "location" && !is_relative(raw_value) {
            continue;
        }
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(lower.as_bytes()),
            HeaderValue::from_str(raw_value),
        ) else {
            continue;
        };
        out.insert(name, value);
    }
    if !out.contains_key(header::CONTENT_TYPE) {
        out.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
    }
    // Plugin output is never a document the browser should sniff a type
    // for, and never anything to frame.
    out.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    (status, out, response.body.clone()).into_response()
}

fn content_type_allowed(value: &str) -> bool {
    let base = value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    ALLOWED_CONTENT_TYPES.contains(&base.as_str())
}

/// A same-site target: one leading slash, and not `//host` (which is
/// protocol-relative and therefore another origin).
fn is_relative(value: &str) -> bool {
    value.starts_with('/') && !value.starts_with("//")
}

/// `GET /plugin-assets/{file}` — a plugin's declared CSS or JS.
///
/// The file name carries a content hash, so the URL changes whenever the
/// bytes do and the response can be cached indefinitely.
pub async fn plugin_asset(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    let Some(hash) = asset_hash(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(asset) = state.plugin_surface.asset(hash).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, asset.content_type),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        asset.body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, headers: &[(&str, &str)], body: &str) -> host::HttpResponse {
        host::HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn a_plugin_cannot_set_arbitrary_headers() {
        let out = build_response(&response(
            200,
            &[
                ("Set-Cookie", "session=stolen"),
                ("Access-Control-Allow-Origin", "*"),
                ("Content-Type", "application/json"),
            ],
            "{}",
        ));
        assert!(out.headers().get("set-cookie").is_none());
        assert!(out.headers().get("access-control-allow-origin").is_none());
        assert_eq!(out.headers()["content-type"], "application/json");
        assert_eq!(out.headers()["x-content-type-options"], "nosniff");
    }

    #[test]
    fn dangerous_content_types_fall_back_to_text() {
        // A plugin serving text/html is fine; serving a script or a
        // downloadable with a chosen name is not on the menu.
        for bad in [
            "application/javascript",
            "text/vbscript",
            "application/x-msdownload",
        ] {
            let out = build_response(&response(200, &[("content-type", bad)], "x"));
            assert_eq!(
                out.headers()["content-type"],
                "text/plain; charset=utf-8",
                "{bad}"
            );
        }
        assert!(content_type_allowed("text/html; charset=utf-8"));
    }

    #[test]
    fn redirects_stay_on_this_site() {
        let out = build_response(&response(302, &[("location", "https://evil.example/")], ""));
        assert!(out.headers().get("location").is_none());
        let out = build_response(&response(302, &[("location", "//evil.example/")], ""));
        assert!(out.headers().get("location").is_none());
        let out = build_response(&response(302, &[("location", "/thanks")], ""));
        assert_eq!(out.headers()["location"], "/thanks");
    }

    fn route(plugin_id: i64, name: &str, method: &str, path: &str) -> RouteDecl {
        RouteDecl {
            plugin_id,
            plugin_name: name.to_owned(),
            method: method.to_owned(),
            path: path.to_owned(),
        }
    }

    #[test]
    fn routes_match_exactly_unless_they_end_in_a_star() {
        let routes = vec![
            route(1, "shop", "GET", "cart"),
            route(1, "shop", "POST", "cart"),
            route(2, "docs", "GET", "pages/*"),
            route(3, "any", "GET", "*"),
        ];
        let owner = |name, method, rest| owner_of(&routes, name, method, rest);

        assert_eq!(owner("shop", "GET", "cart"), Some(1));
        assert_eq!(owner("shop", "POST", "cart"), Some(1));
        // An exact path does not match a longer one.
        assert_eq!(owner("shop", "GET", "cart/items"), None);
        // A method nobody declared is not served.
        assert_eq!(owner("shop", "DELETE", "cart"), None);
        // A prefix route serves its tree, and its own root.
        assert_eq!(owner("docs", "GET", "pages/intro"), Some(2));
        assert_eq!(owner("docs", "GET", "pages"), Some(2));
        assert_eq!(owner("docs", "GET", "other"), None);
        // A bare star serves the whole mount point…
        assert_eq!(owner("any", "GET", "anything/at/all"), Some(3));
        // …but never another plugin's, which is the isolation that makes
        // the mount point mean anything.
        assert_eq!(owner("shop", "GET", "anything"), None);
        assert_eq!(owner("nobody", "GET", "cart"), None);
    }

    #[test]
    fn asset_names_are_a_hash_and_an_extension_or_nothing() {
        assert_eq!(asset_hash("deadbeef.css"), Some("deadbeef"));
        assert_eq!(asset_hash("0123456789abcdef.js"), Some("0123456789abcdef"));
        for bad in [
            "../../etc/passwd",
            "../secret.css",
            "nothex.css",
            "deadbeef.exe",
            "deadbeef",
            ".css",
            "dead/beef.css",
        ] {
            assert_eq!(asset_hash(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_nonsense_status_becomes_a_gateway_error_not_a_success() {
        // Silently turning 999 into 200 would let a broken endpoint be
        // cached as though it had worked.
        // `StatusCode::from_u16` accepts anything up to 999, so a type
        // check alone would have let these through to a browser.
        for bad in [0, 42, 99, 600, 999] {
            let out = build_response(&response(bad, &[], "x"));
            assert_eq!(out.status(), StatusCode::BAD_GATEWAY, "status {bad}");
        }
        let out = build_response(&response(204, &[], ""));
        assert_eq!(out.status(), StatusCode::NO_CONTENT);
    }

    #[test]
    fn only_negotiation_headers_reach_the_guest() {
        let mut headers = HeaderMap::new();
        headers.insert("cookie", HeaderValue::from_static("session=secret"));
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("accept", HeaderValue::from_static("application/json"));
        let seen = forwarded(&headers);
        assert_eq!(
            seen,
            vec![(String::from("accept"), String::from("application/json"))]
        );
    }
}
