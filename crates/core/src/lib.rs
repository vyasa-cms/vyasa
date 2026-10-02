//! Domain logic for Vyasa: users/auth/RBAC, posts, blocks, taxonomies,
//! media rules, comments, and domain events.
//! Pure and testable; must not depend on HTTP or GraphQL layers.

pub mod ai_models;
pub mod block;
pub mod comment;
pub mod content;
pub mod events;
pub mod health;
pub mod media;
pub mod menu;
pub mod notify;
pub mod options;
pub mod post;
pub mod taxonomy;
pub mod user;
pub mod webhooks;

pub use block::{Block, BlockDocument, BlockKind};
pub use events::{Event, OptionChanged};
pub use menu::MenuService;
pub use options::{sanitize_custom_css, CustomCss, OptionsService, PermalinkPattern};
pub use post::{CreatePost, PostService, UpdatePost};
pub use user::{AuthService, AuthSession, Capability, SESSION_TTL};
