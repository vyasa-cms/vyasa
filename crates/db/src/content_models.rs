//! Row-model structs for content and system tables.
//!
//! Plain data carriers with sqlx row mapping; domain rules live in
//! `vyasa-core`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

/// Post content type.
///
/// Three built-in kinds plus whatever plugins registered at boot. The
/// custom variant holds a `&'static str` rather than a `String` so that
/// `PostType` stays `Copy`: it is passed by value through every repository,
/// route and service in the workspace, and making it own its name would
/// have rippled into all of them for no gain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostType {
    /// Chronological content.
    Post,
    /// Static hierarchy content.
    Page,
    /// Reusable block entity (synced patterns).
    Block,
    /// A type registered by a plugin or created by an administrator. The
    /// name is interned (see [`PostType::register`]).
    Custom(&'static str),
}

/// Who holds a custom post type name in the live registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeOwner {
    /// Declared by a plugin, at boot or when it was enabled.
    Plugin,
    /// Created by an administrator (a row of `content_types`).
    Admin,
}

/// The custom post type registry.
///
/// Two sets, because a name's string and its standing are different
/// things. `interned` is the leak-once table: interning leaks each
/// distinct name once, which is what buys the `'static` lifetime — and
/// therefore `Copy` — and a leaked string can never be taken back, since
/// copies of it live on in rows, caches and requests already in flight.
/// `live` is what a request may name, and who holds each name; an
/// administrator's type leaves it when the type is deleted and comes
/// back when it is created again, reusing the interned string, all
/// without a restart.
///
/// Only names administrators introduce are bounded
/// ([`MAX_ADMIN_TYPE_NAMES`]): they are the ones that can churn. Decoding
/// a stored row never fails for want of room, and plugins (whose names
/// are bounded by the live cap and their own declarations) always have
/// it.
struct TypeRegistry {
    interned: std::collections::BTreeSet<&'static str>,
    live: std::collections::BTreeMap<&'static str, TypeOwner>,
    /// Names first interned for an administrator's type.
    admin_names: usize,
}

impl TypeRegistry {
    /// The interned string for `name`, leaking it the first time.
    fn intern(&mut self, name: &str) -> &'static str {
        if let Some(existing) = self.interned.get(name) {
            return existing;
        }
        let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
        self.interned.insert(leaked);
        leaked
    }

    /// Whether `names` may all be made live for `owner`, without changing
    /// anything: none is held by the other owner, the live cap has room
    /// for the new ones, and (for an administrator) the name budget has
    /// room for the never-seen ones.
    fn check_live(&self, names: &[&str], owner: TypeOwner) -> Result<(), vyasa_common::AppError> {
        let mut new = std::collections::BTreeSet::new();
        for &name in names {
            match self.live.get(name) {
                Some(held_by) if *held_by == owner => {}
                Some(TypeOwner::Plugin) => {
                    return Err(vyasa_common::AppError::conflict(format!(
                        "a plugin already declares the post type {name:?}"
                    )))
                }
                Some(TypeOwner::Admin) => {
                    return Err(vyasa_common::AppError::conflict(format!(
                        "{name:?} is a content type created by an administrator"
                    )))
                }
                None => {
                    new.insert(name);
                }
            }
        }
        if self.live.len() + new.len() > MAX_CUSTOM_TYPES {
            return Err(vyasa_common::AppError::validation(format!(
                "a site may have at most {MAX_CUSTOM_TYPES} custom post types, plugins' and \
                 administrators' together"
            )));
        }
        if owner == TypeOwner::Admin {
            let unseen = new.iter().filter(|n| !self.interned.contains(**n)).count();
            if self.admin_names + unseen > MAX_ADMIN_TYPE_NAMES {
                return Err(vyasa_common::AppError::validation(format!(
                    "this server has made {MAX_ADMIN_TYPE_NAMES} different content type names \
                     since it started; restart it before adding a new one (re-creating a \
                     deleted type's name works now)"
                )));
            }
        }
        Ok(())
    }

    /// Makes every name in `names` live for `owner`, or none of them.
    /// Each comes back with whether this call made it live.
    fn make_live(
        &mut self,
        names: &[&str],
        owner: TypeOwner,
    ) -> Result<Vec<(&'static str, bool)>, vyasa_common::AppError> {
        self.check_live(names, owner)?;
        Ok(names
            .iter()
            .map(|&name| {
                if let Some((&interned, _)) = self.live.get_key_value(name) {
                    return (interned, false);
                }
                if owner == TypeOwner::Admin && !self.interned.contains(name) {
                    self.admin_names += 1;
                }
                let interned = self.intern(name);
                self.live.insert(interned, owner);
                (interned, true)
            })
            .collect())
    }

    /// Takes `owner`'s names among `names` out of the live set.
    fn unlive(&mut self, names: &[&str], owner: TypeOwner) -> usize {
        let mut removed = 0;
        for name in names {
            if self.live.get(*name) == Some(&owner) {
                self.live.remove(*name);
                removed += 1;
            }
        }
        removed
    }
}

static POST_TYPES: std::sync::RwLock<TypeRegistry> = std::sync::RwLock::new(TypeRegistry {
    interned: std::collections::BTreeSet::new(),
    live: std::collections::BTreeMap::new(),
    admin_names: 0,
});

/// Most distinct names administrators may introduce as content types in
/// one run of the server. Re-creating a name it has seen costs nothing;
/// past this, a new name needs a restart. Each leaked name is a few dozen
/// bytes, so this bounds the leak at well under a megabyte.
pub const MAX_ADMIN_TYPE_NAMES: usize = 4096;

/// Most custom names of one kind (post types, taxonomies) a site may have.
const MAX_CUSTOM_TYPES: usize = 64;

fn post_types() -> Result<std::sync::RwLockReadGuard<'static, TypeRegistry>, vyasa_common::AppError>
{
    POST_TYPES
        .read()
        .map_err(|_| vyasa_common::AppError::internal_msg("post type registry poisoned"))
}

fn post_types_mut(
) -> Result<std::sync::RwLockWriteGuard<'static, TypeRegistry>, vyasa_common::AppError> {
    POST_TYPES
        .write()
        .map_err(|_| vyasa_common::AppError::internal_msg("post type registry poisoned"))
}

/// Refuses a malformed custom name or a built-in one.
fn check_custom_type_name(name: &str) -> Result<(), vyasa_common::AppError> {
    if !is_valid_custom_type(name) {
        return Err(vyasa_common::AppError::validation(format!(
            "invalid post type name: {name:?}"
        )));
    }
    if matches!(name, "post" | "page" | "block") {
        return Err(vyasa_common::AppError::validation(format!(
            "{name:?} is a built-in post type"
        )));
    }
    Ok(())
}

impl PostType {
    /// Registers a plugin's custom type name, returning the interned type.
    ///
    /// Idempotent: registering the same name twice returns the same
    /// `&'static str`. One name of [`PostType::register_plugin_types`].
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for a malformed name,
    /// a name that collides with a built-in kind, or when the site already
    /// holds [`MAX_CUSTOM_TYPES`]; [`vyasa_common::AppError::Conflict`]
    /// when an administrator's content type holds the name.
    pub fn register(name: &str) -> Result<Self, vyasa_common::AppError> {
        Self::register_plugin_types(&[name])?
            .into_iter()
            .next()
            .ok_or_else(|| vyasa_common::AppError::internal_msg("nothing registered"))
    }

    /// Checks, without registering anything, that a plugin's types could
    /// all be registered — what enabling the plugin should ask first.
    ///
    /// # Errors
    ///
    /// As [`PostType::register_plugin_types`].
    pub fn check_plugin_types(names: &[&str]) -> Result<(), vyasa_common::AppError> {
        for name in names {
            check_custom_type_name(name)?;
        }
        post_types()?.check_live(names, TypeOwner::Plugin)
    }

    /// Registers all of a plugin's type names, or none of them, under one
    /// lock: a set refused because one name is an administrator's content
    /// type leaves nothing half-registered.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for a malformed or
    /// built-in name or when the set does not fit under the cap, and
    /// [`vyasa_common::AppError::Conflict`] naming the first name an
    /// administrator's content type holds.
    pub fn register_plugin_types(names: &[&str]) -> Result<Vec<Self>, vyasa_common::AppError> {
        for name in names {
            check_custom_type_name(name)?;
        }
        Ok(post_types_mut()?
            .make_live(names, TypeOwner::Plugin)?
            .into_iter()
            .map(|(name, _)| PostType::Custom(name))
            .collect())
    }

    /// Takes a plugin's types out of the live registry (a plugin disabled
    /// or removed). Names an administrator holds are left alone. Returns
    /// how many were taken out.
    pub fn unregister_plugin_types(names: &[&str]) -> usize {
        POST_TYPES
            .write()
            .map_or(0, |mut registry| registry.unlive(names, TypeOwner::Plugin))
    }

    /// Checks, without registering anything, that an administrator's
    /// content type could be made live: what creating one asks before it
    /// writes its row, so a refused create costs nothing.
    ///
    /// # Errors
    ///
    /// As [`PostType::register_admin`].
    pub fn check_admin(name: &str) -> Result<(), vyasa_common::AppError> {
        check_custom_type_name(name)?;
        post_types()?.check_live(&[name], TypeOwner::Admin)
    }

    /// Makes an administrator's content type live, returning it and
    /// whether this call is what made it live (`false` when it already
    /// was).
    ///
    /// # Errors
    ///
    /// As [`PostType::register`], with [`vyasa_common::AppError::Conflict`]
    /// when a plugin holds the name, and
    /// [`vyasa_common::AppError::Validation`] when the name is new and
    /// [`MAX_ADMIN_TYPE_NAMES`] have been introduced since the start.
    pub fn register_admin(name: &str) -> Result<(Self, bool), vyasa_common::AppError> {
        check_custom_type_name(name)?;
        let made = post_types_mut()?.make_live(&[name], TypeOwner::Admin)?;
        let (interned, newly) = made
            .into_iter()
            .next()
            .ok_or_else(|| vyasa_common::AppError::internal_msg("nothing registered"))?;
        Ok((PostType::Custom(interned), newly))
    }

    /// Takes an administrator's content type out of the live registry:
    /// requests can no longer name it and it is no longer listed. The
    /// interned string stays, so stored rows still decode and creating
    /// the type again reuses it. A plugin's type is never touched.
    ///
    /// Returns whether the name was live as an administrator's type.
    pub fn unregister_admin(name: &str) -> bool {
        POST_TYPES
            .write()
            .is_ok_and(|mut registry| registry.unlive(&[name], TypeOwner::Admin) == 1)
    }

    /// Whether this process has interned `name` (live or not).
    #[must_use]
    pub fn is_interned(name: &str) -> bool {
        POST_TYPES
            .read()
            .is_ok_and(|registry| registry.interned.contains(name))
    }

    /// Who holds a live custom type name, if anyone.
    #[must_use]
    pub fn owner(name: &str) -> Option<TypeOwner> {
        POST_TYPES
            .read()
            .ok()
            .and_then(|registry| registry.live.get(name).copied())
    }

    /// Every live custom type, plugins' and administrators'.
    #[must_use]
    pub fn custom_types() -> Vec<&'static str> {
        POST_TYPES
            .read()
            .ok()
            .map(|registry| registry.live.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Parses a type named by a request: built-ins plus live customs, and
    /// nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown
    /// values.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "post" => Ok(PostType::Post),
            "page" => Ok(PostType::Page),
            "block" => Ok(PostType::Block),
            other => POST_TYPES
                .read()
                .ok()
                .and_then(|registry| registry.live.get_key_value(other).map(|(name, _)| *name))
                .map(PostType::Custom)
                .ok_or_else(|| {
                    vyasa_common::AppError::validation(format!("unknown post type: {other:?}"))
                }),
        }
    }

    /// Parses a type read back out of the database.
    ///
    /// Deliberately more permissive than [`PostType::parse`]: a plugin can
    /// be uninstalled while its entries are still stored, and a site whose
    /// post list fails to decode because a plugin was removed is much worse
    /// than one showing entries of a type nothing serves any more. Only
    /// interns the name: a stored row never makes a type live.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] only for a name the
    /// column's own constraint should already have refused.
    pub fn from_db(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "post" => Ok(PostType::Post),
            "page" => Ok(PostType::Page),
            "block" => Ok(PostType::Block),
            other if is_valid_custom_type(other) => {
                Ok(PostType::Custom(post_types_mut()?.intern(other)))
            }
            other => Err(vyasa_common::AppError::validation(format!(
                "unknown post type: {other:?}"
            ))),
        }
    }

    /// Returns the database string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PostType::Post => "post",
            PostType::Page => "page",
            PostType::Block => "block",
            PostType::Custom(name) => name,
        }
    }

    /// Whether this is a plugin-registered type.
    #[must_use]
    pub const fn is_custom(self) -> bool {
        matches!(self, PostType::Custom(_))
    }
}

/// The name grammar, matching the column's own constraint.
#[must_use]
pub fn is_valid_custom_type(name: &str) -> bool {
    (2..=32).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The taxonomy interner (post types have [`TypeRegistry`]).
///
/// Leaking is what buys the `'static` lifetime, and therefore `Copy`. It
/// is bounded, and registration happens once per name at boot, so this is
/// a fixed one-time cost rather than something that grows with traffic.
fn intern_in(
    registry: &'static std::sync::RwLock<Option<std::collections::BTreeSet<&'static str>>>,
    name: &str,
) -> Result<&'static str, vyasa_common::AppError> {
    let mut guard = registry
        .write()
        .map_err(|_| vyasa_common::AppError::internal_msg("name registry poisoned"))?;
    let set = guard.get_or_insert_with(std::collections::BTreeSet::new);
    if let Some(existing) = set.get(name) {
        return Ok(existing);
    }
    if set.len() >= MAX_CUSTOM_TYPES {
        return Err(vyasa_common::AppError::validation(format!(
            "a site may register at most {MAX_CUSTOM_TYPES} custom names of one kind"
        )));
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    set.insert(leaked);
    Ok(leaked)
}

impl Serialize for PostType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PostType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        PostType::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl sqlx::Type<sqlx::Postgres> for PostType {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <str as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <str as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for PostType {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(self.as_str(), buf)
    }
}

impl sqlx::Decode<'_, sqlx::Postgres> for PostType {
    fn decode(value: sqlx::postgres::PgValueRef<'_>) -> Result<Self, sqlx::error::BoxDynError> {
        let raw = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        PostType::from_db(raw).map_err(|e| Box::new(e) as sqlx::error::BoxDynError)
    }
}

/// Post lifecycle status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PostStatus {
    /// Unpublished working copy.
    Draft,
    /// Waiting for `scheduled_for`.
    Scheduled,
    /// Publicly visible.
    Published,
    /// Visible only to permitted readers (possibly password-protected).
    Private,
    /// Soft-deleted; slug released for reuse.
    Trash,
}

impl PostStatus {
    /// Parses the database string form.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown
    /// values.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "draft" => Ok(PostStatus::Draft),
            "scheduled" => Ok(PostStatus::Scheduled),
            "published" => Ok(PostStatus::Published),
            "private" => Ok(PostStatus::Private),
            "trash" => Ok(PostStatus::Trash),
            other => Err(vyasa_common::AppError::validation(format!(
                "unknown post status: {other:?}"
            ))),
        }
    }

    /// Returns the database string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PostStatus::Draft => "draft",
            PostStatus::Scheduled => "scheduled",
            PostStatus::Published => "published",
            PostStatus::Private => "private",
            PostStatus::Trash => "trash",
        }
    }
}

/// A row of the `posts` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct PostRow {
    /// Snowflake id.
    pub id: i64,
    /// Content type.
    #[serde(rename = "type")]
    pub post_type: PostType,
    /// Lifecycle status.
    pub status: PostStatus,
    /// URL slug (unique per type while not trashed).
    pub slug: String,
    /// Title.
    pub title: String,
    /// Structured block content (JSONB array).
    pub content: serde_json::Value,
    /// Section tree composing this entry, when it has one. `None` — the
    /// case for every post and most pages — renders through the theme's
    /// template instead.
    pub layout: Option<serde_json::Value>,
    /// Optional summary.
    pub excerpt: Option<String>,
    /// Authoring user.
    pub author_id: i64,
    /// Parent (pages, block nesting).
    pub parent_id: Option<i64>,
    /// Extension metadata.
    pub meta: serde_json::Value,
    /// First publication time.
    pub published_at: Option<DateTime<Utc>>,
    /// Scheduled publication time.
    pub scheduled_for: Option<DateTime<Utc>>,
    /// Pinned to the top of the home listing.
    #[sqlx(default)]
    pub sticky: bool,
    /// BCP 47 tag the entry is written in; empty means the site's.
    #[sqlx(default)]
    pub lang: String,
    /// Entries sharing a group are translations of one another.
    #[sqlx(default)]
    pub translation_group: Option<i64>,
    /// Argon2 hash of the access password, for password-protected posts.
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

/// A row of the `post_revisions` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct PostRevisionRow {
    /// Snowflake id.
    pub id: i64,
    /// Revised post.
    pub post_id: i64,
    /// Snapshot title.
    pub title: String,
    /// Snapshot block content.
    pub content: serde_json::Value,
    /// Snapshot section tree, so a revert restores the arrangement along
    /// with the words.
    pub layout: Option<serde_json::Value>,
    /// Snapshot field values (`meta.fields`). `None` for a revision that
    /// predates custom fields: restoring it keeps the entry's values.
    /// Defaulted so a query that does not select the column still decodes.
    #[sqlx(default)]
    pub fields: Option<serde_json::Value>,
    /// Snapshot author.
    pub author_id: i64,
    /// Whether this snapshot is an editor autosave.
    pub is_autosave: bool,
    /// Snapshot time.
    pub created_at: DateTime<Utc>,
}

/// Term taxonomy.
/// Term taxonomy: the two built in, plus whatever plugins registered.
///
/// Open the same way [`PostType`] is, and for the same reason — see
/// [`PostType::register`] for why the custom name is interned rather than
/// owned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Taxonomy {
    /// Hierarchical categories.
    Category,
    /// Flat tags.
    Tag,
    /// A taxonomy registered by a plugin.
    Custom(&'static str),
}

/// Interned custom taxonomy names.
static CUSTOM_TAXONOMIES: std::sync::RwLock<Option<std::collections::BTreeSet<&'static str>>> =
    std::sync::RwLock::new(None);

impl Taxonomy {
    /// Registers a custom taxonomy name, returning the interned taxonomy.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for a malformed name,
    /// a collision with a built-in one, or when the site is already at its
    /// limit.
    pub fn register(name: &str) -> Result<Self, vyasa_common::AppError> {
        if !is_valid_custom_type(name) {
            return Err(vyasa_common::AppError::validation(format!(
                "invalid taxonomy name: {name:?}"
            )));
        }
        if matches!(name, "category" | "tag") {
            return Err(vyasa_common::AppError::validation(format!(
                "{name:?} is a built-in taxonomy"
            )));
        }
        Ok(Taxonomy::Custom(intern_in(&CUSTOM_TAXONOMIES, name)?))
    }

    /// Every registered custom taxonomy.
    #[must_use]
    pub fn custom_taxonomies() -> Vec<&'static str> {
        CUSTOM_TAXONOMIES
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|set| set.iter().copied().collect()))
            .unwrap_or_default()
    }

    /// Parses a taxonomy named by a request: built-ins plus registered
    /// customs, and nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown
    /// values.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "category" => Ok(Taxonomy::Category),
            "tag" => Ok(Taxonomy::Tag),
            other => CUSTOM_TAXONOMIES
                .read()
                .ok()
                .and_then(|g| g.as_ref().and_then(|set| set.get(other).copied()))
                .map(Taxonomy::Custom)
                .ok_or_else(|| {
                    vyasa_common::AppError::validation(format!("unknown taxonomy: {other:?}"))
                }),
        }
    }

    /// Parses a taxonomy read back out of the database.
    ///
    /// Laxer than [`Taxonomy::parse`] for the same reason
    /// [`PostType::from_db`] is: terms outlive the plugin that registered
    /// their taxonomy.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] only for a name the
    /// column's own constraint should already have refused.
    pub fn from_db(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "category" => Ok(Taxonomy::Category),
            "tag" => Ok(Taxonomy::Tag),
            other if is_valid_custom_type(other) => {
                Ok(Taxonomy::Custom(intern_in(&CUSTOM_TAXONOMIES, other)?))
            }
            other => Err(vyasa_common::AppError::validation(format!(
                "unknown taxonomy: {other:?}"
            ))),
        }
    }

    /// Returns the database string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Taxonomy::Category => "category",
            Taxonomy::Tag => "tag",
            Taxonomy::Custom(name) => name,
        }
    }

    /// Whether this is a plugin-registered taxonomy.
    #[must_use]
    pub const fn is_custom(self) -> bool {
        matches!(self, Taxonomy::Custom(_))
    }
}

impl Serialize for Taxonomy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Taxonomy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Taxonomy::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl sqlx::Type<sqlx::Postgres> for Taxonomy {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <str as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <str as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Taxonomy {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(self.as_str(), buf)
    }
}

impl sqlx::Decode<'_, sqlx::Postgres> for Taxonomy {
    fn decode(value: sqlx::postgres::PgValueRef<'_>) -> Result<Self, sqlx::error::BoxDynError> {
        let raw = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Taxonomy::from_db(raw).map_err(|e| Box::new(e) as sqlx::error::BoxDynError)
    }
}

/// A row of the `terms` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct TermRow {
    /// Snowflake id.
    pub id: i64,
    /// Which taxonomy this term belongs to.
    pub taxonomy: Taxonomy,
    /// Human-readable name.
    pub name: String,
    /// Slug (unique per taxonomy).
    pub slug: String,
    /// Parent term for hierarchies.
    pub parent_id: Option<i64>,
    /// Extension metadata.
    pub meta: serde_json::Value,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// A row of the `comments` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct CommentRow {
    /// Snowflake id.
    pub id: i64,
    /// Commented post.
    pub post_id: i64,
    /// Commenting user, for logged-in commenters.
    pub author_user_id: Option<i64>,
    /// Display name (required for guests).
    pub author_name: String,
    /// Email (moderation contact).
    #[serde(skip_serializing)]
    pub author_email: String,
    /// Comment body.
    pub content: String,
    /// Parent comment for threading.
    pub parent_id: Option<i64>,
    /// Moderation status.
    pub status: CommentStatus,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Comment moderation status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum CommentStatus {
    /// Awaiting moderation.
    Pending,
    /// Publicly visible.
    Approved,
    /// Marked as spam.
    Spam,
    /// Soft-deleted.
    Trash,
}

impl CommentStatus {
    /// Parses the database string form.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown
    /// values.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "pending" => Ok(CommentStatus::Pending),
            "approved" => Ok(CommentStatus::Approved),
            "spam" => Ok(CommentStatus::Spam),
            "trash" => Ok(CommentStatus::Trash),
            other => Err(vyasa_common::AppError::validation(format!(
                "unknown comment status: {other:?}"
            ))),
        }
    }

    /// Returns the database string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CommentStatus::Pending => "pending",
            CommentStatus::Approved => "approved",
            CommentStatus::Spam => "spam",
            CommentStatus::Trash => "trash",
        }
    }
}

/// A row of the `media` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct MediaRow {
    /// Snowflake id.
    pub id: i64,
    /// Uploading user.
    pub owner_id: i64,
    /// Original file name.
    pub file_name: String,
    /// Sniffed MIME type.
    pub mime: String,
    /// Size in bytes.
    pub byte_size: i64,
    /// Where the bytes live.
    pub storage: MediaStorage,
    /// Storage-relative path.
    pub path: String,
    /// Pixel width, when applicable.
    pub width: Option<i32>,
    /// Pixel height, when applicable.
    pub height: Option<i32>,
    /// Blurhash placeholder for images.
    pub blurhash: Option<String>,
    /// Accessibility text.
    pub alt: Option<String>,
    /// In the trash since; `None` in the library.
    #[sqlx(default)]
    pub trashed_at: Option<DateTime<Utc>>,
    /// Caption.
    pub caption: Option<String>,
    /// Derived image variants (phase 16).
    pub derivatives: serde_json::Value,
    /// Upload time.
    pub created_at: DateTime<Utc>,
    /// Hex SHA-256 of the original bytes; `None` for rows from before it
    /// was recorded.
    pub sha256: Option<String>,
    /// Focal point as a fraction of width, for cropped renderings.
    pub focal_x: Option<f32>,
    /// Focal point as a fraction of height.
    pub focal_y: Option<f32>,
}

/// Media byte storage backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MediaStorage {
    /// Local filesystem.
    Local,
    /// S3-compatible object storage.
    S3,
}

impl MediaStorage {
    /// Parses the database string form.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown
    /// values.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "local" => Ok(MediaStorage::Local),
            "s3" => Ok(MediaStorage::S3),
            other => Err(vyasa_common::AppError::validation(format!(
                "unknown media storage: {other:?}"
            ))),
        }
    }

    /// Returns the database string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MediaStorage::Local => "local",
            MediaStorage::S3 => "s3",
        }
    }
}

/// A row of the `themes` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct ThemeRow {
    /// Snowflake id.
    pub id: i64,
    /// Theme name.
    pub name: String,
    /// Version within the name (bumped per install).
    pub version: i32,
    /// Whether this version is the site's active theme.
    pub is_active: bool,
    /// Design tokens (validated by the themes crate).
    pub tokens: serde_json::Value,
    /// Block layout composition.
    pub layout: serde_json::Value,
    /// Optional Tera templates (name → source).
    pub templates: Option<serde_json::Value>,
    /// Parent theme for child themes.
    pub parent_theme_id: Option<i64>,
    /// Install time.
    pub created_at: DateTime<Utc>,
}

/// A row of the `plugins` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct PluginRow {
    /// Snowflake id.
    pub id: i64,
    /// Plugin name (unique).
    pub name: String,
    /// Installed version.
    pub version: String,
    /// Whether the plugin participates in dispatch.
    pub enabled: bool,
    /// Runtime health.
    pub status: PluginStatus,
    /// Declared capabilities.
    pub capabilities: serde_json::Value,
    /// SHA-256 of the active wasm bytes.
    pub wasm_sha256: String,
    /// First install time.
    pub created_at: DateTime<Utc>,
}

/// Plugin runtime status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    /// Uploaded, never loaded.
    Installed,
    /// Loaded and dispatching.
    Loaded,
    /// Loaded but recently failing.
    Degraded,
    /// Failed hard; excluded from dispatch.
    Errored,
    /// Disabled by an administrator.
    Disabled,
}

/// A row of the `plugin_versions` table (without wasm bytes).
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct PluginVersionRow {
    /// Snowflake id.
    pub id: i64,
    /// Owning plugin.
    pub plugin_id: i64,
    /// Version string.
    pub version: String,
    /// SHA-256 of the wasm bytes.
    pub sha256: String,
    /// Install time.
    pub created_at: DateTime<Utc>,
}

/// A row of the `jobs` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct JobRow {
    /// Snowflake id.
    pub id: i64,
    /// Worker kind discriminator.
    pub kind: String,
    /// Kind-specific payload.
    pub payload: serde_json::Value,
    /// Earliest execution time.
    pub run_at: DateTime<Utc>,
    /// Queue state.
    pub status: JobStatus,
    /// Execution attempts so far.
    pub attempts: i32,
    /// Last failure description.
    pub last_error: Option<String>,
    /// Enqueue time.
    pub created_at: DateTime<Utc>,
}

/// Job queue state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Waiting for a worker.
    Queued,
    /// Claimed by a worker.
    Running,
    /// Completed successfully.
    Done,
    /// Failed, pending retry.
    Failed,
    /// Retries exhausted.
    Dead,
}

impl JobStatus {
    /// Parses the database string form.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown
    /// values.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "queued" => Ok(JobStatus::Queued),
            "running" => Ok(JobStatus::Running),
            "done" => Ok(JobStatus::Done),
            "failed" => Ok(JobStatus::Failed),
            "dead" => Ok(JobStatus::Dead),
            other => Err(vyasa_common::AppError::validation(format!(
                "unknown job status: {other:?}"
            ))),
        }
    }

    /// Returns the database string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Dead => "dead",
        }
    }
}

/// A row of the `webhooks` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct WebhookRow {
    /// Snowflake id.
    pub id: i64,
    /// Delivery target URL.
    pub url: String,
    /// HMAC signing secret.
    #[serde(skip_serializing)]
    pub secret: String,
    /// Subscribed event names.
    pub events: serde_json::Value,
    /// Whether deliveries are enabled.
    pub enabled: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// A row of the `ai_logs` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct AiLogRow {
    /// Snowflake id.
    pub id: i64,
    /// Provider used.
    pub provider: String,
    /// Model used.
    pub model: String,
    /// Feature purpose tag.
    pub purpose: String,
    /// Prompt token count.
    pub prompt_tokens: i32,
    /// Completion token count.
    pub completion_tokens: i32,
    /// Computed cost in USD.
    pub cost_usd: f64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::{CommentStatus, JobStatus, MediaStorage, PostStatus, PostType, Taxonomy};

    #[test]
    fn enums_round_trip_with_database_form() {
        let post_types = [PostType::Post, PostType::Page, PostType::Block];
        for value in post_types {
            let parsed = PostType::parse(value.as_str())
                .ok()
                .unwrap_or(PostType::Post);
            assert_eq!(parsed, value);
        }
        let statuses = [
            PostStatus::Draft,
            PostStatus::Scheduled,
            PostStatus::Published,
            PostStatus::Private,
            PostStatus::Trash,
        ];
        for value in statuses {
            let parsed = PostStatus::parse(value.as_str())
                .ok()
                .unwrap_or(PostStatus::Draft);
            assert_eq!(parsed, value);
        }
        for value in [Taxonomy::Category, Taxonomy::Tag] {
            let parsed = Taxonomy::parse(value.as_str())
                .ok()
                .unwrap_or(Taxonomy::Category);
            assert_eq!(parsed, value);
        }
        for value in [
            CommentStatus::Pending,
            CommentStatus::Approved,
            CommentStatus::Spam,
            CommentStatus::Trash,
        ] {
            let parsed = CommentStatus::parse(value.as_str())
                .ok()
                .unwrap_or(CommentStatus::Pending);
            assert_eq!(parsed, value);
        }
        for value in [MediaStorage::Local, MediaStorage::S3] {
            let parsed = MediaStorage::parse(value.as_str())
                .ok()
                .unwrap_or(MediaStorage::Local);
            assert_eq!(parsed, value);
        }
        for value in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Done,
            JobStatus::Failed,
            JobStatus::Dead,
        ] {
            let parsed = JobStatus::parse(value.as_str())
                .ok()
                .unwrap_or(JobStatus::Queued);
            assert_eq!(parsed, value);
        }
    }

    #[test]
    fn enum_parse_rejects_unknown_values() {
        assert!(PostType::parse("snippet").is_err());
        assert!(PostStatus::parse("archived").is_err());
        assert!(Taxonomy::parse("format").is_err());
        assert!(CommentStatus::parse("deleted").is_err());
        assert!(JobStatus::parse("paused").is_err());
    }
}

#[cfg(test)]
mod open_enum_tests {
    use super::{is_valid_custom_type, PostType, Taxonomy, TypeOwner};
    use vyasa_common::AppError;

    /// Registration is process-global and additive, so each test uses its
    /// own names rather than trying to isolate the registry.
    #[test]
    fn a_registered_type_parses_and_an_unregistered_one_does_not() {
        assert!(
            PostType::parse("widget").is_err(),
            "nothing is registered until it is"
        );
        let widget = PostType::register("widget").expect("registers");
        assert_eq!(widget, PostType::Custom("widget"));
        assert_eq!(widget.as_str(), "widget");
        assert!(widget.is_custom());
        assert_eq!(PostType::parse("widget").expect("now known"), widget);

        // Registering twice is idempotent and hands back the same interned
        // string, which is what keeps `PostType` `Copy`.
        let again = PostType::register("widget").expect("idempotent");
        assert_eq!(again, widget);
        assert!(std::ptr::eq(again.as_str(), widget.as_str()));

        assert!(PostType::custom_types().contains(&"widget"));
    }

    #[test]
    fn built_ins_are_not_available_to_plugins() {
        for name in ["post", "page", "block"] {
            let err = PostType::register(name).unwrap_err().to_string();
            assert!(err.contains("built-in"), "{name}: {err}");
        }
        for name in ["category", "tag"] {
            let err = Taxonomy::register(name).unwrap_err().to_string();
            assert!(err.contains("built-in"), "{name}: {err}");
        }
    }

    #[test]
    fn malformed_names_are_refused_before_anything_is_interned() {
        for bad in [
            "",
            "x",
            "-lead",
            "trail-",
            "Upper",
            "under_score",
            "has space",
            "sl/ash",
            "../etc",
            &"x".repeat(33),
        ] {
            assert!(!is_valid_custom_type(bad), "grammar accepted {bad:?}");
            assert!(PostType::register(bad).is_err(), "post type took {bad:?}");
            assert!(Taxonomy::register(bad).is_err(), "taxonomy took {bad:?}");
        }
        assert!(is_valid_custom_type("ab"));
        assert!(is_valid_custom_type("case-study-2"));
    }

    #[test]
    fn decoding_is_laxer_than_parsing_so_rows_outlive_their_plugin() {
        // The point: a plugin can be uninstalled while its entries are
        // still stored. `parse` (request input) refuses an unregistered
        // name; `from_db` (a stored row) accepts a well-formed one, so the
        // post list does not fail to decode because a plugin was removed.
        assert!(PostType::parse("gone-away").is_err());
        let decoded = PostType::from_db("gone-away").expect("stored rows still decode");
        assert_eq!(decoded.as_str(), "gone-away");

        assert!(Taxonomy::parse("vanished").is_err());
        assert_eq!(
            Taxonomy::from_db("vanished").expect("decodes").as_str(),
            "vanished"
        );

        // But a name the column's own constraint would have refused is
        // still an error, not silently accepted.
        assert!(PostType::from_db("Not A Slug").is_err());
        assert!(Taxonomy::from_db("Not A Slug").is_err());
    }

    #[test]
    fn serde_keeps_the_flat_string_wire_format() {
        PostType::register("gadget").expect("registers");
        Taxonomy::register("mood").expect("registers");

        // A data-carrying variant would serialize as `{"custom": "..."}`
        // under a derived impl; the wire format is a bare string and has
        // to stay one.
        for (value, expected) in [
            (PostType::Post, "\"post\""),
            (PostType::Custom("gadget"), "\"gadget\""),
        ] {
            let json = serde_json::to_string(&value).expect("serializes");
            assert_eq!(json, expected);
            let back: PostType = serde_json::from_str(&json).expect("round trips");
            assert_eq!(back, value);
        }
        let json = serde_json::to_string(&Taxonomy::Custom("mood")).expect("serializes");
        assert_eq!(json, "\"mood\"");
        let back: Taxonomy = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back, Taxonomy::Custom("mood"));

        // Deserializing goes through `parse`, so an unregistered name is a
        // rejected request rather than a silently invented type.
        assert!(serde_json::from_str::<PostType>("\"never-registered\"").is_err());
    }

    #[test]
    fn post_types_and_taxonomies_have_separate_namespaces() {
        // The two registries are independent: `series` may be both a post
        // type and a taxonomy as far as these types are concerned. The
        // rule that they may not collide in one *site* lives with the
        // router, which is what actually owns the URL namespace.
        let as_type = PostType::register("series").expect("post type");
        let as_tax = Taxonomy::register("series").expect("taxonomy");
        assert_eq!(as_type.as_str(), as_tax.as_str());
        assert!(PostType::parse("series").is_ok());
        assert!(Taxonomy::parse("series").is_ok());
    }

    #[test]
    fn an_admin_type_is_live_until_it_is_unregistered_and_keeps_its_name() {
        assert!(PostType::parse("cm-alpha").is_err());
        let (alpha, newly) = PostType::register_admin("cm-alpha").expect("registers");
        assert!(newly, "the first registration makes it live");
        assert_eq!(PostType::parse("cm-alpha").expect("live"), alpha);
        assert!(PostType::custom_types().contains(&"cm-alpha"));
        assert_eq!(PostType::owner("cm-alpha"), Some(TypeOwner::Admin));

        // Again is idempotent, and says it was already live.
        let (again, newly) = PostType::register_admin("cm-alpha").expect("idempotent");
        assert!(!newly);
        assert!(std::ptr::eq(again.as_str(), alpha.as_str()));

        // Unregistering takes it out of everything a request can reach,
        // without a restart.
        assert!(PostType::unregister_admin("cm-alpha"));
        assert!(PostType::parse("cm-alpha").is_err());
        assert!(!PostType::custom_types().contains(&"cm-alpha"));
        assert_eq!(PostType::owner("cm-alpha"), None);
        assert!(!PostType::unregister_admin("cm-alpha"), "already gone");

        // Stored rows of it still decode, and creating it again reuses the
        // interned string rather than leaking another.
        assert_eq!(PostType::from_db("cm-alpha").expect("decodes"), alpha);
        let (back, newly) = PostType::register_admin("cm-alpha").expect("again");
        assert!(newly);
        assert!(std::ptr::eq(back.as_str(), alpha.as_str()));
        PostType::unregister_admin("cm-alpha");
    }

    #[test]
    fn a_name_has_one_owner_plugin_or_administrator() {
        PostType::register("cm-plug").expect("plugin type");
        let err = PostType::register_admin("cm-plug").unwrap_err();
        assert!(matches!(err, AppError::Conflict { .. }), "{err}");
        assert_eq!(PostType::owner("cm-plug"), Some(TypeOwner::Plugin));
        // Unregistering an administrator's type never touches a plugin's.
        assert!(!PostType::unregister_admin("cm-plug"));
        assert!(PostType::parse("cm-plug").is_ok());

        PostType::register_admin("cm-adm").expect("admin type");
        let err = PostType::register("cm-adm").unwrap_err();
        assert!(matches!(err, AppError::Conflict { .. }), "{err}");
        assert_eq!(PostType::owner("cm-adm"), Some(TypeOwner::Admin));
        PostType::unregister_admin("cm-adm");
    }

    #[test]
    fn admin_names_follow_the_same_grammar_and_built_in_rules() {
        for bad in [
            "post",
            "page",
            "block",
            "x",
            "trail-",
            "Upper",
            &"x".repeat(33),
        ] {
            assert!(PostType::register_admin(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn decoding_a_stored_name_does_not_make_it_routable() {
        // A row of an uninstalled plugin's type decodes, but that must not
        // turn the name into something a request may name.
        assert_eq!(
            PostType::from_db("cm-stale").expect("decodes").as_str(),
            "cm-stale"
        );
        assert!(PostType::parse("cm-stale").is_err());
        assert!(!PostType::custom_types().contains(&"cm-stale"));
        assert_eq!(PostType::owner("cm-stale"), None);
    }

    #[test]
    fn a_plugins_types_register_all_or_nothing_and_unregister_together() {
        PostType::register_admin("cm-held").expect("admin type");
        // One name an administrator holds refuses the whole set, and the
        // dry run says the same without registering anything.
        let err = PostType::check_plugin_types(&["cm-pa", "cm-held"]).unwrap_err();
        assert!(matches!(err, AppError::Conflict { .. }), "{err}");
        assert!(err.to_string().contains("cm-held"), "{err}");
        let err = PostType::register_plugin_types(&["cm-pa", "cm-held"]).unwrap_err();
        assert!(matches!(err, AppError::Conflict { .. }), "{err}");
        assert_eq!(PostType::owner("cm-pa"), None, "nothing half-registered");
        assert!(PostType::register_plugin_types(&["cm-pa", "Bad Name"]).is_err());
        assert_eq!(PostType::owner("cm-pa"), None);

        PostType::check_plugin_types(&["cm-pa", "cm-pb"]).expect("dry run");
        assert_eq!(
            PostType::owner("cm-pa"),
            None,
            "a dry run registers nothing"
        );
        let types = PostType::register_plugin_types(&["cm-pa", "cm-pb"]).expect("both");
        assert_eq!(types.len(), 2);
        assert_eq!(PostType::owner("cm-pb"), Some(TypeOwner::Plugin));

        // Unregistering a plugin's types leaves an administrator's alone.
        assert_eq!(
            PostType::unregister_plugin_types(&["cm-pa", "cm-pb", "cm-held"]),
            2
        );
        assert!(PostType::parse("cm-pa").is_err());
        assert_eq!(PostType::owner("cm-held"), Some(TypeOwner::Admin));
        PostType::unregister_admin("cm-held");
    }

    #[test]
    fn decoding_rows_of_never_seen_type_names_never_errors() {
        // Rows are decoded whatever this process has seen before: more
        // distinct names than any ceiling the interner ever had.
        for n in 0..600 {
            let name = format!("cm-row-{n}");
            assert_eq!(
                PostType::from_db(&name).expect("decodes").as_str(),
                name.as_str()
            );
        }
        assert!(PostType::is_interned("cm-row-599"));
        assert!(PostType::parse("cm-row-599").is_err(), "and none is live");
    }

    #[test]
    fn churning_admin_types_past_the_old_ceiling_still_allows_new_ones() {
        for n in 0..300 {
            let name = format!("cm-churn-{n}");
            PostType::register_admin(&name).expect("create");
            assert!(PostType::unregister_admin(&name), "delete");
        }
        PostType::register_admin("cm-churn-new").expect("a new type still fits");
        PostType::unregister_admin("cm-churn-new");
        // And a plugin can always register.
        PostType::register("cm-churn-plugin").expect("plugin type");
        assert_eq!(PostType::unregister_plugin_types(&["cm-churn-plugin"]), 1);
    }
}
