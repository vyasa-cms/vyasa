//! Source connection abstraction over WordPress schemas.
//!
//! [`WpSource`] is intentionally tiny: ordered, plain-data fetches. The
//! Postgres implementation works against a WP schema imported into
//! Postgres (e.g. via pgloader); MySQL support plugs in behind the same
//! trait (deviation noted in PHASE-29.md).

use vyasa_common::AppError;

/// One `wp_users` row we care about.
#[derive(Debug, Clone)]
pub struct WpUser {
    /// Source id.
    pub id: i64,
    /// `user_login`.
    pub login: String,
    /// `user_email`.
    pub email: String,
    /// `display_name`.
    pub display_name: String,
}

/// One `wp_posts` row (post or page, non-revision).
#[derive(Debug, Clone)]
pub struct WpPost {
    /// Source `ID`.
    pub id: i64,
    /// `post_type` (`post`, `page`, …).
    pub post_type: String,
    /// `post_status` (`publish`, `draft`, …).
    pub status: String,
    /// `post_name` (slug).
    pub slug: String,
    /// `post_title`.
    pub title: String,
    /// Raw `post_content` HTML.
    pub content_html: String,
    /// `post_excerpt`.
    pub excerpt: String,
    /// `post_author` source user id.
    pub author_id: i64,
    /// Selected `_wp*` meta values (featured image etc.).
    pub meta: Vec<(String, serde_json::Value)>,
    /// Source term ids attached to this post (populated by sources that
    /// join `wp_term_relationships`; reserved for term-linking).
    #[allow(dead_code)]
    pub term_ids: Vec<i64>,
}

/// One `wp_terms` row joined through `wp_term_taxonomy`.
#[derive(Debug, Clone)]
pub struct WpTerm {
    /// Source term_id.
    pub id: i64,
    /// `taxonomy` (`category`, `post_tag`).
    pub taxonomy: String,
    /// Term name.
    pub name: String,
    /// Term slug.
    pub slug: String,
}

/// One approved-ish `wp_comments` row.
#[derive(Debug, Clone)]
pub struct WpComment {
    /// Source comment_ID.
    pub id: i64,
    /// Parent post source id.
    pub post_id: i64,
    /// Parent comment source id.
    pub parent_id: Option<i64>,
    /// `comment_author`.
    pub author_name: String,
    /// `comment_author_email`.
    pub author_email: String,
    /// Sanitized `comment_content` (WP stores rendered HTML).
    pub content_html: String,
}

/// Read-only view of a WordPress database.
pub trait WpSource {
    /// All users, ordered by id.
    fn users(&self) -> impl std::future::Future<Output = Result<Vec<WpUser>, AppError>> + Send;

    /// All posts + pages (no revisions/attachments), ordered by id.
    fn posts(&self) -> impl std::future::Future<Output = Result<Vec<WpPost>, AppError>> + Send;

    /// Category + tag terms with slugs.
    fn terms(&self) -> impl std::future::Future<Output = Result<Vec<WpTerm>, AppError>> + Send;

    /// Comments (any status; importer keeps only non-spam).
    fn comments(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<WpComment>, AppError>> + Send;

    /// Media attachments: (source id, file name, path relative to uploads
    /// base). Empty by default.
    fn attachments(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<(i64, String, String)>, AppError>> + Send
    {
        std::future::ready(Ok(Vec::new()))
    }
}
