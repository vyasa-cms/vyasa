//! Menu domain rules: naming, URL safety, nesting depth.

use vyasa_common::AppError;
use vyasa_db::repo::{MenuItemRow, MenusRepo, NewMenuItem};

use super::draft::{url_allowed, MenuDraft, MenuDraftItem};

/// Maximum menu nesting depth (top-level = 1).
pub const MAX_MENU_DEPTH: usize = 3;
/// Maximum label length.
pub const MAX_LABEL_LEN: usize = 80;

/// Application service for menus.
#[derive(Clone, Debug)]
pub struct MenuService {
    repo: MenusRepo,
}

impl MenuService {
    /// Wraps a menus repository.
    #[must_use]
    pub fn new(repo: MenusRepo) -> Self {
        Self { repo }
    }

    /// Creates a named menu, optionally bound to a theme location.
    ///
    /// # Errors
    /// [`AppError::Validation`] on bad name; [`AppError::Conflict`] on
    /// duplicate slug.
    pub async fn create(
        &self,
        name: &str,
        slug: Option<&str>,
        location: Option<&str>,
    ) -> Result<vyasa_db::repo::MenuRow, AppError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(AppError::validation("menu name must be 1..=60 characters"));
        }
        let slug = match slug.map(str::trim).filter(|s| !s.is_empty()) {
            // A given reference must already be one: the same rule the
            // derived one follows, so a theme can address it verbatim.
            Some(given) => {
                if vyasa_common::slugify(given) != given || given.chars().count() > 60 {
                    return Err(AppError::validation(
                        "The reference may only use lowercase letters, digits and hyphens.",
                    ));
                }
                given.to_owned()
            }
            None => vyasa_common::slugify(name),
        };
        let result = self.repo.create(name, &slug, location).await?;
        crate::events::emit(crate::events::Event::MenuChanged);
        Ok(result)
    }

    /// Lists menus.
    ///
    /// # Errors
    /// Propagates repo errors.
    pub async fn list(&self) -> Result<Vec<vyasa_db::repo::MenuRow>, AppError> {
        self.repo.list().await
    }

    /// Gets one menu with its ordered items.
    ///
    /// # Errors
    /// Propagates repo errors.
    pub async fn get_with_items(
        &self,
        id: i64,
    ) -> Result<(vyasa_db::repo::MenuRow, Vec<MenuItemRow>), AppError> {
        let menu = self.repo.get(id).await?;
        let items = self.repo.items(id).await?;
        Ok((menu, items))
    }

    /// Renders a menu as nested HTML `<ul>` for the given slug.
    ///
    /// Returns an empty string when the menu does not exist (themes render
    /// gracefully without configuration).
    ///
    /// # Errors
    /// Propagates repo errors.
    pub async fn render_html(&self, slug: &str) -> Result<String, AppError> {
        let Ok(menu) = self.repo.get_by_slug(slug).await else {
            return Ok(String::new());
        };
        let items = self.repo.items(menu.id).await?;
        Ok(MenuDraft::from_rows(&menu, &items).render_html())
    }

    /// Makes the database look like `draft`: the menu is created or
    /// renamed by slug, and its items are replaced wholesale, in order.
    ///
    /// This is how an accepted studio proposal's staged menus land —
    /// the same validation the interactive endpoints enforce, applied
    /// as one deliberate write.
    ///
    /// # Errors
    /// [`AppError::Validation`] when the draft breaks a menu rule (the
    /// database is untouched); repo errors otherwise.
    pub async fn apply_draft(&self, draft: &MenuDraft) -> Result<(), AppError> {
        let problems = draft.validate();
        if !problems.is_empty() {
            return Err(AppError::validation(problems.join("; ")));
        }
        let menu = match self.repo.get_by_slug(&draft.slug).await {
            Ok(existing) => {
                if existing.name != draft.name.trim() || existing.location != draft.location {
                    self.repo
                        .update(existing.id, draft.name.trim(), draft.location.as_deref())
                        .await?;
                }
                existing
            }
            Err(AppError::NotFound { .. }) => {
                self.create(&draft.name, Some(&draft.slug), draft.location.as_deref())
                    .await?
            }
            Err(other) => return Err(other),
        };
        // Top-level deletes cascade to subtrees (FK ON DELETE CASCADE).
        for item in self.repo.items(menu.id).await? {
            if item.parent_id.is_none() {
                self.repo.remove_item(item.id).await?;
            }
        }
        self.add_draft_items(menu.id, None, &draft.items).await?;
        crate::events::emit(crate::events::Event::MenuChanged);
        Ok(())
    }

    /// Inserts `items` under `parent`, depth-first so children can name
    /// their parent's fresh id.
    fn add_draft_items<'a>(
        &'a self,
        menu_id: i64,
        parent: Option<i64>,
        items: &'a [MenuDraftItem],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AppError>> + Send + 'a>>
    {
        Box::pin(async move {
            for (order, item) in items.iter().enumerate() {
                let row = self
                    .repo
                    .add_item(
                        menu_id,
                        &NewMenuItem {
                            parent_id: parent,
                            label: item.label.trim().to_owned(),
                            url: item.url.clone(),
                            sort_order: i32::try_from(order).unwrap_or(i32::MAX),
                        },
                    )
                    .await?;
                self.add_draft_items(menu_id, Some(row.id), &item.children)
                    .await?;
            }
            Ok(())
        })
    }

    /// Adds a validated item.
    ///
    /// # Errors
    /// [`AppError::Validation`] on empty/overlong labels or disallowed URLs
    /// or excessive depth.
    pub async fn add_item(
        &self,
        menu_id: i64,
        item: &NewMenuItem,
    ) -> Result<MenuItemRow, AppError> {
        let label = item.label.trim();
        if label.is_empty() || label.chars().count() > MAX_LABEL_LEN {
            return Err(AppError::validation(format!(
                "label must be 1..={MAX_LABEL_LEN} characters"
            )));
        }
        if !url_allowed(&item.url) {
            return Err(AppError::validation(format!(
                "menu url {:?} must start with /, #, http(s):// or mailto:",
                item.url
            )));
        }
        let depth = self.depth_of(menu_id, item.parent_id).await?;
        if depth > MAX_MENU_DEPTH {
            return Err(AppError::validation(format!(
                "menus nest at most {MAX_MENU_DEPTH} levels"
            )));
        }
        let result = self.repo.add_item(menu_id, item).await?;
        crate::events::emit(crate::events::Event::MenuChanged);
        Ok(result)
    }

    /// Removes an item.
    ///
    /// # Errors
    /// Propagates repo errors.
    pub async fn remove_item(&self, item_id: i64) -> Result<(), AppError> {
        self.repo.remove_item(item_id).await?;
        crate::events::emit(crate::events::Event::MenuChanged);
        Ok(())
    }

    /// Deletes a whole menu.
    ///
    /// # Errors
    /// Propagates repo errors.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        self.repo.delete(id).await?;
        crate::events::emit(crate::events::Event::MenuChanged);
        Ok(())
    }

    async fn depth_of(&self, menu_id: i64, mut parent: Option<i64>) -> Result<usize, AppError> {
        let mut depth = 1usize;
        while let Some(pid) = parent {
            depth += 1;
            let items = self.repo.items(menu_id).await?;
            parent = items.iter().find(|i| i.id == pid).and_then(|i| i.parent_id);
            if depth > MAX_MENU_DEPTH + 1 {
                break;
            }
        }
        Ok(depth)
    }
}
