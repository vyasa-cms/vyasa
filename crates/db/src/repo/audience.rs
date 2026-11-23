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

/// A server-side redirect (`/go/<name>`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AudienceLink {
    /// The name after `/go/`.
    pub name: String,
    /// Where it sends the visitor.
    pub url: String,
    /// Whether a click also counts as a conversion.
    pub conversion: bool,
}

/// One day of the traffic chart: views, unique visitors and bot hits.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DayTraffic {
    /// Calendar day (UTC).
    pub day: NaiveDate,
    /// Human page views.
    pub views: i64,
    /// Cookieless unique visitors that day.
    pub visitors: i64,
    /// Requests counted as bots.
    pub bots: i64,
}

/// Dimensions of `audience_counts`. A closed set, so the table's `dim` never holds free text from a request.
pub mod dim {
    /// A crawler or tool request (key `""`).
    pub const BOT: &str = "bot";
    /// A first visit of the day by a visitor (key `""`).
    pub const VISITOR: &str = "visitor";
    /// Device class (`desktop`, `mobile`, `tablet`).
    pub const DEVICE: &str = "device";
    /// Browser family.
    pub const BROWSER: &str = "browser";
    /// `utm_source`.
    pub const UTM_SOURCE: &str = "utm_source";
    /// `utm_medium`.
    pub const UTM_MEDIUM: &str = "utm_medium";
    /// `utm_campaign`.
    pub const UTM_CAMPAIGN: &str = "utm_campaign";
    /// The first page of a visit (a view with no same-site referrer).
    pub const ENTRY: &str = "entry";
    /// A 404: key the path, extra the referrer host.
    pub const NOT_FOUND: &str = "notfound";
    /// An internal `/search` term.
    pub const SEARCH: &str = "search";
    /// A click through `/go/<name>`.
    pub const OUTBOUND: &str = "outbound";
    /// A conversion: a form or newsletter signup, or a `/go/` link marked as one.
    pub const CONVERSION: &str = "conversion";
}

/// Distinct keys one (day, dimension) of `audience_counts` may hold; the rest are counted under [`OTHER_KEY`].
pub const MAX_KEYS_PER_DIM: i64 = 500;

/// The bucket for keys past [`MAX_KEYS_PER_DIM`].
pub const OTHER_KEY: &str = "(other)";

impl AudienceRepo {
    /// Adds `by` to the counter of `dim`/`key`/`extra` for the UTC day `day`. A key that would be the
    /// [`MAX_KEYS_PER_DIM`]th distinct one of its (day, dimension) is counted under `(other)` instead, so request
    /// text (404 paths, tags, search terms) cannot grow the table without bound.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn count_on(
        &self,
        day: NaiveDate,
        dim: &str,
        key: &str,
        extra: &str,
        by: i64,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO audience_counts (day, dim, key, extra, n)
             SELECT $1, $2,
                    CASE WHEN EXISTS (SELECT 1 FROM audience_counts WHERE day = $1 AND dim = $2 AND key = $3 AND extra = $4)
                           OR (SELECT count(*) FROM audience_counts WHERE day = $1 AND dim = $2) < $6
                         THEN $3 ELSE $7 END,
                    CASE WHEN EXISTS (SELECT 1 FROM audience_counts WHERE day = $1 AND dim = $2 AND key = $3 AND extra = $4)
                           OR (SELECT count(*) FROM audience_counts WHERE day = $1 AND dim = $2) < $6
                         THEN $4 ELSE '' END,
                    $5
             ON CONFLICT (day, dim, key, extra)
             DO UPDATE SET n = audience_counts.n + EXCLUDED.n",
        )
        .bind(day)
        .bind(dim)
        .bind(key)
        .bind(extra)
        .bind(by)
        .bind(MAX_KEYS_PER_DIM)
        .bind(OTHER_KEY)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("count {dim}: {e}")))?;
        Ok(())
    }

    /// [`Self::count_on`] for today (UTC).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn count(&self, dim: &str, key: &str, extra: &str, by: i64) -> Result<(), AppError> {
        self.count_on(chrono::Utc::now().date_naive(), dim, key, extra, by)
            .await
    }

    /// Notes a visitor hash for the UTC day `day`. True when it was new that day (the caller counts one unique
    /// visitor).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn first_visit_on(&self, day: NaiveDate, hash: &[u8]) -> Result<bool, AppError> {
        let done = sqlx::query(
            "INSERT INTO audience_visitors (day, hash) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(day)
        .bind(hash)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("note visitor: {e}")))?;
        Ok(done.rows_affected() == 1)
    }

    /// Deletes every visitor hash of a UTC day before `today`, and day counters older than `keep_days`. Run at
    /// startup and on a timer, so no identifier outlives its day on a quiet site either.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn purge(&self, today: NaiveDate, keep_days: i32) -> Result<(), AppError> {
        sqlx::query("DELETE FROM audience_visitors WHERE day < $1")
            .bind(today)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("purge visitors: {e}")))?;
        sqlx::query("DELETE FROM audience_counts WHERE day < $1 - $2::INT")
            .bind(today)
            .bind(keep_days)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("purge counts: {e}")))?;
        Ok(())
    }

    /// Daily views, visitors and bot hits for the last `days` days, oldest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn daily_traffic(&self, days: i32) -> Result<Vec<DayTraffic>, AppError> {
        sqlx::query_as(
            "SELECT d.day,
                    COALESCE(v.views, 0)::BIGINT    AS views,
                    COALESCE(c.visitors, 0)::BIGINT AS visitors,
                    COALESCE(c.bots, 0)::BIGINT     AS bots
             FROM (SELECT day FROM page_views WHERE day > CURRENT_DATE - $1::INT
                   UNION SELECT day FROM audience_counts WHERE day > CURRENT_DATE - $1::INT) d
             LEFT JOIN (SELECT day, sum(views) AS views FROM page_views GROUP BY day) v ON v.day = d.day
             LEFT JOIN (SELECT day,
                               sum(n) FILTER (WHERE dim = 'visitor') AS visitors,
                               sum(n) FILTER (WHERE dim = 'bot')     AS bots
                        FROM audience_counts GROUP BY day) c ON c.day = d.day
             ORDER BY d.day",
        )
        .bind(days)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("daily traffic: {e}")))
    }

    /// The most counted keys of one dimension over the last `days` days.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn top_of(
        &self,
        dim: &str,
        days: i32,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, AppError> {
        sqlx::query_as(
            "SELECT key, extra, sum(n)::BIGINT AS total FROM audience_counts
             WHERE dim = $1 AND day > CURRENT_DATE - $2::INT AND key <> ''
             GROUP BY key, extra ORDER BY total DESC, key LIMIT $3",
        )
        .bind(dim)
        .bind(days)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("top {dim}: {e}")))
    }

    /// Views of one path per day over the last `days` days, oldest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn page_trend(
        &self,
        path: &str,
        days: i32,
    ) -> Result<Vec<(NaiveDate, i64)>, AppError> {
        sqlx::query_as(
            "SELECT day, sum(views)::BIGINT FROM page_views
             WHERE path = $1 AND day > CURRENT_DATE - $2::INT
             GROUP BY day ORDER BY day",
        )
        .bind(path)
        .bind(days)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("page trend: {e}")))
    }

    /// Per-path views of the last `days` days against the `days` before them: `(path, now, before)`.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn path_trends(
        &self,
        days: i32,
        limit: i64,
    ) -> Result<Vec<(String, i64, i64)>, AppError> {
        sqlx::query_as(
            "SELECT path,
                    COALESCE(sum(views) FILTER (WHERE day > CURRENT_DATE - $1::INT), 0)::BIGINT AS recent,
                    COALESCE(sum(views) FILTER (WHERE day <= CURRENT_DATE - $1::INT), 0)::BIGINT AS prior
             FROM page_views WHERE day > CURRENT_DATE - 2 * $1::INT
             GROUP BY path ORDER BY recent DESC, path LIMIT $2",
        )
        .bind(days)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("path trends: {e}")))
    }

    /// The redirect named `name`.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn link(&self, name: &str) -> Result<Option<AudienceLink>, AppError> {
        sqlx::query_as("SELECT name, url, conversion FROM audience_links WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("link: {e}")))
    }

    /// Every redirect, by name.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn links(&self) -> Result<Vec<AudienceLink>, AppError> {
        sqlx::query_as("SELECT name, url, conversion FROM audience_links ORDER BY name")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("links: {e}")))
    }

    /// Creates or replaces the redirect `name`.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_link(&self, name: &str, url: &str, conversion: bool) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO audience_links (name, url, conversion) VALUES ($1, $2, $3)
             ON CONFLICT (name) DO UPDATE SET url = EXCLUDED.url, conversion = EXCLUDED.conversion",
        )
        .bind(name)
        .bind(url)
        .bind(conversion)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("set link: {e}")))?;
        Ok(())
    }

    /// Deletes the redirect `name`.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn delete_link(&self, name: &str) -> Result<(), AppError> {
        sqlx::query("DELETE FROM audience_links WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("delete link: {e}")))?;
        Ok(())
    }
}
