//! Block kind registry.
//!
//! v1 is a static registry describing the core kinds. Phase 33 extends it
//! with a [`BlockType`] trait so plugins can register custom kinds with
//! their own validators and renderers.

use super::types::BlockKind;

/// Static metadata about a block kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockTypeV1 {
    /// The kind this metadata describes.
    pub kind: BlockKind,
    /// Whether instances may contain children.
    pub container: bool,
    /// Whether the kind carries rich inline text (subject to the inline
    /// HTML allowlist).
    pub inline_text: bool,
    /// Human-readable label for admin UIs.
    pub label: &'static str,
}

/// The static v1 registry: every core kind with its metadata.
#[must_use]
#[allow(clippy::too_many_lines, clippy::enum_glob_use)]
pub fn registry_v1() -> &'static [BlockTypeV1] {
    use BlockKind::*;
    &[
        BlockTypeV1 {
            kind: Paragraph,
            container: false,
            inline_text: true,
            label: "Paragraph",
        },
        BlockTypeV1 {
            kind: Heading,
            container: false,
            inline_text: true,
            label: "Heading",
        },
        BlockTypeV1 {
            kind: List,
            container: true,
            inline_text: false,
            label: "List",
        },
        BlockTypeV1 {
            kind: Quote,
            container: true,
            inline_text: true,
            label: "Quote",
        },
        BlockTypeV1 {
            kind: Code,
            container: false,
            inline_text: false,
            label: "Code",
        },
        BlockTypeV1 {
            kind: Table,
            container: true,
            inline_text: false,
            label: "Table",
        },
        BlockTypeV1 {
            kind: Image,
            container: false,
            inline_text: false,
            label: "Image",
        },
        BlockTypeV1 {
            kind: Gallery,
            container: true,
            inline_text: false,
            label: "Gallery",
        },
        BlockTypeV1 {
            kind: Video,
            container: false,
            inline_text: false,
            label: "Video",
        },
        BlockTypeV1 {
            kind: Audio,
            container: false,
            inline_text: false,
            label: "Audio",
        },
        BlockTypeV1 {
            kind: File,
            container: false,
            inline_text: false,
            label: "File",
        },
        BlockTypeV1 {
            kind: Cover,
            container: true,
            inline_text: true,
            label: "Cover",
        },
        BlockTypeV1 {
            kind: MediaText,
            container: true,
            inline_text: false,
            label: "Media & Text",
        },
        BlockTypeV1 {
            kind: Group,
            container: true,
            inline_text: false,
            label: "Group",
        },
        BlockTypeV1 {
            kind: Row,
            container: true,
            inline_text: false,
            label: "Row",
        },
        BlockTypeV1 {
            kind: Columns,
            container: true,
            inline_text: false,
            label: "Columns",
        },
        BlockTypeV1 {
            kind: Grid,
            container: true,
            inline_text: false,
            label: "Grid",
        },
        BlockTypeV1 {
            kind: Buttons,
            container: true,
            inline_text: false,
            label: "Buttons",
        },
        BlockTypeV1 {
            kind: Button,
            container: false,
            inline_text: false,
            label: "Button",
        },
        BlockTypeV1 {
            kind: Separator,
            container: false,
            inline_text: false,
            label: "Separator",
        },
        BlockTypeV1 {
            kind: PageBreak,
            container: false,
            inline_text: false,
            label: "Page Break",
        },
        BlockTypeV1 {
            kind: Details,
            container: true,
            inline_text: false,
            label: "Details",
        },
        BlockTypeV1 {
            kind: Footnotes,
            container: false,
            inline_text: false,
            label: "Footnotes",
        },
        BlockTypeV1 {
            kind: Embed,
            container: false,
            inline_text: false,
            label: "Embed",
        },
        BlockTypeV1 {
            kind: Html,
            container: false,
            inline_text: false,
            label: "HTML",
        },
        BlockTypeV1 {
            kind: Toc,
            container: false,
            inline_text: false,
            label: "Table of contents",
        },
        BlockTypeV1 {
            kind: Callout,
            container: true,
            inline_text: true,
            label: "Callout",
        },
        BlockTypeV1 {
            kind: Timed,
            container: true,
            inline_text: false,
            label: "Timed",
        },
        BlockTypeV1 {
            kind: Pattern,
            container: false,
            inline_text: false,
            label: "Synced pattern",
        },
        BlockTypeV1 {
            kind: Form,
            container: false,
            inline_text: false,
            label: "Form",
        },
    ]
}

/// Looks up registry metadata for a kind.
#[must_use]
pub fn lookup(kind: BlockKind) -> Option<&'static BlockTypeV1> {
    registry_v1().iter().find(|entry| entry.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::{lookup, registry_v1};
    use crate::block::types::BlockKind;

    #[test]
    fn registry_covers_every_kind_exactly_once() {
        assert_eq!(registry_v1().len(), BlockKind::ALL.len());
        for kind in BlockKind::ALL {
            let matches = registry_v1().iter().filter(|e| e.kind == kind).count();
            assert_eq!(matches, 1, "kind {kind:?} appears {matches} times");
            assert!(lookup(kind).is_some());
        }
    }

    #[test]
    fn registry_container_flags_match_kind_semantics() {
        for entry in registry_v1() {
            assert_eq!(
                entry.container,
                entry.kind.is_container(),
                "container flag mismatch for {:?}",
                entry.kind
            );
        }
    }
}
