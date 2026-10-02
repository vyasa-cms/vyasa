//! Audience data: view rollups, form submissions, subscribers.

use chrono::NaiveDate;
use serde::Serialize;
use sqlx::PgPool;
use vyasa_common::AppError;

/// One day of one path's traffic from one referrer host.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ViewRow {
    /// Calendar day (UTC).
    pub day: NaiveDate,
    /// Site-relative path.
    pub path: String,
    /// Referring host, `""` for direct.
    pub referrer: String,
    /// Count.
    pub views: i64,
}

/// A lead-form submission.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FormSubmissionRow {
    /// Snowflake id.
    pub id: i64,
    /// Form name the section declared.
    pub form: String,
    /// Visitor name, may be empty.
    pub name: String,
    /// Visitor email.
    pub email: String,
    /// Message, may be empty.
    pub message: String,
    /// Page the form was on.
    pub path: String,
    /// A defined form's answers, by field key.
    #[sqlx(default)]
    pub data: serde_json::Value,
    /// When someone opened it in the inbox.
    #[sqlx(default)]
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When it arrived.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A newsletter subscriber.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SubscriberRow {
    /// Snowflake id.
    pub id: i64,
    /// Address.
    pub email: String,
    /// `pending` / `confirmed` / `unsubscribed`.
    pub status: String,
    /// Confirm/unsubscribe secret.
    pub token: String,
    /// When they signed up.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When they confirmed, when they did.
    pub confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Repository over the audience tables.
#[derive(Clone, Debug)]
pub struct AudienceRepo {
    pool: PgPool,
}

impl AudienceRepo {
    /// Binds to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Counts one view — an upsert, so writes never race each other.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn record_view(&self, path: &str, referrer: &str) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO page_views (day, path, referrer, views)
             VALUES (CURRENT_DATE, $1, $2, 1)
             ON CONFLICT (day, path, referrer)
             DO UPDATE SET views = page_views.views + 1",
        )
        .bind(path)
        .bind(referrer)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("record view: {e}")))?;
        Ok(())
    }

    /// Daily totals for the last `days` days, oldest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn daily_views(&self, days: i32) -> Result<Vec<(NaiveDate, i64)>, AppError> {
        let rows: Vec<(NaiveDate, i64)> = sqlx::query_as(
            "SELECT day, sum(views)::BIGINT FROM page_views
             WHERE day > CURRENT_DATE - $1::INT
             GROUP BY day ORDER BY day",
        )
        .bind(days)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("daily views: {e}")))?;
        Ok(rows)
    }

    /// Most-viewed paths over the last `days` days.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn top_paths(&self, days: i32, limit: i64) -> Result<Vec<(String, i64)>, AppError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT path, sum(views)::BIGINT AS v FROM page_views
             WHERE day > CURRENT_DATE - $1::INT
             GROUP BY path ORDER BY v DESC LIMIT $2",
        )
        .bind(days)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("top paths: {e}")))?;
        Ok(rows)
    }

    /// Busiest referrer hosts over the last `days` days (direct excluded).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn top_referrers(
        &self,
        days: i32,
        limit: i64,
    ) -> Result<Vec<(String, i64)>, AppError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT referrer, sum(views)::BIGINT AS v FROM page_views
             WHERE day > CURRENT_DATE - $1::INT AND referrer <> ''
             GROUP BY referrer ORDER BY v DESC LIMIT $2",
        )
        .bind(days)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("top referrers: {e}")))?;
        Ok(rows)
    }

    /// Stores one submission.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn add_submission(
        &self,
        form: &str,
        name: &str,
        email: &str,
        message: &str,
        path: &str,
    ) -> Result<i64, AppError> {
        let id = vyasa_common::next_id_i64();
        let _ = id;
        self.add_submission_with(form, name, email, message, path, &serde_json::json!({}))
            .await
    }

    /// Submissions, newest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn submissions(&self, limit: i64) -> Result<Vec<FormSubmissionRow>, AppError> {
        sqlx::query_as(
            "SELECT id, form, name, email, message, path, created_at, data, read_at
             FROM form_submissions ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("submissions: {e}")))
    }

    /// Records a submission with a defined form's answers.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn add_submission_with(
        &self,
        form: &str,
        name: &str,
        email: &str,
        message: &str,
        path: &str,
        data: &serde_json::Value,
    ) -> Result<i64, AppError> {
        let id = vyasa_common::next_id_i64();
        sqlx::query(
            "INSERT INTO form_submissions (id, form, name, email, message, path, data)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(form)
        .bind(name)
        .bind(email)
        .bind(message)
        .bind(path)
        .bind(data)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("form submission: {e}")))?;
        Ok(id)
    }

    /// Submissions for one form, newest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn submissions_for(
        &self,
        form: &str,
        limit: i64,
    ) -> Result<Vec<FormSubmissionRow>, AppError> {
        sqlx::query_as(
            "SELECT id, form, name, email, message, path, created_at, data, read_at
             FROM form_submissions WHERE form = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(form)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("submissions: {e}")))
    }

    /// Unread counts per form.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn unread_by_form(&self) -> Result<Vec<(String, i64)>, AppError> {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT form, count(*) FROM form_submissions WHERE read_at IS NULL GROUP BY form",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("unread: {e}")))
    }

    /// Marks submissions read.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn mark_read(&self, ids: &[i64]) -> Result<u64, AppError> {
        Ok(sqlx::query(
            "UPDATE form_submissions SET read_at = now() WHERE id = ANY($1) AND read_at IS NULL",
        )
        .bind(ids)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("mark read: {e}")))?
        .rows_affected())
    }

    /// Deletes one submission.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn delete_submission(&self, id: i64) -> Result<(), AppError> {
        let done = sqlx::query("DELETE FROM form_submissions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("submission delete: {e}")))?;
        if done.rows_affected() == 0 {
            return Err(AppError::not_found("form_submission", id));
        }
        Ok(())
    }

    /// Creates or refreshes a pending subscriber, returning the row.
    ///
    /// A confirmed subscriber stays confirmed (re-subscribing is a
    /// no-op, not a demotion); an unsubscribed one becomes pending again
    /// with a fresh token, since typing the form is a new request.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn upsert_pending(
        &self,
        email: &str,
        token: &str,
    ) -> Result<SubscriberRow, AppError> {
        sqlx::query_as(
            "INSERT INTO subscribers (id, email, status, token)
             VALUES ($1, $2, 'pending', $3)
             ON CONFLICT (email) DO UPDATE SET
                 status = CASE WHEN subscribers.status = 'confirmed'
                               THEN 'confirmed' ELSE 'pending' END,
                 token  = CASE WHEN subscribers.status = 'confirmed'
                               THEN subscribers.token ELSE EXCLUDED.token END
             RETURNING id, email, status, token, created_at, confirmed_at",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(email)
        .bind(token)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("subscriber upsert: {e}")))
    }

    /// Flips the token's owner to `status`; `Ok(None)` for a bad token.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_status_by_token(
        &self,
        token: &str,
        status: &str,
    ) -> Result<Option<SubscriberRow>, AppError> {
        sqlx::query_as(
            "UPDATE subscribers SET status = $2,
                 confirmed_at = CASE WHEN $2 = 'confirmed' THEN now()
                                     ELSE confirmed_at END
             WHERE token = $1
             RETURNING id, email, status, token, created_at, confirmed_at",
        )
        .bind(token)
        .bind(status)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("subscriber status: {e}")))
    }

    /// Every subscriber, newest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn subscribers(&self, limit: i64) -> Result<Vec<SubscriberRow>, AppError> {
        sqlx::query_as(
            "SELECT id, email, status, token, created_at, confirmed_at
             FROM subscribers ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("subscribers: {e}")))
    }

    /// Confirmed addresses only — the digest's audience.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn confirmed(&self) -> Result<Vec<SubscriberRow>, AppError> {
        sqlx::query_as(
            "SELECT id, email, status, token, created_at, confirmed_at
             FROM subscribers WHERE status = 'confirmed' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("confirmed subscribers: {e}")))
    }

    /// Removes one subscriber outright (admin action; the polite exit is
    /// [`Self::set_status_by_token`] with `unsubscribed`).
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn delete_subscriber(&self, id: i64) -> Result<(), AppError> {
        let done = sqlx::query("DELETE FROM subscribers WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("subscriber delete: {e}")))?;
        if done.rows_affected() == 0 {
            return Err(AppError::not_found("subscriber", id));
        }
        Ok(())
    }
}
