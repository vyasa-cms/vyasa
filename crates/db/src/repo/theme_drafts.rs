//! Theme drafts: the studio's working copies, their revisions, and the
//! conversation attached to each.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use vyasa_common::AppError;

/// One working copy.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThemeDraftRow {
    /// Snowflake id.
    pub id: i64,
    /// Working title.
    pub name: String,
    /// Theme the draft started from, if any.
    pub base_theme_id: Option<i64>,
    /// `ready`, `generating` or `failed`.
    pub status: String,
    /// Why the last assistant run failed, when it did.
    pub status_note: Option<String>,
    /// Design tokens.
    pub tokens: Value,
    /// Layout composition.
    pub layout: Value,
    /// Tera overrides keyed by template name.
    pub templates: Option<Value>,
    /// Theme-shipped CSS/JS, as `{ "css": "…", "js": "…" }`.
    pub assets: Option<Value>,
    /// Sequence of the revision the documents correspond to.
    pub revision: i32,
    /// Who created it.
    pub created_by: Option<i64>,
    /// When.
    pub created_at: DateTime<Utc>,
    /// Last change.
    pub updated_at: DateTime<Utc>,
}

/// One point in a draft's history.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThemeDraftRevisionRow {
    /// Snowflake id.
    pub id: i64,
    /// Owning draft.
    pub draft_id: i64,
    /// 1-based sequence within the draft.
    pub seq: i32,
    /// Tokens at this point.
    pub tokens: Value,
    /// Layout at this point.
    pub layout: Value,
    /// Templates at this point.
    pub templates: Option<Value>,
    /// Theme-shipped CSS/JS, as `{ "css": "…", "js": "…" }`.
    pub assets: Option<Value>,
    /// What changed, in words.
    pub note: String,
    /// `you`, `assistant`, `start` or `revert`.
    pub source: String,
    /// When.
    pub created_at: DateTime<Utc>,
}

/// One turn of the studio conversation.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThemeDraftMessageRow {
    /// Snowflake id.
    pub id: i64,
    /// Owning draft.
    pub draft_id: i64,
    /// `you` or `assistant`.
    pub role: String,
    /// The message.
    pub text: String,
    /// Revision the assistant produced in reply, when it changed something.
    pub revision: Option<i32>,
    /// Documents this reply would write, until someone accepts them.
    ///
    /// `{tokens, layout, templates, changes}`. Cleared on accept, so a
    /// proposal is offered exactly once.
    pub proposal: Option<serde_json::Value>,
    /// The draft revision the proposal was composed against.
    pub proposal_base: Option<i32>,
    /// When.
    pub created_at: DateTime<Utc>,
}

/// The three documents written together.
#[derive(Debug, Clone)]
pub struct DraftDocuments<'a> {
    /// Design tokens.
    pub tokens: &'a Value,
    /// Layout composition.
    pub layout: &'a Value,
    /// Tera overrides, if any.
    pub templates: Option<&'a Value>,
    /// Theme-shipped CSS/JS.
    pub assets: Option<&'a Value>,
}

const DRAFT_COLUMNS: &str = "id, name, base_theme_id, status, status_note, tokens, layout, \
                             templates, assets, revision, created_by, created_at, updated_at";
const REVISION_COLUMNS: &str =
    "id, draft_id, seq, tokens, layout, templates, assets, note, source, created_at";
const MESSAGE_COLUMNS: &str =
    "id, draft_id, role, text, revision, proposal, proposal_base, created_at";

/// Data access for `theme_drafts` and its satellites.
#[derive(Clone, Debug)]
pub struct ThemeDraftsRepo {
    pool: PgPool,
}

fn db_err(what: &str, err: &sqlx::Error) -> AppError {
    AppError::db(format!("{what}: {err}"))
}

impl ThemeDraftsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Creates a draft together with its first revision (`seq` 1,
    /// source `start`) in one transaction.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn create(
        &self,
        name: &str,
        base_theme_id: Option<i64>,
        docs: DraftDocuments<'_>,
        created_by: Option<i64>,
    ) -> Result<ThemeDraftRow, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| db_err("theme draft tx begin failed", &e))?;
        let row = sqlx::query_as::<_, ThemeDraftRow>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO theme_drafts (id, name, base_theme_id, tokens, layout, templates, assets, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {DRAFT_COLUMNS}"
        )))
        .bind(vyasa_common::next_id_i64())
        .bind(name)
        .bind(base_theme_id)
        .bind(docs.tokens)
        .bind(docs.layout)
        .bind(docs.templates)
        .bind(docs.assets)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| db_err("theme draft insert failed", &e))?;
        sqlx::query(
            "INSERT INTO theme_draft_revisions (id, draft_id, seq, tokens, layout, templates, assets, note, source)
             VALUES ($1, $2, 1, $3, $4, $5, $6, $7, 'start')",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(row.id)
        .bind(docs.tokens)
        .bind(docs.layout)
        .bind(docs.templates)
        .bind(docs.assets)
        .bind("Started the draft")
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err("theme draft first revision failed", &e))?;
        tx.commit()
            .await
            .map_err(|e| db_err("theme draft commit failed", &e))?;
        Ok(row)
    }

    /// One draft.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing, [`AppError::Db`] on failure.
    pub async fn get(&self, id: i64) -> Result<ThemeDraftRow, AppError> {
        sqlx::query_as::<_, ThemeDraftRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {DRAFT_COLUMNS} FROM theme_drafts WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err("theme draft lookup failed", &e))?
        .ok_or_else(|| AppError::not_found("theme draft", id))
    }

    /// All drafts, most recently touched first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list(&self) -> Result<Vec<ThemeDraftRow>, AppError> {
        sqlx::query_as::<_, ThemeDraftRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {DRAFT_COLUMNS} FROM theme_drafts ORDER BY updated_at DESC"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("theme draft list failed", &e))
    }

    /// Writes new documents as the next revision and points the draft at
    /// it, atomically. Returns the updated draft.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when the draft is gone, [`AppError::Db`]
    /// on failure.
    pub async fn commit_revision(
        &self,
        draft_id: i64,
        docs: DraftDocuments<'_>,
        note: &str,
        source: &str,
    ) -> Result<ThemeDraftRow, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| db_err("theme draft tx begin failed", &e))?;
        // Lock the draft row so two concurrent commits get distinct seqs.
        let current: Option<i32> =
            sqlx::query_scalar("SELECT revision FROM theme_drafts WHERE id = $1 FOR UPDATE")
                .bind(draft_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| db_err("theme draft lock failed", &e))?;
        let Some(_) = current else {
            return Err(AppError::not_found("theme draft", draft_id));
        };
        let next: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM theme_draft_revisions WHERE draft_id = $1",
        )
        .bind(draft_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| db_err("theme draft seq failed", &e))?;
        sqlx::query(
            "INSERT INTO theme_draft_revisions (id, draft_id, seq, tokens, layout, templates, assets, note, source)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(draft_id)
        .bind(next)
        .bind(docs.tokens)
        .bind(docs.layout)
        .bind(docs.templates)
        .bind(docs.assets)
        .bind(note)
        .bind(source)
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err("theme draft revision insert failed", &e))?;
        let row = sqlx::query_as::<_, ThemeDraftRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE theme_drafts SET tokens = $2, layout = $3, templates = $4, assets = $5,
                    revision = $6, updated_at = now()
             WHERE id = $1 RETURNING {DRAFT_COLUMNS}"
        )))
        .bind(draft_id)
        .bind(docs.tokens)
        .bind(docs.layout)
        .bind(docs.templates)
        .bind(docs.assets)
        .bind(next)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| db_err("theme draft update failed", &e))?;
        tx.commit()
            .await
            .map_err(|e| db_err("theme draft commit failed", &e))?;
        Ok(row)
    }

    /// Renames a draft.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing, [`AppError::Db`] on failure.
    pub async fn rename(&self, id: i64, name: &str) -> Result<ThemeDraftRow, AppError> {
        sqlx::query_as::<_, ThemeDraftRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE theme_drafts SET name = $2, updated_at = now() WHERE id = $1
             RETURNING {DRAFT_COLUMNS}"
        )))
        .bind(id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err("theme draft rename failed", &e))?
        .ok_or_else(|| AppError::not_found("theme draft", id))
    }

    /// Records the assistant's progress on a draft.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_status(
        &self,
        id: i64,
        status: &str,
        note: Option<&str>,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE theme_drafts SET status = $2, status_note = $3, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(note)
        .execute(&self.pool)
        .await
        .map_err(|e| db_err("theme draft status failed", &e))?;
        Ok(())
    }

    /// Deletes a draft with its revisions and messages.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing, [`AppError::Db`] on failure.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let n = sqlx::query("DELETE FROM theme_drafts WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| db_err("theme draft delete failed", &e))?
            .rows_affected();
        if n == 0 {
            return Err(AppError::not_found("theme draft", id));
        }
        Ok(())
    }

    /// A draft's history, newest first. Documents are included so a
    /// caller can diff or restore without a second query.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn revisions(&self, draft_id: i64) -> Result<Vec<ThemeDraftRevisionRow>, AppError> {
        sqlx::query_as::<_, ThemeDraftRevisionRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM theme_draft_revisions
             WHERE draft_id = $1 ORDER BY seq DESC"
        )))
        .bind(draft_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("theme draft revisions failed", &e))
    }

    /// One revision by sequence.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing, [`AppError::Db`] on failure.
    pub async fn revision(
        &self,
        draft_id: i64,
        seq: i32,
    ) -> Result<ThemeDraftRevisionRow, AppError> {
        sqlx::query_as::<_, ThemeDraftRevisionRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM theme_draft_revisions
             WHERE draft_id = $1 AND seq = $2"
        )))
        .bind(draft_id)
        .bind(seq)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err("theme draft revision failed", &e))?
        .ok_or_else(|| AppError::not_found("theme draft revision", seq))
    }

    /// Appends a conversation turn.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn add_message(
        &self,
        draft_id: i64,
        role: &str,
        text: &str,
        revision: Option<i32>,
    ) -> Result<ThemeDraftMessageRow, AppError> {
        self.add_reply(draft_id, role, text, revision, None, None)
            .await
    }

    /// Appends a turn that may carry a proposal awaiting acceptance.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn add_reply(
        &self,
        draft_id: i64,
        role: &str,
        text: &str,
        revision: Option<i32>,
        proposal: Option<&serde_json::Value>,
        proposal_base: Option<i32>,
    ) -> Result<ThemeDraftMessageRow, AppError> {
        sqlx::query_as::<_, ThemeDraftMessageRow>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO theme_draft_messages
                 (id, draft_id, role, text, revision, proposal, proposal_base)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {MESSAGE_COLUMNS}"
        )))
        .bind(vyasa_common::next_id_i64())
        .bind(draft_id)
        .bind(role)
        .bind(text)
        .bind(revision)
        .bind(proposal)
        .bind(proposal_base)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("theme draft message failed", &e))
    }

    /// One message of a draft.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when it does not belong to that draft.
    pub async fn message(
        &self,
        draft_id: i64,
        message_id: i64,
    ) -> Result<ThemeDraftMessageRow, AppError> {
        sqlx::query_as::<_, ThemeDraftMessageRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {MESSAGE_COLUMNS} FROM theme_draft_messages
             WHERE id = $1 AND draft_id = $2"
        )))
        .bind(message_id)
        .bind(draft_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err("theme draft message lookup failed", &e))?
        .ok_or_else(|| AppError::not_found("message", message_id))
    }

    /// Clears a proposal and records the revision it became.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn settle_proposal(
        &self,
        message_id: i64,
        revision: Option<i32>,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE theme_draft_messages
             SET proposal = NULL, proposal_base = NULL, revision = COALESCE($2, revision)
             WHERE id = $1",
        )
        .bind(message_id)
        .bind(revision)
        .execute(&self.pool)
        .await
        .map_err(|e| db_err("settling the proposal failed", &e))?;
        Ok(())
    }

    /// The conversation, oldest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn messages(&self, draft_id: i64) -> Result<Vec<ThemeDraftMessageRow>, AppError> {
        sqlx::query_as::<_, ThemeDraftMessageRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {MESSAGE_COLUMNS} FROM theme_draft_messages
             WHERE draft_id = $1 ORDER BY created_at ASC, id ASC"
        )))
        .bind(draft_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("theme draft messages failed", &e))
    }
}
