//! Comment service with moderation heuristics.

use vyasa_common::AppError;
use vyasa_db::content_models::{CommentRow, CommentStatus};
use vyasa_db::repo::{CommentsRepo, OptionsRepo, PostsRepo};

use crate::events;

/// Moderation mode for comments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModerationMode {
    /// Auto-approve all comments.
    AutoApprove,
    /// Require approval for first comment from an email, then auto-approve.
    RequireFirstApproval,
    /// Require approval for all comments.
    RequireApprovalAll,
}

impl ModerationMode {
    /// Parses from string.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "auto_approve" => ModerationMode::AutoApprove,
            "require_all" => ModerationMode::RequireApprovalAll,
            _ => ModerationMode::RequireFirstApproval,
        }
    }

    /// Returns the string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ModerationMode::AutoApprove => "auto_approve",
            ModerationMode::RequireFirstApproval => "require_first",
            ModerationMode::RequireApprovalAll => "require_all",
        }
    }
}

/// Input for submitting a comment.
pub struct SubmitComment {
    /// Post id.
    pub post_id: i64,
    /// Author user id, if logged in.
    pub author_user_id: Option<i64>,
    /// Display name.
    pub author_name: String,
    /// Email.
    pub author_email: String,
    /// Body.
    pub content: String,
    /// Parent comment id, if reply.
    pub parent_id: Option<i64>,
}

/// Comment moderation operations.
#[derive(Clone, Debug)]
pub struct CommentService {
    comments: CommentsRepo,
    options: OptionsRepo,
    posts: PostsRepo,
}

impl CommentService {
    /// Creates a service over the given repos.
    #[must_use]
    pub fn new(comments: CommentsRepo, options: OptionsRepo, posts: PostsRepo) -> Self {
        Self {
            comments,
            options,
            posts,
        }
    }

    /// Returns the current moderation mode from options, defaulting to `RequireFirstApproval`.
    async fn moderation_mode(&self) -> ModerationMode {
        let default = serde_json::json!("require_first");
        let value = self
            .options
            .get_or_default("comment_moderation", default)
            .await
            .unwrap_or_else(|_| serde_json::json!("require_first"));
        value
            .as_str()
            .map_or(ModerationMode::RequireFirstApproval, ModerationMode::parse)
    }

    /// The most a comment may hold, in characters.
    ///
    /// There was no cap: a single comment could carry megabytes of text
    /// through the public form, stored whole and rendered whole on the
    /// post for every visitor. Ten thousand characters is a long essay.
    pub const MAX_COMMENT_CHARS: usize = 10_000;

    /// Submits a comment, applying moderation heuristics.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for empty fields,
    /// [`AppError::NotFound`] when post missing, [`AppError::Db`] otherwise.
    pub async fn submit(&self, input: SubmitComment) -> Result<CommentRow, AppError> {
        // The post first: a missing post answers the same whatever the
        // comment holds, so validation cannot tell ids apart. A draft, a
        // scheduled post or one in the bin is not on the site, and a
        // comment on it is either spam probing ids or a leak of the fact
        // that the post exists.
        let post = self.posts.get(input.post_id).await?;
        if !matches!(post.status, vyasa_db::content_models::PostStatus::Published) {
            return Err(AppError::validation("Comments are closed on this post."));
        }
        let author_name = input.author_name.trim().to_string();
        let author_email = input.author_email.trim().to_string();
        let content = input.content.trim().to_string();
        if author_name.is_empty() {
            return Err(AppError::validation("author name must not be empty"));
        }
        if author_email.is_empty() || !author_email.contains('@') {
            return Err(AppError::validation("author email must be valid"));
        }
        if content.is_empty() {
            return Err(AppError::validation("comment content must not be empty"));
        }
        if content.chars().count() > Self::MAX_COMMENT_CHARS {
            return Err(AppError::validation(format!(
                "Comments are limited to {} characters.",
                Self::MAX_COMMENT_CHARS
            )));
        }

        // Validate parent and depth cap.
        let parent_id = if let Some(pid) = input.parent_id {
            let parent = self.comments.get(pid).await?;
            if parent.post_id != input.post_id {
                return Err(AppError::validation(
                    "parent comment is for a different post",
                ));
            }
            self.comments.resolve_parent(Some(pid)).await?
        } else {
            None
        };

        // Determine initial status via moderation rules.
        let mode = self.moderation_mode().await;
        let mut status = match mode {
            ModerationMode::AutoApprove => CommentStatus::Approved,
            ModerationMode::RequireApprovalAll => CommentStatus::Pending,
            ModerationMode::RequireFirstApproval => {
                let has_approved = self.comments.has_approved_for_email(&author_email).await?;
                if has_approved {
                    CommentStatus::Approved
                } else {
                    CommentStatus::Pending
                }
            }
        };

        // Link-count heuristic: >2 links forces pending (or spam if >5? For now pending).
        let link_count = CommentsRepo::count_links(&content);
        if link_count > 2 && status == CommentStatus::Approved {
            status = CommentStatus::Pending;
        }
        if link_count > 5 {
            status = CommentStatus::Spam;
        }

        let row = self
            .comments
            .insert(&vyasa_db::repo::NewComment {
                id: vyasa_common::next_id_i64(),
                post_id: input.post_id,
                author_user_id: input.author_user_id,
                author_name: &author_name,
                author_email: &author_email,
                content: &content,
                parent_id,
                status,
            })
            .await?;
        events::emit_comment_added(row.id, row.post_id);
        Ok(row)
    }

    /// Moderates a comment to a new status (admin).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn moderate(
        &self,
        comment_id: i64,
        new_status: CommentStatus,
    ) -> Result<CommentRow, AppError> {
        // Validate transition: pending->approved/spam/trash, approved->spam/trash, spam->approved/trash, trash->approved/spam
        // For simplicity, allow any transition except trash->pending? But we allow all for now, just ensure not no-op?
        // We allow any status change.
        self.comments.update_status(comment_id, new_status).await
    }

    /// Lists approved threaded comments for a post.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_approved_threaded(&self, post_id: i64) -> Result<Vec<CommentRow>, AppError> {
        // Ensure post exists for 404.
        self.posts.get(post_id).await?;
        self.comments.list_approved_threaded(post_id).await
    }

    /// Counts comments matching a moderation status.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_by_status(&self, status: CommentStatus) -> Result<i64, AppError> {
        self.comments.count_by_status(status).await
    }

    /// Lists comments by status (admin, paginated).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_by_status(
        &self,
        status: CommentStatus,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CommentRow>, AppError> {
        self.comments.list_by_status(status, limit, offset).await
    }

    /// Fetches a single comment.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<CommentRow, AppError> {
        self.comments.get(id).await
    }
}
