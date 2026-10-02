//! Post CRUD endpoints.

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use vyasa_common::AppError;
use vyasa_core::block::BlockDocument;
use vyasa_core::post::{CreatePost, UpdatePost};
use vyasa_core::user::Capability;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::PostFilter;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

/// Shared response shape for a post.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PostResponse {
    /// Canonical public path under the current permalink setting.
    pub public_url: Option<String>,
    /// Post id.
    pub id: i64,
    /// Content type (`post`, `page`, `block`).
    #[serde(rename = "type")]
    pub post_type: String,
    /// Lifecycle status.
    pub status: String,
    /// URL slug.
    pub slug: String,
    /// Title.
    pub title: String,
    /// Block document.
    pub content: serde_json::Value,
    /// Section tree composing this entry, when it has one. `null` means the
    /// entry renders through its theme template.
    pub layout: Option<serde_json::Value>,
    /// Optional excerpt.
    pub excerpt: Option<String>,
    /// Author id.
    pub author_id: i64,
    /// Optional parent.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Extension metadata (SEO and the like). Field values are not in
    /// here: they are `fields`.
    pub meta: serde_json::Value,
    /// The entry's field values, keyed by field key: only fields its type
    /// still defines. Media and entry references are ids as strings.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub fields: serde_json::Value,
    /// Keys of media and entry fields whose stored id no longer names
    /// something that exists outside the trash. The stale id is still in
    /// `fields` (saving it back unchanged is accepted); the public site
    /// renders nothing for it. Filled on single-entry responses for a
    /// caller who may edit the entry; `null` otherwise (and in listings):
    /// to anyone else it would say which drafts exist.
    #[serde(default)]
    pub fields_missing: Option<Vec<String>>,
    /// Keys a revision restore dropped because their field is gone or no
    /// longer accepts the old value. Only a restore fills this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_dropped: Vec<String>,
    /// First publication time.
    pub published_at: Option<DateTime<Utc>>,
    /// Scheduled time.
    pub scheduled_for: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Pinned to the top of the home listing.
    pub sticky: bool,
    /// BCP 47 tag; empty means the site's language.
    pub lang: String,
    /// The group this entry shares with its translations.
    pub translation_group: Option<i64>,
    /// Other entries in the group: `{id, lang, slug, title, status}`.
    #[serde(default)]
    pub translations: Vec<serde_json::Value>,
}

impl From<vyasa_db::content_models::PostRow> for PostResponse {
    fn from(row: vyasa_db::content_models::PostRow) -> Self {
        Self {
            public_url: None,
            id: row.id,
            post_type: row.post_type.as_str().to_string(),
            status: row.status.as_str().to_string(),
            slug: row.slug,
            title: row.title,
            content: row.content,
            layout: row.layout,
            excerpt: row.excerpt,
            author_id: row.author_id,
            parent_id: row.parent_id,
            // Filled by `response_with` from the type's definitions; the
            // raw stored object would carry a deleted field's values.
            fields: serde_json::Value::Object(serde_json::Map::new()),
            fields_missing: None,
            fields_dropped: Vec::new(),
            meta: crate::entry_fields::meta_without_fields(row.meta),
            published_at: row.published_at,
            scheduled_for: row.scheduled_for,
            sticky: row.sticky,
            lang: row.lang.clone(),
            translation_group: row.translation_group,
            translations: Vec::new(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Request body for creating a post.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreatePostRequest {
    /// Content type (`post`, `page`, `block`). Defaults to `post`.
    #[serde(rename = "type", default)]
    pub post_type: Option<String>,
    /// Initial status (`draft`, `scheduled`, `published`, `private`, `trash`).
    #[serde(default)]
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
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Scheduled time (required when status is `scheduled`).
    pub scheduled_for: Option<DateTime<Utc>>,
    /// Optional password for private posts.
    pub password: Option<String>,
    /// Optional term ids to assign (replace-set).
    #[serde(default, deserialize_with = "crate::flexible_id::option_vec")]
    pub term_ids: Option<Vec<i64>>,
    /// Section tree composing this entry. Omit for a normal entry.
    pub layout: Option<serde_json::Value>,
    /// Pin to the top of the home listing.
    #[serde(default)]
    pub sticky: bool,
    /// BCP 47 tag of the entry's language; empty for the site's.
    pub lang: Option<String>,
    /// Another entry this one translates; both join one group.
    pub translation_of: Option<i64>,
    /// Field values keyed by field key, checked against the type's
    /// fields (unknown keys refused; required ones needed to publish or
    /// schedule).
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub fields: Option<serde_json::Value>,
}

/// Request body for updating a post; all fields are optional.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdatePostRequest {
    /// Free-form metadata (SEO fields and the like); omit to keep. A
    /// `fields` key in here is ignored: values are written only through
    /// `fields`.
    pub meta: Option<serde_json::Value>,
    /// Field values, replacing the stored ones (`{}` clears them); omit
    /// to keep. Checked against the type's fields.
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub fields: Option<serde_json::Value>,
    /// New status.
    pub status: Option<String>,
    /// New title.
    pub title: Option<String>,
    /// New slug.
    pub slug: Option<String>,
    /// New block document.
    pub content: Option<serde_json::Value>,
    /// New excerpt (use empty string to clear; null keeps current).
    pub excerpt: Option<String>,
    /// New password (`""` clears).
    pub password: Option<String>,
    /// New scheduled time. Omit to keep it, send `null` to clear it.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<DateTime<Utc>>)]
    // Three states, not two: absent keeps the schedule, `null` clears it,
    // a time sets it. The nested option is how serde spells that.
    #[allow(clippy::option_option)]
    pub scheduled_for: Option<Option<DateTime<Utc>>>,
    /// New term ids (`None` keeps, `Some(ids)` replaces).
    #[serde(default, deserialize_with = "crate::flexible_id::option_vec")]
    pub term_ids: Option<Vec<i64>>,
    /// New section tree; omit to keep, `[]` or `null` to clear so the entry
    /// renders through its theme template again.
    pub layout: Option<serde_json::Value>,
    /// When set, the update applies only if the post has not changed since
    /// this moment; otherwise 409, so two tabs cannot silently overwrite
    /// each other.
    #[serde(default)]
    pub expected_updated_at: Option<DateTime<Utc>>,
    /// Pin to the top of the home listing.
    pub sticky: Option<bool>,
    /// BCP 47 tag of the entry's language; empty for the site's.
    pub lang: Option<String>,
    /// Another entry this one translates; both join one group.
    pub translation_of: Option<i64>,
}

/// Query parameters for listing posts.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct ListPostsQuery {
    /// Filter by status.
    pub status: Option<String>,
    /// Filter by type.
    #[serde(rename = "type")]
    pub post_type: Option<String>,
    /// Filter by author id.
    pub author_id: Option<i64>,
    /// Filter by term id.
    pub term_id: Option<i64>,
    /// Search substring in title.
    pub search: Option<String>,
    /// Page number (1-based, default 1).
    pub page: Option<u32>,
    /// Page size (default 20, max 100).
    pub per_page: Option<u32>,
}

/// Paginated list response.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PaginatedPosts {
    /// Posts on this page.
    pub items: Vec<PostResponse>,
    /// Total matching posts.
    pub total: i64,
    /// Current page.
    pub page: u32,
    /// Page size.
    pub per_page: u32,
}

/// Helper: parse optional status string to `PostStatus`.
fn parse_status(value: Option<&String>) -> Result<Option<PostStatus>, AppError> {
    match value {
        Some(raw) if !raw.is_empty() => PostStatus::parse(raw)
            .map(Some)
            .map_err(|_| AppError::validation(format!("invalid post status: {raw:?}"))),
        _ => Ok(None),
    }
}

/// Helper: parse optional type string to `PostType`.
fn parse_post_type(value: Option<&String>) -> Result<Option<PostType>, AppError> {
    match value {
        Some(raw) if !raw.is_empty() => PostType::parse(raw)
            .map(Some)
            .map_err(|_| AppError::validation(format!("invalid post type: {raw:?}"))),
        _ => Ok(None),
    }
}

/// Parses and checks an entry's section tree before it is stored.
///
/// Returns the canonical JSON to persist; an empty tree stores as `[]`,
/// which is how an entry says "render me through the theme template".
///
/// Scopes are checked against the *active* theme's palette when there is
/// one. A `$role` that does not exist, or a hex that does not parse, emits
/// no CSS at all — without this check the only symptom would be a band that
/// silently isn't dark.
///
/// Only pages may compose. A composed body replaces the whole template
/// body, and the `single` template is where the byline, the dated `<time>`
/// and the taxonomy links live — none of which exist as section kinds yet,
/// so a composed post would lose them with no way to put them back. Lift
/// this the moment those kinds exist, not before.
async fn validated_layout(
    state: &AppState,
    post_type: PostType,
    value: serde_json::Value,
) -> Result<serde_json::Value, ApiError> {
    if value.is_null() {
        return Ok(serde_json::json!([]));
    }
    let sections: Vec<vyasa_themes::Section> = serde_json::from_value(value)
        .map_err(|err| ApiError(AppError::validation(format!("layout: {err}"))))?;
    if sections.is_empty() {
        return Ok(serde_json::json!([]));
    }
    if post_type != PostType::Page {
        return Err(ApiError(AppError::validation(
            "layout: only pages compose their own sections",
        )));
    }
    let tokens = match state.themes.get_active().await {
        Ok(Some(active)) => serde_json::from_value::<vyasa_themes::TokenSet>(active.tokens).ok(),
        // A site with no active theme can still be edited; the palette
        // checks are simply unavailable until one is activated.
        Ok(None) | Err(_) => None,
    };
    let registry = crate::plugin_sections::registry_with_plugins(state).await;
    let diags = vyasa_themes::validate_sections(&sections, &registry, tokens.as_ref(), "layout");
    let errors: Vec<_> = diags
        .into_iter()
        .filter(vyasa_themes::TokenDiagnostic::is_error)
        .collect();
    if !errors.is_empty() {
        return Err(ApiError(AppError::validation(
            crate::theme_studio::diagnostics_message(&errors),
        )));
    }
    serde_json::to_value(&sections)
        .map_err(|err| ApiError(AppError::validation(format!("layout: {err}"))))
}

/// Request body for composing a page with the assistant.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ComposeRequest {
    /// What the author asked for, in their words.
    pub message: String,
    /// The block document as the editor holds it, so the designer works
    /// from what the author sees rather than the last save.
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    /// The section tree as the editor holds it, for the same reason.
    #[serde(default)]
    pub sections: Option<serde_json::Value>,
}

/// A proposed page, for the author to keep or discard.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ComposeResponse {
    /// What the designer composed and why. Plain text.
    pub reply: String,
    /// The complete tree it proposes for this page.
    pub sections: serde_json::Value,
    /// Non-blocking notes about the result, mostly contrast. Shown to the
    /// author because a band whose secondary text is unreadable validates
    /// perfectly well and only looks wrong on the published page.
    pub warnings: serde_json::Value,
    /// The agent's work log: `{thought, tool, input, observation}` per
    /// step, so the author can see how the page came to be.
    pub steps: serde_json::Value,
}

/// `POST /api/v1/posts/{id}/compose` — propose a section tree for a page.
///
/// Nothing is stored. The tree comes back for the author to accept in the
/// editor and save like any other edit, so a suggestion that misses costs
/// a glance rather than an undo — and it goes through the same validation,
/// permissions and revisions as a tree they built by hand.
///
/// # Errors
///
/// 400 when the entry is not a page or no text model is registered, 403
/// without `edit_posts` (plus `edit_others` when not the author), 404 when
/// the page is missing, 422 when the designer cannot produce a valid tree.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/compose",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Page id")),
    request_body = ComposeRequest,
    responses(
        (status = 200, description = "Proposed sections", body = ComposeResponse),
        (status = 400, description = "Not a page, or no model", body = ApiErrorBody),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Missing edit_posts / edit_others", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn compose(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<ComposeRequest>,
) -> ApiResult<Json<ComposeResponse>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    if post.post_type != PostType::Page {
        return Err(ApiError(AppError::validation(
            "only pages compose their own sections",
        )));
    }
    if body.message.trim().is_empty() {
        return Err(ApiError(AppError::validation(
            "say what the page should do",
        )));
    }

    // Scopes are judged against the palette the page will render in, so a
    // site with no active theme cannot be composed for meaningfully.
    let tokens = match state.themes.get_active().await? {
        Some(active) => serde_json::from_value::<vyasa_themes::TokenSet>(active.tokens)
            .map_err(|e| ApiError(AppError::validation(format!("active theme tokens: {e}"))))?,
        None => {
            return Err(ApiError(AppError::validation(
                "activate a theme before composing a page",
            )))
        }
    };

    // The editor's unsaved state wins over the stored row: an author who
    // just typed a paragraph expects the designer to have read it.
    let content = body.content.unwrap_or_else(|| post.content.clone());
    let doc: vyasa_core::BlockDocument = serde_json::from_value(content)
        .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
    let prose = vyasa_ai::assist::extract_text(&doc.blocks, 4000);
    let current = vyasa_themes::page_sections(body.sections.as_ref().or(post.layout.as_ref()));

    let identity = state.options_service.site_identity().await?;
    let site = if identity.tagline.is_empty() {
        identity.title.clone()
    } else {
        format!("{} — {}", identity.title, identity.tagline)
    };
    let brand = state.options_service.brand_kit_prompt().await?;
    let vocabulary = crate::rest::content_types::vocabulary_with_sources(&state).await?;
    let provider = crate::ai_registry::text(&state).await?;
    let sink = crate::ai_registry::sink(&state).await;
    let live_registry = crate::plugin_sections::registry_with_plugins(&state).await;
    let system = vyasa_ai::page_designer::system_prompt(&vocabulary);
    let task = vyasa_ai::page_designer::user_prompt(
        &vyasa_ai::page_designer::DesignInput {
            vocabulary: &vocabulary,
            site: &site,
            brand: &brand,
            title: &post.title,
            prose: &prose,
            current: &current,
            message: body.message.trim(),
            model: provider.primary_model(),
        },
        &tokens,
    );
    let content_for_eyes =
        serde_json::to_value(&doc).map_err(|e| ApiError(AppError::internal_msg(e.to_string())))?;
    let mut toolbox = vyasa_ai::page_designer::PageToolbox::new(
        &current,
        &tokens,
        live_registry,
        Some(crate::agent_eyes::LivePageEyes {
            state: state.clone(),
            post: post.clone(),
            content: content_for_eyes,
        }),
    );
    let outcome = vyasa_ai::agent::run_agent(
        provider.as_ref(),
        &sink,
        &vyasa_ai::agent::AgentSpec {
            purpose: "page-designer",
            model: provider.primary_model(),
            system: &system,
            task: &task,
            max_steps: 10,
            attachments: Vec::new(),
        },
        &mut toolbox,
    )
    .await
    .map_err(ApiError)?;

    Ok(Json(ComposeResponse {
        reply: outcome.reply,
        sections: serde_json::to_value(&toolbox.sandbox)
            .map_err(|e| ApiError(AppError::internal_msg(e.to_string())))?,
        warnings: crate::theme_studio::diagnostics_json(&toolbox.warnings),
        steps: serde_json::to_value(&outcome.steps)
            .map_err(|e| ApiError(AppError::internal_msg(e.to_string())))?,
    }))
}

/// Request body for rendering an entry the editor has not saved.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct RenderRequest {
    /// The block document as the editor holds it.
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    /// The section tree as the editor holds it.
    #[serde(default)]
    pub sections: Option<serde_json::Value>,
}

/// `POST /api/v1/posts/{id}/render` — the page as it would look if saved.
///
/// Composing without this is done blind: the tree says "hero", and whether
/// that is the right hero is a question only the rendered page answers.
/// The live theme is used, so what comes back is what a visitor would see.
///
/// # Errors
///
/// 400 when the tree is invalid or no theme is active, 403 without
/// `edit_posts` (plus `edit_others` when not the author), 404 when missing,
/// 422 when the theme cannot render it.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/render",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    request_body = RenderRequest,
    responses(
        (status = 200, description = "Rendered page", content_type = "text/html"),
        (status = 400, description = "Invalid sections", body = ApiErrorBody),
        (status = 403, description = "Missing edit_posts / edit_others", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
        (status = 422, description = "The theme cannot render it; the body says why"),
    )
)]
pub async fn render(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<RenderRequest>,
) -> Result<axum::response::Response, ApiError> {
    let post = policy::post_for_edit(&state, &principal, id).await?;

    // Validated, not trusted: the editor is a client like any other, and a
    // tree with an unknown kind would fail deep inside the renderer with a
    // message about templates rather than about the section.
    let stored = post.layout.clone();
    let tree = match body.sections {
        Some(value) => validated_layout(&state, post.post_type, value).await?,
        None => stored.unwrap_or_else(|| serde_json::json!([])),
    };
    let content = body.content.unwrap_or_else(|| post.content.clone());

    match crate::public::routes::render_entry_candidate(
        &state,
        &post,
        vyasa_themes::page_sections(Some(&tree)),
        &content,
    )
    .await
    {
        Ok(html) => Ok(crate::public::routes::preview_response(html)),
        Err(message) => Ok((axum::http::StatusCode::UNPROCESSABLE_ENTITY, message).into_response()),
    }
}

/// `POST /api/v1/posts` — create a post.
///
/// # Errors
///
/// 400 on validation, 403 without `edit_posts`, 401 unauthenticated.
#[utoipa::path(
    post, path = "/api/v1/posts",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    request_body = CreatePostRequest,
    responses(
        (status = 201, description = "Post created", body = PostResponse),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Missing edit_posts", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<CreatePostRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<PostResponse>)> {
    let body_sticky = body.sticky;
    let body_lang = body.lang.clone();
    let body_translation_of = body.translation_of;
    let post_type = parse_post_type(body.post_type.as_ref())?.unwrap_or(PostType::Post);
    // A translation link is for entries the caller can see, of the same
    // type; decided before anything is written.
    if let Some(other) = body_translation_of {
        let other = policy::post_for_view(&state, &principal, other, None).await?;
        same_type_translation(post_type, &other)?;
    }
    let status = parse_status(body.status.as_ref())?.unwrap_or(PostStatus::Draft);
    if matches!(status, PostStatus::Published | PostStatus::Scheduled) {
        principal.ensure(Capability::PublishPosts)?;
    }
    let content = BlockDocument::from_json(body.content).map_err(ApiError)?;
    let layout = match body.layout {
        Some(value) => Some(validated_layout(&state, post_type, value).await?),
        None => None,
    };
    let created = state
        .posts
        .create_with_fields(
            CreatePost {
                post_type,
                status,
                title: body.title,
                slug: body.slug,
                content,
                excerpt: body.excerpt,
                author_id: principal.user().id,
                parent_id: body.parent_id,
                scheduled_for: body.scheduled_for,
                password: body.password,
                term_ids: body.term_ids,
                layout,
            },
            body.fields,
        )
        .await?;
    if body_sticky {
        vyasa_db::repo::PostsRepo::new(state.pool.clone())
            .set_sticky(created.id, true)
            .await?;
    }
    apply_language(&state, created.id, body_lang, body_translation_of).await?;
    crate::post_workflow::after_save(&state, principal.user().id, None, &created).await;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(respond(&state, &principal, created.id).await?),
    ))
}

/// `GET /api/v1/posts` — list posts, newest first.
///
/// # Errors
///
/// 401 unauthenticated; 403 when filter requires extra caps.
#[utoipa::path(
    get, path = "/api/v1/posts",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(ListPostsQuery),
    responses(
        (status = 200, description = "Paged post list", body = PaginatedPosts),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<ListPostsQuery>,
) -> ApiResult<Json<PaginatedPosts>> {
    let status = parse_status(query.status.as_ref()).map_err(ApiError)?;
    let post_type = parse_post_type(query.post_type.as_ref()).map_err(ApiError)?;
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let page = query.page.unwrap_or(1).max(1);
    let offset = (page - 1).saturating_mul(per_page);
    let filter = PostFilter {
        status,
        sort: vyasa_db::repo::PostSort::Newest,
        post_type,
        author_id: query.author_id,
        term_id: query.term_id,
        search: query.search.clone(),
        published_month: None,
        limit: per_page,
        offset,
        sticky_first: false,
        readable_types: policy::readable_custom_types(&state, Some(&principal)).await,
    };
    let repo = vyasa_db::repo::PostsRepo::new(state.pool.clone());
    let policy::PostListScope { all, own_author } = policy::post_list_scope(&principal);
    let items = repo.list_visible(&filter, all, own_author).await?;
    let total = repo.count_visible(&filter, all, own_author).await?;
    let pattern = crate::permalinks::pattern(&state).await;
    let mut defs = crate::entry_fields::Definitions::default();
    let mut responses = Vec::with_capacity(items.len());
    for row in items {
        responses.push(response_with(&state, row, &pattern, &mut defs).await);
    }
    Ok(Json(PaginatedPosts {
        items: responses,
        total,
        page,
        per_page,
    }))
}

/// Query for optional private-post read token.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct PrivateTokenQuery {
    /// Signed read token for private posts (obtained via verify-password).
    pub token: Option<String>,
}

/// `GET /api/v1/posts/{id}` — fetch a single post.
///
/// # Errors
///
/// 401 unauthenticated, 403 when not visible, 404 when missing/trasked.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("id" = i64, Path, description = "Post id"),
        PrivateTokenQuery,
    ),
    responses(
        (status = 200, description = "Post", body = PostResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Not visible", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Query(q): Query<PrivateTokenQuery>,
) -> ApiResult<Json<PostResponse>> {
    let post = policy::post_for_view(&state, &principal, id, q.token.as_deref()).await?;
    Ok(Json(respond(&state, &principal, post.id).await?))
}

/// `GET /api/v1/posts/slug/{type}/{slug}` — fetch by slug.
///
/// # Errors
///
/// 401/403/404 as for `get`.
#[utoipa::path(
    get, path = "/api/v1/posts/slug/{type}/{slug}",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("type" = String, Path, description = "Post type"),
        ("slug" = String, Path, description = "Post slug"),
        PrivateTokenQuery,
    ),
    responses(
        (status = 200, description = "Post", body = PostResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Not visible", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn get_by_slug(
    State(state): State<AppState>,
    principal: Principal,
    Path((post_type, slug)): Path<(String, String)>,
    Query(q): Query<PrivateTokenQuery>,
) -> ApiResult<Json<PostResponse>> {
    let post_type = PostType::parse(&post_type).map_err(ApiError)?;
    let post =
        policy::post_for_view_by_slug(&state, &principal, post_type, &slug, q.token.as_deref())
            .await?;
    Ok(Json(single_response(&state, &principal, post).await))
}

/// `PUT /api/v1/posts/{id}` — update a post.
///
/// # Errors
///
/// 400/403/404/409 as appropriate.
#[utoipa::path(
    put, path = "/api/v1/posts/{id}",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    request_body = UpdatePostRequest,
    responses(
        (status = 200, description = "Updated post", body = PostResponse),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Missing edit_posts / edit_others", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
        (status = 409, description = "Slug taken", body = ApiErrorBody),
    )
)]
#[allow(clippy::too_many_lines)]
pub async fn update(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<UpdatePostRequest>,
) -> ApiResult<Json<PostResponse>> {
    let body_sticky = body.sticky;
    let body_lang = body.lang.clone();
    let body_translation_of = body.translation_of;
    // Fetch first to check ownership.
    let existing = policy::post_for_edit(&state, &principal, id).await?;
    // A translation link is for entries the caller can see, of the same
    // type.
    if let Some(other) = body_translation_of {
        let other = policy::post_for_view(&state, &principal, other, None).await?;
        same_type_translation(existing.post_type, &other)?;
    }
    if let Some(expected) = body.expected_updated_at {
        // Sub-second precision differs between the database and a JSON
        // round trip; compare to the second.
        if existing.updated_at.timestamp() != expected.timestamp() {
            return Err(ApiError(AppError::conflict(
                "This post changed since you opened it, in another tab or by someone else.",
            )));
        }
    }
    let status = match &body.status {
        Some(raw) if !raw.is_empty() => Some(
            PostStatus::parse(raw)
                .map_err(|_| AppError::validation(format!("invalid post status: {raw:?}")))?,
        ),
        _ => None,
    };
    if matches!(status, Some(PostStatus::Published | PostStatus::Scheduled)) {
        principal.ensure(Capability::PublishPosts)?;
    }
    let content = match body.content {
        Some(value) => Some(BlockDocument::from_json(value).map_err(ApiError)?),
        None => None,
    };
    let layout = match body.layout {
        Some(value) => Some(validated_layout(&state, existing.post_type, value).await?),
        None => None,
    };
    let updated = state
        .posts
        .update(
            id,
            UpdatePost {
                meta: body.meta.clone(),
                fields: body.fields.clone(),
                status,
                title: body.title.clone(),
                slug: body.slug.clone(),
                content: content.clone(),
                excerpt: body.excerpt.clone(),
                scheduled_for: body.scheduled_for.flatten(),
                clear_schedule: matches!(body.scheduled_for, Some(None)),
                password: body.password.clone(),
                term_ids: body.term_ids.clone(),
                layout,
            },
        )
        .await?;
    crate::post_workflow::after_save(&state, principal.user().id, Some(&existing), &updated).await;
    if let Some(sticky) = body_sticky {
        vyasa_db::repo::PostsRepo::new(state.pool.clone())
            .set_sticky(id, sticky)
            .await?;
    }
    apply_language(&state, id, body_lang, body_translation_of).await?;
    Ok(Json(respond(&state, &principal, id).await?))
}

/// `DELETE /api/v1/posts/{id}` — trash a post (or hard-delete with `force`).
///
/// # Errors
///
/// 403/404 as appropriate. Use `?force=true` for permanent deletion
/// (requires `delete_posts`).
#[utoipa::path(
    delete, path = "/api/v1/posts/{id}",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("id" = i64, Path, description = "Post id"),
        ("force" = Option<bool>, Query, description = "Hard delete when true"),
    ),
    responses(
        (status = 200, description = "Trashed post", body = PostResponse),
        (status = 204, description = "Hard-deleted"),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn trash(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Query(params): Query<ForceQuery>,
) -> ApiResult<axum::response::Response> {
    let force = params.force.unwrap_or(false);
    policy::post_for_delete(&state, &principal, id, force).await?;
    if force {
        state.posts.delete(id).await?;
        return Ok(axum::http::StatusCode::NO_CONTENT.into_response());
    }
    let trashed = state.posts.trash(id).await?;
    Ok((
        axum::http::StatusCode::OK,
        Json(single_response(&state, &principal, trashed).await),
    )
        .into_response())
}

/// `POST /api/v1/posts/{id}/restore` — restore a trashed post.
///
/// # Errors
///
/// 400 when not trashed, 403/404, 409 when slug taken.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/restore",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 200, description = "Restored post", body = PostResponse),
        (status = 400, description = "Not trashed", body = ApiErrorBody),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
        (status = 409, description = "Slug taken", body = ApiErrorBody),
    )
)]
pub async fn restore(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<PostResponse>> {
    policy::post_for_edit(&state, &principal, id).await?;
    let restored = state.posts.restore(id).await?;
    Ok(Json(single_response(&state, &principal, restored).await))
}

/// Applies `lang` and `translation_of` after a create or update. The
/// caller has already put the `translation_of` entry through
/// `policy::post_for_view`.
/// A translation is the same entry in another language, so it is of the
/// same type: a group mixing types would show one type's readers another
/// type's entries.
///
/// # Errors
/// `Validation` when `other` is of another type.
fn same_type_translation(
    post_type: PostType,
    other: &vyasa_db::content_models::PostRow,
) -> Result<(), AppError> {
    if other.post_type == post_type {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "a translation must be of the same type: this is a {}, \"{}\" is a {}",
            post_type.as_str(),
            other.title,
            other.post_type.as_str()
        )))
    }
}

async fn apply_language(
    state: &AppState,
    id: i64,
    lang: Option<String>,
    translation_of: Option<i64>,
) -> Result<(), AppError> {
    let repo = vyasa_db::repo::PostsRepo::new(state.pool.clone());
    let group = match translation_of {
        Some(other) => {
            let row = state.posts.get(other).await?;
            let g = if let Some(g) = row.translation_group {
                g
            } else {
                // The other entry starts the group with its own id.
                repo.set_language(other, &row.lang, Some(other)).await?;
                other
            };
            Some(g)
        }
        None => None,
    };
    if lang.is_some() || group.is_some() {
        let current = state.posts.get(id).await?;
        let tag = lang.unwrap_or(current.lang);
        if tag.len() > 12 || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(AppError::validation(
                "lang must be a BCP 47 tag like en, pt-BR",
            ));
        }
        repo.set_language(id, &tag, group).await?;
    }
    Ok(())
}

/// The other members of an entry's translation group that `principal`
/// may see (`policy::sees_post`), for the response. Members the caller
/// could not open are left out: their title, slug and status are theirs.
async fn translations_json(
    state: &AppState,
    principal: &Principal,
    row: &vyasa_db::content_models::PostRow,
) -> Vec<serde_json::Value> {
    let Some(group) = row.translation_group else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for m in vyasa_db::repo::PostsRepo::new(state.pool.clone())
        .translations(group)
        .await
        .unwrap_or_default()
    {
        // A member of a type the caller may not read is not theirs to see
        // (a group from before translations had to share a type).
        if m.id == row.id
            || !policy::sees_post(principal, &m)
            || !policy::may_read_type(state, Some(principal), m.post_type).await
        {
            continue;
        }
        out.push(serde_json::json!({ "id": m.id, "lang": m.lang, "slug": m.slug, "title": m.title, "type": m.post_type, "status": m.status }));
    }
    out
}

async fn response_for(state: &AppState, row: vyasa_db::content_models::PostRow) -> PostResponse {
    let pattern = crate::permalinks::pattern(state).await;
    let mut defs = crate::entry_fields::Definitions::default();
    response_with(state, row, &pattern, &mut defs).await
}

/// A response with the public URL and the values of the fields the type
/// still defines.
async fn response_with(
    state: &AppState,
    row: vyasa_db::content_models::PostRow,
    pattern: &vyasa_core::options::PermalinkPattern,
    defs: &mut crate::entry_fields::Definitions,
) -> PostResponse {
    let url = pattern.path_for(&row);
    let values =
        crate::entry_fields::defined_values(defs.of(&state.pool, row.post_type).await, &row.meta);
    let mut response = PostResponse::from(row);
    response.fields = serde_json::Value::Object(values);
    response.public_url = Some(url);
    response
}

/// A single-entry response: as [`response_for`], plus — for a caller who
/// may edit the entry — the reference fields whose target is gone.
async fn single_response(
    state: &AppState,
    principal: &Principal,
    row: vyasa_db::content_models::PostRow,
) -> PostResponse {
    let mut defs = crate::entry_fields::Definitions::default();
    let missing = if policy::may_edit(Some(principal), &row) {
        Some(
            crate::entry_fields::missing_references(
                &state.pool,
                defs.of(&state.pool, row.post_type).await,
                &row.meta,
            )
            .await,
        )
    } else {
        None
    };
    let pattern = crate::permalinks::pattern(state).await;
    let mut response = response_with(state, row, &pattern, &mut defs).await;
    response.fields_missing = missing;
    response
}

/// A response with the translation group filled in, as `principal` sees it.
async fn respond(
    state: &AppState,
    principal: &Principal,
    id: i64,
) -> Result<PostResponse, AppError> {
    let fresh = state.posts.get(id).await?;
    let mut response = single_response(state, principal, fresh.clone()).await;
    response.translations = translations_json(state, principal, &fresh).await;
    Ok(response)
}

/// `POST /api/v1/posts/{id}/duplicate` — a draft copy, "(copy)" in the
/// title, a fresh slug, the same content, terms and layout.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/duplicate",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses((status = 201, description = "The copy", body = PostResponse))
)]
pub async fn duplicate(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<(axum::http::StatusCode, Json<PostResponse>)> {
    let source = policy::post_for_edit(&state, &principal, id).await?;
    let terms = vyasa_db::repo::TermsRepo::new(state.pool.clone())
        .list_for_post(id)
        .await
        .unwrap_or_default();
    let content: vyasa_core::BlockDocument = serde_json::from_value(source.content.clone())
        .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
    // The copy carries the field values too, minus any that no longer
    // pass (a field deleted or tightened, a reference gone): a copy is a
    // draft, so it can be completed before it is published.
    let values = serde_json::Value::Object(crate::entry_fields::stored(&source.meta));
    let (fields, _dropped) = vyasa_core::content::ContentFieldsService::new(state.pool.clone())
        .restorable_values(source.post_type, &values, None)
        .await?;
    let copy = state
        .posts
        .create_with_fields(
            vyasa_core::post::CreatePost {
                post_type: source.post_type,
                status: vyasa_db::content_models::PostStatus::Draft,
                title: format!("{} (copy)", source.title),
                slug: None,
                content,
                excerpt: source.excerpt.clone(),
                author_id: principal.user().id,
                parent_id: source.parent_id,
                scheduled_for: None,
                password: None,
                term_ids: if terms.is_empty() { None } else { Some(terms) },
                layout: source
                    .layout
                    .clone()
                    .and_then(|v| serde_json::from_value(v).ok()),
            },
            Some(serde_json::Value::Object(fields)),
        )
        .await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(response_for(&state, copy).await),
    ))
}

/// One change applied to many posts.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct BatchRequest {
    pub ids: Vec<i64>,
    /// `publish`, `draft`, `trash`, `restore`, `pin`, `unpin`, or
    /// `add_terms` with `term_ids`.
    pub action: String,
    #[serde(default)]
    pub term_ids: Vec<i64>,
}

/// `POST /api/v1/posts/batch` — the same change on every id; ids the
/// caller may not edit are reported, not applied.
#[utoipa::path(
    post, path = "/api/v1/posts/batch",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    request_body = BatchRequest,
    responses((status = 200, description = "What happened", body = serde_json::Value))
)]
pub async fn batch(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<BatchRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.ids.len() > 200 {
        return Err(ApiError(AppError::validation(
            "at most 200 posts at a time",
        )));
    }
    let mut done = 0usize;
    let mut failed: Vec<serde_json::Value> = Vec::new();
    let repo = vyasa_db::repo::PostsRepo::new(state.pool.clone());
    let terms = vyasa_db::repo::TermsRepo::new(state.pool.clone());
    for id in &body.ids {
        let result: Result<(), AppError> = async {
            // The batch reports a refusal in its own words, per id. Every
            // `Forbidden` here means "someone else's entry" only because
            // `EditPosts` is already settled by the route's declared
            // access, which the guard enforces before this handler runs.
            policy::post_for_edit(&state, &principal, *id)
                .await
                .map_err(|ApiError(err)| match err {
                    AppError::Forbidden { .. } => AppError::forbidden("not yours to change"),
                    other => other,
                })?;
            match body.action.as_str() {
                "publish" => {
                    principal.ensure(Capability::PublishPosts)?;
                    state
                        .posts
                        .update(
                            *id,
                            vyasa_core::post::UpdatePost {
                                status: Some(vyasa_db::content_models::PostStatus::Published),
                                ..Default::default()
                            },
                        )
                        .await?;
                }
                "draft" => {
                    state
                        .posts
                        .update(
                            *id,
                            vyasa_core::post::UpdatePost {
                                status: Some(vyasa_db::content_models::PostStatus::Draft),
                                ..Default::default()
                            },
                        )
                        .await?;
                }
                "trash" => {
                    state.posts.trash(*id).await?;
                }
                "restore" => {
                    state.posts.restore(*id).await?;
                }
                "pin" | "unpin" => {
                    repo.set_sticky(*id, body.action == "pin").await?;
                }
                "add_terms" => {
                    let mut have = terms.list_for_post(*id).await.unwrap_or_default();
                    for t in &body.term_ids {
                        if !have.contains(t) {
                            have.push(*t);
                        }
                    }
                    terms.set_post_terms(*id, &have).await?;
                }
                other => return Err(AppError::validation(format!("unknown action {other:?}"))),
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => done += 1,
            Err(e) => failed.push(serde_json::json!({ "id": id, "message": e.to_string() })),
        }
    }
    Ok(Json(serde_json::json!({ "done": done, "failed": failed })))
}

/// Request for verifying a private post password.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct VerifyPasswordRequest {
    /// Post password.
    pub password: String,
}

/// Response with a signed read token.
#[derive(Serialize, utoipa::ToSchema)]
pub struct VerifyPasswordResponse {
    /// Signed token bound to the post.
    pub token: String,
    /// Expiry time.
    pub expires_at: DateTime<Utc>,
}

/// `POST /api/v1/posts/{id}/verify-password` — verify password, get token.
///
/// # Errors
///
/// 401 on wrong password, 404 when not found, 400 when post is not
/// password-protected.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/verify-password",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    request_body = VerifyPasswordRequest,
    responses(
        (status = 200, description = "Verified; token issued", body = VerifyPasswordResponse),
        (status = 400, description = "Post not password-protected", body = ApiErrorBody),
        (status = 401, description = "Wrong password", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn verify_password(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<VerifyPasswordRequest>,
) -> ApiResult<Json<VerifyPasswordResponse>> {
    // Any authenticated user who knows the password can get a token.
    // Fetch to ensure post exists; service verifies hash.
    let token = state
        .posts
        .verify_and_generate_token(id, &body.password, &state.private_token_secret)
        .await?;
    let expires_at = Utc::now() + chrono::Duration::seconds(900);
    // Touch not needed, but ensure principal could at least see the post
    // exists (already fetched via verify). No extra cap check.
    let _ = principal;
    Ok(Json(VerifyPasswordResponse { token, expires_at }))
}

/// Request for autosave.
#[derive(Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub struct AutosaveRequest {
    /// Title.
    pub title: String,
    /// Block document.
    pub content: serde_json::Value,
    /// Optional excerpt.
    pub excerpt: Option<String>,
    /// The editor's field values; omit to record the stored ones. Checked
    /// like an update's.
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub fields: Option<serde_json::Value>,
}

/// The field values a working copy or autosave records: the request's,
/// checked as an update would check them, or the stored ones.
async fn snapshot_fields(
    state: &AppState,
    post: &vyasa_db::content_models::PostRow,
    sent: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, AppError> {
    let stored = post.meta.get("fields");
    match sent {
        None => Ok(stored.cloned()),
        Some(raw) => {
            let service = vyasa_core::content::ContentFieldsService::new(state.pool.clone());
            let checked = service.validate_values(post.post_type, raw, stored).await?;
            // A deleted field's stored values stay with the snapshot too.
            Ok(Some(serde_json::Value::Object(
                service
                    .keep_orphans(post.post_type, checked, stored)
                    .await?,
            )))
        }
    }
}

/// Response for preview token.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PreviewTokenResponse {
    /// Preview URL (path with token).
    pub url: String,
    /// Expiry time.
    pub expires_at: DateTime<Utc>,
}

/// `GET /api/v1/posts/{id}/revisions` — list revisions.
///
/// # Errors
///
/// 401/403/404 as appropriate.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/revisions",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 200, description = "Revision list", body = [PostRevisionResponse]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn list_revisions(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<PostRevisionResponse>>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    let revs = state.revisions.list(post.id, false).await?;
    let mut defs = crate::entry_fields::Definitions::default();
    let defs = defs.of(&state.pool, post.post_type).await;
    Ok(Json(
        revs.into_iter()
            .map(|rev| revision_response(defs, rev))
            .collect(),
    ))
}

/// `GET /api/v1/posts/{id}/revisions/{rid}` — fetch a single revision.
///
/// # Errors
///
/// 401/403/404 as appropriate.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/revisions/{rid}",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("id" = i64, Path, description = "Post id"),
        ("rid" = i64, Path, description = "Revision id"),
    ),
    responses(
        (status = 200, description = "Revision", body = PostRevisionResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn get_revision(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, rid)): Path<(i64, i64)>,
) -> ApiResult<Json<PostRevisionResponse>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    let rev = state.revisions.get(rid).await?;
    if rev.post_id != id {
        return Err(ApiError(AppError::not_found("revision", rid)));
    }
    let mut defs = crate::entry_fields::Definitions::default();
    Ok(Json(revision_response(
        defs.of(&state.pool, post.post_type).await,
        rev,
    )))
}

/// `POST /api/v1/posts/{id}/revisions/{rid}/restore` — restore a revision.
///
/// # Errors
///
/// 401/403/404 as appropriate.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/revisions/{rid}/restore",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("id" = i64, Path, description = "Post id"),
        ("rid" = i64, Path, description = "Revision id"),
    ),
    responses(
        (status = 200, description = "Restored post", body = PostResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn restore_revision(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, rid)): Path<(i64, i64)>,
) -> ApiResult<Json<PostResponse>> {
    policy::post_for_edit(&state, &principal, id).await?;
    // The revision must be one of this post's: permission was checked
    // against `id`, so a revision of another post is a 404, not a write.
    let restored = state
        .revisions
        .restore_reporting(id, rid, principal.user().id)
        .await?;
    let mut response = single_response(&state, &principal, restored.post).await;
    response.fields_dropped = restored.dropped_fields;
    Ok(Json(response))
}

/// Request for saving a working copy.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SaveRevisionRequest {
    /// Title.
    pub title: String,
    /// Block document.
    pub content: serde_json::Value,
    /// The editor's field values; omit to record the stored ones. Checked
    /// like an update's.
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    #[serde(default)]
    pub fields: Option<serde_json::Value>,
}

/// `POST /api/v1/posts/{id}/revisions` — save a working copy.
///
/// Records the given title and content as a revision **without touching the
/// post**, so what is live stays live. This is what "Save" means on a
/// published post: keep my work, publish nothing. "Update" is a normal
/// `PUT /posts/{id}` afterwards. Identical content to the latest revision is
/// not recorded twice; the latest manual revision is returned either way.
/// An autosave of the same text does not count — it is a different promise.
///
/// # Errors
///
/// 400 on invalid content; 401/403/404 as appropriate.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/revisions",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    request_body = SaveRevisionRequest,
    responses(
        (status = 200, description = "Working copy saved (or unchanged)", body = PostRevisionResponse),
        (status = 400, description = "Invalid content", body = ApiErrorBody),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn save_revision(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<SaveRevisionRequest>,
) -> ApiResult<Json<PostRevisionResponse>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    let content = BlockDocument::from_json(body.content).map_err(ApiError)?;
    content.validate().map_err(ApiError)?;
    let json = content.to_json();
    let fields = snapshot_fields(&state, &post, body.fields.as_ref()).await?;
    let rev = state
        .revisions
        .save_working_copy(
            id,
            principal.user().id,
            &vyasa_core::post::Snapshot {
                title: &body.title,
                content: &json,
                // The editor sends words, not arrangement. Carrying the
                // page's stored sections is what stops restoring a working
                // copy from quietly flattening a composed page.
                layout: post.layout.as_ref(),
                // Field values: the editor's when it sends them, the
                // stored ones otherwise.
                fields: fields.as_ref(),
            },
        )
        .await?;
    let mut defs = crate::entry_fields::Definitions::default();
    Ok(Json(revision_response(
        defs.of(&state.pool, post.post_type).await,
        rev,
    )))
}

/// `PUT /api/v1/posts/{id}/autosave` — autosave (throttled, one per user).
///
/// # Errors
///
/// 401/403/404 as appropriate.
#[utoipa::path(
    put, path = "/api/v1/posts/{id}/autosave",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    request_body = AutosaveRequest,
    responses(
        (status = 200, description = "Autosaved", body = PostRevisionResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn autosave(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<AutosaveRequest>,
) -> ApiResult<Json<PostRevisionResponse>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    let content = BlockDocument::from_json(body.content).map_err(ApiError)?;
    content.validate().map_err(ApiError)?;
    let json = content.to_json();
    let fields = snapshot_fields(&state, &post, body.fields.as_ref()).await?;
    let rev = state
        .revisions
        .autosave(
            id,
            principal.user().id,
            &vyasa_core::post::Snapshot {
                title: &body.title,
                content: &json,
                layout: post.layout.as_ref(),
                fields: fields.as_ref(),
            },
        )
        .await?;
    let mut defs = crate::entry_fields::Definitions::default();
    Ok(Json(revision_response(
        defs.of(&state.pool, post.post_type).await,
        rev,
    )))
}

/// `POST /api/v1/posts/{id}/preview-token` — generate preview token.
///
/// # Errors
///
/// 401/403/404 as appropriate.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/preview-token",
    tag = "posts",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 200, description = "Preview token", body = PreviewTokenResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn preview_token(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<PreviewTokenResponse>> {
    let post = policy::post_for_edit(&state, &principal, id).await?;
    let token = state
        .revisions
        .generate_preview_token(post.id, &state.private_token_secret)
        .map_err(ApiError)?;
    let expires_at = Utc::now() + chrono::Duration::seconds(3600);
    let url = format!("/api/v1/posts/{id}/preview?token={token}");
    Ok(Json(PreviewTokenResponse { url, expires_at }))
}

/// `GET /api/v1/posts/{id}/preview` — public preview with token.
///
/// # Errors
///
/// 401 when token invalid/expired, 404 when not found.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/preview",
    tag = "posts",
    params(
        ("id" = i64, Path, description = "Post id"),
        ("token" = String, Query, description = "Preview token"),
    ),
    responses(
        (status = 200, description = "Post preview", body = PostResponse),
        (status = 401, description = "Invalid token", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<PrivateTokenQuery>,
) -> ApiResult<Json<PostResponse>> {
    let token = q
        .token
        .ok_or_else(|| ApiError(AppError::auth("preview token required")))?;
    state
        .revisions
        .verify_preview_token(id, &token, &state.private_token_secret)
        .map_err(ApiError)?;
    let post = state.posts.get(id).await?;
    Ok(Json(response_for(&state, post).await))
}

/// Response shape for a revision.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PostRevisionResponse {
    /// Revision id.
    pub id: i64,
    /// Post id.
    pub post_id: i64,
    /// Snapshot title.
    pub title: String,
    /// Snapshot content.
    pub content: serde_json::Value,
    /// Author id.
    pub author_id: i64,
    /// Whether this is an autosave.
    pub is_autosave: bool,
    /// Field values recorded with it (`null` for a revision older than
    /// fields).
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub fields: Option<serde_json::Value>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// A revision as served: its field values limited to the fields the type
/// still defines, as an entry's are. What a deleted field left behind
/// stays stored (and listed by `…/orphans`) but is not offered back.
fn revision_response(
    defs: &[vyasa_core::content::FieldDef],
    row: vyasa_db::content_models::PostRevisionRow,
) -> PostRevisionResponse {
    let fields = row.fields.as_ref().map(|stored| {
        let meta = serde_json::json!({ "fields": stored });
        serde_json::Value::Object(crate::entry_fields::defined_values(defs, &meta))
    });
    let mut response = PostRevisionResponse::from(row);
    response.fields = fields;
    response
}

impl From<vyasa_db::content_models::PostRevisionRow> for PostRevisionResponse {
    fn from(row: vyasa_db::content_models::PostRevisionRow) -> Self {
        Self {
            id: row.id,
            post_id: row.post_id,
            title: row.title,
            content: row.content,
            author_id: row.author_id,
            is_autosave: row.is_autosave,
            fields: row.fields,
            created_at: row.created_at,
        }
    }
}

/// Query for `force` hard delete.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct ForceQuery {
    /// Hard delete when true.
    pub force: Option<bool>,
}

/// Distinguishes a field that was omitted from one sent as `null`.
///
/// With a plain `Option` both arrive as `None`, so "leave the schedule
/// alone" and "remove the schedule" were the same request.
#[allow(clippy::option_option)]
fn double_option<'de, D>(d: D) -> Result<Option<Option<DateTime<Utc>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(d).map(Some)
}
