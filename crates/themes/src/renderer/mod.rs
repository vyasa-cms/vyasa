//! Block renderer: content blocks → sanitized HTML.
//!
//! Every text fragment is HTML-escaped or passed through an ammonia
//! allowlist at this boundary — the single sanitization choke point for
//! content. Layout chrome and templates never re-introduce raw strings.

pub mod blocks;
pub mod embed;
pub mod highlight;
pub mod markdown;

pub use blocks::{
    render_block, render_blocks, timed_boundary_within, toc_from_blocks, RenderError,
};

/// Plain-text projection of a JSON value (strings kept, others blanked);
/// used for TOC labels.
pub(crate) fn plain_text(v: Option<&serde_json::Value>) -> String {
    v.and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

use std::collections::{HashMap, HashSet};

/// HTML-escape untrusted text.
/// HTML-escapes text for attribute or content position. Public because
/// the host wraps plugin section output and needs the same escaper the
/// renderer trusts, not a second copy of it.
#[must_use]
pub fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn set<const N: usize>(items: [&'static str; N]) -> HashSet<&'static str> {
    items.into_iter().collect()
}

/// The first image in a document, as the URL a page would load it from.
///
/// Used for the social-card image: an entry with a picture in it should
/// share with that picture, and asking an author to set a second "SEO
/// image" for the one they already placed is a question with an obvious
/// answer.
#[must_use]
pub fn first_image(blocks: &[vyasa_core::block::Block]) -> Option<String> {
    for block in blocks {
        if block.kind == vyasa_core::BlockKind::Image || block.kind == vyasa_core::BlockKind::Cover
        {
            if let Some(src) = crate::renderer::blocks::media_src_of(&block.attrs) {
                return Some(src);
            }
        }
        if let Some(found) = first_image(&block.children) {
            return Some(found);
        }
    }
    None
}

/// The readable prose of a block document, for excerpts and meta
/// descriptions. Structure and markup are dropped; block text is joined
/// with spaces in document order.
#[must_use]
pub fn blocks_to_text(blocks: &[vyasa_core::block::Block]) -> String {
    fn walk(blocks: &[vyasa_core::block::Block], out: &mut String) {
        for block in blocks {
            for field in ["text", "caption", "code"] {
                let text = plain_text(block.attrs.get(field));
                if !text.trim().is_empty() {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(text.trim());
                }
            }
            walk(&block.children, out);
        }
    }
    let mut out = String::new();
    walk(blocks, &mut out);
    out
}

/// Conservative allowlist for visitor-authored comment bodies.
#[must_use]
pub fn sanitize_comment_html(html: &str) -> String {
    ammonia::Builder::default()
        .tags(set(["p", "br", "strong", "em", "code", "a"]))
        .tag_attributes(HashMap::from([("a", set(["href", "title"]))]))
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(html)
        .to_string()
}

/// Ammonia allowlist for inline rich text inside paragraph/quote/caption.
///
/// The tag set is `vyasa_core::block::INLINE_TAGS` so the editor, the
/// validator and the renderer cannot drift apart.
pub(crate) fn sanitize_inline_html(html: &str) -> String {
    ammonia::Builder::default()
        .tags(vyasa_core::block::INLINE_TAGS.iter().copied().collect())
        .tag_attributes(HashMap::from([("a", set(["href", "title"]))]))
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer"))
        .clean(html)
        .to_string()
}

/// Ammonia allowlist for the raw-HTML block escape hatch.
/// Sanitizes HTML produced by a plugin block.
///
/// Same allowlist as [`sanitize_raw_html`] plus `class`, which plugin
/// blocks need in order to be styled by the CSS the plugin ships. `class`
/// carries no behaviour, so widening to it costs nothing; every attribute
/// that does carry behaviour (`on*`, `style`, `srcset`) stays out, as do
/// `script`, `iframe` and `form`.
#[must_use]
pub fn sanitize_plugin_html(html: &str) -> String {
    ammonia::Builder::default()
        .tags(set([
            "a",
            "p",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "ul",
            "ol",
            "li",
            "blockquote",
            "pre",
            "code",
            "strong",
            "em",
            "b",
            "i",
            "u",
            "s",
            "br",
            "hr",
            "span",
            "div",
            "img",
            "figure",
            "figcaption",
            "table",
            "thead",
            "tbody",
            "tr",
            "th",
            "td",
            "dl",
            "dt",
            "dd",
            "small",
            "time",
        ]))
        .tag_attributes(HashMap::from([
            ("a", set(["href", "title", "class"])),
            (
                "img",
                set(["src", "alt", "title", "width", "height", "class", "loading"]),
            ),
            ("div", set(["class"])),
            ("span", set(["class"])),
            ("p", set(["class"])),
            ("ul", set(["class"])),
            ("ol", set(["class"])),
            ("li", set(["class"])),
            ("table", set(["class"])),
            ("figure", set(["class"])),
            ("time", set(["class", "datetime"])),
        ]))
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer"))
        .clean(html)
        .to_string()
}

pub(crate) fn sanitize_raw_html(html: &str) -> String {
    ammonia::Builder::default()
        .tags(set([
            "a",
            "p",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "ul",
            "ol",
            "li",
            "blockquote",
            "pre",
            "code",
            "strong",
            "em",
            "b",
            "i",
            "u",
            "s",
            "br",
            "hr",
            "span",
            "div",
            "img",
            "figure",
            "figcaption",
            "table",
            "thead",
            "tbody",
            "tr",
            "th",
            "td",
        ]))
        .tag_attributes(HashMap::from([
            ("a", set(["href", "title"])),
            ("img", set(["src", "alt", "title", "width", "height"])),
        ]))
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer"))
        .clean(html)
        .to_string()
}
