//! Post domain: lifecycle rules, statuses, slugs.

pub mod revisions;
pub mod service;
pub mod status;
pub mod token;

pub use revisions::{Restored, RevisionService, Snapshot};
pub use service::{CreatePost, PostService, UpdatePost};
pub use status::{ensure_transition, transition_allowed};
