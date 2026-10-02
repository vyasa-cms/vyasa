//! Custom roles: the `roles` table, and how every user fetch carries its
//! custom role's name and capabilities.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_common::AppError;
use vyasa_db::models::Role;
use vyasa_db::repo::{ApiKeysRepo, NewRole, NewUser, RoleUpdate, RolesRepo, UsersRepo};
use vyasa_testkit::TestDb;

fn caps(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

async fn make_user(repo: &UsersRepo, id: i64, name: &str, role: Role) -> i64 {
    repo.insert(&NewUser {
        id,
        email: &format!("{name}@example.com"),
        username: name,
        display_name: name,
        password_hash: Some("argon2-hash"),
        role,
        bio: "",
    })
    .await
    .expect("insert user");
    id
}

async fn make_role(repo: &RolesRepo, slug: &str, name: &str, capabilities: &[&str]) {
    repo.insert(&NewRole {
        slug,
        name,
        description: "",
        capabilities: &caps(capabilities),
    })
    .await
    .expect("insert role");
}

#[tokio::test]
async fn roles_are_inserted_listed_fetched_updated_and_deleted() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let roles = RolesRepo::new(pool.clone());

    let made = roles
        .insert(&NewRole {
            slug: "moderator",
            name: "Moderator",
            description: "Keeps the comments tidy",
            capabilities: &caps(&["moderate_comments", "view_admin"]),
        })
        .await
        .expect("insert");
    assert_eq!(made.slug, "moderator");
    assert_eq!(made.name, "Moderator");
    assert_eq!(made.description, "Keeps the comments tidy");
    assert_eq!(
        made.capabilities,
        caps(&["moderate_comments", "view_admin"])
    );

    // The same slug twice is a conflict, not a database error.
    let clash = roles
        .insert(&NewRole {
            slug: "moderator",
            name: "Again",
            description: "",
            capabilities: &[],
        })
        .await;
    assert!(matches!(clash, Err(AppError::Conflict { .. })), "{clash:?}");

    make_role(&roles, "archivist", "Archivist", &[]).await;
    let listed = roles.list().await.expect("list");
    let slugs: Vec<&str> = listed.iter().map(|(r, _)| r.slug.as_str()).collect();
    assert_eq!(slugs, ["archivist", "moderator"]);
    assert!(listed.iter().all(|(_, users)| *users == 0));

    assert_eq!(roles.get("moderator").await.expect("get").name, "Moderator");
    assert!(matches!(
        roles.get("nope").await,
        Err(AppError::NotFound { .. })
    ));

    // A partial update leaves the other fields alone.
    let updated = roles
        .update(
            "moderator",
            &RoleUpdate {
                capabilities: Some(&caps(&["view_admin"])),
                ..RoleUpdate::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.capabilities, caps(&["view_admin"]));
    assert_eq!(updated.name, "Moderator");
    assert_eq!(updated.description, "Keeps the comments tidy");
    assert!(updated.updated_at >= updated.created_at);

    assert!(matches!(
        roles.update("nope", &RoleUpdate::default()).await,
        Err(AppError::NotFound { .. })
    ));
    // Renaming onto another role's slug is a conflict.
    let onto = roles
        .update(
            "moderator",
            &RoleUpdate {
                slug: Some("archivist"),
                ..RoleUpdate::default()
            },
        )
        .await;
    assert!(matches!(onto, Err(AppError::Conflict { .. })), "{onto:?}");

    roles.delete("archivist").await.expect("delete");
    assert!(matches!(
        roles.get("archivist").await,
        Err(AppError::NotFound { .. })
    ));
    assert!(matches!(
        roles.delete("archivist").await,
        Err(AppError::NotFound { .. })
    ));
    pool.close().await;
}

#[tokio::test]
async fn the_table_refuses_built_in_and_malformed_slugs() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let roles = RolesRepo::new(pool.clone());
    for slug in ["admin", "subscriber", "A", "x", "-lead", "has space"] {
        let result = roles
            .insert(&NewRole {
                slug,
                name: "N",
                description: "",
                capabilities: &[],
            })
            .await;
        assert!(result.is_err(), "{slug:?} must be refused");
    }
    pool.close().await;
}

#[tokio::test]
async fn a_role_in_use_cannot_be_deleted_and_the_refusal_names_the_count() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let roles = RolesRepo::new(pool.clone());
    let users = UsersRepo::new(pool.clone());
    make_role(&roles, "moderator", "Moderator", &["moderate_comments"]).await;
    let a = make_user(&users, 9101, "ann", Role::Author).await;
    let b = make_user(&users, 9102, "bob", Role::Editor).await;
    users
        .set_custom_role(a, Some("moderator"))
        .await
        .expect("a");
    users
        .set_custom_role(b, Some("moderator"))
        .await
        .expect("b");

    let listed = roles.list().await.expect("list");
    assert_eq!(listed[0].1, 2, "user count");

    let refused = roles.delete("moderator").await;
    match refused {
        Err(AppError::Conflict { message }) => {
            assert!(message.contains('2'), "message names the count: {message}");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert!(roles.get("moderator").await.is_ok(), "role survived");

    users.set_custom_role(a, None).await.expect("clear a");
    users.set_custom_role(b, None).await.expect("clear b");
    roles.delete("moderator").await.expect("now deletable");
    pool.close().await;
}

#[tokio::test]
async fn renaming_a_role_keeps_every_assignment() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let roles = RolesRepo::new(pool.clone());
    let users = UsersRepo::new(pool.clone());
    make_role(&roles, "moderator", "Moderator", &["moderate_comments"]).await;
    let id = make_user(&users, 9111, "ann", Role::Author).await;
    users
        .set_custom_role(id, Some("moderator"))
        .await
        .expect("assign");

    let renamed = roles
        .update(
            "moderator",
            &RoleUpdate {
                slug: Some("comment-lead"),
                name: Some("Comment lead"),
                ..RoleUpdate::default()
            },
        )
        .await
        .expect("rename");
    assert_eq!(renamed.slug, "comment-lead");

    let user = users.get(id).await.expect("user");
    assert_eq!(user.custom_role.as_deref(), Some("comment-lead"));
    assert_eq!(user.custom_role_name.as_deref(), Some("Comment lead"));
    assert_eq!(user.custom_role_caps, Some(caps(&["moderate_comments"])));
    pool.close().await;
}

#[tokio::test]
async fn every_user_fetch_carries_the_custom_role_name_and_capabilities() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let roles = RolesRepo::new(pool.clone());
    let users = UsersRepo::new(pool.clone());
    make_role(
        &roles,
        "moderator",
        "Moderator",
        &["moderate_comments", "view_admin"],
    )
    .await;
    let id = make_user(&users, 9121, "ann", Role::Author).await;
    let plain = make_user(&users, 9122, "bob", Role::Editor).await;
    users
        .set_custom_role(id, Some("moderator"))
        .await
        .expect("assign");

    let expect = |user: &vyasa_db::models::UserRow, path: &str| {
        assert_eq!(user.custom_role.as_deref(), Some("moderator"), "{path}");
        assert_eq!(
            user.custom_role_name.as_deref(),
            Some("Moderator"),
            "{path}"
        );
        assert_eq!(
            user.custom_role_caps,
            Some(caps(&["moderate_comments", "view_admin"])),
            "{path}"
        );
    };
    expect(&users.get(id).await.expect("by id"), "get");
    expect(
        &users.get_by_email("ann@example.com").await.expect("email"),
        "get_by_email",
    );
    let listed = users.list(50, 0).await.expect("list");
    expect(listed.iter().find(|u| u.id == id).expect("listed"), "list");
    let (found, _) = users.search("ann", 50, 0).await.expect("search");
    expect(&found[0], "search");
    // RETURNING paths carry it too.
    let edited = users
        .update_identity(id, None, None, Some("Ann B"), None)
        .await
        .expect("update_identity");
    expect(&edited, "update_identity");
    // An API key resolves to its owner with the role attached.
    let keys = ApiKeysRepo::new(pool.clone());
    sqlx::query("INSERT INTO api_keys (id, user_id, name, key_hash) VALUES (1, $1, 'k', 'hash-1')")
        .bind(id)
        .execute(&pool)
        .await
        .expect("key");
    let (_, owner) = keys.resolve("hash-1").await.expect("resolve");
    expect(&owner, "api key resolve");

    // A role edit shows on the very next fetch.
    roles
        .update(
            "moderator",
            &RoleUpdate {
                capabilities: Some(&caps(&["view_admin"])),
                ..RoleUpdate::default()
            },
        )
        .await
        .expect("edit");
    assert_eq!(
        users.get(id).await.expect("refetch").custom_role_caps,
        Some(caps(&["view_admin"]))
    );

    // A user without a custom role carries none of it.
    let bob = users.get(plain).await.expect("bob");
    assert_eq!(bob.custom_role, None);
    assert_eq!(bob.custom_role_name, None);
    assert_eq!(bob.custom_role_caps, None);
    pool.close().await;
}

#[tokio::test]
async fn assigning_a_custom_role_sets_the_base_role_to_subscriber() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let roles = RolesRepo::new(pool.clone());
    let users = UsersRepo::new(pool.clone());
    make_role(&roles, "moderator", "Moderator", &["moderate_comments"]).await;
    make_user(&users, 9131, "root", Role::Admin).await;
    let id = make_user(&users, 9132, "ann", Role::Editor).await;

    users
        .set_custom_role(id, Some("moderator"))
        .await
        .expect("assign");
    let user = users.get(id).await.expect("user");
    assert_eq!(user.role, Role::Subscriber);
    assert_eq!(user.custom_role.as_deref(), Some("moderator"));

    // An unknown role or user is not found; nothing changes.
    assert!(matches!(
        users.set_custom_role(id, Some("nope")).await,
        Err(AppError::NotFound { .. })
    ));
    assert!(matches!(
        users.set_custom_role(404, Some("moderator")).await,
        Err(AppError::NotFound { .. })
    ));
    assert_eq!(
        users.get(id).await.expect("user").custom_role.as_deref(),
        Some("moderator")
    );

    // Assigning a built-in role clears the custom one, by either path.
    users
        .set_role_keeping_an_admin(id, Role::Author)
        .await
        .expect("built-in");
    let user = users.get(id).await.expect("user");
    assert_eq!(user.role, Role::Author);
    assert_eq!(user.custom_role, None);
    assert_eq!(user.custom_role_caps, None);

    users
        .set_custom_role(id, Some("moderator"))
        .await
        .expect("again");
    users
        .set_role(id, Role::Editor)
        .await
        .expect("plain set_role");
    let user = users.get(id).await.expect("user");
    assert_eq!((user.role, user.custom_role), (Role::Editor, None));

    // Clearing leaves a plain subscriber.
    users
        .set_custom_role(id, Some("moderator"))
        .await
        .expect("again");
    users.set_custom_role(id, None).await.expect("clear");
    let user = users.get(id).await.expect("user");
    assert_eq!((user.role, user.custom_role), (Role::Subscriber, None));
    pool.close().await;
}

#[tokio::test]
async fn the_last_admin_cannot_be_given_a_custom_role() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let roles = RolesRepo::new(pool.clone());
    let users = UsersRepo::new(pool.clone());
    make_role(&roles, "moderator", "Moderator", &["moderate_comments"]).await;
    let only = make_user(&users, 9141, "root", Role::Admin).await;
    // A suspended admin does not count as a remaining one.
    let benched = make_user(&users, 9142, "benched", Role::Admin).await;
    users.set_suspended(benched, true).await.expect("suspend");

    let refused = users.set_custom_role(only, Some("moderator")).await;
    assert!(
        matches!(refused, Err(AppError::Validation { .. })),
        "{refused:?}"
    );
    let still = users.get(only).await.expect("user");
    assert_eq!((still.role, still.custom_role), (Role::Admin, None));

    // With a second active admin it goes through.
    make_user(&users, 9143, "second", Role::Admin).await;
    users
        .set_custom_role(only, Some("moderator"))
        .await
        .expect("ok");
    assert_eq!(users.get(only).await.expect("user").role, Role::Subscriber);
    pool.close().await;
}

/// Every user fetch reads the role's list as `Vec<String>`; a NULL in it
/// would fail to decode and lock the role's users out.
#[tokio::test]
async fn a_capability_list_holds_no_null() {
    let db = TestDb::new().await;
    let insert = sqlx::query(
        "INSERT INTO roles (slug, name, capabilities) VALUES ('holey', 'Holey', ARRAY['view_admin', NULL])",
    )
    .execute(db.pool())
    .await;
    let err = insert.expect_err("a NULL capability is refused");
    assert!(
        err.to_string().contains("roles_capabilities_no_null_check"),
        "{err}"
    );
    make_role(&RolesRepo::new(db.pool().clone()), "whole", "Whole", &[]).await;
    let update =
        sqlx::query("UPDATE roles SET capabilities = ARRAY[NULL]::text[] WHERE slug = 'whole'")
            .execute(db.pool())
            .await;
    assert!(update.is_err(), "nor by an update");
}

#[tokio::test]
async fn a_user_is_inserted_with_a_custom_role_or_not_at_all() {
    let db = TestDb::new().await;
    let users = UsersRepo::new(db.pool().clone());
    make_role(
        &RolesRepo::new(db.pool().clone()),
        "helper",
        "Helper",
        &["view_admin"],
    )
    .await;
    let new = |id, name: &'static str, email: &'static str| NewUser {
        id,
        email,
        username: name,
        display_name: name,
        password_hash: None,
        // Not used: the base role under a custom role is subscriber.
        role: Role::Admin,
        bio: "",
    };

    let made = users
        .insert_with_custom_role(&new(97_301, "held", "held@example.com"), "helper")
        .await
        .expect("insert");
    assert_eq!(made.role, Role::Subscriber);
    assert_eq!(made.custom_role.as_deref(), Some("helper"));
    assert_eq!(made.custom_role_name.as_deref(), Some("Helper"));
    assert_eq!(made.custom_role_caps, Some(caps(&["view_admin"])));

    let refused = users
        .insert_with_custom_role(&new(97_302, "lost", "lost@example.com"), "no-such-role")
        .await;
    assert!(
        matches!(refused, Err(AppError::NotFound { .. })),
        "{refused:?}"
    );
    assert!(matches!(
        users.get(97_302).await,
        Err(AppError::NotFound { .. })
    ));
    assert!(users.get_by_email("lost@example.com").await.is_err());
}
