//! Markdown (plus the component whitelist) to Vyasa block documents.
//!
//! The output is the wire format of `docs/BLOCKS.md`: core kinds wherever
//! one exists (`paragraph`, `heading`, `list`, `quote`, `code`, `table`,
//! `image`, `separator`, `callout`, `html`, `group`) and this plugin's own
//! kinds (`docs/tabs`, `docs/steps`, `docs/params`, `docs/code-group`,
//! `docs/file-tree`) only for what core cannot express.

use std::collections::BTreeMap;

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{json, Value};

use crate::components::{self, Node};

/// Where relative links and images in a page point.
#[derive(Debug, Clone, Default)]
pub struct Links {
    /// The docs folder inside the repository (`docs`, or empty).
    pub folder: String,
    /// The page's directory inside the docs folder (`guides`, or empty).
    pub dir: String,
    /// Pages of this import by their path inside the folder
    /// (`guides/install.md`) to their site path (`/docs/guides/install`).
    pub known: BTreeMap<String, String>,
    /// `https://github.com/o/r/blob/<sha>/` for links to files that are not pages.
    pub blob_base: String,
    /// `https://raw.githubusercontent.com/o/r/<sha>/` for images.
    pub raw_base: String,
}

/// The result of converting one page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Converted {
    /// The first level-1 heading, when the page had one (it becomes the
    /// title and is not repeated in the body).
    pub h1: Option<String>,
    /// The blocks.
    pub blocks: Vec<Value>,
    /// Things that were degraded or dropped.
    pub warnings: Vec<String>,
}

/// Converts a page body.
#[must_use]
pub fn convert(body: &str, links: &Links) -> Converted {
    let (nodes, mut warnings) = components::parse(body);
    let mut cx = Cx {
        links,
        h1: None,
        warnings: Vec::new(),
    };
    let blocks = nodes_to_blocks(&nodes, &mut cx);
    warnings.append(&mut cx.warnings);
    Converted {
        h1: cx.h1,
        blocks: group_code(blocks),
        warnings,
    }
}

/// Escapes text for an HTML text node or attribute.
#[must_use]
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

struct Cx<'a> {
    links: &'a Links,
    h1: Option<String>,
    warnings: Vec<String>,
}

fn block(kind: &str, attrs: Value) -> Value {
    json!({ "kind": kind, "attrs": attrs, "children": [] })
}

fn block_with(kind: &str, attrs: Value, children: Vec<Value>) -> Value {
    json!({ "kind": kind, "attrs": attrs, "children": children })
}

fn nodes_to_blocks(nodes: &[Node], cx: &mut Cx) -> Vec<Value> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            Node::Md(text) => out.extend(markdown_blocks(text, cx)),
            Node::Comp { name, children, .. } => match name.as_str() {
                "Callout" => {
                    let tone = tone_of(
                        node.attr("type")
                            .or_else(|| node.attr("tone"))
                            .unwrap_or("note"),
                    );
                    let title = node.attr("title").map(esc).unwrap_or_default();
                    out.push(block_with(
                        "callout",
                        json!({ "tone": tone, "title": title }),
                        nodes_to_blocks(children, cx),
                    ));
                }
                "Tabs" => out.push(tabs(node, children, cx)),
                "Steps" => out.push(steps(children, cx)),
                "Params" => out.push(params(children, cx)),
                other => {
                    cx.warnings.push(format!(
                        "<{other}> outside its parent; its content was kept"
                    ));
                    out.extend(nodes_to_blocks(children, cx));
                }
            },
        }
    }
    out
}

fn tone_of(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "tip" | "success" => "tip",
        "warn" | "warning" => "warning",
        "danger" | "error" | "caution" => "danger",
        _ => "note",
    }
}

fn parse_items(raw: &str) -> Vec<String> {
    let t = raw.trim().trim_start_matches('[').trim_end_matches(']');
    t.split(',')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn tabs(node: &Node, children: &[Node], cx: &mut Cx) -> Value {
    let mut labels = node.attr("items").map(parse_items).unwrap_or_default();
    let mut panes = Vec::new();
    for child in children {
        match child {
            Node::Comp { name, children, .. } if name == "Tab" => {
                if panes.len() >= labels.len() {
                    let label = child
                        .attr("title")
                        .or_else(|| child.attr("value"))
                        .unwrap_or("Tab");
                    labels.push(label.to_owned());
                }
                panes.push(block_with(
                    "group",
                    json!({}),
                    nodes_to_blocks(children, cx),
                ));
            }
            Node::Md(t) if t.trim().is_empty() => {}
            other => {
                cx.warnings
                    .push("<Tabs> holds only <Tab>; other content was dropped".to_owned());
                let _ = other;
            }
        }
    }
    labels.truncate(panes.len());
    let group = node.attr("groupId").map(str::to_owned);
    let mut md = String::new();
    for (label, pane) in labels.iter().zip(&panes) {
        md.push_str(&format!(
            "**{label}**\n\n{}\n\n",
            blocks_markdown(
                pane.get("children")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice)
            )
        ));
    }
    block_with(
        "docs/tabs",
        json!({ "labels": labels, "group": group, "markdown": md.trim_end() }),
        panes,
    )
}

fn steps(children: &[Node], cx: &mut Cx) -> Value {
    let has_step = children
        .iter()
        .any(|c| matches!(c, Node::Comp { name, .. } if name == "Step"));
    let mut items: Vec<Value> = Vec::new();
    if has_step {
        for child in children {
            if let Node::Comp { name, children, .. } = child {
                if name == "Step" {
                    items.push(block_with(
                        "group",
                        json!({}),
                        nodes_to_blocks(children, cx),
                    ));
                }
            }
        }
    } else {
        // Fumadocs style: every `###` heading starts a step.
        let all = nodes_to_blocks(children, cx);
        let mut current: Vec<Value> = Vec::new();
        for b in all {
            let is_h3 = b["kind"] == "heading" && b["attrs"]["level"] == 3;
            if is_h3 && !current.is_empty() {
                items.push(block_with("group", json!({}), std::mem::take(&mut current)));
            }
            current.push(b);
        }
        if !current.is_empty() {
            items.push(block_with("group", json!({}), current));
        }
    }
    let md: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(i, g)| {
            format!(
                "{}. {}",
                i + 1,
                blocks_markdown(
                    g.get("children")
                        .and_then(Value::as_array)
                        .map_or(&[][..], Vec::as_slice)
                )
                .replace('\n', "\n   ")
            )
        })
        .collect();
    block_with("docs/steps", json!({ "markdown": md.join("\n") }), items)
}

fn params(children: &[Node], cx: &mut Cx) -> Value {
    let mut rows = Vec::new();
    let mut md =
        String::from("| Name | Type | Required | Default | Description |\n|---|---|---|---|---|\n");
    for child in children {
        let Node::Comp { name, children, .. } = child else {
            continue;
        };
        if name != "Param" {
            continue;
        }
        let text = children
            .iter()
            .filter_map(|n| {
                if let Node::Md(t) = n {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let description = inline_markdown(text.trim(), cx);
        let field = |k: &str| child.attr(k).unwrap_or("").to_owned();
        let required = child.attr("required").is_some_and(|v| v != "false");
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            field("name"),
            field("type"),
            if required { "yes" } else { "no" },
            field("default"),
            text.trim().replace('\n', " ")
        ));
        rows.push(json!({
            "name": field("name"), "type": field("type"), "required": required,
            "default": field("default"), "description": description,
        }));
    }
    block(
        "docs/params",
        json!({ "rows": rows, "markdown": md.trim_end() }),
    )
}

/// A short Markdown fragment as inline HTML (paragraphs joined by `<br>`).
fn inline_markdown(src: &str, cx: &mut Cx) -> String {
    let blocks = markdown_blocks(src, cx);
    blocks
        .iter()
        .filter_map(|b| b["attrs"]["text"].as_str().map(str::to_owned))
        .collect::<Vec<_>>()
        .join("<br>")
}

/// Markdown text of blocks, for the `.md` mirror of this plugin's blocks.
fn blocks_markdown(blocks: &[Value]) -> String {
    let mut parts = Vec::new();
    for b in blocks {
        let text = b["attrs"]["text"].as_str().map(strip_tags);
        match b["kind"].as_str().unwrap_or("") {
            "paragraph" => parts.push(text.unwrap_or_default()),
            "heading" => parts.push(format!(
                "{} {}",
                "#".repeat(b["attrs"]["level"].as_u64().unwrap_or(2) as usize),
                text.unwrap_or_default()
            )),
            "code" => parts.push(format!(
                "```{}\n{}\n```",
                b["attrs"]["language"].as_str().unwrap_or(""),
                b["attrs"]["code"].as_str().unwrap_or("")
            )),
            _ => {
                if let Some(c) = b.get("children").and_then(Value::as_array) {
                    parts.push(blocks_markdown(c));
                }
            }
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join("\n\n")
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

// ---------------------------------------------------------------- Markdown

const INLINE_OK: &[&str] = &[
    "a", "b", "i", "em", "strong", "code", "br", "s", "del", "ins", "u", "mark", "sub", "sup",
];

enum Frame {
    Quote {
        kind: Option<BlockQuoteKind>,
        blocks: Vec<Value>,
    },
    List {
        ordered: bool,
        items: Vec<Value>,
    },
    Item {
        text: String,
        nested: Vec<Value>,
    },
}

#[derive(Default)]
struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
}

struct Builder<'a, 'b> {
    cx: &'a mut Cx<'b>,
    out: Vec<Value>,
    stack: Vec<Frame>,
    inline: String,
    in_link: u32,
    pending_image: Option<(String, String)>,
    image: Option<(String, String)>,
    heading: Option<u8>,
    code: Option<(String, String)>,
    table: Option<Table>,
    html: Option<String>,
    spill: Vec<Value>,
}

fn markdown_blocks(src: &str, cx: &mut Cx) -> Vec<Value> {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut b = Builder {
        cx,
        out: Vec::new(),
        stack: Vec::new(),
        inline: String::new(),
        in_link: 0,
        pending_image: None,
        image: None,
        heading: None,
        code: None,
        table: None,
        html: None,
        spill: Vec::new(),
    };
    for event in Parser::new_ext(src, opts) {
        b.event(event);
    }
    b.out
}

fn allowed_inline_tag(raw: &str) -> bool {
    let t = raw.trim();
    let Some(inner) = t.strip_prefix('<').and_then(|r| r.strip_suffix('>')) else {
        return false;
    };
    let inner = inner.trim_start_matches('/').trim_end_matches('/').trim();
    // Attributes are never allowed through, `a` included: links come from
    // Markdown syntax, where the destination is rewritten and checked.
    INLINE_OK.contains(&inner.to_ascii_lowercase().as_str())
        && inner.chars().all(|c| c.is_ascii_alphabetic())
}

impl Builder<'_, '_> {
    fn emit(&mut self, v: Value) {
        match self.stack.last_mut() {
            Some(Frame::Quote { blocks, .. }) => blocks.push(v),
            Some(Frame::Item { .. } | Frame::List { .. }) => {
                self.cx
                    .warnings
                    .push("a block inside a list item was moved after the list".to_owned());
                self.spill.push(v);
            }
            None => self.out.push(v),
        }
    }

    fn push_inline(&mut self, s: &str) {
        if let Some((_, alt)) = self.image.as_mut() {
            alt.push_str(&strip_tags(s));
            return;
        }
        self.flush_image_as_link();
        self.inline.push_str(s);
    }

    fn flush_image_as_link(&mut self) {
        if let Some((src, alt)) = self.pending_image.take() {
            self.inline
                .push_str(&format!("<a href=\"{}\">{}</a>", esc(&src), esc(&alt)));
        }
    }

    fn take_inline(&mut self) -> String {
        self.flush_image_as_link();
        std::mem::take(&mut self.inline)
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(end),
            Event::Text(t) => {
                if let Some((_, code)) = self.code.as_mut() {
                    code.push_str(&t);
                } else if let Some(h) = self.html.as_mut() {
                    h.push_str(&t);
                } else {
                    self.push_inline(&esc(&t));
                }
            }
            Event::Code(t) => self.push_inline(&format!("<code>{}</code>", esc(&t))),
            Event::Html(t) => {
                if let Some(h) = self.html.as_mut() {
                    h.push_str(&t);
                }
            }
            Event::InlineHtml(t) => {
                if allowed_inline_tag(&t) {
                    self.push_inline(t.trim());
                } else {
                    self.push_inline(&esc(&t));
                }
            }
            Event::SoftBreak => self.push_inline(" "),
            Event::HardBreak => self.push_inline("<br>"),
            Event::Rule => self.emit(block("separator", json!({}))),
            Event::TaskListMarker(done) => self.push_inline(if done { "[x] " } else { "[ ] " }),
            Event::FootnoteReference(_) => self
                .cx
                .warnings
                .push("footnotes are not supported; the reference was dropped".to_owned()),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.inline.clear();
                self.heading = Some(level as u8);
            }
            Tag::BlockQuote(kind) => self.stack.push(Frame::Quote {
                kind,
                blocks: Vec::new(),
            }),
            Tag::CodeBlock(kind) => {
                let info = match kind {
                    CodeBlockKind::Fenced(i) => i.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((info, String::new()));
            }
            Tag::HtmlBlock => self.html = Some(String::new()),
            Tag::List(first) => {
                // Text already read for the enclosing item belongs to it.
                let tail = self.take_inline();
                if let Some(Frame::Item { text, .. }) = self.stack.last_mut() {
                    text.push_str(&tail);
                }
                self.stack.push(Frame::List {
                    ordered: first.is_some(),
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.inline.clear();
                self.stack.push(Frame::Item {
                    text: String::new(),
                    nested: Vec::new(),
                });
            }
            Tag::Table(_) => self.table = Some(Table::default()),
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.row.clear();
                }
            }
            Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self.push_inline("<em>"),
            Tag::Strong => self.push_inline("<strong>"),
            Tag::Strikethrough => self.push_inline("<s>"),
            Tag::Link {
                dest_url, title, ..
            } => {
                self.in_link += 1;
                let href = resolve(self.cx.links, &dest_url, false);
                let title = if title.is_empty() {
                    String::new()
                } else {
                    format!(" title=\"{}\"", esc(&title))
                };
                self.push_inline(&format!("<a href=\"{}\"{title}>", esc(&href)));
            }
            Tag::Image { dest_url, .. } => {
                self.flush_image_as_link();
                self.image = Some((resolve(self.cx.links, &dest_url, true), String::new()));
            }
            _ => {}
        }
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph => {
                if let Some(Frame::Item { .. }) = self.stack.last() {
                    let t = self.take_inline();
                    if let Some(Frame::Item { text, .. }) = self.stack.last_mut() {
                        if !text.is_empty() && !t.is_empty() {
                            text.push_str("<br><br>");
                        }
                        text.push_str(&t);
                    }
                    return;
                }
                if let (Some((src, alt)), true) =
                    (self.pending_image.take(), self.inline.is_empty())
                {
                    self.emit(block("image", json!({ "url": src, "alt": alt })));
                    return;
                }
                let text = self.take_inline();
                if !text.trim().is_empty() {
                    self.emit(block("paragraph", json!({ "text": text })));
                }
            }
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(2);
                let text = self.take_inline();
                if level == 1 && self.cx.h1.is_none() {
                    self.cx.h1 = Some(strip_tags(&text));
                } else {
                    self.emit(block(
                        "heading",
                        json!({ "level": level.clamp(2, 6), "text": text }),
                    ));
                }
            }
            TagEnd::BlockQuote(_) => {
                if let Some(Frame::Quote { kind, blocks }) = self.stack.pop() {
                    self.finish_quote(kind, blocks);
                }
            }
            TagEnd::CodeBlock => {
                if let Some((info, code)) = self.code.take() {
                    let b = code_block(&info, &code);
                    self.emit(b);
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(h) = self.html.take() {
                    let t = h.trim();
                    if !(t.is_empty() || t.starts_with("<!--") && t.ends_with("-->")) {
                        self.emit(block("html", json!({ "html": t })));
                    }
                }
            }
            TagEnd::Item => {
                let tail = self.take_inline();
                if let Some(Frame::Item { mut text, nested }) = self.stack.pop() {
                    if text.is_empty() {
                        text = tail;
                    } else if !tail.is_empty() {
                        text.push_str("<br>");
                        text.push_str(&tail);
                    }
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(block("paragraph", json!({ "text": text })));
                        items.extend(nested);
                    }
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame::List { ordered, items }) = self.stack.pop() {
                    let list = block_with("list", json!({ "ordered": ordered }), items);
                    match self.stack.last_mut() {
                        Some(Frame::Item { nested, .. }) => nested.push(list),
                        Some(Frame::Quote { blocks, .. }) => blocks.push(list),
                        _ => self.out.push(list),
                    }
                    if !matches!(
                        self.stack.last(),
                        Some(Frame::Item { .. } | Frame::List { .. })
                    ) {
                        let spill = std::mem::take(&mut self.spill);
                        for v in spill {
                            match self.stack.last_mut() {
                                Some(Frame::Quote { blocks, .. }) => blocks.push(v),
                                _ => self.out.push(v),
                            }
                        }
                    }
                }
            }
            TagEnd::TableCell => {
                let cell = self.take_inline();
                if let Some(t) = self.table.as_mut() {
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.header = std::mem::take(&mut t.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.emit(block(
                        "table",
                        json!({ "header": t.header, "rows": t.rows }),
                    ));
                }
            }
            TagEnd::Emphasis => self.push_inline("</em>"),
            TagEnd::Strong => self.push_inline("</strong>"),
            TagEnd::Strikethrough => self.push_inline("</s>"),
            TagEnd::Link => {
                self.in_link = self.in_link.saturating_sub(1);
                self.push_inline("</a>");
            }
            TagEnd::Image => {
                if let Some((src, alt)) = self.image.take() {
                    if self.inline.is_empty() && self.in_link == 0 && self.pending_image.is_none() {
                        self.pending_image = Some((src, alt));
                    } else {
                        self.flush_image_as_link();
                        self.inline.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            esc(&src),
                            esc(&alt)
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    fn finish_quote(&mut self, kind: Option<BlockQuoteKind>, blocks: Vec<Value>) {
        if let Some(kind) = kind {
            let (tone, title) = match kind {
                BlockQuoteKind::Note => ("note", "Note"),
                BlockQuoteKind::Tip => ("tip", "Tip"),
                BlockQuoteKind::Important => ("note", "Important"),
                BlockQuoteKind::Warning => ("warning", "Warning"),
                BlockQuoteKind::Caution => ("danger", "Caution"),
            };
            self.emit(block_with(
                "callout",
                json!({ "tone": tone, "title": title }),
                blocks,
            ));
            return;
        }
        let mut text: Vec<String> = Vec::new();
        let mut rest: Vec<Value> = Vec::new();
        for b in blocks {
            if b["kind"] == "paragraph" {
                text.push(b["attrs"]["text"].as_str().unwrap_or("").to_owned());
            } else {
                rest.push(b);
            }
        }
        if !text.is_empty() {
            self.emit(block("quote", json!({ "text": text.join("<br>") })));
        }
        if !rest.is_empty() {
            self.cx
                .warnings
                .push("a block inside a quotation was moved after it".to_owned());
        }
        for b in rest {
            self.emit(b);
        }
    }
}

fn code_block(info: &str, code: &str) -> Value {
    let mut parts = info.trim().splitn(2, char::is_whitespace);
    let lang = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let meta = parts.next().unwrap_or("");
    let code = code.strip_suffix('\n').unwrap_or(code);
    if lang == "tree" {
        return block(
            "docs/file-tree",
            json!({ "tree": code, "markdown": format!("```\n{code}\n```") }),
        );
    }
    let mut attrs = json!({ "code": code, "language": if lang.is_empty() { Value::Null } else { Value::String(lang) } });
    for key in ["title", "group"] {
        if let Some(v) = meta_value(meta, key) {
            attrs[key] = Value::String(v);
        }
    }
    block("code", attrs)
}

fn meta_value(meta: &str, key: &str) -> Option<String> {
    let at = meta.find(&format!("{key}="))?;
    let rest = &meta[at + key.len() + 1..];
    match rest.chars().next()? {
        q @ ('"' | '\'') => {
            let inner = &rest[1..];
            inner.find(q).map(|e| inner[..e].to_owned())
        }
        _ => Some(rest.split_whitespace().next()?.to_owned()),
    }
}

/// Merges consecutive code blocks sharing a `group` into one code group,
/// at every depth.
fn group_code(blocks: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut run: Vec<Value> = Vec::new();
    let flush = |run: &mut Vec<Value>, out: &mut Vec<Value>| {
        if run.len() >= 2 {
            let mut md = String::new();
            let tabs: Vec<Value> = run
                .iter()
                .map(|b| {
                    let a = &b["attrs"];
                    let lang = a["language"].as_str().unwrap_or("");
                    let label = a["title"]
                        .as_str()
                        .filter(|t| !t.is_empty())
                        .or(Some(lang).filter(|l| !l.is_empty()))
                        .unwrap_or("Code");
                    md.push_str(&format!(
                        "**{label}**\n\n```{lang}\n{}\n```\n\n",
                        a["code"].as_str().unwrap_or("")
                    ));
                    json!({ "label": label, "language": lang, "code": a["code"] })
                })
                .collect();
            out.push(block(
                "docs/code-group",
                json!({ "tabs": tabs, "markdown": md.trim_end() }),
            ));
        } else {
            out.append(run);
        }
        run.clear();
    };
    for mut b in blocks {
        if let Some(children) = b.get("children").and_then(Value::as_array).cloned() {
            if !children.is_empty() {
                b["children"] = Value::Array(group_code(children));
            }
        }
        let group = (b["kind"] == "code")
            .then(|| b["attrs"]["group"].as_str().map(str::to_owned))
            .flatten();
        let same = group.is_some()
            && run
                .last()
                .is_some_and(|l| l["attrs"]["group"].as_str() == group.as_deref());
        if group.is_some() && (run.is_empty() || same) {
            run.push(b);
            continue;
        }
        flush(&mut run, &mut out);
        if group.is_some() {
            run.push(b);
        } else {
            out.push(b);
        }
    }
    flush(&mut run, &mut out);
    out
}

// ------------------------------------------------------------------ links

/// Rewrites a link or image destination found in a page.
#[must_use]
pub fn resolve(links: &Links, href: &str, image: bool) -> String {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
    {
        return href.to_owned();
    }
    if href.starts_with('#') || (href.starts_with('/') && !href.starts_with("//")) {
        return href.to_owned();
    }
    if href.contains(':') || href.starts_with("//") || href.is_empty() {
        // `javascript:`, `data:`, protocol-relative: nothing to link to.
        return "#".to_owned();
    }
    let (path, frag) = match href.split_once('#') {
        Some((p, f)) => (p, format!("#{f}")),
        None => (href, String::new()),
    };
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    let Some(repo_path) = join_normalised(&[&links.folder, &links.dir, path]) else {
        return href.to_owned();
    };
    let inside = if links.folder.is_empty() {
        Some(repo_path.as_str())
    } else {
        repo_path.strip_prefix(&format!("{}/", links.folder))
    };
    if !image {
        if let Some(inner) = inside {
            if let Some(site) = links.known.get(inner) {
                return format!("{site}{frag}");
            }
        }
    }
    let base = if image {
        &links.raw_base
    } else {
        &links.blob_base
    };
    if base.is_empty() {
        return href.to_owned();
    }
    format!("{base}{repo_path}{frag}")
}

/// Joins path fragments and resolves `.` and `..`; `None` when the result
/// would climb out of the repository.
fn join_normalised(parts: &[&str]) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    for part in parts {
        for seg in part.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    stack.pop()?;
                }
                s => stack.push(s),
            }
        }
    }
    Some(stack.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(src: &str) -> Converted {
        convert(src, &Links::default())
    }

    fn kinds(c: &Converted) -> Vec<&str> {
        c.blocks
            .iter()
            .map(|b| b["kind"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn headings_paragraphs_and_the_first_h1_as_title() {
        let c =
            plain("# Title\n\nHello **bold** and `code` and [a link](https://x.dev).\n\n## Next\n");
        assert_eq!(c.h1.as_deref(), Some("Title"));
        assert_eq!(kinds(&c), ["paragraph", "heading"]);
        assert_eq!(
            c.blocks[0]["attrs"]["text"],
            "Hello <strong>bold</strong> and <code>code</code> and <a href=\"https://x.dev\">a link</a>."
        );
        assert_eq!(c.blocks[1]["attrs"], json!({"level": 2, "text": "Next"}));
    }

    #[test]
    fn a_second_h1_becomes_an_h2() {
        let c = plain("# One\n\n# Two\n");
        assert_eq!(c.h1.as_deref(), Some("One"));
        assert_eq!(c.blocks[0]["attrs"]["level"], 2);
    }

    #[test]
    fn text_is_escaped_and_unknown_inline_html_is_text() {
        let c = plain("1 < 2 & <script>x</script> <kbd>K</kbd>\n");
        let t = c.blocks[0]["attrs"]["text"].as_str().unwrap();
        assert!(t.contains("1 &lt; 2 &amp;"), "{t}");
        assert!(t.contains("&lt;script&gt;"), "{t}");
        assert!(!t.contains("<script"), "{t}");
        assert!(t.contains("&lt;kbd&gt;"), "{t}");
    }

    #[test]
    fn lists_nest_and_numbering_is_kept() {
        let c = plain("1. one\n2. two\n   - inner a\n   - inner b\n3. three\n");
        assert_eq!(kinds(&c), ["list"]);
        let list = &c.blocks[0];
        assert_eq!(list["attrs"]["ordered"], true);
        let children = list["children"].as_array().unwrap();
        let ks: Vec<_> = children
            .iter()
            .map(|b| b["kind"].as_str().unwrap())
            .collect();
        assert_eq!(ks, ["paragraph", "paragraph", "list", "paragraph"]);
        assert_eq!(children[2]["attrs"]["ordered"], false);
        assert_eq!(children[2]["children"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_code_block_inside_a_list_item_follows_the_list_with_a_warning() {
        let c = plain("- step\n\n  ```sh\n  ls\n  ```\n- next\n");
        assert_eq!(kinds(&c), ["list", "code"]);
        assert!(
            c.warnings.iter().any(|w| w.contains("list item")),
            "{:?}",
            c.warnings
        );
    }

    #[test]
    fn fenced_code_keeps_language_title_and_exact_text() {
        let c = plain("```ts title=\"a.ts\"\nconst a = 1 < 2;\n```\n");
        assert_eq!(
            c.blocks[0]["attrs"],
            json!({"code": "const a = 1 < 2;", "language": "ts", "title": "a.ts"})
        );
    }

    #[test]
    fn grouped_fences_become_a_code_group() {
        let c = plain("```sh group=\"i\" title=\"npm\"\nnpm i x\n```\n\n```sh group=\"i\" title=\"pnpm\"\npnpm add x\n```\n\n```sh\nlone\n```\n");
        assert_eq!(kinds(&c), ["docs/code-group", "code"]);
        let tabs = c.blocks[0]["attrs"]["tabs"].as_array().unwrap();
        assert_eq!(tabs[0]["label"], "npm");
        assert_eq!(tabs[1]["code"], "pnpm add x");
        assert!(c.blocks[0]["attrs"]["markdown"]
            .as_str()
            .unwrap()
            .contains("**pnpm**"));
    }

    #[test]
    fn a_tree_fence_is_a_file_tree() {
        let c = plain("```tree\napp/\n  page.tsx\n```\n");
        assert_eq!(kinds(&c), ["docs/file-tree"]);
        assert_eq!(c.blocks[0]["attrs"]["tree"], "app/\n  page.tsx");
    }

    #[test]
    fn tables_quotes_rules_and_images() {
        let c = plain("| a | b |\n|---|---|\n| 1 | 2 |\n\n> quoted\n>\n> twice\n\n---\n\n![alt text](pic.png)\n");
        assert_eq!(kinds(&c), ["table", "quote", "separator", "image"]);
        assert_eq!(
            c.blocks[0]["attrs"],
            json!({"header": ["a","b"], "rows": [["1","2"]]})
        );
        assert_eq!(c.blocks[1]["attrs"]["text"], "quoted<br>twice");
        assert_eq!(
            c.blocks[3]["attrs"],
            json!({"url": "pic.png", "alt": "alt text"})
        );
    }

    #[test]
    fn github_alerts_are_callouts() {
        let c = plain("> [!WARNING]\n> Careful here.\n");
        assert_eq!(kinds(&c), ["callout"]);
        assert_eq!(
            c.blocks[0]["attrs"],
            json!({"tone": "warning", "title": "Warning"})
        );
        assert_eq!(c.blocks[0]["children"][0]["attrs"]["text"], "Careful here.");
    }

    #[test]
    fn raw_html_blocks_are_kept_and_comments_dropped() {
        let c =
            plain("<!-- note -->\n\n<details>\n<summary>More</summary>\n\nBody\n\n</details>\n");
        assert!(kinds(&c).contains(&"html"), "{:?}", kinds(&c));
        assert!(c.blocks.iter().all(|b| b["attrs"]["html"]
            .as_str()
            .is_none_or(|h| !h.contains("note"))));
    }

    #[test]
    fn components_map_to_blocks() {
        let src = "<Callout type=\"warning\" title=\"Heads up\">\n\nBe careful.\n\n</Callout>\n\n<Tabs items={[\"npm\",\"pnpm\"]}>\n<Tab>\n\n`npm i`\n\n</Tab>\n<Tab>\n\n`pnpm add`\n\n</Tab>\n</Tabs>\n\n<Steps>\n\n### One\n\nfirst\n\n### Two\n\nsecond\n\n</Steps>\n\n<Params>\n<Param name=\"id\" type=\"string\" required>The *identifier*.</Param>\n<Param name=\"limit\" type=\"number\" default=\"10\">Page size.</Param>\n</Params>\n";
        let c = plain(src);
        assert_eq!(
            kinds(&c),
            ["callout", "docs/tabs", "docs/steps", "docs/params"]
        );
        assert_eq!(
            c.blocks[0]["attrs"],
            json!({"tone": "warning", "title": "Heads up"})
        );
        assert_eq!(c.blocks[1]["attrs"]["labels"], json!(["npm", "pnpm"]));
        assert_eq!(c.blocks[1]["children"].as_array().unwrap().len(), 2);
        assert_eq!(c.blocks[2]["children"].as_array().unwrap().len(), 2);
        let rows = c.blocks[3]["attrs"]["rows"].as_array().unwrap();
        assert_eq!(rows[0]["required"], true);
        assert_eq!(rows[0]["description"], "The <em>identifier</em>.");
        assert_eq!(rows[1]["default"], "10");
        assert!(c.blocks[3]["attrs"]["markdown"]
            .as_str()
            .unwrap()
            .contains("| `id` |"));
    }

    #[test]
    fn links_resolve_against_the_page_and_the_repo() {
        let mut known = BTreeMap::new();
        known.insert(
            "guides/install.md".to_owned(),
            "/docs/guides/install".to_owned(),
        );
        known.insert("plugin-api.md".to_owned(), "/docs/plugin-api".to_owned());
        let links = Links {
            folder: "docs".into(),
            dir: "guides".into(),
            known,
            blob_base: "https://github.com/o/r/blob/abc/".into(),
            raw_base: "https://raw.githubusercontent.com/o/r/abc/".into(),
        };
        assert_eq!(
            resolve(&links, "install.md#flags", false),
            "/docs/guides/install#flags"
        );
        assert_eq!(
            resolve(&links, "../plugin-api.md", false),
            "/docs/plugin-api"
        );
        assert_eq!(
            resolve(&links, "../../crates/x/host.wit", false),
            "https://github.com/o/r/blob/abc/crates/x/host.wit"
        );
        assert_eq!(
            resolve(&links, "img/a.png", true),
            "https://raw.githubusercontent.com/o/r/abc/docs/guides/img/a.png"
        );
        assert_eq!(resolve(&links, "#top", false), "#top");
        assert_eq!(resolve(&links, "https://e.com/a", false), "https://e.com/a");
        assert_eq!(resolve(&links, "javascript:alert(1)", false), "#");
        assert_eq!(resolve(&links, "../../../../etc", false), "../../../../etc");
    }
}
