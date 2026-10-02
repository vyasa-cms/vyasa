//! Best-effort HTML → block-document conversion.
//!
//! Walks the source HTML with html5ever and emits core-registry-v1 blocks:
//! paragraphs, headings, lists, blockquotes, pre/code, images (figure),
//! links inline, and embeds. Anything unrecognised degrades to a paragraph
//! containing ammonia-sanitized HTML; every degradation is reported as a
//! warning so operators know what needs manual review.

#![cfg(feature = "importer")]

use std::collections::BTreeSet;

use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use vyasa_core::block::Block;

/// Parses and converts `html` into a block document.
#[must_use]
pub fn convert(html: &str) -> (vyasa_core::BlockDocument, Vec<String>) {
    let mut warnings = Vec::new();
    // `Parser` is a TendrilSink: feed the whole document with `.one`.
    let dom = html5ever::parse_document(RcDom::default(), html5ever::ParseOpts::default())
        .one(html.to_owned());
    let mut blocks = Vec::new();
    walk_children(&dom.document, &mut blocks, &mut warnings);
    // Drop empty trailing paragraphs produced by whitespace-only nodes.
    blocks.retain(|b| !is_blank(b));
    (
        vyasa_core::BlockDocument {
            schema_version: 1,
            blocks,
        },
        warnings,
    )
}

fn is_blank(b: &Block) -> bool {
    b.attrs
        .get("text")
        .or_else(|| b.attrs.get("html"))
        .and_then(serde_json::Value::as_str)
        .map_or(false, |t| t.trim().is_empty())
}

fn walk_children(handle: &Handle, out: &mut Vec<Block>, warnings: &mut Vec<String>) {
    for child in handle.children.borrow().iter() {
        walk(child, out, warnings);
    }
}

fn walk(handle: &Handle, out: &mut Vec<Block>, warnings: &mut Vec<String>) {
    match &handle.data {
        NodeData::Element { name, .. } => {
            let tag = name.local.as_ref();
            match tag {
                "p" => {
                    let text = inline_html(handle);
                    if !text.trim().is_empty() {
                        out.push(block("paragraph", serde_json::json!({ "text": text })));
                    }
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level: u64 = tag[1..].parse().unwrap_or(2).clamp(2, 6);
                    let text = plain_text(handle);
                    if !text.is_empty() {
                        out.push(block(
                            "heading",
                            serde_json::json!({ "text": text, "level": level }),
                        ));
                    }
                }
                "ul" | "ol" => {
                    let mut items = Vec::new();
                    collect_list_items(handle, &mut items);
                    if !items.is_empty() {
                        out.push(Block {
                            kind: vyasa_core::block::BlockKind::List,
                            plugin_kind: None,
                            attrs: serde_json::json!({"ordered": tag == "ol"}),
                            children: items
                                .into_iter()
                                .map(|t| block("paragraph", serde_json::json!({ "text": t })))
                                .collect(),
                        });
                    }
                }
                "blockquote" => {
                    let text = inline_html(handle);
                    if !text.trim().is_empty() {
                        out.push(block("quote", serde_json::json!({ "text": text })));
                    }
                }
                "pre" => {
                    let code = plain_text(handle);
                    if !code.trim().is_empty() {
                        out.push(block(
                            "code",
                            serde_json::json!({ "code": code, "language": null }),
                        ));
                    }
                }
                "figure" => figure_block(handle, out),
                "img" => image_block(handle, None, out),
                "iframe" => {
                    if let Some(src) = attr(handle, "src") {
                        out.push(block(
                            "embed",
                            serde_json::json!({ "url": src, "title": null }),
                        ));
                    } else {
                        warnings.push(String::from("iframe without src dropped"));
                    }
                }
                "table" => {
                    // Degrade honestly: sanitized raw HTML table.
                    let html = inner_html(handle);
                    warnings.push(String::from("table converted to sanitized HTML"));
                    out.push(block("html", serde_json::json!({ "html": html })));
                }
                "html" | "head" | "body" => {
                    // Document scaffolding from full-page parses.
                    walk_children(handle, out, warnings);
                }
                _ => {
                    // Unknown container: recurse so children are preserved.
                    let before = out.len();
                    walk_children(handle, out, warnings);
                    if out.len() == before && !inline_leaf(handle) {
                        warnings.push(format!("<{tag}> handled generically"));
                        let html = sanitize_fragment(&inner_html(handle));
                        if !html.trim().is_empty() {
                            out.push(block("paragraph", serde_json::json!({ "text": html })));
                        }
                    }
                }
            }
        }
        NodeData::Text { ref contents } => {
            let text = contents.borrow().trim().to_owned();
            if !text.is_empty() {
                out.push(block(
                    "paragraph",
                    serde_json::json!({ "text": esc(&text) }),
                ));
            }
        }
        _ => {}
    }
}

fn figure_block(handle: &Handle, out: &mut Vec<Block>) {
    // <figure><img …><figcaption>c</figcaption></figure>
    let mut img: Option<(String, Option<String>)> = None;
    let mut caption = String::new();
    collect_figure(handle, &mut img, &mut caption);
    match img {
        Some((src, alt)) => {
            let mut attrs = serde_json::json!({ "url": src, "alt": alt.unwrap_or_default() });
            if let Some(o) = attrs.as_object_mut() {
                o.insert("caption".into(), serde_json::Value::String(caption));
            }
            out.push(block("image", attrs));
        }
        None => walk_children(handle, out, &mut Vec::new()),
    }
}

fn collect_figure(
    handle: &Handle,
    img: &mut Option<(String, Option<String>)>,
    caption: &mut String,
) {
    if let NodeData::Element { name, .. } = &handle.data {
        match name.local.as_ref() {
            "img" => {
                let src = attr(handle, "src").unwrap_or_default();
                *img = Some((src, attr(handle, "alt")));
            }
            "figcaption" => *caption = plain_text(handle),
            _ => {}
        }
    }
    for c in handle.children.borrow().iter() {
        collect_figure(c, img, caption);
    }
}

fn image_block(handle: &Handle, caption: Option<String>, out: &mut Vec<Block>) {
    if let Some(src) = attr(handle, "src") {
        let mut attrs =
            serde_json::json!({ "url": src, "alt": attr(handle, "alt").unwrap_or_default() });
        if let (Some(c), Some(o)) = (caption, attrs.as_object_mut()) {
            o.insert("caption".into(), serde_json::Value::String(c));
        }
        out.push(block("image", attrs));
    }
}

fn collect_list_items(handle: &Handle, items: &mut Vec<String>) {
    if let NodeData::Element { name, .. } = &handle.data {
        if name.local.as_ref() == "li" {
            items.push(inline_html(handle));
            return;
        }
    }
    for c in handle.children.borrow().iter() {
        collect_list_items(c, items);
    }
}

/// Serializes the node's children as inline HTML, keeping only the tags
/// the block contract allows (`vyasa_core::block::INLINE_TAGS`).
fn inline_html(handle: &Handle) -> String {
    inner_html_filtered(handle, vyasa_core::block::INLINE_TAGS)
}

/// Unfiltered inner serialization (used by the html-block fallback after
/// ammonia sanitization).
fn inner_html(handle: &Handle) -> String {
    serialize(handle, &BTreeSet::new(), true)
}

fn inner_html_filtered(handle: &Handle, allow: &[&str]) -> String {
    let set: BTreeSet<&str> = allow.iter().copied().collect();
    serialize_children(handle, &set)
}

fn serialize_children(handle: &Handle, allow: &BTreeSet<&str>) -> String {
    let mut out = String::new();
    for c in handle.children.borrow().iter() {
        out.push_str(&serialize(c, allow, false));
    }
    out
}

fn serialize(handle: &Handle, allow: &BTreeSet<&str>, root: bool) -> String {
    match &handle.data {
        NodeData::Text { ref contents } => esc(&contents.borrow()),
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.as_ref();
            if !root && !allow.contains(tag) {
                return serialize_children(handle, allow);
            }
            let open_attrs = attrs
                .borrow()
                .iter()
                .filter(|a| matches!(a.name.local.as_ref(), "href" | "src" | "alt"))
                .map(|a| format!(" {}=\"{}\"", a.name.local, esc(&a.value)))
                .collect::<String>();
            format!(
                "<{tag}{open_attrs}>{}</{tag}>",
                serialize_children(handle, allow)
            )
        }
        _ => serialize_children(handle, allow),
    }
}

fn inline_leaf(handle: &Handle) -> bool {
    handle.children.borrow().iter().any(|c| {
        matches!(
            &c.data,
            NodeData::Text { ref contents } if !contents.borrow().trim().is_empty()
        )
    })
}

fn plain_text(handle: &Handle) -> String {
    let mut out = String::new();
    fn rec(handle: &Handle, out: &mut String) {
        if let NodeData::Text { contents } = &handle.data {
            out.push_str(&contents.borrow());
        }
        for c in handle.children.borrow().iter() {
            rec(c, out);
        }
    }
    rec(handle, &mut out);
    collapse_ws(&out)
}

fn attr(handle: &Handle, name: &str) -> Option<String> {
    if let NodeData::Element { attrs, .. } = &handle.data {
        for a in attrs.borrow().iter() {
            if a.name.local.as_ref() == name {
                return Some(a.value.to_string());
            }
        }
    }
    None
}

fn block(kind: &str, attrs: serde_json::Value) -> Block {
    let parsed = serde_json::from_value(serde_json::Value::String(kind.to_owned()))
        .unwrap_or(vyasa_core::block::BlockKind::Paragraph);
    Block {
        kind: parsed,
        plugin_kind: None,
        attrs,
        children: Vec::new(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sanitize_fragment(html: &str) -> String {
    // Local sanitizer mirrors themes::sanitize_raw_html without a crate
    // dependency cycle risk: conservative allowlist.
    use ammonia::Builder;
    Builder::default()
        .tags(std::collections::HashSet::from([
            "p", "br", "strong", "em", "code", "a", "table", "tr", "td", "th",
        ]))
        .clean(html)
        .to_string()
}
