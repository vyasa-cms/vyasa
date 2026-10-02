//! The structural tests of `/api/v1` authorization: every route as every
//! role, the committed access table, and the rule that routes are only
//! registered through [`super::Guarded`].

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(missing_docs)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use tower::ServiceExt;
use vyasa_core::user::{role_caps, Capability};
use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, UsersRepo};
use vyasa_testkit::TestDb;

use super::tests::{session, test_state};
use super::{Access, RouteRule};
use crate::middleware::auth::SESSION_COOKIE;
use crate::rest;

/// One user per role. The ids are far from the `1` the matrix puts in
/// every `{id}` segment, so no request can delete, suspend or demote a
/// caller.
const CALLERS: [(i64, &str, Role); 5] = [
    (96_000_001, "subscriber", Role::Subscriber),
    (96_000_002, "contributor", Role::Contributor),
    (96_000_003, "author", Role::Author),
    (96_000_004, "editor", Role::Editor),
    (96_000_005, "admin", Role::Admin),
];

/// Routes that answer 403 to a caller who holds the declared access,
/// because the handler also decides on the request itself. Each entry
/// names the capability that lifts the refusal and says why. The matrix
/// asks for someone else's `{id}`, so a caller holding the declared
/// access must get exactly 403 without that capability and be admitted
/// with it; the 401 and missing-access 403 checks are unchanged.
const EXCEPTIONS: &[(&str, &str, Capability, &str)] = &[
    // (method, path, capability that lifts the refusal, reason)
    (
        "GET",
        "/users/{id}",
        Capability::ManageUsers,
        "users::get: reading your own account needs no capability; \
         `if user.user.id != id { ensure(ManageUsers) }` refuses someone else's",
    ),
    (
        "POST",
        "/users/{id}/sessions/revoke",
        Capability::ManageUsers,
        "users::revoke_sessions: ending your own sessions needs no capability; \
         `if id != user.user.id { ensure(ManageUsers) }` refuses someone else's",
    ),
];

/// How a `Public` route is gated inside its handler by something other
/// than a principal's capability.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// `rest::setup::SetupPrincipal`: the setup token before the first
    /// administrator exists, a `manage_options` principal after. The
    /// route cannot declare the capability, because the token holder has
    /// no principal at all.
    Setup,
    /// A signed token in the request; no session stands in for it.
    Token,
}

const SETUP: &str = "the handler takes `rest::setup::SetupPrincipal`";

/// `Public` routes whose handler answers 401 or 403 on its own gate.
///
/// These are checked, not excused. The matrix runs with an administrator
/// present and sends no token, so a `Setup` route must answer 401 without
/// a principal, 403 without `manage_options`, and neither to a caller who
/// holds it; a `Token` route must answer 401 to every caller.
const PUBLIC_GATES: &[(&str, &str, Gate, &str)] = &[
    // (method, path, gate, reason)
    ("GET", "/setup/checks", Gate::Setup, SETUP),
    ("POST", "/setup/account", Gate::Setup, SETUP),
    ("POST", "/setup/site", Gate::Setup, SETUP),
    ("POST", "/setup/content", Gate::Setup, SETUP),
    ("GET", "/setup/verify-url", Gate::Setup, SETUP),
    ("POST", "/setup/delivery", Gate::Setup, SETUP),
    ("POST", "/setup/mail", Gate::Setup, SETUP),
    ("POST", "/setup/mail/test", Gate::Setup, SETUP),
    ("POST", "/setup/assistants", Gate::Setup, SETUP),
    ("POST", "/setup/updates", Gate::Setup, SETUP),
    ("POST", "/setup/keypair", Gate::Setup, SETUP),
    ("POST", "/setup/finish", Gate::Setup, SETUP),
    (
        "GET",
        "/posts/{id}/preview",
        Gate::Token,
        "posts::preview: `q.token.ok_or_else(|| auth(\"preview token required\"))`, \
         then `verify_preview_token`",
    ),
];

/// Statuses that can be produced before a handler's own capability
/// check: a body, query or path extractor or validation (400, 415, 422),
/// or a fetch-then-check handler not finding the placeholder record
/// (404). A caller answered with one of these proves nothing about what
/// the handler would have decided.
const UNPROVEN: [StatusCode; 4] = [
    StatusCode::BAD_REQUEST,
    StatusCode::NOT_FOUND,
    StatusCode::UNSUPPORTED_MEDIA_TYPE,
    StatusCode::UNPROCESSABLE_ENTITY,
];

/// Where the matrix is blind, as (path, method), sorted.
///
/// On every route the matrix proves the guard: 401 without a principal,
/// 403 without the declared access, and no 401 or 403 from the guard
/// for a caller who holds it.
///
/// A route is *seen* when at least one caller the declaration admits got
/// a status outside [`UNPROVEN`]. That shows the handler itself admitted
/// that caller, so it does not ask that caller for more than the route
/// declares. It shows nothing about the other admitted callers, and it
/// does not exercise ownership rules on real records — the placeholder
/// id names none; those rules are tested with the policy functions.
///
/// On the routes below no admitted caller got that far, so the matrix
/// cannot tell whether the handler asks for more than the route
/// declares. Their declared access rests on the reading of each handler
/// recorded in phase 96's classification, not on this test.
///
/// A declaration stricter than its handler is not visible to the matrix
/// on any route.
const BLIND_ROUTES: &[(&str, &str)] = &[
    ("/ai/assist/{kind}", "POST"),
    ("/ai/backfill", "POST"),
    ("/ai/generate", "POST"),
    ("/ai/images", "POST"),
    ("/ai/models", "POST"),
    ("/ai/models/order", "PUT"),
    ("/ai/models/{id}", "DELETE"),
    ("/ai/models/{id}", "PUT"),
    ("/ai/models/{id}/default", "POST"),
    ("/ai/models/{id}/test", "POST"),
    ("/ai/providers/{provider}", "DELETE"),
    ("/ai/providers/{provider}", "PUT"),
    ("/ai/providers/{provider}/catalog", "GET"),
    ("/ai/providers/{provider}/test", "POST"),
    ("/api-keys", "POST"),
    ("/api-keys/{id}", "DELETE"),
    ("/audience/submissions/{id}", "DELETE"),
    ("/audience/subscribers/{id}", "DELETE"),
    ("/auth/login", "POST"),
    ("/auth/mfa", "POST"),
    ("/auth/mfa/confirm", "POST"),
    ("/auth/mfa/disable", "POST"),
    // Phase 98: public, no principal at all; without a body the extractor
    // answers 415/422. Each handler's own refusals (403
    // `registration_closed`, 503, 429) are about the site and the client
    // address, never a capability; tests/registration.rs covers them.
    ("/auth/register", "POST"),
    ("/auth/register/resend", "POST"),
    ("/auth/reset", "POST"),
    ("/auth/verify", "POST"),
    ("/comments/{id}/approve", "POST"),
    ("/comments/{id}/restore", "POST"),
    ("/comments/{id}/screen", "POST"),
    ("/comments/{id}/spam", "POST"),
    ("/comments/{id}/trash", "POST"),
    // Content types and fields (phase 99): no handler asks for anything
    // beyond the route's declared capability; the matrix's probes carry
    // no valid body or no such type, hence 400/404/422.
    ("/content-types", "POST"),
    ("/content-types/{slug}", "DELETE"),
    ("/content-types/{slug}", "PUT"),
    ("/content-types/{slug}/field-order", "PUT"),
    ("/content-types/{slug}/fields", "GET"),
    ("/content-types/{slug}/fields", "POST"),
    ("/content-types/{slug}/fields/{key}", "DELETE"),
    ("/content-types/{slug}/fields/{key}", "PUT"),
    ("/content-types/{slug}/orphans/{key}/clean-up", "POST"),
    ("/embeds/preview", "GET"),
    ("/forms", "POST"),
    ("/forms/submissions/read", "POST"),
    ("/forms/{id}", "DELETE"),
    ("/forms/{id}", "PUT"),
    ("/forms/{id}/submissions", "GET"),
    ("/forms/{id}/submissions.csv", "GET"),
    ("/import", "POST"),
    ("/mail/settings", "PUT"),
    ("/mail/test", "POST"),
    ("/media", "POST"),
    ("/media/batch-delete", "POST"),
    ("/media/{id}", "DELETE"),
    ("/media/{id}", "GET"),
    ("/media/{id}", "PATCH"),
    ("/media/{id}/alt-text", "POST"),
    ("/media/{id}/edit", "POST"),
    ("/media/{id}/raw", "GET"),
    ("/media/{id}/replace", "POST"),
    ("/media/{id}/restore", "POST"),
    ("/media/{id}/transcribe", "POST"),
    ("/media/{id}/transcript", "GET"),
    ("/media/{id}/usage", "GET"),
    ("/menus", "POST"),
    ("/menus/items/{item_id}", "DELETE"),
    ("/menus/items/{item_id}", "PATCH"),
    ("/menus/{id}", "DELETE"),
    ("/menus/{id}", "GET"),
    ("/menus/{id}/items", "POST"),
    ("/options/{key}", "PUT"),
    ("/patterns", "POST"),
    ("/patterns/{id}", "DELETE"),
    ("/patterns/{id}", "GET"),
    ("/patterns/{id}", "PUT"),
    ("/plugins", "POST"),
    ("/plugins/inspect", "POST"),
    ("/plugins/{id}", "DELETE"),
    ("/plugins/{id}/rollback", "POST"),
    ("/posts", "POST"),
    ("/posts/batch", "POST"),
    ("/posts/link-suggestions", "POST"),
    ("/posts/slug/{type}/{slug}", "GET"),
    ("/posts/{id}", "DELETE"),
    ("/posts/{id}", "GET"),
    ("/posts/{id}", "PUT"),
    ("/posts/{id}/audio", "GET"),
    ("/posts/{id}/autofill", "POST"),
    ("/posts/{id}/autosave", "PUT"),
    ("/posts/{id}/check-links", "POST"),
    ("/posts/{id}/comments", "GET"),
    ("/posts/{id}/comments", "POST"),
    ("/posts/{id}/compose", "POST"),
    ("/posts/{id}/duplicate", "POST"),
    ("/posts/{id}/embed", "POST"),
    ("/posts/{id}/language", "GET"),
    ("/posts/{id}/language", "PUT"),
    ("/posts/{id}/link-check", "GET"),
    ("/posts/{id}/lock", "POST"),
    ("/posts/{id}/preview-token", "POST"),
    ("/posts/{id}/read-aloud", "POST"),
    ("/posts/{id}/related", "GET"),
    ("/posts/{id}/render", "POST"),
    ("/posts/{id}/restore", "POST"),
    ("/posts/{id}/revisions", "GET"),
    ("/posts/{id}/revisions", "POST"),
    ("/posts/{id}/revisions/{rid}", "GET"),
    ("/posts/{id}/revisions/{rid}/restore", "POST"),
    ("/posts/{id}/seo-signals", "GET"),
    ("/posts/{id}/terms", "GET"),
    ("/posts/{id}/terms", "POST"),
    ("/posts/{id}/verify-password", "POST"),
    ("/privacy/erase", "POST"),
    ("/privacy/export", "GET"),
    ("/redirects", "POST"),
    ("/redirects/{*from}", "DELETE"),
    ("/registry/install", "POST"),
    ("/roles", "POST"),
    ("/roles/{slug}", "DELETE"),
    ("/roles/{slug}", "PATCH"),
    ("/search", "GET"),
    ("/setup/account", "POST"),
    ("/setup/claim", "POST"),
    ("/setup/mail/test", "POST"),
    ("/setup/site", "POST"),
    ("/setup/verify-url", "GET"),
    ("/terms", "POST"),
    ("/terms/merge", "POST"),
    ("/terms/{id}", "GET"),
    ("/terms/{id}", "PUT"),
    ("/themes", "POST"),
    ("/themes/drafts/{id}", "DELETE"),
    ("/themes/drafts/{id}", "GET"),
    ("/themes/drafts/{id}", "PATCH"),
    ("/themes/drafts/{id}/chat", "POST"),
    ("/themes/drafts/{id}/messages/{mid}/accept", "POST"),
    ("/themes/drafts/{id}/ops", "POST"),
    ("/themes/drafts/{id}/preview", "GET"),
    ("/themes/drafts/{id}/publish", "POST"),
    ("/themes/drafts/{id}/revert", "POST"),
    ("/themes/drafts/{id}/revisions/{seq}", "GET"),
    ("/themes/preview", "POST"),
    ("/themes/{id}", "DELETE"),
    ("/themes/{id}/activate", "POST"),
    ("/themes/{id}/files", "GET"),
    ("/themes/{id}/files/{*path}", "DELETE"),
    ("/themes/{id}/files/{*path}", "PUT"),
    ("/themes/{id}/tokens", "GET"),
    ("/updates/apply", "POST"),
    ("/users", "POST"),
    ("/users/{id}", "DELETE"),
    ("/users/{id}", "GET"),
    ("/users/{id}", "PATCH"),
    // Phase 98: the placeholder id is a 404 after the capability check;
    // the first check is `policy::account_for_management`, i.e.
    // `ensure(ManageUsers)`, the declared access, then reach.
    ("/users/{id}/confirm", "POST"),
    ("/users/{id}/mfa", "DELETE"),
    ("/users/{id}/resend-confirmation", "POST"),
    ("/users/{id}/reset-link", "POST"),
    ("/users/{id}/role", "PUT"),
    ("/users/{id}/sessions/revoke", "POST"),
    ("/users/{id}/suspend", "POST"),
    ("/webhooks", "POST"),
    ("/webhooks/{id}", "DELETE"),
    ("/webhooks/{id}", "PATCH"),
    ("/webhooks/{id}/deliveries/{delivery_id}/redeliver", "POST"),
    ("/webhooks/{id}/rotate-secret", "POST"),
    ("/webhooks/{id}/test", "POST"),
];

fn allowed(role: Role, access: Access) -> bool {
    let caps = role_caps(role);
    match access {
        Access::Public | Access::Authenticated => true,
        Access::Cap(cap) => caps.contains(&cap),
        Access::AnyOf(any) => any.iter().any(|cap| caps.contains(cap)),
    }
}

/// The registered path with `1` for each `{name}` and `x` for a
/// `{*rest}` wildcard.
fn placeholder_uri(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with("{*") {
                "x"
            } else if segment.starts_with('{') {
                "1"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn gate(rule: &RouteRule) -> Option<Gate> {
    PUBLIC_GATES
        .iter()
        .find(|(method, path, _, _)| *method == rule.method.as_str() && *path == rule.path)
        .map(|(_, _, gate, _)| *gate)
}

/// The capability an [`EXCEPTIONS`] route's handler asks for on top of
/// the declared access.
fn lifting_cap(rule: &RouteRule) -> Option<Capability> {
    EXCEPTIONS
        .iter()
        .find(|(method, path, _, _)| *method == rule.method.as_str() && *path == rule.path)
        .map(|(_, _, cap, _)| *cap)
}

/// What the matrix requires of one response.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Exactly 401.
    Unauthorized,
    /// Exactly 403.
    Forbidden,
    /// Neither 401 nor 403.
    Admitted,
}

impl Expect {
    fn holds(self, status: StatusCode) -> bool {
        match self {
            Expect::Unauthorized => status == StatusCode::UNAUTHORIZED,
            Expect::Forbidden => status == StatusCode::FORBIDDEN,
            Expect::Admitted => {
                status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN
            }
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Expect::Unauthorized => "401",
            Expect::Forbidden => "403",
            Expect::Admitted => "neither 401 nor 403",
        }
    }
}

/// `role` is `None` for the anonymous caller.
fn expectation(route: &RouteRule, role: Option<Role>) -> Expect {
    let holds = |cap: Capability| role.is_some_and(|held| role_caps(held).contains(&cap));
    let declared = match (gate(route), route.access, role) {
        (None, Access::Public, _) => true,
        (Some(Gate::Token), _, _) | (Some(Gate::Setup) | None, _, None) => {
            return Expect::Unauthorized
        }
        (Some(Gate::Setup), _, Some(_)) => holds(Capability::ManageOptions),
        (None, access, Some(held)) => allowed(held, access),
    };
    // The handler's own check, for a caller the guard let through.
    let lifted = lifting_cap(route).is_none_or(holds);
    if declared && lifted {
        Expect::Admitted
    } else {
        Expect::Forbidden
    }
}

async fn call(app: &Router, rule: &RouteRule, cookie: Option<&str>) -> Result<StatusCode, String> {
    let mut request = Request::builder()
        .method(rule.method.clone())
        .uri(placeholder_uri(rule.path));
    if let Some(token) = cookie {
        request = request.header(header::COOKIE, format!("{SESSION_COOKIE}={token}"));
    }
    let body = if [Method::POST, Method::PUT, Method::PATCH].contains(&rule.method) {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from("{}")
    } else {
        Body::empty()
    };
    let request = request.body(body).expect("request");
    // Only the status matters; a streaming body is dropped unread.
    match tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request)).await {
        Ok(response) => Ok(response.expect("infallible").status()),
        Err(_) => Err("no response within 30s".to_owned()),
    }
}

#[tokio::test]
async fn every_route_enforces_its_declared_access() {
    let db = TestDb::new().await;
    let users = UsersRepo::new(db.pool().clone());
    for (id, name, role) in CALLERS {
        users
            .insert(&NewUser {
                id,
                email: &format!("{name}-matrix@example.com"),
                username: &format!("{name}matrix"),
                display_name: name,
                password_hash: None,
                role,
                bio: "",
            })
            .await
            .expect("seed user");
    }
    // A fresh database has no update channel and no registry configured,
    // so the handlers that would fetch them stop at validation: nothing
    // here leaves the process.
    let state = test_state(&db);
    let app = rest::router().with_state(state.clone());
    let rules = rest::rules();
    assert!(!rules.is_empty());

    let mut mismatches = Vec::new();
    let mut blind = Vec::new();
    let mut requests = 0_usize;
    for rule in &rules {
        let mut callers: Vec<(&str, Option<Role>, Option<String>)> =
            vec![("anonymous", None, None)];
        for (id, name, role) in CALLERS {
            // A session per request: `POST /auth/logout` ends the one it
            // is called with, and no later caller may inherit that.
            callers.push((name, Some(role), Some(session(&state, id).await)));
        }
        // Statuses of the callers the route must admit.
        let mut admitted = Vec::new();
        for (caller, role, cookie) in callers {
            requests += 1;
            let expected = expectation(rule, role);
            let got = call(&app, rule, cookie.as_deref()).await;
            if let (Expect::Admitted, Ok(status)) = (expected, &got) {
                admitted.push(*status);
            }
            let ok = got.as_ref().is_ok_and(|status| expected.holds(*status));
            if !ok {
                let got = got.map_or_else(|err| err, |status| status.as_u16().to_string());
                mismatches.push(format!(
                    "{} {} as {caller}: expected {}, got {got}",
                    rule.method,
                    rule.path,
                    expected.describe()
                ));
            }
        }
        if !admitted.is_empty() && admitted.iter().all(|status| UNPROVEN.contains(status)) {
            blind.push((rule.path, rule.method.as_str()));
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} of {requests} requests did not match the declared access:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );

    blind.sort_unstable();
    let committed: Vec<(&str, &str)> = BLIND_ROUTES.to_vec();
    assert_eq!(
        blind, committed,
        "BLIND_ROUTES is out of date.\n\
         On a blind route every caller the declaration admits was answered 400, 404, 415 \
         or 422, so the matrix cannot tell whether the handler asks for more than the route \
         declares. A route new to this list must be read by hand: its handler's first \
         `ensure` must be the declared access. Only then add it (sorted by path, then \
         method). A route that left the list is simply removed.\n\
         computed: {blind:#?}"
    );
}

/// An entry that names no registered route, or the wrong kind of route,
/// would be a silent hole in the matrix.
#[test]
fn exception_tables_name_real_routes() {
    let rules = rest::rules();
    let find = |method: &str, path: &str| {
        rules
            .iter()
            .find(|rule| rule.method.as_str() == method && rule.path == path)
    };
    for (method, path, _, reason) in EXCEPTIONS {
        assert!(
            find(method, path).is_some(),
            "EXCEPTIONS names {method} {path}, which is not registered"
        );
        assert!(!reason.is_empty(), "{method} {path} has no reason");
    }
    for (method, path, _, reason) in PUBLIC_GATES {
        let rule = find(method, path).unwrap_or_else(|| {
            panic!("PUBLIC_GATES names {method} {path}, which is not registered")
        });
        assert_eq!(
            rule.access,
            Access::Public,
            "{method} {path}: a gate of its own is only for a Public route"
        );
        assert!(!reason.is_empty(), "{method} {path} has no reason");
    }
    let distinct =
        |keys: Vec<(&str, &str)>| keys.iter().collect::<BTreeSet<_>>().len() == keys.len();
    assert!(distinct(
        EXCEPTIONS.iter().map(|(m, p, _, _)| (*m, *p)).collect()
    ));
    assert!(distinct(
        PUBLIC_GATES.iter().map(|(m, p, _, _)| (*m, *p)).collect()
    ));
}

const ACCESS_TABLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/ROUTE-ACCESS.md");

/// The one `/api/v1` route registered outside [`super::Guarded`], in
/// `public/mod.rs`: every method of a path a plugin declared goes to that
/// plugin, which decides for itself who may call it. It has no
/// [`RouteRule`], so the matrix does not cover it; the scan below keeps it
/// the only one, and the access table carries a fixed row for it.
const PLUGIN_ROUTE: &str = "/api/v1/plugin/{name}/{*rest}";

const PLUGIN_ROUTE_ACCESS: &str = "plugin-decided (public route; the plugin's own checks)";

fn render_access_table(rules: &[RouteRule]) -> String {
    let mut rows: Vec<(&str, &str, String)> = rules
        .iter()
        .map(|rule| (rule.path, rule.method.as_str(), rule.access.describe()))
        .collect();
    rows.push((
        PLUGIN_ROUTE
            .strip_prefix("/api/v1")
            .expect("an /api/v1 path"),
        "ANY",
        PLUGIN_ROUTE_ACCESS.to_owned(),
    ));
    rows.sort();
    let mut out = String::from(
        "# /api/v1 route access\n\n\
         Generated from the route table by `cargo test -p vyasa-api route_access_table_is_committed`\n\
         (`UPDATE_ROUTE_ACCESS=1` rewrites it). Do not edit by hand.\n\n\
         The `ANY /plugin/…` row is the one route registered outside `authz::Guarded`: a\n\
         plugin's own route, public, decided by the plugin.\n\n\
         | Method | Path | Access |\n|---|---|---|\n",
    );
    for (path, method, access) in rows {
        writeln!(out, "| {method} | {path} | {access} |").expect("write to a string");
    }
    out
}

#[test]
fn route_access_table_is_committed() {
    let rendered = render_access_table(&rest::rules());
    if std::env::var("UPDATE_ROUTE_ACCESS").is_ok_and(|value| value == "1") {
        std::fs::write(ACCESS_TABLE, &rendered).expect("write docs/ROUTE-ACCESS.md");
        return;
    }
    let committed = std::fs::read_to_string(ACCESS_TABLE).unwrap_or_default();
    assert_eq!(
        committed, rendered,
        "docs/ROUTE-ACCESS.md is out of date. Review the access change, then run \
         `UPDATE_ROUTE_ACCESS=1 cargo test -p vyasa-api route_access_table_is_committed` \
         and commit the file."
    );
}

/// `line` without a trailing `//` comment. Good enough for the route
/// modules: a `//` inside a string literal only hides the rest of that
/// line from the scan, and `://` is kept.
fn code_of(line: &str) -> &str {
    let mut from = 0;
    while let Some(at) = line[from..].find("//") {
        let at = from + at;
        if at > 0 && line.as_bytes()[at - 1] == b':' {
            from = at + 2;
        } else {
            return &line[..at];
        }
    }
    line
}

/// Every `.rs` file under `crates/api/src/rest`, as (name, source).
fn rest_sources() -> Vec<(String, String)> {
    sources_under(concat!(env!("CARGO_MANIFEST_DIR"), "/src/rest"))
}

/// Every `.rs` file under `dir`, as (path relative to `dir`, source).
fn sources_under(dir: &str) -> Vec<(String, String)> {
    let mut pending = vec![std::path::PathBuf::from(dir)];
    let mut sources = Vec::new();
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).expect("read source directory") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let name = path
                    .strip_prefix(dir)
                    .expect("under the directory")
                    .display()
                    .to_string();
                sources.push((name, std::fs::read_to_string(&path).expect("read source")));
            }
        }
    }
    sources.sort();
    sources
}

#[test]
fn rest_routes_are_only_registered_through_guarded() {
    // Anything that can put a handler on a router without an `Access`.
    // Built from pieces so this file would pass the same scan.
    let forbidden: Vec<String> = [
        [".", "route("],
        [".", "route_service("],
        [".", "nest("],
        [".", "fallback"],
        ["Router", "::new"],
        ["Router", "::<"],
        ["axum::", "routing"],
        ["Method", "Router"],
    ]
    .iter()
    .map(|parts| parts.concat())
    .collect();

    let sources = rest_sources();
    assert!(
        sources.iter().any(|(name, _)| name == "mod.rs") && sources.len() > 10,
        "the scan did not find the route modules"
    );
    let mut offenders = Vec::new();
    for (name, source) in &sources {
        for (index, line) in source.lines().enumerate() {
            let code = code_of(line);
            for pattern in &forbidden {
                if code.contains(pattern.as_str()) {
                    offenders.push(format!(
                        "rest/{name}:{}: `{pattern}` in: {}",
                        index + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "routes under rest/ are registered only through authz::Guarded, which takes an \
         Access for every handler:\n{}",
        offenders.join("\n")
    );
}

/// `at_layered` hands its second closure the path's guarded methods. The
/// closure may only wrap them in a layer: `|methods| methods.layer(…)`
/// and nothing after it, so it cannot add a method of its own.
#[test]
fn layer_closures_only_add_a_layer() {
    let code: String = include_str!("../rest/mod.rs")
        .lines()
        .map(code_of)
        .flat_map(str::chars)
        .filter(|c| !c.is_whitespace())
        .collect();
    let calls = code.matches("at_layered(").count();
    assert!(calls > 0, "rest/mod.rs no longer uses at_layered");

    let open = "|methods|";
    let mut checked = 0;
    let mut rest = code.as_str();
    while let Some(at) = rest.find(open) {
        let closure = &rest[at + open.len()..];
        let body = closure.strip_prefix('{').unwrap_or(closure);
        let braced = body.len() != closure.len();
        let args = body.strip_prefix("methods.layer(").unwrap_or_else(|| {
            panic!(
                "a layer closure must be `|methods| methods.layer(…)`: {}",
                &closure[..closure.len().min(80)]
            )
        });
        // Walk to the parenthesis that closes `.layer(`.
        let mut depth = 1_usize;
        let end = args
            .char_indices()
            .find_map(|(index, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(index)
            })
            .expect("unbalanced parentheses");
        let after = &args[end + 1..];
        // Then the closure ends, and with it the `at_layered(…)` call.
        let closes = if braced {
            after.starts_with("},)") || after.starts_with("})")
        } else {
            after.starts_with(",)") || after.starts_with(')')
        };
        assert!(
            closes,
            "a layer closure does more than add a layer: …{}",
            &after[..after.len().min(80)]
        );
        checked += 1;
        rest = after;
    }
    assert_eq!(
        checked, calls,
        "every at_layered call takes a `|methods| methods.layer(…)` closure"
    );
}

#[test]
fn no_method_is_declared_twice() {
    let mut seen = BTreeSet::new();
    let twice: Vec<String> = rest::rules()
        .iter()
        .filter(|rule| !seen.insert((rule.method.as_str().to_owned(), rule.path)))
        .map(|rule| format!("{} {}", rule.method, rule.path))
        .collect();
    assert!(twice.is_empty(), "declared twice: {}", twice.join(", "));
}

/// `source` without comments, `#[cfg(test)]` modules and whitespace.
///
/// A test module is skipped from its `mod … {` line to the line that
/// closes it, counting braces; braces inside string literals come in
/// pairs in this crate's sources (`"{id}"`), which is all this relies on.
fn production_code(source: &str) -> String {
    let mut out = String::new();
    let mut lines = source.lines().map(code_of);
    while let Some(line) = lines.next() {
        if line.trim() != "#[cfg(test)]" {
            out.extend(line.chars().filter(|c| !c.is_whitespace()));
            continue;
        }
        // The item the attribute is on: only an inline module is skipped.
        let Some(item) = lines.next() else { break };
        let item = item.trim();
        let module = item.starts_with("mod ") || item.starts_with("pub(crate) mod ");
        if !module {
            out.extend(item.chars().filter(|c| !c.is_whitespace()));
            continue;
        }
        let braces = |text: &str| {
            let count = |brace| i64::try_from(text.matches(brace).count()).expect("few braces");
            count('{') - count('}')
        };
        let mut depth = braces(item);
        while depth > 0 {
            let Some(inner) = lines.next() else { break };
            depth += braces(inner);
        }
    }
    out
}

/// The route registrations in `code` (from [`production_code`]) whose
/// path is a literal under `/api/v1`, as (call, path). `/v1…` counts too:
/// it would land there from inside the `/api` nest.
fn api_v1_registrations(code: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for call in ["route", "route_service", "nest", "nest_service"] {
        let open = format!(".{call}(\"");
        let mut rest = code;
        while let Some(at) = rest.find(&open) {
            rest = &rest[at + open.len()..];
            let path = &rest[..rest.find('"').expect("an unterminated literal")];
            if ["/api/v1", "/v1"]
                .iter()
                .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
            {
                found.push((call.to_owned(), path.to_owned()));
            }
        }
    }
    found
}

#[test]
fn the_scan_sees_registrations_and_skips_test_modules() {
    let source = "
        fn router() {
            Router::new()
                .route(
                    \"/api/v1/raw\", // a comment
                    get(handler),
                )
                .nest(\"/v1/inner\", other())
                .route(\"/api/v10\", get(handler))
                .route(\"/plain\", get(handler))
        }
        #[cfg(test)]
        mod tests {
            fn app() { Router::new().route(\"/api/v1/in-a-test\", get(handler)) }
        }
        fn after() { Router::new().route_service(\"/api/v1/late\", service) }
    ";
    let mut found = api_v1_registrations(&production_code(source));
    found.sort();
    let expected: Vec<(String, String)> = [
        ("nest", "/v1/inner"),
        ("route", "/api/v1/raw"),
        ("route_service", "/api/v1/late"),
    ]
    .iter()
    .map(|(call, path)| ((*call).to_owned(), (*path).to_owned()))
    .collect();
    assert_eq!(found, expected);
}

/// Outside `rest/`, `/api/v1` is touched in exactly two places: `main.rs`
/// mounts the guarded router there, and `public/mod.rs` registers the
/// plugin route ([`PLUGIN_ROUTE`]). Anything else would be an `/api/v1`
/// route with no declared access, outside the matrix and the access table.
///
/// The scan reads path literals; a path built at run time or held in a
/// constant would pass it unseen.
#[test]
fn api_v1_routes_outside_rest_are_the_mount_and_the_plugin_route() {
    let sources = sources_under(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let outside_rest: Vec<(&str, String)> = sources
        .iter()
        .filter(|(name, _)| !name.starts_with("rest/") && !name.ends_with("tests.rs"))
        .map(|(name, source)| (name.as_str(), production_code(source)))
        .collect();
    assert!(
        outside_rest.len() > 40 && outside_rest.iter().any(|(name, _)| *name == "main.rs"),
        "the scan did not find the crate's sources"
    );

    let mut found = Vec::new();
    for (name, code) in &outside_rest {
        for (call, path) in api_v1_registrations(code) {
            found.push(format!("{name}: {call} {path}"));
        }
    }
    found.sort();
    assert_eq!(
        found,
        [
            "main.rs: nest /api/v1".to_owned(),
            format!("public/mod.rs: route {PLUGIN_ROUTE}"),
        ],
        "an /api/v1 route is registered outside rest/, so outside authz::Guarded. The plugin \
         route is the only exception (docs/SECURITY.md, Authorization)."
    );

    // The guarded router is mounted once, at `/api/v1`, and nowhere else.
    let mount = concat!(".nest(\"/api/v1\",", "rest::router())");
    let router = concat!("rest::", "router()");
    for (name, code) in &outside_rest {
        let expected = usize::from(*name == "main.rs");
        assert_eq!(
            code.matches(router).count(),
            expected,
            "{name}: the REST router is built only for its one mount in main.rs"
        );
        assert_eq!(code.matches(mount).count(), expected, "{name}");
    }
}

/// The application as `vyasa serve` composes it. axum panics when two
/// routers claim the same path, and only when they are put together, so
/// building it here is what turns such a conflict into a failing test
/// instead of a server that does not start.
#[tokio::test]
async fn the_composed_application_builds_and_routes() {
    let db = TestDb::new().await;
    let state = test_state(&db);
    let app = crate::app_router(&state);

    let get = |uri: &'static str| {
        let app = app.clone();
        async move {
            let request = Request::builder()
                .uri(uri)
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    4096,
                ))))
                .body(Body::empty())
                .expect("request");
            app.oneshot(request).await.expect("infallible").status()
        }
    };
    // A guarded route answers through the mount.
    assert_eq!(get("/api/v1/auth/me").await, StatusCode::UNAUTHORIZED);
    // The plugin route is reached without a principal; no plugin declares
    // this path, so it is the site's 404 rather than a 401.
    assert_eq!(get("/api/v1/plugin/none/x").await, StatusCode::NOT_FOUND);
    // An unregistered `/api/v1` path is not a guarded route either.
    assert_eq!(get("/api/v1/no-such-route").await, StatusCode::NOT_FOUND);
}
