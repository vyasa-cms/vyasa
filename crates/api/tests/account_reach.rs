//! With custom roles, `manage_users` no longer means "administrator". A
//! user manager hands out only roles within their own capabilities, and
//! acts only on accounts that hold no more than they do.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{NewRole, RolesRepo, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn call(base: &str, cookie: &str, method: &str, path: &str, body: Option<Value>) -> ureq::Response {
    let request = ureq::request(method, &format!("{base}/api/v1{path}")).set("cookie", cookie);
    http(match body {
        Some(body) => request.send_json(body),
        None => request.call(),
    })
}

fn status(base: &str, cookie: &str, method: &str, path: &str, body: Option<Value>) -> u16 {
    call(base, cookie, method, path, body).status()
}

fn refusal(r: ureq::Response) -> (u16, String) {
    let status = r.status();
    let body: Value = r.into_json().expect("a JSON body");
    (
        status,
        body["message"].as_str().unwrap_or_default().to_owned(),
    )
}

async fn role(pool: &PgPool, slug: &str, caps: &[&str]) {
    let capabilities: Vec<String> = caps.iter().map(|c| (*c).to_owned()).collect();
    RolesRepo::new(pool.clone())
        .insert(&NewRole {
            slug,
            name: slug,
            description: "",
            capabilities: &capabilities,
        })
        .await
        .expect("role");
}

/// A user holding the custom role `slug`.
async fn seed_with_role(pool: &PgPool, slug: &str) -> common::Seeded {
    let user = common::seed_user(pool, Role::Subscriber).await;
    UsersRepo::new(pool.clone())
        .set_custom_role(user.id, Some(slug))
        .await
        .expect("custom role");
    user
}

async fn roles_of(pool: &PgPool, id: i64) -> Option<(String, Option<String>)> {
    sqlx::query_as("SELECT role, custom_role FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .expect("user roles")
}

async fn field(pool: &PgPool, id: i64, column: &str) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT {column}::text FROM users WHERE id = $1"))
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("user field")
}

/// The roles every test here uses: a user manager who holds nothing
/// else, and two roles to hand out, one within their capabilities and
/// one beyond.
async fn seed_roles(pool: &PgPool) {
    role(pool, "user-manager", &["view_admin", "manage_users"]).await;
    role(pool, "greeter", &["view_admin"]).await;
    role(pool, "drafter", &["view_admin", "edit_posts"]).await;
}

/// Every way of acting on the account `id`, as (what, method, path, body).
///
/// Four of these are a `POST` under `/users`, which the server limits to
/// ten a minute per address together with sign-ins; the tests start a
/// server per account acted on to stay under that.
fn actions(id: i64) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "edit",
            "PATCH",
            format!("/users/{id}"),
            Some(json!({ "email": "taken-over@example.com", "display_name": "Taken over" })),
        ),
        (
            "role",
            "PUT",
            format!("/users/{id}/role"),
            Some(json!({ "role": "subscriber" })),
        ),
        (
            "suspend",
            "POST",
            format!("/users/{id}/suspend"),
            Some(json!({ "suspended": true })),
        ),
        (
            "reinstate",
            "POST",
            format!("/users/{id}/suspend"),
            Some(json!({ "suspended": false })),
        ),
        (
            "reset link",
            "POST",
            format!("/users/{id}/reset-link"),
            None,
        ),
        ("mfa", "DELETE", format!("/users/{id}/mfa"), None),
        (
            "sessions",
            "POST",
            format!("/users/{id}/sessions/revoke"),
            None,
        ),
        ("delete", "DELETE", format!("/users/{id}"), None),
    ]
}

#[tokio::test]
async fn a_user_manager_grants_only_roles_within_their_own_capabilities() {
    let db = TestDb::new().await;
    let pool = db.pool();
    common::seed_user(pool, Role::Admin).await;
    seed_roles(pool).await;
    let manager = seed_with_role(pool, "user-manager").await;
    let subscriber = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &manager.email, &manager.password);
    let set_role = |id: i64, role: &str| {
        call(
            base,
            &cookie,
            "PUT",
            &format!("/users/{id}/role"),
            Some(json!({ "role": role })),
        )
    };

    // Changing a role: built-in roles and custom ones alike.
    for beyond in ["admin", "editor", "author", "contributor", "drafter"] {
        let (status, message) = refusal(set_role(subscriber.id, beyond));
        assert_eq!(status, 403, "granting {beyond}");
        assert!(message.contains("edit_posts"), "{beyond}: {message}");
        // Least of all to themselves.
        assert_eq!(set_role(manager.id, beyond).status(), 403, "self {beyond}");
    }
    assert_eq!(
        roles_of(pool, subscriber.id).await.unwrap(),
        ("subscriber".to_owned(), None)
    );
    assert_eq!(
        roles_of(pool, manager.id).await.unwrap(),
        ("subscriber".to_owned(), Some("user-manager".to_owned()))
    );
    for within in ["greeter", "user-manager", "subscriber"] {
        assert_eq!(
            set_role(subscriber.id, within).status(),
            204,
            "granting {within}"
        );
    }
    assert_eq!(set_role(subscriber.id, "no-such-role").status(), 400);

    // Creating (inviting) a user with a role. (A second server: see
    // `actions` on the limit.)
    drop(server);
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let create = |email: &str, role: &str| {
        call(
            base,
            &cookie,
            "POST",
            "/users",
            Some(json!({ "email": email, "password": "a-long-password", "role": role })),
        )
    };
    for beyond in ["admin", "contributor", "drafter"] {
        let email = format!("new-{beyond}@example.com");
        let (status, message) = refusal(create(&email, beyond));
        assert_eq!(status, 403, "creating a {beyond}");
        assert!(message.contains("edit_posts"), "{beyond}: {message}");
        let made: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email = $1")
            .bind(&email)
            .fetch_one(pool)
            .await
            .expect("count");
        assert_eq!(made, 0, "no {beyond} account is left behind");
    }
    let plain = create("new-subscriber@example.com", "subscriber");
    assert_eq!(plain.status(), 201);
    let custom = create("new-greeter@example.com", "greeter");
    assert_eq!(custom.status(), 201);
    let custom: Value = custom.into_json().unwrap();
    assert_eq!(custom["role"], "subscriber");
    assert_eq!(custom["custom_role"], "greeter");
    assert_eq!(custom["role_name"], "greeter");
    assert_eq!(create("new-x@example.com", "no-such-role").status(), 400);
    // No role named: a subscriber, as before.
    let default = call(
        base,
        &cookie,
        "POST",
        "/users",
        Some(json!({ "email": "new-default@example.com", "password": "a-long-password" })),
    );
    assert_eq!(default.status(), 201);
}

#[tokio::test]
async fn a_user_manager_does_not_act_on_accounts_that_hold_more_than_they_do() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    common::seed_user(pool, Role::Admin).await;
    let editor = common::seed_user(pool, Role::Editor).await;
    seed_roles(pool).await;
    let drafter = seed_with_role(pool, "drafter").await;
    // A subscriber by role, given one capability by a per-user override.
    let overridden = common::seed_user(pool, Role::Subscriber).await;
    sqlx::query("UPDATE users SET meta = '{\"enabled_caps\": [\"manage_options\"]}' WHERE id = $1")
        .bind(overridden.id)
        .execute(pool)
        .await
        .expect("override");
    let manager = seed_with_role(pool, "user-manager").await;
    let server = TestServer::start(common::BIN, &db);
    let admin_cookie = common::login_cookie(server.base(), &admin.email, &admin.password);
    drop(server);

    for (who, target) in [
        ("an administrator", &admin),
        ("an editor", &editor),
        ("a custom role with edit_posts", &drafter),
        ("a subscriber with an override", &overridden),
    ] {
        let server = TestServer::start(common::BIN, &db);
        let base = server.base();
        let cookie = common::login_cookie(base, &manager.email, &manager.password);
        let before = (
            roles_of(pool, target.id).await,
            field(pool, target.id, "email").await,
            field(pool, target.id, "suspended_at").await,
        );
        for (what, method, path, body) in actions(target.id) {
            let (status, message) = refusal(call(base, &cookie, method, &path, body));
            assert_eq!(status, 403, "{what} on {who}");
            assert!(
                message.contains("this account holds capabilities you do not have"),
                "{what} on {who}: {message}"
            );
        }
        // Erasing personal data deletes the account too.
        let erase = call(
            base,
            &cookie,
            "POST",
            "/privacy/erase",
            Some(json!({ "email": target.email })),
        );
        assert_eq!(erase.status(), 403, "erase on {who}");
        let after = (
            roles_of(pool, target.id).await,
            field(pool, target.id, "email").await,
            field(pool, target.id, "suspended_at").await,
        );
        assert_eq!(before, after, "{who} is untouched");
    }
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &manager.email, &manager.password);
    // The administrator's session was not ended, and no reset token was
    // minted for anyone.
    assert_eq!(status(base, &admin_cookie, "GET", "/auth/me", None), 200);
    let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reset_tokens")
        .fetch_one(pool)
        .await
        .expect("tokens");
    assert_eq!(tokens, 0);

    // Reading is what the users list already shows them.
    assert_eq!(
        status(base, &cookie, "GET", &format!("/users/{}", admin.id), None),
        200
    );
    // A user who is not there is still a 404, not a 403.
    assert_eq!(
        status(base, &cookie, "PATCH", "/users/9999999", Some(json!({}))),
        404
    );
}

#[tokio::test]
async fn a_user_manager_manages_accounts_within_their_capabilities() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    seed_roles(pool).await;
    let manager = seed_with_role(pool, "user-manager").await;

    // A subscriber, a custom role within theirs, and a peer.
    let subscriber = common::seed_user(pool, Role::Subscriber).await;
    let greeter = seed_with_role(pool, "greeter").await;
    let peer = seed_with_role(pool, "user-manager").await;
    for (who, target) in [
        ("a subscriber", &subscriber),
        ("a greeter", &greeter),
        ("a peer", &peer),
    ] {
        let server = TestServer::start(common::BIN, &db);
        let base = server.base();
        let cookie = common::login_cookie(base, &manager.email, &manager.password);
        for (what, method, path, body) in actions(target.id) {
            let body = match what {
                "edit" => Some(json!({ "display_name": "Renamed" })),
                _ => body,
            };
            let got = status(base, &cookie, method, &path, body);
            let expected = match what {
                // No mail relay in the test server: the guard was passed.
                "reset link" => 400,
                "edit" | "sessions" => 200,
                _ => 204,
            };
            assert_eq!(got, expected, "{what} on {who}");
        }
        assert!(
            roles_of(pool, target.id).await.is_none(),
            "{who} is deleted"
        );
    }

    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &manager.email, &manager.password);
    // Deleting someone in reach may hand their content to anyone.
    let leaving = common::seed_user(pool, Role::Subscriber).await;
    assert_eq!(
        status(
            base,
            &cookie,
            "DELETE",
            &format!("/users/{}?reassign_to={}", leaving.id, admin.id),
            None
        ),
        204
    );
    // Erasure of someone in reach goes through.
    let erased = common::seed_user(pool, Role::Subscriber).await;
    let done = call(
        base,
        &cookie,
        "POST",
        "/privacy/erase",
        Some(json!({ "email": erased.email })),
    );
    assert_eq!(done.status(), 200);
    assert!(roles_of(pool, erased.id).await.is_none());
    // Their own sessions are theirs to end.
    assert_eq!(
        status(
            base,
            &cookie,
            "POST",
            &format!("/users/{}/sessions/revoke", manager.id),
            None
        ),
        200
    );
}

#[tokio::test]
async fn an_administrator_is_bounded_by_nobody() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    seed_roles(pool).await;

    let other_admin = common::seed_user(pool, Role::Admin).await;
    let editor = common::seed_user(pool, Role::Editor).await;
    let manager = seed_with_role(pool, "user-manager").await;
    for (who, target) in [
        ("another administrator", &other_admin),
        ("an editor", &editor),
        ("a user manager", &manager),
    ] {
        let server = TestServer::start(common::BIN, &db);
        let base = server.base();
        let cookie = common::login_cookie(base, &admin.email, &admin.password);
        for (what, method, path, body) in actions(target.id) {
            let body = match what {
                "edit" => Some(json!({ "display_name": "Renamed" })),
                _ => body,
            };
            let got = status(base, &cookie, method, &path, body);
            let expected = match what {
                "reset link" => 400,
                "edit" | "sessions" => 200,
                _ => 204,
            };
            assert_eq!(got, expected, "{what} on {who}");
        }
    }
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    for role in ["admin", "editor", "drafter", "user-manager"] {
        let created = call(
            base,
            &cookie,
            "POST",
            "/users",
            Some(json!({
                "email": format!("made-{role}@example.com"),
                "password": "a-long-password",
                "role": role,
            })),
        );
        assert_eq!(created.status(), 201, "creating a {role}");
        let id = created.into_json::<Value>().unwrap()["id"]
            .as_i64()
            .unwrap();
        assert_eq!(
            status(
                base,
                &cookie,
                "PUT",
                &format!("/users/{id}/role"),
                Some(json!({ "role": "admin" }))
            ),
            204
        );
    }
}

#[tokio::test]
async fn an_import_is_refused_to_anyone_short_of_a_full_administrator() {
    let db = TestDb::new().await;
    let pool = db.pool();
    common::seed_user(pool, Role::Admin).await;
    role(
        pool,
        "site-keeper",
        &["view_admin", "manage_options", "manage_users"],
    )
    .await;
    let importer = seed_with_role(pool, "site-keeper").await;
    // Holds manage_options, which the route asks for, and nothing else.
    role(pool, "settings-only", &["view_admin", "manage_options"]).await;
    let settings_only = seed_with_role(pool, "settings-only").await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    let archive = |tag: &str| {
        let user = |id: i64, role: &str| {
            json!({
                "id": id, "email": format!("{tag}-{role}@example.com"),
                "username": format!("{tag}-{role}"), "display_name": role,
                "role": role, "bio": "",
            })
        };
        json!({
            "version": 1, "exported_at": "", "site": {}, "terms": [],
            "posts": [{
                "id": 1, "type": "post", "status": "published",
                "title": format!("{tag} post"), "slug": format!("{tag}-post"),
                "content": { "schema_version": 1, "blocks": [] },
                "author_id": 1,
            }],
            "comments": [], "media": [], "menus": [],
            "users": [user(1, "admin"), user(2, "editor"), user(3, "subscriber")],
        })
    };

    // An archive brings in accounts and content wholesale, which neither
    // caller could do by hand: the whole request is refused.
    for (tag, who) in [("keeper", &importer), ("settings", &settings_only)] {
        let cookie = common::login_cookie(base, &who.email, &who.password);
        let (status, message) = refusal(call(base, &cookie, "POST", "/import", Some(archive(tag))));
        assert_eq!(status, 403, "{tag}: {message}");
        assert!(
            message.contains("only a full administrator can import a site archive"),
            "{message}"
        );
        let arrived: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email LIKE $1")
            .bind(format!("{tag}-%@example.com"))
            .fetch_one(pool)
            .await
            .expect("count users");
        assert_eq!(arrived, 0, "{tag}");
        let posts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE slug = $1")
            .bind(format!("{tag}-post"))
            .fetch_one(pool)
            .await
            .expect("count posts");
        assert_eq!(posts, 0, "{tag}");
    }
}

#[tokio::test]
async fn a_role_beyond_the_caller_is_not_edited_down_or_deleted() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    seed_roles(pool).await;
    role(pool, "spare", &["view_admin", "edit_posts"]).await;
    let manager = seed_with_role(pool, "user-manager").await;
    let drafter = seed_with_role(pool, "drafter").await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &manager.email, &manager.password);
    let caps_of = |slug: &'static str| async move {
        sqlx::query_as::<_, (String, Vec<String>)>(
            "SELECT name, capabilities FROM roles WHERE slug = $1",
        )
        .bind(slug)
        .fetch_optional(pool)
        .await
        .expect("role")
    };

    // The takeover in two steps: strip the role its holder has, so the
    // holder falls within reach; then take the account. Refused at the
    // first step, in every form it could take.
    for body in [
        json!({ "capabilities": [] }),
        json!({ "capabilities": ["view_admin"] }),
        json!({ "name": "Mine now" }),
        json!({ "slug": "mine-now" }),
        json!({}),
    ] {
        let (status, message) = refusal(call(
            base,
            &cookie,
            "PATCH",
            "/roles/drafter",
            Some(body.clone()),
        ));
        assert_eq!(status, 403, "PATCH drafter {body}");
        assert!(message.contains("edit_posts"), "{body}: {message}");
    }
    assert_eq!(
        caps_of("drafter").await,
        Some((
            "drafter".to_owned(),
            vec!["view_admin".to_owned(), "edit_posts".to_owned()]
        ))
    );
    // So the second step still is.
    assert_eq!(
        status(
            base,
            &cookie,
            "PATCH",
            &format!("/users/{}", drafter.id),
            Some(json!({ "email": "taken-over@example.com" }))
        ),
        403
    );
    // Deleting one nobody holds is refused as well, before anything else
    // is said about it.
    assert_eq!(status(base, &cookie, "DELETE", "/roles/spare", None), 403);
    assert_eq!(status(base, &cookie, "DELETE", "/roles/drafter", None), 403);
    assert!(caps_of("spare").await.is_some());

    // A role within their own capabilities is theirs to edit and delete.
    let edited = call(
        base,
        &cookie,
        "PATCH",
        "/roles/greeter",
        Some(json!({ "name": "Greeter", "capabilities": [] })),
    );
    assert_eq!(edited.status(), 200);
    assert_eq!(status(base, &cookie, "DELETE", "/roles/greeter", None), 204);

    // An administrator edits and deletes any of them.
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    assert_eq!(
        status(
            base,
            &admin_cookie,
            "PATCH",
            "/roles/drafter",
            Some(json!({ "capabilities": ["view_admin"] }))
        ),
        200
    );
    assert_eq!(
        status(base, &admin_cookie, "DELETE", "/roles/spare", None),
        204
    );
}

#[tokio::test]
async fn personal_data_is_exported_only_for_an_account_within_reach() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let subscriber = common::seed_user(pool, Role::Subscriber).await;
    seed_roles(pool).await;
    let manager = seed_with_role(pool, "user-manager").await;
    let drafter = seed_with_role(pool, "drafter").await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &manager.email, &manager.password);
    let export = |cookie: &str, email: &str| {
        call(
            base,
            cookie,
            "GET",
            &format!("/privacy/export?email={email}"),
            None,
        )
    };

    for beyond in [&admin, &drafter] {
        let (status, message) = refusal(export(&cookie, &beyond.email));
        assert_eq!(status, 403);
        assert!(
            message.contains("this account holds capabilities you do not have"),
            "{message}"
        );
    }
    let mine = export(&cookie, &subscriber.email);
    assert_eq!(mine.status(), 200);
    let mine: Value = mine.into_json().unwrap();
    assert_eq!(mine["account"]["role"], "subscriber");
    assert_eq!(mine["account"]["custom_role"], Value::Null);
    assert_eq!(mine["account"]["role_name"], "Subscriber");
    // An address with no account has only what visitors left under it.
    assert_eq!(export(&cookie, "nobody@example.com").status(), 200);

    // An administrator reads anyone's, and sees the custom role.
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    let theirs: Value = export(&admin_cookie, &drafter.email).into_json().unwrap();
    assert_eq!(theirs["account"]["role"], "subscriber");
    assert_eq!(theirs["account"]["custom_role"], "drafter");
    assert_eq!(theirs["account"]["role_name"], "drafter");
}

/// The hash a reset token is stored under (sha256 hex).
fn token_hash(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(raw.as_bytes())
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn redeem(base: &str, token: &str, password: &str) -> u16 {
    http(
        ureq::post(&format!("{base}/api/v1/auth/reset"))
            .send_json(json!({ "token": token, "password": password })),
    )
    .status()
}

#[tokio::test]
async fn a_reset_link_issued_before_a_promotion_is_dead_after_it() {
    let db = TestDb::new().await;
    let pool = db.pool();
    seed_roles(pool).await;
    let admin = common::seed_user(pool, Role::Admin).await;
    let to_editor = common::seed_user(pool, Role::Subscriber).await;
    let to_custom = common::seed_user(pool, Role::Subscriber).await;
    let greeter = seed_with_role(pool, "greeter").await;
    let untouched = common::seed_user(pool, Role::Subscriber).await;
    let users = UsersRepo::new(pool.clone());
    // A link for each account while it holds next to nothing: mailed to
    // its owner, or asked for by a user manager who could reach it then.
    for (user, token) in [
        (&to_editor, "token-editor"),
        (&to_custom, "token-custom"),
        (&greeter, "token-greeter"),
        (&untouched, "token-untouched"),
    ] {
        users
            .create_reset_token(user.id, &token_hash(token))
            .await
            .expect("token");
    }
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    // Promoted: to a built-in role, to a custom role, and by widening the
    // custom role the account already holds.
    let role = |id: i64, role: &str| {
        status(
            base,
            &cookie,
            "PUT",
            &format!("/users/{id}/role"),
            Some(json!({ "role": role })),
        )
    };
    assert_eq!(role(to_editor.id, "editor"), 204);
    assert_eq!(role(to_custom.id, "user-manager"), 204);
    let widened = status(
        base,
        &cookie,
        "PATCH",
        "/roles/greeter",
        Some(json!({ "capabilities": ["view_admin", "manage_users", "manage_options"] })),
    );
    assert_eq!(widened, 200);

    for (user, token) in [
        (&to_editor, "token-editor"),
        (&to_custom, "token-custom"),
        (&greeter, "token-greeter"),
    ] {
        assert_eq!(redeem(base, token, "taken-over-1"), 404, "{token}");
        // The password is the one the account had.
        common::login_cookie(base, &user.email, &user.password);
    }
    // An account whose role did not change keeps its link.
    assert_eq!(redeem(base, "token-untouched", "my-new-password-1"), 200);
    common::login_cookie(base, &untouched.email, "my-new-password-1");
}

#[tokio::test]
async fn creating_an_account_is_audited() {
    let db = TestDb::new().await;
    let pool = db.pool();
    seed_roles(pool).await;
    let admin = common::seed_user(pool, Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    let create = |name: &str, role: &str| {
        let created = call(
            base,
            &cookie,
            "POST",
            "/users",
            Some(json!({
                "email": format!("{name}@example.com"), "username": name,
                "display_name": name, "password": "a-long-password-1", "role": role,
            })),
        );
        assert_eq!(created.status(), 201, "{name}");
    };
    create("built-in", "author");
    create("custom", "drafter");
    // Refused: nothing is created, and nothing is recorded as created.
    let refused = status(
        base,
        &cookie,
        "POST",
        "/users",
        Some(json!({
            "email": "nobody@example.com", "username": "nobody",
            "display_name": "Nobody", "password": "a-long-password-1", "role": "no-such-role",
        })),
    );
    assert_eq!(refused, 400);

    // The audit write is spawned, not awaited.
    let mut rows: Vec<(Option<i64>, String, Value)> = Vec::new();
    for _ in 0..50 {
        rows = sqlx::query_as(
            "SELECT actor_id, target, detail FROM audit_log WHERE action = 'user.create'",
        )
        .fetch_all(pool)
        .await
        .expect("audit rows");
        if rows.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(rows.len(), 2, "{rows:?}");
    // Who created whom, with which role.
    for (name, detail) in [
        (
            "built-in",
            json!({ "role": "author", "custom_role": null, "invited": false }),
        ),
        (
            "custom",
            json!({ "role": "subscriber", "custom_role": "drafter", "invited": false }),
        ),
    ] {
        let id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
            .bind(name)
            .fetch_one(pool)
            .await
            .expect("created user");
        let row = rows
            .iter()
            .find(|(_, target, _)| *target == format!("user:{id}"))
            .unwrap_or_else(|| panic!("no audit row for {name}: {rows:?}"));
        assert_eq!(row.0, Some(admin.id), "{name}");
        assert_eq!(row.2, detail, "{name}");
    }
}
