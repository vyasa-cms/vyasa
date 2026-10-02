#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(missing_docs)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use tower::ServiceExt;
use vyasa_common::{AiConfig, DbConfig, JobsConfig, LogConfig, Secret, VyasaConfig};
use vyasa_core::user::{Capability, SESSION_TTL};
use vyasa_db::models::Role;
use vyasa_db::repo::{ApiKeysRepo, NewUser, SessionsRepo, UsersRepo};
use vyasa_testkit::TestDb;

use super::{authorize, Access, Guarded, RouteRule};
use crate::middleware::api_key::ApiKeyPrincipal;
use crate::middleware::auth::{lookups, CurrentUser, SESSION_COOKIE};
use crate::middleware::Principal;
use crate::state::AppState;

const ADMIN: i64 = 96_001;
const EDITOR: i64 = 96_002;
const CONTRIBUTOR: i64 = 96_003;
const SUBSCRIBER: i64 = 96_004;
const SUSPENDED: i64 = 96_005;

/// An [`AppState`] whose media, index and registry directories are a
/// temporary directory of its own, removed when this is dropped.
pub(crate) struct TestState {
    state: AppState,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for TestState {
    type Target = AppState;

    fn deref(&self) -> &AppState {
        &self.state
    }
}

pub(crate) fn test_state(db: &TestDb) -> TestState {
    let dir = tempfile::tempdir().expect("temp dir");
    let state = AppState::new(dummy_config(dir.path()), db.pool().clone());
    TestState { state, _dir: dir }
}

fn dummy_config(dir: &Path) -> VyasaConfig {
    VyasaConfig {
        secret_key: None,
        database_url: Secret::new("postgres://dummy".to_string()),
        bind_addr: "127.0.0.1:0".to_string(),
        media_dir: dir.join("media"),
        index_dir: dir.join("index"),
        registry_dir: dir.join("registry"),
        debug: true,
        plugin_trusted_keys: Vec::new(),
        db: DbConfig {
            max_connections: 4,
            acquire_timeout_secs: 5,
        },
        log: LogConfig::default(),
        jobs: JobsConfig::default(),
        ai: AiConfig::default(),
        smtp: None,
        cdn: None,
        storage: None,
        trusted_proxies: Vec::new(),
        trust_cf_connecting_ip: false,
    }
}

/// A handler that records it ran.
async fn ran(State(_): State<AppState>) -> &'static str {
    "ran"
}

fn app() -> (Router<AppState>, Vec<RouteRule>) {
    Guarded::new()
        .at("/open", |r| r.get(Access::Public, ran))
        .at("/me", |r| r.get(Access::Authenticated, ran))
        .at("/themes", |r| {
            r.get(Access::Cap(Capability::ManageThemes), ran)
                .post(Access::Cap(Capability::ManageOptions), ran)
        })
        .at("/media", |r| {
            r.get(
                Access::AnyOf(&[Capability::UploadMedia, Capability::EditPosts]),
                ran,
            )
        })
        .finish()
}

/// State over a fresh database holding one user per role used here.
async fn seeded(db: &TestDb) -> TestState {
    let users = UsersRepo::new(db.pool().clone());
    for (id, name, role) in [
        (ADMIN, "admin", Role::Admin),
        (EDITOR, "editor", Role::Editor),
        (CONTRIBUTOR, "contributor", Role::Contributor),
        (SUBSCRIBER, "subscriber", Role::Subscriber),
        (SUSPENDED, "suspended", Role::Admin),
    ] {
        users
            .insert(&NewUser {
                id,
                email: &format!("{name}-authz@example.com"),
                username: &format!("{name}authz"),
                display_name: name,
                password_hash: None,
                role,
                bio: "",
            })
            .await
            .expect("seed user");
    }
    test_state(db)
}

/// A live session for `user_id`; the token is unique across tests so the
/// per-token lookup counter is not shared.
pub(crate) async fn session(state: &AppState, user_id: i64) -> String {
    let token = format!("authz-{user_id}-{:016x}", vyasa_common::next_id_i64());
    SessionsRepo::new(state.pool.clone())
        .insert(&token, user_id, SESSION_TTL, None, None)
        .await
        .expect("session");
    token
}

enum Auth<'a> {
    None,
    Cookie(&'a str),
    Bearer(&'a str),
}

async fn send(
    router: &Router<AppState>,
    state: &AppState,
    method: Method,
    path: &str,
    auth: Auth<'_>,
) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(path);
    request = match auth {
        Auth::None => request,
        Auth::Cookie(token) => request.header(header::COOKIE, format!("{SESSION_COOKIE}={token}")),
        Auth::Bearer(raw) => request.header(header::AUTHORIZATION, format!("Bearer {raw}")),
    };
    let response = router
        .clone()
        .with_state(state.clone())
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn status(
    router: &Router<AppState>,
    state: &AppState,
    method: Method,
    path: &str,
    auth: Auth<'_>,
) -> StatusCode {
    send(router, state, method, path, auth).await.0
}

#[tokio::test]
async fn public_needs_no_principal() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, _) = app();
    let (code, body) = send(&router, &state, Method::GET, "/open", Auth::None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, "ran");
}

#[tokio::test]
async fn no_principal_is_401_and_handler_not_called() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = {
        let calls = Arc::clone(&calls);
        move || {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                "ran"
            }
        }
    };
    let (router, _) = Guarded::new()
        .at("/me", |r| r.get(Access::Authenticated, counted.clone()))
        .at("/themes", |r| {
            r.get(Access::Cap(Capability::ManageThemes), counted.clone())
        })
        .finish();

    for path in ["/me", "/themes"] {
        assert_eq!(
            status(&router, &state, Method::GET, path, Auth::None).await,
            StatusCode::UNAUTHORIZED,
            "{path} without a cookie"
        );
        assert_eq!(
            status(&router, &state, Method::GET, path, Auth::Cookie("nope")).await,
            StatusCode::UNAUTHORIZED,
            "{path} with an unknown session"
        );
    }

    // Suspended without revoking the sessions: the flag alone must end
    // the session, not the suspend endpoint remembering to delete them.
    let suspended = session(&state, SUSPENDED).await;
    assert_eq!(
        status(
            &router,
            &state,
            Method::GET,
            "/me",
            Auth::Cookie(&suspended)
        )
        .await,
        StatusCode::OK,
        "the session works before suspension"
    );
    assert_eq!(calls.swap(0, Ordering::SeqCst), 1);
    sqlx::query("UPDATE users SET suspended_at = now() WHERE id = $1")
        .bind(SUSPENDED)
        .execute(&state.pool)
        .await
        .expect("suspend");
    assert_eq!(
        SessionsRepo::new(state.pool.clone())
            .count_for_user(SUSPENDED)
            .await
            .expect("count"),
        1,
        "the session row is still there"
    );

    // An expired session.
    let expired = "authz-expired-session";
    SessionsRepo::new(state.pool.clone())
        .insert(expired, ADMIN, chrono::Duration::seconds(-60), None, None)
        .await
        .expect("expired session");

    for path in ["/me", "/themes"] {
        for token in [suspended.as_str(), expired] {
            assert_eq!(
                status(&router, &state, Method::GET, path, Auth::Cookie(token)).await,
                StatusCode::UNAUTHORIZED,
                "{path} with {token}"
            );
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no handler ran on a 401");
}

#[tokio::test]
async fn missing_capability_is_403() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, _) = app();
    let subscriber = session(&state, SUBSCRIBER).await;
    let admin = session(&state, ADMIN).await;
    assert_eq!(
        status(
            &router,
            &state,
            Method::GET,
            "/themes",
            Auth::Cookie(&subscriber)
        )
        .await,
        StatusCode::FORBIDDEN
    );
    let (code, body) = send(
        &router,
        &state,
        Method::GET,
        "/themes",
        Auth::Cookie(&admin),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, "ran");
}

#[tokio::test]
async fn each_method_has_its_own_access() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, rules) = app();
    let admin = session(&state, ADMIN).await;
    let editor = session(&state, EDITOR).await;
    for method in [Method::GET, Method::POST] {
        assert_eq!(
            status(
                &router,
                &state,
                method.clone(),
                "/themes",
                Auth::Cookie(&admin)
            )
            .await,
            StatusCode::OK,
            "admin {method}"
        );
        assert_eq!(
            status(
                &router,
                &state,
                method.clone(),
                "/themes",
                Auth::Cookie(&editor)
            )
            .await,
            StatusCode::FORBIDDEN,
            "editor {method}"
        );
    }
    let themes: Vec<(Method, Access)> = rules
        .iter()
        .filter(|rule| rule.path == "/themes")
        .map(|rule| (rule.method.clone(), rule.access))
        .collect();
    assert_eq!(
        themes,
        vec![
            (Method::GET, Access::Cap(Capability::ManageThemes)),
            (Method::POST, Access::Cap(Capability::ManageOptions)),
        ]
    );
}

#[tokio::test]
async fn undeclared_method_is_405() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, _) = app();
    let admin = session(&state, ADMIN).await;
    assert_eq!(
        status(
            &router,
            &state,
            Method::DELETE,
            "/themes",
            Auth::Cookie(&admin)
        )
        .await,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

#[tokio::test]
async fn any_of_accepts_either() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, _) = app();
    let contributor = session(&state, CONTRIBUTOR).await;
    let subscriber = session(&state, SUBSCRIBER).await;
    assert_eq!(
        status(
            &router,
            &state,
            Method::GET,
            "/media",
            Auth::Cookie(&contributor)
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status(
            &router,
            &state,
            Method::GET,
            "/media",
            Auth::Cookie(&subscriber)
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn api_key_needs_its_own_grant() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, _) = app();
    let raw = "vy_authz_test_key_edit_posts_only";
    ApiKeysRepo::new(state.pool.clone())
        .insert(
            96_100,
            ADMIN,
            "edit posts only",
            &ApiKeyPrincipal::hash_raw(raw),
            &serde_json::json!(["edit_posts"]),
        )
        .await
        .expect("api key");
    assert_eq!(
        status(&router, &state, Method::GET, "/themes", Auth::Bearer(raw)).await,
        StatusCode::FORBIDDEN,
        "the owner may manage themes; the key may not"
    );
    assert_eq!(
        status(&router, &state, Method::GET, "/media", Auth::Bearer(raw)).await,
        StatusCode::OK
    );
    assert_eq!(
        status(
            &router,
            &state,
            Method::GET,
            "/me",
            Auth::Bearer("vy_unknown")
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn extractors_reuse_the_resolved_principal() {
    async fn who(principal: Principal, current: CurrentUser) -> String {
        format!("{}:{}", principal.user().id, current.user.id)
    }

    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, _) = Guarded::new()
        .at("/who", |r| r.get(Access::Authenticated, who))
        .finish();
    let editor = session(&state, EDITOR).await;
    assert_eq!(lookups::count(&editor), 0);
    let (code, body) = send(&router, &state, Method::GET, "/who", Auth::Cookie(&editor)).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, format!("{EDITOR}:{EDITOR}"));
    assert_eq!(
        lookups::count(&editor),
        1,
        "the guard resolves the session once; both extractors reuse it"
    );

    // `authorize` leaves the principal where the extractors look for it.
    let request = Request::builder()
        .uri("/who")
        .header(header::COOKIE, format!("{SESSION_COOKIE}={editor}"))
        .body(())
        .expect("request");
    let (mut parts, ()) = request.into_parts();
    assert!(authorize(&mut parts, &state, Access::Authenticated)
        .await
        .is_ok());
    let stored = parts
        .extensions
        .get::<Principal>()
        .expect("principal stored");
    assert_eq!(stored.user().id, EDITOR);
}

#[tokio::test]
async fn api_key_cannot_become_current_user() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let session_only = {
        let calls = Arc::clone(&calls);
        move |current: CurrentUser| {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                current.user.id.to_string()
            }
        }
    };
    let (router, _) = Guarded::new()
        .at("/session-cap", |r| {
            r.get(Access::Cap(Capability::EditPosts), session_only.clone())
        })
        .at("/session-any", |r| {
            r.get(Access::Authenticated, session_only.clone())
        })
        .finish();
    let raw = "vy_authz_test_key_not_a_session";
    ApiKeysRepo::new(state.pool.clone())
        .insert(
            96_101,
            ADMIN,
            "edit posts",
            &ApiKeyPrincipal::hash_raw(raw),
            &serde_json::json!(["edit_posts"]),
        )
        .await
        .expect("api key");
    for path in ["/session-cap", "/session-any"] {
        assert_eq!(
            status(&router, &state, Method::GET, path, Auth::Bearer(raw)).await,
            StatusCode::UNAUTHORIZED,
            "{path}: the key passes the guard but is not a session"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the handler did not run");

    // The same routes do serve a session.
    let admin = session(&state, ADMIN).await;
    let (code, body) = send(
        &router,
        &state,
        Method::GET,
        "/session-cap",
        Auth::Cookie(&admin),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, ADMIN.to_string());
}

#[tokio::test]
async fn bearer_plus_cookie_on_session_only_route_is_401() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let session_only = {
        let calls = Arc::clone(&calls);
        move |current: CurrentUser| {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                current.user.id.to_string()
            }
        }
    };
    let (router, _) = Guarded::new()
        .at("/session-users", |r| {
            r.get(Access::Cap(Capability::ManageUsers), session_only.clone())
        })
        .finish();
    let raw = "vy_authz_test_key_manage_users";
    ApiKeysRepo::new(state.pool.clone())
        .insert(
            96_102,
            ADMIN,
            "manage users",
            &ApiKeyPrincipal::hash_raw(raw),
            &serde_json::json!(["manage_users"]),
        )
        .await
        .expect("api key");
    let subscriber = session(&state, SUBSCRIBER).await;

    // The guard authorises the key (an admin's, granted manage_users). The
    // handler must not then run as the subscriber whose cookie rides along.
    let request = Request::builder()
        .method(Method::GET)
        .uri("/session-users")
        .header(header::AUTHORIZATION, format!("Bearer {raw}"))
        .header(header::COOKIE, format!("{SESSION_COOKIE}={subscriber}"))
        .body(Body::empty())
        .expect("request");
    let response = router
        .clone()
        .with_state(state.clone())
        .oneshot(request)
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the handler did not run");
    assert_eq!(
        lookups::count(&subscriber),
        0,
        "the cookie was never resolved"
    );

    // The cookie alone is judged on its own: a subscriber may not.
    assert_eq!(
        status(
            &router,
            &state,
            Method::GET,
            "/session-users",
            Auth::Cookie(&subscriber)
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn head_uses_gets_access() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, _) = app();
    let subscriber = session(&state, SUBSCRIBER).await;
    let admin = session(&state, ADMIN).await;
    assert_eq!(
        status(&router, &state, Method::HEAD, "/themes", Auth::None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(
            &router,
            &state,
            Method::HEAD,
            "/themes",
            Auth::Cookie(&subscriber)
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(
            &router,
            &state,
            Method::HEAD,
            "/themes",
            Auth::Cookie(&admin)
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn layered_routes_are_still_guarded() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let (router, rules) = Guarded::new()
        .at_layered(
            "/upload",
            |r| r.post(Access::Cap(Capability::ManageThemes), ran),
            |methods| methods.layer(axum::extract::DefaultBodyLimit::max(1024)),
        )
        .finish();
    assert_eq!(rules.len(), 1);
    let subscriber = session(&state, SUBSCRIBER).await;
    let admin = session(&state, ADMIN).await;
    assert_eq!(
        status(&router, &state, Method::POST, "/upload", Auth::None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(
            &router,
            &state,
            Method::POST,
            "/upload",
            Auth::Cookie(&subscriber)
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(
            &router,
            &state,
            Method::POST,
            "/upload",
            Auth::Cookie(&admin)
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn merged_routers_keep_their_rules_and_guards() {
    let db = TestDb::new().await;
    let state = seeded(&db).await;
    let extra = Guarded::new().at("/plugins", |r| {
        r.get(Access::Cap(Capability::ManagePlugins), ran)
            .delete(Access::Cap(Capability::ManagePlugins), ran)
    });
    let merged = Guarded::new()
        .at("/open", |r| r.get(Access::Public, ran))
        .merge(extra);
    let (router, rules) = merged.finish();
    assert_eq!(rules.len(), 3, "one rule from the base, two merged in");
    assert_eq!(
        rules
            .iter()
            .filter(|rule| rule.path == "/plugins")
            .map(|rule| (rule.method.clone(), rule.access))
            .collect::<Vec<_>>(),
        vec![
            (Method::GET, Access::Cap(Capability::ManagePlugins)),
            (Method::DELETE, Access::Cap(Capability::ManagePlugins)),
        ]
    );
    let subscriber = session(&state, SUBSCRIBER).await;
    let admin = session(&state, ADMIN).await;
    for method in [Method::GET, Method::DELETE] {
        assert_eq!(
            status(&router, &state, method.clone(), "/plugins", Auth::None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(
                &router,
                &state,
                method.clone(),
                "/plugins",
                Auth::Cookie(&subscriber)
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(&router, &state, method, "/plugins", Auth::Cookie(&admin)).await,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn rules_list_every_registered_method() {
    let (_, rules) = app();
    let triples: Vec<(Method, &str, Access)> = rules
        .iter()
        .map(|rule| (rule.method.clone(), rule.path, rule.access))
        .collect();
    assert_eq!(
        triples,
        vec![
            (Method::GET, "/open", Access::Public),
            (Method::GET, "/me", Access::Authenticated),
            (
                Method::GET,
                "/themes",
                Access::Cap(Capability::ManageThemes)
            ),
            (
                Method::POST,
                "/themes",
                Access::Cap(Capability::ManageOptions)
            ),
            (
                Method::GET,
                "/media",
                Access::AnyOf(&[Capability::UploadMedia, Capability::EditPosts])
            ),
        ]
    );
}

#[tokio::test]
async fn describe_is_stable() {
    assert_eq!(Access::Public.describe(), "public");
    assert_eq!(Access::Authenticated.describe(), "authenticated");
    assert_eq!(Access::Cap(Capability::EditPosts).describe(), "edit_posts");
    assert_eq!(
        Access::AnyOf(&[Capability::UploadMedia, Capability::EditPosts]).describe(),
        "upload_media or edit_posts"
    );
}
