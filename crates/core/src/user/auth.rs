//! Authentication service: login verification and session lifecycle.

use chrono::Duration;
use rand::distributions::Alphanumeric;
use rand::Rng;
use vyasa_common::AppError;
use vyasa_db::models::UserRow;
use vyasa_db::repo::{SessionsRepo, UsersRepo};

use super::password::{self, DUMMY_HASH};

/// What every refused sign-in says, whatever the reason: no account, a
/// wrong password, or (mapped by the API) an unconfirmed address. One
/// sentence, so no answer can be told apart from another.
pub const INVALID_CREDENTIALS: &str = "invalid email or password";

/// Session lifetime for browser logins.
pub const SESSION_TTL: Duration = Duration::days(14);

/// A live session: the user plus the opaque session token.
#[derive(Clone, Debug)]
pub struct AuthSession {
    /// Opaque session token (cookie value).
    pub token: String,
    /// Authenticated user.
    pub user: UserRow,
}

/// Login and session management.
#[derive(Clone, Debug)]
pub struct AuthService {
    users: UsersRepo,
    sessions: SessionsRepo,
}

impl AuthService {
    /// Creates a service over the given repositories.
    #[must_use]
    pub fn new(users: UsersRepo, sessions: SessionsRepo) -> Self {
        Self { users, sessions }
    }

    /// Verifies credentials and creates a session.
    ///
    /// Fails uniformly with [`AppError::Auth`] for unknown email and wrong
    /// password alike (no user enumeration). To keep timing uniform, an
    /// unknown email still runs one argon2 verification against a dummy
    /// hash.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Auth`] on bad credentials,
    /// [`AppError::EmailUnconfirmed`] when the credentials are right but
    /// the account has not confirmed its address (no session is made), or
    /// [`AppError::Db`] on database failure.
    pub async fn login(
        &self,
        email: &str,
        password_attempt: &str,
        user_agent: Option<&str>,
        ip: Option<&str>,
    ) -> Result<AuthSession, AppError> {
        // Trimmed as every per-address count trims it: " a@x" is the
        // account the lockout counts the attempt towards. Case is the
        // column's (`citext`).
        let user = self.users.get_by_email(email.trim()).await;
        match user {
            Ok(user) => {
                let hash = user.password_hash.clone().unwrap_or_default();
                if hash.is_empty() {
                    // Passwordless account (invited, never activated):
                    // burn the same argon2 work to stay time-uniform.
                    drop(password::verify_password(password_attempt, DUMMY_HASH));
                    return Err(AppError::auth(INVALID_CREDENTIALS));
                }
                password::verify_password(password_attempt, &hash)?;
                if user.suspended_at.is_some() {
                    return Err(AppError::auth(
                        "this account is suspended; ask an administrator",
                    ));
                }
                // Said only now that the password has checked out, so it
                // tells nobody which addresses have accounts.
                ensure_confirmed(&user)?;
                // Best effort: a failed stamp must not fail the sign-in.
                let _ = self.users.touch_login(user.id).await;
                let token = new_session_token();
                self.sessions
                    .insert(&token, user.id, SESSION_TTL, user_agent, ip)
                    .await?;
                Ok(AuthSession { token, user })
            }
            Err(AppError::NotFound { .. }) => {
                // Unknown email: same failure, same work.
                drop(password::verify_password(password_attempt, DUMMY_HASH));
                Err(AppError::auth(INVALID_CREDENTIALS))
            }
            Err(err) => Err(err),
        }
    }

    /// Resolves a session token to its user.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Auth`] when the token is unknown or expired, or
    /// its user is suspended, unconfirmed or gone.
    pub async fn resolve(&self, token: &str) -> Result<AuthSession, AppError> {
        let session = self
            .sessions
            .get_live(token)
            .await
            .map_err(|_| AppError::auth("session expired or invalid"))?;
        let user = self
            .users
            .get(session.user_id)
            .await
            .map_err(|_| AppError::auth("session user no longer exists"))?;
        // A suspended account has no live session, whatever the sessions
        // table still holds: suspension must not depend on every caller
        // remembering to revoke them. Same answer as an unknown token.
        //
        // Nor has an account whose address is unconfirmed: sign-in never
        // gives it a session, and one that exists anyway is not honoured.
        if user.suspended_at.is_some() || user.email_verified_at.is_none() {
            return Err(AppError::auth("session expired or invalid"));
        }
        Ok(AuthSession {
            token: session.id,
            user,
        })
    }

    /// Revokes a session.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn logout(&self, token: &str) -> Result<(), AppError> {
        self.sessions.delete(token).await
    }
}

/// Refuses an account whose address has not been confirmed.
///
/// Every step that ends in a session must pass this once the caller has
/// proved who they are: [`AuthService::login`] does, and so must a second
/// factor step or any other path that creates a session for a user id.
///
/// # Errors
///
/// Returns [`AppError::EmailUnconfirmed`] when `email_verified_at` is
/// unset.
pub fn ensure_confirmed(user: &UserRow) -> Result<(), AppError> {
    if user.email_verified_at.is_none() {
        return Err(AppError::EmailUnconfirmed);
    }
    Ok(())
}

/// Generates a 128-bit session token, hex-encoded (32 chars).
fn new_session_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::new_session_token;

    #[test]
    fn session_tokens_are_32_alphanumeric_chars() {
        for _ in 0..100 {
            let token = new_session_token();
            assert_eq!(token.len(), 32);
            assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn session_tokens_are_unique() {
        let a = new_session_token();
        let b = new_session_token();
        assert_ne!(a, b);
    }
}
