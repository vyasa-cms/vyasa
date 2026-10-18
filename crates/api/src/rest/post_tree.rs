//! The tree of a hierarchical post type: reading it and reordering it.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::post::hierarchy::{self, Move};
use vyasa_core::user::Capability;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::posts::TreeRow;

use crate::error::{ApiError, ApiResult};
use crate::middleware::principal::MaybePrincipal;
use crate::middleware::Principal;
use crate::state::AppState;

/// Most nodes one tree answer carries.
const MAX_NODES: usize = 5000;

/// `GET …/tree` query.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct TreeQuery {
    /// `all` returns every status; it needs `edit_posts`.
    pub status: Option<String>,
}

/// One node of the tree.
#[derive(Serialize, utoipa::ToSchema)]
pub struct TreeNodeJson {
    /// Entry id, as a string.
    pub id: String,
    /// Parent id, as a string.
    pub parent_id: Option<String>,
    /// Order among siblings.
    pub position: i32,
    /// Title.
    pub title: String,
    /// URL segment.
    pub slug: String,
    /// Slash-joined slugs from the root.
    pub path: String,
    /// Public address.
    pub url: String,
    /// Lifecycle status.
    pub status: String,
    /// Nesting depth, 1 at the top.
    pub depth: usize,
}

/// The tree answer.
#[derive(Serialize, utoipa::ToSchema)]
pub struct TreeResponse {
    /// The post type.
    #[serde(rename = "type")]
    pub post_type: String,
    /// Always true (a type that is not hierarchical answers 400).
    pub hierarchical: bool,
    /// More nodes exist than were returned.
    pub truncated: bool,
    /// Nodes in reading order.
    pub nodes: Vec<TreeNodeJson>,
}

/// `PUT …/tree` body.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct MovesBody {
    /// The moves, applied as a whole.
    pub moves: Vec<MoveJson>,
}

/// One requested move.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct MoveJson {
    /// Entry id.
    #[serde(deserialize_with = "crate::flexible_id::required")]
    #[schema(value_type = String)]
    pub id: i64,
    /// New parent, `null` for the top.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    #[schema(value_type = Option<String>)]
    pub parent_id: Option<i64>,
    /// New position.
    pub position: i32,
}

/// The hierarchical type named in the path, or a 400 naming why not.
fn hierarchical_type(slug: &str) -> Result<PostType, ApiError> {
    let post_type = PostType::parse(slug)
        .map_err(|_| AppError::validation(format!("unknown post type: {slug:?}")))?;
    if !post_type.is_hierarchical() {
        return Err(ApiError(AppError::validation(format!(
            "{slug} is not a hierarchical type"
        ))));
    }
    Ok(post_type)
}

/// Builds the answer from the type's live rows; `everything` keeps drafts
/// and protected entries, otherwise only published open entries whose
/// ancestors are all shown too.
fn build(post_type: PostType, rows: &[TreeRow], everything: bool) -> TreeResponse {
    let nodes: Vec<hierarchy::Node> = rows
        .iter()
        .map(|r| hierarchy::Node {
            id: r.id,
            parent_id: r.parent_id,
            position: r.position,
            slug: r.slug.clone(),
        })
        .collect();
    let by_id: BTreeMap<i64, &TreeRow> = rows.iter().map(|r| (r.id, r)).collect();
    let mut shown: BTreeSet<i64> = BTreeSet::new();
    let mut out = Vec::new();
    let mut truncated = false;
    for (id, depth) in hierarchy::preorder(&nodes) {
        let Some(row) = by_id.get(&id) else { continue };
        let open = everything || (row.status == PostStatus::Published && !row.locked);
        let parent_shown = row
            .parent_id
            .filter(|p| by_id.contains_key(p))
            .is_none_or(|p| shown.contains(&p));
        if !open || !parent_shown {
            continue;
        }
        shown.insert(id);
        if out.len() >= MAX_NODES {
            truncated = true;
            break;
        }
        let path = row.path.clone().unwrap_or_else(|| row.slug.clone());
        out.push(TreeNodeJson {
            id: id.to_string(),
            parent_id: row.parent_id.map(|p| p.to_string()),
            position: row.position,
            title: row.title.clone(),
            slug: row.slug.clone(),
            url: format!("/{}/{}", post_type.base(), path),
            path,
            status: row.status.as_str().to_owned(),
            depth,
        });
    }
    TreeResponse {
        post_type: post_type.as_str().to_owned(),
        hierarchical: true,
        truncated,
        nodes: out,
    }
}

/// `GET /api/v1/post-types/{type}/tree` — the entries of a hierarchical
/// type in reading order. Public callers see published, unprotected
/// entries; `status=all` (needs `edit_posts`) lists everything.
#[utoipa::path(
    get, path = "/api/v1/post-types/{type}/tree", tag = "posts",
    params(("type" = String, Path, description = "Post type slug"), TreeQuery),
    responses(
        (status = 200, description = "The tree", body = TreeResponse),
        (status = 400, description = "Not a hierarchical type", body = crate::error::ApiErrorBody),
        (status = 403, description = "status=all without edit_posts", body = crate::error::ApiErrorBody),
    )
)]
pub async fn get(
    State(state): State<AppState>,
    MaybePrincipal(principal): MaybePrincipal,
    Path(slug): Path<String>,
    Query(query): Query<TreeQuery>,
) -> ApiResult<Json<TreeResponse>> {
    let post_type = hierarchical_type(&slug)?;
    let everything = query.status.as_deref() == Some("all");
    if everything {
        principal
            .as_ref()
            .ok_or_else(|| AppError::forbidden("status=all needs a session that edits posts"))?
            .ensure(Capability::EditPosts)?;
    } else if !crate::policy::may_read_type(&state, principal.as_ref(), post_type).await {
        return Err(ApiError(AppError::not_found("post type", &slug)));
    }
    let rows = state.posts.tree(post_type).await?;
    Ok(Json(build(post_type, &rows, everything)))
}

/// `PUT /api/v1/post-types/{type}/tree` — reorder and re-parent entries
/// in one request, applied as a whole. Published entries whose address
/// changes keep a redirect from the old one.
#[utoipa::path(
    put, path = "/api/v1/post-types/{type}/tree", tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("type" = String, Path, description = "Post type slug")),
    request_body = MovesBody,
    responses(
        (status = 200, description = "The new tree", body = TreeResponse),
        (status = 400, description = "A rule the final tree breaks", body = crate::error::ApiErrorBody),
        (status = 403, description = "Missing edit_posts / edit_others", body = crate::error::ApiErrorBody),
        (status = 404, description = "An id is not an entry of the type", body = crate::error::ApiErrorBody),
        (status = 409, description = "A path is taken", body = crate::error::ApiErrorBody),
    )
)]
pub async fn put(
    State(state): State<AppState>,
    principal: Principal,
    Path(slug): Path<String>,
    Json(body): Json<MovesBody>,
) -> ApiResult<Json<TreeResponse>> {
    let post_type = hierarchical_type(&slug)?;
    principal.ensure(Capability::EditPosts)?;
    let rows = state.posts.tree(post_type).await?;
    let known: BTreeMap<i64, &TreeRow> = rows.iter().map(|r| (r.id, r)).collect();
    let moves: Vec<Move> = body
        .moves
        .iter()
        .map(|m| Move {
            id: m.id,
            parent_id: m.parent_id,
            position: m.position,
        })
        .collect();
    for m in &moves {
        if !known.contains_key(&m.id) {
            return Err(ApiError(AppError::not_found("post", m.id)));
        }
    }
    // Everything whose place or address changes must be the caller's own,
    // or the caller must edit others' entries.
    let before: Vec<hierarchy::Node> = rows
        .iter()
        .map(|r| hierarchy::Node {
            id: r.id,
            parent_id: r.parent_id,
            position: r.position,
            slug: r.slug.clone(),
        })
        .collect();
    let after = hierarchy::apply_moves(&before, &moves).map_err(AppError::validation)?;
    let mut touched: BTreeSet<i64> = moves.iter().map(|m| m.id).collect();
    touched.extend(
        hierarchy::changed_paths(&before, &after)
            .into_iter()
            .map(|(id, _, _)| id),
    );
    let someone_elses = touched.iter().any(|id| {
        known
            .get(id)
            .is_some_and(|r| r.author_id != principal.user().id)
    });
    if someone_elses {
        principal.ensure(Capability::EditOthers)?;
    }
    let outcome = state.posts.move_entries(post_type, &moves).await?;
    let base = post_type.base();
    for (id, old, new) in &outcome.changed_paths {
        if known
            .get(id)
            .is_some_and(|r| r.status == PostStatus::Published)
        {
            if let Err(e) = crate::seo::add_redirect(
                &state.pool,
                &format!("/{base}/{old}"),
                &format!("/{base}/{new}"),
            )
            .await
            {
                tracing::warn!("could not record redirect: {e}");
            }
        }
    }
    let rows = state.posts.tree(post_type).await?;
    Ok(Json(build(post_type, &rows, true)))
}
