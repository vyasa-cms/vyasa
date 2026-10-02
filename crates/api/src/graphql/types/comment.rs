#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! Comment GraphQL type.

use async_graphql::{Object, SimpleObject};

/// GraphQL wrapper for a comment row.
#[derive(Clone, Debug)]
pub struct GqlComment(pub vyasa_db::content_models::CommentRow);

#[Object]
impl GqlComment {
    async fn id(&self) -> i64 {
        self.0.id
    }
    async fn post_id(&self) -> i64 {
        self.0.post_id
    }
    async fn author_name(&self) -> &str {
        &self.0.author_name
    }
    async fn content(&self) -> &str {
        &self.0.content
    }
    async fn parent_id(&self) -> Option<i64> {
        self.0.parent_id
    }
    async fn status(&self) -> &str {
        self.0.status.as_str()
    }
    async fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.created_at
    }
}

/// Cursor connection for comments.
#[derive(SimpleObject, Clone)]
pub struct CommentEdge {
    /// Cursor.
    pub cursor: String,
    /// Node.
    pub node: GqlComment,
}

#[derive(SimpleObject, Clone)]
pub struct CommentPageInfo {
    /// Whether there is a next page.
    pub has_next_page: bool,
    /// Last cursor.
    pub end_cursor: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct CommentConnection {
    /// Edges.
    pub edges: Vec<CommentEdge>,
    /// Page info.
    pub page_info: CommentPageInfo,
    /// Total count.
    pub total_count: i64,
}
