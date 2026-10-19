//! Post domain: lifecycle rules, statuses, slugs.

pub mod hierarchy;
pub mod revisions;
pub mod service;
pub mod status;
pub mod token;

pub use revisions::{Restored, RevisionService, Snapshot};
pub use service::{CreatePost, MoveOutcome, PostService, UpdatePost};
pub use status::{ensure_transition, transition_allowed};
