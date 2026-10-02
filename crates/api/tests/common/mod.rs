//! Helpers shared by the API integration tests. Each test binary compiles
//! this module separately and uses a subset, hence the allow.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicI64, Ordering};

use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, UsersRepo};

/// The server binary built for these tests.
pub const BIN: &str = env!("CARGO_BIN_EXE_vyasa");

/// Every seeded user gets this password.
pub const PASSWORD: &str = "pw-secret-1";

static NEXT_ID: AtomicI64 = AtomicI64::new(1_000);

/// A user created by [`seed_user`].
pub struct Seeded {
    pub id: i64,
    pub email: String,
    pub password: String,
}

/// Inserts a user with `role` and [`PASSWORD`]. Ids and emails are unique
/// within the process, so one test may seed several users.
pub async fn seed_user(pool: &PgPool, role: Role) -> Seeded {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let email = format!("user{id}@example.com");
    let username = format!("user{id}");
    let hash = vyasa_core::user::password::hash_password(PASSWORD).expect("hash");
    UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id,
            email: &email,
            username: &username,
            display_name: &username,
            password_hash: Some(&hash),
            role,
            bio: "",
        })
        .await
        .expect("seed user");
    Seeded {
        id,
        email,
        password: PASSWORD.to_owned(),
    }
}

/// Signs in and returns the session cookie as `name=value`.
pub fn login_cookie(base: &str, email: &str, password: &str) -> String {
    let resp = ureq::post(&format!("{base}/api/v1/auth/login"))
        .send_json(serde_json::json!({ "email": email, "password": password }))
        .expect("login request");
    assert_eq!(resp.status(), 200, "login {email}");
    resp.all("set-cookie")
        .into_iter()
        .find_map(|c| c.split(';').next().map(str::to_owned))
        .expect("a session cookie")
}
