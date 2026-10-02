//! Navigation menus: CRUD service over [`vyasa_db::repo::MenusRepo`].

mod draft;
mod service;

pub use draft::{MenuDraft, MenuDraftItem, MAX_MENU_ITEMS};
pub use service::MenuService;
