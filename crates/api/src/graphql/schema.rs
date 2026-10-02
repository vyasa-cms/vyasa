#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! GraphQL schema.

use async_graphql::{Context, Object};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use vyasa_db::content_models::{PostStatus, PostType, Taxonomy};

use crate::graphql::context::GqlContext;
use crate::graphql::types::comment::{CommentConnection, CommentEdge, CommentPageInfo, GqlComment};
use crate::graphql::types::media::GqlMedia;
use crate::graphql::types::post::{GqlPost, GqlSearchHit, PageInfo, PostConnection, PostEdge};
use crate::graphql::types::term::{GqlTerm, GqlTermWithCount, GqlTermWrapper};
use crate::graphql::types::user::GqlUser;

/// Root query object.
pub struct QueryRoot;

#[Object]
impl QueryRoot {
    /// Current authenticated user (`null` when unauthenticated — errors when not logged in).
    async fn me(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlUser> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        Ok(GqlUser(principal.user().clone()))
    }

    /// Listing users — requires `manage_users` capability. Paginated.
    async fn users(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> async_graphql::Result<Vec<GqlUser>> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        principal.ensure(vyasa_core::user::Capability::ManageUsers)?;
        // cursor offset based
        let offset = decode_cursor(after.as_deref())?;
        let limit = first.unwrap_or(20).clamp(1, 100) as u32;
        let rows = gql
            .state
            .users
            .list(limit, offset as u32)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(rows.into_iter().map(GqlUser).collect())
    }

    /// Fetch a single user by id — requires ManageUsers or self.
    async fn user(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<GqlUser> {
        let gql = ctx.data::<GqlContext>()?;
        let principal = gql.require_principal()?;
        if principal.user().id != id {
            principal.ensure(vyasa_core::user::Capability::ManageUsers)?;
        }
        let row = gql
            .state
            .users
            .get(id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(GqlUser(row))
    }

    /// Paged posts with optional filters, cursor pagination.
    #[allow(clippy::too_many_arguments)]
    async fn posts(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        status: Option<String>,
        post_type: Option<String>,
        author_id: Option<i64>,
        term_id: Option<i64>,
        search: Option<String>,
    ) -> async_graphql::Result<PostConnection> {
        let gql = ctx.data::<GqlContext>()?;
        let offset = decode_cursor(after.as_deref())?;
        let limit = first.unwrap_or(20).clamp(1, 100) as u32;
        // Parse filters.
        let status_filter = match status.as_deref() {
            Some(s) if !s.is_empty() => {
                Some(PostStatus::parse(s).map_err(|e| async_graphql::Error::new(e.to_string()))?)
            }
            _ => None,
        };
        let type_filter = match post_type.as_deref() {
            Some(s) if !s.is_empty() => {
                Some(PostType::parse(s).map_err(|e| async_graphql::Error::new(e.to_string()))?)
            }
            _ => None,
        };
        let can_edit_others = gql.can(vyasa_core::user::Capability::EditOthers);
        let own_author = gql
            .principal
            .as_ref()
            .filter(|p| p.ensure(vyasa_core::user::Capability::EditPosts).is_ok())
            .map(|p| p.user().id);
        let filter = vyasa_db::repo::PostFilter {
            status: status_filter,
            sort: vyasa_db::repo::PostSort::Newest,
            post_type: type_filter,
            author_id,
            term_id,
            search: search.clone(),
            published_month: None,
            limit,
            offset: u32::try_from(offset)
                .map_err(|_| async_graphql::Error::new("cursor too large"))?,
            sticky_first: false,
            readable_types: crate::policy::readable_custom_types(
                &gql.state,
                gql.principal.as_ref(),
            )
            .await,
        };
        let repo = vyasa_db::repo::PostsRepo::new(gql.state.pool.clone());
        let items = repo
            .list_visible(&filter, can_edit_others, own_author)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let total = repo
            .count_visible(&filter, can_edit_others, own_author)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let mut edges = Vec::new();
        for (idx, row) in items.into_iter().enumerate() {
            let cur_offset = offset + idx as i64;
            edges.push(PostEdge {
                cursor: encode_cursor(cur_offset + 1),
                node: GqlPost(row),
            });
        }
        let has_next = (offset + edges.len() as i64) < total;
        let end_cursor = edges.last().map(|e| e.cursor.clone());
        Ok(PostConnection {
            edges,
            page_info: PageInfo {
                has_next_page: has_next,
                end_cursor,
                has_previous_page: offset > 0,
            },
            total_count: total,
        })
    }

    /// Fetch a single post by id — respects draft/private visibility.
    async fn post(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GqlPost>> {
        let gql = ctx.data::<GqlContext>()?;
        let row = match gql.state.posts.get(id).await {
            Ok(r) => r,
            Err(e) if e.to_string().contains("not found") => return Ok(None),
            Err(e) => return Err(async_graphql::Error::new(e.to_string())),
        };
        if !can_see(&gql, &row) {
            return Err(async_graphql::Error::new(
                "forbidden: you do not have permission to view this post",
            ));
        }
        // An entry of a type the caller may not read does not exist for them.
        if !crate::policy::may_read_type(&gql.state, gql.principal.as_ref(), row.post_type).await {
            return Ok(None);
        }
        Ok(Some(GqlPost(row)))
    }

    /// Full-text search over published posts.
    ///
    /// Published-only regardless of who is asking: the index holds nothing
    /// else, and every hit is re-read through the repository so an entry
    /// left behind by an unpublish cannot leak.
    async fn search(
        &self,
        ctx: &Context<'_>,
        query: String,
        limit: Option<i32>,
    ) -> async_graphql::Result<Vec<GqlSearchHit>> {
        let gql = ctx.data::<GqlContext>()?;
        let term = query.trim();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let limit = usize::try_from(limit.unwrap_or(10))
            .unwrap_or(10)
            .clamp(1, 50);

        let Some(index) = gql.state.search_index.as_ref() else {
            return Err(async_graphql::Error::new("search index unavailable"));
        };

        let mut out = Vec::new();
        for hit in index.search(term, None, limit, 0).unwrap_or_default() {
            let Ok(id) = i64::try_from(hit.id) else {
                continue;
            };
            let Ok(row) = gql.state.posts.get(id).await else {
                continue;
            };
            if row.status != PostStatus::Published || !can_see(gql, &row) {
                continue;
            }
            if !crate::policy::may_read_type(&gql.state, gql.principal.as_ref(), row.post_type)
                .await
            {
                continue;
            }
            out.push(GqlSearchHit {
                snippet: hit.snippet,
                post: GqlPost(row),
            });
        }
        Ok(out)
    }

    /// Fetch a post by type + slug — respects visibility.
    async fn post_by_slug(
        &self,
        ctx: &Context<'_>,
        post_type: String,
        slug: String,
    ) -> async_graphql::Result<Option<GqlPost>> {
        let gql = ctx.data::<GqlContext>()?;
        let pt =
            PostType::parse(&post_type).map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let row = match gql.state.posts.get_by_slug(pt, &slug).await {
            Ok(r) => r,
            Err(e) if e.to_string().contains("not found") => return Ok(None),
            Err(e) => return Err(async_graphql::Error::new(e.to_string())),
        };
        if !can_see(&gql, &row) {
            return Err(async_graphql::Error::new(
                "forbidden: you do not have permission to view this post",
            ));
        }
        if !crate::policy::may_read_type(&gql.state, gql.principal.as_ref(), row.post_type).await {
            return Ok(None);
        }
        Ok(Some(GqlPost(row)))
    }

    /// List terms, optionally filtered by taxonomy.
    async fn terms(
        &self,
        ctx: &Context<'_>,
        taxonomy: Option<String>,
        with_counts: Option<bool>,
    ) -> async_graphql::Result<Vec<GqlTermWithCount>> {
        let gql = ctx.data::<GqlContext>()?;
        let tax = match taxonomy.as_deref() {
            Some(s) if !s.is_empty() => {
                Some(Taxonomy::parse(s).map_err(|e| async_graphql::Error::new(e.to_string()))?)
            }
            _ => None,
        };
        let rows = gql
            .state
            .terms
            .list(tax, with_counts.unwrap_or(false))
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        // Map TermRepo::TermWithCount into GqlTermWithCount
        Ok(rows
            .into_iter()
            .map(|twc| GqlTermWithCount {
                term: GqlTermWrapper(twc.term),
                post_count: twc.post_count,
            })
            .collect())
    }

    /// Fetch a term by id.
    async fn term(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GqlTerm>> {
        let gql = ctx.data::<GqlContext>()?;
        match gql.state.terms.get(id).await {
            Ok(row) => Ok(Some(GqlTerm(row))),
            Err(e) if e.to_string().contains("not found") => Ok(None),
            Err(e) => Err(async_graphql::Error::new(e.to_string())),
        }
    }

    /// List media (paginated via cursor — simple wrapper over offset).
    async fn media(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> async_graphql::Result<Vec<GqlMedia>> {
        let gql = ctx.data::<GqlContext>()?;
        let offset = decode_cursor(after.as_deref())?;
        let limit = first.unwrap_or(20).clamp(1, 100) as i64;
        let rows = gql
            .state
            .media
            .list(limit, offset)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(rows.into_iter().map(GqlMedia).collect())
    }

    /// Fetch a single media by id.
    async fn media_by_id(
        &self,
        ctx: &Context<'_>,
        id: i64,
    ) -> async_graphql::Result<Option<GqlMedia>> {
        let gql = ctx.data::<GqlContext>()?;
        match gql.state.media.get(id).await {
            Ok(row) => Ok(Some(GqlMedia(row))),
            Err(e) if e.to_string().contains("not found") => Ok(None),
            Err(e) => Err(async_graphql::Error::new(e.to_string())),
        }
    }

    /// Approved threaded comments for a post, with cursor pagination.
    async fn comments(
        &self,
        ctx: &Context<'_>,
        post_id: i64,
        first: Option<i32>,
        after: Option<String>,
    ) -> async_graphql::Result<CommentConnection> {
        let gql = ctx.data::<GqlContext>()?;
        // Comments follow the visibility of their parent post: missing,
        // hidden (a protected entry included: GraphQL has no unlock) or of
        // a type the caller may not read all answer alike.
        crate::policy::comment_target(&gql.state, gql.principal.as_ref(), post_id, None)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let all = gql
            .state
            .comments
            .list_approved_threaded(post_id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let total = all.len() as i64;
        let offset = decode_cursor(after.as_deref())?;
        let limit = first.unwrap_or(20).clamp(1, 100) as usize;
        let start = offset as usize;
        let end = (start + limit).min(all.len());
        let slice = if start < all.len() {
            &all[start..end]
        } else {
            &[] as &[vyasa_db::content_models::CommentRow]
        };
        let mut edges = Vec::new();
        for (idx, row) in slice.iter().enumerate() {
            let cur = start + idx;
            edges.push(CommentEdge {
                cursor: encode_cursor(cur as i64 + 1),
                node: GqlComment(row.clone()),
            });
        }
        Ok(CommentConnection {
            edges: edges.clone(),
            page_info: CommentPageInfo {
                has_next_page: (start + slice.len()) < all.len(),
                end_cursor: edges.last().map(|e| e.cursor.clone()),
            },
            total_count: total,
        })
    }

    /// Fetch a single option value (public subset only unless authenticated with ManageOptions).
    async fn option(
        &self,
        ctx: &Context<'_>,
        key: String,
    ) -> async_graphql::Result<Option<async_graphql::Json<serde_json::Value>>> {
        let gql = ctx.data::<GqlContext>()?;
        // Public allowlist — safe to expose without caps.
        const PUBLIC: &[&str] = &["site_title", "site_tagline", "site_url", "posts_per_page"];
        let is_public = PUBLIC.contains(&key.as_str());
        if !is_public {
            // Require ManageOptions to read non-public keys.
            let principal = gql.require_principal()?;
            principal.ensure(vyasa_core::user::Capability::ManageOptions)?;
        }
        let val = match gql.state.options.get(&key).await {
            Ok(v) => v,
            Err(e) if e.to_string().contains("not found") => return Ok(None),
            Err(e) => return Err(async_graphql::Error::new(e.to_string())),
        };
        Ok(Some(async_graphql::Json(val)))
    }

    /// Headless `site` query: identity + public settings in one call.
    async fn site(&self, ctx: &Context<'_>) -> async_graphql::Result<SiteObject> {
        let gql = ctx.data::<GqlContext>()?;
        Ok(SiteObject {
            title: gql
                .state
                .options_service
                .string_option("site_title")
                .await
                .unwrap_or_default(),
            tagline: gql
                .state
                .options_service
                .string_option("site_tagline")
                .await
                .unwrap_or_default(),
            posts_per_page: gql
                .state
                .options_service
                .posts_per_page()
                .await
                .unwrap_or(10),
        })
    }

    /// All public options as key/value pairs.
    async fn options(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<OptionEntry>> {
        let gql = ctx.data::<GqlContext>()?;
        // Only public subset exposed via list.
        const PUBLIC: &[&str] = &["site_title", "site_tagline", "site_url", "posts_per_page"];
        let mut out = Vec::new();
        for key in PUBLIC {
            if let Ok(val) = gql.state.options.get(key).await {
                out.push(OptionEntry {
                    key: (*key).to_string(),
                    value: async_graphql::Json(val),
                });
            }
        }
        Ok(out)
    }
}

/// Public site identity for headless consumers.
#[derive(async_graphql::SimpleObject)]
pub struct SiteObject {
    /// Site title.
    pub title: String,
    /// Tagline.
    pub tagline: String,
    /// Effective listing page size.
    pub posts_per_page: i64,
}

#[derive(Clone)]
struct OptionEntry {
    key: String,
    value: async_graphql::Json<serde_json::Value>,
}

#[async_graphql::Object]
impl OptionEntry {
    async fn key(&self) -> &str {
        &self.key
    }
    async fn value(&self) -> &async_graphql::Json<serde_json::Value> {
        &self.value
    }
}

fn can_see(gql: &GqlContext, row: &vyasa_db::content_models::PostRow) -> bool {
    if row.status == PostStatus::Published && row.password_hash.is_none() {
        return true;
    }
    // Unpublished or protected: require scoped edit access.
    let Some(principal) = &gql.principal else {
        return false;
    };
    if row.author_id == principal.user().id && gql.can(vyasa_core::user::Capability::EditPosts) {
        return true;
    }
    gql.can(vyasa_core::user::Capability::EditOthers)
}

fn encode_cursor(offset: i64) -> String {
    BASE64.encode(offset.to_string().as_bytes())
}

fn decode_cursor(s: Option<&str>) -> async_graphql::Result<i64> {
    let Some(s) = s else { return Ok(0) };
    if s.is_empty() {
        return Ok(0);
    }
    let bytes = BASE64
        .decode(s)
        .map_err(|e| async_graphql::Error::new(format!("invalid cursor: {e}")))?;
    let str = String::from_utf8(bytes)
        .map_err(|e| async_graphql::Error::new(format!("invalid cursor utf8: {e}")))?;
    let val: i64 = str
        .parse()
        .map_err(|e| async_graphql::Error::new(format!("invalid cursor offset: {e}")))?;
    if val < 0 {
        Err(async_graphql::Error::new("invalid negative cursor"))
    } else {
        Ok(val)
    }
}
