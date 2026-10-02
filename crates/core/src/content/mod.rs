//! Content types and custom fields managed from the admin (phase 99).
//!
//! [`types`] owns administrator-created post types and their place in the
//! live post type registry; [`fields`] owns field definitions on any type
//! and the validation of the values entries store in `meta.fields`.

pub mod fields;
pub mod types;

pub use fields::{
    is_allowed_url, is_site_relative_url, ContentFieldsService, FieldChanges, FieldDef, FieldKind,
    NewField,
};
/// Changes to content types, and field creates, take turns in this
/// process: the registry and the tables change together, and a field
/// cannot be added to a type while that type is being deleted.
pub(crate) static CONTENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub use types::{ContentTypesService, NewType, TypeChanges, RESERVED_TYPE_SLUGS};
