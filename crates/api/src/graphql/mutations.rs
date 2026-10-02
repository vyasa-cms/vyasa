#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! GraphQL mutations — thin wrappers over the same services REST uses.

use async_graphql::{Context, ErrorExtensions, InputObject, Object, SimpleObject};
use chrono::{DateTime, Utc};

use vyasa_core::block::BlockDocument;
use vyasa_core::user::Capability;
use vyasa_db::content_models::{CommentStatus, PostStatus, PostType, Taxonomy};

use crate::graphql::context::GqlContext;
use crate::graphql::types::comment::GqlComment;
use crate::graphql::types::media::GqlMedia;
use crate::graphql::types::post::GqlPost;
use crate::graphql::types::term::GqlTerm;

/// Helper: map AppError to GraphQL error with machine code in extensions.
fn gql_err(e: vyasa_common::AppError) -> async_graphql::Error {
    let code = e.error_code();
    // The category travels in `extensions.code`; repeating it in the message
    // only makes the sentence longer for whoever renders it.
    let msg = e.client_message();
    let mut err = async_graphql::Error::new(msg);
    err = err.extend_with(|_, ext| ext.set("code", async_graphql::Value::String(code.clone())));
    err
}

/// Input for creating a post via GraphQL.
#[derive(InputObject)]
pub struct CreatePostInput {
    /// Content type (`post`, `page`, `block`). Defaults to `post`.
    pub post_type: Option<String>,
    /// Initial status (`draft`, `scheduled`, `published`, `private`, `trash`).
    pub status: Option<String>,
    /// Title.
    pub title: String,
    /// Desired slug; derived from title when absent.
    pub slug: Option<String>,
    /// Block document (`{schema_version, blocks}`).
    pub content: serde_json::Value,
    /// Optional excerpt.
    pub excerpt: Option<String>,
    /// Optional parent id.
    pub parent_id: Option<i64>,
    /// Scheduled time (required when status is `scheduled`).
    pub scheduled_for: Option<DateTime<Utc>>,
    /// Optional password for private posts.
    pub password: Option<String>,
    /// Optional term ids to assign (replace-set).
    pub term_ids: Option<Vec<i64>>,
    /// Field values keyed by field key, checked against the type's fields.
    pub fields: Option<serde_json::Value>,
}

/// Input for updating a post.
#[derive(InputObject)]
pub struct UpdatePostInput {
    /// New status.
    pub status: Option<String>,
    /// New title.
    pub title: Option<String>,
    /// New slug.
    pub slug: Option<String>,
    /// New block document.
    pub content: Option<serde_json::Value>,
    /// New excerpt.
    pub excerpt: Option<String>,
    /// New scheduled time.
    pub scheduled_for: Option<DateTime<Utc>>,
    /// New password (`""` clears).
    pub password: Option<String>,
    /// New term ids (`None` keeps, `Some(ids)` replaces).
    pub term_ids: Option<Vec<i64>>,
    /// Field values replacing the stored ones (`{}` clears); omit to keep.
    pub fields: Option<serde_json::Value>,
}

/// Input for creating a term.
#[derive(InputObject)]
pub struct CreateTermInput {
    /// Taxonomy (`category` or `tag`).
    pub taxonomy: String,
    /// Human name.
    pub name: String,
    /// Optional slug.
    pub slug: Option<String>,
    /// Optional parent id.
    pub parent_id: Option<i64>,
    /// Optional meta JSON.
    pub meta: Option<serde_json::Value>,
}

/// Input for updating a term.
#[derive(InputObject)]
pub struct UpdateTermInput {
    /// New name.
    pub name: Option<String>,
    /// New slug.
    pub slug: Option<String>,
    /// New parent id (`None` keeps, `Some(None)` clears, `Some(Some(id))` sets).
    pub parent_id: Option<Option<i64>>,
    /// New meta.
    pub meta: Option<serde_json::Value>,
}

/// Input for submitting a comment.
#[derive(InputObject)]
pub struct SubmitCommentInput {
    /// Post id.
    pub post_id: i64,
    /// Display name (for guests; if authenticated and omitted, uses user's display_name).
    pub author_name: Option<String>,
    /// Email (for guests; if authenticated and omitted, uses user's email).
    pub author_email: Option<String>,
    /// Body.
    pub content: String,
    /// Parent comment id, if reply.
    pub parent_id: Option<i64>,
}

/// Payload for deletion / merge.
#[derive(SimpleObject)]
pub struct DeletePayload {
    /// Whether the operation succeeded.
    pub success: bool,
}

/// Root mutation object.
pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Create a post — requires `edit_posts`. Mirrors `POST /api/v1/posts`.
    async fn create_post(
        &self,
        ctx: &Context<'_>,
        input: CreatePostInput,
    ) -> async_graphql::Result<GqlPost> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        principal.ensure(Capability::EditPosts).map_err(gql_err)?;

        let post_type = match &input.post_type {
            Some(s) if !s.is_empty() => PostType::parse(s).map_err(gql_err)?,
            _ => PostType::Post,
        };
        let status = match &input.status {
            Some(s) if !s.is_empty() => PostStatus::parse(s).map_err(gql_err)?,
            _ => PostStatus::Draft,
        };
        if matches!(status, PostStatus::Published | PostStatus::Scheduled) {
            principal
                .ensure(Capability::PublishPosts)
                .map_err(gql_err)?;
        }
        let content = BlockDocument::from_json(input.content).map_err(gql_err)?;
        let created = gql
            .state
            .posts
            .create_with_fields(
                vyasa_core::post::CreatePost {
                    post_type,
                    status,
                    title: input.title,
                    slug: input.slug,
                    content,
                    excerpt: input.excerpt,
                    author_id: principal.user().id,
                    parent_id: input.parent_id,
                    scheduled_for: input.scheduled_for,
                    password: input.password,
                    term_ids: input.term_ids,
                    layout: None,
                },
                input.fields,
            )
            .await
            .map_err(gql_err)?;
        crate::post_workflow::after_save(&gql.state, principal.user().id, None, &created).await;
        Ok(GqlPost(created))
    }

    /// Update a post — requires `edit_posts` + `edit_others` if not owner. Mirrors `PUT /api/v1/posts/{id}`.
    async fn update_post(
        &self,
        ctx: &Context<'_>,
        id: i64,
        input: UpdatePostInput,
    ) -> async_graphql::Result<GqlPost> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        let existing = gql.state.posts.get(id).await.map_err(gql_err)?;
        principal.ensure(Capability::EditPosts).map_err(gql_err)?;
        if existing.author_id != principal.user().id {
            principal.ensure(Capability::EditOthers).map_err(gql_err)?;
        }
        let status = match &input.status {
            Some(s) if !s.is_empty() => Some(PostStatus::parse(s).map_err(gql_err)?),
            _ => None,
        };
        if matches!(status, Some(PostStatus::Published | PostStatus::Scheduled)) {
            principal
                .ensure(Capability::PublishPosts)
                .map_err(gql_err)?;
        }
        let content = match input.content {
            Some(v) => Some(BlockDocument::from_json(v).map_err(gql_err)?),
            None => None,
        };
        let updated = gql
            .state
            .posts
            .update(
                id,
                vyasa_core::post::UpdatePost {
                    meta: None,
                    fields: input.fields,
                    status,
                    title: input.title,
                    slug: input.slug,
                    content,
                    excerpt: input.excerpt,
                    scheduled_for: input.scheduled_for,
                    clear_schedule: false,
                    password: input.password,
                    term_ids: input.term_ids,
                    layout: None,
                },
            )
            .await
            .map_err(gql_err)?;
        crate::post_workflow::after_save(
            &gql.state,
            principal.user().id,
            Some(&existing),
            &updated,
        )
        .await;
        Ok(GqlPost(updated))
    }

    /// Trash a post — requires `edit_posts` (+ `edit_others` if not owner). Mirrors `DELETE /api/v1/posts/{id}`.
    async fn trash_post(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<GqlPost> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        let existing = gql.state.posts.get(id).await.map_err(gql_err)?;
        principal.ensure(Capability::EditPosts).map_err(gql_err)?;
        if existing.author_id != principal.user().id {
            principal.ensure(Capability::EditOthers).map_err(gql_err)?;
        }
        let trashed = gql.state.posts.trash(id).await.map_err(gql_err)?;
        Ok(GqlPost(trashed))
    }

    /// Restore a trashed post to draft — requires `edit_posts` (+ `edit_others` if not owner).
    async fn restore_post(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<GqlPost> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        let existing = gql.state.posts.get(id).await.map_err(gql_err)?;
        principal.ensure(Capability::EditPosts).map_err(gql_err)?;
        if existing.author_id != principal.user().id {
            principal.ensure(Capability::EditOthers).map_err(gql_err)?;
        }
        let restored = gql.state.posts.restore(id).await.map_err(gql_err)?;
        Ok(GqlPost(restored))
    }

    /// Hard-delete a post — requires `delete_posts`. Mirrors `DELETE /api/v1/posts/{id}?force=true`.
    async fn delete_post(
        &self,
        ctx: &Context<'_>,
        id: i64,
    ) -> async_graphql::Result<DeletePayload> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        let existing = gql.state.posts.get(id).await.map_err(gql_err)?;
        principal.ensure(Capability::EditPosts).map_err(gql_err)?;
        if existing.author_id != principal.user().id {
            principal.ensure(Capability::EditOthers).map_err(gql_err)?;
        }
        principal.ensure(Capability::DeletePosts).map_err(gql_err)?;
        gql.state.posts.delete(id).await.map_err(gql_err)?;
        Ok(DeletePayload { success: true })
    }

    /// Publish a post (convenience over `updatePost` with `status: "published"`).
    async fn publish_post(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<GqlPost> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        let existing = gql.state.posts.get(id).await.map_err(gql_err)?;
        principal.ensure(Capability::EditPosts).map_err(gql_err)?;
        if existing.author_id != principal.user().id {
            principal.ensure(Capability::EditOthers).map_err(gql_err)?;
        }
        // Use service update with status Published — transition checks will enforce rules.
        let updated = gql
            .state
            .posts
            .update(
                id,
                vyasa_core::post::UpdatePost {
                    meta: None,
                    fields: None,
                    status: Some(PostStatus::Published),
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
            .await
            .map_err(gql_err)?;
        Ok(GqlPost(updated))
    }

    /// Create a term — requires `manage_categories`.
    async fn create_term(
        &self,
        ctx: &Context<'_>,
        input: CreateTermInput,
    ) -> async_graphql::Result<GqlTerm> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        principal
            .ensure(Capability::ManageCategories)
            .map_err(gql_err)?;
        let taxonomy = Taxonomy::parse(&input.taxonomy).map_err(gql_err)?;
        let term = gql
            .state
            .terms
            .create(
                taxonomy,
                &input.name,
                input.slug.as_deref(),
                input.parent_id,
                input.meta,
            )
            .await
            .map_err(gql_err)?;
        Ok(GqlTerm(term))
    }

    /// Update a term — requires `manage_categories`.
    async fn update_term(
        &self,
        ctx: &Context<'_>,
        id: i64,
        input: UpdateTermInput,
    ) -> async_graphql::Result<GqlTerm> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        principal
            .ensure(Capability::ManageCategories)
            .map_err(gql_err)?;
        let term = gql
            .state
            .terms
            .update(
                id,
                input.name.as_deref(),
                input.slug.as_deref(),
                input.parent_id,
                input.meta,
            )
            .await
            .map_err(gql_err)?;
        Ok(GqlTerm(term))
    }

    /// Delete a term, optionally reassigning posts/children — requires `manage_categories`.
    async fn delete_term(
        &self,
        ctx: &Context<'_>,
        id: i64,
        reassign_to: Option<i64>,
    ) -> async_graphql::Result<DeletePayload> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        principal
            .ensure(Capability::ManageCategories)
            .map_err(gql_err)?;
        gql.state
            .terms
            .delete(id, reassign_to)
            .await
            .map_err(gql_err)?;
        Ok(DeletePayload { success: true })
    }

    /// Merge `from` into `into` — requires `manage_categories`.
    async fn merge_terms(
        &self,
        ctx: &Context<'_>,
        from: i64,
        into_term: i64,
    ) -> async_graphql::Result<DeletePayload> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        principal
            .ensure(Capability::ManageCategories)
            .map_err(gql_err)?;
        gql.state
            .terms
            .merge(from, into_term)
            .await
            .map_err(gql_err)?;
        Ok(DeletePayload { success: true })
    }

    /// Submit a comment — public (MaybePrincipal), mirrors `POST /posts/{id}/comments`.
    async fn submit_comment(
        &self,
        ctx: &Context<'_>,
        input: SubmitCommentInput,
    ) -> async_graphql::Result<GqlComment> {
        let gql = ctx.data::<GqlContext>()?;
        // Missing, hidden or of a type the caller may not read: the same
        // answer, before anything about the comment is checked.
        // GraphQL has no unlock: a protected entry takes comments only
        // from those who may edit it.
        crate::policy::comment_target(&gql.state, gql.principal.as_ref(), input.post_id, None)
            .await
            .map_err(gql_err)?;
        // Resolve author from principal if not provided.
        let (author_name, author_email, author_user_id) = match &gql.principal {
            Some(principal) => {
                let user = principal.user();
                let name = input
                    .author_name
                    .unwrap_or_else(|| user.display_name.clone());
                let email = input.author_email.unwrap_or_else(|| user.email.clone());
                (name, email, Some(user.id))
            }
            None => {
                let name = input.author_name.ok_or_else(|| {
                    gql_err(vyasa_common::AppError::validation(
                        "author_name required for guest",
                    ))
                })?;
                let email = input.author_email.ok_or_else(|| {
                    gql_err(vyasa_common::AppError::validation(
                        "author_email required for guest",
                    ))
                })?;
                (name, email, None)
            }
        };
        let comment = gql
            .state
            .comments
            .submit(vyasa_core::comment::service::SubmitComment {
                post_id: input.post_id,
                author_user_id,
                author_name,
                author_email,
                content: input.content,
                parent_id: input.parent_id,
            })
            .await
            .map_err(gql_err)?;
        Ok(GqlComment(comment))
    }

    /// Moderate a comment — requires `moderate_comments`.
    async fn moderate_comment(
        &self,
        ctx: &Context<'_>,
        id: i64,
        status: String,
    ) -> async_graphql::Result<GqlComment> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        principal
            .ensure(Capability::ModerateComments)
            .map_err(gql_err)?;
        let new_status = CommentStatus::parse(&status).map_err(gql_err)?;
        let row = gql
            .state
            .comments
            .moderate(id, new_status)
            .await
            .map_err(gql_err)?;
        Ok(GqlComment(row))
    }

    /// Upload media — **REST-only** (documented deviation). Returns an error directing to REST.
    async fn upload_media(
        &self,
        _ctx: &Context<'_>,
        _file: Option<String>,
    ) -> async_graphql::Result<GqlMedia> {
        Err(gql_err(vyasa_common::AppError::validation(
            "media upload via GraphQL multipart is not supported; use POST /api/v1/media (multipart/form-data) and then query media(id) via GraphQL",
        )))
    }
}
