//! HTTP middleware: auth extractors and request guards.

pub mod api_key;
pub mod auth;
pub mod guards;
pub mod principal;

pub use auth::{request_context, CurrentUser, SESSION_COOKIE};
pub use principal::{MaybePrincipal, Principal};

pub mod csrf;
pub mod headers;
pub mod lockout;
pub mod rate_limit;
