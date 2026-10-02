//! Purge of accounts that registered and never confirmed their address.

use std::time::Duration;

use sqlx::PgPool;
use vyasa_core::user::UNCONFIRMED_ACCOUNT_LIFETIME;
use vyasa_db::repo::UsersRepo;

/// How often the purge runs. The lifetime is counted in days, so an hour's
/// slack is nothing.
pub const PURGE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Deletes unconfirmed accounts older than
/// [`UNCONFIRMED_ACCOUNT_LIFETIME`], once. Returns how many went.
///
/// Such an account cannot sign in, so it owns nothing; one that somehow
/// does (or is an administrator) is skipped rather than reassigned -- see
/// [`UsersRepo::purge_unconfirmed`].
///
/// # Errors
///
/// Returns [`vyasa_common::AppError::Db`] on database failure.
pub async fn purge_unconfirmed_once(pool: &PgPool) -> Result<u64, vyasa_common::AppError> {
    let cutoff = chrono::Utc::now() - UNCONFIRMED_ACCOUNT_LIFETIME;
    UsersRepo::new(pool.clone()).purge_unconfirmed(cutoff).await
}

/// Spawns the purge loop: once at start, then every [`PURGE_INTERVAL`].
#[must_use]
pub fn spawn(pool: PgPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PURGE_INTERVAL);
        loop {
            interval.tick().await;
            match purge_unconfirmed_once(&pool).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("purged {n} unconfirmed account(s)"),
                Err(err) => tracing::warn!("unconfirmed account purge failed: {err}"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::purge_unconfirmed_once;
    use vyasa_db::models::Role;
    use vyasa_db::repo::{NewUser, UsersRepo};
    use vyasa_testkit::TestDb;

    async fn account(repo: &UsersRepo, pool: &sqlx::PgPool, id: i64, confirmed: bool, age: &str) {
        let email = format!("u{id}@example.com");
        let username = format!("u{id}");
        let new = NewUser {
            id,
            email: &email,
            username: &username,
            display_name: &username,
            password_hash: Some("argon2-hash"),
            role: Role::Subscriber,
            bio: "",
        };
        if confirmed {
            repo.insert(&new).await.expect("insert");
        } else {
            repo.insert_unconfirmed(&new, None).await.expect("insert");
        }
        sqlx::query("UPDATE users SET created_at = now() - $2::interval WHERE id = $1")
            .bind(id)
            .bind(age)
            .execute(pool)
            .await
            .expect("age");
    }

    #[tokio::test]
    async fn only_unconfirmed_accounts_past_seven_days_are_purged() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let repo = UsersRepo::new(pool.clone());
        account(&repo, &pool, 1, false, "7 days 1 hour").await;
        account(&repo, &pool, 2, false, "6 days 23 hours").await;
        account(&repo, &pool, 3, false, "0 seconds").await;
        account(&repo, &pool, 4, true, "7 days 1 hour").await;
        account(&repo, &pool, 5, true, "2 years").await;

        assert_eq!(purge_unconfirmed_once(&pool).await.expect("purge"), 1);
        let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM users ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("list");
        assert_eq!(left, vec![2, 3, 4, 5]);
        assert_eq!(purge_unconfirmed_once(&pool).await.expect("again"), 0);
        pool.close().await;
    }
}
