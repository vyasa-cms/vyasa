#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! Term GraphQL type.

use async_graphql::{Object, SimpleObject};

/// GraphQL wrapper for a term row.
#[derive(Clone, Debug)]
pub struct GqlTerm(pub vyasa_db::content_models::TermRow);

#[Object]
impl GqlTerm {
    async fn id(&self) -> i64 {
        self.0.id
    }
    async fn taxonomy(&self) -> &str {
        self.0.taxonomy.as_str()
    }
    async fn name(&self) -> &str {
        &self.0.name
    }
    async fn slug(&self) -> &str {
        &self.0.slug
    }
    async fn parent_id(&self) -> Option<i64> {
        self.0.parent_id
    }
    async fn meta(&self) -> async_graphql::Json<serde_json::Value> {
        async_graphql::Json(self.0.meta.clone())
    }
    async fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.created_at
    }
}

/// Term with post count (for `terms` query with counts).
#[derive(SimpleObject, Clone)]
pub struct GqlTermWithCount {
    /// Term.
    pub term: GqlTermWrapper,
    /// Number of posts assigned.
    pub post_count: i64,
}

/// Simple wrapper used inside GqlTermWithCount to avoid ComplexObject nesting issues.
#[derive(Clone, Debug)]
pub struct GqlTermWrapper(pub vyasa_db::content_models::TermRow);

#[Object]
impl GqlTermWrapper {
    async fn id(&self) -> i64 {
        self.0.id
    }
    async fn taxonomy(&self) -> &str {
        self.0.taxonomy.as_str()
    }
    async fn name(&self) -> &str {
        &self.0.name
    }
    async fn slug(&self) -> &str {
        &self.0.slug
    }
    async fn parent_id(&self) -> Option<i64> {
        self.0.parent_id
    }
    async fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.created_at
    }
}
