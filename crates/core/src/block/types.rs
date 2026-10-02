//! Block kinds and the wire-format types.
//!
//! JSON shape (stable contract between editor and server):
//!
//! ```json
//! {"kind": "heading", "attrs": {"level": 2, "text": "Hi"}, "children": []}
//! ```
//!
//! `kind` discriminates; `attrs` carries per-kind properties; `children`
//! holds nested blocks for container kinds. Never change the shape without
//! bumping [`crate::block::doc::SCHEMA_VERSION`].

use serde::{Deserialize, Serialize};

/// Every block kind supported by the core registry v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    // Text
    /// Rich-text paragraph.
    Paragraph,
    /// Section heading.
    Heading,
    /// Ordered/unordered list.
    List,
    /// Quoted passage.
    Quote,
    /// Pre-formatted code.
    Code,
    /// Data table.
    Table,
    // Media
    /// Single image reference.
    Image,
    /// Image gallery.
    Gallery,
    /// Video embed/reference.
    Video,
    /// Audio reference.
    Audio,
    /// Downloadable file reference.
    File,
    /// Full-width image with overlay text.
    Cover,
    /// Side-by-side media and text.
    #[serde(rename = "media_text")]
    MediaText,
    // Design / layout
    /// Generic container.
    Group,
    /// Horizontal flex row.
    Row,
    /// Fixed column layout.
    Columns,
    /// Grid layout.
    Grid,
    /// Button group.
    Buttons,
    /// Single link button.
    Button,
    /// Horizontal rule.
    Separator,
    /// Pagination anchor.
    #[serde(rename = "page_break")]
    PageBreak,
    /// Collapsible disclosure.
    Details,
    // Special
    /// Footnotes section.
    Footnotes,
    /// Allowlisted iframe embed.
    Embed,
    /// Sanitized raw HTML escape hatch.
    Html,
    /// Table of contents built from the document's headings.
    Toc,
    /// A note, tip or warning box holding other blocks.
    Callout,
    /// Blocks shown only between two moments (a launch notice, a season).
    Timed,
    /// A synced pattern: `attrs.pattern_id` names a saved arrangement of
    /// blocks that is resolved into `children` before rendering.
    Pattern,
    /// A form an editor designed: `attrs.form_slug` names it; the API
    /// fills `attrs.definition` before rendering.
    Form,
    /// A block kind contributed by a plugin.
    ///
    /// The name itself lives on [`Block::plugin_kind`], not here: keeping
    /// this variant a unit keeps `BlockKind` `Copy`, which the whole
    /// codebase relies on. `Plugin` is deliberately absent from
    /// [`BlockKind::ALL`] and from [`BlockKind::parse`] — it is reachable
    /// only through [`Block`]'s deserializer, which is the one place that
    /// has the name to put somewhere.
    Plugin,
}

impl BlockKind {
    /// All v1 kinds.
    pub const ALL: [BlockKind; 30] = [
        BlockKind::Paragraph,
        BlockKind::Heading,
        BlockKind::List,
        BlockKind::Quote,
        BlockKind::Code,
        BlockKind::Table,
        BlockKind::Image,
        BlockKind::Gallery,
        BlockKind::Video,
        BlockKind::Audio,
        BlockKind::File,
        BlockKind::Cover,
        BlockKind::MediaText,
        BlockKind::Group,
        BlockKind::Row,
        BlockKind::Columns,
        BlockKind::Grid,
        BlockKind::Buttons,
        BlockKind::Button,
        BlockKind::Separator,
        BlockKind::PageBreak,
        BlockKind::Details,
        BlockKind::Footnotes,
        BlockKind::Embed,
        BlockKind::Html,
        BlockKind::Toc,
        BlockKind::Callout,
        BlockKind::Timed,
        BlockKind::Pattern,
        BlockKind::Form,
    ];

    /// Wire name of the kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Paragraph => "paragraph",
            BlockKind::Heading => "heading",
            BlockKind::List => "list",
            BlockKind::Quote => "quote",
            BlockKind::Code => "code",
            BlockKind::Table => "table",
            BlockKind::Image => "image",
            BlockKind::Gallery => "gallery",
            BlockKind::Video => "video",
            BlockKind::Audio => "audio",
            BlockKind::File => "file",
            BlockKind::Cover => "cover",
            BlockKind::MediaText => "media_text",
            BlockKind::Group => "group",
            BlockKind::Row => "row",
            BlockKind::Columns => "columns",
            BlockKind::Grid => "grid",
            BlockKind::Buttons => "buttons",
            BlockKind::Button => "button",
            BlockKind::Separator => "separator",
            BlockKind::PageBreak => "page_break",
            BlockKind::Details => "details",
            BlockKind::Footnotes => "footnotes",
            BlockKind::Embed => "embed",
            BlockKind::Html => "html",
            BlockKind::Toc => "toc",
            BlockKind::Callout => "callout",
            BlockKind::Timed => "timed",
            BlockKind::Pattern => "pattern",
            BlockKind::Form => "form",
            BlockKind::Plugin => "plugin",
        }
    }

    /// Parses a wire name.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown
    /// kinds.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        for kind in Self::ALL {
            if kind.as_str() == value {
                return Ok(kind);
            }
        }
        Err(vyasa_common::AppError::validation(format!(
            "unknown block kind: {value:?}"
        )))
    }

    /// Whether blocks of this kind may contain children.
    #[must_use]
    pub fn is_container(self) -> bool {
        matches!(
            self,
            BlockKind::Group
                | BlockKind::Row
                | BlockKind::Columns
                | BlockKind::Grid
                | BlockKind::Buttons
                | BlockKind::Details
                | BlockKind::Gallery
                | BlockKind::Quote
                | BlockKind::List
                | BlockKind::Cover
                | BlockKind::MediaText
                | BlockKind::Table
                | BlockKind::Callout
                | BlockKind::Timed
        )
    }
}

/// serde default for `Block::attrs`.
fn null_attrs() -> serde_json::Value {
    serde_json::Value::Null
}

/// A single content block.
///
/// Serialization is hand-written rather than derived because the wire
/// format has one `kind` string that means two things: a core kind, or a
/// plugin-contributed `namespace/name`. A derived impl would have to
/// choose one, and choosing "core only" is what made plugin blocks
/// impossible to store — a document containing one failed to deserialize,
/// so the block could never round-trip through the editor at all.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Which block this is.
    pub kind: BlockKind,
    /// For [`BlockKind::Plugin`], the declared `namespace/name`.
    ///
    /// Always `None` for core kinds; always `Some` for `Plugin` (the
    /// deserializer never produces one without it).
    pub plugin_kind: Option<String>,
    /// Per-kind attributes (validated against the registry).
    pub attrs: serde_json::Value,
    /// Nested blocks (container kinds only).
    pub children: Vec<Block>,
}

impl Block {
    /// A core block with no children.
    #[must_use]
    pub fn new(kind: BlockKind, attrs: serde_json::Value) -> Self {
        Self {
            kind,
            plugin_kind: None,
            attrs,
            children: Vec::new(),
        }
    }

    /// The wire name of this block: the core kind, or the plugin's
    /// declared `namespace/name`.
    #[must_use]
    pub fn kind_name(&self) -> &str {
        match (self.kind, self.plugin_kind.as_deref()) {
            (BlockKind::Plugin, Some(name)) => name,
            _ => self.kind.as_str(),
        }
    }
}

impl Serialize for Block {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let extra = usize::from(!self.attrs.is_null()) + usize::from(!self.children.is_empty());
        let mut map = serializer.serialize_map(Some(1 + extra))?;
        map.serialize_entry("kind", self.kind_name())?;
        if !self.attrs.is_null() {
            map.serialize_entry("attrs", &self.attrs)?;
        }
        if !self.children.is_empty() {
            map.serialize_entry("children", &self.children)?;
        }
        map.end()
    }
}

/// The derived shape, minus the `kind` interpretation.
#[derive(Deserialize)]
struct RawBlock {
    kind: String,
    #[serde(default = "null_attrs")]
    attrs: serde_json::Value,
    #[serde(default)]
    children: Vec<Block>,
}

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawBlock::deserialize(deserializer)?;
        // A core kind wins; anything else is a plugin kind, whose *name*
        // is checked by the validator rather than here, so that a bad name
        // produces a `blocks[$[0]]: ...` message pointing at the block
        // instead of an opaque serde error.
        let (kind, plugin_kind) = match BlockKind::parse(&raw.kind) {
            Ok(k) if k != BlockKind::Plugin => (k, None),
            _ => (BlockKind::Plugin, Some(raw.kind)),
        };
        Ok(Self {
            kind,
            plugin_kind,
            attrs: raw.attrs,
            children: raw.children,
        })
    }
}
