//! Content types and field values as every surface sees them (phase 99).
//!
//! Two questions recur wherever an entry goes — the router, bindings,
//! search, the sitemap, the vocabulary — and are answered here once:
//!
//! - **Which custom types are served?** Plugins declare theirs on the
//!   plugin surface; administrators create theirs in `content_types`. Both
//!   live in one registry ([`PostType`]); [`served_type`] and
//!   [`served_types`] read the declaration (labels, flags) from whichever
//!   owner holds the slug. Nothing here is cached, so a type created,
//!   relabelled or deleted takes effect on the next request.
//! - **What does an entry's field hold, for this reader?** The stored
//!   values live in `posts.meta.fields`. Only values of fields that are
//!   still defined are shown or served; references to media and entries
//!   are resolved for templates, and anything that no longer resolves for
//!   the public (deleted, trashed, not published, protected, of a type no
//!   longer served) renders as nothing. URL values are re-checked at
//!   render time.

use std::collections::HashMap;

use serde_json::{Map, Value};
use sqlx::PgPool;
use vyasa_core::content::{is_allowed_url, ContentFieldsService, FieldDef, FieldKind};
use vyasa_db::content_models::{PostRow, PostStatus, PostType, TypeOwner};
use vyasa_db::repo::{ContentTypesRepo, MediaRepo, PostsRepo};

use crate::plugin_surface::PluginSurface;

/// A custom post type as the router and listings need it, whoever owns it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TypeDecl {
    /// URL and storage slug.
    pub slug: String,
    /// Singular label.
    pub singular: String,
    /// Plural label.
    pub plural: String,
    /// Whether entries get public URLs.
    pub public: bool,
    /// Whether `/{slug}` lists them.
    pub has_archive: bool,
    /// Created by an administrator (rather than declared by a plugin).
    pub admin: bool,
}

impl From<crate::plugin_surface::PostTypeDecl> for TypeDecl {
    fn from(d: crate::plugin_surface::PostTypeDecl) -> Self {
        Self {
            slug: d.slug,
            singular: d.singular,
            plural: d.plural,
            public: d.public,
            has_archive: d.has_archive,
            admin: false,
        }
    }
}

impl From<vyasa_db::repo::ContentTypeRow> for TypeDecl {
    fn from(r: vyasa_db::repo::ContentTypeRow) -> Self {
        Self {
            slug: r.slug,
            singular: r.singular,
            plural: r.plural,
            public: r.public,
            has_archive: r.has_archive,
            admin: true,
        }
    }
}

/// The live custom type `slug`, from its plugin or its administrator.
///
/// The in-memory registry is asked first, so a slug nobody holds costs no
/// query (this runs on every `/{slug}` request).
pub async fn served_type(surface: &PluginSurface, pool: &PgPool, slug: &str) -> Option<TypeDecl> {
    match PostType::owner(slug)? {
        TypeOwner::Plugin => surface.post_type(slug).await.map(TypeDecl::from),
        TypeOwner::Admin => ContentTypesRepo::new(pool.clone())
            .get(slug)
            .await
            .ok()
            .map(TypeDecl::from),
    }
}

/// Every live custom type: plugins' first (in slug order), then
/// administrators'. A stored administrator type that is not live (its
/// slug became reserved, or no room under the cap at boot) is left out,
/// as the router leaves it out.
pub async fn served_types(surface: &PluginSurface, pool: &PgPool) -> Vec<TypeDecl> {
    let mut out: Vec<TypeDecl> = surface
        .post_types()
        .await
        .into_iter()
        .map(TypeDecl::from)
        .collect();
    let admin = ContentTypesRepo::new(pool.clone())
        .list()
        .await
        .unwrap_or_default();
    out.extend(
        admin
            .into_iter()
            .filter(|r| PostType::owner(&r.slug) == Some(TypeOwner::Admin))
            .map(TypeDecl::from),
    );
    out
}

/// Whether the public may see entries of `post_type` at their own URL.
pub async fn publicly_served(surface: &PluginSurface, pool: &PgPool, post_type: PostType) -> bool {
    match post_type {
        PostType::Post | PostType::Page => true,
        PostType::Block => false,
        PostType::Custom(slug) => served_type(surface, pool, slug)
            .await
            .is_some_and(|d| d.public),
    }
}

/// Field definitions per type, read once per request or batch.
#[derive(Default)]
pub struct Definitions(HashMap<&'static str, Vec<FieldDef>>);

impl Definitions {
    /// The definitions of `post_type` (empty for a type without fields,
    /// or when they cannot be read: a surface showing an entry degrades
    /// to showing no fields rather than failing).
    pub async fn of(&mut self, pool: &PgPool, post_type: PostType) -> &[FieldDef] {
        let key = post_type.as_str();
        if !self.0.contains_key(key) {
            let defs = ContentFieldsService::new(pool.clone())
                .definitions(post_type)
                .await
                .unwrap_or_default();
            self.0.insert(key, defs);
        }
        self.0.get(key).map_or(&[], Vec::as_slice)
    }

    /// The definitions of `post_type` already read with [`Self::of`]
    /// (empty when they were not).
    #[must_use]
    pub fn cached(&self, post_type: PostType) -> &[FieldDef] {
        self.0.get(post_type.as_str()).map_or(&[], Vec::as_slice)
    }
}

/// The values an entry stores, as an object (`{}` when it has none).
#[must_use]
pub fn stored(meta: &Value) -> Map<String, Value> {
    meta.get("fields")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The stored values of fields that are still defined, in definition
/// order. A deleted field's values stay stored but are not served.
#[must_use]
pub fn defined_values(defs: &[FieldDef], meta: &Value) -> Map<String, Value> {
    let values = stored(meta);
    defs.iter()
        .filter_map(|d| values.get(&d.key).map(|v| (d.key.clone(), v.clone())))
        .collect()
}

/// `meta` without its `fields` key: values travel as `fields` beside it,
/// never twice.
#[must_use]
pub fn meta_without_fields(mut meta: Value) -> Value {
    if let Some(obj) = meta.as_object_mut() {
        obj.remove("fields");
    }
    meta
}

/// Field definitions per type shared by every resolver of one request
/// (GraphQL): a page of entries reads each type's definitions once.
#[derive(Default)]
pub struct SharedDefinitions {
    inner: tokio::sync::Mutex<HashMap<&'static str, std::sync::Arc<Vec<FieldDef>>>>,
    loads: std::sync::atomic::AtomicUsize,
}

impl SharedDefinitions {
    /// The definitions of `post_type`, read from the database the first
    /// time this request asks.
    pub async fn of(&self, pool: &PgPool, post_type: PostType) -> std::sync::Arc<Vec<FieldDef>> {
        let key = post_type.as_str();
        let mut map = self.inner.lock().await;
        if let Some(defs) = map.get(key) {
            return std::sync::Arc::clone(defs);
        }
        self.loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let defs = std::sync::Arc::new(
            ContentFieldsService::new(pool.clone())
                .definitions(post_type)
                .await
                .unwrap_or_default(),
        );
        map.insert(key, std::sync::Arc::clone(&defs));
        defs
    }

    /// How many types were read from the database (a test's proof that
    /// the cache is shared).
    #[cfg(test)]
    pub fn loads(&self) -> usize {
        self.loads.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The media and entry ids an entry's reference fields hold.
fn reference_ids(defs: &[FieldDef], meta: &Value) -> (Vec<i64>, Vec<i64>) {
    let values = stored(meta);
    let mut media = Vec::new();
    let mut entries = Vec::new();
    for def in defs {
        let Some(id) = values.get(&def.key).and_then(id_of) else {
            continue;
        };
        match def.kind {
            FieldKind::Media => media.push(id),
            FieldKind::Entry => entries.push(id),
            _ => {}
        }
    }
    (media, entries)
}

/// Keys of reference fields (media, entry) whose stored id no longer
/// names something that exists outside the trash — review focus 3: the
/// API keeps serving the stale id and marks the key here, so an editor
/// can say "this image was deleted" and a round-trip save keeps working
/// (an unchanged reference is not re-checked on save).
///
/// Two queries at most, whatever the number of references.
pub async fn missing_references(pool: &PgPool, defs: &[FieldDef], meta: &Value) -> Vec<String> {
    let (media_ids, entry_ids) = reference_ids(defs, meta);
    if media_ids.is_empty() && entry_ids.is_empty() {
        return Vec::new();
    }
    let live_media: std::collections::HashSet<i64> = MediaRepo::new(pool.clone())
        .get_many_live(&media_ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.id)
        .collect();
    let live_entries: std::collections::HashSet<i64> = PostsRepo::new(pool.clone())
        .get_many(&entry_ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.status != PostStatus::Trash)
        .map(|p| p.id)
        .collect();
    let values = stored(meta);
    defs.iter()
        .filter(|def| {
            let Some(id) = values.get(&def.key).and_then(id_of) else {
                return false;
            };
            match def.kind {
                FieldKind::Media => !live_media.contains(&id),
                FieldKind::Entry => !live_entries.contains(&id),
                _ => false,
            }
        })
        .map(|def| def.key.clone())
        .collect()
}

/// A stored reference id (a decimal string, or a number from an older
/// writer).
fn id_of(value: &Value) -> Option<i64> {
    match value {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// Resolves the references of many entries for display at once: every
/// media item and every referenced entry of a page of entries in one query
/// each, and each referenced entry's type checked once.
#[derive(Default)]
pub struct Resolved {
    media: HashMap<i64, vyasa_db::content_models::MediaRow>,
    /// Referenced entries a visitor may see.
    entries: HashMap<i64, PostRow>,
}

impl Resolved {
    /// Loads what `items` (definitions and stored meta) reference.
    pub async fn load(
        surface: &PluginSurface,
        pool: &PgPool,
        items: &[(&[FieldDef], &Value)],
    ) -> Self {
        let mut media_ids = Vec::new();
        let mut entry_ids = Vec::new();
        for (defs, meta) in items {
            let (m, e) = reference_ids(defs, meta);
            media_ids.extend(m);
            entry_ids.extend(e);
        }
        media_ids.sort_unstable();
        media_ids.dedup();
        entry_ids.sort_unstable();
        entry_ids.dedup();
        let media = MediaRepo::new(pool.clone())
            .get_many_live(&media_ids)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.id, m))
            .collect();
        let rows = PostsRepo::new(pool.clone())
            .get_many(&entry_ids)
            .await
            .unwrap_or_default();
        let mut served: HashMap<&'static str, bool> = HashMap::new();
        let mut entries = HashMap::new();
        for row in rows {
            if row.status != PostStatus::Published || row.password_hash.is_some() {
                continue;
            }
            let key = row.post_type.as_str();
            let visible = if let Some(v) = served.get(key) {
                *v
            } else {
                let v = publicly_served(surface, pool, row.post_type).await;
                served.insert(key, v);
                v
            };
            if visible {
                entries.insert(row.id, row);
            }
        }
        Self { media, entries }
    }

    /// What a template sees for one entry's fields: every defined key,
    /// with `""` for no value; a media value as `{id, url, alt}` and an
    /// entry value as `{id, title, url}` — or `""` when the reference no
    /// longer resolves for the public (deleted, trashed, not published,
    /// protected, of a type not served publicly); a URL value only while
    /// it still passes the URL rule. Strings are autoescaped by the theme
    /// engine like every other value; nothing here is marked safe.
    #[must_use]
    pub fn render(
        &self,
        pattern: &vyasa_core::options::PermalinkPattern,
        defs: &[FieldDef],
        meta: &Value,
    ) -> Map<String, Value> {
        let values = stored(meta);
        defs.iter()
            .map(|def| {
                let shown = match values.get(&def.key) {
                    None | Some(Value::Null) => None,
                    Some(value) => self.render_one(pattern, def.kind, value),
                };
                (
                    def.key.clone(),
                    shown.unwrap_or_else(|| Value::String(String::new())),
                )
            })
            .collect()
    }

    fn render_one(
        &self,
        pattern: &vyasa_core::options::PermalinkPattern,
        kind: FieldKind,
        value: &Value,
    ) -> Option<Value> {
        match kind {
            FieldKind::Url => value
                .as_str()
                .filter(|s| is_allowed_url(s))
                .map(|s| Value::String(s.to_owned())),
            FieldKind::Media => {
                let id = id_of(value)?;
                let row = self.media.get(&id)?;
                Some(serde_json::json!({
                    "id": id.to_string(),
                    "url": format!("/api/v1/media/{id}/raw"),
                    "alt": row.alt.clone().unwrap_or_default(),
                }))
            }
            FieldKind::Entry => {
                let id = id_of(value)?;
                let row = self.entries.get(&id)?;
                Some(serde_json::json!({
                    "id": id.to_string(),
                    "title": row.title,
                    "url": pattern.path_for(row),
                }))
            }
            _ => Some(value.clone()),
        }
    }
}

/// [`Resolved::render`] for a single entry.
pub async fn render_values(
    surface: &PluginSurface,
    pool: &PgPool,
    pattern: &vyasa_core::options::PermalinkPattern,
    defs: &[FieldDef],
    meta: &Value,
) -> Map<String, Value> {
    Resolved::load(surface, pool, &[(defs, meta)])
        .await
        .render(pattern, defs, meta)
}

/// The non-empty text values (`text` and `textarea` fields) of an entry,
/// as `(label, value)` in definition order: what search indexes and the
/// Markdown mirror and `llms.txt` list.
#[must_use]
pub fn text_values(defs: &[FieldDef], meta: &Value) -> Vec<(String, String)> {
    let values = stored(meta);
    defs.iter()
        .filter(|d| matches!(d.kind, FieldKind::Text | FieldKind::Textarea))
        .filter_map(|d| {
            let text = values.get(&d.key)?.as_str()?.trim();
            (!text.is_empty()).then(|| (d.label.clone(), text.to_owned()))
        })
        .collect()
}

/// The custom types served publicly right now, for listings that filter
/// many rows at once (sitemap, `llms.txt`, search).
pub async fn public_custom_slugs(
    surface: &PluginSurface,
    pool: &PgPool,
) -> std::collections::HashSet<String> {
    served_types(surface, pool)
        .await
        .into_iter()
        .filter(|d| d.public)
        .map(|d| d.slug)
        .collect()
}

/// Whether entries of `post_type` are listed publicly, given
/// [`public_custom_slugs`].
#[must_use]
pub fn listed_publicly(
    post_type: PostType,
    public_custom: &std::collections::HashSet<String>,
) -> bool {
    match post_type {
        PostType::Post | PostType::Page => true,
        PostType::Block => false,
        PostType::Custom(slug) => public_custom.contains(slug),
    }
}
