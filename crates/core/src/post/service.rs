//! Post domain service: lifecycle rules over the block format.

use vyasa_common::{slugify, AppError};
use vyasa_db::content_models::{PostRow, PostStatus, PostType};
use vyasa_db::repo::{NewPost, PostFilter, PostUpdate, PostsRepo};

use crate::block::BlockDocument;
use crate::content::ContentFieldsService;
use crate::events;
use crate::post::status as post_status;
use crate::post::token;

// Re-export transition helpers for tests.
pub use crate::post::status::{ensure_transition, transition_allowed};

/// Post lifecycle operations.
#[derive(Clone, Debug)]
pub struct PostService {
    posts: PostsRepo,
    terms: vyasa_db::repo::TermsRepo,
    fields: ContentFieldsService,
}

/// Input for creating a post.
pub struct CreatePost {
    /// Content type (post/page/block).
    pub post_type: PostType,
    /// Initial status (default draft).
    pub status: PostStatus,
    /// Title (required, used for slug default).
    pub title: String,
    /// Desired slug (slugified; derived from title when `None`).
    pub slug: Option<String>,
    /// Block content (validated).
    pub content: BlockDocument,
    /// Optional excerpt.
    pub excerpt: Option<String>,
    /// Author id.
    pub author_id: i64,
    /// Optional parent id.
    pub parent_id: Option<i64>,
    /// Optional scheduled time (required when status is scheduled).
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional password for private posts (hashed with argon2).
    pub password: Option<String>,
    /// Optional term ids to assign (replace-set).
    pub term_ids: Option<Vec<i64>>,
    /// Section tree composing this entry, when it has one.
    ///
    /// Opaque here: sections are a theme concept and this crate sits below
    /// the theme crate, so the caller validates the tree before handing it
    /// over. Stored verbatim.
    pub layout: Option<serde_json::Value>,
}

/// Input for updating a post; `None` fields keep current values.
#[derive(Default)]
pub struct UpdatePost {
    /// New status.
    pub status: Option<PostStatus>,
    /// New title.
    pub title: Option<String>,
    /// New slug.
    pub slug: Option<String>,
    /// New content.
    pub content: Option<BlockDocument>,
    /// New excerpt.
    pub excerpt: Option<String>,
    /// New scheduled time.
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    /// Drop the schedule. `scheduled_for: None` means "keep", so
    /// unscheduling needs its own word.
    pub clear_schedule: bool,
    /// New password (`None` keeps, `Some("")` clears, `Some(pwd)` sets).
    pub password: Option<String>,
    /// New term ids (`None` keeps, `Some(ids)` replaces).
    pub term_ids: Option<Vec<i64>>,
    /// Free-form metadata (SEO fields and the like); `None` keeps it.
    pub meta: Option<serde_json::Value>,
    /// New custom field values (a JSON object keyed by field key),
    /// validated and replacing the stored ones; `None` keeps them. The
    /// only way values are written: a `fields` key inside `meta` is
    /// ignored.
    pub fields: Option<serde_json::Value>,
    /// New section tree; `None` keeps the current one, an empty array
    /// clears it so the entry renders through its theme template again.
    /// Opaque and caller-validated, as on [`CreatePost`].
    pub layout: Option<serde_json::Value>,
}

impl PostService {
    /// Creates a service over the posts repository.
    #[must_use]
    pub fn new(posts: PostsRepo) -> Self {
        let terms = vyasa_db::repo::TermsRepo::new(posts.pool().clone());
        Self::with_terms(posts, terms)
    }

    /// Creates a service over the given repositories (for term-aware tests).
    #[must_use]
    pub fn with_terms(posts: PostsRepo, terms: vyasa_db::repo::TermsRepo) -> Self {
        let fields = ContentFieldsService::new(posts.pool().clone());
        Self {
            posts,
            terms,
            fields,
        }
    }

    /// Creates a post: validates content, derives a unique slug, inserts.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for empty titles, invalid blocks,
    /// scheduling mistakes, or publishing or scheduling an entry whose
    /// type has required fields; [`AppError::Db`] on database failure.
    pub async fn create(&self, input: CreatePost) -> Result<PostRow, AppError> {
        self.create_with_fields(input, None).await
    }

    /// Creates a post with custom field values (`meta.fields`), checked
    /// by [`ContentFieldsService::validate_values`]; a published or
    /// scheduled entry also needs every required field.
    ///
    /// # Errors
    ///
    /// As [`PostService::create`], and [`AppError::Validation`] naming
    /// each field (`fields.<key>: …`) whose value is refused or missing.
    pub async fn create_with_fields(
        &self,
        input: CreatePost,
        fields: Option<serde_json::Value>,
    ) -> Result<PostRow, AppError> {
        let title = input.title.trim().to_string();
        if title.is_empty() {
            return Err(AppError::validation("title must not be empty"));
        }
        input.content.validate()?;
        if input.status == PostStatus::Scheduled && input.scheduled_for.is_none() {
            return Err(AppError::validation("scheduled posts need scheduled_for"));
        }
        if input.status == PostStatus::Private && input.password.is_some() {
            // Validate password length if provided for private creation.
            if let Some(pwd) = &input.password {
                if !pwd.is_empty() && pwd.len() < 8 {
                    return Err(AppError::validation(
                        "password must be at least 8 characters",
                    ));
                }
            }
        }
        let base_slug = match &input.slug {
            Some(slug) => {
                let slug = slugify(slug);
                if slug.is_empty() {
                    return Err(AppError::validation("slug must not be empty"));
                }
                slug
            }
            None => slugify(&title),
        };
        let base_slug = if base_slug.is_empty() {
            format!("post-{}", vyasa_common::next_id_i64())
        } else {
            base_slug
        };
        let password_hash = match input.password {
            Some(pwd) if !pwd.is_empty() => Some(crate::user::password::hash_password(&pwd)?),
            _ => None,
        };
        // Terms are checked before anything is written, so a bad id is a
        // clean 404 and never leaves a post behind without its terms.
        let term_ids = match input.term_ids {
            Some(ids) => Some(self.checked_term_ids(ids).await?),
            None => None,
        };
        let values = match &fields {
            Some(raw) => {
                self.fields
                    .validate_values(input.post_type, raw, None)
                    .await?
            }
            None => serde_json::Map::new(),
        };
        if publishes(input.status) {
            self.fields.check_required(input.post_type, &values).await?;
        }
        let meta = if values.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::json!({ "fields": values })
        };

        let published_at = (input.status == PostStatus::Published).then(chrono::Utc::now);
        let content = input.content.to_json();
        // The repo probes for a free slug and inserts in one transaction,
        // serialised per (type, base slug), so same-title creates take
        // turns. A writer outside that lock (an update renaming a slug, a
        // create from another base that lands on one of our suffixes) can
        // still win the slug; the unique index turns that into a Conflict
        // and the create probes again.
        let mut attempt = 0;
        let row = loop {
            attempt += 1;
            let result = self
                .posts
                .insert_with_unique_slug(
                    &NewPost {
                        id: vyasa_common::next_id_i64(),
                        post_type: input.post_type,
                        status: input.status,
                        slug: base_slug.clone(),
                        title: title.clone(),
                        content: content.clone(),
                        excerpt: input.excerpt.clone(),
                        author_id: input.author_id,
                        parent_id: input.parent_id,
                        meta: meta.clone(),
                        published_at,
                        scheduled_for: input.scheduled_for,
                        password_hash: password_hash.clone(),
                        layout: input.layout.clone(),
                    },
                    term_ids.as_deref(),
                )
                .await;
            match result {
                Err(AppError::Conflict { .. }) if attempt < SLUG_ATTEMPTS => {}
                other => break other?,
            }
        };
        if row.status == PostStatus::Published {
            events::emit_published(row.id, row.author_id);
        } else {
            events::emit_updated(row.id);
        }
        Ok(row)
    }

    /// Dedups `ids` and checks that each names a term.
    async fn checked_term_ids(&self, ids: Vec<i64>) -> Result<Vec<i64>, AppError> {
        let mut deduped = ids;
        deduped.sort_unstable();
        deduped.dedup();
        for term_id in &deduped {
            self.terms.get(*term_id).await?;
        }
        Ok(deduped)
    }

    /// Fetches a post by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<PostRow, AppError> {
        self.posts.get(id).await
    }

    /// Fetches a post by type + slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing or trashed.
    pub async fn get_by_slug(&self, post_type: PostType, slug: &str) -> Result<PostRow, AppError> {
        self.posts.get_by_slug(post_type, slug).await
    }

    /// Lists posts by filter, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self, filter: &PostFilter) -> Result<Vec<PostRow>, AppError> {
        self.posts.list(filter).await
    }

    /// Counts posts matching a filter.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count(&self, filter: &PostFilter) -> Result<i64, AppError> {
        self.posts.count(filter).await
    }

    /// Updates a post: validates new content, checks status transition,
    /// re-checks slug uniqueness when the slug changes.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] on rule violations (with the
    /// offending transition named, or each refused or missing field value
    /// as `fields.<key>: …`), [`AppError::NotFound`] when missing.
    #[allow(clippy::too_many_lines)]
    pub async fn update(&self, id: i64, input: UpdatePost) -> Result<PostRow, AppError> {
        let current = self.posts.get(id).await?;
        if let Some(content) = &input.content {
            content.validate()?;
        }
        if input.meta.as_ref().is_some_and(|m| !m.is_object()) {
            return Err(AppError::validation("meta must be a JSON object"));
        }
        if let Some(status) = input.status {
            post_status::ensure_transition(current.status, status)?;
            // A cleared schedule does not count as "already has a date".
            let has_date = input.scheduled_for.is_some()
                || (!input.clear_schedule && current.scheduled_for.is_some());
            if status == PostStatus::Scheduled && !has_date {
                return Err(AppError::validation("scheduled posts need scheduled_for"));
            }
        }
        let mut slug = None;
        if let Some(requested) = &input.slug {
            let candidate = slugify(requested);
            if candidate.is_empty() {
                return Err(AppError::validation("slug must not be empty"));
            }
            if candidate != current.slug {
                slug = Some(candidate);
            }
        }
        // Leaving the trash reclaims the slug, which another post may have
        // taken in the meantime (trashed posts are outside the unique
        // index). The index would refuse it too; checking first names it.
        let leaving_trash = current.status == PostStatus::Trash
            && input.status.is_some_and(|s| s != PostStatus::Trash);
        let wanted_slug = slug.as_deref().unwrap_or(&current.slug);
        if (slug.is_some() || leaving_trash)
            && self
                .posts
                .slug_taken(current.post_type, wanted_slug, Some(id))
                .await?
        {
            return Err(AppError::conflict(if slug.is_some() {
                format!("slug {wanted_slug:?} is already in use")
            } else {
                format!("slug {wanted_slug:?} was taken by another post while this one was trashed")
            }));
        }
        // Field values are written only through `fields`. A `fields` key
        // inside `meta` is ignored (the repository keeps the stored
        // values): editors echo the whole meta on every save, and an echo
        // must neither fail on values gone stale nor overwrite newer ones.
        // Required is checked on the way into published or scheduled, and
        // whenever the values of an entry that is (or stays) published or
        // scheduled are written.
        let stored_fields = current.meta.get("fields");
        // `fields` replaces the values of the defined fields; what a
        // deleted field left behind stays until its clean-up.
        let new_fields = match &input.fields {
            Some(raw) => {
                let checked = self
                    .fields
                    .validate_values(current.post_type, raw, stored_fields)
                    .await?;
                Some(
                    self.fields
                        .keep_orphans(current.post_type, checked, stored_fields)
                        .await?,
                )
            }
            None => None,
        };
        let resulting = input.status.unwrap_or(current.status);
        let entering = input
            .status
            .is_some_and(|s| s != current.status && publishes(s));
        if publishes(resulting) && (entering || new_fields.is_some()) {
            let effective = match &new_fields {
                Some(values) => values.clone(),
                None => stored_fields
                    .and_then(serde_json::Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
            };
            self.fields
                .check_required(current.post_type, &effective)
                .await?;
        }
        let fields_changed = new_fields.as_ref().is_some_and(|values| {
            stored_fields.and_then(serde_json::Value::as_object) != Some(values)
                && !(values.is_empty() && stored_fields.is_none())
        });

        // updated_at only when content, title, excerpt, slug, layout or
        // field values changed.
        let content_changed = input.content.as_ref().is_some_and(|c| {
            serde_json::to_string(&c.to_json()).map_or(true, |s| {
                s != serde_json::to_string(&current.content).unwrap_or_default()
            })
        });
        let touched = content_changed
            || fields_changed
            || input.title.as_ref().is_some_and(|t| *t != current.title)
            || input.excerpt.is_some()
            || slug.is_some()
            || input
                .layout
                .as_ref()
                .is_some_and(|l| current.layout.as_ref() != Some(l));

        // Everything that can be refused is checked before the write.
        let password_hash = match input.password {
            None => None,
            Some(pwd) if pwd.is_empty() => Some(None), // clear
            Some(pwd) if pwd.len() < 8 => {
                return Err(AppError::validation(
                    "password must be at least 8 characters",
                ));
            }
            Some(pwd) => Some(Some(crate::user::password::hash_password(&pwd)?)),
        };
        let term_ids = match input.term_ids {
            Some(ids) => Some(self.checked_term_ids(ids).await?),
            None => None,
        };

        // One transaction for the fields, password and terms; written only
        // while the status is still the one the transition was checked
        // against (the publisher may have moved it since).
        let updated = self
            .posts
            .update(&PostUpdate {
                id,
                status: input.status,
                slug,
                title: input.title,
                content: input.content.map(|c| c.to_json()),
                excerpt: input.excerpt,
                parent_id: None,
                meta: input.meta,
                fields: new_fields.map(serde_json::Value::Object),
                scheduled_for: input.scheduled_for,
                clear_schedule: input.clear_schedule,
                password_hash,
                term_ids,
                expect_status: Some(current.status),
                touch_updated_at: touched,
                layout: input.layout,
            })
            .await?;

        // Exactly one event per save.
        match input.status {
            Some(PostStatus::Published) if current.status != PostStatus::Published => {
                events::emit_published(updated.id, updated.author_id);
            }
            Some(PostStatus::Trash) if current.status != PostStatus::Trash => {
                events::emit_trashed(updated.id);
            }
            Some(_) if leaving_trash => events::emit_restored(updated.id),
            _ => events::emit_updated(updated.id),
        }

        Ok(updated)
    }

    /// Moves a post to trash (slug released for reuse).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing; [`AppError::Db`]
    /// otherwise.
    pub async fn trash(&self, id: i64) -> Result<PostRow, AppError> {
        let row = self
            .update(
                id,
                UpdatePost {
                    meta: None,
                    fields: None,
                    status: Some(PostStatus::Trash),
                    title: None,
                    slug: None,
                    content: None,
                    excerpt: None,
                    scheduled_for: None,
                    clear_schedule: false,
                    password: None,
                    term_ids: None,
                    layout: None,
                },
            )
            .await?;
        Ok(row)
    }

    /// Restores a trashed post to draft; conflicts when another post has
    /// claimed the slug in the meantime.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when not trashed,
    /// [`AppError::Conflict`] when the slug was taken,
    /// [`AppError::NotFound`] when missing.
    pub async fn restore(&self, id: i64) -> Result<PostRow, AppError> {
        let current = self.posts.get(id).await?;
        if current.status != PostStatus::Trash {
            return Err(AppError::validation(format!(
                "only trashed posts can be restored (status is {})",
                current.status.as_str()
            )));
        }
        // update() re-checks the slug on the way out of the trash and
        // emits the Restored event.
        let row = self
            .update(
                id,
                UpdatePost {
                    meta: None,
                    fields: None,
                    status: Some(PostStatus::Draft),
                    title: None,
                    slug: None,
                    content: None,
                    excerpt: None,
                    scheduled_for: None,
                    clear_schedule: false,
                    password: None,
                    term_ids: None,
                    layout: None,
                },
            )
            .await?;
        Ok(row)
    }

    /// Permanently deletes a post.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        // Touch the row first so missing ids 404 instead of no-op.
        self.posts.get(id).await?;
        self.posts.hard_delete(id).await?;
        events::emit_deleted(id);
        Ok(())
    }

    /// Verifies a password for a private post.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when the post has no password,
    /// [`AppError::Auth`] when the password is wrong,
    /// [`AppError::NotFound`] when missing.
    pub async fn verify_password(&self, post_id: i64, password: &str) -> Result<(), AppError> {
        let post = self.posts.get(post_id).await?;
        let hash = post
            .password_hash
            .ok_or_else(|| AppError::validation("post is not password protected"))?;
        crate::user::password::verify_password(password, &hash)?;
        Ok(())
    }

    /// Generates a signed read token for a private post after verifying
    /// the password.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Auth`] on wrong password, [`AppError::NotFound`]
    /// when missing.
    pub async fn verify_and_generate_token(
        &self,
        post_id: i64,
        password: &str,
        secret: &[u8],
    ) -> Result<String, AppError> {
        self.verify_password(post_id, password).await?;
        token::generate_token(post_id, secret, token::DEFAULT_TTL_SECS)
    }

    /// Verifies a private-post read token.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Auth`] when invalid.
    pub fn verify_token(&self, post_id: i64, token: &str, secret: &[u8]) -> Result<(), AppError> {
        token::verify_token(token, secret, post_id)
    }

    /// Publishes due scheduled posts (system transition). Called by the
    /// publisher worker and on-demand after scheduling.
    ///
    /// Returns the number of posts published.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn publish_due(&self) -> Result<usize, AppError> {
        // Claim-and-flip in one statement: concurrent publishers split
        // the due rows between them, and a post moved out of `scheduled`
        // after it came due is not published over the author's change.
        let published = self
            .posts
            .publish_scheduled_due(chrono::Utc::now(), 100)
            .await?;
        for post in &published {
            events::emit_published(post.id, post.author_id);
        }
        Ok(published.len())
    }
}

/// Whether an entry in `status` is published or on its way: what
/// required fields are enforced for.
const fn publishes(status: PostStatus) -> bool {
    matches!(status, PostStatus::Published | PostStatus::Scheduled)
}

/// How many times a create re-probes for a free slug after a writer
/// outside the per-base lock took the one it chose.
const SLUG_ATTEMPTS: u32 = 5;

#[cfg(test)]
mod tests {
    use super::{ensure_transition, transition_allowed, PostStatus};

    #[test]
    fn legal_transitions() {
        assert!(transition_allowed(PostStatus::Draft, PostStatus::Published));
        assert!(transition_allowed(PostStatus::Draft, PostStatus::Scheduled));
        assert!(transition_allowed(PostStatus::Published, PostStatus::Draft));
        assert!(transition_allowed(PostStatus::Trash, PostStatus::Draft));
        assert!(transition_allowed(PostStatus::Draft, PostStatus::Draft));
        // Scheduled -> Published is system-only
        assert!(!transition_allowed(
            PostStatus::Scheduled,
            PostStatus::Published
        ));
    }

    #[test]
    fn illegal_transitions() {
        // Trash must go through draft (restore) before publishing again.
        assert!(!transition_allowed(
            PostStatus::Trash,
            PostStatus::Published
        ));
        assert!(!transition_allowed(
            PostStatus::Trash,
            PostStatus::Scheduled
        ));
        let err = ensure_transition(PostStatus::Trash, PostStatus::Published)
            .err()
            .map_or(String::new(), |e| e.to_string());
        assert!(err.contains("trash -> published"), "err: {err}");
    }
}
