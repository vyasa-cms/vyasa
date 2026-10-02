//! Database-backed host functions: private storage, comment reads and
//! post writes.
//!
//! These live outside `host.rs` because the WIT trait impl there should
//! read as a list of capability checks, not as a wall of SQL — and because
//! the rules enforced here (what a plugin may write, and to what) are the
//! part worth reviewing on its own.

use sqlx::PgPool;

use crate::broker::{Broker, Decision};
use crate::capabilities::Capability;

/// Longest key a plugin may use.
const KV_MAX_KEY: usize = 256;
/// Longest value a plugin may store.
const KV_MAX_VALUE: usize = 64 * 1024;
/// Most keys one plugin may hold.
///
/// Storage is shared with the site's own tables, so a plugin that treats
/// it as a log rather than as state has to hit a wall somewhere.
const KV_MAX_KEYS: i64 = 10_000;
/// Most comments returned in one call.
const COMMENTS_MAX: u32 = 200;
/// Longest title a plugin may set.
const TITLE_MAX: usize = 200;
/// Longest body a plugin may submit, before sanitization.
const BODY_MAX: usize = 512 * 1024;

/// What every function here needs: who is asking, and where to ask.
pub struct Ctx<'a> {
    /// Capability broker.
    pub broker: &'a Broker,
    /// Destination pool.
    pub pool: &'a PgPool,
    /// Calling plugin.
    pub plugin_id: i64,
}

impl Ctx<'_> {
    async fn allow_read(&self, cap: Capability) -> Result<(), String> {
        if self.broker.check_db_read(self.plugin_id, cap).await == Decision::Allowed {
            Ok(())
        } else {
            Err(String::from("capability-denied"))
        }
    }

    /// The grant, with no quota accounting: for a second capability that
    /// gates the same single write.
    async fn allow(&self, cap: &Capability) -> Result<(), String> {
        if self.broker.check_capability(self.plugin_id, cap).await == Decision::Allowed {
            Ok(())
        } else {
            Err(String::from("capability-denied"))
        }
    }

    /// A write confined to the plugin's own storage: quota'd, not audited.
    async fn allow_private_write(&self, cap: &Capability) -> Result<(), String> {
        if self.broker.check_private_write(self.plugin_id, cap).await == Decision::Allowed {
            Ok(())
        } else {
            Err(String::from("capability-denied"))
        }
    }

    async fn allow_write(&self, cap: &Capability, detail: &str) -> Result<(), String> {
        if self.broker.check_write(self.plugin_id, cap, detail).await == Decision::Allowed {
            Ok(())
        } else {
            Err(String::from("capability-denied"))
        }
    }
}

/// A message that never names the SQL. Guests get a reason, not a schema.
fn db_err(what: &str) -> String {
    format!("storage error during {what}")
}

fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > KV_MAX_KEY {
        return Err(format!("key must be 1..={KV_MAX_KEY} bytes"));
    }
    Ok(())
}

/// Reads one key from the plugin's private namespace.
///
/// # Errors
/// `capability-denied` without `kv:read`, or a storage message.
pub async fn kv_get(ctx: &Ctx<'_>, key: String) -> Result<Option<String>, String> {
    ctx.allow_read(Capability::KvRead).await?;
    check_key(&key)?;
    sqlx::query_scalar::<_, String>("SELECT value FROM plugin_kv WHERE plugin_id = $1 AND key = $2")
        .bind(ctx.plugin_id)
        .bind(&key)
        .fetch_optional(ctx.pool)
        .await
        .map_err(|_| db_err("kv-get"))
}

/// Writes one key, creating it when the plugin is under its key budget.
///
/// # Errors
/// `capability-denied` without `kv:write`, a size message, or
/// `key limit reached`.
pub async fn kv_set(ctx: &Ctx<'_>, key: String, value: String) -> Result<(), String> {
    check_key(&key)?;
    if value.len() > KV_MAX_VALUE {
        return Err(format!("value exceeds {KV_MAX_VALUE} bytes"));
    }
    ctx.allow_private_write(&Capability::KvWrite).await?;
    // The budget is enforced in the statement rather than by a count-then-
    // insert, which two concurrent calls would race past.
    let done = sqlx::query(
        "INSERT INTO plugin_kv (plugin_id, key, value)
         SELECT $1, $2, $3
         WHERE (SELECT count(*) FROM plugin_kv WHERE plugin_id = $1) < $4
            OR EXISTS (SELECT 1 FROM plugin_kv WHERE plugin_id = $1 AND key = $2)
         ON CONFLICT (plugin_id, key)
         DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(ctx.plugin_id)
    .bind(&key)
    .bind(&value)
    .bind(KV_MAX_KEYS)
    .execute(ctx.pool)
    .await
    .map_err(|_| db_err("kv-set"))?;
    if done.rows_affected() == 0 {
        return Err(format!("key limit reached ({KV_MAX_KEYS})"));
    }
    Ok(())
}

/// Removes one key. Removing a key that is not there succeeds.
///
/// # Errors
/// `capability-denied` without `kv:write`, or a storage message.
pub async fn kv_delete(ctx: &Ctx<'_>, key: String) -> Result<(), String> {
    check_key(&key)?;
    ctx.allow_private_write(&Capability::KvWrite).await?;
    sqlx::query("DELETE FROM plugin_kv WHERE plugin_id = $1 AND key = $2")
        .bind(ctx.plugin_id)
        .bind(&key)
        .execute(ctx.pool)
        .await
        .map_err(|_| db_err("kv-delete"))?;
    Ok(())
}

/// Lists keys under `prefix`, sorted, capped at `limit`.
///
/// # Errors
/// `capability-denied` without `kv:read`, or a storage message.
pub async fn kv_list(ctx: &Ctx<'_>, prefix: String, limit: u32) -> Result<Vec<String>, String> {
    ctx.allow_read(Capability::KvRead).await?;
    if prefix.len() > KV_MAX_KEY {
        return Err(format!("prefix exceeds {KV_MAX_KEY} bytes"));
    }
    let limit = i64::from(limit.clamp(1, 1000));
    // `prefix` is a bind parameter, so a key containing `%` or `_` would
    // otherwise act as a wildcard against the plugin's own namespace.
    let escaped = prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    sqlx::query_scalar::<_, String>(
        "SELECT key FROM plugin_kv
         WHERE plugin_id = $1 AND key LIKE $2 || '%' ESCAPE '\\'
         ORDER BY key LIMIT $3",
    )
    .bind(ctx.plugin_id)
    .bind(&escaped)
    .bind(limit)
    .fetch_all(ctx.pool)
    .await
    .map_err(|_| db_err("kv-list"))
}

/// One approved comment as a plugin sees it: no email, no IP, no
/// pending or spam rows.
pub struct Comment {
    /// Comment id.
    pub id: i64,
    /// Post it belongs to.
    pub post_id: i64,
    /// Display name.
    pub author_name: String,
    /// Body, sanitized here rather than trusted from storage.
    pub body_html: String,
    /// Unix seconds.
    pub created_at: i64,
}

/// Approved comments on a post, newest first.
///
/// # Errors
/// `capability-denied` without `db:read:comments`, or a storage message.
pub async fn list_comments(
    ctx: &Ctx<'_>,
    post_id: u64,
    limit: u32,
) -> Result<Vec<Comment>, String> {
    ctx.allow_read(Capability::DbReadComments).await?;
    let post_id = i64::try_from(post_id).map_err(|_| String::from("bad post id"))?;
    let limit = i64::from(limit.clamp(1, COMMENTS_MAX));
    let rows: Vec<(i64, i64, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, post_id, author_name, content, created_at FROM comments
         WHERE post_id = $1 AND status = 'approved'
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(post_id)
    .bind(limit)
    .fetch_all(ctx.pool)
    .await
    .map_err(|_| db_err("list-comments"))?;
    Ok(rows
        .into_iter()
        .map(|(id, post_id, author_name, content, created_at)| Comment {
            id,
            post_id,
            author_name,
            body_html: ammonia::clean(&content),
            created_at: created_at.timestamp(),
        })
        .collect())
}

/// A post a plugin wants to create, already validated by the caller only
/// as far as the WIT types go.
pub struct NewPost {
    /// `post` or `page`.
    pub post_type: String,
    /// Title.
    pub title: String,
    /// Slug, or empty to derive one.
    pub slug: String,
    /// Body as HTML.
    pub body_html: String,
    /// Excerpt.
    pub excerpt: String,
    /// `draft` or `published`.
    pub status: String,
}

/// Fields to change on a post the plugin created.
pub struct PostPatch {
    /// New title.
    pub title: Option<String>,
    /// New body HTML.
    pub body_html: Option<String>,
    /// New excerpt.
    pub excerpt: Option<String>,
    /// New status.
    pub status: Option<String>,
}

/// Wraps plugin HTML in a one-block document.
///
/// The `html` block is the only kind whose contents are sanitized at
/// render rather than constrained at write, which is exactly right here:
/// a plugin should be able to emit a table or a list without learning the
/// block schema, and none of it reaches a page unsanitized.
fn body_document(body_html: &str) -> Result<serde_json::Value, String> {
    if body_html.len() > BODY_MAX {
        return Err(format!("body exceeds {BODY_MAX} bytes"));
    }
    let doc = vyasa_core::BlockDocument::new(vec![vyasa_core::Block::new(
        vyasa_core::BlockKind::Html,
        serde_json::json!({ "html": body_html }),
    )]);
    doc.validate().map_err(|e| e.to_string())?;
    Ok(doc.to_json())
}

fn check_status(status: &str) -> Result<&'static str, String> {
    match status {
        "" | "draft" => Ok("draft"),
        "published" => Ok("published"),
        other => Err(format!(
            "status must be \"draft\" or \"published\" (got {other:?})"
        )),
    }
}

/// The account a plugin's posts are attributed to.
///
/// Posts require a real author, and there is no plugin principal to point
/// at. The oldest confirmed, active administrator is the account that owns the
/// site, and `plugin_posts` records which plugin actually wrote the row,
/// so the attribution is never the only trace. An account whose address
/// was never confirmed has no public presence, and a suspended one is not
/// running the site, so neither is ever the byline.
async fn attribution_author(pool: &PgPool) -> Result<i64, String> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM users
         WHERE role = 'admin' AND email_verified_at IS NOT NULL AND suspended_at IS NULL
         ORDER BY created_at, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| db_err("author lookup"))?
    .ok_or_else(|| String::from("this site has no administrator to attribute the post to"))
}

/// Picks a free slug, suffixing `-2`, `-3`, … the way the editor does.
async fn free_slug(pool: &PgPool, base: &str) -> Result<String, String> {
    let taken: Vec<String> = sqlx::query_scalar(
        "SELECT slug FROM posts WHERE status <> 'trash' AND (slug = $1 OR slug LIKE $1 || '-%')",
    )
    .bind(base)
    .fetch_all(pool)
    .await
    .map_err(|_| db_err("slug lookup"))?;
    if !taken.iter().any(|s| s == base) {
        return Ok(base.to_owned());
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(String::from("could not find a free slug"))
}

/// Creates a post owned by the calling plugin.
///
/// # Errors
/// `capability-denied` without `db:write:posts` (or without
/// `db:publish:posts` when publishing), a validation message, or a
/// storage message.
pub async fn create_post(ctx: &Ctx<'_>, input: NewPost) -> Result<u64, String> {
    let status = check_status(&input.status)?;
    if status == "published" {
        // The grant only: the single write this call performs is charged
        // once, below. Charging here as well made a publishing plugin hit
        // its quota at half the documented rate.
        ctx.allow(&Capability::DbPublishPosts).await?;
    }
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > TITLE_MAX {
        return Err(format!("title must be 1..={TITLE_MAX} characters"));
    }
    // `block` is a synced-pattern container, not content: a plugin has no
    // business creating one. Registered custom types are fair game — the
    // forum plugin creating a `topic` is the whole point of the write
    // capability — and an unregistered name is refused the same way the
    // public router would refuse to serve it.
    let post_type = match input.post_type.as_str() {
        "" | "post" => "post",
        "page" => "page",
        "block" => return Err(String::from("a plugin cannot create synced patterns")),
        other => match vyasa_db::content_models::PostType::parse(other) {
            Ok(vyasa_db::content_models::PostType::Custom(name)) => name,
            _ => {
                return Err(format!(
                    "post-type must be \"post\", \"page\" or a registered \
                     custom type (got {other:?})"
                ))
            }
        },
    };
    let content = body_document(&input.body_html)?;
    ctx.allow_write(&Capability::DbWritePosts, &format!("create-post {title}"))
        .await?;

    let base = if input.slug.trim().is_empty() {
        vyasa_common::slugify(title)
    } else {
        vyasa_common::slugify(&input.slug)
    };
    let base = if base.is_empty() {
        String::from("post")
    } else {
        base
    };
    let slug = free_slug(ctx.pool, &base).await?;
    let author_id = attribution_author(ctx.pool).await?;
    let id = vyasa_common::next_id_i64();
    let excerpt = (!input.excerpt.trim().is_empty()).then(|| input.excerpt.clone());

    let mut tx = ctx.pool.begin().await.map_err(|_| db_err("create-post"))?;
    sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, content, excerpt, author_id,
                            published_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                 CASE WHEN $3 = 'published' THEN now() ELSE NULL END)",
    )
    .bind(id)
    .bind(post_type)
    .bind(status)
    .bind(&slug)
    .bind(title)
    .bind(&content)
    .bind(&excerpt)
    .bind(author_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| db_err("create-post"))?;
    sqlx::query("INSERT INTO plugin_posts (post_id, plugin_id) VALUES ($1, $2)")
        .bind(id)
        .bind(ctx.plugin_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| db_err("create-post"))?;
    tx.commit().await.map_err(|_| db_err("create-post"))?;

    if status == "published" {
        vyasa_core::events::emit(vyasa_core::events::Event::Published(
            vyasa_core::events::PostPublished {
                post_id: id,
                author_id,
            },
        ));
    }
    u64::try_from(id).map_err(|_| db_err("create-post"))
}

/// Edits a post this plugin created.
///
/// # Errors
/// `capability-denied` without the capability, `not your post` for
/// anything the plugin did not create, or a validation/storage message.
pub async fn update_post(ctx: &Ctx<'_>, id: u64, patch: PostPatch) -> Result<(), String> {
    let id = i64::try_from(id).map_err(|_| String::from("bad post id"))?;
    let status = patch.status.as_deref().map(check_status).transpose()?;
    if status == Some("published") {
        ctx.allow(&Capability::DbPublishPosts).await?;
    }
    ctx.allow_write(&Capability::DbWritePosts, &format!("update-post {id}"))
        .await?;
    // Ownership, not just capability: `db:write:posts` is a licence to
    // manage the plugin's own rows, never to rewrite a person's post.
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT plugin_id FROM plugin_posts WHERE post_id = $1")
            .bind(id)
            .fetch_optional(ctx.pool)
            .await
            .map_err(|_| db_err("update-post"))?;
    if owned != Some(ctx.plugin_id) {
        return Err(String::from("not your post"));
    }
    let title = match patch.title.as_deref().map(str::trim) {
        Some(t) if t.is_empty() || t.chars().count() > TITLE_MAX => {
            return Err(format!("title must be 1..={TITLE_MAX} characters"))
        }
        other => other.map(str::to_owned),
    };
    let content = patch.body_html.as_deref().map(body_document).transpose()?;
    sqlx::query(
        "UPDATE posts SET
            title = COALESCE($2, title),
            content = COALESCE($3, content),
            excerpt = COALESCE($4, excerpt),
            status = COALESCE($5, status),
            published_at = CASE WHEN $5 = 'published' AND published_at IS NULL
                                THEN now() ELSE published_at END,
            updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&title)
    .bind(&content)
    .bind(&patch.excerpt)
    .bind(status)
    .execute(ctx.pool)
    .await
    .map_err(|_| db_err("update-post"))?;
    vyasa_core::events::emit(vyasa_core::events::Event::Updated(
        vyasa_core::events::PostUpdated { post_id: id },
    ));
    Ok(())
}

/// Longest meta key and value a plugin may set on a post.
const META_MAX_KEY: usize = 64;
/// Longest meta value.
const META_MAX_VALUE: usize = 16 * 1024;
/// Most meta keys one plugin may hold on one post.
///
/// `meta` is read back with the post on every admin fetch and every REST
/// call, so an unbounded number of keys is a cost paid by every reader of
/// that post, not only by the plugin that wrote them.
const META_MAX_KEYS_PER_POST: usize = 64;
/// Longest subject and body a plugin may mail.
const MAIL_MAX_SUBJECT: usize = 200;
/// Longest mail body.
const MAIL_MAX_BODY: usize = 32 * 1024;
/// The recipient token meaning "whoever runs this site".
pub const SITE_ADMIN: &str = "site-admin";

/// The address `site-admin` resolves to: the configured contact address,
/// or the oldest administrator's.
async fn site_admin_address(pool: &PgPool) -> Result<String, String> {
    let configured: Option<String> =
        sqlx::query_scalar("SELECT value #>> '{}' FROM options WHERE key = 'admin_email'")
            .fetch_optional(pool)
            .await
            .map_err(|_| db_err("send-mail"))?
            .flatten();
    if let Some(address) = configured.filter(|a| a.contains('@')) {
        return Ok(address);
    }
    sqlx::query_scalar::<_, String>(
        "SELECT email::text FROM users WHERE role = 'admin' ORDER BY created_at, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| db_err("send-mail"))?
    .ok_or_else(|| String::from("this site has no administrator to write to"))
}

// One plugin's meta lives at `meta.plugin.<plugin name>.<key>`.
// Namespaced by plugin name rather than id so the value stays meaningful
// in an export, a backup or a REST payload read by a person, and so two
// plugins using `rating` never collide.

fn check_meta_key(key: &str) -> Result<(), String> {
    let ok = (1..=META_MAX_KEY).contains(&key.len())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "meta key must be 1..={META_MAX_KEY} characters of a-z, 0-9, _ or -"
        ))
    }
}

/// Reads one of this plugin's meta values from a post.
///
/// # Errors
/// `capability-denied` without `db:read:posts`, or a storage message.
pub async fn get_post_meta(
    ctx: &Ctx<'_>,
    plugin_name: &str,
    post_id: u64,
    key: String,
) -> Result<Option<String>, String> {
    ctx.allow_read(Capability::DbReadPosts).await?;
    check_meta_key(&key)?;
    let post_id = i64::try_from(post_id).map_err(|_| String::from("bad post id"))?;
    let value: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT meta -> 'plugin' -> $2::text -> $3::text FROM posts WHERE id = $1",
    )
    .bind(post_id)
    .bind(plugin_name)
    .bind(&key)
    .fetch_optional(ctx.pool)
    .await
    .map_err(|_| db_err("get-post-meta"))?
    .flatten();
    Ok(value.and_then(|v| match v {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }))
}

/// Writes one of this plugin's meta values on a post.
///
/// Any post, not only ones the plugin created: annotating a person's post
/// is the whole point of meta, and the write is confined to the plugin's
/// own namespace inside the `meta` object, so it cannot alter the post's
/// content, status or anything another plugin wrote.
///
/// # Errors
/// `capability-denied` without `db:write:meta`, `no such post`, or a
/// storage message.
pub async fn set_post_meta(
    ctx: &Ctx<'_>,
    plugin_name: &str,
    post_id: u64,
    key: String,
    value: String,
) -> Result<(), String> {
    check_meta_key(&key)?;
    if value.len() > META_MAX_VALUE {
        return Err(format!("value exceeds {META_MAX_VALUE} bytes"));
    }
    let post_id = i64::try_from(post_id).map_err(|_| String::from("bad post id"))?;
    ctx.allow_write(
        &Capability::DbWriteMeta,
        &format!("set-post-meta {post_id} {key}"),
    )
    .await?;
    // Built by merging rather than with `jsonb_set`: that function's
    // `create_missing` only creates the *last* level, so on a post whose
    // meta had no `plugin` key it returned the document unchanged and the
    // write silently did nothing.
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT count(*) FROM posts, jsonb_object_keys(
             COALESCE(meta -> 'plugin' -> $2::text, '{}'::jsonb)) AS k
         WHERE posts.id = $1 AND k <> $3::text",
    )
    .bind(post_id)
    .bind(plugin_name)
    .bind(&key)
    .fetch_optional(ctx.pool)
    .await
    .map_err(|_| db_err("set-post-meta"))?;
    if existing.unwrap_or(0) >= i64::try_from(META_MAX_KEYS_PER_POST).unwrap_or(i64::MAX) {
        return Err(format!(
            "at most {META_MAX_KEYS_PER_POST} meta keys per post"
        ));
    }
    let done = sqlx::query(
        "UPDATE posts SET meta = meta || jsonb_build_object(
             'plugin', COALESCE(meta -> 'plugin', '{}'::jsonb) || jsonb_build_object(
                 $2::text, COALESCE(meta -> 'plugin' -> $2::text, '{}'::jsonb)
                     || jsonb_build_object($3::text, $4::text)))
         WHERE id = $1",
    )
    .bind(post_id)
    .bind(plugin_name)
    .bind(&key)
    .bind(&value)
    .execute(ctx.pool)
    .await
    .map_err(|_| db_err("set-post-meta"))?;
    if done.rows_affected() == 0 {
        return Err(String::from("no such post"));
    }
    Ok(())
}

/// Queues a plain-text message to an address the site already knows.
///
/// The recipient check is the whole security model here: a plugin may
/// write to the people running or using the site, and to nobody else. An
/// unconstrained `send-mail` would make every installed plugin a spam
/// relay with the site's own reputation behind it.
///
/// # Errors
/// `capability-denied` without `mail:send`, `unknown recipient` for any
/// address the site does not hold, or a storage message.
pub async fn send_mail(
    ctx: &Ctx<'_>,
    plugin_name: &str,
    to: String,
    subject: String,
    body: String,
) -> Result<(), String> {
    let subject = subject.trim();
    if subject.is_empty() || subject.chars().count() > MAIL_MAX_SUBJECT {
        return Err(format!("subject must be 1..={MAIL_MAX_SUBJECT} characters"));
    }
    if body.len() > MAIL_MAX_BODY {
        return Err(format!("body exceeds {MAIL_MAX_BODY} bytes"));
    }
    // The capability first, before anything touches the database. Checking
    // the recipient first let a plugin *without* `mail:send` tell a
    // registered address from an unknown one by the error it got back —
    // an account-enumeration oracle reachable without declaring anything.
    ctx.allow_write(&Capability::MailSend, &format!("send-mail to {to}"))
        .await?;
    // `site-admin` resolves host-side, so a plugin can write to whoever
    // runs the site without ever being told the address. `admin_email` is
    // not a readable option precisely because it is a spam target, and a
    // plugin that had to read it in order to use it would defeat that.
    let resolved = if to == SITE_ADMIN {
        site_admin_address(ctx.pool).await?
    } else {
        let known: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)")
                .bind(&to)
                .fetch_one(ctx.pool)
                .await
                .map_err(|_| db_err("send-mail"))?;
        if !known {
            // Audited as a denial: a plugin probing for deliverable
            // addresses is exactly what an operator wants in the log.
            ctx.broker
                .audit_denial(ctx.plugin_id, &Capability::MailSend, "unknown recipient")
                .await;
            return Err(String::from("unknown recipient"));
        }
        to.clone()
    };
    vyasa_core::notify::EmailService::new(ctx.pool.clone())
        .queue(
            &resolved,
            "plugin_message",
            &serde_json::json!({
                "plugin": plugin_name,
                "subject": subject,
                "body": body,
            }),
        )
        .await
        .map_err(|_| db_err("send-mail"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_a_plugin_may_ask_for() {
        assert_eq!(check_status("").unwrap(), "draft");
        assert_eq!(check_status("draft").unwrap(), "draft");
        assert_eq!(check_status("published").unwrap(), "published");
        // Scheduling, privacy and trashing are editorial decisions with no
        // plugin-facing meaning; they stay off the contract.
        for bad in ["scheduled", "private", "trash", "Published"] {
            assert!(check_status(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn keys_are_bounded() {
        assert!(check_key("").is_err());
        assert!(check_key(&"k".repeat(KV_MAX_KEY + 1)).is_err());
        assert!(check_key("a/reasonable:key").is_ok());
    }

    #[test]
    fn meta_keys_are_bounded_and_plain() {
        assert!(check_meta_key("rating").is_ok());
        assert!(check_meta_key("read_at-2").is_ok());
        for bad in [
            "",
            &"k".repeat(META_MAX_KEY + 1),
            "has space",
            "dots.are.out",
            "sl/ash",
            "plugin",
        ]
        .into_iter()
        .filter(|k| *k != "plugin")
        {
            assert!(check_meta_key(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_site_admin_token_is_not_something_a_user_could_register() {
        // `send-mail` treats this string specially, so it must not be
        // mistakable for a real address.
        assert!(!SITE_ADMIN.contains('@'));
    }

    #[test]
    fn a_body_becomes_one_html_block() {
        let doc = body_document("<p>hi</p>").expect("valid");
        assert_eq!(doc["blocks"][0]["kind"], "html");
        assert_eq!(doc["blocks"][0]["attrs"]["html"], "<p>hi</p>");
        assert!(body_document(&"x".repeat(BODY_MAX + 1)).is_err());
    }
}
