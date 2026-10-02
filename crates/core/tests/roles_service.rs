//! The roles service: validation, the escalation (subset) guard, in-use
//! delete, and assignment.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_common::AppError;
use vyasa_core::user::{can, effective_caps, Capability, RoleInput, RolePatch, RolesService};
use vyasa_db::models::{Role, UserRow};
use vyasa_db::repo::{NewUser, RolesRepo, UsersRepo};
use vyasa_testkit::TestDb;

struct Ctx {
    _db: TestDb,
    pool: sqlx::PgPool,
    users: UsersRepo,
    roles: RolesService,
}

async fn setup() -> Ctx {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let users = UsersRepo::new(pool.clone());
    let roles = RolesService::new(RolesRepo::new(pool.clone()), users.clone());
    Ctx {
        _db: db,
        pool,
        users,
        roles,
    }
}

async fn user(ctx: &Ctx, name: &str, role: Role) -> UserRow {
    ctx.users
        .insert(&NewUser {
            id: vyasa_common::next_id_i64(),
            email: &format!("{name}@example.com"),
            username: name,
            display_name: name,
            password_hash: Some("x"),
            role,
            bio: "",
        })
        .await
        .expect("user")
}

fn input(slug: &str, name: &str, capabilities: &[&str]) -> RoleInput {
    RoleInput {
        slug: slug.to_owned(),
        name: name.to_owned(),
        description: String::new(),
        capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
    }
}

#[tokio::test]
async fn a_valid_role_is_created_with_its_capabilities_deduplicated() {
    let ctx = setup().await;
    let admin = user(&ctx, "root", Role::Admin).await;
    let role = ctx
        .roles
        .create(
            &admin,
            input(
                "moderator",
                "  Moderator ",
                &["moderate_comments", "view_admin", "moderate_comments"],
            ),
        )
        .await
        .expect("create");
    assert_eq!(role.name, "Moderator");
    assert_eq!(role.capabilities, ["moderate_comments", "view_admin"]);

    // A role that can do nothing is allowed.
    let none = ctx
        .roles
        .create(&admin, input("nobody", "Nobody", &[]))
        .await
        .expect("empty");
    assert!(none.capabilities.is_empty());

    let listed = ctx.roles.list().await.expect("list");
    assert_eq!(listed.len(), 2);

    let twice = ctx
        .roles
        .create(&admin, input("moderator", "Again", &[]))
        .await;
    assert!(matches!(twice, Err(AppError::Conflict { .. })), "{twice:?}");
    ctx.pool.close().await;
}

#[tokio::test]
async fn bad_slugs_names_and_capabilities_are_validation_errors() {
    let ctx = setup().await;
    let admin = user(&ctx, "root", Role::Admin).await;
    let too_long = "a".repeat(41);
    for slug in [
        "",
        "x",
        "-lead",
        "Lead",
        "has space",
        "under_score",
        "é-role",
        too_long.as_str(),
    ] {
        let result = ctx.roles.create(&admin, input(slug, "Name", &[])).await;
        assert!(
            matches!(result, Err(AppError::Validation { .. })),
            "slug {slug:?}: {result:?}"
        );
    }
    for slug in ["admin", "editor", "author", "contributor", "subscriber"] {
        let result = ctx.roles.create(&admin, input(slug, "Name", &[])).await;
        assert!(
            matches!(result, Err(AppError::Validation { .. })),
            "reserved {slug:?}: {result:?}"
        );
    }
    // The longest and shortest legal slugs pass.
    let longest = "a".repeat(40);
    for slug in ["ab", "9-lives", longest.as_str()] {
        ctx.roles
            .create(&admin, input(slug, "Name", &[]))
            .await
            .unwrap_or_else(|e| panic!("slug {slug:?} should pass: {e:?}"));
    }

    let long_name = "n".repeat(61);
    for name in ["", "   ", long_name.as_str()] {
        let result = ctx.roles.create(&admin, input("named", name, &[])).await;
        assert!(
            matches!(result, Err(AppError::Validation { .. })),
            "name {name:?}: {result:?}"
        );
    }

    let unknown = ctx
        .roles
        .create(&admin, input("rooty", "Rooty", &["view_admin", "root"]))
        .await;
    match unknown {
        Err(AppError::Validation { message }) => assert!(message.contains("root"), "{message}"),
        other => panic!("expected validation, got {other:?}"),
    }

    // The same rules hold on update.
    ctx.roles
        .create(&admin, input("moderator", "Moderator", &[]))
        .await
        .expect("create");
    for patch in [
        RolePatch {
            slug: Some("admin".into()),
            ..RolePatch::default()
        },
        RolePatch {
            slug: Some("Bad Slug".into()),
            ..RolePatch::default()
        },
        RolePatch {
            name: Some(String::new()),
            ..RolePatch::default()
        },
        RolePatch {
            capabilities: Some(vec!["fly".into()]),
            ..RolePatch::default()
        },
    ] {
        let result = ctx.roles.update(&admin, "moderator", patch).await;
        assert!(
            matches!(result, Err(AppError::Validation { .. })),
            "{result:?}"
        );
    }
    ctx.pool.close().await;
}

#[tokio::test]
async fn a_caller_cannot_create_edit_or_assign_a_role_beyond_their_own_capabilities() {
    let ctx = setup().await;
    let admin = user(&ctx, "root", Role::Admin).await;
    // An editor's capabilities: no manage_users.
    let editor = user(&ctx, "ed", Role::Editor).await;
    let target = user(&ctx, "tess", Role::Author).await;

    let refused = ctx
        .roles
        .create(
            &editor,
            input("people", "People", &["view_admin", "manage_users"]),
        )
        .await;
    match &refused {
        Err(AppError::Forbidden { message }) => {
            assert!(message.contains("manage_users"), "{message}");
        }
        other => panic!("expected forbidden, got {other:?}"),
    }
    assert!(ctx.roles.list().await.expect("list").is_empty());

    // Within their own set is fine.
    ctx.roles
        .create(
            &editor,
            input("moderator", "Moderator", &["moderate_comments"]),
        )
        .await
        .expect("subset");

    // Editing a role up past the caller is refused...
    let widened = ctx
        .roles
        .update(
            &editor,
            "moderator",
            RolePatch {
                capabilities: Some(vec!["manage_options".into()]),
                ..RolePatch::default()
            },
        )
        .await;
    assert!(
        matches!(widened, Err(AppError::Forbidden { .. })),
        "{widened:?}"
    );

    // ...and so is touching (even renaming) a role that already exceeds them.
    ctx.roles
        .create(&admin, input("people", "People", &["manage_users"]))
        .await
        .expect("admin creates");
    let renamed = ctx
        .roles
        .update(
            &editor,
            "people",
            RolePatch {
                name: Some("Persons".into()),
                ..RolePatch::default()
            },
        )
        .await;
    assert!(
        matches!(renamed, Err(AppError::Forbidden { .. })),
        "{renamed:?}"
    );

    // Even to take capabilities out of it: its holders would fall within
    // the caller's reach.
    let stripped = ctx
        .roles
        .update(
            &editor,
            "people",
            RolePatch {
                capabilities: Some(Vec::new()),
                ..RolePatch::default()
            },
        )
        .await;
    assert!(
        matches!(stripped, Err(AppError::Forbidden { .. })),
        "{stripped:?}"
    );
    let deleted = ctx.roles.delete(&editor, "people").await;
    assert!(
        matches!(deleted, Err(AppError::Forbidden { .. })),
        "{deleted:?}"
    );
    assert_eq!(
        ctx.roles.get("people").await.expect("kept").capabilities,
        ["manage_users"]
    );
    // A role they do hold in full is theirs to narrow and delete.
    ctx.roles
        .update(
            &editor,
            "moderator",
            RolePatch {
                capabilities: Some(Vec::new()),
                ..RolePatch::default()
            },
        )
        .await
        .expect("narrow");

    // Assigning it is refused too; the target is untouched.
    let assigned = ctx.roles.assign(&editor, target.id, Some("people")).await;
    assert!(
        matches!(assigned, Err(AppError::Forbidden { .. })),
        "{assigned:?}"
    );
    assert_eq!(
        ctx.users.get(target.id).await.expect("t").role,
        Role::Author
    );

    // A per-user override counts: the guard uses effective capabilities.
    sqlx::query("UPDATE users SET meta = '{\"disabled_caps\": [\"manage_users\"]}' WHERE id = $1")
        .bind(admin.id)
        .execute(&ctx.pool)
        .await
        .expect("meta");
    let limited = ctx.users.get(admin.id).await.expect("admin");
    let assigned = ctx.roles.assign(&limited, target.id, Some("people")).await;
    assert!(
        matches!(assigned, Err(AppError::Forbidden { .. })),
        "{assigned:?}"
    );
    ctx.pool.close().await;
}

#[tokio::test]
async fn an_assigned_user_holds_exactly_the_role_and_edits_show_on_refetch() {
    let ctx = setup().await;
    let admin = user(&ctx, "root", Role::Admin).await;
    let target = user(&ctx, "tess", Role::Editor).await;
    ctx.roles
        .create(
            &admin,
            input(
                "moderator",
                "Moderator",
                &["moderate_comments", "view_admin"],
            ),
        )
        .await
        .expect("create");

    ctx.roles
        .assign(&admin, target.id, Some("moderator"))
        .await
        .expect("assign");
    let tess = ctx.users.get(target.id).await.expect("tess");
    assert_eq!(tess.role, Role::Subscriber);
    assert_eq!(
        effective_caps(&tess),
        [Capability::ModerateComments, Capability::ViewAdmin]
    );
    assert!(!can(&tess, Capability::EditPosts), "editor caps are gone");

    ctx.roles
        .update(
            &admin,
            "moderator",
            RolePatch {
                slug: Some("comment-lead".into()),
                capabilities: Some(vec!["moderate_comments".into()]),
                ..RolePatch::default()
            },
        )
        .await
        .expect("edit");
    let tess = ctx.users.get(target.id).await.expect("tess");
    assert_eq!(tess.custom_role.as_deref(), Some("comment-lead"));
    assert_eq!(effective_caps(&tess), [Capability::ModerateComments]);

    // In use: delete is a conflict naming the count.
    match ctx.roles.delete(&admin, "comment-lead").await {
        Err(AppError::Conflict { message }) => assert!(message.contains('1'), "{message}"),
        other => panic!("expected conflict, got {other:?}"),
    }
    assert!(matches!(
        ctx.roles.assign(&admin, target.id, Some("nope")).await,
        Err(AppError::NotFound { .. })
    ));

    ctx.roles
        .assign(&admin, target.id, None)
        .await
        .expect("clear");
    let tess = ctx.users.get(target.id).await.expect("tess");
    assert_eq!(effective_caps(&tess), [Capability::ViewAdmin]);
    ctx.roles
        .delete(&admin, "comment-lead")
        .await
        .expect("delete");
    ctx.pool.close().await;
}

#[tokio::test]
async fn the_last_admin_cannot_be_assigned_a_custom_role() {
    let ctx = setup().await;
    let admin = user(&ctx, "root", Role::Admin).await;
    ctx.roles
        .create(&admin, input("moderator", "Moderator", &["view_admin"]))
        .await
        .expect("create");
    let refused = ctx.roles.assign(&admin, admin.id, Some("moderator")).await;
    assert!(
        matches!(refused, Err(AppError::Validation { .. })),
        "{refused:?}"
    );
    assert_eq!(ctx.users.get(admin.id).await.expect("a").role, Role::Admin);
    ctx.pool.close().await;
}
