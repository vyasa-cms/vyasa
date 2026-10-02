//! Block validation: per-kind attribute rules, inline-HTML allowlist,
//! structural limits.
//!
//! Validation is the editor↔server contract enforcement point; rendering
//! (phase 22) applies ammonia sanitization as defense in depth on top.

use vyasa_common::AppError;

use super::types::{Block, BlockKind};

/// Maximum block nesting depth.
pub const MAX_DEPTH: usize = 32;
/// Maximum total blocks per document.
pub const MAX_BLOCKS: usize = 10_000;
/// Maximum length of any single text field (chars).
const MAX_TEXT_LEN: usize = 100_000;
/// Maximum serialized size of a plugin block's attrs.
///
/// Plugin block attrs cross the sandbox boundary as a JSON string on every
/// render, so this is a request-cost bound as much as a storage one.
pub const MAX_PLUGIN_ATTRS_BYTES: usize = 64 * 1024;

/// Inline HTML tags allowed inside rich-text fields (paragraph text,
/// heading text, quote text, captions).
///
/// This is the single source of truth. The renderer's ammonia allowlist and
/// the importer's filter both read it, and the admin editor offers exactly
/// these marks — so nothing an author can apply vanishes on publish, and
/// nothing that would vanish can be applied.
pub const INLINE_TAGS: &[&str] = &[
    "a", "b", "i", "em", "strong", "code", "br", "s", "del", "ins", "u", "mark", "sub", "sup",
];

/// Result of successful validation (currently a unit; kept as a type so
/// future versions can carry computed metadata like anchors).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Validated {
    /// Number of blocks visited.
    pub block_count: usize,
}

/// Validates a whole block tree.
///
/// # Errors
///
/// Returns [`AppError::Validation`] with a `blocks[{path}]`-style prefix
/// describing the first violation.
pub fn validate(root: &[Block]) -> Result<Validated, AppError> {
    let mut state = State {
        count: 0,
        anchors: Vec::new(),
    };
    validate_list(root, 0, "$", &mut state)?;
    if state.count > MAX_BLOCKS {
        return Err(AppError::validation(format!(
            "document exceeds the maximum of {MAX_BLOCKS} blocks ({})",
            state.count
        )));
    }
    Ok(Validated {
        block_count: state.count,
    })
}

struct State {
    count: usize,
    anchors: Vec<String>,
}

fn validate_list(
    blocks: &[Block],
    depth: usize,
    path: &str,
    state: &mut State,
) -> Result<(), AppError> {
    if depth > MAX_DEPTH {
        return Err(AppError::validation(format!(
            "blocks[{path}]: nesting exceeds the maximum depth of {MAX_DEPTH}"
        )));
    }
    for (index, block) in blocks.iter().enumerate() {
        let child_path = format!("{path}[{index}]");
        state.count += 1;
        if state.count > MAX_BLOCKS {
            return Err(AppError::validation(format!(
                "document exceeds the maximum of {MAX_BLOCKS} blocks"
            )));
        }
        validate_block(block, depth, &child_path, state)?;
    }
    Ok(())
}

fn validate_block(
    block: &Block,
    depth: usize,
    path: &str,
    state: &mut State,
) -> Result<(), AppError> {
    if !block.children.is_empty() && !block.kind.is_container() {
        return Err(AppError::validation(format!(
            "blocks[{path}]: {} blocks cannot have children",
            block.kind.as_str()
        )));
    }
    // Child kind restrictions for containers.
    if block.kind == BlockKind::Buttons {
        for (index, child) in block.children.iter().enumerate() {
            if child.kind != BlockKind::Button {
                return Err(AppError::validation(format!(
                    "blocks[{path}]: buttons may only contain button children (child {index} is {})",
                    child.kind.as_str()
                )));
            }
        }
    }
    if block.kind == BlockKind::Gallery {
        for (index, child) in block.children.iter().enumerate() {
            if child.kind != BlockKind::Image {
                return Err(AppError::validation(format!(
                    "blocks[{path}]: gallery may only contain image children (child {index} is {})",
                    child.kind.as_str()
                )));
            }
        }
    }
    if block.kind == BlockKind::Plugin {
        let name = block.plugin_kind.as_deref().unwrap_or("");
        if !is_plugin_kind_name(name) {
            return Err(AppError::validation(format!(
                "blocks[{path}]: unknown block kind {name:?} (plugin blocks are named \
                 `namespace/name`, lowercase letters, digits and hyphens)"
            )));
        }
    }
    validate_attrs(block, path, state)?;
    validate_list(
        &block.children,
        depth + 1,
        &format!("{path}.children"),
        state,
    )
}

fn str_field<'a>(attrs: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    attrs.get(name).and_then(|v| v.as_str())
}

/// Enforces text-length and shape rules per kind; unknown attr keys are
/// preserved (forward compatibility) but typed fields are checked.
#[allow(clippy::too_many_lines)]
fn validate_attrs(block: &Block, path: &str, state: &mut State) -> Result<(), AppError> {
    let attrs = &block.attrs;
    if attrs.is_null() {
        return Ok(());
    }
    if !attrs.is_object() {
        return Err(AppError::validation(format!(
            "blocks[{path}]: attrs must be an object"
        )));
    }
    let check_len = |text: &str, field: &str| -> Result<(), AppError> {
        if text.chars().count() > MAX_TEXT_LEN {
            Err(AppError::validation(format!(
                "blocks[{path}]: attrs.{field} exceeds {MAX_TEXT_LEN} characters"
            )))
        } else {
            Ok(())
        }
    };
    match block.kind {
        BlockKind::Form => {
            if str_field(attrs, "form_slug").is_none_or(str::is_empty) {
                return Err(missing(path, "form_slug"));
            }
        }
        BlockKind::Pattern => {
            // The reference; its children are filled in at render time.
            let has_id = attrs
                .get("pattern_id")
                .is_some_and(|v| v.is_string() || v.is_number());
            if !has_id {
                return Err(missing(path, "pattern_id"));
            }
        }
        BlockKind::Paragraph | BlockKind::Quote | BlockKind::Cover => {
            if let Some(text) = str_field(attrs, "text") {
                check_len(text, "text")?;
                check_inline_html(text, path)?;
            }
        }
        BlockKind::Heading => {
            let Some(text) = str_field(attrs, "text") else {
                return Err(missing(path, "text"));
            };
            check_len(text, "text")?;
            check_inline_html(text, path)?;
            let level = attrs
                .get("level")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| missing(path, "level"))?;
            if !(2..=6).contains(&level) {
                return Err(AppError::validation(format!(
                    "blocks[{path}]: heading level must be 2..=6 (got {level})"
                )));
            }
        }
        BlockKind::List => {
            let ordered = attrs.get("ordered").and_then(serde_json::Value::as_bool);
            if attrs.get("ordered").is_some() && ordered.is_none() {
                return Err(AppError::validation(format!(
                    "blocks[{path}]: attrs.ordered must be a boolean"
                )));
            }
        }
        BlockKind::Code => {
            let Some(code) = str_field(attrs, "code") else {
                return Err(missing(path, "code"));
            };
            check_len(code, "code")?;
            if let Some(lang) = str_field(attrs, "language") {
                // Language must be an identifier-ish token, not free text.
                if lang.len() > 40
                    || !lang.chars().all(|c| {
                        c.is_ascii_alphanumeric() || c == '-' || c == '+' || c == '#' || c == '_'
                    })
                {
                    return Err(AppError::validation(format!(
                        "blocks[{path}]: invalid code language {lang:?}"
                    )));
                }
            }
        }
        BlockKind::Image | BlockKind::Audio | BlockKind::Video | BlockKind::File => {
            // Media references may use either mediaId (numeric) or url.
            let has_media_id = attrs.get("mediaId").is_some_and(serde_json::Value::is_u64);
            let has_url = str_field(attrs, "url").is_some_and(|u| !u.is_empty());
            if !has_media_id && !has_url {
                return Err(AppError::validation(format!(
                    "blocks[{path}]: media blocks need attrs.mediaId or attrs.url"
                )));
            }
            if let Some(alt) = str_field(attrs, "alt") {
                check_len(alt, "alt")?;
            }
        }
        BlockKind::Button => {
            let Some(label) = str_field(attrs, "label") else {
                return Err(missing(path, "label"));
            };
            check_len(label, "label")?;
            let Some(href) = str_field(attrs, "href") else {
                return Err(missing(path, "href"));
            };
            check_url(href, path)?;
        }
        BlockKind::Embed => {
            let Some(url) = str_field(attrs, "url") else {
                return Err(missing(path, "url"));
            };
            check_url(url, path)?;
        }
        BlockKind::Html => {
            // Raw HTML is accepted here but is always ammonia-sanitized at
            // render; size-checked only.
            let Some(html) = str_field(attrs, "html") else {
                return Err(missing(path, "html"));
            };
            check_len(html, "html")?;
        }
        BlockKind::Toc => {
            if let Some(depth) = attrs.get("depth") {
                let ok = depth.as_u64().is_some_and(|d| (2..=6).contains(&d));
                if !ok {
                    return Err(AppError::validation(format!(
                        "blocks[{path}]: toc depth must be 2..=6"
                    )));
                }
            }
        }
        BlockKind::Callout => {
            if let Some(tone) = str_field(attrs, "tone") {
                if !matches!(tone, "note" | "tip" | "warning" | "danger") {
                    return Err(AppError::validation(format!(
                        "blocks[{path}]: callout tone must be note, tip, warning or danger"
                    )));
                }
            }
            if let Some(title) = str_field(attrs, "title") {
                check_len(title, "title")?;
                check_inline_html(title, path)?;
            }
        }
        BlockKind::Timed => {
            for key in ["from", "until"] {
                if let Some(raw) = str_field(attrs, key) {
                    if !raw.is_empty() && chrono::DateTime::parse_from_rfc3339(raw).is_err() {
                        return Err(AppError::validation(format!(
                            "blocks[{path}]: attrs.{key} must be an RFC 3339 date-time"
                        )));
                    }
                }
            }
        }
        BlockKind::Table => {
            if let Some(text) = str_field(attrs, "caption") {
                check_len(text, "caption")?;
                check_inline_html(text, path)?;
            }
            // Cells are inline rich text, so they get the same early check.
            let rows = attrs.get("rows").and_then(serde_json::Value::as_array);
            let header = attrs.get("header").and_then(serde_json::Value::as_array);
            let cells = rows
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_array)
                .flatten()
                .chain(header.into_iter().flatten());
            for cell in cells {
                if let Some(text) = cell.as_str() {
                    check_len(text, "rows")?;
                    check_inline_html(text, path)?;
                }
            }
        }
        BlockKind::Separator
        | BlockKind::PageBreak
        | BlockKind::Footnotes
        | BlockKind::Group
        | BlockKind::Row
        | BlockKind::Columns
        | BlockKind::Grid
        | BlockKind::Buttons
        | BlockKind::Gallery
        | BlockKind::Details
        | BlockKind::MediaText => {
            if let Some(text) = str_field(attrs, "caption") {
                check_len(text, "caption")?;
                check_inline_html(text, path)?;
            }
        }
        // A plugin owns its own attribute schema, so the host checks only
        // what it is entitled to: size. Every string in here is handed to
        // the plugin as data and its rendered output is sanitized on the
        // way back, so no inline-HTML rule applies at this end.
        BlockKind::Plugin => {
            let len = serde_json::to_string(attrs).map_or(0, |s| s.len());
            if len > MAX_PLUGIN_ATTRS_BYTES {
                return Err(AppError::validation(format!(
                    "blocks[{path}]: attrs exceed {MAX_PLUGIN_ATTRS_BYTES} bytes ({len})"
                )));
            }
            // The renderer reads the plugin's output back out of a
            // `__`-prefixed attr. Authors do not get to write one, or a
            // stored document could inject HTML that no plugin produced.
            if let Some(map) = attrs.as_object() {
                if let Some(key) = map.keys().find(|k| k.starts_with("__")) {
                    return Err(AppError::validation(format!(
                        "blocks[{path}]: attrs.{key} is reserved (keys may not start with `__`)"
                    )));
                }
            }
        }
    }
    // Collect heading anchors for duplicate detection.
    if block.kind == BlockKind::Heading {
        if let Some(text) = str_field(attrs, "text") {
            let anchor = vyasa_common::slugify(text);
            let anchor = if anchor.is_empty() {
                "section".to_string()
            } else {
                anchor
            };
            if state.anchors.contains(&anchor) {
                let mut n = 2;
                while state.anchors.contains(&format!("{anchor}-{n}")) {
                    n += 1;
                }
                state.anchors.push(format!("{anchor}-{n}"));
            } else {
                state.anchors.push(anchor);
            }
        }
    }
    Ok(())
}

/// `namespace/name`, both parts lowercase alphanumeric with internal
/// hyphens. The slash is required: it is what keeps a plugin from ever
/// shadowing a core kind, present or future.
#[must_use]
pub fn is_plugin_kind_name(name: &str) -> bool {
    let Some((namespace, local)) = name.split_once('/') else {
        return false;
    };
    let part_ok = |p: &str| {
        (1..=32).contains(&p.len())
            && p.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && p.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    part_ok(namespace) && part_ok(local)
}

fn missing(path: &str, field: &str) -> AppError {
    AppError::validation(format!("blocks[{path}]: missing required attrs.{field}"))
}

/// Rejects `<script>`-style payloads in inline text early (full
/// sanitization still happens at render).
fn check_inline_html(text: &str, path: &str) -> Result<(), AppError> {
    let lower = text.to_ascii_lowercase();
    for forbidden in ["<script", "javascript:", "<iframe", "onerror=", "<style"] {
        if lower.contains(forbidden) {
            return Err(AppError::validation(format!(
                "blocks[{path}]: inline text contains forbidden markup {forbidden:?}"
            )));
        }
    }
    Ok(())
}

/// URL scheme check for href/embed targets.
fn check_url(url: &str, path: &str) -> Result<(), AppError> {
    let looks_safe = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with('/')
        || url.starts_with('#')
        || url.starts_with("mailto:");
    if looks_safe {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "blocks[{path}]: url must be http(s), relative, anchor, or mailto (got {url:?})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::block::types::{Block, BlockKind};
    use serde_json::json;

    fn block(kind: BlockKind, attrs: serde_json::Value) -> Block {
        Block::new(kind, attrs)
    }

    #[test]
    fn toc_callout_and_timed_have_their_rules() {
        assert!(validate(&[block(BlockKind::Toc, json!({}))]).is_ok());
        assert!(validate(&[block(BlockKind::Toc, json!({"depth": 3}))]).is_ok());
        assert!(validate(&[block(BlockKind::Toc, json!({"depth": 1}))]).is_err());
        let callout = |tone: &str| Block {
            kind: BlockKind::Callout,
            plugin_kind: None,
            attrs: json!({"tone": tone, "title": "Heads up"}),
            children: vec![block(BlockKind::Paragraph, json!({"text": "x"}))],
        };
        assert!(validate(&[callout("tip")]).is_ok());
        assert!(validate(&[callout("purple")]).is_err());
        let timed = |from: &str| Block {
            kind: BlockKind::Timed,
            plugin_kind: None,
            attrs: json!({"from": from}),
            children: vec![block(BlockKind::Paragraph, json!({"text": "x"}))],
        };
        assert!(validate(&[timed("2026-09-10T09:00:00Z")]).is_ok());
        assert!(validate(&[timed("next tuesday")]).is_err());
    }

    #[test]
    fn table_cells_are_checked_like_any_other_text() {
        let doc = vec![block(
            BlockKind::Table,
            json!({"header": ["a"], "rows": [["<script>alert(1)</script>"]]}),
        )];
        let err = validate(&doc).unwrap_err().to_string();
        assert!(err.contains("forbidden markup"), "{err}");

        let ok = vec![block(
            BlockKind::Table,
            json!({"header": ["<strong>a</strong>"], "rows": [["b", "c"]]}),
        )];
        assert!(validate(&ok).is_ok());
    }

    #[test]
    fn valid_minimal_document() {
        let doc = vec![block(
            BlockKind::Heading,
            json!({"level": 2, "text": "Title"}),
        )];
        let validated = validate(&doc).expect("valid");
        assert_eq!(validated.block_count, 1);
    }

    #[test]
    fn heading_requires_text_and_level() {
        assert!(validate(&[block(BlockKind::Heading, json!({"level": 2}))]).is_err());
        assert!(validate(&[block(BlockKind::Heading, json!({"text": "x"}))]).is_err());
        assert!(validate(&[block(BlockKind::Heading, json!({"level": 7, "text": "x"}))]).is_err());
        assert!(validate(&[block(BlockKind::Heading, json!({"level": 1, "text": "x"}))]).is_err());
    }

    #[test]
    fn non_containers_reject_children() {
        let doc = vec![Block {
            kind: BlockKind::Paragraph,
            plugin_kind: None,
            attrs: json!({"text": "hi"}),
            children: vec![block(BlockKind::Paragraph, json!({"text": "nested"}))],
        }];
        let err = validate(&doc).unwrap_err().to_string();
        assert!(err.contains("cannot have children"), "err: {err}");
    }

    #[test]
    fn buttons_only_contain_buttons() {
        let ok = vec![Block {
            kind: BlockKind::Buttons,
            plugin_kind: None,
            attrs: json!({}),
            children: vec![block(
                BlockKind::Button,
                json!({"label": "Go", "href": "https://example.com"}),
            )],
        }];
        assert!(validate(&ok).is_ok());

        let bad = vec![Block {
            kind: BlockKind::Buttons,
            plugin_kind: None,
            attrs: json!({}),
            children: vec![block(BlockKind::Paragraph, json!({"text": "no"}))],
        }];
        assert!(validate(&bad).is_err());
    }

    #[test]
    fn gallery_only_contains_images() {
        let bad = vec![Block {
            kind: BlockKind::Gallery,
            plugin_kind: None,
            attrs: json!({}),
            children: vec![block(
                BlockKind::Video,
                json!({"url": "https://example.com/v.mp4"}),
            )],
        }];
        assert!(validate(&bad).is_err());
    }

    #[test]
    fn media_blocks_need_reference() {
        assert!(validate(&[block(BlockKind::Image, json!({"alt": "no ref"}))]).is_err());
        assert!(validate(&[block(BlockKind::Image, json!({"mediaId": 5}))]).is_ok());
        assert!(validate(&[block(
            BlockKind::Image,
            json!({"url": "https://example.com/x.png"})
        )])
        .is_ok());
    }

    #[test]
    fn inline_html_allowlist_rejects_script_payloads() {
        for payload in [
            "<script>alert(1)</script>",
            "click javascript:void(0)",
            "<iframe src=x>",
            "<img onerror=alert(1)>",
            "<style>body{}</style>",
        ] {
            let doc = vec![block(BlockKind::Paragraph, json!({"text": payload}))];
            assert!(validate(&doc).is_err(), "payload accepted: {payload}");
        }
        // Allowed inline markup passes.
        let doc = vec![block(
            BlockKind::Paragraph,
            json!({"text": "some <strong>bold</strong> and <em>italic</em>"}),
        )];
        assert!(validate(&doc).is_ok());
    }

    #[test]
    fn url_scheme_is_restricted() {
        assert!(validate(&[block(
            BlockKind::Button,
            json!({"label": "x", "href": "ftp://bad.example"})
        )])
        .is_err());
        assert!(validate(&[block(
            BlockKind::Button,
            json!({"label": "x", "href": "/relative"})
        )])
        .is_ok());
        assert!(validate(&[block(
            BlockKind::Button,
            json!({"label": "x", "href": "#anchor"})
        )])
        .is_ok());
    }

    #[test]
    fn code_language_is_constrained() {
        assert!(validate(&[block(
            BlockKind::Code,
            json!({"code": "x", "language": "rust"})
        )])
        .is_ok());
        assert!(validate(&[block(
            BlockKind::Code,
            json!({"code": "x", "language": "not a language!!"})
        )])
        .is_err());
        assert!(validate(&[block(BlockKind::Code, json!({"language": "rust"}))]).is_err());
    }

    #[test]
    fn depth_limit_is_enforced() {
        let mut block = block(BlockKind::Group, json!({}));
        for _ in 0..(super::MAX_DEPTH + 2) {
            block = Block {
                kind: BlockKind::Group,
                plugin_kind: None,
                attrs: serde_json::Value::Null,
                children: vec![block],
            };
        }
        let err = validate(&[block]).unwrap_err().to_string();
        assert!(err.contains("depth"), "err: {err}");
    }

    #[test]
    fn block_count_limit_is_enforced() {
        let doc = vec![block(BlockKind::Paragraph, json!({"text": "x"})); super::MAX_BLOCKS + 1];
        let err = validate(&doc).unwrap_err().to_string();
        assert!(err.contains("maximum"), "err: {err}");
    }

    #[test]
    fn attrs_must_be_object() {
        let doc = vec![block(BlockKind::Paragraph, json!(["not", "an", "object"]))];
        assert!(validate(&doc).is_err());
    }

    #[test]
    fn duplicate_headings_get_unique_anchors_without_failing() {
        let doc = vec![
            block(BlockKind::Heading, json!({"level": 2, "text": "Same"})),
            block(BlockKind::Heading, json!({"level": 2, "text": "Same"})),
        ];
        assert!(validate(&doc).is_ok());
    }
}

#[cfg(test)]
mod plugin_block_tests {
    use super::{is_plugin_kind_name, validate, MAX_PLUGIN_ATTRS_BYTES};
    use crate::block::doc::{BlockDocument, SCHEMA_VERSION};
    use crate::block::types::{Block, BlockKind};
    use serde_json::json;

    fn doc(blocks: &serde_json::Value) -> Result<BlockDocument, vyasa_common::AppError> {
        BlockDocument::from_json(json!({"schema_version": SCHEMA_VERSION, "blocks": blocks}))
    }

    #[test]
    fn a_plugin_block_round_trips_through_the_wire_format() {
        // The whole point: this document used to fail to deserialize, so a
        // plugin block could not be stored at all.
        let parsed = doc(&json!([{"kind": "acme/chart", "attrs": {"series": [1, 2]}}]))
            .expect("plugin blocks parse");
        assert_eq!(parsed.blocks[0].kind, BlockKind::Plugin);
        assert_eq!(parsed.blocks[0].plugin_kind.as_deref(), Some("acme/chart"));
        assert_eq!(parsed.blocks[0].kind_name(), "acme/chart");

        // And back out again unchanged: `kind` is the plugin name, not
        // the literal string "plugin".
        let out = parsed.to_json();
        assert_eq!(out["blocks"][0]["kind"], "acme/chart");
        assert_eq!(BlockDocument::from_json(out).expect("reparse"), parsed);
    }

    #[test]
    fn core_blocks_still_serialize_exactly_as_before() {
        let parsed = doc(&json!([{"kind": "separator"}])).expect("parse");
        let out = parsed.to_json();
        assert_eq!(out["blocks"][0]["kind"], "separator");
        // attrs and children stay omitted when empty.
        assert!(out["blocks"][0].get("attrs").is_none(), "{out}");
        assert!(out["blocks"][0].get("children").is_none(), "{out}");
    }

    #[test]
    fn an_unnamespaced_kind_is_still_rejected() {
        // Anything without a slash is a typo for a core kind, not a
        // plugin block, and must keep failing loudly.
        for bad in ["nope", "plugin", "acme/", "/chart", "Acme/Chart", "acme/-x"] {
            let err = doc(&json!([{"kind": bad}])).unwrap_err().to_string();
            assert!(err.contains("unknown block kind"), "{bad}: {err}");
        }
    }

    #[test]
    fn plugin_kind_names_are_namespaced_and_lowercase() {
        assert!(is_plugin_kind_name("acme/chart"));
        assert!(is_plugin_kind_name("a1/b2-c3"));
        assert!(!is_plugin_kind_name("acme/chart/extra"));
        assert!(!is_plugin_kind_name(&format!("acme/{}", "x".repeat(33))));
    }

    #[test]
    fn reserved_attrs_and_oversized_attrs_are_refused() {
        let err = validate(&[Block {
            kind: BlockKind::Plugin,
            plugin_kind: Some(String::from("acme/chart")),
            attrs: json!({"__vyasa_resolved_html": "<b>forged</b>"}),
            children: Vec::new(),
        }])
        .unwrap_err()
        .to_string();
        assert!(err.contains("reserved"), "{err}");

        let big = "x".repeat(MAX_PLUGIN_ATTRS_BYTES + 1);
        let err = validate(&[Block {
            kind: BlockKind::Plugin,
            plugin_kind: Some(String::from("acme/chart")),
            attrs: json!({"blob": big}),
            children: Vec::new(),
        }])
        .unwrap_err()
        .to_string();
        assert!(err.contains("attrs exceed"), "{err}");
    }

    #[test]
    fn plugin_blocks_cannot_have_children() {
        let err = validate(&[Block {
            kind: BlockKind::Plugin,
            plugin_kind: Some(String::from("acme/chart")),
            attrs: serde_json::Value::Null,
            children: vec![Block::new(BlockKind::Separator, serde_json::Value::Null)],
        }])
        .unwrap_err()
        .to_string();
        assert!(err.contains("cannot have children"), "{err}");
    }
}
