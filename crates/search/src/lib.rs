//! Full-text search over published posts, backed by Tantivy.

pub mod index;
pub mod pipeline;

pub mod query;

pub use index::{IndexManager, SearchDoc};
pub use query::{escape_query, SearchHit};
