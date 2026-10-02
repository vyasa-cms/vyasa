#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! Post GraphQL type.

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Object, SimpleObject};

use crate::graphql::loaders::{AuthorLoader, MediaLoader, TermsForPostLoader};

use super::media::GqlMedia;
use super::term::GqlTerm;
use super::user::GqlUser;

/// A type's field definitions, read once per request (the request's
/// shared cache) however many entries the query returns.
async fn definitions(
    ctx: &Context<'_>,
    gql: &crate::graphql::context::GqlContext,
    post_type: vyasa_db::content_models::PostType,
) -> std::sync::Arc<Vec<vyasa_core::content::FieldDef>> {
    match ctx.data_opt::<std::sync::Arc<crate::entry_fields::SharedDefinitions>>() {
        Some(shared) => shared.of(&gql.state.pool, post_type).await,
        None => {
            crate::entry_fields::SharedDefinitions::default()
                .of(&gql.state.pool, post_type)
                .await
        }
    }
}

/// GraphQL wrapper for a post row.
#[derive(Clone, Debug)]
pub struct GqlPost(pub vyasa_db::content_models::PostRow);

/// One search hit: the post, plus why it matched.
#[derive(SimpleObject, Clone)]
pub struct GqlSearchHit {
    /// Highlighted body fragment with `<mark>` around the matched terms.
    ///
    /// The index escapes the source text before inserting its own markup,
    /// so this is safe to render — but it is HTML, not plain text.
    pub snippet: String,
    /// The matching post.
    pub post: GqlPost,
}

/// Pagination edge.
#[derive(SimpleObject, Clone)]
pub struct PostEdge {
    /// Opaque cursor.
    pub cursor: String,
    /// Post node.
    pub node: GqlPost,
}

/// Page info.
#[derive(SimpleObject, Clone)]
pub struct PageInfo {
    /// Whether there is a next page.
    pub has_next_page: bool,
    /// Cursor of last item.
    pub end_cursor: Option<String>,
    /// Whether there is a previous page (always false for forward paging).
    pub has_previous_page: bool,
}

/// Paginated post connection (cursor pagination).
#[derive(SimpleObject, Clone)]
pub struct PostConnection {
    /// Edges.
    pub edges: Vec<PostEdge>,
    /// Page info.
    pub page_info: PageInfo,
    /// Total matching (visible) count.
    pub total_count: i64,
}

#[Object]
impl GqlPost {
    async fn id(&self) -> i64 {
        self.0.id
    }
    async fn url(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        let gql = ctx.data::<crate::graphql::context::GqlContext>()?;
        Ok(crate::permalinks::path(&gql.state, &self.0).await)
    }
    async fn slug(&self) -> &str {
        &self.0.slug
    }
    async fn title(&self) -> &str {
        &self.0.title
    }
    async fn status(&self) -> &str {
        self.0.status.as_str()
    }
    #[graphql(name = "type")]
    async fn post_type(&self) -> &str {
        self.0.post_type.as_str()
    }
    async fn excerpt(&self) -> Option<&str> {
        self.0.excerpt.as_deref()
    }
    async fn content(&self) -> async_graphql::Json<serde_json::Value> {
        async_graphql::Json(self.0.content.clone())
    }
    /// Extension metadata. Field values are not in here: they are `fields`.
    async fn meta(&self) -> async_graphql::Json<serde_json::Value> {
        async_graphql::Json(crate::entry_fields::meta_without_fields(
            self.0.meta.clone(),
        ))
    }
    /// The entry's field values keyed by field key — only fields its type
    /// still defines. Media and entry references are ids as strings; one
    /// whose target is gone is still served (see `fieldsMissing`).
    async fn fields(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<async_graphql::Json<serde_json::Value>> {
        let gql = ctx.data::<crate::graphql::context::GqlContext>()?;
        let defs = definitions(ctx, gql, self.0.post_type).await;
        let values = crate::entry_fields::defined_values(&defs, &self.0.meta);
        Ok(async_graphql::Json(serde_json::Value::Object(values)))
    }
    /// Keys of media and entry fields whose stored id names nothing that
    /// exists outside the trash any more. Only for a caller who may edit
    /// the entry (`null` otherwise): to anyone else it would say which
    /// drafts exist.
    async fn fields_missing(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<Vec<String>>> {
        let gql = ctx.data::<crate::graphql::context::GqlContext>()?;
        if !crate::policy::may_edit(gql.principal.as_ref(), &self.0) {
            return Ok(None);
        }
        let defs = definitions(ctx, gql, self.0.post_type).await;
        Ok(Some(
            crate::entry_fields::missing_references(&gql.state.pool, &defs, &self.0.meta).await,
        ))
    }
    async fn author_id(&self) -> i64 {
        self.0.author_id
    }
    async fn parent_id(&self) -> Option<i64> {
        self.0.parent_id
    }
    async fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.created_at
    }
    async fn updated_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.updated_at
    }
    async fn published_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.0.published_at
    }
    async fn scheduled_for(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.0.scheduled_for
    }

    /// Author resolved via batched loader.
    async fn author(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GqlUser>> {
        let loader = ctx
            .data::<DataLoader<AuthorLoader>>()
            .map_err(|e| async_graphql::Error::new(format!("loader missing: {e:?}")))?;
        let row = loader
            .load_one(self.0.author_id)
            .await
            .map_err(|e| async_graphql::Error::new(format!("author load failed: {e:?}")))?;
        Ok(row.map(GqlUser))
    }

    /// Terms for this post via batched loader.
    async fn terms(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlTerm>> {
        let loader = ctx
            .data::<DataLoader<TermsForPostLoader>>()
            .map_err(|e| async_graphql::Error::new(format!("loader missing: {e:?}")))?;
        let rows = loader
            .load_one(self.0.id)
            .await
            .map_err(|e| async_graphql::Error::new(format!("terms load failed: {e:?}")))?;
        let terms = rows.unwrap_or_default().into_iter().map(GqlTerm).collect();
        Ok(terms)
    }

    /// Featured media derived from `meta.featured_media_id` via batched loader.
    async fn featured_media(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GqlMedia>> {
        let Some(val) = self
            .0
            .meta
            .get("featured_media_id")
            .and_then(|v| v.as_i64())
        else {
            return Ok(None);
        };
        let loader = ctx
            .data::<DataLoader<MediaLoader>>()
            .map_err(|e| async_graphql::Error::new(format!("loader missing: {e:?}")))?;
        let row = loader
            .load_one(val)
            .await
            .map_err(|e| async_graphql::Error::new(format!("media load failed: {e:?}")))?;
        Ok(row.map(GqlMedia))
    }
}

// Needed for SimpleObject derives that embed GqlPost.
impl From<vyasa_db::content_models::PostRow> for GqlPost {
    fn from(row: vyasa_db::content_models::PostRow) -> Self {
        Self(row)
    }
}
