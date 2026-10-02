//! Custom roles over HTTP: managing them, assigning them, and what a user
//! who holds one can then do.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::ApiKeysRepo;
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn json_body(r: ureq::Response) -> Value {
    r.into_json().expect("a JSON body")
}

/// How a request is authenticated.
#[derive(Clone, Copy)]
enum As<'a> {
    Cookie(&'a str),
    Key(&'a str),
    Nobody,
}

fn call(
    base: &str,
    caller: As<'_>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> ureq::Response {
    let mut request = ureq::request(method, &format!("{base}/api/v1{path}"));
    request = match caller {
        As::Cookie(cookie) => request.set("cookie", cookie),
        As::Key(raw) => request.set("authorization", &format!("Bearer {raw}")),
        As::Nobody => request,
    };
    http(match body {
        Some(body) => request.send_json(body),
        None => request.call(),
    })
}

/// The status and the error message, for a refusal.
fn refusal(r: ureq::Response) -> (u16, String) {
    let status = r.status();
    let body = json_body(r);
    (
        status,
        body["message"].as_str().unwrap_or_default().to_owned(),
    )
}

fn create_role(base: &str, cookie: &str, slug: &str, name: &str, caps: &[&str]) -> Value {
    let created = call(
        base,
        As::Cookie(cookie),
        "POST",
        "/roles",
        Some(json!({ "slug": slug, "name": name, "capabilities": caps })),
    );
    assert_eq!(created.status(), 201, "create role {slug}");
    json_body(created)
}

fn assign(base: &str, cookie: &str, user: i64, role: &str) -> ureq::Response {
    call(
        base,
        As::Cookie(cookie),
        "PUT",
        &format!("/users/{user}/role"),
        Some(json!({ "role": role })),
    )
}

fn create_post(base: &str, caller: As<'_>, title: &str, status: &str) -> ureq::Response {
    call(
        base,
        caller,
        "POST",
        "/posts",
        Some(json!({
            "title": title,
            "status": status,
            "content": { "schema_version": 1, "blocks": [] },
        })),
    )
}

/// A PNG by signature: the upload path sniffs magic bytes and does not
/// decode the image.
fn upload(base: &str, cookie: &str) -> ureq::Response {
    let boundary = "----vyasaroles";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    let mut png = vec![0u8; 256];
    png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    body.extend_from_slice(&png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    http(
        ureq::post(&format!("{base}/api/v1/media"))
            .set("cookie", cookie)
            .set(
                "content-type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body),
    )
}

async fn custom_role_of(pool: &PgPool, id: i64) -> (String, Option<String>) {
    sqlx::query_as("SELECT role, custom_role FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("user roles")
}

/// A server whose uploads land in a scratch directory.
fn server_with_media(db: &TestDb) -> (TestServer, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("vyasa-custom-roles-{}", db.name()));
    std::fs::create_dir_all(dir.join("media")).expect("scratch");
    let server = TestServer::builder(common::BIN, db)
        .env("VYASA_MEDIA_DIR", dir.join("media").display().to_string())
        .start();
    (server, dir)
}

#[tokio::test]
async fn roles_are_listed_created_edited_and_deleted() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    common::seed_user(pool, Role::Author).await;
    common::seed_user(pool, Role::Author).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    let me = As::Cookie(&cookie);

    // The five built-in roles, in their fixed order, with who holds them.
    let listed = json_body(call(base, me, "GET", "/roles", None));
    let rows = listed.as_array().expect("an array");
    let slugs: Vec<&str> = rows.iter().map(|r| r["slug"].as_str().unwrap()).collect();
    assert_eq!(
        slugs,
        ["admin", "editor", "author", "contributor", "subscriber"]
    );
    assert!(rows.iter().all(|r| r["built_in"] == true));
    assert_eq!(rows[0]["name"], "Administrator");
    assert_eq!(rows[0]["users"], 1);
    assert_eq!(rows[2]["users"], 2);
    assert_eq!(rows[4]["users"], 0);
    assert_eq!(rows[4]["capabilities"], json!(["view_admin"]));
    assert_eq!(rows[0]["capabilities"].as_array().unwrap().len(), 12);
    assert!(rows[1]["description"]
        .as_str()
        .is_some_and(|d| !d.is_empty()));

    // Create: the role comes back in the listing's shape.
    let created = call(
        base,
        me,
        "POST",
        "/roles",
        Some(json!({
            "slug": "zz-moderator",
            "name": "  Comment moderator ",
            "description": " Keeps the comments tidy ",
            "capabilities": ["view_admin", "moderate_comments", "view_admin"],
        })),
    );
    assert_eq!(created.status(), 201);
    assert_eq!(
        json_body(created),
        json!({
            "slug": "zz-moderator",
            "name": "Comment moderator",
            "description": "Keeps the comments tidy",
            "capabilities": ["view_admin", "moderate_comments"],
            "built_in": false,
            "users": 0,
        })
    );
    // The description is optional, and a role may hold nothing.
    let nothing = create_role(base, &cookie, "aa-nothing", "Zero", &[]);
    assert_eq!(nothing["description"], "");
    assert_eq!(nothing["capabilities"], json!([]));

    // Built-in first, then custom by name (not by slug).
    let listed = json_body(call(base, me, "GET", "/roles", None));
    let slugs: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["slug"].as_str().unwrap())
        .collect();
    assert_eq!(
        slugs,
        [
            "admin",
            "editor",
            "author",
            "contributor",
            "subscriber",
            "zz-moderator",
            "aa-nothing"
        ]
    );

    // Refusals on create.
    for (body, status, code) in [
        (
            json!({ "slug": "Bad Slug", "name": "x", "capabilities": [] }),
            400,
            "validation_failed",
        ),
        (
            json!({ "slug": "editor", "name": "x", "capabilities": [] }),
            400,
            "validation_failed",
        ),
        (
            json!({ "slug": "fine", "name": "", "capabilities": [] }),
            400,
            "validation_failed",
        ),
        (
            json!({ "slug": "fine", "name": "x", "capabilities": ["fly"] }),
            400,
            "validation_failed",
        ),
        (
            json!({ "slug": "zz-moderator", "name": "x", "capabilities": [] }),
            409,
            "conflict",
        ),
    ] {
        let refused = call(base, me, "POST", "/roles", Some(body.clone()));
        assert_eq!(refused.status(), status, "{body}");
        assert_eq!(json_body(refused)["code"], code, "{body}");
    }

    // Edit: any subset of the fields; the rest stay.
    let edited = call(
        base,
        me,
        "PATCH",
        "/roles/zz-moderator",
        Some(json!({ "capabilities": ["view_admin"] })),
    );
    assert_eq!(edited.status(), 200);
    let edited = json_body(edited);
    assert_eq!(edited["capabilities"], json!(["view_admin"]));
    assert_eq!(edited["name"], "Comment moderator");
    let renamed = call(
        base,
        me,
        "PATCH",
        "/roles/zz-moderator",
        Some(json!({ "slug": "moderator", "name": "Moderator", "description": "" })),
    );
    assert_eq!(renamed.status(), 200);
    let renamed = json_body(renamed);
    assert_eq!(renamed["slug"], "moderator");
    assert_eq!(renamed["name"], "Moderator");
    assert_eq!(renamed["description"], "");
    assert_eq!(renamed["capabilities"], json!(["view_admin"]));
    // Refusals on edit.
    let onto = call(
        base,
        me,
        "PATCH",
        "/roles/moderator",
        Some(json!({ "slug": "aa-nothing" })),
    );
    assert_eq!(onto.status(), 409);
    let unknown_cap = call(
        base,
        me,
        "PATCH",
        "/roles/moderator",
        Some(json!({ "capabilities": ["fly"] })),
    );
    assert_eq!(unknown_cap.status(), 400);
    let missing = call(
        base,
        me,
        "PATCH",
        "/roles/zz-moderator",
        Some(json!({ "name": "Gone" })),
    );
    assert_eq!(missing.status(), 404);
    assert_eq!(json_body(missing)["code"], "role_not_found");

    // Delete.
    assert_eq!(
        call(base, me, "DELETE", "/roles/moderator", None).status(),
        204
    );
    assert_eq!(
        call(base, me, "DELETE", "/roles/moderator", None).status(),
        404
    );
}

#[tokio::test]
async fn roles_take_manage_users_and_built_in_roles_are_fixed() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let editor = common::seed_user(pool, Role::Editor).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    let editor_cookie = common::login_cookie(base, &editor.email, &editor.password);
    create_role(base, &admin_cookie, "helper", "Helper", &["view_admin"]);

    let role = json!({ "slug": "mine", "name": "Mine", "capabilities": [] });
    for (method, path, body) in [
        ("GET", "/roles", None),
        ("POST", "/roles", Some(role)),
        ("PATCH", "/roles/helper", Some(json!({ "name": "Mine" }))),
        ("DELETE", "/roles/helper", None),
    ] {
        let as_editor = call(base, As::Cookie(&editor_cookie), method, path, body.clone());
        assert_eq!(as_editor.status(), 403, "{method} {path} as an editor");
        let anonymous = call(base, As::Nobody, method, path, body);
        assert_eq!(anonymous.status(), 401, "{method} {path} signed out");
    }

    // Not even an administrator edits or deletes a built-in role.
    for slug in ["admin", "editor", "author", "contributor", "subscriber"] {
        let (status, message) = refusal(call(
            base,
            As::Cookie(&admin_cookie),
            "PATCH",
            &format!("/roles/{slug}"),
            Some(json!({ "name": "Renamed", "capabilities": [] })),
        ));
        assert_eq!(status, 400, "PATCH {slug}");
        assert!(message.contains("built-in"), "{message}");
        let (status, message) = refusal(call(
            base,
            As::Cookie(&admin_cookie),
            "DELETE",
            &format!("/roles/{slug}"),
            None,
        ));
        assert_eq!(status, 400, "DELETE {slug}");
        assert!(message.contains("built-in"), "{message}");
    }
}

#[tokio::test]
async fn a_custom_role_grants_exactly_its_capabilities_and_an_edit_applies_at_once() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let writer = common::seed_user(pool, Role::Editor).await;
    let (server, dir) = server_with_media(&db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    // Signed in before the role exists: the same session is used throughout.
    let writer_cookie = common::login_cookie(base, &writer.email, &writer.password);
    let me = As::Cookie(&writer_cookie);

    create_role(
        base,
        &admin_cookie,
        "drafter",
        "Drafter",
        &["edit_posts", "upload_media"],
    );
    assert_eq!(
        assign(base, &admin_cookie, writer.id, "drafter").status(),
        204
    );
    assert_eq!(
        custom_role_of(pool, writer.id).await,
        ("subscriber".to_owned(), Some("drafter".to_owned()))
    );

    // What the account now says about itself.
    let profile = json_body(call(base, me, "GET", "/auth/me", None));
    assert_eq!(profile["role"], "subscriber");
    assert_eq!(profile["custom_role"], "drafter");
    assert_eq!(profile["role_name"], "Drafter");
    let caps = json_body(call(base, me, "GET", "/auth/me/caps", None));
    assert_eq!(caps, json!(["edit_posts", "upload_media"]));
    // A built-in role's user carries its label and no custom role.
    let admin_profile = json_body(call(
        base,
        As::Cookie(&admin_cookie),
        "GET",
        "/auth/me",
        None,
    ));
    assert_eq!(admin_profile["custom_role"], Value::Null);
    assert_eq!(admin_profile["role_name"], "Administrator");
    // The users list and a single user carry the same fields.
    let users = json_body(call(base, As::Cookie(&admin_cookie), "GET", "/users", None));
    let row = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == writer.id)
        .expect("the writer");
    assert_eq!(row["custom_role"], "drafter");
    assert_eq!(row["role_name"], "Drafter");
    let role = json_body(call(base, As::Cookie(&admin_cookie), "GET", "/roles", None));
    let drafter = role
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["slug"] == "drafter")
        .expect("the role")
        .clone();
    assert_eq!(drafter["users"], 1);
    // A custom-role user is not counted under the base role beneath it.
    assert_eq!(role[4]["slug"], "subscriber");
    assert_eq!(role[4]["users"], 0);

    // Held: drafts and uploads.
    assert_eq!(create_post(base, me, "A draft", "draft").status(), 201);
    assert_eq!(upload(base, &writer_cookie).status(), 201);
    // Not held: the editor's old capabilities are gone, and nothing else
    // came with the role.
    assert_eq!(call(base, me, "GET", "/users", None).status(), 403);
    assert_eq!(call(base, me, "GET", "/roles", None).status(), 403);
    assert_eq!(call(base, me, "GET", "/comments", None).status(), 403);
    assert_eq!(create_post(base, me, "Live", "published").status(), 403);

    // The role gains a capability: the very next request of a session
    // that was open all along has it.
    let widened = call(
        base,
        As::Cookie(&admin_cookie),
        "PATCH",
        "/roles/drafter",
        Some(json!({ "capabilities": ["edit_posts", "upload_media", "publish_posts"] })),
    );
    assert_eq!(widened.status(), 200);
    assert_eq!(create_post(base, me, "Live", "published").status(), 201);
    // And loses one.
    let narrowed = call(
        base,
        As::Cookie(&admin_cookie),
        "PATCH",
        "/roles/drafter",
        Some(json!({ "capabilities": ["publish_posts"] })),
    );
    assert_eq!(narrowed.status(), 200);
    assert_eq!(create_post(base, me, "Another", "draft").status(), 403);
    assert_eq!(upload(base, &writer_cookie).status(), 403);

    // A rename keeps the assignment.
    let renamed = call(
        base,
        As::Cookie(&admin_cookie),
        "PATCH",
        "/roles/drafter",
        Some(json!({ "slug": "publisher", "name": "Publisher" })),
    );
    assert_eq!(renamed.status(), 200);
    let profile = json_body(call(base, me, "GET", "/auth/me", None));
    assert_eq!(profile["custom_role"], "publisher");
    assert_eq!(profile["role_name"], "Publisher");

    // A built-in role replaces the custom one.
    assert_eq!(
        assign(base, &admin_cookie, writer.id, "author").status(),
        204
    );
    assert_eq!(
        custom_role_of(pool, writer.id).await,
        ("author".to_owned(), None)
    );
    let profile = json_body(call(base, me, "GET", "/auth/me", None));
    assert_eq!(profile["custom_role"], Value::Null);
    assert_eq!(profile["role_name"], "Author");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_role_beyond_the_caller_is_not_created_edited_or_assigned() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let manager = common::seed_user(pool, Role::Subscriber).await;
    let subscriber = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    create_role(
        base,
        &admin_cookie,
        "user-manager",
        "User manager",
        &["view_admin", "manage_users"],
    );
    create_role(base, &admin_cookie, "drafter", "Drafter", &["edit_posts"]);
    assert_eq!(
        assign(base, &admin_cookie, manager.id, "user-manager").status(),
        204
    );
    let cookie = common::login_cookie(base, &manager.email, &manager.password);
    let me = As::Cookie(&cookie);

    // Create.
    let (status, message) = refusal(call(
        base,
        me,
        "POST",
        "/roles",
        Some(json!({
            "slug": "sneaky",
            "name": "Sneaky",
            "capabilities": ["view_admin", "manage_options", "edit_posts"],
        })),
    ));
    assert_eq!(status, 403);
    assert!(
        message.contains("manage_options") && message.contains("edit_posts"),
        "names what is missing: {message}"
    );
    // Edit: widening a role they could otherwise manage, and touching one
    // that already exceeds them.
    create_role(base, &cookie, "greeter", "Greeter", &["view_admin"]);
    let widen = call(
        base,
        me,
        "PATCH",
        "/roles/greeter",
        Some(json!({ "capabilities": ["view_admin", "manage_plugins"] })),
    );
    assert_eq!(widen.status(), 403);
    let rename = call(
        base,
        me,
        "PATCH",
        "/roles/drafter",
        Some(json!({ "name": "Mine now" })),
    );
    assert_eq!(rename.status(), 403);
    // Assign.
    let (status, message) = refusal(assign(base, &cookie, subscriber.id, "drafter"));
    assert_eq!(status, 403);
    assert!(message.contains("edit_posts"), "{message}");
    assert_eq!(
        custom_role_of(pool, subscriber.id).await,
        ("subscriber".to_owned(), None)
    );
    let roles = json_body(call(base, me, "GET", "/roles", None));
    let of = |slug: &str| {
        roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["slug"] == slug)
            .cloned()
    };
    assert!(of("sneaky").is_none());
    assert_eq!(
        of("greeter").unwrap()["capabilities"],
        json!(["view_admin"])
    );
    assert_eq!(of("drafter").unwrap()["name"], "Drafter");

    // Within their own capabilities all three work.
    assert_eq!(
        assign(base, &cookie, subscriber.id, "greeter").status(),
        204
    );
    assert_eq!(
        assign(base, &cookie, subscriber.id, "user-manager").status(),
        204
    );
    // An administrator is bounded by nothing.
    assert_eq!(
        assign(base, &admin_cookie, subscriber.id, "drafter").status(),
        204
    );
}

#[tokio::test]
async fn a_role_in_use_is_not_deleted_and_the_last_admin_keeps_their_role() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let one = common::seed_user(pool, Role::Author).await;
    let two = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    let me = As::Cookie(&cookie);
    create_role(base, &cookie, "helper", "Helper", &["view_admin"]);

    // Not a role at all.
    let (status, message) = refusal(assign(base, &cookie, one.id, "no-such-role"));
    assert_eq!(status, 400);
    assert!(message.contains("no-such-role"), "{message}");
    // Not a user.
    assert_eq!(assign(base, &cookie, 9_999_999, "helper").status(), 404);

    assert_eq!(assign(base, &cookie, one.id, "helper").status(), 204);
    assert_eq!(assign(base, &cookie, two.id, "helper").status(), 204);
    let refused = call(base, me, "DELETE", "/roles/helper", None);
    assert_eq!(refused.status(), 409);
    let body = json_body(refused);
    assert_eq!(body["code"], "conflict");
    assert!(
        body["message"].as_str().unwrap().contains("2 user(s)"),
        "{body}"
    );

    // The only administrator cannot be moved onto a custom role: their
    // built-in role would become subscriber.
    let (status, message) = refusal(assign(base, &cookie, admin.id, "helper"));
    assert_eq!(status, 400);
    assert!(message.contains("last remaining admin"), "{message}");
    assert_eq!(
        custom_role_of(pool, admin.id).await,
        ("admin".to_owned(), None)
    );

    // Freed, the role goes.
    assert_eq!(assign(base, &cookie, one.id, "author").status(), 204);
    assert_eq!(assign(base, &cookie, two.id, "subscriber").status(), 204);
    assert_eq!(
        call(base, me, "DELETE", "/roles/helper", None).status(),
        204
    );
}

/// Hashes a raw key the way the server does (sha256 hex).
fn key_hash(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(raw.as_bytes())
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[tokio::test]
async fn an_api_key_is_bounded_by_its_owners_custom_role() {
    const KEY: &str = "vy_custom_role_key_00000000001";
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let owner = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    create_role(
        base,
        &cookie,
        "drafter",
        "Drafter",
        &["edit_posts", "upload_media"],
    );
    assert_eq!(assign(base, &cookie, owner.id, "drafter").status(), 204);
    // The key claims more than the role holds.
    ApiKeysRepo::new(pool.clone())
        .insert(
            97_201,
            owner.id,
            "drafts",
            &key_hash(KEY),
            &json!(["edit_posts", "publish_posts", "manage_users"]),
        )
        .await
        .expect("key");
    let key = As::Key(KEY);

    assert_eq!(create_post(base, key, "By key", "draft").status(), 201);
    assert_eq!(create_post(base, key, "Live", "published").status(), 403);
    assert_eq!(call(base, key, "GET", "/users", None).status(), 403);

    // The role loses the capability, and the key with it.
    let narrowed = call(
        base,
        As::Cookie(&cookie),
        "PATCH",
        "/roles/drafter",
        Some(json!({ "capabilities": ["upload_media"] })),
    );
    assert_eq!(narrowed.status(), 200);
    assert_eq!(create_post(base, key, "Again", "draft").status(), 403);
    // And gains one the key was granted all along.
    let widened = call(
        base,
        As::Cookie(&cookie),
        "PATCH",
        "/roles/drafter",
        Some(json!({ "capabilities": ["edit_posts", "publish_posts"] })),
    );
    assert_eq!(widened.status(), 200);
    assert_eq!(create_post(base, key, "Live", "published").status(), 201);
}

#[tokio::test]
async fn an_archive_carries_custom_roles_and_who_holds_them() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let holder = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    let me = As::Cookie(&cookie);
    create_role(
        base,
        &cookie,
        "drafter",
        "Drafter",
        &["edit_posts", "upload_media"],
    );
    assert_eq!(assign(base, &cookie, holder.id, "drafter").status(), 204);

    let mut archive = json_body(call(base, me, "GET", "/export?format=json", None));
    assert_eq!(
        archive["roles"],
        json!([{
            "slug": "drafter",
            "name": "Drafter",
            "description": "",
            "capabilities": ["edit_posts", "upload_media"],
        }])
    );
    let exported = archive["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == holder.id)
        .expect("the holder")
        .clone();
    assert_eq!(exported["role"], "subscriber");
    assert_eq!(exported["custom_role"], "drafter");

    // Onto a site that has neither the role nor the user.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(holder.id)
        .execute(pool)
        .await
        .expect("delete user");
    sqlx::query("DELETE FROM roles")
        .execute(pool)
        .await
        .expect("delete roles");
    // A user whose role the archive does not define arrives without it.
    archive["users"].as_array_mut().unwrap().push(json!({
        "id": 5, "email": "stray@example.com", "username": "stray",
        "display_name": "Stray", "role": "subscriber", "bio": "",
        "custom_role": "not-in-the-archive",
    }));
    let report = json_body(call(base, me, "POST", "/import", Some(archive.clone())));
    assert_eq!(report["roles"], 1, "{report}");
    assert_eq!(report["users"], 2, "{report}");
    let warnings = report["warnings"].to_string();
    assert!(warnings.contains("not-in-the-archive"), "{warnings}");
    let restored: (String, Option<String>) =
        sqlx::query_as("SELECT role, custom_role FROM users WHERE email = $1")
            .bind(&holder.email)
            .fetch_one(pool)
            .await
            .expect("the imported holder");
    assert_eq!(
        restored,
        ("subscriber".to_owned(), Some("drafter".to_owned()))
    );
    let stray: (String, Option<String>) =
        sqlx::query_as("SELECT role, custom_role FROM users WHERE email = 'stray@example.com'")
            .fetch_one(pool)
            .await
            .expect("the stray");
    assert_eq!(stray, ("subscriber".to_owned(), None));
    let caps: Vec<String> =
        sqlx::query_scalar("SELECT capabilities FROM roles WHERE slug = 'drafter'")
            .fetch_one(pool)
            .await
            .expect("the imported role");
    assert_eq!(caps, ["edit_posts", "upload_media"]);

    // A second import adds nothing.
    let again = json_body(call(base, me, "POST", "/import", Some(archive)));
    assert_eq!(again["roles"], 0, "{again}");
    assert_eq!(again["users"], 0, "{again}");

    // An archive from before custom roles still loads.
    let old = json!({
        "version": 1, "exported_at": "", "site": {}, "terms": [], "posts": [],
        "comments": [], "media": [], "menus": [],
        "users": [{
            "id": 6, "email": "old@example.com", "username": "old",
            "display_name": "Old", "role": "author", "bio": "",
        }],
    });
    let report = json_body(call(base, me, "POST", "/import", Some(old)));
    assert_eq!(report["users"], 1, "{report}");
}

#[tokio::test]
async fn an_import_creates_no_role_for_an_importer_short_of_a_full_administrator() {
    const ROLES: &str = "SELECT COUNT(*) FROM roles WHERE slug IN ('greeter', 'themer')";
    const USERS: &str =
        "SELECT COUNT(*) FROM users WHERE username IN ('within', 'beyond', 'undefined')";
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let importer = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    // Holds what the route asks for (manage_options) and manages users,
    // but little else.
    create_role(
        base,
        &admin_cookie,
        "site-keeper",
        "Site keeper",
        &["view_admin", "manage_options", "manage_users"],
    );
    create_role(base, &admin_cookie, "drafter", "Drafter", &["edit_posts"]);
    assert_eq!(
        assign(base, &admin_cookie, importer.id, "site-keeper").status(),
        204
    );
    let cookie = common::login_cookie(base, &importer.email, &importer.password);

    let user = |id: i64, name: &str, custom: &str| {
        json!({
            "id": id, "email": format!("{name}@example.com"), "username": name,
            "display_name": name, "role": "subscriber", "bio": "", "custom_role": custom,
        })
    };
    let archive = json!({
        "version": 1, "exported_at": "", "site": {}, "terms": [], "posts": [],
        "comments": [], "media": [], "menus": [],
        "roles": [
            { "slug": "greeter", "name": "Greeter", "capabilities": ["view_admin"] },
            { "slug": "themer", "name": "Themer", "capabilities": ["manage_themes"] },
        ],
        "users": [
            user(1, "within", "greeter"),
            user(2, "beyond", "drafter"),
            user(3, "undefined", "themer"),
        ],
    });
    // Refused whole: not even the role and the user within the importer's
    // own capabilities arrive.
    let (status, message) = refusal(call(
        base,
        As::Cookie(&cookie),
        "POST",
        "/import",
        Some(archive.clone()),
    ));
    assert_eq!(status, 403, "{message}");
    assert!(
        message.contains("only a full administrator can import a site archive"),
        "{message}"
    );
    let count = |sql: &'static str| async move {
        sqlx::query_scalar::<_, i64>(sql)
            .fetch_one(pool)
            .await
            .expect("count")
    };
    assert_eq!(count(ROLES).await, 0);
    assert_eq!(count(USERS).await, 0);

    // The administrator loads the same archive, roles and holders included.
    let report = json_body(call(
        base,
        As::Cookie(&admin_cookie),
        "POST",
        "/import",
        Some(archive),
    ));
    assert_eq!(report["roles"], 2, "{report}");
    assert_eq!(report["users"], 3, "{report}");
    assert_eq!(count(ROLES).await, 2);
    let held: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT username, custom_role FROM users \
         WHERE username IN ('within', 'beyond', 'undefined') ORDER BY username",
    )
    .fetch_all(pool)
    .await
    .expect("imported users");
    assert_eq!(
        held,
        [
            ("beyond".to_owned(), Some("drafter".to_owned())),
            ("undefined".to_owned(), Some("themer".to_owned())),
            ("within".to_owned(), Some("greeter".to_owned())),
        ]
    );
}

/// One GraphQL query, as `data`.
fn graphql(base: &str, cookie: &str, query: &str) -> Value {
    let reply = json_body(http(
        ureq::post(&format!("{base}/api/graphql"))
            .set("cookie", cookie)
            .send_json(json!({ "query": query })),
    ));
    assert!(reply.get("errors").is_none(), "{reply}");
    reply["data"].clone()
}

#[tokio::test]
async fn a_custom_role_is_what_a_user_is_shown_as_everywhere() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let holder = common::seed_user(pool, Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);
    create_role(
        base,
        &admin_cookie,
        "drafter",
        "Draft writer",
        &["view_admin", "edit_posts"],
    );
    assert_eq!(
        assign(base, &admin_cookie, holder.id, "drafter").status(),
        204
    );
    let cookie = common::login_cookie(base, &holder.email, &holder.password);

    // `GET /viewer`, and with it a plugin's `current-viewer` (both read
    // `Principal::viewer`): the custom role's slug, never the `subscriber`
    // it is stored over.
    let viewer = json_body(call(base, As::Cookie(&cookie), "GET", "/viewer", None));
    assert_eq!(viewer["signedIn"], true);
    assert_eq!(viewer["role"], "drafter");
    assert_eq!(viewer["roleName"], "Draft writer");
    let viewer = json_body(call(
        base,
        As::Cookie(&admin_cookie),
        "GET",
        "/viewer",
        None,
    ));
    assert_eq!(viewer["role"], "admin");
    assert_eq!(viewer["roleName"], "Administrator");
    let nobody = json_body(call(base, As::Nobody, "GET", "/viewer", None));
    assert_eq!(nobody, json!({ "signedIn": false }));

    // GraphQL: the built-in base, the custom slug, and the name to show.
    let me = graphql(base, &cookie, "{ me { role customRole roleName } }");
    assert_eq!(
        me["me"],
        json!({ "role": "subscriber", "customRole": "drafter", "roleName": "Draft writer" })
    );
    let me = graphql(base, &admin_cookie, "{ me { role customRole roleName } }");
    assert_eq!(
        me["me"],
        json!({ "role": "admin", "customRole": null, "roleName": "Administrator" })
    );
    let one = graphql(
        base,
        &admin_cookie,
        &format!("{{ user(id: {}) {{ customRole roleName }} }}", holder.id),
    );
    assert_eq!(
        one["user"],
        json!({ "customRole": "drafter", "roleName": "Draft writer" })
    );

    // REST, as before.
    let me = json_body(call(base, As::Cookie(&cookie), "GET", "/auth/me", None));
    assert_eq!(me["role"], "subscriber");
    assert_eq!(me["custom_role"], "drafter");
    assert_eq!(me["role_name"], "Draft writer");
}

/// The four `/roles` routes are session-only, like the other account
/// routes: a key is refused whatever it was granted, even an
/// administrator's with every capability.
#[tokio::test]
async fn an_api_key_is_refused_on_every_roles_route() {
    const KEY: &str = "vy_custom_role_admin_key_000001";
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    create_role(base, &cookie, "greeter", "Greeter", &["view_admin"]);
    let every_capability: Vec<Value> =
        json_body(call(base, As::Cookie(&cookie), "GET", "/roles", None))[0]["capabilities"]
            .as_array()
            .expect("the administrator's capabilities")
            .clone();
    assert_eq!(every_capability.len(), 12);
    ApiKeysRepo::new(pool.clone())
        .insert(
            97_401,
            admin.id,
            "everything",
            &key_hash(KEY),
            &json!(every_capability),
        )
        .await
        .expect("key");
    let key = As::Key(KEY);
    // The key works, and holds `manage_users`, where keys are accepted.
    assert_eq!(call(base, key, "GET", "/users", None).status(), 200);

    let role = json!({ "slug": "by-key", "name": "By key", "capabilities": ["view_admin"] });
    for (method, path, body) in [
        ("GET", "/roles", None),
        ("POST", "/roles", Some(role)),
        (
            "PATCH",
            "/roles/greeter",
            Some(json!({ "name": "Renamed by key" })),
        ),
        ("DELETE", "/roles/greeter", None),
    ] {
        let (status, message) = refusal(call(base, key, method, path, body));
        assert_eq!(status, 401, "{method} {path}: {message}");
    }
    // Nothing was written.
    let listed = json_body(call(base, As::Cookie(&cookie), "GET", "/roles", None));
    let custom: Vec<(&str, &str)> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["built_in"] == false)
        .map(|r| (r["slug"].as_str().unwrap(), r["name"].as_str().unwrap()))
        .collect();
    assert_eq!(custom, [("greeter", "Greeter")]);
}
