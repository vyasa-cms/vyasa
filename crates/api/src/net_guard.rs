//! The outbound-request guard lives in `vyasa_jobs` so the webhook worker
//! shares it; this re-export keeps the `crate::net_guard` paths working.

pub use vyasa_jobs::net_guard::*;
