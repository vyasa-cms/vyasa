//! Public registration at the service level: unconfirmed accounts, the
//! default-role rule, no change to an existing account, confirmation
//! links, the sign-in refusal and the two options.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use serde_json::json;
use vyasa_common::AppError;
use vyasa_core::options::{OptionsService, FULL_ADMINISTRATOR_OPTION_KEYS};
use vyasa_core::user::password::{hash_password, verify_password};
use vyasa_core::user::registration::hash_token;
use vyasa_core::user::{
    can, ensure_confirmed, AuthService, Capability, CreateUser, NewRegistration, RegisterOutcome,
    RegistrationService, ResendOutcome, UsersService, VerifyOutcome,
};
use vyasa_db::models::{Role, TokenPurpose, UserRow};
use vyasa_db::repo::{
    NewRole, OptionsRepo, RoleUpdate, RolesRepo, SessionsRepo, TokensRepo, UsersRepo,
};
use vyasa_testkit::TestDb;

struct Ctx {
    _db: TestDb,
    pool: sqlx::PgPool,
    users: UsersRepo,
    roles: RolesRepo,
    tokens: TokensRepo,
    options: OptionsService,
    auth: AuthService,
    reg: RegistrationService,
}

async fn setup() -> Ctx {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let users = UsersRepo::new(pool.clone());
    let roles = RolesRepo::new(pool.clone());
    let tokens = TokensRepo::new(pool.clone());
    let options = OptionsService::new(OptionsRepo::new(pool.clone()));
    let auth = AuthService::new(users.clone(), SessionsRepo::new(pool.clone()));
    let reg = RegistrationService::new(
        users.clone(),
        tokens.clone(),
        roles.clone(),
        options.clone(),
    );
    Ctx {
        _db: db,
        pool,
        users,
        roles,
        tokens,
        options,
        auth,
        reg,
    }
}

async fn open(ctx: &Ctx) {
    ctx.options
        .put("registration_enabled", json!(true))
        .await
        .expect("enable");
}

/// A registration; the visitor names only themselves, not their username.
fn request(email: &str, display_name: &str, password: &str) -> NewRegistration {
    NewRegistration {
        email: email.to_owned(),
        display_name: Some(display_name.to_owned()),
        password: password.to_owned(),
    }
}

async fn role(ctx: &Ctx, slug: &str, caps: &[&str]) {
    let caps: Vec<String> = caps.iter().map(|c| (*c).to_owned()).collect();
    ctx.roles
        .insert(&NewRole {
            slug,
            name: slug,
            description: "",
            capabilities: &caps,
        })
        .await
        .expect("role");
}

/// The account a confirmation confirmed, and whether it still needs a
/// password; panics on a password mismatch.
fn confirmed(outcome: VerifyOutcome) -> (UserRow, bool) {
    match outcome {
        VerifyOutcome::Confirmed {
            user,
            needs_password,
        } => (user, needs_password),
        VerifyOutcome::PasswordMismatch => panic!("expected Confirmed, got PasswordMismatch"),
    }
}

fn created(outcome: RegisterOutcome) -> (UserRow, String) {
    match outcome {
        RegisterOutcome::Created { user, token } => (user, token),
        other => panic!("expected Created, got {other:?}"),
    }
}

async fn count(ctx: &Ctx) -> i64 {
    ctx.users.count().await.unwrap()
}

// --- Off by default ---------------------------------------------------------

#[tokio::test]
async fn registration_is_closed_until_it_is_turned_on() {
    let ctx = setup().await;
    let status = ctx.reg.status().await.unwrap();
    assert!(!status.enabled);
    assert_eq!(status.password_min_length, 8);

    let refused = ctx
        .reg
        .register(request("a@example.com", "a", "correct horse"))
        .await;
    assert!(matches!(refused, Err(AppError::Forbidden { .. })));
    assert_eq!(count(&ctx).await, 0);

    open(&ctx).await;
    assert!(ctx.reg.status().await.unwrap().enabled);
    pool_close(ctx).await;
}

async fn pool_close(ctx: Ctx) {
    ctx.pool.close().await;
}

// --- A new account ----------------------------------------------------------

#[tokio::test]
async fn registering_makes_an_unconfirmed_subscriber_who_cannot_sign_in_until_confirmed() {
    let ctx = setup().await;
    open(&ctx).await;
    let (user, token) = created(
        ctx.reg
            .register(NewRegistration {
                display_name: Some("  Ada L.  ".to_owned()),
                ..request("  Ada@Example.com ", "Ada", "correct horse")
            })
            .await
            .unwrap(),
    );
    assert_eq!(user.email, "Ada@Example.com");
    assert!(generated_from(&user.username, "ada-l"), "{}", user.username);
    assert_eq!(user.display_name, "Ada L.");
    assert_eq!(user.role, Role::Subscriber);
    assert!(user.custom_role.is_none());
    assert!(user.email_verified_at.is_none());
    assert!(verify_password("correct horse", user.password_hash.as_deref().unwrap()).is_ok());
    // The token is stored hashed, as a confirmation token, for a day.
    let (purpose, hours): (String, f64) = sqlx::query_as(
        "SELECT purpose, (EXTRACT(EPOCH FROM (expires_at - now())) / 3600)::float8
         FROM reset_tokens WHERE token_hash = $1",
    )
    .bind(hash_token(&token))
    .fetch_one(&ctx.pool)
    .await
    .expect("stored hashed");
    assert_eq!(purpose, "verify");
    assert!((23.9..=24.0).contains(&hours), "{hours}");

    // A wrong password learns nothing about the account's state...
    let wrong = ctx.auth.login("ada@example.com", "wrong", None, None).await;
    assert!(matches!(wrong, Err(AppError::Auth { .. })));
    // ...the right one is told to confirm first, and gets no session.
    let right = ctx
        .auth
        .login("ada@example.com", "correct horse", None, None)
        .await;
    assert!(matches!(right, Err(AppError::EmailUnconfirmed)));
    assert!(matches!(
        ensure_confirmed(&user),
        Err(AppError::EmailUnconfirmed)
    ));
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);

    // Opened with the password chosen at sign-up: confirmed, and kept.
    let (confirmed_user, needs_password) = confirmed(
        ctx.reg
            .verify(&token, Some("correct horse"))
            .await
            .expect("confirmed"),
    );
    assert_eq!(confirmed_user.id, user.id);
    assert!(confirmed_user.email_verified_at.is_some());
    assert!(!needs_password);
    assert!(ensure_confirmed(&confirmed_user).is_ok());
    let session = ctx
        .auth
        .login("ada@example.com", "correct horse", None, None)
        .await
        .expect("signs in");
    assert_eq!(
        ctx.auth.resolve(&session.token).await.unwrap().user.id,
        user.id
    );
    pool_close(ctx).await;
}

/// The sign-in lookup trims the address as every per-address count does,
/// so " ada@example.com" is the account the lockout counts it towards.
#[tokio::test]
async fn a_sign_in_finds_the_account_whatever_space_surrounds_the_address() {
    let ctx = setup().await;
    let hash = hash_password("correct horse").unwrap();
    ctx.users
        .insert(&vyasa_db::repo::NewUser {
            id: 98_901,
            email: "Ada@Example.com",
            username: "ada",
            display_name: "Ada",
            password_hash: Some(&hash),
            role: Role::Subscriber,
            bio: "",
        })
        .await
        .unwrap();
    for spelling in [
        "ada@example.com ",
        " ADA@example.com",
        "\tada@example.com\n",
    ] {
        let session = ctx
            .auth
            .login(spelling, "correct horse", None, None)
            .await
            .unwrap_or_else(|err| panic!("{spelling:?}: {err}"));
        assert_eq!(session.user.id, 98_901);
    }
    // A wrong password with a trailing space is the ordinary refusal.
    let wrong = ctx
        .auth
        .login("ada@example.com ", "wrong", None, None)
        .await;
    assert!(matches!(wrong, Err(AppError::Auth { .. })));
    pool_close(ctx).await;
}

#[tokio::test]
async fn a_session_of_an_unconfirmed_account_does_not_resolve() {
    let ctx = setup().await;
    open(&ctx).await;
    let (user, _) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .unwrap(),
    );
    // However it came to exist.
    SessionsRepo::new(ctx.pool.clone())
        .insert(
            "stray-session",
            user.id,
            chrono::Duration::hours(1),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        ctx.auth.resolve("stray-session").await,
        Err(AppError::Auth { .. })
    ));
    pool_close(ctx).await;
}

#[tokio::test]
async fn accounts_made_any_other_way_are_confirmed_and_sign_in_as_before() {
    let ctx = setup().await;
    let made = UsersService::new(ctx.users.clone())
        .create(CreateUser {
            email: "staff@example.com".to_owned(),
            username: None,
            display_name: None,
            password: Some("correct horse".to_owned()),
            role: Role::Editor,
        })
        .await
        .unwrap();
    assert!(made.email_verified_at.is_some());
    assert!(ctx
        .auth
        .login("staff@example.com", "correct horse", None, None)
        .await
        .is_ok());
    pool_close(ctx).await;
}

/// An administrator creating an account for an address an unconfirmed
/// sign-up holds is told what holds it.
#[tokio::test]
async fn creating_over_an_unconfirmed_sign_up_names_it_in_the_conflict() {
    let ctx = setup().await;
    open(&ctx).await;
    created(
        ctx.reg
            .register(request("held@example.com", "held", "correct horse"))
            .await
            .unwrap(),
    );
    let users = UsersService::new(ctx.users.clone());
    let create = |email: &str| CreateUser {
        email: email.to_owned(),
        username: None,
        display_name: None,
        password: None,
        role: Role::Author,
    };
    match users.create(create("HELD@example.com")).await {
        Err(AppError::Conflict { message }) => assert_eq!(
            message,
            "an unconfirmed sign-up holds this address; confirm or delete it first"
        ),
        other => panic!("expected a conflict, got {other:?}"),
    }
    users.create(create("staff@example.com")).await.unwrap();
    match users.create(create("staff@example.com")).await {
        Err(AppError::Conflict { message }) => {
            assert_eq!(message, "email or username is already taken");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    pool_close(ctx).await;
}

#[tokio::test]
async fn malformed_requests_are_refused_before_anything_is_stored() {
    let ctx = setup().await;
    open(&ctx).await;
    for bad in [
        request("not-an-address", "a", "correct horse"),
        request("a b@example.com", "a", "correct horse"),
        request("@example.com", "a", "correct horse"),
        request("a@", "a", "correct horse"),
        request("   ", "a", "correct horse"),
        request("a@example.com", "a", "short"),
        NewRegistration {
            display_name: Some("x".repeat(101)),
            ..request("a@example.com", "a", "correct horse")
        },
    ] {
        let refused = ctx.reg.register(bad).await;
        assert!(
            matches!(refused, Err(AppError::Validation { .. })),
            "{refused:?}"
        );
    }
    assert_eq!(count(&ctx).await, 0);
    pool_close(ctx).await;
}

// --- The default role -------------------------------------------------------

#[tokio::test]
async fn the_configured_default_role_is_used_built_in_or_custom() {
    let ctx = setup().await;
    open(&ctx).await;
    ctx.options
        .put("registration_default_role", json!("contributor"))
        .await
        .unwrap();
    let (user, _) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .unwrap(),
    );
    assert_eq!(user.role, Role::Contributor);
    assert!(user.custom_role.is_none());

    role(&ctx, "member", &["view_admin", "upload_media"]).await;
    ctx.options
        .put("registration_default_role", json!("member"))
        .await
        .unwrap();
    let (user, _) = created(
        ctx.reg
            .register(request("b@example.com", "b", "correct horse"))
            .await
            .unwrap(),
    );
    assert_eq!(user.role, Role::Subscriber);
    assert_eq!(user.custom_role.as_deref(), Some("member"));
    assert!(can(&user, Capability::UploadMedia));
    assert!(user.email_verified_at.is_none());
    pool_close(ctx).await;
}

#[tokio::test]
async fn a_default_role_with_an_administrative_capability_is_refused_at_save() {
    let ctx = setup().await;
    for key in ["registration_enabled", "registration_default_role"] {
        assert!(FULL_ADMINISTRATOR_OPTION_KEYS.contains(&key), "{key}");
    }
    // The built-in administrator and editor, and shapes that are not a
    // role at all.
    for bad in [
        json!("admin"),
        json!("editor"),
        json!(""),
        json!(7),
        json!(null),
        json!("nobody"),
    ] {
        let refused = ctx
            .options
            .put("registration_default_role", bad.clone())
            .await;
        assert!(
            matches!(refused, Err(AppError::Validation { .. })),
            "{bad}: {refused:?}"
        );
    }
    // A custom role holding any one of the seven.
    for (slug, cap) in [
        ("users", "manage_users"),
        ("options", "manage_options"),
        ("plugins", "manage_plugins"),
        ("themes", "manage_themes"),
        ("others", "edit_others"),
        ("moderators", "moderate_comments"),
        ("taxonomists", "manage_categories"),
    ] {
        role(&ctx, slug, &["view_admin", cap]).await;
        let refused = ctx
            .options
            .put("registration_default_role", json!(slug))
            .await;
        assert!(
            matches!(refused, Err(AppError::Validation { .. })),
            "{slug}: {refused:?}"
        );
        assert!(ctx
            .options
            .validate_with_lookups("registration_default_role", &json!(slug))
            .await
            .is_err());
    }
    // Nothing was stored by the refused writes.
    assert_eq!(
        ctx.options.registration_default_role().await.unwrap(),
        "subscriber"
    );
    // Every other built-in role and a harmless custom role are accepted.
    role(&ctx, "member", &["view_admin", "edit_posts"]).await;
    for ok in ["subscriber", "contributor", "author", "member"] {
        ctx.options
            .put("registration_default_role", json!(ok))
            .await
            .unwrap_or_else(|err| panic!("{ok}: {err}"));
        assert_eq!(ctx.options.registration_default_role().await.unwrap(), ok);
    }
    // The switch is a boolean and nothing else.
    for bad in [json!("yes"), json!(1), json!(null)] {
        assert!(ctx.options.put("registration_enabled", bad).await.is_err());
    }
    pool_close(ctx).await;
}

/// Review focus 5: the role was fine when it was chosen.
#[tokio::test]
async fn a_default_role_later_widened_deleted_or_forced_falls_back_to_subscriber() {
    let ctx = setup().await;
    open(&ctx).await;
    role(&ctx, "member", &["view_admin"]).await;
    ctx.options
        .put("registration_default_role", json!("member"))
        .await
        .unwrap();

    // Widened past the line.
    let wider = vec!["view_admin".to_owned(), "manage_plugins".to_owned()];
    ctx.roles
        .update(
            "member",
            &RoleUpdate {
                capabilities: Some(&wider),
                ..RoleUpdate::default()
            },
        )
        .await
        .unwrap();
    let (user, _) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .expect("nothing errors"),
    );
    assert_eq!(user.role, Role::Subscriber);
    assert!(user.custom_role.is_none());
    assert!(!can(&user, Capability::ManagePlugins));

    // A role over the site's shared categories: past an author's power,
    // so it falls back too (saved before it was ruled out, or forced).
    role(&ctx, "taxonomists", &["view_admin", "manage_categories"]).await;
    OptionsRepo::new(ctx.pool.clone())
        .set("registration_default_role", &json!("taxonomists"))
        .await
        .unwrap();
    let (user, _) = created(
        ctx.reg
            .register(request("t@example.com", "t", "correct horse"))
            .await
            .expect("nothing errors"),
    );
    assert_eq!(user.role, Role::Subscriber);
    assert!(user.custom_role.is_none());
    assert!(!can(&user, Capability::ManageCategories));

    // Deleted.
    ctx.roles.delete("member").await.unwrap();
    let (user, _) = created(
        ctx.reg
            .register(request("b@example.com", "b", "correct horse"))
            .await
            .expect("nothing errors"),
    );
    assert_eq!(user.role, Role::Subscriber);
    assert!(user.custom_role.is_none());

    // Written straight into storage, past the validator (or saved before
    // editors were ruled out).
    for forced in [json!("admin"), json!("editor"), json!(42), json!("")] {
        OptionsRepo::new(ctx.pool.clone())
            .set("registration_default_role", &forced)
            .await
            .unwrap();
        let name = format!("c{}", vyasa_common::next_id_i64());
        let (user, _) = created(
            ctx.reg
                .register(request(
                    &format!("{name}@example.com"),
                    &name,
                    "correct horse",
                ))
                .await
                .expect("nothing errors"),
        );
        assert_eq!(user.role, Role::Subscriber, "{forced}");
        assert!(user.custom_role.is_none());
    }
    pool_close(ctx).await;
}

// --- An address that already has an account ---------------------------------

/// Review focus 1 (confirmed and suspended accounts).
#[tokio::test]
async fn an_address_with_an_account_is_told_so_by_mail_and_nothing_about_it_changes() {
    let ctx = setup().await;
    open(&ctx).await;
    let staff = UsersService::new(ctx.users.clone())
        .create(CreateUser {
            email: "staff@example.com".to_owned(),
            username: Some("staff".to_owned()),
            display_name: None,
            password: Some("original password".to_owned()),
            role: Role::Editor,
        })
        .await
        .unwrap();
    let barred = UsersService::new(ctx.users.clone())
        .create(CreateUser {
            email: "barred@example.com".to_owned(),
            username: Some("barred".to_owned()),
            display_name: None,
            password: Some("original password".to_owned()),
            role: Role::Subscriber,
        })
        .await
        .unwrap();
    ctx.users.set_suspended(barred.id, true).await.unwrap();
    // Suspended before it ever confirmed.
    let (pending, _) = created(
        ctx.reg
            .register(request(
                "pending@example.com",
                "pending",
                "original password",
            ))
            .await
            .unwrap(),
    );
    ctx.users.set_suspended(pending.id, true).await.unwrap();

    for (before, spelling) in [
        (staff.id, " STAFF@example.com "),
        (barred.id, "barred@example.com"),
        (pending.id, "pending@example.com"),
    ] {
        let before = ctx.users.get(before).await.unwrap();
        let tokens_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reset_tokens")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
        // Even with the account's own password.
        for password in ["another password", "original password"] {
            let outcome = ctx
                .reg
                .register(NewRegistration {
                    display_name: Some("Someone Else".to_owned()),
                    ..request(spelling, "newcomer", password)
                })
                .await
                .unwrap();
            match outcome {
                RegisterOutcome::AlreadyRegistered { user } => assert_eq!(user.id, before.id),
                other => panic!("expected AlreadyRegistered, got {other:?}"),
            }
        }
        let after = ctx.users.get(before.id).await.unwrap();
        assert_eq!(after.password_hash, before.password_hash);
        assert_eq!(after.username, before.username);
        assert_eq!(after.display_name, before.display_name);
        assert_eq!(after.role, before.role);
        assert_eq!(after.email_verified_at, before.email_verified_at);
        assert_eq!(after.updated_at, before.updated_at);
        let tokens_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reset_tokens")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
        assert_eq!(tokens_after, tokens_before);
    }
    assert_eq!(count(&ctx).await, 3);
    assert_eq!(usernames_like(&ctx, "newcomer%").await, 0);
    pool_close(ctx).await;
}

/// Review focus 3.
#[tokio::test]
async fn case_and_whitespace_in_the_address_do_not_make_a_second_account() {
    let ctx = setup().await;
    open(&ctx).await;
    let (first, _) = created(
        ctx.reg
            .register(request("A@x.com ", "first", "correct horse"))
            .await
            .unwrap(),
    );
    let again = ctx
        .reg
        .register(request("a@x.com", "second", "correct horse"))
        .await
        .unwrap();
    match again {
        RegisterOutcome::ConfirmationResent { user, .. } => assert_eq!(user.id, first.id),
        other => panic!("expected ConfirmationResent, got {other:?}"),
    }
    assert_eq!(count(&ctx).await, 1);
    // Resend finds it however it is typed, too.
    assert!(matches!(
        ctx.reg.resend("\tA@X.COM\n").await.unwrap(),
        ResendOutcome::Send { .. }
    ));
    pool_close(ctx).await;
}

/// Review focus 2, the honest half: the same person submitting the form
/// again.
#[tokio::test]
async fn registering_again_with_the_same_password_resends_and_changes_nothing() {
    let ctx = setup().await;
    open(&ctx).await;
    let (first, first_token) = created(
        ctx.reg
            .register(request("a@example.com", "ada", "correct horse"))
            .await
            .unwrap(),
    );
    // The same form again.
    let again = ctx
        .reg
        .register(request("a@example.com", "ada", "correct horse"))
        .await
        .unwrap();
    let RegisterOutcome::ConfirmationResent {
        user,
        token,
        password_cleared,
    } = again
    else {
        panic!("expected ConfirmationResent, got {again:?}");
    };
    assert_eq!(user.id, first.id);
    assert!(!password_cleared);
    assert_ne!(token, first_token);
    let stored = ctx.users.get(first.id).await.unwrap();
    assert_eq!(stored.password_hash, first.password_hash);
    assert_eq!(count(&ctx).await, 1);

    // Either link confirms; the password set at registration works.
    let (_, needs_password) = confirmed(
        ctx.reg
            .verify(&first_token, Some("correct horse"))
            .await
            .unwrap(),
    );
    assert!(!needs_password);
    assert!(ctx
        .auth
        .login("a@example.com", "correct horse", None, None)
        .await
        .is_ok());
    pool_close(ctx).await;
}

/// Review focus 2, the hostile half: two parties, one address. Neither
/// password survives; whoever holds the mailbox sets the one that counts.
#[tokio::test]
async fn a_second_registration_with_another_password_leaves_the_account_without_one() {
    let ctx = setup().await;
    open(&ctx).await;
    // Somebody registers an address that may not be theirs...
    let (first, first_token) = created(
        ctx.reg
            .register(request(
                "owner@example.com",
                "squatter",
                "squatters password",
            ))
            .await
            .unwrap(),
    );
    // ...and then somebody else does, with a password of their own.
    let second = ctx
        .reg
        .register(NewRegistration {
            display_name: Some("The Owner".to_owned()),
            ..request("owner@example.com", "owner", "owners password")
        })
        .await
        .unwrap();
    let RegisterOutcome::ConfirmationResent {
        user,
        token: second_token,
        password_cleared,
    } = second
    else {
        panic!("expected ConfirmationResent, got {second:?}");
    };
    assert_eq!(user.id, first.id);
    assert!(password_cleared);
    assert_eq!(count(&ctx).await, 1);
    // The first request's password was not replaced by the second's: the
    // account has none. Its identity is as first registered.
    let stored = ctx.users.get(first.id).await.unwrap();
    assert!(stored.password_hash.is_none());
    assert!(generated_from(&stored.username, "squatter"));
    assert_eq!(stored.display_name, "squatter");
    assert!(stored.email_verified_at.is_none());
    assert_eq!(usernames_like(&ctx, "the-owner%").await, 0);

    // A third attempt cannot put a password back, whoever makes it.
    for password in ["squatters password", "owners password", "a third password"] {
        let third = ctx
            .reg
            .register(request("owner@example.com", "third", password))
            .await
            .unwrap();
        assert!(matches!(
            third,
            RegisterOutcome::ConfirmationResent {
                password_cleared: true,
                ..
            }
        ));
        assert!(ctx
            .users
            .get(first.id)
            .await
            .unwrap()
            .password_hash
            .is_none());
    }
    // Resending says the same.
    assert!(matches!(
        ctx.reg.resend("owner@example.com").await.unwrap(),
        ResendOutcome::Send {
            needs_password: true,
            ..
        }
    ));

    // Whichever link the mailbox's owner opens, the account is confirmed
    // and the caller is told a password has to be set.
    // With either party's password it is a mismatch: there is none to
    // match, and nothing changes.
    for password in ["squatters password", "owners password"] {
        assert!(matches!(
            ctx.reg.verify(&second_token, Some(password)).await,
            Ok(VerifyOutcome::PasswordMismatch)
        ));
    }
    for token in [second_token, first_token.clone()] {
        match ctx.reg.verify(&token, None).await {
            Ok(outcome) => {
                let (user, needs_password) = confirmed(outcome);
                assert_eq!(user.id, first.id);
                assert!(needs_password);
            }
            // The second one opened: the account's links ended with the first.
            Err(err) => assert!(matches!(err, AppError::Validation { .. }), "{err:?}"),
        }
    }
    assert!(ctx
        .users
        .get(first.id)
        .await
        .unwrap()
        .email_verified_at
        .is_some());
    // Neither party's password signs in.
    for password in ["squatters password", "owners password"] {
        let refused = ctx
            .auth
            .login("owner@example.com", password, None, None)
            .await;
        assert!(matches!(refused, Err(AppError::Auth { .. })), "{refused:?}");
    }
    // The owner sets one through the mailbox (the reset path) and is in.
    ctx.users
        .set_password(first.id, &hash_password("chosen by the owner").unwrap())
        .await
        .unwrap();
    assert!(ctx
        .auth
        .login("owner@example.com", "chosen by the owner", None, None)
        .await
        .is_ok());
    pool_close(ctx).await;
}

// --- Usernames ------------------------------------------------------------

/// Whether `username` is `base`, a dash and six lowercase base32 characters.
fn generated_from(username: &str, base: &str) -> bool {
    username
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|suffix| {
            suffix.len() == 6
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        })
}

async fn usernames_like(ctx: &Ctx, pattern: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username ILIKE $1")
        .bind(pattern)
        .fetch_one(&ctx.pool)
        .await
        .unwrap()
}

/// Review focus 1, the follow-up half: whatever the address, nothing the
/// visitor sent is reserved, so no later request or public lookup can tell
/// a new address from one with an account.
#[tokio::test]
async fn nothing_a_registration_sends_is_reserved_whatever_the_address() {
    let ctx = setup().await;
    open(&ctx).await;
    UsersService::new(ctx.users.clone())
        .create(CreateUser {
            email: "staff@example.com".to_owned(),
            username: Some("staff".to_owned()),
            display_name: None,
            password: Some("original password".to_owned()),
            role: Role::Editor,
        })
        .await
        .unwrap();
    let (barred, _) = created(
        ctx.reg
            .register(request("barred@example.com", "Barred", "correct horse"))
            .await
            .unwrap(),
    );
    ctx.users.set_suspended(barred.id, true).await.unwrap();
    created(
        ctx.reg
            .register(request("pending@example.com", "Pending", "correct horse"))
            .await
            .unwrap(),
    );

    let mut new_account = None;
    for email in [
        "new@example.com",
        "staff@example.com",
        "pending@example.com",
        "barred@example.com",
    ] {
        // The same display name, and a would-be username, every time.
        let outcome = ctx
            .reg
            .register(request(email, "Probe Name", "correct horse"))
            .await
            .unwrap_or_else(|err| panic!("{email}: {err:?}"));
        if let RegisterOutcome::Created { user, .. } = outcome {
            new_account = Some(user);
        }
        // The name as sent is never anybody's username...
        for sent in ["Probe Name", "probe-name", "probename"] {
            assert!(
                ctx.users
                    .public_author_by_username(sent)
                    .await
                    .unwrap()
                    .is_none(),
                "{email}: {sent}"
            );
            assert_eq!(usernames_like(&ctx, sent).await, 0, "{email}: {sent}");
        }
        // ...and it can be registered again from another new address.
        let other = format!("other-{}@example.com", vyasa_common::next_id_i64());
        assert!(matches!(
            ctx.reg
                .register(request(&other, "Probe Name", "correct horse"))
                .await
                .unwrap(),
            RegisterOutcome::Created { .. }
        ));
    }
    // The one account made has a username nobody could have worked out,
    // and no public page until it is confirmed.
    let user = new_account.expect("the new address made an account");
    assert!(
        generated_from(&user.username, "probe-name"),
        "{}",
        user.username
    );
    assert!(ctx
        .users
        .public_author_by_username(&user.username)
        .await
        .unwrap()
        .is_none());
    ctx.reg.confirm(user.id).await.unwrap();
    assert!(ctx
        .users
        .public_author_by_username(&user.username)
        .await
        .unwrap()
        .is_some());
    // Same display name, different usernames.
    assert_eq!(usernames_like(&ctx, "probe-name-%").await, 5);
    pool_close(ctx).await;
}

#[tokio::test]
async fn without_a_display_name_the_username_is_a_neutral_one() {
    let ctx = setup().await;
    open(&ctx).await;
    for (email, display_name) in [
        ("a@example.com", None),
        ("b@example.com", Some("   ")),
        ("c@example.com", Some("!!!")),
    ] {
        let (user, _) = created(
            ctx.reg
                .register(NewRegistration {
                    display_name: display_name.map(str::to_owned),
                    ..request(email, "", "correct horse")
                })
                .await
                .unwrap(),
        );
        assert!(
            generated_from(&user.username, "member"),
            "{}",
            user.username
        );
        let expected = display_name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or_else(|| user.username.clone(), str::to_owned);
        assert_eq!(user.display_name, expected);
    }
    pool_close(ctx).await;
}

// --- Confirmation links -----------------------------------------------------

/// Review focus 4, and the purpose rule.
#[tokio::test]
async fn a_link_that_is_spent_expired_unknown_purged_or_of_another_purpose_is_refused() {
    let ctx = setup().await;
    open(&ctx).await;
    let bad = |result: Result<_, AppError>| {
        assert!(
            matches!(result, Err(AppError::Validation { .. })),
            "not a plain refusal"
        );
    };

    // Opened twice.
    let (_, token) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .unwrap(),
    );
    ctx.reg.verify(&token, None).await.expect("first open");
    bad(ctx.reg.verify(&token, None).await.map(|_| ()));
    bad(ctx
        .reg
        .verify(&token, Some("correct horse"))
        .await
        .map(|_| ()));

    // Never issued, empty, or the stored hash itself.
    for invented in ["vy-nope", "", &hash_token(&token)] {
        bad(ctx.reg.verify(invented, None).await.map(|_| ()));
        bad(ctx
            .reg
            .verify(invented, Some("correct horse"))
            .await
            .map(|_| ()));
    }

    // Expired.
    let (late, token) = created(
        ctx.reg
            .register(request("b@example.com", "b", "correct horse"))
            .await
            .unwrap(),
    );
    sqlx::query("UPDATE reset_tokens SET expires_at = now() - interval '1 second'")
        .execute(&ctx.pool)
        .await
        .unwrap();
    bad(ctx.reg.verify(&token, None).await.map(|_| ()));
    bad(ctx
        .reg
        .verify(&token, Some("correct horse"))
        .await
        .map(|_| ()));
    assert!(ctx
        .users
        .get(late.id)
        .await
        .unwrap()
        .email_verified_at
        .is_none());

    // A password-reset token does not confirm the account it belongs to.
    ctx.tokens
        .issue(
            late.id,
            TokenPurpose::Reset,
            &hash_token("vy-reset"),
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();
    bad(ctx.reg.verify("vy-reset", None).await.map(|_| ()));
    assert!(ctx
        .users
        .get(late.id)
        .await
        .unwrap()
        .email_verified_at
        .is_none());
    // Nor does a confirmation token reset a password.
    let ResendOutcome::Send { token, .. } = ctx.reg.resend("b@example.com").await.unwrap() else {
        panic!("expected a resend");
    };
    assert!(ctx
        .users
        .consume_reset_token(&hash_token(&token))
        .await
        .is_err());

    // The account was purged before the link was opened.
    sqlx::query("UPDATE users SET created_at = now() - interval '8 days' WHERE id = $1")
        .bind(late.id)
        .execute(&ctx.pool)
        .await
        .unwrap();
    let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
    assert_eq!(ctx.users.purge_unconfirmed(cutoff).await.unwrap(), 1);
    bad(ctx.reg.verify(&token, None).await.map(|_| ()));
    pool_close(ctx).await;
}

#[tokio::test]
async fn resend_only_ever_mails_an_unconfirmed_account() {
    let ctx = setup().await;
    open(&ctx).await;
    let (user, first) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .unwrap(),
    );
    let ResendOutcome::Send {
        user: target,
        token,
        needs_password,
    } = ctx.reg.resend(" A@example.com").await.unwrap()
    else {
        panic!("expected a resend");
    };
    assert_eq!(target.id, user.id);
    assert!(!needs_password);
    assert_ne!(token, first);

    // Nobody by that address, or not an address at all.
    for unknown in ["nobody@example.com", "", "   ", "not an address"] {
        assert!(matches!(
            ctx.reg.resend(unknown).await.unwrap(),
            ResendOutcome::Nothing
        ));
    }
    // Suspended: nothing.
    ctx.users.set_suspended(user.id, true).await.unwrap();
    assert!(matches!(
        ctx.reg.resend("a@example.com").await.unwrap(),
        ResendOutcome::Nothing
    ));
    ctx.users.set_suspended(user.id, false).await.unwrap();
    // Confirmed: nothing.
    ctx.reg.verify(&token, None).await.unwrap();
    assert!(matches!(
        ctx.reg.resend("a@example.com").await.unwrap(),
        ResendOutcome::Nothing
    ));
    pool_close(ctx).await;
}

#[tokio::test]
async fn an_administrator_can_confirm_or_resend_for_an_account() {
    let ctx = setup().await;
    open(&ctx).await;
    let (user, token) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .unwrap(),
    );
    let fresh = ctx.reg.issue_confirmation(&user).await.expect("resend");
    assert_ne!(fresh, token);

    let vouched = ctx.reg.confirm(user.id).await.expect("confirm");
    assert!(vouched.email_verified_at.is_some());
    // An administrator vouches for the mailbox, not for the password a
    // registrant typed: it is removed, and the owner sets one by mail.
    assert!(vouched.password_hash.is_none());
    assert!(matches!(
        ctx.auth
            .login("a@example.com", "correct horse", None, None)
            .await,
        Err(AppError::Auth { .. })
    ));
    // The mailed links are dead once someone vouched for the address.
    assert!(ctx.reg.verify(&token, None).await.is_err());
    assert!(ctx.reg.verify(&fresh, Some("correct horse")).await.is_err());
    // There is nothing to resend for a confirmed account.
    assert!(matches!(
        ctx.reg.issue_confirmation(&vouched).await,
        Err(AppError::Validation { .. })
    ));
    assert!(matches!(
        ctx.reg.confirm(404).await,
        Err(AppError::NotFound { .. })
    ));
    // Confirming a confirmed account again leaves its password alone.
    ctx.users
        .set_password(user.id, &hash_password("set by the owner").unwrap())
        .await
        .unwrap();
    let again = ctx.reg.confirm(user.id).await.unwrap();
    assert!(again.password_hash.is_some());
    assert_eq!(again.email_verified_at, vouched.email_verified_at);
    assert!(ctx
        .auth
        .login("a@example.com", "set by the owner", None, None)
        .await
        .is_ok());
    pool_close(ctx).await;
}

// --- The password at confirmation (final review C1) -------------------------

/// One registration by a stranger, and the link opened by anyone (the
/// owner, or a mail scanner that follows links): the stranger's password
/// must not become a working credential.
#[tokio::test]
async fn confirming_without_the_password_removes_it() {
    let ctx = setup().await;
    open(&ctx).await;
    let (user, token) = created(
        ctx.reg
            .register(request(
                "victim@example.com",
                "squatter",
                "squatters password",
            ))
            .await
            .unwrap(),
    );
    let (confirmed_user, needs_password) = confirmed(ctx.reg.verify(&token, None).await.unwrap());
    assert_eq!(confirmed_user.id, user.id);
    assert!(confirmed_user.email_verified_at.is_some());
    assert!(confirmed_user.password_hash.is_none());
    assert!(needs_password, "the caller mails a set-password link");
    assert!(ctx
        .users
        .get(user.id)
        .await
        .unwrap()
        .password_hash
        .is_none());
    assert!(matches!(
        ctx.auth
            .login("victim@example.com", "squatters password", None, None)
            .await,
        Err(AppError::Auth { .. })
    ));
    pool_close(ctx).await;
}

#[tokio::test]
async fn a_wrong_password_at_confirmation_changes_nothing_and_spends_nothing() {
    let ctx = setup().await;
    open(&ctx).await;
    let (user, token) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .unwrap(),
    );
    for wrong in ["incorrect horse", "", "correct horse "] {
        assert!(
            matches!(
                ctx.reg.verify(&token, Some(wrong)).await,
                Ok(VerifyOutcome::PasswordMismatch)
            ),
            "{wrong:?}"
        );
    }
    let stored = ctx.users.get(user.id).await.unwrap();
    assert!(stored.email_verified_at.is_none());
    assert_eq!(stored.password_hash, user.password_hash);
    // The link still works, with the right password.
    let (_, needs_password) =
        confirmed(ctx.reg.verify(&token, Some("correct horse")).await.unwrap());
    assert!(!needs_password);
    assert!(ctx
        .auth
        .login("a@example.com", "correct horse", None, None)
        .await
        .is_ok());
    pool_close(ctx).await;
}

/// The password is checked and kept in one step: a contest that clears it
/// between the check and the confirmation does not let it through.
#[tokio::test]
async fn a_password_cleared_after_the_check_is_not_kept() {
    let ctx = setup().await;
    open(&ctx).await;
    let (user, token) = created(
        ctx.reg
            .register(request("a@example.com", "a", "correct horse"))
            .await
            .unwrap(),
    );
    let stored = user.password_hash.clone().unwrap();
    ctx.users.clear_unconfirmed_password(user.id).await.unwrap();
    // The storage step refuses to keep a hash that is no longer there.
    assert!(ctx
        .users
        .confirm_email_with_token(
            &hash_token(&token),
            vyasa_db::repo::ConfirmPassword::KeepIf(&stored)
        )
        .await
        .unwrap()
        .is_none());
    assert!(ctx
        .users
        .get(user.id)
        .await
        .unwrap()
        .email_verified_at
        .is_none());
    assert!(matches!(
        ctx.reg.verify(&token, Some("correct horse")).await,
        Ok(VerifyOutcome::PasswordMismatch)
    ));
    pool_close(ctx).await;
}
