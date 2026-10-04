//! Custom roles repository: administrator-defined roles in the `roles`
//! table. Validation and the escalation guard live in `vyasa-core`; the
//! table's own constraints are the backstop.

use sqlx::PgPool;
use vyasa_common::AppError;

use crate::models::RoleRow;

const ROLE_COLUMNS: &str = "slug, name, description, capabilities, created_at, updated_at";

/// Data access for the `roles` table.
#[derive(Clone, Debug)]
pub struct RolesRepo {
    pool: PgPool,
}

/// Parameters for inserting a role.
#[derive(Clone, Copy, Debug)]
pub struct NewRole<'a> {
    /// Identifier (primary key).
    pub slug: &'a str,
    /// Display name.
    pub name: &'a str,
    /// What the role is for.
    pub description: &'a str,
    /// Capability names the role grants.
    pub capabilities: &'a [String],
}

/// Fields of a role to change; `None` leaves a field as it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct RoleUpdate<'a> {
    /// New slug. Users holding the role follow it (`ON UPDATE CASCADE`).
    pub slug: Option<&'a str>,
    /// New display name.
    pub name: Option<&'a str>,
    /// New description.
    pub description: Option<&'a str>,
    /// New capability list (replaces the old one).
    pub capabilities: Option<&'a [String]>,
}

impl RolesRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Every custom role with the number of users holding it, by slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self) -> Result<Vec<(RoleRow, i64)>, AppError> {
        #[derive(sqlx::FromRow)]
        struct Counted {
            #[sqlx(flatten)]
            role: RoleRow,
            users: i64,
        }
        let rows: Vec<Counted> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {ROLE_COLUMNS},
                    (SELECT COUNT(*) FROM users u WHERE u.custom_role = roles.slug) AS users
             FROM roles ORDER BY slug"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("role list failed: {err}")))?;
        Ok(rows.into_iter().map(|c| (c.role, c.users)).collect())
    }

    /// Fetches a role by slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such role, or
    /// [`AppError::Db`] on database failure.
    pub async fn get(&self, slug: &str) -> Result<RoleRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {ROLE_COLUMNS} FROM roles WHERE slug = $1"
        )))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("role lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("role", slug))
    }

    /// Inserts a role.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Conflict`] when the slug is taken, or
    /// [`AppError::Db`] on database failure (including a slug or name the
    /// table's constraints refuse).
    pub async fn insert(&self, role: &NewRole<'_>) -> Result<RoleRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO roles (slug, name, description, capabilities)
             VALUES ($1, $2, $3, $4)
             RETURNING {ROLE_COLUMNS}"
        )))
        .bind(role.slug)
        .bind(role.name)
        .bind(role.description)
        .bind(role.capabilities)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| write_error(err, role.slug, "role insert failed"))
    }

    /// Updates a role; each field is optional. Changing the slug keeps
    /// every assignment: `users.custom_role` follows it.
    ///
    /// Replacing the capabilities with a different set also deletes the
    /// outstanding password-reset and invitation tokens of every user who holds the
    /// role, in the same statement: a link issued for what the account
    /// was must not open what it has become.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such role,
    /// [`AppError::Conflict`] when the new slug is taken, or
    /// [`AppError::Db`] on database failure.
    pub async fn update(&self, slug: &str, update: &RoleUpdate<'_>) -> Result<RoleRow, AppError> {
        // Replacing the capabilities changes what every holder is, so
        // their outstanding reset tokens go in the same statement, as they
        // do when one account's role changes (`UsersRepo::set_role`). The
        // same set again, in any order, changes nothing and keeps them;
        // confirmation tokens grant nothing and always stay.
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "WITH dropped AS (
                DELETE FROM reset_tokens
                WHERE purpose = 'reset'
                  AND $5::text[] IS NOT NULL
                  AND EXISTS (
                      SELECT 1 FROM roles WHERE slug = $1
                      AND NOT (capabilities @> $5::text[] AND capabilities <@ $5::text[])
                  )
                  AND user_id IN (SELECT id FROM users WHERE custom_role = $1)
             )
             UPDATE roles SET
                slug = COALESCE($2, slug),
                name = COALESCE($3, name),
                description = COALESCE($4, description),
                capabilities = COALESCE($5, capabilities),
                updated_at = now()
             WHERE slug = $1
             RETURNING {ROLE_COLUMNS}"
        )))
        .bind(slug)
        .bind(update.slug)
        .bind(update.name)
        .bind(update.description)
        .bind(update.capabilities)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| write_error(err, update.slug.unwrap_or(slug), "role update failed"))?
        .ok_or_else(|| AppError::not_found("role", slug))
    }

    /// Deletes a role no user holds.
    ///
    /// The role row is locked before its users are counted. An assignment
    /// racing the delete either waits on that lock and then finds no role,
    /// or is stopped by the foreign key (`ON DELETE RESTRICT`), which is
    /// reported as the same conflict.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such role,
    /// [`AppError::Conflict`] naming the number of users when it is in
    /// use, or [`AppError::Db`] on database failure.
    pub async fn delete(&self, slug: &str) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
        let found: Option<String> =
            sqlx::query_scalar("SELECT slug FROM roles WHERE slug = $1 FOR UPDATE")
                .bind(slug)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("role lock failed: {err}")))?;
        if found.is_none() {
            return Err(AppError::not_found("role", slug));
        }
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE custom_role = $1")
            .bind(slug)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("role user count failed: {err}")))?;
        if users > 0 {
            return Err(in_use(users));
        }
        sqlx::query("DELETE FROM roles WHERE slug = $1")
            .bind(slug)
            .execute(&mut *tx)
            .await
            .map_err(|err| match err {
                sqlx::Error::Database(db) if db.is_foreign_key_violation() => in_use(1),
                err => AppError::db(format!("role delete failed: {err}")),
            })?;
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("tx commit failed: {err}")))
    }
}

fn in_use(users: i64) -> AppError {
    AppError::conflict(format!(
        "this role is assigned to {users} user(s); give them another role before deleting it"
    ))
}

fn write_error(err: sqlx::Error, slug: &str, what: &str) -> AppError {
    match err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::conflict(format!("a role with the slug {slug:?} already exists"))
        }
        err => AppError::db(format!("{what}: {err}")),
    }
}
