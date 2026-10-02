#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! Media GraphQL type.

use async_graphql::Object;

/// GraphQL wrapper for a media row.
#[derive(Clone, Debug)]
pub struct GqlMedia(pub vyasa_db::content_models::MediaRow);

#[Object]
impl GqlMedia {
    async fn id(&self) -> i64 {
        self.0.id
    }
    async fn owner_id(&self) -> i64 {
        self.0.owner_id
    }
    async fn file_name(&self) -> &str {
        &self.0.file_name
    }
    async fn mime(&self) -> &str {
        &self.0.mime
    }
    async fn byte_size(&self) -> i64 {
        self.0.byte_size
    }
    async fn storage(&self) -> &str {
        self.0.storage.as_str()
    }
    async fn path(&self) -> &str {
        &self.0.path
    }
    async fn width(&self) -> Option<i32> {
        self.0.width
    }
    async fn height(&self) -> Option<i32> {
        self.0.height
    }
    async fn blurhash(&self) -> Option<&str> {
        self.0.blurhash.as_deref()
    }
    async fn alt(&self) -> Option<&str> {
        self.0.alt.as_deref()
    }
    async fn caption(&self) -> Option<&str> {
        self.0.caption.as_deref()
    }
    async fn derivatives(&self) -> async_graphql::Json<serde_json::Value> {
        async_graphql::Json(self.0.derivatives.clone())
    }
    async fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.created_at
    }
}
