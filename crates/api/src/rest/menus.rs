//! Navigation menu endpoints (ManageThemes capability).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_core::menu::MenuService;
use vyasa_db::repo::{MenuItemRow, NewMenuItem};

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::state::AppState;

fn service(state: &AppState) -> MenuService {
    MenuService::new(state.menus.clone())
}

/// A menu without items.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MenuResponse {
    /// Snowflake id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Slug.
    pub slug: String,
    /// Theme location slot.
    pub location: Option<String>,
}

/// One menu item.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MenuItemResponse {
    /// Item id.
    pub id: i64,
    /// Parent item id for nesting.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Link text.
    pub label: String,
    /// Href.
    pub url: String,
    /// Sibling order.
    pub sort_order: i32,
}

/// Create payload.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateMenuBody {
    /// Display name.
    pub name: String,
    /// The reference a theme addresses the menu by. Derived from the name
    /// when absent. The admin has always sent one and the server has
    /// always thrown it away, so the slug an operator typed was never the
    /// slug they got.
    #[serde(default)]
    pub slug: Option<String>,
    /// Theme location slot (`header`, `footer`, …).
    #[serde(default)]
    pub location: Option<String>,
}

/// Item create payload.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct AddItemBody {
    /// Parent item for nesting.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Link text.
    pub label: String,
    /// Target href.
    pub url: String,
    /// Sibling order.
    #[serde(default)]
    pub sort_order: i32,
}

impl From<vyasa_db::repo::MenuRow> for MenuResponse {
    fn from(m: vyasa_db::repo::MenuRow) -> Self {
        Self {
            id: m.id,
            name: m.name,
            slug: m.slug,
            location: m.location,
        }
    }
}

impl From<MenuItemRow> for MenuItemResponse {
    fn from(i: MenuItemRow) -> Self {
        Self {
            id: i.id,
            parent_id: i.parent_id,
            label: i.label,
            url: i.url,
            sort_order: i.sort_order,
        }
    }
}

/// `POST /api/v1/menus` — create a menu.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateMenuBody>,
) -> ApiResult<(StatusCode, Json<MenuResponse>)> {
    let menu = service(&state)
        .create(&body.name, body.slug.as_deref(), body.location.as_deref())
        .await
        .map_err(ApiError)?;
    invalidate(&state);
    Ok((StatusCode::CREATED, Json(menu.into())))
}

/// `GET /api/v1/menus` — list menus.
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<MenuResponse>>> {
    let menus = service(&state).list().await.map_err(ApiError)?;
    Ok(Json(menus.into_iter().map(Into::into).collect()))
}

/// `GET /api/v1/menus/{id}` — one menu with ordered items.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<(MenuResponse, Vec<MenuItemResponse>)>> {
    let (menu, items) = service(&state).get_with_items(id).await.map_err(ApiError)?;
    Ok(Json((
        menu.into(),
        items.into_iter().map(Into::into).collect(),
    )))
}

/// `DELETE /api/v1/menus/{id}` — delete a menu and its items.
pub async fn delete(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    service(&state).delete(id).await.map_err(ApiError)?;
    invalidate(&state);
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/menus/{id}/items` — append an item.
pub async fn add_item(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<AddItemBody>,
) -> ApiResult<(StatusCode, Json<MenuItemResponse>)> {
    let item = NewMenuItem {
        parent_id: body.parent_id,
        label: body.label,
        url: body.url,
        sort_order: body.sort_order,
    };
    let row = service(&state)
        .add_item(id, &item)
        .await
        .map_err(ApiError)?;
    invalidate(&state);
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Partial update for a menu item. Absent fields are left alone.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateItemBody {
    /// New link text.
    pub label: Option<String>,
    /// New href.
    pub url: Option<String>,
    /// New parent item. Ignored when `detach` is true.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Move the item to the top level, clearing any parent.
    #[serde(default)]
    pub detach: bool,
    /// New position among siblings.
    pub sort_order: Option<i32>,
}

/// `PATCH /api/v1/menus/items/{item_id}` — rename, re-parent or reorder.
#[utoipa::path(
    patch, path = "/api/v1/menus/items/{item_id}",
    tag = "menus",
    security(("session_cookie" = [])),
    params(("item_id" = i64, Path, description = "Menu item id")),
    request_body = UpdateItemBody,
    responses(
        (status = 200, description = "Updated", body = MenuItemResponse),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn update_item(
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
    Json(body): Json<UpdateItemBody>,
) -> ApiResult<Json<MenuItemResponse>> {
    let row = state
        .menus
        .update_item(
            item_id,
            body.label.as_deref(),
            body.url.as_deref(),
            // Three-state: `detach` clears the parent, a value re-parents,
            // and absent leaves the item where it is.
            if body.detach {
                Some(None)
            } else {
                body.parent_id.map(Some)
            },
            body.sort_order,
        )
        .await
        .map_err(ApiError)?;
    vyasa_core::events::emit(vyasa_core::events::Event::MenuChanged);
    invalidate(&state);
    Ok(Json(row.into()))
}

/// `DELETE /api/v1/menus/items/{item_id}` — remove an item (children cascade).
pub async fn remove_item(
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
) -> ApiResult<StatusCode> {
    service(&state)
        .remove_item(item_id)
        .await
        .map_err(ApiError)?;
    invalidate(&state);
    Ok(StatusCode::NO_CONTENT)
}

fn invalidate(state: &AppState) {
    if let Some(cache) = &state.render_cache {
        cache.invalidate_options();
    }
}
