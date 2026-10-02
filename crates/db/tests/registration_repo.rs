//! Storage for public registration: confirmed addresses, purpose-bound
//! tokens, the API-key refusal and the purge of stale unconfirmed accounts.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use chrono::{Duration, Utc};
use vyasa_db::models::{Role, TokenPurpose};
use vyasa_db::repo::{
    ApiKeysRepo, ConfirmPassword, NewRole, NewUser, RolesRepo, TokensRepo, UnconfirmedInsertError,
    UsersRepo,
};
use vyasa_testkit::TestDb;

fn new_user<'a>(id: i64, email: &'a str, username: &'a str) -> NewUser<'a> {
    NewUser {
        id,
        email,
        username,
        display_name: username,
        password_hash: Some("argon2-hash"),
        role: Role::Subscriber,
        bio: "",
    }
}

async fn age(pool: &sqlx::PgPool, id: i64, days: i64) {
    sqlx::query("UPDATE users SET created_at = now() - ($2 * interval '1 day') WHERE id = $1")
        .bind(id)
        .bind(days)
        .execute(pool)
        .await
        .expect("age");
}

/// Runs the migration's own SQL against a schema put back to what it was
/// before it, with accounts and a token already there.
#[tokio::test]
async fn the_migration_confirms_every_existing_account_and_marks_old_tokens_reset() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    sqlx::raw_sql(
        "DROP INDEX users_unconfirmed_idx;
         ALTER TABLE users DROP COLUMN email_verified_at;
         ALTER TABLE reset_tokens DROP COLUMN purpose;
         INSERT INTO users (id, email, username, display_name, role, created_at, updated_at)
         VALUES (1, 'old@example.com', 'old', 'Old', 'admin',
                 '2024-01-02T03:04:05Z', '2024-02-03T04:05:06Z'),
                (2, 'older@example.com', 'older', 'Older', 'subscriber',
                 '2023-01-02T03:04:05Z', '2023-02-03T04:05:06Z');
         INSERT INTO reset_tokens (id, user_id, token_hash, expires_at)
         VALUES (10, 1, 'pending', now() + interval '1 hour');",
    )
    .execute(&pool)
    .await
    .expect("rewind");

    sqlx::raw_sql(include_str!("../migrations/0050_registration.sql"))
        .execute(&pool)
        .await
        .expect("migrate");

    let rows: Vec<(i64, bool, bool)> = sqlx::query_as(
        "SELECT id, email_verified_at = created_at,
                updated_at IN ('2024-02-03T04:05:06Z', '2023-02-03T04:05:06Z')
         FROM users ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![(1, true, true), (2, true, true)]);
    let purpose: String = sqlx::query_scalar("SELECT purpose FROM reset_tokens WHERE id = 10")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(purpose, "reset");
    // The token issued before the migration still resets a password.
    let repo = UsersRepo::new(pool.clone());
    assert_eq!(repo.consume_reset_token("pending").await.unwrap(), 1);
    pool.close().await;
}

#[tokio::test]
async fn only_registration_makes_an_unconfirmed_account() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    RolesRepo::new(pool.clone())
        .insert(&NewRole {
            slug: "member",
            name: "Member",
            description: "",
            capabilities: &["view_admin".to_owned()],
        })
        .await
        .expect("role");

    // The ordinary paths (administrator, invitation, wizard, CLI, import).
    let plain = repo
        .insert(&new_user(1, "a@example.com", "a"))
        .await
        .unwrap();
    assert!(plain.email_verified_at.is_some());
    let custom = repo
        .insert_with_custom_role(&new_user(2, "b@example.com", "b"), "member")
        .await
        .unwrap();
    assert!(custom.email_verified_at.is_some());
    // A raw insert that has never heard of the column.
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES (3, 'c@example.com', 'c', 'C', 'author')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo.get(3).await.unwrap().email_verified_at.is_some());

    // Registration.
    let pending = repo
        .insert_unconfirmed(&new_user(4, "d@example.com", "d"), None)
        .await
        .unwrap();
    assert!(pending.email_verified_at.is_none());
    assert_eq!(pending.role, Role::Subscriber);
    let pending_custom = repo
        .insert_unconfirmed(&new_user(5, "e@example.com", "e"), Some("member"))
        .await
        .unwrap();
    assert!(pending_custom.email_verified_at.is_none());
    assert_eq!(pending_custom.custom_role.as_deref(), Some("member"));
    // A role that is not there leaves no account behind.
    let missing = repo
        .insert_unconfirmed(&new_user(6, "f@example.com", "f"), Some("gone"))
        .await;
    assert!(matches!(missing, Err(UnconfirmedInsertError::RoleMissing)));
    assert!(repo.get(6).await.is_err());

    // Which uniqueness rule refused a row is said by the database, not
    // read out of a message.
    for (email, username, email_clash) in [
        ("D@example.com", "fresh", true),
        ("fresh@example.com", "D", false),
    ] {
        let clash = repo
            .insert_unconfirmed(&new_user(7, email, username), None)
            .await;
        match clash {
            Err(UnconfirmedInsertError::EmailTaken) => assert!(email_clash),
            Err(UnconfirmedInsertError::UsernameTaken) => assert!(!email_clash),
            other => panic!("{email} {username}: {other:?}"),
        }
    }
    // A clash that is neither (the id) is not mistaken for one.
    let same_id = repo
        .insert_unconfirmed(&new_user(4, "g@example.com", "g"), None)
        .await;
    assert!(matches!(same_id, Err(UnconfirmedInsertError::Other(_))));
    pool.close().await;
}

#[tokio::test]
async fn a_token_is_only_good_for_the_purpose_it_was_issued_for() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let tokens = TokensRepo::new(pool.clone());
    repo.insert_unconfirmed(&new_user(1, "a@example.com", "a"), None)
        .await
        .unwrap();

    tokens
        .issue(1, TokenPurpose::Verify, "verify-hash", Duration::hours(24))
        .await
        .unwrap();
    tokens
        .issue(1, TokenPurpose::Reset, "reset-hash", Duration::hours(1))
        .await
        .unwrap();

    // What a per-account mail limit counts.
    assert_eq!(
        tokens
            .issued_since(1, TokenPurpose::Verify, Duration::hours(1))
            .await
            .unwrap(),
        1
    );

    // Presented for the other purpose, neither is accepted -- and neither
    // is spent by the attempt.
    assert_eq!(
        tokens
            .consume("verify-hash", TokenPurpose::Reset)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        tokens
            .consume("reset-hash", TokenPurpose::Verify)
            .await
            .unwrap(),
        None
    );
    assert!(repo.consume_reset_token("verify-hash").await.is_err());
    assert!(repo
        .confirm_email_with_token("reset-hash", ConfirmPassword::Clear)
        .await
        .unwrap()
        .is_none());

    // Spending a reset token on its own does not confirm the account
    // (`reset_password_with_token`, which also sets the password, does).
    assert_eq!(repo.consume_reset_token("reset-hash").await.unwrap(), 1);
    assert!(repo.get(1).await.unwrap().email_verified_at.is_none());

    // For its own purpose each works, once.
    assert_eq!(
        tokens
            .consume("verify-hash", TokenPurpose::Verify)
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        tokens
            .consume("verify-hash", TokenPurpose::Verify)
            .await
            .unwrap(),
        None
    );

    // The lifetime is the one asked for.
    tokens
        .issue(1, TokenPurpose::Verify, "day", Duration::hours(24))
        .await
        .unwrap();
    let hours: f64 = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM (expires_at - now())) / 3600)::float8
         FROM reset_tokens WHERE token_hash = 'day'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((23.9..=24.0).contains(&hours), "{hours}");
    sqlx::query("UPDATE reset_tokens SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        tokens.consume("day", TokenPurpose::Verify).await.unwrap(),
        None
    );
    pool.close().await;
}

#[tokio::test]
async fn confirming_with_a_token_is_single_use_and_ends_the_other_links() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let tokens = TokensRepo::new(pool.clone());
    repo.insert_unconfirmed(&new_user(1, "a@example.com", "a"), None)
        .await
        .unwrap();
    for hash in ["first", "second"] {
        tokens
            .issue(1, TokenPurpose::Verify, hash, Duration::hours(24))
            .await
            .unwrap();
    }
    tokens
        .issue(1, TokenPurpose::Reset, "reset", Duration::hours(1))
        .await
        .unwrap();

    // Kept only while it is still the hash the caller checked.
    assert!(repo
        .confirm_email_with_token("first", ConfirmPassword::KeepIf("another-hash"))
        .await
        .unwrap()
        .is_none());
    assert!(repo.get(1).await.unwrap().email_verified_at.is_none());
    let confirmed = repo
        .confirm_email_with_token("first", ConfirmPassword::KeepIf("argon2-hash"))
        .await
        .unwrap()
        .expect("confirmed");
    assert_eq!(confirmed.id, 1);
    assert!(confirmed.email_verified_at.is_some());
    assert_eq!(confirmed.password_hash.as_deref(), Some("argon2-hash"));
    // The link itself, and the other confirmation link, are spent.
    for hash in ["first", "second", "invented"] {
        assert!(repo
            .confirm_email_with_token(hash, ConfirmPassword::Clear)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .confirmation_token_holder(hash)
            .await
            .unwrap()
            .is_none());
    }
    // A pending password reset is a different thing and is left alone.
    assert_eq!(repo.consume_reset_token("reset").await.unwrap(), 1);

    // An administrator's confirmation: no token, same end state.
    repo.insert_unconfirmed(&new_user(2, "b@example.com", "b"), None)
        .await
        .unwrap();
    tokens
        .issue(2, TokenPurpose::Verify, "third", Duration::hours(24))
        .await
        .unwrap();
    let by_hand = repo.confirm_email_clearing_password(2).await.unwrap();
    assert!(by_hand.email_verified_at.is_some());
    assert!(
        by_hand.password_hash.is_none(),
        "vouching is not for the password"
    );
    assert!(repo
        .confirm_email_with_token("third", ConfirmPassword::Clear)
        .await
        .unwrap()
        .is_none());
    // Confirming again keeps the first time, and a password set since.
    repo.set_password(2, "set-since").await.unwrap();
    let again = repo.confirm_email_clearing_password(2).await.unwrap();
    assert_eq!(again.email_verified_at, by_hand.email_verified_at);
    assert_eq!(again.password_hash.as_deref(), Some("set-since"));
    assert!(repo.confirm_email_clearing_password(404).await.is_err());
    pool.close().await;
}

#[tokio::test]
async fn a_confirmation_link_opened_without_the_password_clears_it() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let tokens = TokensRepo::new(pool.clone());
    repo.insert_unconfirmed(&new_user(1, "a@example.com", "a"), None)
        .await
        .unwrap();
    tokens
        .issue(1, TokenPurpose::Verify, "link", Duration::hours(24))
        .await
        .unwrap();
    // Looking does not spend it.
    let holder = repo.confirmation_token_holder("link").await.unwrap();
    assert_eq!(holder.map(|u| u.id), Some(1));
    assert!(repo
        .confirmation_token_holder("link")
        .await
        .unwrap()
        .is_some());
    assert!(repo
        .confirmation_token_holder("invented")
        .await
        .unwrap()
        .is_none());

    let confirmed = repo
        .confirm_email_with_token("link", ConfirmPassword::Clear)
        .await
        .unwrap()
        .expect("confirmed");
    assert!(confirmed.email_verified_at.is_some());
    assert!(confirmed.password_hash.is_none());
    pool.close().await;
}

#[tokio::test]
async fn a_reset_through_the_mailbox_sets_the_password_and_confirms_the_address() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let tokens = TokensRepo::new(pool.clone());
    repo.insert_unconfirmed(&new_user(1, "a@example.com", "a"), None)
        .await
        .unwrap();
    tokens
        .issue(1, TokenPurpose::Verify, "link", Duration::hours(24))
        .await
        .unwrap();
    repo.create_reset_token(1, "reset").await.unwrap();
    // A confirmation token is not a reset token.
    assert!(repo
        .reset_password_with_token("link", "new-hash")
        .await
        .is_err());

    let done = repo
        .reset_password_with_token("reset", "new-hash")
        .await
        .unwrap();
    assert_eq!(done.user_id, 1);
    assert!(done.confirmed_now);
    let user = repo.get(1).await.unwrap();
    assert!(user.email_verified_at.is_some());
    assert_eq!(user.password_hash.as_deref(), Some("new-hash"));
    // The confirmation links ended with it; the reset link is spent.
    assert!(repo
        .confirmation_token_holder("link")
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .reset_password_with_token("reset", "other-hash")
        .await
        .is_err());

    // A confirmed account is reset as before and keeps its first
    // confirmation time.
    repo.create_reset_token(1, "again").await.unwrap();
    let done = repo
        .reset_password_with_token("again", "newer-hash")
        .await
        .unwrap();
    assert!(!done.confirmed_now);
    let after = repo.get(1).await.unwrap();
    assert_eq!(after.email_verified_at, user.email_verified_at);
    assert_eq!(after.password_hash.as_deref(), Some("newer-hash"));
    pool.close().await;
}

#[tokio::test]
async fn a_password_is_only_cleared_on_an_unconfirmed_account() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    repo.insert(&new_user(1, "a@example.com", "a"))
        .await
        .unwrap();
    repo.insert_unconfirmed(&new_user(2, "b@example.com", "b"), None)
        .await
        .unwrap();

    assert!(!repo.clear_unconfirmed_password(1).await.unwrap());
    assert!(repo.get(1).await.unwrap().password_hash.is_some());
    assert!(repo.clear_unconfirmed_password(2).await.unwrap());
    assert!(repo.get(2).await.unwrap().password_hash.is_none());
    pool.close().await;
}

#[tokio::test]
async fn an_unconfirmed_owners_api_key_does_not_resolve() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let keys = ApiKeysRepo::new(pool.clone());
    repo.insert_unconfirmed(&new_user(1, "a@example.com", "a"), None)
        .await
        .unwrap();
    keys.insert(7, 1, "k", "key-hash", &serde_json::json!([]))
        .await
        .unwrap();

    assert!(matches!(
        keys.resolve("key-hash").await,
        Err(vyasa_common::AppError::NotFound { .. })
    ));
    repo.confirm_email_clearing_password(1).await.unwrap();
    assert_eq!(keys.resolve("key-hash").await.unwrap().1.id, 1);
    pool.close().await;
}

#[tokio::test]
async fn the_purge_deletes_only_stale_unconfirmed_accounts_that_own_nothing() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    let tokens = TokensRepo::new(pool.clone());

    // Stale and unconfirmed: goes, with its token.
    repo.insert_unconfirmed(&new_user(1, "stale@example.com", "stale"), None)
        .await
        .unwrap();
    tokens
        .issue(1, TokenPurpose::Verify, "stale-link", Duration::hours(24))
        .await
        .unwrap();
    age(&pool, 1, 8).await;
    // Unconfirmed but recent: stays.
    repo.insert_unconfirmed(&new_user(2, "fresh@example.com", "fresh"), None)
        .await
        .unwrap();
    age(&pool, 2, 6).await;
    // Old but confirmed: stays.
    repo.insert(&new_user(3, "member@example.com", "member"))
        .await
        .unwrap();
    age(&pool, 3, 400).await;
    // Stale, unconfirmed, and somehow the author of a post, a revision or
    // a media item: skipped, never reassigned.
    for (id, name) in [(4, "poster"), (5, "reviser"), (6, "uploader")] {
        repo.insert_unconfirmed(&new_user(id, &format!("{name}@example.com"), name), None)
            .await
            .unwrap();
        age(&pool, id, 30).await;
    }
    sqlx::raw_sql(
        "INSERT INTO posts (id, type, status, title, slug, content, author_id)
         VALUES (100, 'post', 'draft', 'T', 't', '[]'::jsonb, 4);
         INSERT INTO post_revisions (id, post_id, author_id, title, content)
         VALUES (200, 100, 5, 'T', '[]'::jsonb);
         INSERT INTO media (id, owner_id, file_name, mime, byte_size, path)
         VALUES (300, 6, 'f.png', 'image/png', 1, 'k');",
    )
    .execute(&pool)
    .await
    .expect("content");
    // Stale and unconfirmed but an administrator (made by hand, by
    // someone): never purged.
    repo.insert_unconfirmed(&new_user(7, "root@example.com", "root"), None)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = 7")
        .execute(&pool)
        .await
        .unwrap();
    age(&pool, 7, 30).await;

    let cutoff = Utc::now() - Duration::days(7);
    assert_eq!(repo.purge_unconfirmed(cutoff).await.unwrap(), 1);
    let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM users ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![2, 3, 4, 5, 6, 7]);
    // The purged account's link is gone with it: opening it is "no such
    // link", not an error.
    assert!(repo
        .confirm_email_with_token("stale-link", ConfirmPassword::Clear)
        .await
        .unwrap()
        .is_none());
    // Nothing left to do the second time.
    assert_eq!(repo.purge_unconfirmed(cutoff).await.unwrap(), 0);
    pool.close().await;
}

/// Rows that can never go must not stand in front of the ones that can,
/// however small the batch.
#[tokio::test]
async fn rows_the_purge_cannot_delete_do_not_hold_up_the_rest() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    for id in 1..=8 {
        let name = format!("u{id}");
        repo.insert_unconfirmed(&new_user(id, &format!("{name}@example.com"), &name), None)
            .await
            .unwrap();
        age(&pool, id, 60 - id).await;
    }
    // The lowest ids: an administrator, an author, and two the database
    // itself refuses to delete (another table points at them).
    sqlx::raw_sql(
        "UPDATE users SET role = 'admin' WHERE id = 1;
         INSERT INTO posts (id, type, status, title, slug, content, author_id)
         VALUES (100, 'post', 'draft', 'T', 't', '[]'::jsonb, 2);
         CREATE TABLE ledger (user_id BIGINT NOT NULL REFERENCES users (id));
         INSERT INTO ledger VALUES (3), (4);",
    )
    .execute(&pool)
    .await
    .expect("obstacles");

    let cutoff = Utc::now() - Duration::days(7);
    // One call, batches of two: the rows that cannot go come first, and
    // the four behind them still go.
    assert_eq!(
        repo.purge_unconfirmed_limited(cutoff, 2, 100)
            .await
            .unwrap(),
        4
    );
    let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM users ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![1, 2, 3, 4]);
    assert_eq!(repo.purge_unconfirmed(cutoff).await.unwrap(), 0);
    pool.close().await;
}

#[tokio::test]
async fn an_unconfirmed_account_has_no_public_author_page() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    repo.insert(&new_user(1, "a@example.com", "ada"))
        .await
        .unwrap();
    repo.insert_unconfirmed(&new_user(2, "b@example.com", "pending"), None)
        .await
        .unwrap();

    for spelling in ["ada", "ADA"] {
        let found = repo.public_author_by_username(spelling).await.unwrap();
        assert_eq!(found.map(|u| u.id), Some(1), "{spelling}");
    }
    // Nor is its name on anything public.
    let names = repo.display_names(&[1, 2]).await.unwrap();
    assert_eq!(names.keys().copied().collect::<Vec<_>>(), vec![1]);
    for hidden in ["pending", "nobody"] {
        assert!(repo
            .public_author_by_username(hidden)
            .await
            .unwrap()
            .is_none());
    }
    repo.confirm_email_clearing_password(2).await.unwrap();
    assert_eq!(repo.display_names(&[1, 2]).await.unwrap().len(), 2);
    assert!(repo
        .public_author_by_username("pending")
        .await
        .unwrap()
        .is_some());
    pool.close().await;
}

/// Content is never handed to an account that could not have written it:
/// one that has not confirmed its address, or one that is suspended.
#[tokio::test]
async fn content_is_only_reassigned_to_an_active_confirmed_account() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    repo.insert(&NewUser {
        role: Role::Admin,
        ..new_user(1, "root@example.com", "root")
    })
    .await
    .unwrap();
    repo.insert(&new_user(2, "leaving@example.com", "leaving"))
        .await
        .unwrap();
    repo.insert_unconfirmed(&new_user(3, "pending@example.com", "pending"), None)
        .await
        .unwrap();
    repo.insert(&new_user(4, "barred@example.com", "barred"))
        .await
        .unwrap();
    repo.set_suspended(4, true).await.unwrap();
    repo.insert(&new_user(5, "heir@example.com", "heir"))
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO posts (id, type, status, title, slug, content, author_id)
         VALUES (100, 'post', 'published', 'T', 't', '[]'::jsonb, 2);",
    )
    .execute(&pool)
    .await
    .unwrap();

    for heir in [3, 4] {
        let refused = repo.delete_reassigning(2, Some(heir)).await;
        match refused {
            Err(vyasa_common::AppError::Validation { message }) => assert_eq!(
                message,
                "content can only be reassigned to an active, confirmed account"
            ),
            other => panic!("heir {heir}: {other:?}"),
        }
        assert!(repo.get(2).await.is_ok(), "nothing deleted");
        let author: i64 = sqlx::query_scalar("SELECT author_id FROM posts WHERE id = 100")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(author, 2, "nothing moved");
    }
    repo.delete_reassigning(2, Some(5)).await.expect("to heir");
    let author: i64 = sqlx::query_scalar("SELECT author_id FROM posts WHERE id = 100")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(author, 5);
    pool.close().await;
}

/// However many are due, one run deletes at most `max`; the rest go next
/// time.
#[tokio::test]
async fn one_purge_run_stops_at_its_ceiling() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = UsersRepo::new(pool.clone());
    for id in 1..=5 {
        let name = format!("u{id}");
        repo.insert_unconfirmed(&new_user(id, &format!("{name}@example.com"), &name), None)
            .await
            .unwrap();
        age(&pool, id, 30).await;
    }
    let cutoff = Utc::now() - Duration::days(7);
    assert_eq!(
        repo.purge_unconfirmed_limited(cutoff, 2, 3).await.unwrap(),
        3
    );
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 2);
    assert_eq!(
        repo.purge_unconfirmed_limited(cutoff, 2, 3).await.unwrap(),
        2
    );
    assert_eq!(repo.purge_unconfirmed(cutoff).await.unwrap(), 0);
    pool.close().await;
}
