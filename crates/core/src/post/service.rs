//! Post domain service: lifecycle rules over the block format.

use vyasa_common::{slugify, AppError};
use vyasa_db::content_models::{PostRow, PostStatus, PostType};
use vyasa_db::repo::{NewPost, PostFilter, PostUpdate, PostsRepo};

use crate::block::BlockDocument;
use crate::content::ContentFieldsService;
use crate::events;
use crate::post::hierarchy;
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
    /// Order among siblings of a hierarchical type; `None` appends.
    pub position: Option<i32>,
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
    /// New parent: `None` keeps, `Some(None)` moves to the root.
    pub parent_id: Option<Option<i64>>,
    /// New position among siblings (hierarchical types only).
    pub position: Option<i32>,
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
    #[allow(clippy::too_many_lines)]
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

        let (path_prefix, position) = if input.post_type.is_hierarchical() {
            self.placement(input.post_type, input.parent_id, &base_slug, input.position)
                .await?
        } else {
            (None, 0)
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
                        position,
                        path_prefix: path_prefix.clone(),
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

    /// The live entries of a hierarchical type as the rules' nodes.
    async fn hierarchy_nodes(&self, post_type: PostType) -> Result<Vec<hierarchy::Node>, AppError> {
        Ok(self
            .posts
            .tree_rows(post_type)
            .await?
            .into_iter()
            .map(|r| hierarchy::Node {
                id: r.id,
                parent_id: r.parent_id,
                position: r.position,
                slug: r.slug,
            })
            .collect())
    }

    /// Where a new entry of a hierarchical type goes: the path prefix
    /// (the parent's path and `/`, empty at the root) and the position.
    async fn placement(
        &self,
        post_type: PostType,
        parent_id: Option<i64>,
        slug: &str,
        position: Option<i32>,
    ) -> Result<(Option<String>, i32), AppError> {
        let nodes = self.hierarchy_nodes(post_type).await?;
        let prefix = match parent_id {
            None => {
                if hierarchy::RESERVED_ROOT_SLUGS.contains(&slug) {
                    return Err(AppError::validation(format!(
                        "the slug {slug:?} is reserved at the top level of this type"
                    )));
                }
                String::new()
            }
            Some(pid) => {
                let parent = match self.posts.get(pid).await {
                    Ok(p) => p,
                    Err(AppError::NotFound { .. }) => {
                        return Err(AppError::validation(
                            "parent must be a live entry of the same type",
                        ));
                    }
                    Err(e) => return Err(e),
                };
                if parent.post_type != post_type || parent.status == PostStatus::Trash {
                    return Err(AppError::validation(
                        "parent must be a live entry of the same type",
                    ));
                }
                let depth = hierarchy::depth_of(&nodes, pid).unwrap_or(1);
                if depth + 1 > hierarchy::MAX_DEPTH {
                    return Err(AppError::validation(format!(
                        "an entry would sit {} levels deep; the limit is {}",
                        depth + 1,
                        hierarchy::MAX_DEPTH
                    )));
                }
                format!("{}/", parent.path.as_deref().unwrap_or(&parent.slug))
            }
        };
        let position = position.unwrap_or_else(|| {
            nodes
                .iter()
                .filter(|n| n.parent_id == parent_id)
                .map(|n| n.position)
                .max()
                .map_or(10, |max| max.saturating_add(10))
        });
        Ok((Some(prefix), position))
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

    /// Fetches the live entry of a hierarchical type at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is none (a trashed entry
    /// has no address).
    pub async fn get_by_path(&self, post_type: PostType, path: &str) -> Result<PostRow, AppError> {
        self.posts.get_by_path(post_type, path).await
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
        let hier = current.post_type.is_hierarchical();
        if !hier && input.position.is_some() {
            return Err(AppError::validation("this type is not hierarchical"));
        }
        let wanted_slug = slug.as_deref().unwrap_or(&current.slug);
        if !hier
            && (slug.is_some() || leaving_trash)
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

        // A hierarchical entry that changes slug, parent or position (or
        // comes back from the trash) gets its subtree's paths rewritten.
        let tree_changes = if hier
            && (slug.is_some()
                || leaving_trash
                || input.parent_id.is_some()
                || input.position.is_some())
        {
            Some(
                self.subtree_changes(&current, slug.as_deref(), input.parent_id, input.position)
                    .await?,
            )
        } else {
            None
        };

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
        if let Some(changes) = &tree_changes {
            self.posts.apply_tree(current.post_type, changes).await?;
        }
        let updated = self
            .posts
            .update(&PostUpdate {
                id,
                status: input.status,
                slug,
                title: input.title,
                content: input.content.map(|c| c.to_json()),
                excerpt: input.excerpt,
                parent_id: if hier { None } else { input.parent_id },
                position: None,
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
                    parent_id: None,
                    position: None,
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
                    parent_id: None,
                    position: None,
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

/// What a batch of moves did.
#[derive(Debug, Clone)]
pub struct MoveOutcome {
    /// Entries whose address changed: `(id, old path, new path)`.
    pub changed_paths: Vec<(i64, String, String)>,
    /// Every entry whose parent, position or path changed.
    pub touched: Vec<i64>,
}

impl PostService {
    /// The live entries of a hierarchical type in reading order.
    ///
    /// # Errors
    /// [`AppError::Validation`] for a type that is not hierarchical.
    pub async fn tree(
        &self,
        post_type: PostType,
    ) -> Result<Vec<vyasa_db::repo::posts::TreeRow>, AppError> {
        if !post_type.is_hierarchical() {
            return Err(AppError::validation(format!(
                "{} is not a hierarchical type",
                post_type.as_str()
            )));
        }
        self.posts.tree_rows(post_type).await
    }

    /// Applies `moves` as a whole.
    ///
    /// # Errors
    /// [`AppError::Validation`] naming the rule the final tree breaks;
    /// [`AppError::Conflict`] if a path is taken by an entry outside the tree view.
    pub async fn move_entries(
        &self,
        post_type: PostType,
        moves: &[hierarchy::Move],
    ) -> Result<MoveOutcome, AppError> {
        if !post_type.is_hierarchical() {
            return Err(AppError::validation(format!(
                "{} is not a hierarchical type",
                post_type.as_str()
            )));
        }
        let before = self.hierarchy_nodes(post_type).await?;
        let after = hierarchy::apply_moves(&before, moves).map_err(AppError::validation)?;
        let changed_paths = hierarchy::changed_paths(&before, &after);
        let mut touched: std::collections::BTreeSet<i64> =
            changed_paths.iter().map(|(id, _, _)| *id).collect();
        for node in &after {
            let was = before.iter().find(|b| b.id == node.id);
            if was.is_some_and(|b| b.parent_id != node.parent_id || b.position != node.position) {
                touched.insert(node.id);
            }
        }
        let mut changes = Vec::with_capacity(touched.len());
        for node in after.iter().filter(|n| touched.contains(&n.id)) {
            let path = hierarchy::path_of(&after, node.id)
                .ok_or_else(|| AppError::internal_msg("a validated tree has no path"))?;
            changes.push(vyasa_db::repo::TreeChange {
                id: node.id,
                parent_id: node.parent_id,
                position: node.position,
                path,
            });
        }
        if !changes.is_empty() {
            self.posts.apply_tree(post_type, &changes).await?;
        }
        let touched: Vec<i64> = touched.into_iter().collect();
        for id in &touched {
            events::emit_updated(*id);
        }
        Ok(MoveOutcome {
            changed_paths,
            touched,
        })
    }

    /// Recomputes every path of a hierarchical type from its parent links and
    /// slugs, returning how many entries it wrote. For an import, which
    /// inserts entries under throwaway paths and links the parents afterwards.
    ///
    /// # Errors
    /// [`AppError::Validation`] when the links break a rule (a loop, too deep).
    pub async fn rebuild_paths(&self, post_type: PostType) -> Result<usize, AppError> {
        let nodes = self.hierarchy_nodes(post_type).await?;
        hierarchy::validate_tree(&nodes).map_err(AppError::validation)?;
        let mut changes = Vec::with_capacity(nodes.len());
        for node in &nodes {
            let path = hierarchy::path_of(&nodes, node.id)
                .ok_or_else(|| AppError::internal_msg("a validated tree has no path"))?;
            changes.push(vyasa_db::repo::TreeChange {
                id: node.id,
                parent_id: node.parent_id,
                position: node.position,
                path,
            });
        }
        if !changes.is_empty() {
            self.posts.apply_tree(post_type, &changes).await?;
        }
        Ok(changes.len())
    }

    /// The writes that give `current` a new slug, parent or position (or
    /// bring it back from the trash) and its subtree the paths that follow,
    /// after checking the resulting tree as a whole.
    #[allow(clippy::option_option)]
    async fn subtree_changes(
        &self,
        current: &PostRow,
        new_slug: Option<&str>,
        new_parent: Option<Option<i64>>,
        new_position: Option<i32>,
    ) -> Result<Vec<vyasa_db::repo::TreeChange>, AppError> {
        let mut nodes = self.hierarchy_nodes(current.post_type).await?;
        if !nodes.iter().any(|n| n.id == current.id) {
            nodes.push(hierarchy::Node {
                id: current.id,
                parent_id: current.parent_id,
                position: current.position,
                slug: current.slug.clone(),
            });
        }
        for node in &mut nodes {
            if node.id == current.id {
                if let Some(slug) = new_slug {
                    slug.clone_into(&mut node.slug);
                }
                if let Some(parent) = new_parent {
                    node.parent_id = parent;
                }
                if let Some(position) = new_position {
                    node.position = position;
                }
            }
        }
        hierarchy::validate_tree(&nodes).map_err(AppError::validation)?;
        let mut changes = Vec::new();
        for id in hierarchy::subtree_ids(&nodes, current.id) {
            let Some(node) = nodes.iter().find(|n| n.id == id) else {
                continue;
            };
            let path = hierarchy::path_of(&nodes, id)
                .ok_or_else(|| AppError::internal_msg("a validated tree has no path"))?;
            changes.push(vyasa_db::repo::TreeChange {
                id,
                parent_id: node.parent_id,
                position: node.position,
                path,
            });
        }
        Ok(changes)
    }
}

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
