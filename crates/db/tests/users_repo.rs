//! The users repository: lookups, roles, profiles, import keys and the
//! password-reset token lifecycle.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_db::models::{Role, TokenPurpose};
use vyasa_db::repo::{NewRole, NewUser, RoleUpdate, RolesRepo, TokensRepo, UsersRepo};
use vyasa_testkit::TestDb;

async fn make(repo: &UsersRepo, id: i64, name: &str, role: Role) -> i64 {
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
    .expect("insert");
    id
}

#[tokio::test]
async fn identity_lookups_are_case_insensitive_and_absent_ids_are_not_found() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let id = make(&repo, 9001, "ada", Role::Admin).await;

    assert_eq!(repo.get(id).await.expect("by id").username, "ada");
    // email and username are citext, so the casing someone types at a
    // login box does not decide whether they can sign in.
    for spelling in ["ada@example.com", "Ada@Example.com", "ADA@EXAMPLE.COM"] {
        assert_eq!(repo.get_by_email(spelling).await.expect(spelling).id, id);
    }
    assert!(repo.get(404).await.is_err());
    assert!(repo.get_by_email("nobody@example.com").await.is_err());
    pool.close().await;
}

#[tokio::test]
async fn duplicate_identities_are_refused_however_they_are_spelled() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    make(&repo, 9001, "ada", Role::Admin).await;

    // Same email, different case: the classic sign-up collision.
    let clash = repo
        .insert(&NewUser {
            id: 9002,
            email: "ADA@example.com",
            username: "someone-else",
            display_name: "Someone",
            password_hash: None,
            role: Role::Subscriber,
            bio: "",
        })
        .await;
    assert!(clash.is_err(), "a duplicate email must be refused");

    let clash = repo
        .insert(&NewUser {
            id: 9003,
            email: "other@example.com",
            username: "AdA",
            display_name: "Someone",
            password_hash: None,
            role: Role::Subscriber,
            bio: "",
        })
        .await;
    assert!(clash.is_err(), "a duplicate username must be refused");
    pool.close().await;
}

#[tokio::test]
async fn counting_and_listing_reflect_roles_and_paginate() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    make(&repo, 9001, "ada", Role::Admin).await;
    make(&repo, 9002, "grace", Role::Editor).await;
    make(&repo, 9003, "alan", Role::Editor).await;

    assert_eq!(repo.count().await.unwrap(), 3);
    assert_eq!(repo.count_by_role(Role::Admin).await.unwrap(), 1);
    assert_eq!(repo.count_by_role(Role::Editor).await.unwrap(), 2);
    assert_eq!(repo.count_by_role(Role::Author).await.unwrap(), 0);

    let page = repo.list(2, 0).await.unwrap();
    assert_eq!(page.len(), 2);
    let rest = repo.list(2, 2).await.unwrap();
    assert_eq!(rest.len(), 1);
    // No row appears on two pages.
    assert!(page.iter().all(|p| rest.iter().all(|r| r.id != p.id)));

    let names = repo.display_names(&[9001, 9003, 404]).await.unwrap();
    assert_eq!(names.get(&9001).map(String::as_str), Some("ada"));
    assert_eq!(names.get(&9003).map(String::as_str), Some("alan"));
    assert!(!names.contains_key(&404), "an unknown id is simply absent");
    assert!(
        repo.display_names(&[]).await.unwrap().is_empty(),
        "no ids, no query"
    );
    pool.close().await;
}

#[tokio::test]
async fn a_profile_role_and_password_can_each_be_changed_on_their_own() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let id = make(&repo, 9001, "ada", Role::Contributor).await;

    repo.update_profile(id, Some("Ada Lovelace"), Some("Writes about engines"))
        .await
        .expect("profile");
    let row = repo.get(id).await.unwrap();
    assert_eq!(row.display_name, "Ada Lovelace");
    assert_eq!(row.bio, "Writes about engines");
    // A profile edit is not a role change.
    assert_eq!(row.role, Role::Contributor);

    // Each field is independent: `None` leaves the stored value alone
    // rather than blanking it.
    repo.update_profile(id, None, Some("A new bio"))
        .await
        .expect("bio only");
    let row = repo.get(id).await.unwrap();
    assert_eq!(row.display_name, "Ada Lovelace", "name untouched");
    assert_eq!(row.bio, "A new bio");

    repo.set_role(id, Role::Editor).await.expect("role");
    assert_eq!(repo.get(id).await.unwrap().role, Role::Editor);

    repo.set_password(id, "new-argon2-hash")
        .await
        .expect("password");
    assert_eq!(
        repo.get(id).await.unwrap().password_hash.as_deref(),
        Some("new-argon2-hash")
    );
    // …and none of that disturbed the identity.
    assert_eq!(repo.get(id).await.unwrap().username, "ada");
    pool.close().await;
}

#[tokio::test]
async fn an_import_key_maps_back_to_the_account_it_was_written_on() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let id = make(&repo, 9001, "ada", Role::Admin).await;
    assert_eq!(repo.find_id_by_import_key("wp:42").await.unwrap(), None);

    repo.set_import_key(id, "wp:42").await.expect("set");
    assert_eq!(repo.find_id_by_import_key("wp:42").await.unwrap(), Some(id));
    // A key nobody carries stays unmatched, so a second import does not
    // silently attach to the wrong account.
    assert_eq!(repo.find_id_by_import_key("wp:99").await.unwrap(), None);
    pool.close().await;
}

#[tokio::test]
async fn a_reset_token_works_once_and_an_expired_one_never_does() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let id = make(&repo, 9001, "ada", Role::Admin).await;

    repo.create_reset_token(id, "hash-one")
        .await
        .expect("issue");
    assert_eq!(repo.consume_reset_token("hash-one").await.unwrap(), id);
    // Single use: replaying a token someone found in a mailbox or a log
    // must not work a second time.
    assert!(repo.consume_reset_token("hash-one").await.is_err());
    // And a token nobody issued is not a way in.
    assert!(repo.consume_reset_token("invented").await.is_err());

    // An expired token is refused even though it was never used.
    repo.create_reset_token(id, "hash-two")
        .await
        .expect("issue");
    sqlx::query("UPDATE reset_tokens SET expires_at = now() - interval '1 minute'")
        .execute(&pool)
        .await
        .expect("expire");
    assert!(repo.consume_reset_token("hash-two").await.is_err());
    pool.close().await;
}

#[tokio::test]
async fn deleting_a_user_takes_their_tokens_with_them() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let id = make(&repo, 9001, "ada", Role::Admin).await;
    repo.create_reset_token(id, "hash").await.expect("issue");

    repo.delete(id).await.expect("delete");
    assert!(repo.get(id).await.is_err());
    // An outstanding reset token must not survive the account: it would be
    // a credential for a user that no longer exists.
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM reset_tokens WHERE user_id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    pool.close().await;
}

/// A link mailed (or handed to a user manager) for a low-privilege account
/// must not become a way into the account it is promoted to.
#[tokio::test]
async fn a_role_change_drops_the_accounts_outstanding_reset_tokens() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let roles = RolesRepo::new(pool.clone());
    make(&repo, 9001, "ada", Role::Admin).await;
    let target = make(&repo, 9002, "grace", Role::Subscriber).await;
    let bystander = make(&repo, 9003, "linus", Role::Subscriber).await;
    let caps = vec!["view_admin".to_owned()];
    for slug in ["greeter", "other"] {
        roles
            .insert(&NewRole {
                slug,
                name: slug,
                description: "",
                capabilities: &caps,
            })
            .await
            .expect("role");
    }
    repo.create_reset_token(bystander, "bystander")
        .await
        .expect("issue");

    // Every way a role is written.
    repo.create_reset_token(target, "one").await.expect("issue");
    repo.set_role(target, Role::Editor).await.expect("set_role");
    assert!(repo.consume_reset_token("one").await.is_err());

    repo.create_reset_token(target, "two").await.expect("issue");
    repo.set_role_keeping_an_admin(target, Role::Author)
        .await
        .expect("set_role_keeping_an_admin");
    assert!(repo.consume_reset_token("two").await.is_err());

    repo.create_reset_token(target, "three")
        .await
        .expect("issue");
    repo.set_custom_role(target, Some("greeter"))
        .await
        .expect("assign");
    assert!(repo.consume_reset_token("three").await.is_err());

    repo.create_reset_token(target, "four")
        .await
        .expect("issue");
    repo.set_custom_role(target, None).await.expect("clear");
    assert!(repo.consume_reset_token("four").await.is_err());

    // A refused change leaves the token alone: nothing changed.
    repo.create_reset_token(9001, "admin").await.expect("issue");
    assert!(repo
        .set_role_keeping_an_admin(9001, Role::Editor)
        .await
        .is_err());
    assert!(repo.set_custom_role(9001, Some("greeter")).await.is_err());
    assert_eq!(repo.consume_reset_token("admin").await.unwrap(), 9001);

    // A custom role whose capabilities are replaced changes what every
    // holder is; renaming or re-describing it does not.
    repo.set_custom_role(target, Some("greeter"))
        .await
        .expect("assign");
    repo.set_custom_role(bystander, Some("other"))
        .await
        .expect("assign");
    repo.create_reset_token(target, "five")
        .await
        .expect("issue");
    repo.create_reset_token(target, "six").await.expect("issue");
    repo.create_reset_token(bystander, "bystander-two")
        .await
        .expect("issue");
    let rename = RoleUpdate {
        slug: Some("welcomer"),
        name: Some("Welcomer"),
        description: Some("Says hello"),
        capabilities: None,
    };
    roles.update("greeter", &rename).await.expect("rename");
    assert_eq!(repo.consume_reset_token("five").await.unwrap(), target);
    let wider = vec!["view_admin".to_owned(), "manage_users".to_owned()];
    let widen = RoleUpdate {
        slug: None,
        name: None,
        description: None,
        capabilities: Some(&wider),
    };
    roles.update("welcomer", &widen).await.expect("widen");
    assert!(repo.consume_reset_token("six").await.is_err());
    // Holders of another role are not touched.
    assert_eq!(
        repo.consume_reset_token("bystander-two").await.unwrap(),
        bystander
    );
    pool.close().await;
}

/// A role write that leaves what the account can do as it was leaves its
/// links alone: giving an account the role it already has, clearing a
/// custom role it does not have, and saving a role's capabilities
/// unchanged (in another order) are not changes. A confirmation link
/// grants nothing but the address, so no role write takes it.
#[tokio::test]
async fn a_role_write_that_changes_nothing_keeps_the_accounts_links() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let roles = RolesRepo::new(pool.clone());
    let tokens = TokensRepo::new(pool.clone());
    make(&repo, 9001, "ada", Role::Admin).await;
    let target = make(&repo, 9002, "grace", Role::Author).await;
    let caps = vec!["view_admin".to_owned(), "edit_posts".to_owned()];
    roles
        .insert(&NewRole {
            slug: "greeter",
            name: "Greeter",
            description: "",
            capabilities: &caps,
        })
        .await
        .expect("role");

    // The built-in role it already holds, both ways it is written.
    repo.create_reset_token(target, "one").await.expect("issue");
    repo.set_role(target, Role::Author).await.expect("set_role");
    repo.set_role_keeping_an_admin(target, Role::Author)
        .await
        .expect("same role");
    // No custom role to clear.
    repo.set_custom_role(target, None).await.expect("clear");
    assert_eq!(repo.consume_reset_token("one").await.unwrap(), target);

    // The custom role it already holds; the role re-saved with the same
    // capabilities in another order.
    repo.set_custom_role(target, Some("greeter"))
        .await
        .expect("assign");
    repo.create_reset_token(target, "two").await.expect("issue");
    repo.set_custom_role(target, Some("greeter"))
        .await
        .expect("same custom role");
    let reordered = vec!["edit_posts".to_owned(), "view_admin".to_owned()];
    roles
        .update(
            "greeter",
            &RoleUpdate {
                slug: None,
                name: Some("Greeter"),
                description: None,
                capabilities: Some(&reordered),
            },
        )
        .await
        .expect("re-save");
    assert_eq!(repo.consume_reset_token("two").await.unwrap(), target);

    // A real change still ends a reset link, but not a confirmation link.
    repo.create_reset_token(target, "three")
        .await
        .expect("issue");
    tokens
        .issue(
            target,
            TokenPurpose::Verify,
            "confirm",
            chrono::Duration::hours(24),
        )
        .await
        .expect("verify token");
    repo.set_role(target, Role::Editor).await.expect("set_role");
    assert!(repo.consume_reset_token("three").await.is_err());
    assert_eq!(
        tokens
            .consume("confirm", TokenPurpose::Verify)
            .await
            .unwrap(),
        Some(target)
    );
    pool.close().await;
}
