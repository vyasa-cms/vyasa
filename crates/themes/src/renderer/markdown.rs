//! Blocks → Markdown: the machine-readable mirror of an entry.
//!
//! AI answer engines cite what they can read, and what they read best is
//! plain markdown at a real URL. This renders the same block document
//! the HTML renderer consumes, so the `.md` twin of a page never drifts
//! from the page — one source, two serializations.
//!
//! Inline text attrs hold allowlisted HTML (`INLINE_TAGS`); a small
//! converter maps those tags to markdown and strips the rest, escaping
//! nothing back in — the output is text, not HTML.

use std::fmt::Write as _;

use vyasa_core::block::{Block, BlockKind};

/// Renders a whole document. Blocks that have no textual meaning
/// (separators, page breaks) become idiomatic markdown; blocks that are
/// purely visual chrome (cover backgrounds, embeds) reduce to their text
/// or a link.
#[must_use]
pub fn blocks_to_markdown(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        let piece = block_md(block, 0);
        if piece.trim().is_empty() {
            continue;
        }
        out.push_str(piece.trim_end());
        out.push_str("\n\n");
    }
    out.trim_end().to_owned() + "\n"
}

fn block_md(block: &Block, depth: usize) -> String {
    let attrs = &block.attrs;
    let text = |field: &str| {
        attrs
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(inline_to_md)
            .unwrap_or_default()
    };
    match block.kind {
        BlockKind::Paragraph => text("text"),
        BlockKind::Heading => {
            let level = attrs
                .get("level")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(2)
                .clamp(1, 6);
            let level = usize::try_from(level).unwrap_or(2);
            format!("{} {}", "#".repeat(level), text("text"))
        }
        BlockKind::List => list_md(block, depth, false),
        BlockKind::Quote => {
            let body = text("text");
            let cite = attrs
                .get("citation")
                .and_then(serde_json::Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .map(|c| format!("\n> — {}", inline_to_md(c)))
                .unwrap_or_default();
            format!("> {}{cite}", body.replace('\n', "\n> "))
        }
        BlockKind::Code => {
            let code = attrs
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let lang = attrs
                .get("language")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!("```{lang}\n{code}\n```")
        }
        BlockKind::Image
        | BlockKind::Cover
        | BlockKind::Video
        | BlockKind::Audio
        | BlockKind::File
        | BlockKind::Embed => media_md(block, &text("text")),
        BlockKind::Separator => "---".to_owned(),
        BlockKind::PageBreak | BlockKind::Html => String::new(),
        BlockKind::Details => {
            let summary = text("summary");
            let body = children_md(block, depth);
            if summary.is_empty() {
                body
            } else {
                format!("**{summary}**\n\n{body}")
            }
        }
        BlockKind::Table => table_md(block),
        BlockKind::Button => {
            let label = text("label");
            match attrs.get("url").and_then(serde_json::Value::as_str) {
                Some(url) if !label.is_empty() => format!("[{label}]({url})"),
                _ => label,
            }
        }
        BlockKind::Gallery => {
            let mut out = String::new();
            for child in &block.children {
                let piece = block_md(child, depth);
                if !piece.is_empty() {
                    let _ = writeln!(out, "{piece}");
                }
            }
            out.trim_end().to_owned()
        }
        // Containers and anything future: the children carry the meaning.
        _ => children_md(block, depth),
    }
}

/// Media-family blocks: an image line, a labelled link, or a bare URL.
fn media_md(block: &Block, overlay: &str) -> String {
    let attrs = &block.attrs;
    match block.kind {
        BlockKind::Image | BlockKind::Cover => {
            let alt = attrs
                .get("alt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            match (super::blocks::media_src_of(attrs), overlay.is_empty()) {
                (Some(src), true) => format!("![{alt}]({src})"),
                (Some(src), false) => format!("![{alt}]({src})\n\n{overlay}"),
                (None, false) => overlay.to_owned(),
                (None, true) => String::new(),
            }
        }
        BlockKind::Embed => attrs
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(|u| format!("<{u}>"))
            .unwrap_or_default(),
        _ => {
            let label = attrs
                .get("caption")
                .and_then(serde_json::Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .map_or_else(|| block.kind.as_str().to_owned(), inline_to_md);
            super::blocks::media_src_of(attrs)
                .map(|src| format!("[{label}]({src})"))
                .unwrap_or_default()
        }
    }
}

fn children_md(block: &Block, depth: usize) -> String {
    if depth > 8 {
        return String::new();
    }
    let mut out = String::new();
    for child in &block.children {
        let piece = block_md(child, depth + 1);
        if !piece.trim().is_empty() {
            out.push_str(piece.trim_end());
            out.push_str("\n\n");
        }
    }
    out.trim_end().to_owned()
}

fn list_md(block: &Block, depth: usize, _nested: bool) -> String {
    let ordered = block
        .attrs
        .get("ordered")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let indent = "  ".repeat(depth.min(6));
    let mut out = String::new();
    let mut n = 0usize;
    for item in &block.children {
        if item.kind == BlockKind::List {
            out.push_str(&list_md(item, depth + 1, true));
            continue;
        }
        n += 1;
        let marker = if ordered {
            format!("{n}.")
        } else {
            "-".to_owned()
        };
        let text = item
            .attrs
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(inline_to_md)
            .unwrap_or_default();
        let _ = writeln!(out, "{indent}{marker} {text}");
        for sub in &item.children {
            if sub.kind == BlockKind::List {
                out.push_str(&list_md(sub, depth + 1, true));
            }
        }
    }
    out
}

fn table_md(block: &Block) -> String {
    // Rows are children with cell children; cells carry `text`.
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in &block.children {
        let cells: Vec<String> = row
            .children
            .iter()
            .map(|c| {
                c.attrs
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(inline_to_md)
                    .unwrap_or_default()
                    .replace('|', "\\|")
            })
            .collect();
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    let Some(first) = rows.first() else {
        return String::new();
    };
    let mut out = format!("| {} |\n", first.join(" | "));
    let _ = writeln!(
        out,
        "|{}",
        " --- |".repeat(first.len().max(1)).trim_start_matches(' ')
    );
    for row in rows.iter().skip(1) {
        let _ = writeln!(out, "| {} |", row.join(" | "));
    }
    out.trim_end().to_owned()
}

/// Allowlisted inline HTML → markdown: `<strong>`/`<b>` become `**`,
/// `<em>`/`<i>` become `*`, `<code>` becomes backticks, `<a href>`
/// becomes a link, `<br>` a newline; every other tag is dropped and its
/// text kept. Entities are resolved (the common five).
pub(crate) fn inline_to_md(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let mut link_href: Option<String> = None;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[start + 1..start + end];
        rest = &rest[start + end + 1..];
        let (closing, name) = match tag.strip_prefix('/') {
            Some(n) => (true, n),
            None => (false, tag),
        };
        let name = name
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_ascii_lowercase();
        match name.as_str() {
            "strong" | "b" => out.push_str("**"),
            "em" | "i" => out.push('*'),
            "code" => out.push('`'),
            "s" | "del" => out.push_str("~~"),
            "br" => out.push('\n'),
            "a" => {
                if closing {
                    if let Some(href) = link_href.take() {
                        let _ = write!(out, "]({href})");
                    }
                } else if let Some(href) = attr_of(tag, "href") {
                    out.push('[');
                    link_href = Some(href);
                }
            }
            _ => {}
        }
    }
    out.push_str(rest);
    if link_href.is_some() {
        out.push(']'); // unclosed link: close the bracket, drop the target
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn attr_of(tag: &str, name: &str) -> Option<String> {
    let idx = tag.find(&format!("{name}=\""))?;
    let rest = &tag[idx + name.len() + 2..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn block(kind: &str, attrs: &serde_json::Value) -> Block {
        serde_json::from_value(serde_json::json!({
            "kind": kind, "attrs": attrs, "children": []
        }))
        .unwrap()
    }

    fn with_children(kind: &str, attrs: &serde_json::Value, children: Vec<Block>) -> Block {
        let mut b = block(kind, attrs);
        b.children = children;
        b
    }

    #[test]
    fn a_document_reads_like_markdown_not_markup() {
        let doc = vec![
            block(
                "heading",
                &serde_json::json!({"level": 2, "text": "Why <em>Rust</em>?"}),
            ),
            block(
                "paragraph",
                &serde_json::json!({"text": "It is <strong>fast</strong> — see <a href=\"/docs\">the docs</a>."}),
            ),
            block(
                "code",
                &serde_json::json!({"language": "rust", "code": "fn main() {}"}),
            ),
            block("separator", &serde_json::json!({})),
            block("image", &serde_json::json!({"mediaId": 7, "alt": "A crab"})),
        ];
        let md = blocks_to_markdown(&doc);
        assert_eq!(
            md,
            "## Why *Rust*?\n\nIt is **fast** — see [the docs](/docs).\n\n\
             ```rust\nfn main() {}\n```\n\n---\n\n![A crab](/api/v1/media/7/raw)\n"
        );
    }

    #[test]
    fn lists_quotes_and_containers_flatten_faithfully() {
        let doc = vec![
            with_children(
                "list",
                &serde_json::json!({"ordered": true}),
                vec![
                    block("paragraph", &serde_json::json!({"text": "First"})),
                    block("paragraph", &serde_json::json!({"text": "Second"})),
                ],
            ),
            block(
                "quote",
                &serde_json::json!({"text": "Simplicity", "citation": "Someone"}),
            ),
            with_children(
                "group",
                &serde_json::json!({}),
                vec![block("paragraph", &serde_json::json!({"text": "Inside"}))],
            ),
        ];
        let md = blocks_to_markdown(&doc);
        assert!(md.contains("1. First\n2. Second"), "{md}");
        assert!(md.contains("> Simplicity\n> — Someone"), "{md}");
        assert!(md.contains("Inside"), "{md}");
    }

    #[test]
    fn hostile_inline_html_degrades_to_text() {
        assert_eq!(
            inline_to_md("a <script>evil()</script> b &amp; c"),
            "a evil() b & c"
        );
        assert_eq!(inline_to_md("<a href=\"/x\">left open"), "[left open]");
    }
}
