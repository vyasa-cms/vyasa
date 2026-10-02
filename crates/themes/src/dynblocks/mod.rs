//! Registry of dynamic block kinds.
//!
//! Dynamic blocks are named slots whose *content* is resolved at render
//! time from site data (latest posts, category lists, …). This module keeps
//! the themes crate DB-free: it defines the [`DynamicBlock`] trait (with the
//! `resolve` signature real implementations will fill in during phases
//! 25/26), per-kind settings validation, and the built-in kind registry.

mod product;
pub mod queries;
mod resolvers;
pub use resolvers::card_html_for_test;
pub mod sections;

pub use queries::{
    CommentNodeData, ContentQueries, EntrySort, MonthArchiveData, PostCardData, TermLinkData,
};

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::layout::MapSettings;
use crate::tokens::TokenDiagnostic;

/// Render context handed to dynamic blocks: the data seam plus the page
/// currently being rendered.
pub struct ResolveContext<'a> {
    /// Site content accessor (implemented by api over repos).
    pub queries: &'a dyn ContentQueries,
    /// Post/page id when rendering a single entry; `None` elsewhere.
    pub post_id: Option<i64>,
    /// Structured content of the current entry, when rendering one — used
    /// by the `toc` block.
    pub content_blocks: Option<&'a [vyasa_core::block::Block]>,
    /// Comment the visitor is replying to (`?reply_to=`), when any. The
    /// comment form nests its reply under it.
    pub reply_to: Option<i64>,
    /// Studio preview render: resolvers may emit editing markers
    /// (`data-vy-edit`). Never set on public renders — a test guards it.
    pub editor: bool,
}

impl<'a> ResolveContext<'a> {
    /// Builds a context.
    #[must_use]
    pub const fn new(queries: &'a dyn ContentQueries, post_id: Option<i64>) -> Self {
        Self {
            queries,
            post_id,
            content_blocks: None,
            reply_to: None,
            editor: false,
        }
    }

    /// Builder-style setter for the studio-preview flag.
    #[must_use]
    pub const fn with_editor(mut self, editor: bool) -> Self {
        self.editor = editor;
        self
    }

    /// Builder-style setter for the comment being replied to.
    #[must_use]
    pub const fn with_reply_to(mut self, reply_to: Option<i64>) -> Self {
        self.reply_to = reply_to;
        self
    }

    /// Builder-style setter for the current entry's blocks.
    #[must_use]
    pub const fn with_blocks(mut self, blocks: &'a [vyasa_core::block::Block]) -> Self {
        self.content_blocks = Some(blocks);
        self
    }
}

/// What a dynamic block contributes to a rendered template.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockPayload {
    /// Rendered HTML fragment for the block's slot.
    Html(String),
    /// Structured data handed to the Tera template context.
    Data(serde_json::Value),
    /// The block renders nothing in this context (e.g. empty archive).
    Empty,
}

/// A dynamic block implementation.
///
/// Settings validation is available immediately (used by layout
/// validation); resolution requires data access and is implemented against
/// real queries in phases 25/26.
pub trait DynamicBlock: Send + Sync {
    /// Registry key, e.g. `"latest-posts"`.
    fn kind(&self) -> &'static str;
    /// Human-readable description for editors.
    fn description(&self) -> &'static str;
    /// Whether instances may hold nested sections.
    ///
    /// Containers are what make side-by-side composition possible; every
    /// other kind is a leaf and the validator rejects children on it.
    fn container(&self) -> bool {
        false
    }
    /// JSON Schema describing accepted settings.
    fn settings_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": true})
    }
    /// Semantic settings validation; default accepts anything.
    fn validate_settings(&self, settings: &MapSettings) -> Vec<TokenDiagnostic> {
        let _ = settings;
        Vec::new()
    }
    /// Insert-library grouping in the studio: `structure`, `marketing`,
    /// `content` or `navigation`.
    fn category(&self) -> &'static str {
        "content"
    }
    /// Starter settings for a freshly inserted instance, so a new section
    /// shows something real instead of an empty rectangle. Also handed to
    /// the assistants, whose first attempt at a kind starts from it.
    fn sample(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    /// String-typed settings keys the canvas may edit in place.
    fn inline_editable(&self) -> &'static [&'static str] {
        &[]
    }
    /// Resolves this block against render context and settings.
    ///
    /// The default returns [`BlockPayload::Empty`]; real resolvers live in
    /// [`resolvers`] and are wired through `builtin_registry`.
    fn resolve<'a>(
        &'a self,
        ctx: &'a ResolveContext<'a>,
        _settings: &'a MapSettings,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BlockPayload, String>> + Send + 'a>,
    > {
        let _ = ctx;
        Box::pin(std::future::ready(Ok(BlockPayload::Empty)))
    }
}

/// Collection of known dynamic block kinds, keyed by [`DynamicBlock::kind`].
#[derive(Clone, Default)]
pub struct DynBlockRegistry {
    blocks: BTreeMap<String, Arc<dyn DynamicBlock>>,
}

impl std::fmt::Debug for DynBlockRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynBlockRegistry")
            .field("kinds", &self.kinds())
            .finish()
    }
}

impl DynBlockRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a block kind; later registrations replace earlier ones.
    pub fn register(&mut self, block: Arc<dyn DynamicBlock>) {
        self.blocks.insert(block.kind().to_owned(), block);
    }

    /// Look up a kind.
    #[must_use]
    pub fn get(&self, kind: &str) -> Option<Arc<dyn DynamicBlock>> {
        self.blocks.get(kind).cloned()
    }

    /// All registered kinds, sorted.
    #[must_use]
    pub fn kinds(&self) -> Vec<&str> {
        self.blocks.keys().map(String::as_str).collect()
    }
}

fn check_menu(s: &MapSettings) -> Vec<String> {
    match s.get("slug") {
        None | Some(serde_json::Value::Null) => vec![], // default menu
        Some(serde_json::Value::String(text)) => {
            if text.is_empty() || text.len() > 60 {
                vec!["slug must be 1..=60 characters".to_owned()]
            } else {
                vec![]
            }
        }
        Some(_) => vec!["slug must be a string".to_owned()],
    }
}

fn uint_setting(
    settings: &MapSettings,
    key: &str,
    min: u64,
    max: u64,
    default: u64,
) -> Result<u64, String> {
    match settings.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .filter(|v| (min..=max).contains(v))
            .ok_or_else(|| format!("{key} must be an integer in [{min}, {max}]")),
        Some(_) => Err(format!("{key} must be an integer in [{min}, {max}]")),
    }
}

fn bool_setting(settings: &MapSettings, key: &str) -> Result<bool, String> {
    match settings.get(key) {
        None => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

/// A built-in dynamic block kind: declarative settings validation plus a
/// data-backed resolver.
pub(crate) struct Builtin {
    kind: &'static str,
    description: &'static str,
    schema: serde_json::Value,
    check: fn(&MapSettings) -> Vec<String>,
    resolve_fn: resolvers::ResolveFn,
    /// Holds nested sections (band, columns, grid, group).
    container: bool,
}

impl DynamicBlock for Builtin {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn container(&self) -> bool {
        self.container
    }
    fn settings_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }
    fn validate_settings(&self, settings: &MapSettings) -> Vec<TokenDiagnostic> {
        (self.check)(settings)
            .into_iter()
            .map(|message| TokenDiagnostic::Error {
                path: format!("{}.settings", self.kind),
                message,
            })
            .collect()
    }
    fn resolve<'a>(
        &'a self,
        ctx: &'a ResolveContext<'a>,
        settings: &'a MapSettings,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BlockPayload, String>> + Send + 'a>,
    > {
        (self.resolve_fn)(ctx, settings)
    }
    fn category(&self) -> &'static str {
        sections::kind_category(self.kind)
    }
    fn sample(&self) -> serde_json::Value {
        sections::kind_sample(self.kind)
    }
    fn inline_editable(&self) -> &'static [&'static str] {
        sections::kind_inline(self.kind)
    }
}

/// `collection`: the generic bound card list. Richer, domain-aware cards
/// are a plugin's business (phase 54); this is the one that always works.
fn collection_kind() -> Builtin {
    Builtin {
        kind: "collection",
        description: "Entries from any content source — posts, pages, or a \
                      registered content type — as cards, optionally sorted \
                      and filtered by the type's custom fields",
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "bind": {
                    "type": "object",
                    "properties": {
                        "source": {"type": "string", "description": "post type slug"},
                        "term": {"type": "string", "description": "term slug, or taxonomy:slug"},
                        "sort": {
                            "type": "string",
                            "description": "newest, oldest, title, updated, or field:<key> \
                                            (field:<key>:desc) to order by a custom field",
                            "pattern": "^(newest|oldest|title|updated|field:[a-z][a-z0-9_]{0,39}(:asc|:desc)?)$"
                        },
                        "where": {
                            "type": "object",
                            "description": "custom field key -> value the entry's field must equal \
                                            (or include, for multi-choice fields); at most 4",
                            "maxProperties": 4,
                            "additionalProperties": {"type": ["string", "number", "boolean"]}
                        },
                        "limit": {"type": "integer", "minimum": 1, "maximum": 24}
                    },
                    "required": ["source"],
                    "additionalProperties": false
                },
                "heading": {"type": "string", "maxLength": 120},
                "columns": {"type": "integer", "minimum": 1, "maximum": 4},
                "show_image": {"type": "boolean"},
                "show_excerpt": {"type": "boolean"}
            },
            "required": ["bind"],
            "additionalProperties": false
        }),
        check: check_collection,
        resolve_fn: resolvers::collection,
        container: false,
    }
}

fn check_collection(s: &MapSettings) -> Vec<String> {
    let mut errors = match crate::binding::Binding::from_settings(s) {
        Ok(Some(_)) => Vec::new(),
        Ok(None) => vec!["collection needs a bind object naming its source".to_owned()],
        Err(errors) => errors,
    };
    if let Err(e) = uint_setting(s, "columns", 1, 4, 3) {
        errors.push(e);
    }
    errors
}

fn check_latest_posts(s: &MapSettings) -> Vec<String> {
    uint_setting(s, "count", 1, 20, 5)
        .err()
        .into_iter()
        .collect()
}

fn check_categories_list(s: &MapSettings) -> Vec<String> {
    bool_setting(s, "show_counts").err().into_iter().collect()
}

fn check_archives(s: &MapSettings) -> Vec<String> {
    uint_setting(s, "months", 1, 36, 12)
        .err()
        .into_iter()
        .collect()
}

fn check_search_box(s: &MapSettings) -> Vec<String> {
    let bad = |msg: &str| Some(msg.to_owned());
    match s.get("placeholder") {
        None | Some(serde_json::Value::Null) => vec![],
        Some(serde_json::Value::String(text)) => if text.chars().count() > 100 {
            bad("placeholder must be at most 100 characters")
        } else {
            None
        }
        .into_iter()
        .collect(),
        Some(_) => vec!["placeholder must be a string".to_owned()],
    }
}

fn check_tag_cloud(s: &MapSettings) -> Vec<String> {
    uint_setting(s, "count", 1, 50, 20)
        .err()
        .into_iter()
        .collect()
}

fn check_comments(s: &MapSettings) -> Vec<String> {
    uint_setting(s, "per_page", 1, 100, 20)
        .err()
        .into_iter()
        .collect()
}

const NO_SETTINGS: fn(&MapSettings) -> Vec<String> = |_| Vec::new();

/// Placeholder for kinds whose rendering is engine-driven (post-content,
/// pagination): they render nothing until their data lands.
fn resolve_empty<'a>(
    _ctx: &'a ResolveContext<'a>,
    _settings: &'a MapSettings,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<BlockPayload, String>> + Send + 'a>>
{
    Box::pin(std::future::ready(Ok(BlockPayload::Empty)))
}

/// The built-in dynamic kinds every theme can compose with.
///
/// Resolution is wired to real queries in phases 25/26.
#[must_use]
pub fn builtin_registry() -> DynBlockRegistry {
    let mut reg = DynBlockRegistry::new();
    for b in blocks_list() {
        reg.register(Arc::new(b));
    }
    // Layout containers and marketing sections: the vocabulary that makes a
    // page more than a stack of blog primitives.
    for b in sections::section_kinds() {
        reg.register(Arc::new(b));
    }
    reg
}

fn blocks_first_half() -> [Builtin; 6] {
    [
        Builtin {
            kind: "latest-posts",
            description: "Most recent published posts",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"count": {"type": "integer", "minimum": 1, "maximum": 20}},
                "additionalProperties": false
            }),
            check: check_latest_posts,
            resolve_fn: resolvers::latest_posts,
            container: false,
        },
        Builtin {
            kind: "categories-list",
            description: "Category navigation list",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"show_counts": {"type": "boolean"}},
                "additionalProperties": false
            }),
            check: check_categories_list,
            resolve_fn: resolvers::categories_list,
            container: false,
        },
        Builtin {
            kind: "archives",
            description: "Monthly archive links",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"months": {"type": "integer", "minimum": 1, "maximum": 36}},
                "additionalProperties": false
            }),
            check: check_archives,
            resolve_fn: resolvers::archives,
            container: false,
        },
        Builtin {
            kind: "search-box",
            description: "Site search form",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"placeholder": {"type": "string", "maxLength": 100}},
                "additionalProperties": false
            }),
            check: check_search_box,
            resolve_fn: resolvers::search_box,
            container: false,
        },
        Builtin {
            kind: "tag-cloud",
            description: "Tag cloud weighted by usage",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"count": {"type": "integer", "minimum": 1, "maximum": 50}},
                "additionalProperties": false
            }),
            check: check_tag_cloud,
            resolve_fn: resolvers::tag_cloud,
            container: false,
        },
        Builtin {
            kind: "post-content",
            description: "Sanitized body of the current post/page",
            schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            check: NO_SETTINGS,
            resolve_fn: resolve_empty,
            container: false,
        },
    ]
}

fn blocks_second_half() -> [Builtin; 6] {
    [
        Builtin {
            kind: "breadcrumbs",
            description: "Ancestor trail for the current view",
            schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            check: NO_SETTINGS,
            resolve_fn: resolvers::breadcrumbs,
            container: false,
        },
        Builtin {
            kind: "menu",
            description: "Named navigation menu",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"slug": {"type": "string", "minLength": 1, "maxLength": 60}},
                "additionalProperties": false
            }),
            check: check_menu,
            resolve_fn: resolvers::navigation_menu,
            container: false,
        },
        Builtin {
            kind: "docs-nav",
            description: "Page-tree navigation (documentation themes)",
            schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            check: NO_SETTINGS,
            resolve_fn: resolvers::docs_nav,
            container: false,
        },
        Builtin {
            kind: "toc",
            description: "Table of contents from current page headings",
            schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            check: NO_SETTINGS,
            resolve_fn: resolvers::toc,
            container: false,
        },
        Builtin {
            kind: "pagination",
            description: "Prev/next or numbered links for the current listing",
            schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            check: NO_SETTINGS,
            resolve_fn: resolve_empty,
            container: false,
        },
        Builtin {
            kind: "comments",
            description: "Comment thread and form for the current post",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"per_page": {"type": "integer", "minimum": 1, "maximum": 100}},
                "additionalProperties": false
            }),
            check: check_comments,
            resolve_fn: resolvers::comments,
            container: false,
        },
    ]
}

/// Blocks backed by the AI integrations. They render nothing until the
/// site has the matching model and switch, so a theme can include them
/// unconditionally.
fn blocks_ai() -> [Builtin; 3] {
    [
        Builtin {
            kind: "viewer-greeting",
            description: "Greets the signed-in reader by name (filled in by the browser)",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "greeting": {"type": "string", "maxLength": 60},
                    "anonymous": {"type": "string", "maxLength": 120}
                },
                "additionalProperties": false
            }),
            check: check_viewer_greeting,
            resolve_fn: resolvers::viewer_greeting,
            container: false,
        },
        Builtin {
            kind: "related-posts",
            description:
                "Posts nearest in meaning to the one being read (needs an embedding model)",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer", "minimum": 1, "maximum": 10},
                    "heading": {"type": "string", "maxLength": 100}
                },
                "additionalProperties": false
            }),
            check: check_related_posts,
            resolve_fn: resolvers::related_posts,
            container: false,
        },
        Builtin {
            kind: "read-aloud",
            description: "Audio player for the post's generated recording (needs a speech model)",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"label": {"type": "string", "maxLength": 100}},
                "additionalProperties": false
            }),
            check: check_read_aloud,
            resolve_fn: resolvers::read_aloud,
            container: false,
        },
    ]
}

fn check_related_posts(s: &MapSettings) -> Vec<String> {
    uint_setting(s, "count", 1, 10, 5)
        .err()
        .into_iter()
        .collect()
}

fn check_read_aloud(_s: &MapSettings) -> Vec<String> {
    Vec::new()
}

fn check_viewer_greeting(_s: &MapSettings) -> Vec<String> {
    Vec::new()
}

fn blocks_list() -> [Builtin; 16] {
    let mut all = Vec::with_capacity(16);
    all.extend(blocks_first_half());
    all.extend(blocks_second_half());
    all.extend(blocks_ai());
    all.push(collection_kind());
    match <[Builtin; 16]>::try_from(all) {
        Ok(arr) => arr,
        Err(_) => unreachable!("exactly 16 builtins"),
    }
}
