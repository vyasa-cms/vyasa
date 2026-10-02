//! Menus repository: named nav menus with ordered, nestable items.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use vyasa_common::AppError;

/// One navigation menu.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MenuRow {
    /// Snowflake id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// URL-safe identifier.
    pub slug: String,
    /// Theme slot this menu fills (`header`, `footer`, …); `None` = unused.
    pub location: Option<String>,
}

/// One entry in a menu.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MenuItemRow {
    /// Snowflake id.
    pub id: i64,
    /// Owning menu.
    pub menu_id: i64,
    /// Parent item for nesting.
    pub parent_id: Option<i64>,
    /// Link text.
    pub label: String,
    /// Target href.
    pub url: String,
    /// Ascending position within siblings.
    pub sort_order: i32,
}

/// Payload to create a menu item.
#[derive(Debug, Clone, Deserialize)]
pub struct NewMenuItem {
    /// Parent item id (nesting).
    pub parent_id: Option<i64>,
    /// Link text.
    pub label: String,
    /// Target href.
    pub url: String,
    /// Position within siblings.
    pub sort_order: i32,
}

/// Data access for menus.
#[derive(Clone, Debug)]
pub struct MenusRepo {
    pool: PgPool,
}

impl MenusRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Creates a menu.
    ///
    /// # Errors
    /// [`AppError::Conflict`] on duplicate slug/name; [`AppError::Db`] otherwise.
    pub async fn create(
        &self,
        name: &str,
        slug: &str,
        location: Option<&str>,
    ) -> Result<MenuRow, AppError> {
        sqlx::query_as::<_, MenuRow>(
            "INSERT INTO menus (id, name, slug, location) VALUES ($1, $2, $3, $4)
             RETURNING id, name, slug, location",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(name)
        .bind(slug)
        .bind(location)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| match &err {
            sqlx::Error::Database(db)
                if db
                    .constraint()
                    .is_some_and(|c| c.contains("unique") || c.contains("_key")) =>
            {
                AppError::conflict(format!("menu \"{name}\" already exists"))
            }
            _ => AppError::db(format!("menu create failed: {err}")),
        })
    }

    /// Lists all menus.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list(&self) -> Result<Vec<MenuRow>, AppError> {
        sqlx::query_as::<_, MenuRow>("SELECT id, name, slug, location FROM menus ORDER BY name")
            .fetch_all(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("menu list failed: {err}")))
    }

    /// Fetches one menu by id.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<MenuRow, AppError> {
        sqlx::query_as::<_, MenuRow>("SELECT id, name, slug, location FROM menus WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("menu get failed: {err}")))?
            .ok_or_else(|| AppError::not_found("menu", id))
    }

    /// Fetches a menu by slug.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn get_by_slug(&self, slug: &str) -> Result<MenuRow, AppError> {
        sqlx::query_as::<_, MenuRow>("SELECT id, name, slug, location FROM menus WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("menu get failed: {err}")))?
            .ok_or_else(|| AppError::not_found("menu", slug))
    }

    /// Updates name/location.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn update(
        &self,
        id: i64,
        name: &str,
        location: Option<&str>,
    ) -> Result<(), AppError> {
        let result = sqlx::query(
            "UPDATE menus SET name = $2, location = $3, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(name)
        .bind(location)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("menu update failed: {err}")))?;
        if result.rows_affected() == 0 {
            return Err(AppError::not_found("menu", id));
        }
        Ok(())
    }

    /// Deletes a menu and its items (cascade).
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let result = sqlx::query("DELETE FROM menus WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("menu delete failed: {err}")))?;
        if result.rows_affected() == 0 {
            return Err(AppError::not_found("menu", id));
        }
        Ok(())
    }

    /// Adds an item to a menu.
    ///
    /// # Errors
    /// [`AppError::Validation`] when the parent belongs to another menu;
    /// [`AppError::NotFound`] when the menu or parent is missing.
    pub async fn add_item(
        &self,
        menu_id: i64,
        item: &NewMenuItem,
    ) -> Result<MenuItemRow, AppError> {
        if let Some(pid) = item.parent_id {
            let parent_menu: Option<i64> =
                sqlx::query_scalar("SELECT menu_id FROM menu_items WHERE id = $1")
                    .bind(pid)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|err| AppError::db(format!("parent lookup failed: {err}")))?;
            if parent_menu.is_none() {
                return Err(AppError::not_found("menu_item", pid));
            }
            if parent_menu != Some(menu_id) {
                return Err(AppError::validation(
                    "parent item must belong to the same menu",
                ));
            }
        }
        sqlx::query_as::<_, MenuItemRow>(
            "INSERT INTO menu_items (id, menu_id, parent_id, label, url, sort_order)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, menu_id, parent_id, label, url, sort_order",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(menu_id)
        .bind(item.parent_id)
        .bind(&item.label)
        .bind(&item.url)
        .bind(item.sort_order)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("menu item insert failed: {err}")))
    }

    /// Items of a menu in the order they are shown.
    ///
    /// Siblings come back in `sort_order`; the renderer picks each parent's
    /// children out of the list itself, so that is the only order that
    /// matters. The previous `ORDER BY COALESCE(parent_id, id)` grouped
    /// top-level items by their own id, which put them on the site in the
    /// order they were created no matter how the admin arranged them —
    /// the admin sorted client-side and never noticed.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn items(&self, menu_id: i64) -> Result<Vec<MenuItemRow>, AppError> {
        sqlx::query_as::<_, MenuItemRow>(
            "SELECT id, menu_id, parent_id, label, url, sort_order
             FROM menu_items WHERE menu_id = $1
             ORDER BY sort_order, id",
        )
        .bind(menu_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("menu items failed: {err}")))
    }

    /// Updates one item's label, url, parent or position.
    ///
    /// Passing `None` leaves that field as it was. Re-parenting is validated
    /// against the same menu so an item cannot be moved into another menu's
    /// tree.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when the item is missing,
    /// [`AppError::Validation`] when the new parent belongs elsewhere or is
    /// the item itself.
    pub async fn update_item(
        &self,
        item_id: i64,
        label: Option<&str>,
        url: Option<&str>,
        parent_id: Option<Option<i64>>,
        sort_order: Option<i32>,
    ) -> Result<MenuItemRow, AppError> {
        let current: Option<(i64, Option<i64>)> =
            sqlx::query_as("SELECT menu_id, parent_id FROM menu_items WHERE id = $1")
                .bind(item_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| AppError::db(format!("menu item lookup failed: {err}")))?;
        let Some((menu_id, _)) = current else {
            return Err(AppError::not_found("menu_item", item_id));
        };

        if let Some(Some(new_parent)) = parent_id {
            if new_parent == item_id {
                return Err(AppError::validation("an item cannot be its own parent"));
            }
            let parent_menu: Option<i64> =
                sqlx::query_scalar("SELECT menu_id FROM menu_items WHERE id = $1")
                    .bind(new_parent)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|err| AppError::db(format!("parent lookup failed: {err}")))?;
            match parent_menu {
                None => return Err(AppError::not_found("menu_item", new_parent)),
                Some(other) if other != menu_id => {
                    return Err(AppError::validation(
                        "parent item must belong to the same menu",
                    ));
                }
                Some(_) => {}
            }
        }

        // `parent_id` is three-state: absent (leave), Some(None) (detach to
        // top level), Some(Some(id)) (re-parent). COALESCE cannot express
        // "set to NULL", so the flag drives it explicitly.
        let (set_parent, parent_value) = match parent_id {
            None => (false, None),
            Some(value) => (true, value),
        };

        sqlx::query_as::<_, MenuItemRow>(
            "UPDATE menu_items
                SET label = COALESCE($2, label),
                    url = COALESCE($3, url),
                    parent_id = CASE WHEN $4 THEN $5 ELSE parent_id END,
                    sort_order = COALESCE($6, sort_order)
              WHERE id = $1
              RETURNING id, menu_id, parent_id, label, url, sort_order",
        )
        .bind(item_id)
        .bind(label)
        .bind(url)
        .bind(set_parent)
        .bind(parent_value)
        .bind(sort_order)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("menu item update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("menu_item", item_id))
    }

    /// Removes one item (children cascade).
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn remove_item(&self, item_id: i64) -> Result<(), AppError> {
        let result = sqlx::query("DELETE FROM menu_items WHERE id = $1")
            .bind(item_id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("menu item delete failed: {err}")))?;
        if result.rows_affected() == 0 {
            return Err(AppError::not_found("menu_item", item_id));
        }
        Ok(())
    }
}
