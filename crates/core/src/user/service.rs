//! User management service: creation, updates, role changes.

use vyasa_common::{slugify, AppError};
use vyasa_db::models::{Role, UserRow};
use vyasa_db::repo::{NewUser, UsersRepo};

use super::password;

/// User administration and profile operations.
#[derive(Clone, Debug)]
pub struct UsersService {
    users: UsersRepo,
}

/// Parameters for creating a user.
pub struct CreateUser {
    /// Email address (must be unique, case-insensitive).
    pub email: String,
    /// Desired username; derived from email when `None`.
    pub username: Option<String>,
    /// Display name; falls back to username.
    pub display_name: Option<String>,
    /// Initial password (hashed before storage). `None` creates a
    /// passwordless (invited) account that cannot log in until a password
    /// is set.
    pub password: Option<String>,
    /// Site role.
    pub role: Role,
}

/// Fields a user may update on their own profile.
pub struct UpdateProfile {
    /// New display name.
    pub display_name: Option<String>,
    /// New biography.
    pub bio: Option<String>,
    /// New password (re-hashed).
    pub password: Option<String>,
}

impl UsersService {
    /// Creates a service over the users repository.
    #[must_use]
    pub fn new(users: UsersRepo) -> Self {
        Self { users }
    }

    /// Creates a user; generates id, username, and password hash.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for empty email or weak
    /// passwords, [`AppError::Conflict`] when email/username is taken,
    /// [`AppError::Db`] on other database failures.
    pub async fn create(&self, input: CreateUser) -> Result<UserRow, AppError> {
        self.create_as(input, None).await
    }

    /// Creates a user who holds the custom role `slug` from the start
    /// (their built-in role is `subscriber`; `input.role` is not used).
    /// One write: a role that is not there leaves no account behind.
    ///
    /// Whether the caller may hand the role out is the caller's to decide
    /// first.
    ///
    /// # Errors
    ///
    /// As [`Self::create`], plus [`AppError::NotFound`] when there is no
    /// such role.
    pub async fn create_with_custom_role(
        &self,
        input: CreateUser,
        slug: &str,
    ) -> Result<UserRow, AppError> {
        self.create_as(input, Some(slug)).await
    }

    async fn create_as(
        &self,
        input: CreateUser,
        custom_role: Option<&str>,
    ) -> Result<UserRow, AppError> {
        let email = input.email.trim().to_string();
        if email.is_empty() || !email.contains('@') {
            return Err(AppError::validation("email must be a valid address"));
        }
        let username = derived_username(input.username.as_deref(), &email)?;
        let password_hash = match input.password {
            Some(password) => {
                password::validate_password(&password)?;
                Some(password::hash_password(&password)?)
            }
            None => None,
        };
        let display_name = input.display_name.unwrap_or_else(|| username.clone());
        let new = NewUser {
            id: vyasa_common::next_id_i64(),
            email: &email,
            username: &username,
            display_name: &display_name,
            password_hash: password_hash.as_deref(),
            role: input.role,
            bio: "",
        };
        let result = match custom_role {
            Some(slug) => self.users.insert_with_custom_role(&new, slug).await,
            None => self.users.insert(&new).await,
        };
        match result {
            Ok(user) => Ok(user),
            Err(AppError::Db { message }) if message.contains("duplicate key") => {
                // Only someone who manages users creates one, and they can
                // see the pending account in the list: say what holds the
                // address, so they know to confirm or delete it.
                let pending = matches!(
                    self.users.get_by_email(&email).await,
                    Ok(held) if held.email_verified_at.is_none()
                );
                Err(AppError::conflict(if pending {
                    "an unconfirmed sign-up holds this address; confirm or delete it first"
                } else {
                    "email or username is already taken"
                }))
            }
            Err(err) => Err(err),
        }
    }

    /// Fetches a user by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<UserRow, AppError> {
        self.users.get(id).await
    }

    /// Lists users, newest first, paged.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self, limit: u32, offset: u32) -> Result<Vec<UserRow>, AppError> {
        self.users.list(limit, offset).await
    }

    /// Updates a user's own profile fields.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for weak passwords or
    /// [`AppError::Db`] on database failure.
    pub async fn update_profile(&self, id: i64, update: UpdateProfile) -> Result<(), AppError> {
        if let Some(password) = &update.password {
            password::validate_password(password)?;
            let hash = password::hash_password(password)?;
            self.users.set_password(id, &hash).await?;
        }
        if update.display_name.is_some() || update.bio.is_some() {
            self.users
                .update_profile(id, update.display_name.as_deref(), update.bio.as_deref())
                .await?;
        }
        Ok(())
    }

    /// Changes a user's role (admin operation).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when demoting the last active
    /// (non-suspended) admin, or [`AppError::Db`] on database failure.
    pub async fn set_role(&self, id: i64, role: Role) -> Result<(), AppError> {
        self.users.set_role_keeping_an_admin(id, role).await
    }

    /// Suspends or reinstates an account (admin operation). Suspending
    /// is refused when it would leave no active admin; reinstating is
    /// never refused.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when suspending the last active
    /// admin, [`AppError::NotFound`] when missing, or [`AppError::Db`] on
    /// database failure.
    pub async fn set_suspended(&self, id: i64, suspended: bool) -> Result<(), AppError> {
        self.users
            .set_suspended_keeping_an_admin(id, suspended)
            .await
    }

    /// Deletes a user that owns no posts or media (admin operation);
    /// blocked for the last active admin. Same as
    /// [`Self::delete_reassigning`] with no target.
    ///
    /// # Errors
    ///
    /// As [`Self::delete_reassigning`].
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        self.delete_reassigning(id, None).await
    }

    /// Deletes a user, attributing their posts, media and revisions to
    /// `reassign_to` in the same transaction (WordPress's "attribute all
    /// content to"). Without a target, a user who owns posts or media is
    /// not deleted; their revisions on other people's posts pass to those
    /// posts' authors. Never deletes the last active admin.
    ///
    /// # Errors
    ///
    /// [`AppError::Conflict`] when the user owns content and no target was
    /// given; [`AppError::Validation`] for the last active admin, a
    /// target equal to `id`, or a target that is suspended or unconfirmed; [`AppError::NotFound`] for a missing user or
    /// target; [`AppError::Db`] on database failure.
    pub async fn delete_reassigning(
        &self,
        id: i64,
        reassign_to: Option<i64>,
    ) -> Result<(), AppError> {
        self.users.delete_reassigning(id, reassign_to).await
    }

    /// Counts users (first-run detection for the admin bootstrap).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count(&self) -> Result<i64, AppError> {
        self.users.count().await
    }
}

/// Resolves the username: explicit (slugified) or derived from the email
/// local part.
///
/// # Errors
///
/// Returns [`AppError::Validation`] when the result would be empty.
pub(crate) fn derived_username(username: Option<&str>, email: &str) -> Result<String, AppError> {
    if let Some(name) = username {
        let slug = slugify(name);
        if slug.is_empty() {
            return Err(AppError::validation("username cannot be empty"));
        }
        return Ok(slug);
    }
    let local = email.split('@').next().unwrap_or_default();
    let slug = slugify(local);
    if slug.is_empty() {
        return Err(AppError::validation(
            "cannot derive a username from this email; provide one",
        ));
    }
    Ok(slug)
}
