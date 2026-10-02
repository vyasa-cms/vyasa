//! User repository.

use sqlx::PgPool;
use vyasa_common::AppError;

use super::tokens::{consume_in, issue_in};
use crate::models::{TokenPurpose, UserRow};

/// The select list for a [`UserRow`], for `SELECT ... FROM users` and for
/// `RETURNING` on `users` alike.
///
/// The custom role's name and capabilities come from `roles` by scalar
/// subquery (a primary-key lookup each), so they are read in the same
/// statement as the user and work in a `RETURNING` clause, where a join
/// cannot. Every query that maps to [`UserRow`] must use this list; the
/// table must be referred to as `users` (no alias).
pub const USER_COLUMNS: &str = "id, email, username, display_name, password_hash, role, bio,
     avatar_media_id, meta, created_at, updated_at, last_login_at, suspended_at,
     custom_role, email_verified_at,
     (SELECT r.name FROM roles r WHERE r.slug = users.custom_role) AS custom_role_name,
     (SELECT r.capabilities FROM roles r WHERE r.slug = users.custom_role) AS custom_role_caps";

/// Data access for the `users` table.
#[derive(Clone, Debug)]
pub struct UsersRepo {
    pool: PgPool,
}

impl UsersRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fetches a user by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when the user does not exist, or
    /// [`AppError::Db`] on database failure.
    pub async fn get(&self, id: i64) -> Result<UserRow, AppError> {
        sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("user lookup failed: {err}")))?
            .ok_or_else(|| AppError::not_found("user", id))
    }

    /// Fetches a user by email (case-insensitive via citext).
    ///
    /// The `$1::citext` cast is required: sqlx binds parameters as `text`,
    /// and `citext = text` would silently fall back to case-sensitive
    /// text equality.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when the user does not exist, or
    /// [`AppError::Db`] on database failure.
    pub async fn get_by_email(&self, email: &str) -> Result<UserRow, AppError> {
        sqlx::query_as(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE email = $1::citext"
        ))
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("user", email))
    }

    /// Inserts a new user row.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure (e.g. uniqueness
    /// violation; callers map constraint errors to friendly conflicts).
    pub async fn insert(&self, user: &NewUser<'_>) -> Result<UserRow, AppError> {
        sqlx::query_as(&format!(
            "INSERT INTO users (id, email, username, display_name, password_hash, role, bio)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING {USER_COLUMNS}"
        ))
        .bind(user.id)
        .bind(user.email)
        .bind(user.username)
        .bind(user.display_name)
        .bind(user.password_hash)
        .bind(user.role.as_str())
        .bind(user.bio)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user insert failed: {err}")))
    }

    /// Inserts a new user who holds the custom role `slug` from the
    /// start: one statement, so there is never an account without the
    /// role. The base role is `subscriber`, whatever `user.role` says.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such role (nothing
    /// is inserted), [`AppError::Db`] on other database failures (e.g. a
    /// uniqueness violation, as [`Self::insert`]).
    pub async fn insert_with_custom_role(
        &self,
        user: &NewUser<'_>,
        slug: &str,
    ) -> Result<UserRow, AppError> {
        sqlx::query_as(&format!(
            "INSERT INTO users
                 (id, email, username, display_name, password_hash, role, bio, custom_role)
             VALUES ($1, $2, $3, $4, $5, 'subscriber', $6, $7)
             RETURNING {USER_COLUMNS}"
        ))
        .bind(user.id)
        .bind(user.email)
        .bind(user.username)
        .bind(user.display_name)
        .bind(user.password_hash)
        .bind(user.bio)
        .bind(slug)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| match err {
            sqlx::Error::Database(db) if db.is_foreign_key_violation() => {
                AppError::not_found("role", slug)
            }
            err => AppError::db(format!("user insert failed: {err}")),
        })
    }

    /// Updates the password hash of a user.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_password(&self, id: i64, password_hash: &str) -> Result<(), AppError> {
        sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
            .bind(password_hash)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("password update failed: {err}")))?;
        Ok(())
    }

    /// Counts users.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count(&self) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("user count failed: {err}")))
    }

    /// Counts the accounts whose address is not confirmed yet (made by
    /// registration and not yet confirmed).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_unconfirmed(&self) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email_verified_at IS NULL")
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("unconfirmed count failed: {err}")))
    }

    /// Counts the *active* (not suspended) users with a given role.
    ///
    /// A suspended administrator cannot sign in, so counting one toward
    /// "is there still an admin" would let the last usable admin go.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_by_role(&self, role: crate::models::Role) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = $1 AND suspended_at IS NULL")
            .bind(role.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("user count failed: {err}")))
    }

    /// How many users hold each built-in role as their whole role: users
    /// with a custom role (whose base role is `subscriber`) are not
    /// counted. Roles nobody holds are absent.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_by_builtin_role(&self) -> Result<Vec<(String, i64)>, AppError> {
        sqlx::query_as(
            "SELECT role, COUNT(*) FROM users WHERE custom_role IS NULL GROUP BY role ORDER BY role",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user count failed: {err}")))
    }

    /// Display names for a set of ids, in one query. Used by the public
    /// site to attribute posts without N lookups per listing, so an
    /// account that has not confirmed its address is left out, as it is
    /// everywhere public.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn display_names(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, String>, AppError> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, display_name FROM users
                 WHERE id = ANY($1) AND email_verified_at IS NOT NULL",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user names lookup failed: {err}")))?;
        Ok(rows.into_iter().collect())
    }

    /// Lists users, newest first.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self, limit: u32, offset: u32) -> Result<Vec<UserRow>, AppError> {
        sqlx::query_as(&format!(
            "SELECT {USER_COLUMNS} FROM users
             ORDER BY created_at DESC, id DESC LIMIT $1 OFFSET $2"
        ))
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user list failed: {err}")))
    }

    /// Users matching `term` in email, username or display name; all
    /// users for an empty term. Newest first.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn search(
        &self,
        term: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<UserRow>, i64), AppError> {
        let pattern = format!("%{}%", term.trim().replace('%', "\\%"));
        let rows: Vec<UserRow> = sqlx::query_as(&format!(
            "SELECT {USER_COLUMNS} FROM users
             WHERE $1 = '' OR email ILIKE $2 OR username ILIKE $2 OR display_name ILIKE $2
             ORDER BY created_at DESC, id DESC LIMIT $3 OFFSET $4"
        ))
        .bind(term.trim())
        .bind(&pattern)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user search failed: {err}")))?;
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users
             WHERE $1 = '' OR email ILIKE $2 OR username ILIKE $2 OR display_name ILIKE $2",
        )
        .bind(term.trim())
        .bind(&pattern)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user count failed: {err}")))?;
        Ok((rows, total))
    }

    /// Records a successful sign-in.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn touch_login(&self, id: i64) -> Result<(), AppError> {
        sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("login stamp failed: {err}")))?;
        Ok(())
    }

    /// Suspends or reinstates an account.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn set_suspended(&self, id: i64, suspended: bool) -> Result<(), AppError> {
        let n = sqlx::query(
            "UPDATE users SET suspended_at = CASE WHEN $2 THEN now() ELSE NULL END WHERE id = $1",
        )
        .bind(id)
        .bind(suspended)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("suspend failed: {err}")))?
        .rows_affected();
        if n == 0 {
            return Err(AppError::not_found("user", id));
        }
        Ok(())
    }

    /// An administrator's edit: identity fields, each optional.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Db`] on
    /// a duplicate email or username.
    pub async fn update_identity(
        &self,
        id: i64,
        email: Option<&str>,
        username: Option<&str>,
        display_name: Option<&str>,
        bio: Option<&str>,
    ) -> Result<UserRow, AppError> {
        sqlx::query_as(&format!(
            "UPDATE users SET
                email = COALESCE($2, email),
                username = COALESCE($3, username),
                display_name = COALESCE($4, display_name),
                bio = COALESCE($5, bio)
             WHERE id = $1 RETURNING {USER_COLUMNS}"
        ))
        .bind(id)
        .bind(email)
        .bind(username)
        .bind(display_name)
        .bind(bio)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("user", id))
    }

    /// Sets or clears the avatar.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_avatar(&self, id: i64, media_id: Option<i64>) -> Result<(), AppError> {
        sqlx::query("UPDATE users SET avatar_media_id = $2 WHERE id = $1")
            .bind(id)
            .bind(media_id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("avatar update failed: {err}")))?;
        Ok(())
    }

    /// Updates profile fields (display name and/or bio).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn update_profile(
        &self,
        id: i64,
        display_name: Option<&str>,
        bio: Option<&str>,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE users SET
                display_name = COALESCE($1, display_name),
                bio = COALESCE($2, bio)
             WHERE id = $3",
        )
        .bind(display_name)
        .bind(bio)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("profile update failed: {err}")))?;
        Ok(())
    }

    /// Changes a user's built-in role, clearing any custom role.
    ///
    /// Like every role write here, this deletes the account's outstanding
    /// password-reset and invitation tokens in the same statement: a link
    /// issued while the account held little must not be a way into the
    /// account it has become. A write that leaves the role as it was (the
    /// same built-in role, no custom role) is no change and keeps them;
    /// confirmation tokens are always kept (see `drop_reset_tokens`).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_role(&self, id: i64, role: crate::models::Role) -> Result<(), AppError> {
        sqlx::query(
            "WITH dropped AS (
                DELETE FROM reset_tokens
                WHERE user_id = $2 AND purpose = 'reset'
                  AND EXISTS (SELECT 1 FROM users WHERE id = $2
                              AND (role <> $1 OR custom_role IS NOT NULL))
             )
             UPDATE users SET role = $1, custom_role = NULL WHERE id = $2",
        )
        .bind(role.as_str())
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("role update failed: {err}")))?;
        Ok(())
    }

    /// Changes a user's built-in role unless that would leave no active
    /// admin. Any custom role is cleared.
    ///
    /// The check and the write share a transaction that locks every
    /// active admin row first, so two concurrent demotions of the last two
    /// admins cannot both see "one other admin left".
    ///
    /// The account's outstanding reset tokens go in the same transaction,
    /// unless the role is the one it already holds (see [`Self::set_role`]).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when demoting the last active
    /// admin, [`AppError::NotFound`] when missing, [`AppError::Db`]
    /// otherwise.
    pub async fn set_role_keeping_an_admin(
        &self,
        id: i64,
        role: crate::models::Role,
    ) -> Result<(), AppError> {
        let mut tx = self.begin().await?;
        let admins = lock_active_admins(&mut tx).await?;
        let user = lock_user(&mut tx, id).await?;
        if user.is_active_admin() && role != crate::models::Role::Admin && admins <= 1 {
            return Err(AppError::validation(
                "cannot demote the last remaining admin",
            ));
        }
        sqlx::query("UPDATE users SET role = $1, custom_role = NULL WHERE id = $2")
            .bind(role.as_str())
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("role update failed: {err}")))?;
        if user.role != role.as_str() || user.custom_role.is_some() {
            drop_reset_tokens(&mut tx, id).await?;
        }
        commit(tx).await
    }

    /// Assigns the custom role `slug` to a user, or clears it with `None`.
    ///
    /// A user with a custom role has the built-in role `subscriber`
    /// underneath, so assigning one to an administrator demotes them: it
    /// is refused for the last active admin. Clearing leaves the user a
    /// plain subscriber. Same locking as
    /// [`Self::set_role_keeping_an_admin`]: every active admin row in id
    /// order, then the target.
    ///
    /// The account's outstanding reset tokens go in the same transaction,
    /// unless the custom role is the one it already holds, or there was
    /// none to clear (see [`Self::set_role`]).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when this would demote the last
    /// active admin, [`AppError::NotFound`] for a missing user or role,
    /// [`AppError::Db`] otherwise.
    pub async fn set_custom_role(&self, id: i64, slug: Option<&str>) -> Result<(), AppError> {
        let mut tx = self.begin().await?;
        let admins = lock_active_admins(&mut tx).await?;
        let user = lock_user(&mut tx, id).await?;
        let Some(slug) = slug else {
            // Only a user who has a custom role is touched: their base role
            // is already `subscriber`.
            sqlx::query("UPDATE users SET custom_role = NULL WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("role update failed: {err}")))?;
            if user.custom_role.is_some() {
                drop_reset_tokens(&mut tx, id).await?;
            }
            return commit(tx).await;
        };
        if user.is_active_admin() && admins <= 1 {
            return Err(AppError::validation(
                "cannot demote the last remaining admin",
            ));
        }
        sqlx::query("UPDATE users SET role = 'subscriber', custom_role = $1 WHERE id = $2")
            .bind(slug)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| match err {
                sqlx::Error::Database(db) if db.is_foreign_key_violation() => {
                    AppError::not_found("role", slug)
                }
                err => AppError::db(format!("role update failed: {err}")),
            })?;
        if user.custom_role.as_deref() != Some(slug) {
            drop_reset_tokens(&mut tx, id).await?;
        }
        commit(tx).await
    }

    /// Suspends or reinstates an account, unless suspending would leave
    /// no active admin. Reinstating is never refused.
    ///
    /// Like [`Self::set_role_keeping_an_admin`], the check and the write
    /// share a transaction that locks every active admin row first, so
    /// two admins suspending each other cannot both see "one other admin
    /// left".
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when suspending the last active
    /// admin, [`AppError::NotFound`] when missing, [`AppError::Db`]
    /// otherwise.
    pub async fn set_suspended_keeping_an_admin(
        &self,
        id: i64,
        suspended: bool,
    ) -> Result<(), AppError> {
        if !suspended {
            return self.set_suspended(id, false).await;
        }
        let mut tx = self.begin().await?;
        let admins = lock_active_admins(&mut tx).await?;
        let user = lock_user(&mut tx, id).await?;
        if user.is_active_admin() && admins <= 1 {
            return Err(AppError::validation(
                "the last administrator cannot be suspended",
            ));
        }
        // An account already suspended keeps the time it was suspended at.
        sqlx::query("UPDATE users SET suspended_at = COALESCE(suspended_at, now()) WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("suspend failed: {err}")))?;
        commit(tx).await
    }

    /// Deletes a user (sessions/api_keys cascade).
    ///
    /// Fails with a database error when the user still owns posts or
    /// revisions; [`Self::delete_reassigning`] is the safe path.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("user delete failed: {err}")))?;
        Ok(())
    }

    /// Deletes a user, first handing what they own to `reassign_to`, all
    /// in one transaction -- WordPress's "attribute all content to".
    ///
    /// - Posts and media they own move to `reassign_to`. Without a target
    ///   and with content to move, nothing is deleted: a
    ///   [`AppError::Conflict`] says how much and asks for a target.
    /// - Revisions they made on posts are history, not ownership: they
    ///   move to `reassign_to` when given, otherwise to the post's author.
    /// - The heir must be active and confirmed: content never passes to an
    ///   account that could not have written it.
    /// - The last active admin is never deleted. Every active admin row is
    ///   locked before counting, so concurrent deletes (or demotions via
    ///   [`Self::set_role_keeping_an_admin`]) cannot race past the check.
    ///
    /// # Errors
    ///
    /// [`AppError::NotFound`] for a missing user or target,
    /// [`AppError::Validation`] for the last admin, reassigning to the
    /// user themselves, or to an account that is suspended or has not
    /// confirmed its address, [`AppError::Conflict`] for content without a
    /// target, [`AppError::Db`] otherwise.
    pub async fn delete_reassigning(
        &self,
        id: i64,
        reassign_to: Option<i64>,
    ) -> Result<(), AppError> {
        if reassign_to == Some(id) {
            return Err(AppError::validation(
                "content cannot be reassigned to the user being deleted",
            ));
        }
        let mut tx = self.begin().await?;
        let admins = lock_active_admins(&mut tx).await?;
        let user = lock_user(&mut tx, id).await?;
        if user.is_active_admin() && admins <= 1 {
            return Err(AppError::validation(
                "cannot delete the last remaining admin",
            ));
        }
        if let Some(target) = reassign_to {
            // FOR SHARE: the heir cannot be deleted, suspended or have its
            // confirmation undone under us.
            let found: Option<bool> = sqlx::query_scalar(
                "SELECT email_verified_at IS NOT NULL AND suspended_at IS NULL
                 FROM users WHERE id = $1 FOR SHARE",
            )
            .bind(target)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("user lookup failed: {err}")))?;
            match found {
                None => return Err(AppError::not_found("user", target)),
                // Content handed to an account that has not confirmed its
                // address would give it a public byline it never earned.
                Some(false) => {
                    return Err(AppError::validation(
                        "content can only be reassigned to an active, confirmed account",
                    ));
                }
                Some(true) => {}
            }
        }
        let (posts, media): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM posts WHERE author_id = $1),
                    (SELECT COUNT(*) FROM media WHERE owner_id = $1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::db(format!("ownership count failed: {err}")))?;
        match reassign_to {
            None if posts > 0 || media > 0 => {
                return Err(AppError::conflict(format!(
                    "this user owns {posts} post(s) and {media} media item(s); \
                     choose a user to reassign them to before deleting"
                )));
            }
            None => {
                sqlx::query(
                    "UPDATE post_revisions r SET author_id = p.author_id
                     FROM posts p WHERE r.post_id = p.id AND r.author_id = $1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("revision handover failed: {err}")))?;
            }
            Some(target) => {
                for (what, sql) in [
                    (
                        "posts",
                        "UPDATE posts SET author_id = $2 WHERE author_id = $1",
                    ),
                    (
                        "revisions",
                        "UPDATE post_revisions SET author_id = $2 WHERE author_id = $1",
                    ),
                    (
                        "media",
                        "UPDATE media SET owner_id = $2 WHERE owner_id = $1",
                    ),
                ] {
                    sqlx::query(sql)
                        .bind(id)
                        .bind(target)
                        .execute(&mut *tx)
                        .await
                        .map_err(|err| AppError::db(format!("{what} reassign failed: {err}")))?;
                }
            }
        }
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("user delete failed: {err}")))?;
        commit(tx).await
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, AppError> {
        self.pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))
    }
}

/// Role and suspension of a locked user row.
struct LockedUser {
    role: String,
    custom_role: Option<String>,
    suspended: bool,
}

impl LockedUser {
    fn is_active_admin(&self) -> bool {
        self.role == crate::models::Role::Admin.as_str() && !self.suspended
    }
}

async fn lock_user(conn: &mut sqlx::PgConnection, id: i64) -> Result<LockedUser, AppError> {
    let row: Option<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT role, custom_role, suspended_at IS NOT NULL FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|err| AppError::db(format!("user lock failed: {err}")))?;
    let (role, custom_role, suspended) = row.ok_or_else(|| AppError::not_found("user", id))?;
    Ok(LockedUser {
        role,
        custom_role,
        suspended,
    })
}

/// Deletes `id`'s outstanding password-reset and invitation tokens, inside
/// the transaction that changes its role: a token issued for the account
/// it was cannot be redeemed for the account it is now.
///
/// Confirmation tokens stay: one confirms the address and grants nothing
/// else (signing in still takes the password), so what the account can do
/// does not change what it opens.
async fn drop_reset_tokens(conn: &mut sqlx::PgConnection, id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM reset_tokens WHERE user_id = $1 AND purpose = 'reset'")
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(|err| AppError::db(format!("reset token cleanup failed: {err}")))?;
    Ok(())
}

/// Locks every active admin row and returns how many there are.
///
/// Always taken *before* the target user's row, in id order, so two
/// transactions removing different admins queue on the same locks
/// instead of deadlocking on each other's rows.
async fn lock_active_admins(conn: &mut sqlx::PgConnection) -> Result<usize, AppError> {
    let admins: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM users WHERE role = 'admin' AND suspended_at IS NULL
         ORDER BY id FOR UPDATE",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|err| AppError::db(format!("admin lock failed: {err}")))?;
    Ok(admins.len())
}

async fn commit(tx: sqlx::Transaction<'static, sqlx::Postgres>) -> Result<(), AppError> {
    tx.commit()
        .await
        .map_err(|err| AppError::db(format!("tx commit failed: {err}")))
}

/// Parameters for inserting a user.
pub struct NewUser<'a> {
    /// Snowflake id.
    pub id: i64,
    /// Email (citext-unique).
    pub email: &'a str,
    /// Username (citext-unique).
    pub username: &'a str,
    /// Display name.
    pub display_name: &'a str,
    /// Argon2 hash, if the user has a password yet.
    pub password_hash: Option<&'a str>,
    /// Site role.
    pub role: crate::models::Role,
    /// Biography.
    pub bio: &'a str,
}

impl UsersRepo {
    /// Finds a user id whose meta records the given importer key
    /// (`meta ->> key` present).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn find_id_by_import_key(&self, key: &str) -> Result<Option<i64>, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE meta ? $1 LIMIT 1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("user import lookup failed: {err}")))
    }

    /// Records an import pointer in a user's meta.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_import_key(&self, user_id: i64, key: &str) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE users SET meta = meta || jsonb_build_object($2, $1::bigint) WHERE id = $1",
        )
        .bind(user_id)
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("user import mark failed: {err}")))?;
        Ok(())
    }
}

impl UsersRepo {
    /// Stores a hashed password-reset token (1h expiry, single-use).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn create_reset_token(&self, user_id: i64, token_hash: &str) -> Result<(), AppError> {
        issue_in(
            &self.pool,
            user_id,
            TokenPurpose::Reset,
            token_hash,
            chrono::Duration::hours(1),
        )
        .await
    }

    /// Consumes a valid reset token and returns the user id. A token
    /// issued to confirm an address is not one.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when invalid/expired/used.
    pub async fn consume_reset_token(&self, token_hash: &str) -> Result<i64, AppError> {
        consume_in(&self.pool, token_hash, TokenPurpose::Reset)
            .await?
            .ok_or_else(|| AppError::not_found("reset_token", ""))
    }

    /// Redeems a password-reset (or set-password) token: spends it, stores
    /// `password_hash` as the account's password, and marks the account's
    /// address confirmed if it was not (ending its confirmation links), in
    /// one transaction. The link was mailed to the address, so following
    /// it proves the mailbox as a confirmation link does, and the password
    /// is the one its owner just chose.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when the token is invalid, expired, used or
    /// not a reset token; [`AppError::Db`] on failure.
    pub async fn reset_password_with_token(
        &self,
        token_hash: &str,
        password_hash: &str,
    ) -> Result<ResetRedeemed, AppError> {
        let mut tx = self.begin().await?;
        let user_id = consume_in(&mut *tx, token_hash, TokenPurpose::Reset)
            .await?
            .ok_or_else(|| AppError::not_found("reset_token", ""))?;
        let was_confirmed: Option<bool> = sqlx::query_scalar(
            "SELECT email_verified_at IS NOT NULL FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::db(format!("user lock failed: {err}")))?;
        let was_confirmed = was_confirmed.ok_or_else(|| AppError::not_found("user", user_id))?;
        sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
            .bind(password_hash)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("password update failed: {err}")))?;
        confirm_in(&mut tx, user_id, false).await?;
        commit(tx).await?;
        Ok(ResetRedeemed {
            user_id,
            confirmed_now: !was_confirmed,
        })
    }
}

/// What [`UsersRepo::reset_password_with_token`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetRedeemed {
    /// Whose password was set.
    pub user_id: i64,
    /// The address was not confirmed before, and is now.
    pub confirmed_now: bool,
}

/// What redeeming a confirmation link does to the account's password.
///
/// Anyone can register anyone's address, so a password stored on an
/// unconfirmed account may be a stranger's. It survives confirmation only
/// when the person holding the link (the mailbox's owner) shows they know
/// it.
#[derive(Clone, Copy, Debug)]
pub enum ConfirmPassword<'a> {
    /// Keep the password, provided it is still exactly this hash (the one
    /// the caller checked the link holder's password against). If it has
    /// changed or gone since, nothing is redeemed.
    KeepIf(&'a str),
    /// Remove the password; the owner sets one through the mailbox. An
    /// address confirmed already keeps its password.
    Clear,
}

/// Public registration (phase 98).
impl UsersRepo {
    /// Inserts an account made by public registration: the only insert
    /// that leaves `email_verified_at` NULL. With `custom_role` the
    /// account holds that role from the start (base role `subscriber`, as
    /// [`Self::insert_with_custom_role`]); without, `user.role`.
    ///
    /// # Errors
    ///
    /// Says which uniqueness rule refused the row, or that the custom role
    /// is not there (nothing is inserted in either case); any other
    /// database failure is [`UnconfirmedInsertError::Other`].
    pub async fn insert_unconfirmed(
        &self,
        user: &NewUser<'_>,
        custom_role: Option<&str>,
    ) -> Result<UserRow, UnconfirmedInsertError> {
        let role = match custom_role {
            Some(_) => crate::models::Role::Subscriber,
            None => user.role,
        };
        sqlx::query_as(&format!(
            "INSERT INTO users
                 (id, email, username, display_name, password_hash, role, bio, custom_role,
                  email_verified_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL)
             RETURNING {USER_COLUMNS}"
        ))
        .bind(user.id)
        .bind(user.email)
        .bind(user.username)
        .bind(user.display_name)
        .bind(user.password_hash)
        .bind(role.as_str())
        .bind(user.bio)
        .bind(custom_role)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| {
            if let sqlx::Error::Database(db) = &err {
                if db.is_foreign_key_violation() {
                    return UnconfirmedInsertError::RoleMissing;
                }
                if db.is_unique_violation() {
                    match db.constraint() {
                        Some("users_email_key") => return UnconfirmedInsertError::EmailTaken,
                        Some("users_username_key") => return UnconfirmedInsertError::UsernameTaken,
                        _ => {}
                    }
                }
            }
            UnconfirmedInsertError::Other(AppError::db(format!("user insert failed: {err}")))
        })
    }

    /// The account whose public author page is `/author/{username}`
    /// (case-insensitive), if there is one. An account that has not
    /// confirmed its address has no public presence: it is not found.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn public_author_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserRow>, AppError> {
        sqlx::query_as(&format!(
            "SELECT {USER_COLUMNS} FROM users
             WHERE username = $1::citext AND email_verified_at IS NOT NULL"
        ))
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("author lookup failed: {err}")))
    }

    /// The account a live confirmation token belongs to, without spending
    /// the token. `None` when the token is unknown, used, expired, of
    /// another purpose, or its account is gone.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn confirmation_token_holder(
        &self,
        token_hash: &str,
    ) -> Result<Option<UserRow>, AppError> {
        sqlx::query_as(&format!(
            "SELECT {USER_COLUMNS} FROM users
             WHERE id IN (SELECT user_id FROM reset_tokens
                          WHERE token_hash = $1 AND purpose = 'verify' AND used = false
                            AND expires_at > now())
             LIMIT 1"
        ))
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("confirmation token lookup failed: {err}")))
    }

    /// Redeems a confirmation token: marks its account's address confirmed,
    /// does to its password what `password` says, and deletes the account's
    /// other confirmation tokens, in one transaction. `None` when the token
    /// is unknown, used, expired, of another purpose, or its account is
    /// gone (tokens go with their account) -- and, for
    /// [`ConfirmPassword::KeepIf`], when the stored password is no longer
    /// that hash; in every `None` case the token is not spent.
    ///
    /// An address confirmed already keeps its first confirmation time and
    /// its password.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn confirm_email_with_token(
        &self,
        token_hash: &str,
        password: ConfirmPassword<'_>,
    ) -> Result<Option<UserRow>, AppError> {
        let mut tx = self.begin().await?;
        // The token and its account are locked together, so the password
        // compared here is the one the confirmation keeps: a contest that
        // clears it waits for this transaction, and then finds the address
        // confirmed and leaves it alone.
        let held: Option<(i64, Option<String>)> = sqlx::query_as(
            "SELECT u.id, u.password_hash FROM reset_tokens t JOIN users u ON u.id = t.user_id
             WHERE t.token_hash = $1 AND t.purpose = 'verify' AND t.used = false
               AND t.expires_at > now()
             LIMIT 1
             FOR UPDATE OF t, u",
        )
        .bind(token_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::db(format!("confirmation token lookup failed: {err}")))?;
        let Some((user_id, stored)) = held else {
            return Ok(None);
        };
        let clear = match password {
            ConfirmPassword::KeepIf(expected) => {
                if stored.as_deref() != Some(expected) {
                    return Ok(None);
                }
                false
            }
            ConfirmPassword::Clear => true,
        };
        if consume_in(&mut *tx, token_hash, TokenPurpose::Verify)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let user = confirm_in(&mut tx, user_id, clear).await?;
        commit(tx).await?;
        Ok(user)
    }

    /// Marks an account's address confirmed without a token (an
    /// administrator vouching for it), removes the password an unconfirmed
    /// account was registered with, and deletes its outstanding
    /// confirmation tokens. Vouching for a mailbox says nothing about who
    /// typed that password, so the owner sets one through the mailbox. An
    /// address confirmed already keeps its first confirmation time and its
    /// password.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Db`] on
    /// database failure.
    pub async fn confirm_email_clearing_password(&self, id: i64) -> Result<UserRow, AppError> {
        let mut tx = self.begin().await?;
        let user = confirm_in(&mut tx, id, true)
            .await?
            .ok_or_else(|| AppError::not_found("user", id))?;
        commit(tx).await?;
        Ok(user)
    }

    /// Removes the password of an account that is still unconfirmed, so
    /// nobody can sign in to it until one is set through the mailbox.
    /// Returns whether an account was changed; a confirmed account never
    /// is.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn clear_unconfirmed_password(&self, id: i64) -> Result<bool, AppError> {
        sqlx::query(
            "UPDATE users SET password_hash = NULL
             WHERE id = $1 AND email_verified_at IS NULL AND password_hash IS NOT NULL",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|done| done.rows_affected() > 0)
        .map_err(|err| AppError::db(format!("password clear failed: {err}")))
    }

    /// Deletes accounts that registered before `older_than` and never
    /// confirmed their address; returns how many.
    ///
    /// Nothing is ever reassigned. An account is left alone when it is an
    /// administrator, or when it is the author of a post or a revision or
    /// the owner of a media item -- none of which an unconfirmed account
    /// can come to be by itself, so such a row is left for a person to
    /// look at. Each account is its own statement whose conditions are
    /// checked again under the row's lock: an account confirmed a moment
    /// before its turn is not deleted. One the database refuses to delete
    /// is logged and passed over.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] when the candidates cannot be listed.
    pub async fn purge_unconfirmed(
        &self,
        older_than: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, AppError> {
        self.purge_unconfirmed_limited(older_than, 500, PURGE_CEILING)
            .await
    }

    /// [`Self::purge_unconfirmed`] with the batch size and the per-run
    /// ceiling spelled out (for tests). The scan walks the candidates in id
    /// order, a batch at a time, and never looks at the same account twice
    /// in one call: rows that cannot go do not hold up the ones behind
    /// them. It stops once `max` accounts are deleted; the rest wait for
    /// the next run.
    ///
    /// # Errors
    ///
    /// As [`Self::purge_unconfirmed`].
    #[doc(hidden)]
    pub async fn purge_unconfirmed_limited(
        &self,
        older_than: chrono::DateTime<chrono::Utc>,
        batch: i64,
        max: u64,
    ) -> Result<u64, AppError> {
        let mut purged = 0;
        let mut after = i64::MIN;
        while purged < max {
            let candidates: Vec<i64> = sqlx::query_scalar(&format!(
                "SELECT u.id FROM users u
                 WHERE u.id > $2 AND {PURGEABLE}
                 ORDER BY u.id LIMIT $3"
            ))
            .bind(older_than)
            .bind(after)
            .bind(batch.max(1))
            .fetch_all(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("unconfirmed account scan failed: {err}")))?;
            let Some(last) = candidates.last() else {
                return Ok(purged);
            };
            after = *last;
            for id in candidates {
                if purged >= max {
                    break;
                }
                let deleted = sqlx::query(&format!(
                    "DELETE FROM users u WHERE u.id = $2 AND {PURGEABLE}"
                ))
                .bind(older_than)
                .bind(id)
                .execute(&self.pool)
                .await;
                match deleted {
                    Ok(done) => purged += done.rows_affected(),
                    Err(err) => {
                        tracing::warn!("unconfirmed account {id} was not purged: {err}");
                    }
                }
            }
        }
        Ok(purged)
    }
}

/// The most accounts one purge run deletes.
const PURGE_CEILING: u64 = 10_000;

/// What makes `users u` a row the purge may delete; `$1` is the cutoff.
const PURGEABLE: &str = "u.email_verified_at IS NULL AND u.created_at < $1
    AND u.role <> 'admin'
    AND NOT EXISTS (SELECT 1 FROM posts p WHERE p.author_id = u.id)
    AND NOT EXISTS (SELECT 1 FROM post_revisions r WHERE r.author_id = u.id)
    AND NOT EXISTS (SELECT 1 FROM media m WHERE m.owner_id = u.id)";

/// Why [`UsersRepo::insert_unconfirmed`] inserted nothing.
#[derive(Debug)]
pub enum UnconfirmedInsertError {
    /// The address already has an account (`users_email_key`).
    EmailTaken,
    /// The username already has an account (`users_username_key`).
    UsernameTaken,
    /// The custom role asked for does not exist.
    RoleMissing,
    /// Anything else.
    Other(AppError),
}

/// Confirms `id`'s address and drops its confirmation tokens. With
/// `clear_password`, an account that was unconfirmed until now loses its
/// password (the right-hand sides read the row as it was, so an address
/// confirmed already keeps its own).
async fn confirm_in(
    conn: &mut sqlx::PgConnection,
    id: i64,
    clear_password: bool,
) -> Result<Option<UserRow>, AppError> {
    let user: Option<UserRow> = sqlx::query_as(&format!(
        "UPDATE users SET email_verified_at = COALESCE(email_verified_at, now()),
                          password_hash = CASE WHEN $2 AND email_verified_at IS NULL
                                               THEN NULL ELSE password_hash END
         WHERE id = $1 RETURNING {USER_COLUMNS}"
    ))
    .bind(id)
    .bind(clear_password)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|err| AppError::db(format!("email confirmation failed: {err}")))?;
    sqlx::query("DELETE FROM reset_tokens WHERE user_id = $1 AND purpose = 'verify'")
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(|err| AppError::db(format!("confirmation token cleanup failed: {err}")))?;
    Ok(user)
}
