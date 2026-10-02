//! Administrator-created content types.
//!
//! A type is a row of `content_types` and a name in the live post type
//! registry ([`PostType::register_admin`]), kept in step: creating a type
//! makes it live at once and deleting it takes it out at once, so neither
//! needs a restart. Creates and deletes in this process take turns (one
//! lock), so the registry and the table never disagree about a slug two
//! administrators are racing for.

use sqlx::PgPool;
use vyasa_common::AppError;
use vyasa_db::content_models::{is_valid_custom_type, PostType, Taxonomy};
use vyasa_db::repo::{
    ContentFieldsRepo, ContentTypeRow, ContentTypeUpdate, ContentTypesRepo, NewContentType,
    TypeDeletion,
};

use super::CONTENT_LOCK;

/// Slugs no content type may take.
///
/// Built-in kinds and taxonomies; the words the phase 99 spec reserves;
/// and every first path segment the server routes itself — `app_router`
/// in `crates/api/src/main.rs` and `public::router` — with a file name
/// (`robots.txt`, `feed.xml`) listed by its stem. A type's slug is its URL
/// segment (`/{slug}/…`), so one of these would shadow the site's own
/// pages or be shadowed by them. The api crate keeps a test that every
/// literal first segment of its routers is in this list.
pub const RESERVED_TYPE_SLUGS: &[&str] = &[
    // Built-in post types and taxonomies.
    "post",
    "page",
    "block",
    "category",
    "tag",
    // Reserved by the spec.
    "admin",
    "api",
    "feed",
    "author",
    "search",
    "sitemap",
    "media",
    "assets",
    "plugin",
    // The routers' other first segments.
    "login",
    "metrics",
    "unlock",
    "archive",
    "comment",
    "form",
    "newsletter",
    "preview",
    "plugin-assets",
    "theme-assets",
    "logo",
    "brand",
    "registry",
    // File names, by stem: favicon.ico, custom.css, atom.xml, robots.txt,
    // llms.txt, indexnow.txt (feed.xml and sitemap.xml are above).
    "favicon",
    "custom",
    "atom",
    "robots",
    "llms",
    "indexnow",
    // Reserved for plugins' types since phase 33; kept in step.
    "date",
];

/// Longest singular or plural label.
const MAX_LABEL: usize = 80;
/// Longest description.
const MAX_DESCRIPTION: usize = 500;

/// Runs a registry-and-table change on a task of its own, so it finishes
/// even if the request that asked for it goes away half-way (a client
/// disconnecting must not leave a slug live without its row, or a row
/// without its slug).
async fn to_completion<T: Send + 'static>(
    work: impl std::future::Future<Output = Result<T, AppError>> + Send + 'static,
) -> Result<T, AppError> {
    tokio::spawn(work)
        .await
        .map_err(|e| AppError::internal_msg(format!("content type change failed: {e}")))?
}

/// A content type to create.
#[derive(Clone, Debug)]
pub struct NewType {
    /// The `posts.type` value and URL segment; cannot change later.
    pub slug: String,
    /// Singular label ("Product").
    pub singular: String,
    /// Plural label ("Products").
    pub plural: String,
    /// What the type is for (may be empty).
    pub description: String,
    /// Whether entries get public URLs.
    pub public: bool,
    /// Whether `/{slug}/` lists the entries.
    pub has_archive: bool,
}

/// Changes to a content type; `None` keeps a value. There is no slug: it
/// is in URLs and templates, so it never changes.
#[derive(Clone, Debug, Default)]
pub struct TypeChanges {
    /// New singular label.
    pub singular: Option<String>,
    /// New plural label.
    pub plural: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New public flag.
    pub public: Option<bool>,
    /// New archive flag.
    pub has_archive: Option<bool>,
}

/// Checks a slug for a new content type: the grammar, then built-in and
/// reserved words.
///
/// The grammar is the spec's `^[a-z][a-z0-9-]{1,39}$` narrowed to what
/// `posts.type` can store (migration 0026): at most 32 characters and no
/// trailing hyphen.
///
/// # Errors
///
/// Returns [`AppError::Validation`] naming the rule broken.
pub fn check_type_slug(slug: &str) -> Result<(), AppError> {
    if !is_valid_custom_type(slug) {
        return Err(AppError::validation(format!(
            "a content type's slug is 2 to 32 characters: lowercase letters, digits and \
             hyphens, starting with a letter and not ending with a hyphen (got {slug:?})"
        )));
    }
    if RESERVED_TYPE_SLUGS.contains(&slug) {
        return Err(AppError::validation(format!(
            "{slug:?} is reserved: it is a built-in type or one of the site's own addresses"
        )));
    }
    Ok(())
}

/// A trimmed label of 1 to `max` characters with no control characters.
fn label(what: &str, value: &str, max: usize) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::validation(format!("{what} must not be empty")));
    }
    text(what, value, max)
}

/// Text of at most `max` characters with no control characters.
fn text(what: &str, value: &str, max: usize) -> Result<String, AppError> {
    if value.chars().count() > max {
        return Err(AppError::validation(format!(
            "{what} must be at most {max} characters"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(AppError::validation(format!(
            "{what} must not contain control characters"
        )));
    }
    Ok(value.to_owned())
}

/// The 409 for a type that still has entries.
fn in_use(slug: &str, entries: i64) -> AppError {
    let noun = if entries == 1 { "entry" } else { "entries" };
    AppError::conflict(format!(
        "the content type {slug:?} has {entries} {noun} (counting drafts and the trash); \
         delete them first"
    ))
}

/// The 409 for a slug under which field definitions remain from an
/// earlier owner: the count, the keys (at most ten), and how to remove
/// them — the field delete works for a type that is no longer live.
fn left_over_fields(slug: &str, left: &[vyasa_db::repo::ContentFieldRow]) -> AppError {
    let n = left.len();
    let noun = if n == 1 {
        "field definition"
    } else {
        "field definitions"
    };
    let mut keys: Vec<String> = left
        .iter()
        .take(10)
        .map(|f| format!("{:?}", f.key))
        .collect();
    if n > 10 {
        keys.push(format!("and {} more", n - 10));
    }
    AppError::conflict(format!(
        "{n} {noun} for {slug:?} are left from an earlier owner of the slug (a plugin's type): \
         {}; delete them first (DELETE /api/v1/content-types/{slug}/fields/<key>) or choose \
         another slug",
        keys.join(", ")
    ))
}

/// Administrator-created content types.
#[derive(Clone, Debug)]
pub struct ContentTypesService {
    types: ContentTypesRepo,
    fields: ContentFieldsRepo,
}

impl ContentTypesService {
    /// Creates the service over `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            types: ContentTypesRepo::new(pool.clone()),
            fields: ContentFieldsRepo::new(pool),
        }
    }

    /// Every administrator-created type, by slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self) -> Result<Vec<ContentTypeRow>, AppError> {
        self.types.list().await
    }

    /// One type.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] (`content_type`) when there is no
    /// such administrator type.
    pub async fn get(&self, slug: &str) -> Result<ContentTypeRow, AppError> {
        self.types.get(slug).await
    }

    /// Creates a type and makes it live in the registry at once.
    ///
    /// `plugin_slugs` are the post types the enabled plugins declare (the
    /// api layer knows them); a slug among them is refused, as is one a
    /// plugin still holds in the live registry, a live custom taxonomy's,
    /// and one under which entries are already stored. Nothing is made
    /// live, and no name is spent, unless the row is written.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for a malformed or reserved slug, a
    /// bad label, or when the site is at its 64 custom types;
    /// [`AppError::Conflict`] when the slug is an existing type's, a
    /// plugin's or a taxonomy's, entries of it are already stored, or
    /// field definitions an earlier owner left under it remain (two
    /// administrators racing for one slug: one wins, the other gets this).
    pub async fn create(
        &self,
        input: NewType,
        plugin_slugs: &[impl AsRef<str>],
    ) -> Result<ContentTypeRow, AppError> {
        let slug = input.slug.as_str();
        check_type_slug(slug)?;
        let singular = label("the singular label", &input.singular, MAX_LABEL)?;
        let plural = label("the plural label", &input.plural, MAX_LABEL)?;
        let description = text("the description", input.description.trim(), MAX_DESCRIPTION)?;
        if plugin_slugs.iter().any(|p| p.as_ref() == slug) {
            return Err(AppError::conflict(format!(
                "a plugin declares the post type {slug:?}; choose another slug"
            )));
        }

        let types = self.types.clone();
        let fields = self.fields.clone();
        let slug = slug.to_owned();
        to_completion(async move {
            let _turn = CONTENT_LOCK.lock().await;
            match types.get(&slug).await {
                Ok(_) => {
                    return Err(AppError::conflict(format!(
                        "a content type with the slug {slug:?} already exists"
                    )))
                }
                Err(AppError::NotFound { .. }) => {}
                Err(other) => return Err(other),
            }
            // Entries already stored under the slug (a plugin removed
            // since, an import) would be adopted silently.
            let stored = types.count_entries(&slug).await?;
            if stored > 0 {
                let noun = if stored == 1 { "entry" } else { "entries" };
                return Err(AppError::conflict(format!(
                    "{stored} {noun} of the type {slug:?} are already stored (counting drafts \
                     and the trash); delete them or choose another slug"
                )));
            }
            // Field definitions an earlier owner of the slug left behind
            // (a plugin's type, the plugin removed since) would be adopted
            // just as silently, with options nobody here chose.
            let left = fields.list(&slug).await?;
            if !left.is_empty() {
                return Err(left_over_fields(&slug, &left));
            }
            // Types and taxonomies share the `/{slug}/…` namespace.
            if Taxonomy::custom_taxonomies().contains(&slug.as_str()) {
                return Err(AppError::conflict(format!(
                    "{slug:?} is a taxonomy's slug; choose another slug"
                )));
            }
            // Checked before the row is written, made live after: a create
            // that fails anywhere spends no name and leaves nothing live.
            PostType::check_admin(&slug)?;
            let row = types
                .insert(&NewContentType {
                    slug: &slug,
                    singular: &singular,
                    plural: &plural,
                    description: &description,
                    public: input.public,
                    has_archive: input.has_archive,
                })
                .await?;
            if let Err(e) = PostType::register_admin(&slug) {
                // A plugin took the name or the last place since the
                // check (plugins do not take this lock): undo the row.
                if let Err(undo) = types.delete_if_unused(&slug).await {
                    tracing::error!(slug, "content type row left behind: {undo}");
                }
                return Err(e);
            }
            Ok(row)
        })
        .await
    }

    /// Relabels a type or changes its flags. The slug never changes.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for a bad label,
    /// [`AppError::NotFound`] when there is no such type.
    pub async fn update(
        &self,
        slug: &str,
        changes: TypeChanges,
    ) -> Result<ContentTypeRow, AppError> {
        let singular = changes
            .singular
            .map(|v| label("the singular label", &v, MAX_LABEL))
            .transpose()?;
        let plural = changes
            .plural
            .map(|v| label("the plural label", &v, MAX_LABEL))
            .transpose()?;
        let description = changes
            .description
            .map(|v| text("the description", v.trim(), MAX_DESCRIPTION))
            .transpose()?;
        self.types
            .update(
                slug,
                &ContentTypeUpdate {
                    singular: singular.as_deref(),
                    plural: plural.as_deref(),
                    description: description.as_deref(),
                    public: changes.public,
                    has_archive: changes.has_archive,
                },
            )
            .await
    }

    /// Entries of the type in any status, trash included — what a refused
    /// delete reports.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn entry_count(&self, slug: &str) -> Result<i64, AppError> {
        self.types.count_entries(slug).await
    }

    /// Deletes a type that has no entries (in any status), with its field
    /// definitions, and takes it out of the registry at once.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such type, and
    /// [`AppError::Conflict`] naming the count when entries exist.
    pub async fn delete(&self, slug: &str) -> Result<(), AppError> {
        let types = self.types.clone();
        let slug = slug.to_owned();
        to_completion(async move {
            let _turn = CONTENT_LOCK.lock().await;
            types.get(&slug).await?;
            let entries = types.count_entries(&slug).await?;
            if entries > 0 {
                return Err(in_use(&slug, entries));
            }
            // Out of the registry before the row goes, so no new entry of
            // it can be started while it is being deleted; back in if the
            // delete does not happen after all.
            let was_live = PostType::unregister_admin(&slug);
            let outcome = types.delete_if_unused(&slug).await;
            let relive = || {
                if was_live {
                    if let Err(e) = PostType::register_admin(&slug) {
                        tracing::warn!(slug, "content type left the registry: {e}");
                    }
                }
            };
            match outcome {
                Ok(TypeDeletion::Deleted) => Ok(()),
                Ok(TypeDeletion::InUse(entries)) => {
                    relive();
                    Err(in_use(&slug, entries))
                }
                Ok(TypeDeletion::Missing) => Err(AppError::not_found("content_type", &slug)),
                Err(e) => {
                    relive();
                    Err(e)
                }
            }
        })
        .await
    }

    /// Makes every stored type live. Call at boot **before** the plugins
    /// register theirs: an administrator's type wins its slug, and a
    /// plugin declaring it is then refused
    /// ([`PostType::register_plugin_types`] answers Conflict). Returns the
    /// types that could not be made live, with the reason — a slug that
    /// has become reserved since it was created, or no room under the cap
    /// — and logs them; they stay stored. Idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] when the types cannot be read.
    pub async fn load_into_registry(&self) -> Result<Vec<(String, AppError)>, AppError> {
        let _turn = CONTENT_LOCK.lock().await;
        let mut skipped = Vec::new();
        for row in self.types.list().await? {
            let made =
                check_type_slug(&row.slug).and_then(|()| PostType::register_admin(&row.slug));
            if let Err(e) = made {
                tracing::error!(slug = row.slug, "content type not registered: {e}");
                skipped.push((row.slug, e));
            }
        }
        Ok(skipped)
    }
}
