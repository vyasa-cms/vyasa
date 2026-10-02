//! Per-block-kind HTML rendering.
//!
//! One function per kind, dispatched from [`render_block`]. All text is
//! escaped; inline-HTML-bearing fields go through the ammonia allowlist in
//! [`super`]. Container kinds recurse into their children.

use std::fmt::Write as _;

use vyasa_core::block::{Block, BlockKind};

use super::embed::EmbedOutcome;
use super::{esc, sanitize_inline_html, sanitize_raw_html};

/// Rendering failure (unknown structure rather than unsafe content).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("block render failed: {0}")]
pub struct RenderError(pub String);

fn err(msg: impl Into<String>) -> RenderError {
    RenderError(msg.into())
}

/// Extracts heading entries (level 2..=6) with their generated anchor ids
/// from a rendered document. Call after (or before)
/// [`render_blocks`] — anchors are assigned deterministically in document
/// order, so the TOC matches either way.
#[must_use]
pub fn toc_from_blocks(blocks: &[Block]) -> Vec<super::super::dynblocks::queries::TocEntryData> {
    let mut out = Vec::new();
    walk_headings(blocks, &mut out);
    out
}

/// Depth-first heading collector shared with [`toc_from_blocks`].
fn walk_headings(blocks: &[Block], out: &mut Vec<super::super::dynblocks::queries::TocEntryData>) {
    use vyasa_core::block::BlockKind;
    for b in blocks {
        if b.kind == BlockKind::Heading {
            let level = b
                .attrs
                .get("level")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(2)
                .clamp(2, 6) as u8;
            let text = super::plain_text(b.attrs.get("text"));
            out.push(super::super::dynblocks::queries::TocEntryData {
                level,
                anchor: format!("#vy-h-{}", out.len() + 1),
                text,
            });
        }
        walk_headings(&b.children, out);
    }
}

/// Attribute key carrying a plugin block's already-rendered HTML.
///
/// Set by the application after it calls the owning plugin, and stripped
/// from anything an author can store (see the block validator), so this is
/// never a way to smuggle HTML past the editor's own rules.
pub const RESOLVED_ATTR: &str = "__vyasa_resolved_html";

/// Renders a full block document.
///
/// # Errors
/// Returns [`RenderError`] when a block is structurally invalid for its
/// kind (e.g. a list without children).
pub fn render_blocks(blocks: &[Block]) -> Result<String, RenderError> {
    // What a block may need from the document around it: the headings,
    // for a table of contents, and the time, for a block with a window.
    let ctx = DocContext {
        toc: toc_from_blocks(blocks),
        now: chrono::Utc::now(),
        headings: std::cell::Cell::new(0),
    };
    render_blocks_in(blocks, &ctx)
}

/// Document-level facts a block renders against.
struct DocContext {
    toc: Vec<super::super::dynblocks::queries::TocEntryData>,
    now: chrono::DateTime<chrono::Utc>,
    /// Headings numbered per document, in order, so `#vy-h-N` from
    /// [`toc_from_blocks`] lands on the Nth heading. This used to be a
    /// process-wide counter that never reset, so every table of contents
    /// pointed at ids from some earlier render.
    headings: std::cell::Cell<usize>,
}

impl DocContext {
    fn next_heading_id(&self) -> String {
        let n = self.headings.get() + 1;
        self.headings.set(n);
        format!("vy-h-{n}")
    }
}

fn render_blocks_in(blocks: &[Block], ctx: &DocContext) -> Result<String, RenderError> {
    let mut out = String::new();
    for b in blocks {
        out.push_str(&render_block_in(b, ctx)?);
        out.push('\n');
    }
    Ok(out)
}

/// Whether any timed block in `blocks` opens or closes within `secs`
/// from now, which is when a cached page would go stale.
#[must_use]
pub fn timed_boundary_within(blocks: &[Block], secs: i64) -> bool {
    let now = chrono::Utc::now();
    timed_boundary_between(blocks, now, now + chrono::Duration::seconds(secs))
}

fn timed_boundary_between(
    blocks: &[Block],
    now: chrono::DateTime<chrono::Utc>,
    soon: chrono::DateTime<chrono::Utc>,
) -> bool {
    blocks.iter().any(|b| {
        let near = b.kind == BlockKind::Timed
            && ["from", "until"].iter().any(|key| {
                b.attrs
                    .get(*key)
                    .and_then(serde_json::Value::as_str)
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .is_some_and(|d| d > now && d <= soon)
            });
        near || timed_boundary_between(&b.children, now, soon)
    })
}

/// Whether a timed block is inside its window at `now`. A bound that is
/// missing or unreadable does not bound.
fn timed_is_open(attrs: &serde_json::Value, now: chrono::DateTime<chrono::Utc>) -> bool {
    let bound = |key: &str| {
        attrs
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
    };
    bound("from").is_none_or(|from| now >= from) && bound("until").is_none_or(|until| now < until)
}

fn render_callout(block: &Block, ctx: &DocContext) -> Result<String, RenderError> {
    let attrs = &block.attrs;
    let tone = attrs
        .get("tone")
        .and_then(serde_json::Value::as_str)
        .filter(|t| matches!(*t, "note" | "tip" | "warning" | "danger"))
        .unwrap_or("note");
    let title = inline_text(attrs, "title");
    let heading = if title.trim().is_empty() {
        String::new()
    } else {
        format!("<p class=\"vy-callout__title\">{title}</p>")
    };
    Ok(format!(
        "<aside class=\"vy-callout vy-callout--{tone}\" role=\"note\">{heading}{}</aside>",
        render_blocks_in(&block.children, ctx)?
    ))
}

/// Outside its window a timed block renders nothing at all: not a
/// wrapper, not a comment a reader could find in the source.
fn render_timed(block: &Block, ctx: &DocContext) -> Result<String, RenderError> {
    if !timed_is_open(&block.attrs, ctx.now) {
        return Ok(String::new());
    }
    Ok(format!(
        "<div class=\"vy-timed\">{}</div>",
        render_blocks_in(&block.children, ctx)?
    ))
}

/// A defined form: the API puts the definition on `attrs.definition`
/// (`{slug, name, fields: [{key,label,kind,required,options}]}`); an
/// unresolved block renders nothing. Posts through the same endpoint as
/// the fixed contact form, with the honeypot and the form's slug.
#[allow(
    clippy::format_push_string,
    clippy::format_collect,
    clippy::redundant_closure_for_method_calls
)] // one form, written top to bottom
fn render_form(attrs: &serde_json::Value) -> String {
    let Some(def) = attrs.get("definition") else {
        return String::new();
    };
    let slug = def.get("slug").and_then(|v| v.as_str()).unwrap_or("");
    let mut out = format!(
        "<form class=\"vy-form vy-signup-form\" method=\"post\" action=\"/form\"><input type=\"hidden\" name=\"form\" value=\"{}\"><input type=\"text\" name=\"website\" tabindex=\"-1\" autocomplete=\"off\" style=\"position:absolute;left:-9999px\" aria-hidden=\"true\">",
        super::esc(slug)
    );
    for (i, f) in def
        .get("fields")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let key = f.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let label = f.get("label").and_then(|v| v.as_str()).unwrap_or(key);
        let kind = f.get("kind").and_then(|v| v.as_str()).unwrap_or("text");
        let required = f.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
        let req = if required { " required" } else { "" };
        let id = format!("vy-f-{slug}-{i}");
        let (k, lb) = (super::esc(key), super::esc(label));
        let control = match kind {
            "textarea" => format!("<textarea id=\"{id}\" name=\"{k}\" rows=\"4\"{req}></textarea>"),
            "select" => {
                let opts: String = f
                    .get("options")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|o| o.as_str())
                    .map(|o| format!("<option>{}</option>", super::esc(o)))
                    .collect();
                format!("<select id=\"{id}\" name=\"{k}\"{req}>{opts}</select>")
            }
            "checkbox" => {
                format!("<input id=\"{id}\" type=\"checkbox\" name=\"{k}\" value=\"yes\"{req}>")
            }
            "email" => format!("<input id=\"{id}\" type=\"email\" name=\"{k}\"{req}>"),
            "number" => format!("<input id=\"{id}\" type=\"number\" name=\"{k}\"{req}>"),
            _ => format!("<input id=\"{id}\" type=\"text\" name=\"{k}\"{req}>"),
        };
        if kind == "checkbox" {
            out.push_str(&format!(
                "<label class=\"vy-form-check\">{control} {lb}</label>"
            ));
        } else {
            out.push_str(&format!(
                "<label for=\"{id}\">{lb}{}</label>{control}",
                if required { " *" } else { "" }
            ));
        }
    }
    out.push_str("<button type=\"submit\" class=\"vy-button\">Send</button></form>");
    out
}

fn render_toc(attrs: &serde_json::Value, ctx: &DocContext) -> String {
    let depth = attrs
        .get("depth")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(3)
        .clamp(2, 6) as u8;
    let entries: Vec<_> = ctx.toc.iter().filter(|e| e.level <= depth).collect();
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::from("<nav class=\"vy-toc\" aria-label=\"Table of contents\"><ol>");
    for e in entries {
        let _ = write!(
            out,
            "<li class=\"vy-toc__l{}\"><a href=\"#{}\">{}</a></li>",
            e.level,
            esc(e.anchor.trim_start_matches('#')),
            esc(&e.text)
        );
    }
    out.push_str("</ol></nav>");
    out
}

/// Media URL from attrs: an explicit `url`, or `/api/v1/media/{id}/raw`
/// derived from `mediaId`.
///
/// Accepted URL shapes are exactly those the validator's `check_url` lets
/// through for links: absolute http(s), or root-relative. The admin stores
/// library images as `/api/v1/media/{id}/raw`, so a renderer that only took
/// `https://` turned every uploaded image into a render error — and a render
/// error is a 500 for the whole post.
/// [`media_src`] for callers outside the renderer.
///
/// Unescaped: this feeds a URL into a header or an attribute the caller
/// escapes itself, and double-escaping an ampersand in a query string
/// produces a link that 404s.
#[must_use]
pub fn media_src_of(attrs: &serde_json::Value) -> Option<String> {
    if let Some(u) = attrs.get("url").and_then(serde_json::Value::as_str) {
        let root_relative = u.starts_with('/') && !u.starts_with("//");
        if u.starts_with("https://") || u.starts_with("http://") || root_relative {
            return Some(u.to_owned());
        }
    }
    let id = attrs.get("mediaId").and_then(serde_json::Value::as_u64)?;
    Some(format!("/api/v1/media/{id}/raw"))
}

fn media_src(attrs: &serde_json::Value) -> Option<String> {
    if let Some(u) = attrs.get("url").and_then(serde_json::Value::as_str) {
        let root_relative = u.starts_with('/') && !u.starts_with("//");
        if u.starts_with("https://") || u.starts_with("http://") || root_relative {
            return Some(esc(u));
        }
    }
    let id = attrs.get("mediaId").and_then(serde_json::Value::as_u64)?;
    Some(format!("/api/v1/media/{id}/raw"))
}

/// `srcset`/`sizes` attributes for a media-library URL, so browsers pick
/// a real derivative instead of always downloading the original.
///
/// Only `/api/v1/media/{id}/raw` sources qualify — the widths are the
/// pipeline's fixed sizes, and the endpoint gracefully serves the
/// original when a variant does not exist (small uploads, old rows), so
/// a candidate is never a 404. External URLs get nothing: their sizes
/// are unknowable from here.
pub(crate) fn responsive_attrs(src: &str) -> String {
    let Some(id) = src
        .strip_prefix("/api/v1/media/")
        .and_then(|rest| rest.strip_suffix("/raw"))
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
    else {
        return String::new();
    };
    format!(
        " srcset=\"/api/v1/media/{id}/raw?variant=thumb 320w, \
         /api/v1/media/{id}/raw?variant=medium 768w, \
         /api/v1/media/{id}/raw?variant=large 1280w\" \
         sizes=\"(min-width: 1100px) 780px, 100vw\""
    )
}

fn alt_text(attrs: &serde_json::Value) -> String {
    esc(attrs
        .get("alt")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(""))
}

fn inline_text(attrs: &serde_json::Value, field: &str) -> String {
    match attrs.get(field).and_then(serde_json::Value::as_str) {
        Some(t) => sanitize_inline_html(t),
        None => String::new(),
    }
}

/// Renders one block (including its subtree).
///
/// # Errors
/// See [`render_blocks`].
pub fn render_block(block: &Block) -> Result<String, RenderError> {
    let ctx = DocContext {
        toc: toc_from_blocks(std::slice::from_ref(block)),
        now: chrono::Utc::now(),
        headings: std::cell::Cell::new(0),
    };
    render_block_in(block, &ctx)
}

#[allow(clippy::too_many_lines)]
fn render_block_in(block: &Block, ctx: &DocContext) -> Result<String, RenderError> {
    let attrs = &block.attrs;
    let html = match block.kind {
        BlockKind::Paragraph => {
            format!("<p class=\"vy-p\">{}</p>", inline_text(attrs, "text"))
        }
        BlockKind::Heading => {
            let level = attrs
                .get("level")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(2)
                .clamp(2, 6);
            format!(
                "<h{level} class=\"vy-h\" id=\"{}\">{}</h{level}>",
                ctx.next_heading_id(),
                inline_text(attrs, "text")
            )
        }
        BlockKind::List => render_list(block),
        BlockKind::Quote => render_quote(attrs),
        BlockKind::Code => render_code(attrs)?,
        BlockKind::Table => render_table(attrs)?,
        BlockKind::Gallery => render_gallery(block)?,
        BlockKind::Image | BlockKind::Video | BlockKind::Audio | BlockKind::File => {
            render_media(block)?
        }
        BlockKind::Cover => render_cover(attrs)?,
        BlockKind::MediaText => {
            let src = media_src(attrs).ok_or_else(|| err("media-text block has no source"))?;
            format!(
                "<div class=\"vy-media-text\"><img src=\"{src}\" alt=\"{}\"><div>{}</div></div>",
                alt_text(attrs),
                inline_text(attrs, "text")
            )
        }
        BlockKind::Group => {
            format!(
                "<div class=\"vy-group\">{}</div>",
                render_blocks_in(&block.children, ctx)?
            )
        }
        BlockKind::Row => {
            format!(
                "<div class=\"vy-row\">{}</div>",
                render_blocks_in(&block.children, ctx)?
            )
        }
        BlockKind::Toc => render_toc(attrs, ctx),
        BlockKind::Callout => render_callout(block, ctx)?,
        BlockKind::Timed => render_timed(block, ctx)?,
        // Resolved before rendering (the API fills `children` from the
        // saved pattern); an unresolved one renders nothing rather than a
        // stray marker.
        BlockKind::Form => render_form(attrs),
        BlockKind::Pattern => format!(
            "<div class=\"vy-pattern\">{}</div>",
            render_blocks_in(&block.children, ctx)?
        ),
        BlockKind::Columns => {
            format!(
                "<div class=\"vy-columns\">{}</div>",
                columns_inner(&block.children)?
            )
        }
        BlockKind::Grid => {
            format!(
                "<div class=\"vy-grid\">{}</div>",
                columns_inner(&block.children)?
            )
        }
        BlockKind::Buttons => render_buttons_group(block)?,
        BlockKind::Button => render_button(attrs)?,
        BlockKind::Separator => "<hr class=\"vy-separator\">".to_owned(),
        BlockKind::PageBreak => "<span class=\"vy-page-break\" data-pagebreak></span>".to_owned(),
        BlockKind::Details => {
            let summary = inline_text(attrs, "summary")
                .replace("<p>", "")
                .replace("</p>", "");
            format!(
                "<details class=\"vy-details\"><summary>{summary}</summary>{}</details>",
                render_blocks_in(&block.children, ctx)?
            )
        }
        BlockKind::Footnotes => render_footnotes(block),
        BlockKind::Embed => render_embed(attrs),
        BlockKind::Html => format!(
            "<div class=\"vy-html\">{}</div>",
            sanitize_raw_html(
                attrs
                    .get("html")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| err("html block missing attrs.html"))?
            )
        ),
        // A plugin block arrives here already resolved: the application
        // called the owning plugin and put the result in `RESOLVED_ATTR`.
        // An unresolved one means the plugin is disabled, uninstalled or
        // trapped — which must not take the surrounding post down, so it
        // renders as a comment a reader never sees and an author can find
        // in the page source.
        BlockKind::Plugin => {
            let name = block.plugin_kind.as_deref().unwrap_or("?");
            match attrs.get(RESOLVED_ATTR).and_then(serde_json::Value::as_str) {
                Some(html) => format!(
                    "<div class=\"vy-plugin-block\" data-block=\"{}\">{}</div>",
                    esc(name),
                    crate::renderer::sanitize_plugin_html(html)
                ),
                None => format!("<!-- vyasa: unresolved plugin block {} -->", esc(name)),
            }
        }
    };
    Ok(html)
}

fn render_gallery(block: &Block) -> Result<String, RenderError> {
    let mut figs = Vec::new();
    for item in gallery_items(block) {
        figs.push(format!(
            "<img src=\"{}\" alt=\"{}\" loading=\"lazy\"{}>",
            item.0,
            item.1,
            responsive_attrs(&item.0)
        ));
    }
    if figs.is_empty() {
        return Err(err("gallery block is empty"));
    }
    Ok(format!("<div class=\"vy-gallery\">{}</div>", figs.join("")))
}

fn render_quote(attrs: &serde_json::Value) -> String {
    // `cite` is the source URL (the HTML attribute); `citation` is the
    // visible attribution the editor collects. Both are optional.
    let cite = attrs
        .get("cite")
        .and_then(serde_json::Value::as_str)
        .filter(|c| !c.is_empty())
        .map_or_else(String::new, |c| format!(" cite=\"{}\"", esc(c)));
    let attribution = attrs
        .get("citation")
        .and_then(serde_json::Value::as_str)
        .filter(|c| !c.trim().is_empty())
        .map_or_else(String::new, |c| {
            format!("<footer><cite>{}</cite></footer>", sanitize_inline_html(c))
        });
    format!(
        "<blockquote class=\"vy-quote\"{cite}><p>{}</p>{attribution}</blockquote>",
        inline_text(attrs, "text")
    )
}

fn render_code(attrs: &serde_json::Value) -> Result<String, RenderError> {
    let raw = attrs
        .get("code")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| err("code block missing attrs.code"))?;
    let language = attrs
        .get("language")
        .and_then(serde_json::Value::as_str)
        .filter(|l| !l.is_empty());
    // A recognised language renders highlighted server-side (spans with
    // theme-styled classes, no visitor JavaScript); anything else stays
    // the plain escaped block it always was.
    let (code, lang) = match language.and_then(|l| super::highlight::highlight(raw, l)) {
        Some(html) => (
            html,
            format!(" class=\"language-{}\"", esc(language.unwrap_or(""))),
        ),
        None => (
            esc(raw),
            language.map_or_else(String::new, |l| format!(" class=\"language-{}\"", esc(l))),
        ),
    };
    Ok(format!(
        "<pre class=\"vy-code\"><code{lang}>{code}</code></pre>"
    ))
}

/// A list's children are its items. A `list` child is a nested list and
/// belongs inside the item before it (an empty item is opened if there is
/// none), which is the shape both editors write for indented items.
fn render_list(block: &Block) -> String {
    let tag = if block
        .attrs
        .get("ordered")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        "ol"
    } else {
        "ul"
    };
    let mut items = String::new();
    let mut open = false;
    for child in &block.children {
        if child.kind == BlockKind::List {
            if !open {
                items.push_str("<li>");
                open = true;
            }
            items.push_str(&render_list(child));
            continue;
        }
        if open {
            items.push_str("</li>\n");
        }
        let _ = write!(items, "<li>{}", inline_text(&child.attrs, "text"));
        open = true;
    }
    if open {
        items.push_str("</li>\n");
    }
    format!("<{tag} class=\"vy-list\">{items}</{tag}>")
}

fn render_media(block: &Block) -> Result<String, RenderError> {
    let attrs = &block.attrs;
    let src = media_src(attrs)
        .ok_or_else(|| err(format!("{} block has no source", block.kind.as_str())))?;
    // An empty caption must not leave an empty <figcaption> behind.
    let caption_text = inline_text(attrs, "caption");
    let figcaption = if caption_text.trim().is_empty() {
        String::new()
    } else {
        format!("<figcaption>{caption_text}</figcaption>")
    };
    Ok(match block.kind {
        BlockKind::Image => format!(
            "<figure class=\"vy-image\"><img src=\"{src}\" alt=\"{}\" loading=\"lazy\"\
             {}{}>{figcaption}</figure>",
            alt_text(attrs),
            dimensions(attrs),
            responsive_attrs(&src)
        ),
        BlockKind::Video => format!(
            "<figure class=\"vy-video\"><video controls preload=\"metadata\" src=\"{src}\"></video>{figcaption}</figure>"
        ),
        BlockKind::Audio => format!(
            "<figure class=\"vy-audio\"><audio controls src=\"{src}\"></audio>{figcaption}</figure>"
        ),
        _ => {
            // A download link with no visible text is unusable; fall back to
            // the file's own name from the URL.
            let caption = attrs
                .get("caption")
                .and_then(serde_json::Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .map_or_else(
                    || {
                        src.rsplit('/')
                            .find(|seg| seg.contains('.'))
                            .map_or_else(|| String::from("Download"), esc)
                    },
                    esc,
                );
            format!("<p class=\"vy-file\"><a href=\"{src}\" rel=\"noopener\">{caption}</a></p>")
        }
    })
}

/// `width`/`height` attributes when the block carries them, so the
/// browser can reserve the space and the page stops shifting as images
/// load. Emitted only when both are present and sane.
fn dimensions(attrs: &serde_json::Value) -> String {
    let num = |key: &str| {
        attrs
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .filter(|v| *v > 0 && *v <= 20000)
    };
    match (num("width"), num("height")) {
        (Some(w), Some(h)) => format!(" width=\"{w}\" height=\"{h}\""),
        _ => String::new(),
    }
}

fn render_cover(attrs: &serde_json::Value) -> Result<String, RenderError> {
    // The raw URL, not `media_src`: that one is HTML-escaped, and the
    // browser decodes `&#x27;` back to `'` before the CSS parser sees it,
    // so an HTML escape alone let a URL close `url('…')` and write rules.
    let raw = media_src_of(attrs).ok_or_else(|| err("cover block has no source"))?;
    let src = esc(&css_url_escape(&raw));
    Ok(format!(
        "<section class=\"vy-cover\" style=\"background-image:url('{src}')\"><h2>{}</h2></section>",
        inline_text(attrs, "text")
    ))
}

/// Makes a URL safe inside CSS `url('…')`: percent-encodes every byte
/// that could end the string or the function (`'` `"` `(` `)` `\`),
/// whitespace and control characters, and `<`/`>`. A valid URL keeps its
/// meaning — those characters are percent-encoded in a URL anyway.
fn css_url_escape(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        let unsafe_char = matches!(c, '\'' | '"' | '(' | ')' | '\\' | '<' | '>')
            || c.is_whitespace()
            || c.is_control();
        if unsafe_char {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                let _ = write!(out, "%{b:02X}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn render_buttons_group(block: &Block) -> Result<String, RenderError> {
    let mut buttons = Vec::new();
    for child in &block.children {
        if child.kind == BlockKind::Button {
            buttons.push(render_button(&child.attrs)?);
        }
    }
    if buttons.is_empty() {
        return Err(err("buttons block has no button children"));
    }
    Ok(format!(
        "<div class=\"vy-buttons\">{}</div>",
        buttons.join("")
    ))
}

fn render_footnotes(block: &Block) -> String {
    let mut items = Vec::new();
    for child in &block.children {
        items.push(format!("<li>{}</li>", inline_text(&child.attrs, "text")));
    }
    format!(
        "<section class=\"vy-footnotes\"><ol>{}</ol></section>",
        items.join("")
    )
}

fn render_table(attrs: &serde_json::Value) -> Result<String, RenderError> {
    let rows = attrs
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| err("table block missing attrs.rows"))?;
    let cell = |v: &serde_json::Value| sanitize_inline_html(v.as_str().unwrap_or(""));
    let mut body = String::from("<tbody>");
    for row in rows {
        body.push_str("<tr>");
        for c in row.as_array().unwrap_or(&Vec::new()) {
            let _ = write!(body, "<td>{}</td>", cell(c));
        }
        body.push_str("</tr>");
    }
    body.push_str("</tbody>");
    let header = attrs
        .get("header")
        .and_then(serde_json::Value::as_array)
        .map_or_else(String::new, |cols| {
            let ths: Vec<String> = cols
                .iter()
                .map(|c| format!("<th>{}</th>", cell(c)))
                .collect();
            format!("<thead><tr>{}</tr></thead>", ths.join(""))
        });
    Ok(format!(
        "<figure class=\"vy-table\"><table>{}{body}</table><figcaption>{}</figcaption></figure>",
        header,
        inline_text(attrs, "caption")
    ))
}

fn gallery_items(block: &Block) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = Vec::new();
    for child in &block.children {
        if child.kind == BlockKind::Image {
            if let Some(src) = media_src(&child.attrs) {
                items.push((src, alt_text(&child.attrs)));
            }
        }
    }
    if items.is_empty() {
        if let Some(arr) = block
            .attrs
            .get("items")
            .and_then(serde_json::Value::as_array)
        {
            for it in arr {
                if let Some(u) = it.get("url").and_then(serde_json::Value::as_str) {
                    if u.starts_with("https://") {
                        items.push((
                            esc(u),
                            esc(it
                                .get("alt")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")),
                        ));
                    }
                }
            }
        }
    }
    items
}

fn columns_inner(children: &[Block]) -> Result<String, RenderError> {
    let mut cols = Vec::new();
    for child in children {
        cols.push(format!(
            "<div class=\"vy-col\">{}</div>",
            render_block(child)?
        ));
    }
    Ok(cols.join(""))
}

fn render_button(attrs: &serde_json::Value) -> Result<String, RenderError> {
    let label = inline_text(attrs, "label");
    let href = attrs
        .get("href")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| err("button block missing attrs.href"))?;
    let safe_href = if href.starts_with("https://")
        || href.starts_with("http://")
        || href.starts_with('/')
        || href.starts_with('#')
        || href.starts_with("mailto:")
    {
        esc(href)
    } else {
        return Err(err(format!("button href scheme not allowed: {href}")));
    };
    Ok(format!(
        "<a class=\"vy-button\" href=\"{safe_href}\" rel=\"noopener\">{label}</a>"
    ))
}

fn render_embed(attrs: &serde_json::Value) -> String {
    let Some(url) = attrs.get("url").and_then(serde_json::Value::as_str) else {
        return String::new();
    };
    let caption = inline_text(attrs, "caption");
    match super::embed::resolve_embed(url) {
        EmbedOutcome::Iframe { provider, src } => format!(
            "<figure class=\"vy-embed vy-embed-{provider}\"><iframe src=\"{src}\" title=\"{}\" \
             frameborder=\"0\" allowfullscreen loading=\"lazy\" referrerpolicy=\"strict-origin-when-cross-origin\" \
             sandbox=\"allow-scripts allow-same-origin allow-presentation\"></iframe>\
             <figcaption>{caption}</figcaption></figure>",
            esc(attrs.get("title").and_then(serde_json::Value::as_str).unwrap_or("Embedded content"))
        ),
        EmbedOutcome::Link => format!(
            "<p class=\"vy-embed-link\"><a href=\"{}\" rel=\"noopener noreferrer nofollow\">{}</a>\
             <figcaption>{caption}</figcaption></p>",
            esc(url),
            esc(url)
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::render_cover;

    #[test]
    fn a_cover_url_cannot_break_out_of_css() {
        let payload = "https://x/');position:fixed;inset:0;background:url('https://evil/";
        let html = render_cover(&serde_json::json!({"url": payload, "text": "t"})).unwrap();
        let style = html
            .split("style=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap();
        // One declaration, one url() whose argument never closes early.
        assert!(style.starts_with("background-image:url('"), "{style}");
        assert!(style.ends_with("')"), "{style}");
        let inner = &style["background-image:url('".len()..style.len() - 2];
        for bad in ['\'', '"', '(', ')', '\\', ' ', '&'] {
            assert!(!inner.contains(bad), "{bad:?} in {inner}");
        }
        assert!(!inner.contains("&#x27;"), "{inner}");
        assert!(inner.contains("%27%29;position:fixed"), "{inner}");

        // Newlines and backslashes (CSS escapes) are encoded too.
        let html = render_cover(&serde_json::json!({"url": "https://x/a\\b\nc d"})).unwrap();
        assert!(html.contains("https://x/a%5Cb%0Ac%20d"), "{html}");
    }

    #[test]
    fn only_http_or_root_relative_urls_are_covers() {
        for bad in [
            "javascript:alert(1)",
            "//evil.example/x.png",
            "data:image/png;base64,AA",
        ] {
            assert!(
                render_cover(&serde_json::json!({"url": bad})).is_err(),
                "{bad}"
            );
        }
        let ok = render_cover(&serde_json::json!({"url": "/api/v1/media/3/raw"})).unwrap();
        assert!(ok.contains("url('/api/v1/media/3/raw')"), "{ok}");
    }
}
