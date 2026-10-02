//! Sessions: ending every one a user holds.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, SessionsRepo, UsersRepo};
use vyasa_testkit::TestDb;

#[tokio::test]
async fn revoking_a_user_ends_all_of_their_sessions_and_nobody_elses() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let users = UsersRepo::new(pool.clone());
    for (id, name) in [(95_001, "victim"), (95_002, "bystander")] {
        users
            .insert(&NewUser {
                id,
                email: &format!("{name}@example.com"),
                username: name,
                display_name: name,
                password_hash: None,
                role: Role::Author,
                bio: "",
            })
            .await
            .expect("user");
    }

    // The victim is signed in on two devices; someone else on one.
    let sessions = SessionsRepo::new(pool.clone());
    let ttl = chrono::Duration::hours(1);
    for token in ["victim-laptop", "victim-phone"] {
        sessions
            .insert(token, 95_001, ttl, None, None)
            .await
            .expect("session");
    }
    sessions
        .insert("bystander-laptop", 95_002, ttl, None, None)
        .await
        .expect("session");

    // A password reset without this left a stolen session alive for as
    // long as it had left to run: the credential was changed and the
    // access it had granted was not.
    let ended = sessions.delete_for_user(95_001).await.expect("revoke");
    assert_eq!(ended, 2);
    assert_eq!(sessions.count_for_user(95_001).await.unwrap(), 0);
    assert!(
        sessions.get_live("victim-phone").await.is_err(),
        "a revoked session must not authenticate"
    );
    // And it is per user, not a purge.
    assert_eq!(sessions.count_for_user(95_002).await.unwrap(), 1);
    assert!(sessions.get_live("bystander-laptop").await.is_ok());
}
