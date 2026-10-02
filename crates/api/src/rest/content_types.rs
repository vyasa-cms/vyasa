//! What kinds of content this site holds — the sources a binding may name.
//!
//! One list, three consumers: the studio's Data panel, the binding editor
//! in the inspector, and both AI assistants (whose vocabulary carries it as
//! `sources`). It is assembled at request time from the built-in types plus
//! whatever enabled plugins have registered, so it is never a copy that can
//! drift from `register-post-types`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::content::{
    ContentFieldsService, ContentTypesService, FieldChanges, FieldKind, NewField, NewType,
    TypeChanges,
};
use vyasa_db::content_models::{PostType, TypeOwner};

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::state::AppState;

/// One content type as the designer sees it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ContentType {
    /// The `posts.type` value and URL segment.
    pub slug: String,
    /// Singular label ("Product").
    pub singular: String,
    /// Plural label ("Products").
    pub plural: String,
    /// Whether entries get public URLs.
    pub public: bool,
    /// Whether `/{slug}` lists entries.
    pub has_archive: bool,
    /// True when a plugin registered it (and it goes away with the plugin).
    pub plugin: bool,
    /// Who owns it: `builtin`, `plugin` or `admin` (only `admin` types
    /// can be relabelled or deleted here).
    pub owner: String,
    /// What the type is for (administrators' types; empty otherwise).
    pub description: String,
    /// Published entries right now.
    pub count: i64,
}

/// Every source a binding may draw from, built-ins first.
///
/// # Errors
/// Database failure counting entries.
pub async fn list_types(state: &AppState) -> Result<Vec<ContentType>, vyasa_common::AppError> {
    let counts: std::collections::HashMap<String, i64> =
        vyasa_db::repo::PostsRepo::new(state.pool.clone())
            .count_by_type()
            .await?
            .into_iter()
            .collect();
    let n = |slug: &str| counts.get(slug).copied().unwrap_or(0);

    let mut out = vec![
        ContentType {
            slug: "post".into(),
            singular: "Post".into(),
            plural: "Posts".into(),
            public: true,
            has_archive: true,
            plugin: false,
            owner: "builtin".into(),
            description: String::new(),
            count: n("post"),
        },
        ContentType {
            slug: "page".into(),
            singular: "Page".into(),
            plural: "Pages".into(),
            public: true,
            has_archive: false,
            plugin: false,
            owner: "builtin".into(),
            description: String::new(),
            count: n("page"),
        },
    ];
    for decl in state.plugin_surface.post_types().await {
        out.push(ContentType {
            count: n(&decl.slug),
            slug: decl.slug,
            singular: decl.singular,
            plural: decl.plural,
            public: decl.public,
            has_archive: decl.has_archive,
            plugin: true,
            owner: "plugin".into(),
            description: String::new(),
        });
    }
    // Administrators' types, live ones only: a stored type the registry
    // could not take at boot is not a source anything can draw from.
    for row in ContentTypesService::new(state.pool.clone()).list().await? {
        if PostType::owner(&row.slug) != Some(TypeOwner::Admin) {
            continue;
        }
        out.push(ContentType {
            count: n(&row.slug),
            slug: row.slug,
            singular: row.singular,
            plural: row.plural,
            public: row.public,
            has_archive: row.has_archive,
            plugin: false,
            owner: "admin".into(),
            description: row.description,
        });
    }
    Ok(out)
}

/// The theme vocabulary with the live site grafted on: content sources for
/// bindings, and the per-type template names a theme could override.
///
/// Every consumer of the vocabulary — the studio UI, the theme assistant,
/// the page designer — goes through here, so "add a grid of the newest
/// products" works the day the products plugin is enabled, with no prompt
/// edits anywhere.
///
/// # Errors
/// Database failure while listing types.
pub async fn vocabulary_with_sources(
    state: &AppState,
) -> Result<serde_json::Value, vyasa_common::AppError> {
    let registry = crate::plugin_sections::registry_with_plugins(state).await;
    let mut vocab = vyasa_themes::registry_schema_for(&registry);
    // Tag each plugin-owned kind with its plugin, so the studio can show
    // where a section comes from and what turns it off.
    let owners: std::collections::HashMap<String, String> = state
        .plugin_surface
        .sections()
        .await
        .into_iter()
        .map(|d| (d.kind, d.plugin_name))
        .collect();
    if let Some(blocks) = vocab
        .get_mut("blocks")
        .and_then(serde_json::Value::as_array_mut)
    {
        for block in blocks.iter_mut() {
            let Some(kind) = block.get("kind").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if let Some(owner) = owners.get(kind) {
                if let Some(obj) = block.as_object_mut() {
                    obj.insert("plugin".to_owned(), serde_json::json!(owner));
                }
            }
        }
    }
    let types = list_types(state).await?;
    let type_templates: Vec<String> = types
        .iter()
        .filter(|t| t.owner != "builtin" && t.public)
        .flat_map(|t| {
            [
                format!("single-{}.html", t.slug),
                format!("archive-{}.html", t.slug),
            ]
        })
        .collect();
    // Each source's custom fields, so a binding can be sorted or filtered
    // by one (`sort: "field:<key>"`, `where: {<key>: value}`) and a
    // template can print `entry.fields.<key>`.
    let fields = ContentFieldsService::new(state.pool.clone());
    let mut source_fields = serde_json::Map::new();
    for t in &types {
        let rows = fields.list(&t.slug).await.unwrap_or_default();
        if !rows.is_empty() {
            source_fields.insert(
                t.slug.clone(),
                rows.into_iter()
                    .map(|f| serde_json::json!({"key": f.key, "label": f.label, "kind": f.kind}))
                    .collect(),
            );
        }
    }
    if let Some(obj) = vocab.as_object_mut() {
        obj.insert(
            "sources".to_owned(),
            serde_json::to_value(&types).unwrap_or_default(),
        );
        obj.insert(
            "source_fields".to_owned(),
            serde_json::Value::Object(source_fields),
        );
        obj.insert(
            "type_templates".to_owned(),
            serde_json::json!(type_templates),
        );
    }
    Ok(vocab)
}

/// `GET /api/v1/content-types` — the sources a binding may draw from.
#[utoipa::path(
    get, path = "/api/v1/content-types", tag = "content",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Content types, built-ins first", body = [ContentType]))
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<ContentType>>> {
    Ok(Json(list_types(&state).await?))
}

/// An administrator's content type as stored.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AdminType {
    /// The `posts.type` value and URL segment; immutable.
    pub slug: String,
    /// Singular label.
    pub singular: String,
    /// Plural label.
    pub plural: String,
    /// What the type is for.
    pub description: String,
    /// Whether entries get public URLs.
    pub public: bool,
    /// Whether `/{slug}` lists the entries.
    pub has_archive: bool,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last change.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<vyasa_db::repo::ContentTypeRow> for AdminType {
    fn from(r: vyasa_db::repo::ContentTypeRow) -> Self {
        Self {
            slug: r.slug,
            singular: r.singular,
            plural: r.plural,
            description: r.description,
            public: r.public,
            has_archive: r.has_archive,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Body of `POST /content-types`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTypeRequest {
    /// `^[a-z][a-z0-9-]*[a-z0-9]$`, 2 to 32 characters, not reserved.
    pub slug: String,
    /// Singular label, 1 to 120 characters.
    pub singular: String,
    /// Plural label, 1 to 120 characters.
    pub plural: String,
    /// Up to 500 characters.
    #[serde(default)]
    pub description: String,
    /// Whether entries get public URLs (default true).
    #[serde(default = "yes")]
    pub public: bool,
    /// Whether `/{slug}` lists the entries (default true).
    #[serde(default = "yes")]
    pub has_archive: bool,
}

const fn yes() -> bool {
    true
}

/// Body of `PUT /content-types/{slug}`: what to change. The slug cannot
/// change; sending a different one is refused.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTypeRequest {
    /// Must equal the path's slug when sent.
    pub slug: Option<String>,
    /// New singular label.
    pub singular: Option<String>,
    /// New plural label.
    pub plural: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New public flag.
    pub public: Option<bool>,
    /// New archive flag.
    pub has_archive: Option<bool>,
}

/// Refuses a slug that is not an administrator's type, saying whose it
/// is, before the service answers a bare 404.
fn admin_only(slug: &str) -> Result<(), ApiError> {
    match slug {
        "post" | "page" | "block" => Err(ApiError(AppError::validation(format!(
            "{slug:?} is a built-in type; it cannot be changed here"
        )))),
        _ if PostType::owner(slug) == Some(TypeOwner::Plugin) => {
            Err(ApiError(AppError::validation(format!(
                "{slug:?} belongs to a plugin; it changes with the plugin"
            ))))
        }
        _ => Ok(()),
    }
}

/// The slugs the enabled plugins declare, which a new type may not take.
async fn plugin_slugs(state: &AppState) -> Vec<String> {
    state
        .plugin_surface
        .post_types()
        .await
        .into_iter()
        .map(|d| d.slug)
        .collect()
}

/// Rendered pages list types and their archives, so a change to types
/// makes every cached page stale.
fn pages_changed(state: &AppState) {
    state
        .surface_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Re-indexes a type's entries after a change to what search may show of
/// them. A failure is logged, not returned: the change itself is made.
async fn reindex(state: &AppState, slug: &str) {
    let Ok(post_type) = PostType::parse(slug) else {
        return;
    };
    if let Err(e) = crate::search_indexer::reindex_type(state, post_type).await {
        tracing::warn!(
            slug,
            "search reindex after a content type change failed: {e}"
        );
    }
}

/// `POST /api/v1/content-types` — create a content type; it is live at
/// once (entries, URL, archive, templates, bindings, search).
#[utoipa::path(
    post, path = "/api/v1/content-types", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    request_body = CreateTypeRequest,
    responses(
        (status = 201, description = "Created", body = AdminType),
        (status = 400, description = "Malformed or reserved slug, bad label, 64 types already", body = ApiErrorBody),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
        (status = 409, description = "Slug taken by a type, a plugin, a taxonomy or stored entries", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<CreateTypeRequest>,
) -> ApiResult<(StatusCode, Json<AdminType>)> {
    let plugins = plugin_slugs(&state).await;
    let row = ContentTypesService::new(state.pool.clone())
        .create(
            NewType {
                slug: body.slug,
                singular: body.singular,
                plural: body.plural,
                description: body.description,
                public: body.public,
                has_archive: body.has_archive,
            },
            &plugins,
        )
        .await?;
    pages_changed(&state);
    crate::audit::record(
        &state,
        principal.user(),
        "content_type.create",
        format!("content_type:{}", row.slug),
        serde_json::json!({}),
    );
    Ok((StatusCode::CREATED, Json(AdminType::from(row))))
}

/// `PUT /api/v1/content-types/{slug}` — relabel a type or change its
/// flags. The slug never changes.
#[utoipa::path(
    put, path = "/api/v1/content-types/{slug}", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("slug" = String, Path, description = "Type slug")),
    request_body = UpdateTypeRequest,
    responses(
        (status = 200, description = "Updated", body = AdminType),
        (status = 400, description = "Bad label, a slug change, or not an administrator's type", body = ApiErrorBody),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
        (status = 404, description = "No such type", body = ApiErrorBody),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    principal: Principal,
    Path(slug): Path<String>,
    Json(body): Json<UpdateTypeRequest>,
) -> ApiResult<Json<AdminType>> {
    admin_only(&slug)?;
    let visibility_changes = body.public.is_some();
    let archive_changes = body.has_archive.is_some();
    if body.slug.as_deref().is_some_and(|s| s != slug) {
        return Err(ApiError(AppError::validation(
            "a content type's slug cannot change: it is in URLs and templates",
        )));
    }
    let row = ContentTypesService::new(state.pool.clone())
        .update(
            &slug,
            TypeChanges {
                singular: body.singular,
                plural: body.plural,
                description: body.description,
                public: body.public,
                has_archive: body.has_archive,
            },
        )
        .await?;
    pages_changed(&state);
    if visibility_changes || archive_changes {
        // An edge copy of a page listing the type's entries (or of an
        // entry, or of its archive) would outlive the flip until its TTL;
        // the render cache's epoch does not reach the edge, so purge it
        // too.
        state.cdn.purge_all().await;
    }
    if visibility_changes {
        // A type made public becomes searchable; one made private leaves
        // the index (query-time filters cover the meantime).
        reindex(&state, &slug).await;
    }
    crate::audit::record(
        &state,
        principal.user(),
        "content_type.update",
        format!("content_type:{slug}"),
        serde_json::json!({}),
    );
    Ok(Json(AdminType::from(row)))
}

/// `DELETE /api/v1/content-types/{slug}` — delete a type that has no
/// entries in any status (trash included), with its field definitions.
#[utoipa::path(
    delete, path = "/api/v1/content-types/{slug}", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("slug" = String, Path, description = "Type slug")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 400, description = "A built-in or plugin type", body = ApiErrorBody),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
        (status = 404, description = "No such type", body = ApiErrorBody),
        (status = 409, description = "Entries exist; the message names the count", body = ApiErrorBody),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(slug): Path<String>,
) -> ApiResult<StatusCode> {
    admin_only(&slug)?;
    ContentTypesService::new(state.pool.clone())
        .delete(&slug)
        .await?;
    pages_changed(&state);
    crate::audit::record(
        &state,
        principal.user(),
        "content_type.delete",
        format!("content_type:{slug}"),
        serde_json::json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// One field definition.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContentField {
    /// The type it belongs to.
    pub type_slug: String,
    /// Key in an entry's `fields`; immutable.
    pub key: String,
    /// Label shown in the editor.
    pub label: String,
    /// Help shown under the input.
    pub help: String,
    /// `text`, `textarea`, `number`, `boolean`, `date`, `choice`, `url`,
    /// `media` or `entry`.
    pub kind: String,
    /// Whether publishing or scheduling needs a value.
    pub required: bool,
    /// Per-kind options.
    pub options: serde_json::Value,
    /// Order within the type, lowest first.
    pub position: i32,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last change.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<vyasa_db::repo::ContentFieldRow> for ContentField {
    fn from(r: vyasa_db::repo::ContentFieldRow) -> Self {
        Self {
            type_slug: r.type_slug,
            key: r.key,
            label: r.label,
            help: r.help,
            kind: r.kind,
            required: r.required,
            options: r.options,
            position: r.position,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Body of `POST /content-types/{slug}/fields`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentFieldInput {
    /// `^[a-z][a-z0-9_]{0,39}$`, unique within the type.
    pub key: String,
    /// 1 to 120 characters.
    pub label: String,
    /// Up to 500 characters.
    #[serde(default)]
    pub help: String,
    /// One of the nine kinds.
    pub kind: String,
    /// Needed to publish or schedule (default false).
    #[serde(default)]
    pub required: bool,
    /// Per-kind options (default `{}`).
    #[serde(default = "empty_object")]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub options: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

/// Body of `PUT /content-types/{slug}/fields/{key}`: what to change. The
/// key cannot change; sending a different one is refused.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentFieldChanges {
    /// Must equal the path's key when sent.
    pub key: Option<String>,
    /// New label.
    pub label: Option<String>,
    /// New help text.
    pub help: Option<String>,
    /// New kind (refused while any entry stores a value for the field).
    pub kind: Option<String>,
    /// New required flag.
    pub required: Option<bool>,
    /// New options, replacing the old ones.
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub options: Option<serde_json::Value>,
}

/// Body of `PUT /content-types/{slug}/field-order`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentFieldOrder {
    /// Every field key of the type, in the new order.
    pub keys: Vec<String>,
}

/// Values stored under a key no field defines any more.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContentFieldOrphan {
    /// The key.
    pub key: String,
    /// Entries holding a value for it.
    pub entries: i64,
}

/// What a clean-up removed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContentFieldCleanUp {
    /// Entries whose stored value was removed.
    pub entries: u64,
}

fn fields_service(state: &AppState) -> ContentFieldsService {
    ContentFieldsService::new(state.pool.clone())
}

/// `GET /api/v1/content-types/{slug}/fields` — a type's fields in order
/// (built-in, plugin and administrators' types alike). Anyone who edits
/// entries reads them: the editor renders its Fields panel from this.
#[utoipa::path(
    get, path = "/api/v1/content-types/{slug}/fields", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("slug" = String, Path, description = "Type slug")),
    responses(
        (status = 200, description = "Fields in order", body = [ContentField]),
        (status = 400, description = "`block` has no fields", body = ApiErrorBody),
        (status = 404, description = "No such type", body = ApiErrorBody),
    )
)]
pub async fn list_fields(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<Vec<ContentField>>> {
    let rows = fields_service(&state).list(&slug).await?;
    Ok(Json(rows.into_iter().map(ContentField::from).collect()))
}

/// `POST /api/v1/content-types/{slug}/fields` — add a field after the
/// type's last one.
#[utoipa::path(
    post, path = "/api/v1/content-types/{slug}/fields", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("slug" = String, Path, description = "Type slug")),
    request_body = ContentFieldInput,
    responses(
        (status = 201, description = "Created", body = ContentField),
        (status = 400, description = "Bad key, label, kind or options; 64 fields already", body = ApiErrorBody),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
        (status = 404, description = "No such type", body = ApiErrorBody),
        (status = 409, description = "Key taken, or a deleted field's values are still stored under it", body = ApiErrorBody),
    )
)]
pub async fn create_field(
    State(state): State<AppState>,
    principal: Principal,
    Path(slug): Path<String>,
    Json(body): Json<ContentFieldInput>,
) -> ApiResult<(StatusCode, Json<ContentField>)> {
    let kind = FieldKind::parse(&body.kind)?;
    let row = fields_service(&state)
        .create(
            &slug,
            NewField {
                key: body.key,
                label: body.label,
                help: body.help,
                kind,
                required: body.required,
                options: body.options,
            },
        )
        .await?;
    pages_changed(&state);
    crate::audit::record(
        &state,
        principal.user(),
        "content_field.create",
        format!("content_field:{slug}.{}", row.key),
        serde_json::json!({ "kind": row.kind }),
    );
    Ok((StatusCode::CREATED, Json(ContentField::from(row))))
}

/// `PUT /api/v1/content-types/{slug}/fields/{key}` — change a field.
#[utoipa::path(
    put, path = "/api/v1/content-types/{slug}/fields/{key}", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("slug" = String, Path, description = "Type slug"),
        ("key" = String, Path, description = "Field key"),
    ),
    request_body = ContentFieldChanges,
    responses(
        (status = 200, description = "Updated", body = ContentField),
        (status = 400, description = "Bad label, kind or options, or a key change", body = ApiErrorBody),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
        (status = 404, description = "No such field", body = ApiErrorBody),
        (status = 409, description = "Kind change while entries store a value; the message names the count", body = ApiErrorBody),
    )
)]
pub async fn update_field(
    State(state): State<AppState>,
    principal: Principal,
    Path((slug, key)): Path<(String, String)>,
    Json(body): Json<ContentFieldChanges>,
) -> ApiResult<Json<ContentField>> {
    if body.key.as_deref().is_some_and(|k| k != key) {
        return Err(ApiError(AppError::validation(
            "a field's key cannot change: entries store their values under it",
        )));
    }
    let kind = body.kind.as_deref().map(FieldKind::parse).transpose()?;
    let kind_changes = kind.is_some();
    let row = fields_service(&state)
        .update(
            &slug,
            &key,
            FieldChanges {
                label: body.label,
                help: body.help,
                kind,
                required: body.required,
                options: body.options,
            },
        )
        .await?;
    pages_changed(&state);
    if kind_changes {
        reindex(&state, &slug).await;
    }
    crate::audit::record(
        &state,
        principal.user(),
        "content_field.update",
        format!("content_field:{slug}.{key}"),
        serde_json::json!({}),
    );
    Ok(Json(ContentField::from(row)))
}

/// `DELETE /api/v1/content-types/{slug}/fields/{key}` — delete a field.
/// Values entries store under it stay stored (and are no longer shown or
/// served) until a clean-up removes them.
#[utoipa::path(
    delete, path = "/api/v1/content-types/{slug}/fields/{key}", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("slug" = String, Path, description = "Type slug"),
        ("key" = String, Path, description = "Field key"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
        (status = 404, description = "No such field", body = ApiErrorBody),
    )
)]
pub async fn delete_field(
    State(state): State<AppState>,
    principal: Principal,
    Path((slug, key)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    fields_service(&state).delete(&slug, &key).await?;
    pages_changed(&state);
    // Its text stops matching in search.
    reindex(&state, &slug).await;
    crate::audit::record(
        &state,
        principal.user(),
        "content_field.delete",
        format!("content_field:{slug}.{key}"),
        serde_json::json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/content-types/{slug}/field-order` — reorder a type's
/// fields; `keys` must name each of them once.
#[utoipa::path(
    put, path = "/api/v1/content-types/{slug}/field-order", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("slug" = String, Path, description = "Type slug")),
    request_body = ContentFieldOrder,
    responses(
        (status = 200, description = "Fields in the new order", body = [ContentField]),
        (status = 400, description = "Not exactly the type's keys", body = ApiErrorBody),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
    )
)]
pub async fn reorder_fields(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<ContentFieldOrder>,
) -> ApiResult<Json<Vec<ContentField>>> {
    let rows = fields_service(&state).reorder(&slug, &body.keys).await?;
    pages_changed(&state);
    Ok(Json(rows.into_iter().map(ContentField::from).collect()))
}

/// `GET /api/v1/content-types/{slug}/orphans` — keys entries still store
/// values under although no field defines them (what a clean-up removes).
#[utoipa::path(
    get, path = "/api/v1/content-types/{slug}/orphans", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(("slug" = String, Path, description = "Type slug")),
    responses(
        (status = 200, description = "Orphaned keys with entry counts", body = [ContentFieldOrphan]),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
    )
)]
pub async fn orphans(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<Vec<ContentFieldOrphan>>> {
    let found = fields_service(&state).orphaned_values(&slug).await?;
    Ok(Json(
        found
            .into_iter()
            .map(|(key, entries)| ContentFieldOrphan { key, entries })
            .collect(),
    ))
}

/// `POST /api/v1/content-types/{slug}/orphans/{key}/clean-up` — remove
/// the values stored under a key no field defines, from every entry of
/// the type and its revisions. Refused (409) while a field defines it.
#[utoipa::path(
    post, path = "/api/v1/content-types/{slug}/orphans/{key}/clean-up", tag = "content",
    security(("session_cookie" = []), ("api_key" = [])),
    params(
        ("slug" = String, Path, description = "Type slug"),
        ("key" = String, Path, description = "Orphaned key"),
    ),
    responses(
        (status = 200, description = "Entries changed", body = ContentFieldCleanUp),
        (status = 403, description = "Missing manage_options", body = ApiErrorBody),
        (status = 409, description = "A field still defines the key", body = ApiErrorBody),
    )
)]
pub async fn clean_up(
    State(state): State<AppState>,
    principal: Principal,
    Path((slug, key)): Path<(String, String)>,
) -> ApiResult<Json<ContentFieldCleanUp>> {
    let entries = fields_service(&state).clean_up(&slug, &key).await?;
    pages_changed(&state);
    crate::audit::record(
        &state,
        principal.user(),
        "content_field.clean_up",
        format!("content_field:{slug}.{key}"),
        serde_json::json!({ "entries": entries }),
    );
    Ok(Json(ContentFieldCleanUp { entries }))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::too_many_lines
    )]

    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;
    use vyasa_db::models::Role;
    use vyasa_db::repo::{NewUser, UsersRepo};
    use vyasa_testkit::TestDb;

    use crate::authz::tests::{session, test_state};
    use crate::middleware::auth::SESSION_COOKIE;
    use crate::state::AppState;

    const ADMIN: i64 = 99_901;

    /// Counts whole-site edge purges.
    #[derive(Default)]
    struct Recorder {
        all: AtomicUsize,
    }

    impl crate::cdn::Purger for Recorder {
        fn purge<'a>(
            &'a self,
            _urls: Vec<String>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }

        fn purge_all<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            self.all.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    async fn send(
        state: &AppState,
        token: &str,
        method: Method,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, String) {
        let response = crate::rest::router()
            .with_state(state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("infallible");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("body");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn epoch(state: &AppState) -> u64 {
        state.surface_epoch.load(Ordering::Relaxed)
    }

    /// Flipping a type's visibility purges the edge as well as the render
    /// cache: an edge copy of a type made private would otherwise keep
    /// serving its entries until the TTL ran out. Reordering fields and
    /// cleaning up values change what pages show, so they move the
    /// render cache's epoch too.
    #[tokio::test]
    async fn a_visibility_flip_purges_the_edge_and_field_changes_move_the_epoch() {
        let db = TestDb::new().await;
        let base = test_state(&db);
        UsersRepo::new(base.pool.clone())
            .insert(&NewUser {
                id: ADMIN,
                email: "ct-purge-admin@example.com",
                username: "ctpurgeadmin",
                display_name: "Admin",
                password_hash: None,
                role: Role::Admin,
                bio: "",
            })
            .await
            .expect("admin");
        let recorder = Arc::new(Recorder::default());
        let mut state = (*base).clone();
        state.cdn = Arc::clone(&recorder) as Arc<dyn crate::cdn::Purger>;
        state.surface_epoch = Arc::new(AtomicU64::new(0));
        let token = session(&state, ADMIN).await;

        let (status, body) = send(
            &state,
            &token,
            Method::POST,
            "/content-types",
            json!({"slug": "ty-edge-purge", "singular": "P", "plural": "Ps"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        // A relabel touches no visibility: no edge purge.
        let (status, body) = send(
            &state,
            &token,
            Method::PUT,
            "/content-types/ty-edge-purge",
            json!({"plural": "Things"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(recorder.all.load(Ordering::SeqCst), 0);
        for public in [false, true] {
            let (status, body) = send(
                &state,
                &token,
                Method::PUT,
                "/content-types/ty-edge-purge",
                json!({ "public": public }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        assert_eq!(recorder.all.load(Ordering::SeqCst), 2, "one purge per flip");
        let (status, body) = send(
            &state,
            &token,
            Method::PUT,
            "/content-types/ty-edge-purge",
            json!({ "has_archive": false }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            recorder.all.load(Ordering::SeqCst),
            3,
            "and per archive change"
        );

        for key in ["a", "b"] {
            let (status, body) = send(
                &state,
                &token,
                Method::POST,
                "/content-types/ty-edge-purge/fields",
                json!({"key": key, "label": key, "kind": "text"}),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
        }
        let before = epoch(&state);
        let (status, body) = send(
            &state,
            &token,
            Method::PUT,
            "/content-types/ty-edge-purge/field-order",
            json!({"keys": ["b", "a"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(epoch(&state) > before, "a reorder moves the epoch");

        let (status, body) = send(
            &state,
            &token,
            Method::DELETE,
            "/content-types/ty-edge-purge/fields/a",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let before = epoch(&state);
        let (status, body) = send(
            &state,
            &token,
            Method::POST,
            "/content-types/ty-edge-purge/orphans/a/clean-up",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(epoch(&state) > before, "a clean-up moves the epoch");

        // Leave the process-global registry as it was.
        send(
            &state,
            &token,
            Method::DELETE,
            "/content-types/ty-edge-purge/fields/b",
            json!({}),
        )
        .await;
        let (status, body) = send(
            &state,
            &token,
            Method::DELETE,
            "/content-types/ty-edge-purge",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
}
