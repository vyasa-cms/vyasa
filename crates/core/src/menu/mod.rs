//! Navigation menus: CRUD service over [`vyasa_db::repo::MenusRepo`].

mod draft;
mod service;

pub use draft::{extras_problem as item_extras_problem, MenuDraft, MenuDraftItem, MAX_MENU_ITEMS};
pub use service::MenuService;
