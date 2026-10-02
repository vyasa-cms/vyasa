//! Block layout schema: which blocks compose each template type.
//!
//! A layout is a map from the six template types to an ordered list of
//! [`Section`]s. Static chrome (header, footer, …) and dynamic content
//! blocks (`latest-posts`, `comments`, …) share one representation; dynamic
//! kinds are validated against per-kind setting schemas from
//! [`crate::dynblocks`]. Themes stay pure data — nothing here touches a
//! database or HTTP.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tokens::TokenDiagnostic;

/// The six template types a theme must (or may) provide.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateType {
    /// Blog index / home.
    Index,
    /// Single post or custom post type entry.
    Single,
    /// Term archive (category, tag).
    Archive,
    /// Static page.
    Page,
    /// Search results.
    Search,
    /// Not-found page.
    NotFound,
}

impl TemplateType {
    /// All template types, canonical order.
    pub const ALL: [Self; 6] = [
        Self::Index,
        Self::Single,
        Self::Archive,
        Self::Page,
        Self::Search,
        Self::NotFound,
    ];

    /// The kebab-case name used in `layout.json`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Single => "single",
            Self::Archive => "archive",
            Self::Page => "page",
            Self::Search => "search",
            Self::NotFound => "not-found",
        }
    }
}

/// A screen size a section may be withheld from.
///
/// Two values, not three: "hide on tablets" has murky boundaries and is
/// rarely what anyone means, while "not on phones" and "phones only" are
/// both ordinary requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Screen {
    /// Hidden at the small breakpoint and narrower.
    Mobile,
    /// Hidden anywhere wider than the small breakpoint.
    Desktop,
}

impl Screen {
    /// The CSS class the renderer marks the section with.
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Mobile => "vy-hide-mobile",
            Self::Desktop => "vy-hide-desktop",
        }
    }
}

/// How wide a section's content runs.
///
/// A designer's first question after "what goes here" is "how wide" —
/// and the only answers used to be a band's `contained` flag and luck.
/// This is the whole vocabulary: prose-narrow, the ordinary reading
/// column, a wide showcase, or the full viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SectionWidth {
    /// A prose column (~42rem) — long text reads best narrow.
    Narrow,
    /// The theme's content width. The default, spelled out.
    Content,
    /// Wider than the column (~92rem), still padded.
    Wide,
    /// Edge to edge.
    Full,
}

impl SectionWidth {
    /// The wrapper class the renderer marks the section with.
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Narrow => "vy-w-narrow",
            Self::Content => "vy-w-content",
            Self::Wide => "vy-w-wide",
            Self::Full => "vy-w-full",
        }
    }
}

/// One section in a template's composition tree.
///
/// `kind` is either a static region name or a registered dynamic kind. `id`
/// is stable across renders ("hero:home") and unique within its template; it
/// doubles as cache key, CSS scope class and admin-editor identifier.
///
/// Sections nest. A container kind (`band`, `columns`, `grid`) holds
/// `children`, which is what makes side-by-side composition expressible at
/// all — the previous flat list could only stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Section {
    /// Stable identifier, unique within its template.
    pub id: String,
    /// Static region name or dynamic kind.
    pub kind: String,
    /// Kind-specific settings; validated against the registry schema when
    /// the kind is dynamic.
    #[serde(default, skip_serializing_if = "MapSettings::is_empty")]
    pub settings: MapSettings,
    /// Screens this section is not shown at.
    ///
    /// Separate from `settings` because it applies to every kind, and from
    /// `scope` because it is not a colour. A decorative band that is noise
    /// on a phone, or a compact summary meant only for one, had no way to
    /// say so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_on: Option<Screen>,
    /// Nested sections. Only container kinds may carry them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Section>,
    /// Colour and spacing overrides for this section and everything inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<crate::scope::StyleScope>,
    /// How wide the content runs; absent means the theme's content width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<SectionWidth>,
}

impl Section {
    /// A section with no children, settings or scope.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            settings: MapSettings::default(),
            children: Vec::new(),
            scope: None,
            hide_on: None,
            width: None,
        }
    }

    /// This section and every descendant, depth-first, parents before
    /// children.
    #[must_use]
    pub fn walk(&self) -> Vec<&Self> {
        let mut out = vec![self];
        for child in &self.children {
            out.extend(child.walk());
        }
        out
    }
}

/// Free-form JSON object of block settings.
///
/// Modeled as an ordered string-keyed pair list internally; serialized as a
/// JSON object (sorted keys via `BTreeMap`) so output stays canonical.
/// Semantic validation happens in the dynblock registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
pub struct MapSettings(Vec<(String, serde_json::Value)>);

impl Serialize for MapSettings {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<String, serde_json::Value>>()
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MapSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map =
            std::collections::BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
        Ok(Self(map.into_iter().collect()))
    }
}

impl MapSettings {
    /// Whether there are no settings at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Look up a setting by name.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Iterate entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &serde_json::Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Insert or replace a setting (used by merge semantics).
    pub fn set(&mut self, key: impl Into<String>, value: serde_json::Value) {
        let key = key.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }
}

/// Per-template block compositions.
///
/// Serialized with kebab-case keys so `layout.json` reads
/// `"not-found"` like [`TemplateType::as_str`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Layout {
    /// Home / blog index.
    pub index: Vec<Section>,
    /// Single post view.
    pub single: Vec<Section>,
    /// Term archive.
    pub archive: Vec<Section>,
    /// Static page.
    pub page: Vec<Section>,
    /// Search results.
    pub search: Vec<Section>,
    /// 404 page.
    pub not_found: Vec<Section>,
}

impl Layout {
    /// The sections for a template type.
    #[must_use]
    pub fn for_template(&self, t: TemplateType) -> &[Section] {
        match t {
            TemplateType::Index => &self.index,
            TemplateType::Single => &self.single,
            TemplateType::Archive => &self.archive,
            TemplateType::Page => &self.page,
            TemplateType::Search => &self.search,
            TemplateType::NotFound => &self.not_found,
        }
    }

    /// Mutable access to the sections for a template type.
    pub fn for_template_mut(&mut self, t: TemplateType) -> &mut Vec<Section> {
        match t {
            TemplateType::Index => &mut self.index,
            TemplateType::Single => &mut self.single,
            TemplateType::Archive => &mut self.archive,
            TemplateType::Page => &mut self.page,
            TemplateType::Search => &mut self.search,
            TemplateType::NotFound => &mut self.not_found,
        }
    }
}

/// Static region names every theme may compose with.
/// Static region names every theme may compose with.
///
/// `hero` used to be here as an empty chrome slot. It is now a real section
/// kind with a headline and calls to action, and a name cannot be both — a
/// static region short-circuits resolution, so the registered kind would
/// never render. Nothing referenced the old slot.
pub const STATIC_REGIONS: [&str; 7] = [
    "header",
    "nav",
    "content",
    "post-content",
    "sidebar-left",
    "sidebar-right",
    "footer",
];

/// Validates a layout against a dynamic-block registry.
///
/// Checks performed:
/// - every template type carries at least one block,
/// - block ids are non-empty, lowercase `[a-z0-9:_-]`, unique per template,
/// - `kind` is either one of [`STATIC_REGIONS`] or a registered dynamic
///   kind, and its settings pass the kind's validator.
///
/// # Errors
/// Returns one diagnostic per violation, each pointing at
/// `<template>[<index>].<field>`.
#[must_use]
pub fn validate_layout(
    layout: &Layout,
    registry: &crate::dynblocks::DynBlockRegistry,
) -> Vec<TokenDiagnostic> {
    validate_layout_with_tokens(layout, registry, None)
}

/// Validates a layout, and its style scopes when a token set is available.
///
/// Scopes can only be checked against the palette they will render inside,
/// so callers that have the tokens should pass them — that is what catches an
/// unreadable dark band before it becomes a revision.
#[must_use]
pub fn validate_layout_with_tokens(
    layout: &Layout,
    registry: &crate::dynblocks::DynBlockRegistry,
    tokens: Option<&crate::tokens::TokenSet>,
) -> Vec<TokenDiagnostic> {
    let mut out = Vec::new();
    for t in TemplateType::ALL {
        let sections = layout.for_template(t);
        if sections.is_empty() {
            out.push(TokenDiagnostic::Error {
                path: t.as_str().to_owned(),
                message: "template has no sections".to_owned(),
            });
            continue;
        }
        out.extend(validate_sections(sections, registry, tokens, t.as_str()));
    }
    out
}

/// Validates one standalone section tree — a single page's composition
/// rather than a whole theme layout.
///
/// The rules are a template's rules minus "must not be empty": an empty tree
/// is how an entry says "render me through the theme's template", not an
/// error. `path` prefixes every diagnostic so the caller can point at the
/// field the tree arrived in.
#[must_use]
pub fn validate_sections(
    sections: &[Section],
    registry: &crate::dynblocks::DynBlockRegistry,
    tokens: Option<&crate::tokens::TokenSet>,
    path: &str,
) -> Vec<TokenDiagnostic> {
    let mut out = Vec::new();
    // Ids must be unique across the whole tree, not just among siblings:
    // they name CSS scopes and cache entries, both of which are flat.
    let mut seen: Vec<String> = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        validate_section(
            section,
            registry,
            tokens,
            &format!("{path}[{i}]"),
            &mut seen,
            &mut out,
            0,
        );
    }
    out
}

/// Reads the section tree stored on an entry.
///
/// Everything that is not a usable tree — a missing column, a JSON `null`,
/// an empty array, or a shape written by some future version — reads as
/// "not composed", and the entry falls back to its theme template. Render
/// time is the wrong place to fail: the write path already validates, and a
/// live page must not break because one row is odd.
#[must_use]
pub fn page_sections(stored: Option<&serde_json::Value>) -> Vec<Section> {
    stored
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value::<Vec<Section>>(v.clone()).ok())
        .unwrap_or_default()
}

/// How deep sections may nest before the layout is considered pathological.
const MAX_DEPTH: usize = 5;

fn validate_section(
    section: &Section,
    registry: &crate::dynblocks::DynBlockRegistry,
    tokens: Option<&crate::tokens::TokenSet>,
    path: &str,
    seen: &mut Vec<String>,
    out: &mut Vec<TokenDiagnostic>,
    depth: usize,
) {
    if section.id.is_empty()
        || !section
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '_' | '-'))
    {
        out.push(TokenDiagnostic::Error {
            path: format!("{path}.id"),
            message: format!("\"{}\" must match [a-z0-9:_-]+", section.id),
        });
    }
    if seen.contains(&section.id) {
        out.push(TokenDiagnostic::Error {
            path: format!("{path}.id"),
            message: format!("duplicate section id \"{}\"", section.id),
        });
    }
    seen.push(section.id.clone());

    if depth > MAX_DEPTH {
        out.push(TokenDiagnostic::Error {
            path: path.to_owned(),
            message: format!("sections nested deeper than {MAX_DEPTH} levels"),
        });
        return;
    }

    if let (Some(scope), Some(tokens)) = (section.scope.as_ref(), tokens) {
        out.extend(crate::scope::validate_scope(scope, tokens, path));
    }

    let container = if STATIC_REGIONS.contains(&section.kind.as_str()) {
        false
    } else if let Some(spec) = registry.get(&section.kind) {
        for d in spec.validate_settings(&section.settings) {
            out.push(TokenDiagnostic::Error {
                path: format!("{path}.settings"),
                message: d.to_string(),
            });
        }
        spec.container()
    } else if section.kind.contains('/') {
        // A plugin's section whose plugin is not serving right now. The
        // stored layout survives its plugin the way stored entries do; it
        // renders as a comment until the plugin returns — so the studio
        // warns, it does not lock the draft.
        out.push(TokenDiagnostic::Warning {
            path: format!("{path}.kind"),
            message: format!(
                "\"{}\" is a plugin section that is not currently served — its \
                 plugin may be disabled; it renders as nothing until it returns",
                section.kind
            ),
        });
        false
    } else {
        out.push(TokenDiagnostic::Error {
            path: format!("{path}.kind"),
            message: format!(
                "unknown kind \"{}\" (expected a static region or one of: {})",
                section.kind,
                registry.kinds().join(", ")
            ),
        });
        false
    };

    if !section.children.is_empty() && !container {
        out.push(TokenDiagnostic::Error {
            path: format!("{path}.children"),
            message: format!(
                "\"{}\" holds no children — use a container kind (band, columns, grid, group)",
                section.kind
            ),
        });
        return;
    }

    for (i, child) in section.children.iter().enumerate() {
        validate_section(
            child,
            registry,
            tokens,
            &format!("{path}.children[{i}]"),
            seen,
            out,
            depth + 1,
        );
    }
}

/// Child-theme merge: per-template override, extend by id.
///
/// For each template type:
/// - an **empty** child list inherits the parent list unchanged,
/// - a non-empty child list starts from the parent's blocks and applies the
///   child's entries in order — a matching id extends the existing block
///   (child settings win per key), a new id is appended after it.
#[must_use]
pub fn merge_layouts(parent: &Layout, child: &Layout) -> Layout {
    let mut out = parent.clone();
    for t in TemplateType::ALL {
        let child_blocks = child.for_template(t);
        if child_blocks.is_empty() {
            continue;
        }
        let target = out.for_template_mut(t);
        for cb in child_blocks {
            match find_by_id(target, &cb.id) {
                Some(pb) => {
                    for (k, v) in cb.settings.iter() {
                        pb.settings.set(k.to_owned(), v.clone());
                    }
                    if cb.scope.is_some() {
                        pb.scope.clone_from(&cb.scope);
                    }
                    // A child stating children replaces the subtree outright;
                    // merging trees positionally would be guesswork.
                    if !cb.children.is_empty() {
                        pb.children.clone_from(&cb.children);
                    }
                }
                None => target.push(cb.clone()),
            }
        }
    }
    out
}

/// Finds a section by id anywhere in a tree.
fn find_by_id<'a>(sections: &'a mut [Section], id: &str) -> Option<&'a mut Section> {
    for section in sections.iter_mut() {
        if section.id == id {
            return Some(section);
        }
        // `if let` on a recursive borrow keeps the borrow checker happy where
        // a chained `?` would not.
        let found = find_by_id(&mut section.children, id);
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Canonical form used for snapshots and cache keys: settings keys sorted,
/// empty setting maps dropped, JSON rendered deterministically.
#[must_use]
pub fn canonical_layout_json(layout: &Layout) -> String {
    let mut copy = layout.clone();
    for t in TemplateType::ALL {
        for section in copy.for_template_mut(t) {
            canonicalize(section);
        }
    }
    serde_json::to_string_pretty(&copy).unwrap_or_default()
}

fn canonicalize(section: &mut Section) {
    section.settings.0.sort_by(|a, b| a.0.cmp(&b.0));
    section.settings.0.dedup_by(|a, b| a.0 == b.0);
    for child in &mut section.children {
        canonicalize(child);
    }
}
