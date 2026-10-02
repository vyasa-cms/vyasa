//! Theme engine for Vyasa: design-token schema and validation,
//! block layout composition, sandboxed Tera rendering, and .vytheme packages.
//! Themes are data (tokens + layout + optional templates), never executable code.

pub mod base_css;
pub mod binding;
pub mod cache;
pub mod css;
pub mod custom_css;
pub mod dynblocks;
pub mod engine;
pub mod layout;
pub mod package;
pub use package::package_parse_tokens_layout;
pub mod render;
pub mod renderer;
pub mod scope;
pub mod sections;
pub mod studio;
pub mod tokens;

pub use crate::cache::keys;
pub use base_css::base_css;
pub use binding::{
    Binding, FieldOrder, DEFAULT_BOUND_ENTRIES, MAX_BOUND_ENTRIES, MAX_BOUND_FILTERS,
};
pub use cache::{etag_for, CacheMetrics, CacheService, CachedPage, ConditionalGet};
pub use css::{font_stack, scopes_css, scopes_to_css, tokens_to_css};
pub use custom_css::namespace_custom_css;
pub use dynblocks::card_html_for_test;
pub use dynblocks::queries::{PageNodeData, TocEntryData};
pub use dynblocks::{
    builtin_registry, BlockPayload, CommentNodeData, ContentQueries, DynBlockRegistry,
    DynamicBlock, EntrySort, MonthArchiveData, PostCardData, ResolveContext, TermLinkData,
};
pub use engine::{Engine, EngineError};
pub use layout::{
    canonical_layout_json, merge_layouts, page_sections, validate_layout, validate_sections,
    Layout, MapSettings, Screen, Section, TemplateType, STATIC_REGIONS,
};
pub use render::{render_page, PageContext, PostCard, RenderRequest, SiteMeta, TermLink};
pub use renderer::blocks::RESOLVED_ATTR as PLUGIN_BLOCK_RESOLVED_ATTR;
pub use renderer::embed::{resolve_embed as resolve_embed_url, EmbedOutcome};
pub use renderer::markdown::blocks_to_markdown;
pub use renderer::{blocks_to_text, first_image, sanitize_comment_html, sanitize_plugin_html};
pub use renderer::{
    render_block, render_blocks, timed_boundary_within, toc_from_blocks, RenderError,
};
pub use scope::{validate_scope, ColorRef, StyleScope};
pub use sections::{composes, render_tree, sections_css, ResolvedSections};
pub use studio::{
    apply as apply_studio_ops, apply_with as apply_studio_ops_with, registry_schema,
    registry_schema_for, Applied, DraftState, StudioOp, TEMPLATE_NAMES,
};
pub use tokens::{
    compile_tokens, contrast_ratio, json_schema, parse_token_set, validate_token_set, ColorPalette,
    ColorSlot, Density, Direction, FontChoice, FontFace, HexColor, LayoutMetrics, ScaleRatio,
    ShadowLevel, Spacing, TokenDiagnostic, TokenSet, Typography, CONTRAST_THRESHOLD,
    TOKEN_SCHEMA_VERSION,
};

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    #[test]
    fn defaults_validate_clean_and_compile() {
        let tokens = TokenSet::default();
        let diags = validate_token_set(&tokens);
        assert_eq!(diags, Vec::new(), "default palette must be warning-free");
        let css = tokens_to_css(&tokens);
        assert!(css.contains(":root {"));
        assert!(css.contains("--vy-color-primary: #2563eb;"));
        assert!(css.contains("[data-theme=\"dark\"]"));
    }

    #[test]
    fn hex_color_parsing() {
        assert!(HexColor::parse("#FFF").is_ok());
        let six = HexColor::parse("#A1B2c3").expect("valid");
        assert_eq!(six.normalized(), "#a1b2c3");
        assert_eq!(six.channels()[0], 0xa1);
        for bad in ["fff", "#12345", "#1234567", "blue"] {
            assert!(HexColor::parse(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn contrast_math_matches_reference_values() {
        let white = HexColor(String::from("#ffffff"));
        let black = HexColor(String::from("#000000"));
        assert!((contrast_ratio(&white, &black) - 21.0).abs() < 0.01);
        assert!(contrast_ratio(&white, &white) >= 1.0);
    }

    #[test]
    fn low_contrast_is_warning_not_error() {
        let mut tokens = TokenSet::default();
        tokens.colors.text.light = HexColor(String::from("#cccccc"));
        let diags = validate_token_set(&tokens);
        assert!(diags.iter().all(|d| !d.is_error()));
        assert!(
            diags
                .iter()
                .any(|d| d.path() == "colors.text" && d.to_string().contains("light contrast")),
            "expected a light-mode contrast warning, got: {diags:?}"
        );
    }
}
