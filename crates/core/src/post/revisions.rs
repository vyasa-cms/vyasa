//! Revision snapshots, autosave, pruning, and preview tokens.

use chrono::{DateTime, Duration, Utc};
use vyasa_common::AppError;
use vyasa_db::content_models::PostRevisionRow;
use vyasa_db::repo::{PostUpdate, RevisionsRepo};

use crate::post::token;

/// The parts of an entry a revision captures, and a revert restores.
///
/// Grouped rather than passed as loose arguments because they have to stay
/// in step: a page whose sections changed but whose words did not is still
/// a change, and comparing only the words would drop it on the floor.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot<'a> {
    /// Entry title.
    pub title: &'a str,
    /// Block document JSON.
    pub content: &'a serde_json::Value,
    /// Section tree, for an entry that composes itself.
    pub layout: Option<&'a serde_json::Value>,
    /// Custom field values (`meta.fields`); `None` when there are none.
    pub fields: Option<&'a serde_json::Value>,
}

impl<'a> Snapshot<'a> {
    /// Borrows the current state of a stored row.
    #[must_use]
    pub fn of(post: &'a vyasa_db::content_models::PostRow) -> Self {
        Self {
            title: &post.title,
            content: &post.content,
            layout: post.layout.as_ref(),
            fields: post.meta.get("fields"),
        }
    }

    /// Whether `row` already records exactly this state.
    #[must_use]
    fn matches(&self, row: &PostRevisionRow) -> bool {
        row.title == self.title
            && row.content == *self.content
            && row.layout.as_ref() == self.layout
            && no_values_as_none(row.fields.as_ref()) == no_values_as_none(self.fields)
    }

    fn new_revision(
        &self,
        post_id: i64,
        author_id: i64,
        is_autosave: bool,
    ) -> vyasa_db::repo::revisions::NewRevision<'a> {
        vyasa_db::repo::revisions::NewRevision {
            id: vyasa_common::next_id_i64(),
            post_id,
            title: self.title,
            content: self.content.clone(),
            author_id,
            is_autosave,
            layout: self.layout.cloned(),
            // Always recorded, `{}` when there are none: NULL is kept for
            // revisions from before fields, which restore differently.
            fields: Some(
                self.fields
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
            ),
        }
    }
}

/// `None` for no values at all, whichever way that is written.
fn no_values_as_none(fields: Option<&serde_json::Value>) -> Option<&serde_json::Value> {
    fields.filter(|v| v.as_object().is_none_or(|m| !m.is_empty()))
}

/// What a revision restore did.
#[derive(Clone, Debug)]
pub struct Restored {
    /// The entry as restored.
    pub post: vyasa_db::content_models::PostRow,
    /// Keys of the revision's field values that were not restored: their
    /// field is gone, no longer takes the value, or the value names a
    /// media item or entry it may not.
    pub dropped_fields: Vec<String>,
}

/// Revision lifecycle operations.
#[derive(Clone, Debug)]
pub struct RevisionService {
    revisions: RevisionsRepo,
    posts: vyasa_db::repo::PostsRepo,
    fields: crate::content::ContentFieldsService,
}

impl RevisionService {
    /// Creates a service over the given repositories.
    #[must_use]
    pub fn new(revisions: RevisionsRepo, posts: vyasa_db::repo::PostsRepo) -> Self {
        let fields = crate::content::ContentFieldsService::new(posts.pool().clone());
        Self {
            revisions,
            posts,
            fields,
        }
    }

    /// Creates a revision snapshot if the content hash differs from the
    /// latest revision. Returns the new revision if created.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn snapshot_if_changed(
        &self,
        post_id: i64,
        author_id: i64,
        snap: &Snapshot<'_>,
    ) -> Result<Option<PostRevisionRow>, AppError> {
        // Against the newest *manual* revision, as `save_working_copy`
        // already does. Compared against the newest of any kind, an
        // autosave of the same text swallowed the revision an explicit
        // save should have made: the author pressed Save and the history
        // recorded nothing.
        let manual = self.revisions.list_for_post(post_id, false).await?;
        if let Some(latest) = manual.first() {
            if snap.matches(latest) {
                return Ok(None);
            }
        }
        let rev = self
            .revisions
            .insert(&snap.new_revision(post_id, author_id, false))
            .await?;
        Ok(Some(rev))
    }

    /// Records `title`/`content` as a manual revision without touching the
    /// post — a working copy. Compared against the newest *manual* revision
    /// only: an autosave of the same text must not stand in for an explicit
    /// save, because the editor treats the two differently on the next
    /// visit (a saved working copy is loaded; an autosave is only offered).
    ///
    /// # Errors
    /// Database errors.
    pub async fn save_working_copy(
        &self,
        post_id: i64,
        author_id: i64,
        snap: &Snapshot<'_>,
    ) -> Result<PostRevisionRow, AppError> {
        let manual = self.revisions.list_for_post(post_id, false).await?;
        if let Some(latest) = manual.first() {
            if snap.matches(latest) {
                return Ok(latest.clone());
            }
        }
        self.revisions
            .insert(&snap.new_revision(post_id, author_id, false))
            .await
    }

    /// Lists revisions for a post, newest first (excludes autosaves by default).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(
        &self,
        post_id: i64,
        include_autosave: bool,
    ) -> Result<Vec<PostRevisionRow>, AppError> {
        self.revisions
            .list_for_post(post_id, include_autosave)
            .await
    }

    /// Fetches a single revision.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, revision_id: i64) -> Result<PostRevisionRow, AppError> {
        self.revisions.get(revision_id).await
    }

    /// Restores a revision's content (title, blocks, sections and field
    /// values) to `post_id`. Creates a new revision
    /// for the current state before restoring, then updates the post and
    /// announces the change (search index, caches, webhooks).
    ///
    /// The revision must belong to `post_id`: the caller authorised the
    /// request against that post, so a revision of any other post is
    /// reported missing rather than written somewhere the caller was
    /// never checked for.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when the post or revision is missing
    /// or the revision belongs to another post, [`AppError::Db`] otherwise.
    pub async fn restore(
        &self,
        post_id: i64,
        revision_id: i64,
        requester_id: i64,
    ) -> Result<vyasa_db::content_models::PostRow, AppError> {
        Ok(self
            .restore_reporting(post_id, revision_id, requester_id)
            .await?
            .post)
    }

    /// [`RevisionService::restore`], also saying which field values of
    /// the revision were not restored because they no longer fit (see
    /// [`crate::content::ContentFieldsService::restorable_values`]).
    ///
    /// # Errors
    ///
    /// As [`RevisionService::restore`].
    pub async fn restore_reporting(
        &self,
        post_id: i64,
        revision_id: i64,
        requester_id: i64,
    ) -> Result<Restored, AppError> {
        let rev = self.revisions.get(revision_id).await?;
        if rev.post_id != post_id {
            return Err(AppError::not_found("revision", revision_id));
        }
        let post = self.posts.get(post_id).await?;
        // The revision's field values, as far as they still fit the type's
        // fields; a revision from before fields existed (NULL) keeps the
        // entry's current values.
        let (fields, dropped_fields) = match &rev.fields {
            Some(values) => {
                let (kept, dropped) = self
                    .fields
                    .restorable_values(post.post_type, values, post.meta.get("fields"))
                    .await?;
                (Some(serde_json::Value::Object(kept)), dropped)
            }
            None => (None, Vec::new()),
        };
        // Snapshot current state before overwriting.
        let _ = self
            .snapshot_if_changed(post.id, requester_id, &Snapshot::of(&post))
            .await?;
        let updated = self
            .posts
            .update(&PostUpdate {
                id: post_id,
                status: None,
                slug: None,
                title: Some(rev.title.clone()),
                content: Some(rev.content.clone()),
                excerpt: None,
                parent_id: None,
                meta: None,
                fields,
                scheduled_for: None,
                clear_schedule: false,
                password_hash: None,
                term_ids: None,
                expect_status: None,
                touch_updated_at: true,
                // A revision predating page composition has no layout of
                // its own; restoring it must not silently keep the
                // arrangement the author is reverting away from.
                layout: Some(rev.layout.clone().unwrap_or_else(|| serde_json::json!([]))),
            })
            .await?;
        crate::events::emit_updated(updated.id);
        Ok(Restored {
            post: updated,
            dropped_fields,
        })
    }

    /// Autosaves: one per (post, user), throttled (caller should rate-limit;
    /// this method just upserts). Returns the new autosave revision.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn autosave(
        &self,
        post_id: i64,
        author_id: i64,
        snap: &Snapshot<'_>,
    ) -> Result<PostRevisionRow, AppError> {
        // Delete existing autosave for this post+author.
        let _ = self.revisions.delete_autosave(post_id, author_id).await?;
        self.revisions
            .insert(&snap.new_revision(post_id, author_id, true))
            .await
    }

    /// Prunes revisions for a post according to the adaptive policy.
    /// Returns the number of deleted revisions.
    ///
    /// Policy (relative to `now`):
    /// - 0-24h: keep all
    /// - 24h-7d: keep hourly (most recent per hour bucket)
    /// - 7d-30d: keep daily (most recent per day bucket)
    /// - 30d+: delete
    ///
    /// Autosaves are excluded (kept as-is).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn prune(&self, post_id: i64, now: DateTime<Utc>) -> Result<usize, AppError> {
        let revisions = self.revisions.list_for_post(post_id, false).await?;
        let keep_ids = select_keeps(&revisions, now);
        let delete_ids: Vec<i64> = revisions
            .iter()
            .filter(|r| !keep_ids.contains(&r.id))
            .map(|r| r.id)
            .collect();
        let n = usize::try_from(self.revisions.delete_ids(&delete_ids).await?).unwrap_or(0);
        Ok(n)
    }

    /// Generates a preview token for a post.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Internal`] on HMAC failure.
    pub fn generate_preview_token(&self, post_id: i64, secret: &[u8]) -> Result<String, AppError> {
        token::generate_preview_token(post_id, secret)
    }

    /// Verifies a preview token.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Auth`] when invalid.
    pub fn verify_preview_token(
        &self,
        post_id: i64,
        token: &str,
        secret: &[u8],
    ) -> Result<(), AppError> {
        token::verify_preview_token(token, secret, post_id)
    }
}

/// Pure function: which revision ids to keep, given `now`.
///
/// `revisions` must be sorted newest-first (as returned by the repo).
#[must_use]
pub fn select_keeps(revisions: &[PostRevisionRow], now: DateTime<Utc>) -> Vec<i64> {
    let mut keep = Vec::new();
    let mut seen_hour: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_day: std::collections::HashSet<String> = std::collections::HashSet::new();

    for rev in revisions {
        let age = now.signed_duration_since(rev.created_at);
        if age < Duration::hours(24) {
            keep.push(rev.id);
        } else if age < Duration::days(7) {
            // Hourly bucket: YYYY-MM-DD-HH
            let key = rev.created_at.format("%Y-%m-%d-%H").to_string();
            if seen_hour.insert(key) {
                keep.push(rev.id);
            }
        } else if age < Duration::days(30) {
            // Daily bucket: YYYY-MM-DD
            let key = rev.created_at.format("%Y-%m-%d").to_string();
            if seen_day.insert(key) {
                keep.push(rev.id);
            }
        } else {
            // Older than 30 days: prune (do not keep)
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::select_keeps;
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::json;
    use vyasa_db::content_models::PostRevisionRow;

    fn rev(id: i64, age: Duration) -> PostRevisionRow {
        PostRevisionRow {
            id,
            post_id: 1,
            title: format!("rev {id}"),
            content: json!({}),
            author_id: 1,
            is_autosave: false,
            layout: None,
            fields: None,
            created_at: Utc::now() - age,
        }
    }

    #[test]
    fn prune_keeps_all_within_24h() {
        let now = Utc::now();
        let revs = vec![
            rev(1, Duration::hours(1)),
            rev(2, Duration::hours(2)),
            rev(3, Duration::hours(23)),
        ];
        let keep = select_keeps(&revs, now);
        assert_eq!(keep.len(), 3);
    }

    #[test]
    fn prune_hourly_for_week() {
        let now = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        // 3 revisions within same hour bucket (25h ago, same hour 11:00)
        let base = now - Duration::hours(25);
        let revs = vec![
            PostRevisionRow {
                id: 1,
                post_id: 1,
                title: "a".into(),
                content: json!({}),
                author_id: 1,
                is_autosave: false,
                layout: None,
                fields: None,
                created_at: base,
            },
            PostRevisionRow {
                id: 2,
                post_id: 1,
                title: "b".into(),
                content: json!({}),
                author_id: 1,
                is_autosave: false,
                layout: None,
                fields: None,
                created_at: base + Duration::minutes(10),
            },
            PostRevisionRow {
                id: 3,
                post_id: 1,
                title: "c".into(),
                content: json!({}),
                author_id: 1,
                is_autosave: false,
                layout: None,
                fields: None,
                created_at: base + Duration::minutes(20),
            },
        ];
        let keep = select_keeps(&revs, now);
        // Only most recent per hour bucket kept
        assert_eq!(keep.len(), 1);
    }

    #[test]
    fn prune_daily_for_month() {
        let now = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let base = now - Duration::days(10);
        let revs = vec![
            PostRevisionRow {
                id: 1,
                post_id: 1,
                title: "a".into(),
                content: json!({}),
                author_id: 1,
                is_autosave: false,
                layout: None,
                fields: None,
                created_at: base,
            },
            PostRevisionRow {
                id: 2,
                post_id: 1,
                title: "b".into(),
                content: json!({}),
                author_id: 1,
                is_autosave: false,
                layout: None,
                fields: None,
                created_at: base + Duration::hours(1),
            },
        ];
        let keep = select_keeps(&revs, now);
        assert_eq!(keep.len(), 1);
    }

    #[test]
    fn prune_deletes_old() {
        let now = Utc::now();
        let revs = vec![rev(1, Duration::days(40))];
        let keep = select_keeps(&revs, now);
        assert!(keep.is_empty());
    }

    #[test]
    fn prune_synthetic_500() {
        let now = Utc::now();
        let mut revs = Vec::new();
        for i in 0..500_i64 {
            revs.push(rev(i, Duration::hours(i)));
        }
        // Sorted newest-first already (i=0 is newest? Actually i=0 age 0, i=1 age 1h, so newest is i=0, second i=1, etc. That's newest-first.
        let keep = select_keeps(&revs, now);
        // Policy should reduce significantly
        assert!(keep.len() < 500);
        assert!(keep.len() > 10);
        // All within 24h kept: first 24
        // 24h-7d (6 days * 24h = 144) -> hourly: 144, but dedup per hour -> 144? Actually 6 days hourly = 144, but we have 500 hours = 20 days, so 24 + 144 + 14 daily = ~182
        // So keep should be around 24 + 144 + 23 = 191
        assert!(keep.len() < 250);
    }
}
