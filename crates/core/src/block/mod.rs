//! The block content model: types, validation, registry, documents.
//!
//! Content is stored as structured blocks (never HTML soup); rendered
//! HTML is a cache produced by the theme engine (phase 22).

pub mod doc;
pub mod registry;
pub mod types;
pub mod validate;

pub use doc::{BlockDocument, SCHEMA_VERSION};
pub use registry::{lookup, registry_v1, BlockTypeV1};
pub use types::{Block, BlockKind};
pub use validate::{is_plugin_kind_name, validate, Validated, INLINE_TAGS, MAX_BLOCKS, MAX_DEPTH};
