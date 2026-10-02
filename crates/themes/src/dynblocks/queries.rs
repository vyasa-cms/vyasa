//! The data seam between themes and content storage.
//!
//! [`ContentQueries`] is defined here (themes crate) and implemented by the
//! api crate over repositories/services, keeping the dependency direction
//! legal: themes never touch sqlx. Plugin hook filtering (phase 33) will
//! wrap implementations of this trait.

use std::future::Future;
use std::pin::Pin;

/// One post card as dynamic blocks consume it.
#[derive(Debug, Clone, PartialEq)]
pub struct PostCardData {
    /// Post id.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Canonical URL.
    pub url: String,
    /// Plain-text excerpt.
    pub excerpt: String,
    /// Thumbnail URL when a featured image exists.
    pub thumb_url: Option<String>,
    /// Focal point of the thumbnail as `"x% y%"`, for `object-position`.
    pub thumb_focal: Option<String>,
    /// Blurhash of the thumbnail for LQIP placeholders.
    pub blurhash: Option<String>,
    /// Formatted date string.
    pub date: String,
    /// Author display name.
    pub author: String,
}

/// One category/tag with optional usage count.
#[derive(Debug, Clone, PartialEq)]
pub struct TermLinkData {
    /// Term name.
    pub name: String,
    /// Archive URL for the term.
    pub url: String,
    /// Number of published posts (when requested).
    pub count: Option<i64>,
}

/// One month archive entry.
#[derive(Debug, Clone, PartialEq)]
pub struct MonthArchiveData {
    /// Human label ("March 2026").
    pub label: String,
    /// Archive URL.
    pub url: String,
    /// Published-post count in that month.
    pub count: i64,
}

/// One node of the page tree (docs nav).
#[derive(Debug, Clone, PartialEq)]
pub struct PageNodeData {
    /// Page id.
    pub id: i64,
    /// Page title.
    pub title: String,
    /// Public URL.
    pub url: String,
    /// Nesting depth (1 = top level).
    pub depth: u8,
}

/// One entry of a table of contents.
#[derive(Debug, Clone, PartialEq)]
pub struct TocEntryData {
    /// Heading level (2..=6).
    pub level: u8,
    /// Anchor id usable as `#href`.
    pub anchor: String,
    /// Plain-text heading content.
    pub text: String,
}

/// One approved comment ready to render on a single page.
#[derive(Debug, Clone, PartialEq)]
pub struct CommentNodeData {
    /// Comment id.
    pub id: i64,
    /// Author display name.
    pub author: String,
    /// Formatted date.
    pub date: String,
    /// Sanitized HTML body (sanitized upstream at the REST boundary).
    pub html: String,
    /// Nested replies, oldest-first per level.
    pub children: Vec<CommentNodeData>,
}

/// How a bound section orders its entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntrySort {
    /// Most recently created first — what "latest" has always meant here.
    #[default]
    Newest,
    /// Oldest first.
    Oldest,
    /// Alphabetical by title.
    Title,
    /// Most recently updated first.
    Updated,
}

impl EntrySort {
    /// The names a `bind.sort` setting may carry.
    pub const NAMES: [&'static str; 4] = ["newest", "oldest", "title", "updated"];

    /// Parses a setting value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "newest" => Some(Self::Newest),
            "oldest" => Some(Self::Oldest),
            "title" => Some(Self::Title),
            "updated" => Some(Self::Updated),
            _ => None,
        }
    }
}

/// What a resolver may ask the site for.
///
/// All methods return boxed futures so implementors can be async without
/// the themes crate pulling a runtime; return types are owned/clonable so
/// plugin wrappers can post-process freely.
pub trait ContentQueries: Send + Sync {
    /// Published entries of one post type, for bound sections.
    ///
    /// `source` names a post type — built-in or plugin-registered. An
    /// unknown source resolves to an empty list rather than an error: it
    /// is the survival rule a disabled plugin's type already follows, and
    /// a page must not 500 because a binding outlived its plugin. `term`
    /// filters by term slug (`"news"` means a category; `"shelf:fiction"`
    /// names another taxonomy).
    fn entries<'a>(
        &'a self,
        source: &'a str,
        sort: EntrySort,
        limit: u32,
        term: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PostCardData>, String>> + Send + 'a>>;

    /// Published entries for a bound section, honouring its field sort
    /// and `where` conditions. The default ignores those (an
    /// implementation without fields shows the plain listing); the site's
    /// own implementation applies them.
    fn bound_entries<'a>(
        &'a self,
        bind: &'a crate::binding::Binding,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PostCardData>, String>> + Send + 'a>> {
        self.entries(&bind.source, bind.sort, bind.limit, bind.term.as_deref())
    }

    /// Categories with counts (published posts).
    fn categories<'a>(
        &'a self,
        show_counts: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TermLinkData>, String>> + Send + 'a>>;

    /// Most-used tags.
    fn popular_tags<'a>(
        &'a self,
        count: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TermLinkData>, String>> + Send + 'a>>;

    /// Monthly archive links, newest first.
    fn monthly_archives<'a>(
        &'a self,
        months: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MonthArchiveData>, String>> + Send + 'a>>;

    /// Rendered `<ul>` HTML for a named navigation menu; empty when the
    /// menu does not exist.
    fn navigation_menu<'a>(
        &'a self,
        slug: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    /// Published page tree ordered for navigation (parents before children).
    fn page_tree<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<PageNodeData>, String>> + Send + 'a>,
    >;

    /// Published posts nearest in meaning to `post_id`, best first. The
    /// default is empty so a queries implementation without embeddings
    /// simply renders nothing for the block.
    fn related_posts<'a>(
        &'a self,
        post_id: i64,
        count: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PostCardData>, String>> + Send + 'a>> {
        let _ = (post_id, count);
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    /// URL of the post's read-aloud recording, when one exists.
    fn post_audio_url<'a>(
        &'a self,
        post_id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>> {
        let _ = post_id;
        Box::pin(std::future::ready(Ok(None)))
    }

    /// Approved comment tree for a post (oldest-first per level).
    fn approved_comments<'a>(
        &'a self,
        post_id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CommentNodeData>, String>> + Send + 'a>>;
}
