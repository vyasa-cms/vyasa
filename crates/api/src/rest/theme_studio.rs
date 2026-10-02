//! Theme studio REST: working copies, typed edits, history, previews and
//! publishing. Every route needs `ManageThemes`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use vyasa_common::AppError;
use vyasa_db::repo::ThemeDraftsRepo;
use vyasa_themes::StudioOp;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::rest::themes::ThemeResponse;
use crate::state::AppState;
use crate::theme_studio::{self, draft_json, draft_summary_json, Start};

fn drafts(state: &AppState) -> ThemeDraftsRepo {
    ThemeDraftsRepo::new(state.pool.clone())
}

/// `GET /api/v1/themes/vocabulary` — what a theme can be made of: static
/// regions, dynamic blocks with their settings schemas, template names
/// with their built-in sources, the token schema, and the live content
/// sources a binding may draw from. Generated from the registry, so the
/// editor and the validator never disagree.
#[utoipa::path(
    get, path = "/api/v1/themes/vocabulary", tag = "themes",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Blocks, templates, sources and token schema", body = serde_json::Value))
)]
pub async fn vocabulary(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(
        crate::rest::content_types::vocabulary_with_sources(&state).await?,
    ))
}

/// `GET /api/v1/themes/drafts` — working copies, most recent first.
#[utoipa::path(
    get, path = "/api/v1/themes/drafts", tag = "themes",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Drafts", body = serde_json::Value))
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows = drafts(&state).list().await?;
    Ok(Json(Value::Array(
        rows.iter().map(draft_summary_json).collect(),
    )))
}

/// Body for starting a draft.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateDraftRequest {
    /// Working title; defaults to the base theme's name.
    #[serde(default)]
    pub name: Option<String>,
    /// Installed theme version to copy; defaults to the live theme.
    ///
    /// Accepts a number or a string: a browser cannot round-trip a 64-bit
    /// id as a JSON number, so the admin sends ids as strings.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub base_theme_id: Option<i64>,
}

/// `POST /api/v1/themes/drafts` — start a working copy from an installed
/// theme (the live one by default).
#[utoipa::path(
    post, path = "/api/v1/themes/drafts", tag = "themes",
    security(("session_cookie" = [])),
    request_body(content = CreateDraftRequest),
    responses(
        (status = 201, description = "Draft", body = serde_json::Value),
        (status = 404, description = "Unknown base theme", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<CreateDraftRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let start = body.base_theme_id.map_or(Start::Active, Start::Theme);
    let row = theme_studio::create_draft(
        &state,
        body.name.as_deref(),
        start,
        Some(principal.user().id),
    )
    .await?;
    let warnings = theme_studio::current_warnings(&row);
    Ok((StatusCode::CREATED, Json(draft_json(&row, &warnings))))
}

/// `GET /api/v1/themes/drafts/{id}` — the full draft: documents, status,
/// current warnings.
#[utoipa::path(
    get, path = "/api/v1/themes/drafts/{id}", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    responses(
        (status = 200, description = "Draft", body = serde_json::Value),
        (status = 404, description = "Unknown draft", body = ApiErrorBody),
    )
)]
pub async fn get(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let row = drafts(&state).get(id).await?;
    let warnings = theme_studio::current_warnings(&row);
    Ok(Json(draft_json(&row, &warnings)))
}

/// Body for renaming a draft.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct RenameRequest {
    /// New working title.
    pub name: String,
}

/// `PATCH /api/v1/themes/drafts/{id}` — rename.
#[utoipa::path(
    patch, path = "/api/v1/themes/drafts/{id}", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    request_body(content = RenameRequest),
    responses((status = 200, description = "Draft", body = serde_json::Value))
)]
pub async fn rename(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<RenameRequest>,
) -> ApiResult<Json<Value>> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError(AppError::validation(
            "name must be 1-80 characters",
        )));
    }
    let row = drafts(&state).rename(id, name).await?;
    let warnings = theme_studio::current_warnings(&row);
    Ok(Json(draft_json(&row, &warnings)))
}

/// `DELETE /api/v1/themes/drafts/{id}` — discard a draft with its history.
#[utoipa::path(
    delete, path = "/api/v1/themes/drafts/{id}", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    responses((status = 204, description = "Deleted"), (status = 404, description = "Unknown draft", body = ApiErrorBody))
)]
pub async fn delete(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    drafts(&state).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A batch of edits.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct OpsRequest {
    /// Operations, applied in order and validated as a whole.
    #[schema(value_type = Vec<Object>)]
    pub ops: Vec<StudioOp>,
    /// Optional note for the revision; defaults to a description of the
    /// changes.
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /api/v1/themes/drafts/{id}/ops` — apply edits as one revision.
///
/// 400 carries every diagnostic (one per line, `error: <path>: <why>`)
/// and leaves the draft untouched. 200 returns the draft with any
/// non-blocking warnings (contrast, incomplete dark palette).
#[utoipa::path(
    post, path = "/api/v1/themes/drafts/{id}/ops", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    request_body(content = OpsRequest),
    responses(
        (status = 200, description = "Draft after the edits", body = serde_json::Value),
        (status = 400, description = "Edits rejected by validation", body = ApiErrorBody),
        (status = 404, description = "Unknown draft", body = ApiErrorBody),
    )
)]
pub async fn apply(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<OpsRequest>,
) -> ApiResult<Json<Value>> {
    if body.ops.is_empty() {
        return Err(ApiError(AppError::validation("ops must not be empty")));
    }
    let (row, warnings) =
        theme_studio::apply_ops(&state, id, &body.ops, "you", body.note.as_deref()).await?;
    Ok(Json(draft_json(&row, &warnings)))
}

/// `GET /api/v1/themes/drafts/{id}/revisions` — history, newest first.
/// Documents are left out; fetch one revision for those.
#[utoipa::path(
    get, path = "/api/v1/themes/drafts/{id}/revisions", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    responses((status = 200, description = "Revisions", body = serde_json::Value))
)]
pub async fn revisions(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let rows = drafts(&state).revisions(id).await?;
    Ok(Json(Value::Array(
        rows.iter()
            .map(|r| {
                json!({
                    "seq": r.seq,
                    "note": r.note,
                    "source": r.source,
                    "created_at": r.created_at,
                })
            })
            .collect(),
    )))
}

/// `GET /api/v1/themes/drafts/{id}/revisions/{seq}` — one revision with
/// its documents, for comparing or previewing an earlier state.
#[utoipa::path(
    get, path = "/api/v1/themes/drafts/{id}/revisions/{seq}", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id"), ("seq" = i32, Path, description = "Revision sequence")),
    responses((status = 200, description = "Revision", body = serde_json::Value), (status = 404, description = "Unknown revision", body = ApiErrorBody))
)]
pub async fn revision(
    State(state): State<AppState>,
    Path((id, seq)): Path<(i64, i32)>,
) -> ApiResult<Json<Value>> {
    let r = drafts(&state).revision(id, seq).await?;
    Ok(Json(json!({
        "seq": r.seq,
        "note": r.note,
        "source": r.source,
        "created_at": r.created_at,
        "tokens": r.tokens,
        "layout": r.layout,
        "templates": r.templates.unwrap_or_else(|| json!({})),
        "assets": r.assets,
    })))
}

/// Body for going back in history.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct RevertRequest {
    /// Revision to make current again.
    pub seq: i32,
}

/// `POST /api/v1/themes/drafts/{id}/revert` — make an earlier revision
/// current. Recorded as a new revision, so it can itself be undone.
#[utoipa::path(
    post, path = "/api/v1/themes/drafts/{id}/revert", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    request_body(content = RevertRequest),
    responses((status = 200, description = "Draft", body = serde_json::Value), (status = 404, description = "Unknown revision", body = ApiErrorBody))
)]
pub async fn revert(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<RevertRequest>,
) -> ApiResult<Json<Value>> {
    let row = theme_studio::revert(&state, id, body.seq).await?;
    let warnings = theme_studio::current_warnings(&row);
    Ok(Json(draft_json(&row, &warnings)))
}

/// Body for publishing.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct PublishRequest {
    /// Theme package name (`[a-z0-9-]{1,60}`); publishes as its next version.
    pub name: String,
    /// Make it live straight away.
    #[serde(default)]
    pub activate: bool,
}

/// `POST /api/v1/themes/drafts/{id}/publish` — install the draft as the
/// next version of `name`, optionally activating it. The draft stays.
#[utoipa::path(
    post, path = "/api/v1/themes/drafts/{id}/publish", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    request_body(content = PublishRequest),
    responses(
        (status = 201, description = "Installed theme version", body = ThemeResponse),
        (status = 400, description = "Bad name or invalid draft", body = ApiErrorBody),
    )
)]
pub async fn publish(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<PublishRequest>,
) -> ApiResult<(StatusCode, Json<ThemeResponse>)> {
    let theme = theme_studio::publish(&state, id, body.name.trim(), body.activate).await?;
    crate::audit::record(
        &state,
        principal.user(),
        if body.activate {
            "theme.publish_activate"
        } else {
            "theme.publish"
        },
        format!("theme:{}", theme.name),
        json!({ "version": theme.version, "draft_id": id }),
    );
    let mut out = ThemeResponse::from(theme);
    out.is_active = body.activate;
    Ok((StatusCode::CREATED, Json(out)))
}

/// Which page to preview.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct PreviewQuery {
    /// Site path such as `/`, `/post/hello`, `/category/news`,
    /// `/search?s=rust`. Defaults to the home page.
    #[serde(default)]
    pub path: Option<String>,
}

/// `GET /api/v1/themes/drafts/{id}/preview?path=` — the draft rendered
/// over real published content. Never cached.
#[utoipa::path(
    get, path = "/api/v1/themes/drafts/{id}/preview", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id"), PreviewQuery),
    responses(
        (status = 200, description = "HTML page", content_type = "text/html"),
        (status = 422, description = "The draft cannot render; the body says why"),
    )
)]
pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<PreviewQuery>,
) -> Result<Response, ApiError> {
    let row = drafts(&state).get(id).await?;
    let path = q.path.as_deref().unwrap_or("/");
    Ok(render_or_explain(
        &state,
        &row.tokens,
        &row.layout,
        row.templates.as_ref(),
        row.assets.as_ref(),
        path,
        row.base_theme_id,
    )
    .await)
}

/// Documents to render without saving.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CandidateRequest {
    /// Installed version supplying bundled images and fonts.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub base_theme_id: Option<i64>,
    /// The theme's own CSS and JS, so an unsaved edit previews with it.
    #[serde(default)]
    pub assets: Option<Value>,
    /// Design tokens.
    pub tokens: Value,
    /// Layout composition.
    pub layout: Value,
    /// Tera overrides keyed by template name.
    #[serde(default)]
    pub templates: Option<Value>,
    /// Site path to render; defaults to the home page.
    #[serde(default)]
    pub path: Option<String>,
}

/// `POST /api/v1/themes/preview` — render documents the studio has not
/// saved yet, so a colour being dragged shows up without a revision per
/// frame. Never cached.
#[utoipa::path(
    post, path = "/api/v1/themes/preview", tag = "themes",
    security(("session_cookie" = [])),
    request_body(content = CandidateRequest),
    responses(
        (status = 200, description = "HTML page", content_type = "text/html"),
        (status = 422, description = "The documents cannot render; the body says why"),
    )
)]
pub async fn candidate(
    State(state): State<AppState>,
    Json(body): Json<CandidateRequest>,
) -> Result<Response, ApiError> {
    let path = body.path.as_deref().unwrap_or("/");
    Ok(render_or_explain(
        &state,
        &body.tokens,
        &body.layout,
        body.templates.as_ref(),
        body.assets.as_ref(),
        path,
        body.base_theme_id,
    )
    .await)
}

async fn render_or_explain(
    state: &AppState,
    tokens: &Value,
    layout: &Value,
    templates: Option<&Value>,
    assets: Option<&Value>,
    path: &str,
    base_theme_id: Option<i64>,
) -> Response {
    match crate::public::routes::render_candidate(
        state,
        tokens,
        layout,
        templates,
        assets,
        path,
        base_theme_id,
    )
    .await
    {
        Ok(html) => crate::public::routes::preview_response(html),
        Err(message) => (StatusCode::UNPROCESSABLE_ENTITY, message).into_response(),
    }
}

/// `GET /api/v1/themes/drafts/{id}/messages` — the conversation with the
/// assistant, oldest first.
#[utoipa::path(
    get, path = "/api/v1/themes/drafts/{id}/messages", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    responses((status = 200, description = "Messages", body = serde_json::Value))
)]
pub async fn messages(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let rows = drafts(&state).messages(id).await?;
    Ok(Json(Value::Array(
        rows.iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "role": m.role,
                    "text": m.text,
                    "revision": m.revision,
                    // What it would change, not the documents themselves —
                    // the conversation is polled while a run is in flight
                    // and three whole documents per turn is a lot of wire.
                    "proposal": m.proposal.as_ref().map(|p| json!({
                        "changes": p.get("changes").cloned().unwrap_or(Value::Null),
                        "base": m.proposal_base,
                        // The agent's work log, so the panel can show how
                        // the proposal came to be.
                        "steps": p.get("steps").cloned().unwrap_or(Value::Null),
                    })),
                    "created_at": m.created_at,
                })
            })
            .collect(),
    )))
}

/// `POST /api/v1/themes/drafts/{id}/messages/{mid}/accept` — write the
/// documents a reply proposed.
///
/// The assistant used to commit as it answered, so a change nobody wanted
/// still landed and had to be undone. Accepting is the only thing that
/// writes; declining is simply never pressing it, and the proposal stays
/// on the message until the draft moves past it.
///
/// # Errors
///
/// 404 when the message is not this draft's, 409 when the draft has
/// changed since the proposal was composed, 422 when the reply carried
/// nothing to apply.
#[utoipa::path(
    post, path = "/api/v1/themes/drafts/{id}/messages/{mid}/accept", tag = "themes",
    security(("session_cookie" = [])),
    params(
        ("id" = i64, Path, description = "Draft id"),
        ("mid" = i64, Path, description = "Message id"),
    ),
    responses(
        (status = 200, description = "The draft, with the proposal applied", body = serde_json::Value),
        (status = 404, description = "No such message", body = ApiErrorBody),
        (status = 409, description = "The draft moved on", body = ApiErrorBody),
        (status = 422, description = "Nothing to apply", body = ApiErrorBody),
    )
)]
pub async fn accept_proposal(
    State(state): State<AppState>,
    Path((id, mid)): Path<(i64, i64)>,
) -> ApiResult<Json<Value>> {
    let repo = drafts(&state);
    let message = repo.message(id, mid).await?;
    let Some(proposal) = message.proposal else {
        return Err(ApiError(AppError::validation(
            "that reply had nothing to apply",
        )));
    };
    let draft = repo.get(id).await?;
    // Accepting a proposal composed against an older draft would discard
    // whatever was done in between without saying so.
    if message.proposal_base != Some(draft.revision) {
        return Err(ApiError(AppError::conflict(
            "the draft changed after this was proposed — ask again",
        )));
    }

    let field = |key: &str| proposal.get(key).cloned().unwrap_or(Value::Null);
    let tokens = field("tokens");
    let layout = field("layout");
    let templates = field("templates");
    // Older proposals have no assets field: accepting them must retain the
    // current stylesheet. Explicit null from a new proposal clears it.
    let assets = proposal
        .get("assets")
        .cloned()
        .unwrap_or_else(|| draft.assets.clone().unwrap_or(Value::Null));
    vyasa_themes::DraftState::from_json_with_assets(
        &tokens,
        &layout,
        Some(&templates),
        (!assets.is_null()).then_some(&assets),
    )
    .map_err(|e| ApiError(AppError::validation(e)))?;
    // Staged navigation, validated in full BEFORE anything is written:
    // a proposal that cannot land whole does not land half.
    let menus: Vec<vyasa_core::menu::MenuDraft> = proposal
        .get("menus")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|e| {
            ApiError(AppError::validation(format!(
                "staged menus unreadable: {e}"
            )))
        })?
        .unwrap_or_default();
    for draft_menu in &menus {
        let problems = draft_menu.validate();
        if !problems.is_empty() {
            return Err(ApiError(AppError::validation(format!(
                "staged menu \"{}\": {}",
                draft_menu.slug,
                problems.join("; ")
            ))));
        }
    }
    let changes = proposal
        .get("changes")
        .and_then(Value::as_array)
        .map_or_else(
            || "assistant changes".to_owned(),
            |list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            },
        );

    let row = repo
        .commit_revision(
            id,
            vyasa_db::repo::DraftDocuments {
                tokens: &tokens,
                layout: &layout,
                templates: (!templates.is_null()).then_some(&templates),
                assets: (!assets.is_null()).then_some(&assets),
            },
            &changes,
            "assistant",
        )
        .await?;
    // Menus are site data, not draft documents: they land through the
    // same service the Menus page uses, only on accept.
    let menu_service = vyasa_core::menu::MenuService::new(state.menus.clone());
    for draft_menu in &menus {
        menu_service.apply_draft(draft_menu).await?;
    }
    repo.settle_proposal(mid, Some(row.revision)).await?;
    Ok(Json(crate::theme_studio::draft_json(
        &row,
        &crate::theme_studio::current_warnings(&row),
    )))
}

/// A request to the assistant.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ChatRequest {
    /// What to change, or a question about the draft.
    pub message: String,
    /// Images from the media library to show a vision model (a
    /// moodboard, a screenshot of a site you like). Up to four.
    ///
    /// Numbers or strings, like every other id on this API.
    #[serde(default, deserialize_with = "crate::flexible_id::option_vec")]
    pub media_ids: Option<Vec<i64>>,
}

/// `POST /api/v1/themes/drafts/{id}/chat` — ask the assistant. Records
/// the message, marks the draft `generating` and queues the run; poll
/// the draft and its messages for the outcome. 202 on queue.
///
/// 400 when no text (or vision, with images) model is registered.
#[utoipa::path(
    post, path = "/api/v1/themes/drafts/{id}/chat", tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Draft id")),
    request_body(content = ChatRequest),
    responses(
        (status = 202, description = "Queued", body = serde_json::Value),
        (status = 400, description = "Empty message or no model", body = ApiErrorBody),
        (status = 409, description = "A run is still in progress", body = ApiErrorBody),
    )
)]
pub async fn chat(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<ChatRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let media_ids = body.media_ids.unwrap_or_default();
    let message_id = crate::theme_assistant::ask(&state, id, &body.message, &media_ids).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "message_id": message_id, "status": "generating" })),
    ))
}
