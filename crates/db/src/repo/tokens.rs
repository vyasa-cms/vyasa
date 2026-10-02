//! The hashed, single-use token store (`reset_tokens`): password-reset and
//! invitation links, and the links that confirm a registered address.
//!
//! Callers hash the token; only the hash is stored. Every token carries
//! the purpose it was issued for and is refused for any other.

use chrono::Duration;
use sqlx::{PgExecutor, PgPool};
use vyasa_common::AppError;

use crate::models::TokenPurpose;

/// The one rule for a token that can still be used, with the hash bound
/// as `$1` and the purpose as `$2`: issued for that purpose, not used, not
/// expired. Looking a token up ([`TokensRepo::is_live`]) and spending it
/// ([`consume_in`]) both use it, so the two cannot disagree.
macro_rules! live_token {
    () => {
        "token_hash = $1 AND purpose = $2 AND used = false AND expires_at > now()"
    };
}

/// Data access for `reset_tokens`.
#[derive(Clone, Debug)]
pub struct TokensRepo {
    pool: PgPool,
}

impl TokensRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Stores the hash of a token for `user_id`, good for `purpose` only,
    /// once, until `ttl` from now.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure (including a user that
    /// does not exist).
    pub async fn issue(
        &self,
        user_id: i64,
        purpose: TokenPurpose,
        token_hash: &str,
        ttl: Duration,
    ) -> Result<(), AppError> {
        issue_in(&self.pool, user_id, purpose, token_hash, ttl).await
    }

    /// Spends a token and returns whose it was. `None` when the hash is
    /// unknown, already used, expired, or was issued for another purpose
    /// (in which case it is not spent).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn consume(
        &self,
        token_hash: &str,
        purpose: TokenPurpose,
    ) -> Result<Option<i64>, AppError> {
        consume_in(&self.pool, token_hash, purpose).await
    }

    /// Whether a token could be spent for `purpose` now, without spending
    /// it: false when the hash is unknown, already used, expired, or was
    /// issued for another purpose. One indexed lookup, so a caller can
    /// refuse a dead token before doing anything costly; spending it
    /// ([`Self::consume`]) decides again.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn is_live(&self, token_hash: &str, purpose: TokenPurpose) -> Result<bool, AppError> {
        sqlx::query_scalar(concat!(
            "SELECT EXISTS (SELECT 1 FROM reset_tokens WHERE ",
            live_token!(),
            ")"
        ))
        .bind(token_hash)
        .bind(purpose.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("token lookup failed: {err}")))
    }

    /// How many tokens of `purpose` were issued to `user_id` in the last
    /// `window` (used or not): what a per-account mail limit counts.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn issued_since(
        &self,
        user_id: i64,
        purpose: TokenPurpose,
        window: Duration,
    ) -> Result<i64, AppError> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM reset_tokens
             WHERE user_id = $1 AND purpose = $2
               AND created_at > now() - ($3 * interval '1 second')",
        )
        .bind(user_id)
        .bind(purpose.as_str())
        .bind(window.num_seconds())
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("token count failed: {err}")))
    }
}

pub(crate) async fn issue_in(
    db: impl PgExecutor<'_>,
    user_id: i64,
    purpose: TokenPurpose,
    token_hash: &str,
    ttl: Duration,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO reset_tokens (id, user_id, token_hash, expires_at, purpose)
         VALUES ($1, $2, $3, now() + ($4 * interval '1 second'), $5)",
    )
    .bind(vyasa_common::next_id_i64())
    .bind(user_id)
    .bind(token_hash)
    .bind(ttl.num_seconds())
    .bind(purpose.as_str())
    .execute(db)
    .await
    .map_err(|err| AppError::db(format!("token insert failed: {err}")))?;
    Ok(())
}

pub(crate) async fn consume_in(
    db: impl PgExecutor<'_>,
    token_hash: &str,
    purpose: TokenPurpose,
) -> Result<Option<i64>, AppError> {
    sqlx::query_scalar(concat!(
        "UPDATE reset_tokens SET used = true WHERE ",
        live_token!(),
        " RETURNING user_id"
    ))
    .bind(token_hash)
    .bind(purpose.as_str())
    .fetch_optional(db)
    .await
    .map_err(|err| AppError::db(format!("token consume failed: {err}")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use chrono::Duration;
    use vyasa_testkit::TestDb;

    use super::TokensRepo;
    use crate::models::{Role, TokenPurpose};
    use crate::repo::{NewUser, UsersRepo};

    /// Looking a token up and spending it apply one rule: a token is live
    /// exactly while it could be spent, and looking spends nothing.
    #[tokio::test]
    async fn a_token_is_live_only_while_it_could_be_spent() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        UsersRepo::new(pool.clone())
            .insert(&NewUser {
                id: 1,
                email: "a@example.com",
                username: "a",
                display_name: "a",
                password_hash: None,
                role: Role::Subscriber,
                bio: "",
            })
            .await
            .unwrap();
        let tokens = TokensRepo::new(pool.clone());
        for hash in ["live", "spent", "expired"] {
            tokens
                .issue(1, TokenPurpose::Reset, hash, Duration::hours(1))
                .await
                .unwrap();
        }
        tokens
            .issue(1, TokenPurpose::Verify, "other-purpose", Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(
            tokens.consume("spent", TokenPurpose::Reset).await.unwrap(),
            Some(1)
        );
        sqlx::query(
            "UPDATE reset_tokens SET expires_at = now() - interval '1 second'
             WHERE token_hash = 'expired'",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Unknown, used, expired, issued for another purpose.
        for dead in ["unknown", "spent", "expired", "other-purpose"] {
            assert!(
                !tokens.is_live(dead, TokenPurpose::Reset).await.unwrap(),
                "{dead}"
            );
            assert_eq!(
                tokens.consume(dead, TokenPurpose::Reset).await.unwrap(),
                None,
                "{dead}: spending agrees"
            );
        }
        // Looking, twice, spends nothing; the token refused for the wrong
        // purpose is still good for its own.
        for _ in 0..2 {
            assert!(tokens.is_live("live", TokenPurpose::Reset).await.unwrap());
        }
        assert!(tokens
            .is_live("other-purpose", TokenPurpose::Verify)
            .await
            .unwrap());
        assert_eq!(
            tokens.consume("live", TokenPurpose::Reset).await.unwrap(),
            Some(1)
        );
        assert!(!tokens.is_live("live", TokenPurpose::Reset).await.unwrap());
        pool.close().await;
    }
}
