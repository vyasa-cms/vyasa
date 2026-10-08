//! Boot-time steps that make a container start with nothing but
//! environment variables: the first administrator from the environment.

use sqlx::PgPool;
use vyasa_common::AppError;
use vyasa_db::repo::{OptionsRepo, UsersRepo};

/// What happened to `VYASA_ADMIN_EMAIL` / `VYASA_ADMIN_PASSWORD`.
#[derive(Debug, PartialEq, Eq)]
pub enum FirstAdmin {
    /// An administrator with this email was created.
    Created(String),
    /// Users already exist; the variables were ignored.
    UsersExist,
    /// Neither variable is set.
    NotConfigured,
    /// Only one of the two is set; ignored.
    HalfConfigured,
}

/// Creates the first administrator from `VYASA_ADMIN_EMAIL` and
/// `VYASA_ADMIN_PASSWORD` when both are set and no user exists, exactly as
/// `vyasa admin create` does, and marks setup `done`. The password is
/// never logged.
///
/// # Errors
///
/// A weak password or an invalid email is a validation error the boot
/// must stop on; database failures pass through.
pub async fn first_admin_from_env(pool: &PgPool) -> Result<FirstAdmin, AppError> {
    let email = std::env::var("VYASA_ADMIN_EMAIL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let password = std::env::var("VYASA_ADMIN_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty());
    let (email, password) = match (email, password) {
        (Some(e), Some(p)) => (e, p),
        (None, None) => return Ok(FirstAdmin::NotConfigured),
        _ => return Ok(FirstAdmin::HalfConfigured),
    };
    if UsersRepo::new(pool.clone()).count().await? > 0 {
        return Ok(FirstAdmin::UsersExist);
    }
    let users = vyasa_core::user::UsersService::new(UsersRepo::new(pool.clone()));
    let user = match users
        .create(vyasa_core::user::CreateUser {
            email: email.trim().to_owned(),
            username: None,
            display_name: None,
            password: Some(password),
            role: vyasa_db::models::Role::Admin,
        })
        .await
    {
        Ok(user) => user,
        // Two instances booting together both saw an empty table; the one
        // that lost the insert is in the same place as "users exist".
        Err(AppError::Conflict { .. }) => return Ok(FirstAdmin::UsersExist),
        Err(err) => return Err(err),
    };
    if let Err(err) = OptionsRepo::new(pool.clone())
        .set(
            crate::setup::PROGRESS_KEY,
            &serde_json::Value::String("done".to_owned()),
        )
        .await
    {
        // Nothing half-created: the next boot starts from an empty table.
        let _ = UsersRepo::new(pool.clone()).delete(user.id).await;
        return Err(err);
    }
    Ok(FirstAdmin::Created(user.email))
}
