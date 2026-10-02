//! Content fields repository: field definitions in `content_fields`, and
//! the queries over the values entries store in `posts.meta.fields`.
//! Key and kind rules and value validation live in `vyasa-core`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use vyasa_common::AppError;

const FIELD_COLUMNS: &str = "type_slug, key, label, help, kind, required, options, position, \
                             created_at, updated_at";

/// What counts as no value at all: absent, `null`, `""` or `[]`.
const EMPTY_VALUES: &str = "('null'::jsonb, '\"\"'::jsonb, '[]'::jsonb)";

/// One field definition.
#[derive(Clone, Debug, PartialEq, sqlx::FromRow, serde::Serialize)]
pub struct ContentFieldRow {
    /// The type the field belongs to (built-in, plugin or admin).
    pub type_slug: String,
    /// Key in `meta.fields`; immutable.
    pub key: String,
    /// Label shown in the editor.
    pub label: String,
    /// Help text shown under the input.
    pub help: String,
    /// Kind (`text`, `number`, …).
    pub kind: String,
    /// Whether publishing or scheduling needs a value.
    pub required: bool,
    /// Per-kind options.
    pub options: serde_json::Value,
    /// Order within the type, lowest first.
    pub position: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last change.
    pub updated_at: DateTime<Utc>,
}

/// Parameters for inserting a field; it goes after the type's last one.
#[derive(Clone, Debug)]
pub struct NewContentField<'a> {
    /// Owning type.
    pub type_slug: &'a str,
    /// Key.
    pub key: &'a str,
    /// Label.
    pub label: &'a str,
    /// Help text.
    pub help: &'a str,
    /// Kind.
    pub kind: &'a str,
    /// Required on publish.
    pub required: bool,
    /// Options (a JSON object).
    pub options: serde_json::Value,
}

/// Fields of a definition to change; `None` leaves one as it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContentFieldUpdate<'a> {
    /// New label.
    pub label: Option<&'a str>,
    /// New help text.
    pub help: Option<&'a str>,
    /// New kind.
    pub kind: Option<&'a str>,
    /// New required flag.
    pub required: Option<bool>,
    /// New options (replace the old ones).
    pub options: Option<&'a serde_json::Value>,
}

/// Data access for `content_fields` and field values.
#[derive(Clone, Debug)]
pub struct ContentFieldsRepo {
    pool: PgPool,
}

impl ContentFieldsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A type's fields in order.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self, type_slug: &str) -> Result<Vec<ContentFieldRow>, AppError> {
        sqlx::query_as(&format!(
            "SELECT {FIELD_COLUMNS} FROM content_fields
             WHERE type_slug = $1 ORDER BY position, key"
        ))
        .bind(type_slug)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content field list failed: {err}")))
    }

    /// Every field of every type, by type then position (an export).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_all(&self) -> Result<Vec<ContentFieldRow>, AppError> {
        sqlx::query_as(&format!(
            "SELECT {FIELD_COLUMNS} FROM content_fields ORDER BY type_slug, position, key"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content field list failed: {err}")))
    }

    /// One field.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Db`] on
    /// database failure.
    pub async fn get(&self, type_slug: &str, key: &str) -> Result<ContentFieldRow, AppError> {
        sqlx::query_as(&format!(
            "SELECT {FIELD_COLUMNS} FROM content_fields WHERE type_slug = $1 AND key = $2"
        ))
        .bind(type_slug)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content field lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("content_field", format!("{type_slug}.{key}")))
    }

    /// How many fields a type has.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count(&self, type_slug: &str) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM content_fields WHERE type_slug = $1")
            .bind(type_slug)
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("content field count failed: {err}")))
    }

    /// Inserts a field after the type's last one.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Conflict`] when the key is taken on the type,
    /// [`AppError::Db`] on database failure (including a value the
    /// table's constraints refuse).
    pub async fn insert(&self, field: &NewContentField<'_>) -> Result<ContentFieldRow, AppError> {
        sqlx::query_as(&format!(
            "INSERT INTO content_fields (type_slug, key, label, help, kind, required, options, position)
             VALUES ($1, $2, $3, $4, $5, $6, $7,
                     (SELECT COALESCE(MAX(position) + 1, 0) FROM content_fields WHERE type_slug = $1))
             RETURNING {FIELD_COLUMNS}"
        ))
        .bind(field.type_slug)
        .bind(field.key)
        .bind(field.label)
        .bind(field.help)
        .bind(field.kind)
        .bind(field.required)
        .bind(&field.options)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| match err {
            sqlx::Error::Database(db) if db.is_unique_violation() => AppError::conflict(format!(
                "the type {:?} already has a field {:?}",
                field.type_slug, field.key
            )),
            err => AppError::db(format!("content field insert failed: {err}")),
        })
    }

    /// Updates a field; each part is optional.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Db`] on
    /// database failure.
    pub async fn update(
        &self,
        type_slug: &str,
        key: &str,
        update: &ContentFieldUpdate<'_>,
    ) -> Result<ContentFieldRow, AppError> {
        sqlx::query_as(&format!(
            "UPDATE content_fields SET
                label = COALESCE($3, label),
                help = COALESCE($4, help),
                kind = COALESCE($5, kind),
                required = COALESCE($6, required),
                options = COALESCE($7, options),
                updated_at = now()
             WHERE type_slug = $1 AND key = $2
             RETURNING {FIELD_COLUMNS}"
        ))
        .bind(type_slug)
        .bind(key)
        .bind(update.label)
        .bind(update.help)
        .bind(update.kind)
        .bind(update.required)
        .bind(update.options)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content field update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("content_field", format!("{type_slug}.{key}")))
    }

    /// Deletes a field definition; stored values stay (see
    /// [`ContentFieldsRepo::clear_values`]). Returns whether a definition
    /// was deleted.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete(&self, type_slug: &str, key: &str) -> Result<bool, AppError> {
        let done = sqlx::query("DELETE FROM content_fields WHERE type_slug = $1 AND key = $2")
            .bind(type_slug)
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("content field delete failed: {err}")))?;
        Ok(done.rows_affected() > 0)
    }

    /// Sets the order of a type's fields to that of `keys` (the caller
    /// checks they are exactly the type's keys) and returns the fields in
    /// their new order.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn reorder(
        &self,
        type_slug: &str,
        keys: &[String],
    ) -> Result<Vec<ContentFieldRow>, AppError> {
        sqlx::query(
            "UPDATE content_fields f SET position = (k.ord - 1)::int, updated_at = now()
             FROM unnest($2::text[]) WITH ORDINALITY AS k(key, ord)
             WHERE f.type_slug = $1 AND f.key = k.key",
        )
        .bind(type_slug)
        .bind(keys)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content field reorder failed: {err}")))?;
        self.list(type_slug).await
    }

    /// Entries of the type (any status, trash included) storing a
    /// non-empty value for `key`. `null`, `""` and `[]` are no value.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_entries_with_value(
        &self,
        type_slug: &str,
        key: &str,
    ) -> Result<i64, AppError> {
        sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM posts
             WHERE type = $1 AND jsonb_typeof(meta->'fields') = 'object'
               AND (meta->'fields'->$2) IS NOT NULL
               AND (meta->'fields'->$2) NOT IN {EMPTY_VALUES}"
        ))
        .bind(type_slug)
        .bind(key)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("field value count failed: {err}")))
    }

    /// For each key with stored non-empty values on the type's entries
    /// (any status), how many entries hold one, by key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn stored_value_counts(
        &self,
        type_slug: &str,
    ) -> Result<Vec<(String, i64)>, AppError> {
        // `jsonb_each` refuses a non-object, and a WHERE clause is no
        // promise about evaluation order, so the guard sits inside it.
        sqlx::query_as(&format!(
            "SELECT f.key, COUNT(*) FROM posts p,
                    jsonb_each(CASE WHEN jsonb_typeof(p.meta->'fields') = 'object'
                                    THEN p.meta->'fields' ELSE '{{}}'::jsonb END) AS f(key, value)
             WHERE p.type = $1 AND f.value NOT IN {EMPTY_VALUES}
             GROUP BY f.key ORDER BY f.key"
        ))
        .bind(type_slug)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("field value counts failed: {err}")))
    }

    /// Removes `key` from the stored values of the type's entries (any
    /// status) and of their revisions and autosaves, in one transaction,
    /// so restoring a revision cannot bring a cleaned-up value back.
    /// Leaves `updated_at` alone: this is upkeep, not an edit. Returns how
    /// many entries changed.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn clear_values(&self, type_slug: &str, key: &str) -> Result<u64, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
        let entries = sqlx::query(
            "UPDATE posts SET meta = CASE
                    WHEN (meta->'fields') - $2 = '{}'::jsonb THEN meta - 'fields'
                    ELSE jsonb_set(meta, '{fields}', (meta->'fields') - $2) END
             WHERE type = $1 AND jsonb_typeof(meta->'fields') = 'object'
               AND (meta->'fields') ? $2",
        )
        .bind(type_slug)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::db(format!("field value clean-up failed: {err}")))?
        .rows_affected();
        sqlx::query(
            "UPDATE post_revisions r SET fields = r.fields - $2
             FROM posts p
             WHERE r.post_id = p.id AND p.type = $1
               AND jsonb_typeof(r.fields) = 'object' AND r.fields ? $2",
        )
        .bind(type_slug)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::db(format!("revision value clean-up failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("tx commit failed: {err}")))?;
        Ok(entries)
    }
}
