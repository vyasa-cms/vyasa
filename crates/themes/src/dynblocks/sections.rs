//! Layout containers and marketing sections.
//!
//! The original vocabulary was fifteen blog and docs primitives — enough to
//! compose an index, a post and an archive, and nothing else. A theme could
//! not express a hero with a call to action, a three-column feature grid or a
//! dark band, so neither could the assistant.
//!
//! These kinds close that gap. They fall in two groups:
//!
//! - **containers** (`band`, `columns`, `grid`, `group`) hold nested
//!   sections. They are the only kinds where `children` is legal.
//! - **content sections** (`hero`, `feature-grid`, `cta-band`, …) render from
//!   their own settings rather than from site queries, so they need no data
//!   access and stay pure.
//!
//! Everything here is still data: settings are validated against a schema and
//! rendered through the same escaping path as any other block.

use std::fmt::Write as _;

use serde_json::json;

use super::{BlockPayload, Builtin};
use crate::layout::MapSettings;

/// Reads a positive integer setting, defaulting when absent.
fn uint(s: &MapSettings, key: &str, lo: u64, hi: u64, default: u64) -> Result<u64, String> {
    match s.get(key) {
        None => Ok(default),
        Some(v) => match v.as_u64() {
            Some(n) if (lo..=hi).contains(&n) => Ok(n),
            _ => Err(format!("{key} must be an integer in {lo}..={hi}")),
        },
    }
}

/// Reads a string setting, defaulting when absent.
fn text(s: &MapSettings, key: &str) -> Option<String> {
    s.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Rejects settings the schema does not name, so a typo surfaces as an error
/// rather than silently doing nothing.
fn only(s: &MapSettings, allowed: &[&str]) -> Vec<String> {
    s.iter()
        .filter(|(k, _)| !allowed.contains(k))
        .map(|(k, _)| {
            format!(
                "unknown setting \"{k}\" (expected one of: {})",
                allowed.join(", ")
            )
        })
        .collect()
}

/* ------------------------------------------------------------ containers -- */

fn check_columns(s: &MapSettings) -> Vec<String> {
    let mut out = only(
        s,
        &[
            "count",
            "count_tablet",
            "count_mobile",
            "gap",
            "align",
            "ratio",
        ],
    );
    if let Err(e) = uint(s, "count", 1, 6, 2) {
        out.push(e);
    }
    for (key, choices) in [
        ("align", &["start", "center", "end", "stretch"][..]),
        ("ratio", &["equal", "wide-start", "wide-end"][..]),
    ] {
        if let Some(value) = s.get(key) {
            if !value.as_str().is_some_and(|v| choices.contains(&v)) {
                out.push(format!("invalid {key}"));
            }
        }
    }
    // Between the two: four columns going straight to one skipped the size
    // most visitors are actually on. Defaults to half the desktop count.
    if let Err(e) = uint(s, "count_tablet", 1, 6, 2) {
        out.push(e);
    }
    // Columns must say what they do on a phone; defaulting to one is the
    // right answer often enough to be the default, not a required setting.
    if let Err(e) = uint(s, "count_mobile", 1, 2, 1) {
        out.push(e);
    }
    if let Err(e) = uint(s, "gap", 0, 12, 3) {
        out.push(e);
    }
    out
}

fn check_band(s: &MapSettings) -> Vec<String> {
    let mut out = only(s, &["contained", "align"]);
    if let Some(v) = s.get("contained") {
        if !v.is_boolean() {
            out.push("contained must be true or false".to_owned());
        }
    }
    out
}

/* ------------------------------------------------------ content sections -- */

fn check_hero(s: &MapSettings) -> Vec<String> {
    let mut out = only(
        s,
        &[
            "eyebrow",
            "headline",
            "subhead",
            "primary_label",
            "primary_url",
            "secondary_label",
            "secondary_url",
            "align",
            "image",
            "image_alt",
            "image_position",
        ],
    );
    if text(s, "headline").is_none_or(|h| h.trim().is_empty()) {
        out.push("headline is required".to_owned());
    }
    for (key, choices) in [
        ("align", ["start", "center"]),
        ("image_position", ["start", "end"]),
    ] {
        if s.get(key)
            .is_some_and(|v| v.as_str().is_none_or(|v| !choices.contains(&v)))
        {
            out.push(format!("{key} must be one of {}", choices.join(", ")));
        }
    }
    // The renderer split the hero around an image for a while before the
    // validator admitted the key, so a theme that shipped one could not
    // install. Same rule as the image section.
    if let Some(v) = text(s, "image") {
        if !v.trim().is_empty() && media_url(&v).is_none() {
            out.push(format!(
                "image {v:?} must be https://, a site path starting with /, or a data:image URL"
            ));
        }
    }
    for key in ["primary_url", "secondary_url"] {
        if let Some(url) = text(s, key) {
            if !safe_url(&url) {
                out.push(format!("{key} must be http(s), relative or an anchor"));
            }
        }
    }
    out
}

fn check_items(s: &MapSettings) -> Vec<String> {
    let mut out = only(s, &["heading", "intro", "items", "columns"]);
    if let Err(e) = uint(s, "columns", 1, 4, 3) {
        out.push(e);
    }
    match s.get("items") {
        None => out.push("items is required".to_owned()),
        Some(v) => match v.as_array() {
            None => out.push("items must be an array".to_owned()),
            Some(a) if a.len() > 12 => out.push("items holds at most 12 entries".to_owned()),
            Some(a) => {
                for (i, item) in a.iter().enumerate() {
                    if let Some(url) = item.get("url") {
                        if url
                            .as_str()
                            .is_none_or(|url| !url.is_empty() && !safe_url(url))
                        {
                            out.push(format!(
                                "items[{i}].url must be http(s), relative or an anchor"
                            ));
                        }
                    }
                    if item
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .is_none()
                    {
                        out.push(format!("items[{i}].title is required"));
                    }
                }
            }
        },
    }
    out
}

fn check_stats(s: &MapSettings) -> Vec<String> {
    let mut out = only(s, &["items"]);
    match s.get("items").and_then(serde_json::Value::as_array) {
        None => out.push("items must be an array".to_owned()),
        Some(a) if a.len() > 6 => out.push("items holds at most 6 entries".to_owned()),
        Some(a) => {
            for (i, item) in a.iter().enumerate() {
                if item
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
                {
                    out.push(format!("items[{i}].value is required"));
                }
            }
        }
    }
    out
}

fn check_signup(s: &MapSettings) -> Vec<String> {
    let mut out = only(s, &["mode", "form", "heading", "subhead", "button_label"]);
    if let Some(mode) = text(s, "mode") {
        if mode != "newsletter" && mode != "contact" {
            out.push("mode must be \"newsletter\" or \"contact\"".to_owned());
        }
    }
    out
}

fn check_cta(s: &MapSettings) -> Vec<String> {
    let mut out = only(s, &["headline", "subhead", "label", "url"]);
    if text(s, "headline").is_none_or(|h| h.trim().is_empty()) {
        out.push("headline is required".to_owned());
    }
    if let Some(url) = text(s, "url") {
        if !safe_url(&url) {
            out.push("url must be http(s), relative or an anchor".to_owned());
        }
    }
    out
}

fn safe_url(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with('/')
        || url.starts_with('#')
        || url.starts_with("mailto:")
}

/* ------------------------------------------------------------- rendering -- */

/// Escapes text for HTML. Sections render their own markup, so every author
/// string goes through here before reaching the page.
fn esc(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn button(label: Option<String>, url: Option<String>, class: &str) -> String {
    match (label, url) {
        (Some(l), Some(u)) if !l.trim().is_empty() && safe_url(&u) => {
            format!(
                "<a class=\"vy-btn {class}\" href=\"{}\">{}</a>",
                esc(&u),
                esc(&l)
            )
        }
        _ => String::new(),
    }
}

fn render_hero(s: &MapSettings, editor: bool) -> String {
    let eyebrow = text(s, "eyebrow")
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            format!(
                "<p class=\"vy-eyebrow\"{}>{}</p>",
                edit_marker(editor, "eyebrow"),
                esc(&v)
            )
        })
        .unwrap_or_default();
    let headline = text(s, "headline").unwrap_or_default();
    let subhead = text(s, "subhead")
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            format!(
                "<p class=\"vy-hero-sub\"{}>{}</p>",
                edit_marker(editor, "subhead"),
                esc(&v)
            )
        })
        .unwrap_or_default();
    let actions = format!(
        "{}{}",
        button(
            text(s, "primary_label"),
            text(s, "primary_url"),
            "vy-btn-primary"
        ),
        button(
            text(s, "secondary_label"),
            text(s, "secondary_url"),
            "vy-btn-ghost"
        ),
    );
    let actions = if actions.is_empty() {
        String::new()
    } else {
        format!("<p class=\"vy-actions\">{actions}</p>")
    };
    let media = text(s, "image")
        .and_then(|v| media_url(v.as_str()).map(str::to_owned))
        .map(|src| {
            let alt = text(s, "image_alt").unwrap_or_default();
            format!(
                "<img class=\"vy-hero-media\" src=\"{}\" alt=\"{}\" fetchpriority=\"high\"{}>",
                esc(&src),
                esc(&alt),
                crate::renderer::blocks::responsive_attrs(&src)
            )
        });
    let copy = format!(
        "{eyebrow}<h1 class=\"vy-hero-title\"{}>{}</h1>{subhead}{actions}",
        edit_marker(editor, "headline"),
        esc(&headline)
    );
    let align = if text(s, "align").as_deref() == Some("center") {
        " vy-hero--center"
    } else {
        ""
    };
    let position = if text(s, "image_position").as_deref() == Some("start") {
        " vy-hero--media-start"
    } else {
        ""
    };
    match media {
        // With an image the hero splits: words one side, picture the
        // other, stacking on phones. Without one, exactly what it was.
        Some(img) => format!(
            "<div class=\"vy-hero vy-hero--split{align}{position}\"><div class=\"vy-hero-copy\">{copy}</div>{img}</div>"
        ),
        None => format!("<div class=\"vy-hero{align}\">{copy}</div>"),
    }
}

fn render_items(s: &MapSettings, kind: &str) -> String {
    let columns = uint(s, "columns", 1, 4, 3).unwrap_or(3);
    let heading = text(s, "heading")
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("<h2 class=\"vy-section-title\">{}</h2>", esc(&v)))
        .unwrap_or_default();
    let intro = text(s, "intro")
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("<p class=\"vy-section-intro\">{}</p>", esc(&v)))
        .unwrap_or_default();

    let empty = Vec::new();
    let items = s
        .get("items")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);

    let cards = items.iter().fold(String::new(), |mut acc, item| {
        let title = item
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let body = item
            .get("body")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let title_html = item
            .get("url")
            .and_then(serde_json::Value::as_str)
            .filter(|url| safe_url(url) && !url.is_empty())
            .map_or_else(
                || esc(title),
                |url| format!("<a href=\"{}\">{}</a>", esc(url), esc(title)),
            );
        acc.push_str("<li class=\"vy-card\">");
        if let Some(label) = item.get("eyebrow").and_then(serde_json::Value::as_str) {
            let _ = write!(acc, "<p class=\"vy-eyebrow\">{}</p>", esc(label));
        }
        let _ = write!(acc, "<h3 class=\"vy-card-title\">{title_html}</h3>");
        if !body.is_empty() {
            let _ = write!(acc, "<p class=\"vy-card-body\">{}</p>", esc(body));
        }
        acc.push_str("</li>");
        acc
    });

    format!(
        "<div class=\"vy-{kind}\">{heading}{intro}<ul class=\"vy-cards\" data-columns=\"{columns}\">{cards}</ul></div>"
    )
}

fn render_stats(s: &MapSettings) -> String {
    let empty = Vec::new();
    let items = s
        .get("items")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);
    let cells = items.iter().fold(String::new(), |mut acc, item| {
        let value = item.get("value").and_then(serde_json::Value::as_str).unwrap_or("");
        let label = item.get("label").and_then(serde_json::Value::as_str).unwrap_or("");
        let _ = write!(
            acc,
            "<li class=\"vy-stat\"><span class=\"vy-stat-value\">{}</span><span class=\"vy-stat-label\">{}</span></li>",
            esc(value),
            esc(label)
        );
        acc
    });
    format!("<ul class=\"vy-stats\">{cells}</ul>")
}

fn render_cta(s: &MapSettings, editor: bool) -> String {
    let headline = text(s, "headline").unwrap_or_default();
    let subhead = text(s, "subhead")
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            format!(
                "<p class=\"vy-cta-sub\"{}>{}</p>",
                edit_marker(editor, "subhead"),
                esc(&v)
            )
        })
        .unwrap_or_default();
    format!(
        "<div class=\"vy-cta\"><h2 class=\"vy-cta-title\"{}>{}</h2>{subhead}<p class=\"vy-actions\">{}</p></div>",
        edit_marker(editor, "headline"),
        esc(&headline),
        button(text(s, "label"), text(s, "url"), "vy-btn-primary")
    )
}

fn render_faq(s: &MapSettings) -> String {
    let heading = text(s, "heading")
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("<h2 class=\"vy-section-title\">{}</h2>", esc(&v)))
        .unwrap_or_default();
    let empty = Vec::new();
    let items = s
        .get("items")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);
    let rows = items.iter().fold(String::new(), |mut acc, item| {
        let q = item
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let a = item
            .get("body")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let _ = write!(
            acc,
            "<details class=\"vy-faq-item\"><summary>{}</summary><p>{}</p></details>",
            esc(q),
            esc(a)
        );
        acc
    });
    // Rich-result food: the same questions as FAQPage JSON-LD, inline —
    // schema.org blocks are valid anywhere in the body, which saves the
    // head builder from ever needing to know what sections rendered.
    let schema = faq_schema(items);
    format!("<div class=\"vy-faq\">{heading}{rows}</div>{schema}")
}

/// The `FAQPage` structured-data block for a faq section's items; empty
/// when no item has both a question and an answer.
fn faq_schema(items: &[serde_json::Value]) -> String {
    let entities: Vec<serde_json::Value> = items
        .iter()
        .filter_map(|item| {
            let q = item.get("title").and_then(serde_json::Value::as_str)?;
            let a = item.get("body").and_then(serde_json::Value::as_str)?;
            (!q.trim().is_empty() && !a.trim().is_empty()).then(|| {
                serde_json::json!({
                    "@type": "Question",
                    "name": q,
                    "acceptedAnswer": {"@type": "Answer", "text": a}
                })
            })
        })
        .collect();
    if entities.is_empty() {
        return String::new();
    }
    let doc = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "FAQPage",
        "mainEntity": entities,
    });
    // `</script>` inside a JSON string would close the element early.
    let json = doc.to_string().replace("</", "<\\/");
    format!("<script type=\"application/ld+json\">{json}</script>")
}

/// A no-JavaScript lead/newsletter form posting to `/form`.
///
/// The `website` field is a honeypot: visually parked off-screen, so a
/// person never fills it and a form-stuffing bot usually does. The
/// handler answers a filled honeypot with the same thanks page, so the
/// bot learns nothing.
fn render_signup(s: &MapSettings) -> String {
    let mode = text(s, "mode").unwrap_or_else(|| "newsletter".to_owned());
    let mode = if mode == "contact" {
        "contact"
    } else {
        "newsletter"
    };
    let form = text(s, "form").unwrap_or_else(|| mode.to_owned());
    let heading = text(s, "heading")
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("<h2 class=\"vy-section-title\">{}</h2>", esc(&v)))
        .unwrap_or_default();
    let subhead = text(s, "subhead")
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("<p class=\"vy-signup-subhead\">{}</p>", esc(&v)))
        .unwrap_or_default();
    let button = text(s, "button_label").filter(|v| !v.trim().is_empty());
    let button = button.as_deref().unwrap_or(if mode == "contact" {
        "Send"
    } else {
        "Subscribe"
    });
    let message = if mode == "contact" {
        "<label class=\"vy-signup-field\"><span>Message</span>\
         <textarea name=\"message\" rows=\"4\"></textarea></label>"
    } else {
        ""
    };
    format!(
        "<div class=\"vy-signup\">{heading}{subhead}\
         <form class=\"vy-signup-form\" method=\"post\" action=\"/form\">\
         <input type=\"hidden\" name=\"form\" value=\"{}\">\
         <input type=\"hidden\" name=\"mode\" value=\"{mode}\">\
         <input class=\"vy-hp\" type=\"text\" name=\"website\" tabindex=\"-1\" autocomplete=\"off\" aria-hidden=\"true\">\
         <label class=\"vy-signup-field\"><span>Name</span>\
         <input type=\"text\" name=\"name\" autocomplete=\"name\"></label>\
         <label class=\"vy-signup-field\"><span>Email</span>\
         <input type=\"email\" name=\"email\" required autocomplete=\"email\"></label>\
         {message}\
         <button type=\"submit\" class=\"vy-button\">{}</button>\
         </form></div>",
        esc(&form),
        esc(button)
    )
}

/// Renders a content section to HTML, or `None` when the kind is a container
/// or not one of ours.
///
/// `editor` is the studio-preview flag: it adds `data-vy-edit` markers to
/// the text the canvas may edit in place, and nothing else. Public renders
/// pass `false` and their output is identical to what it always was.
#[must_use]
pub fn render(kind: &str, settings: &MapSettings, editor: bool) -> Option<String> {
    Some(match kind {
        "hero" => render_hero(settings, editor),
        "feature-grid" => render_items(settings, "features"),
        "stats-band" => render_stats(settings),
        "cta-band" => render_cta(settings, editor),
        "faq" => render_faq(settings),
        "logo-wall" => render_items(settings, "logos"),
        "image" => render_image(settings, editor),
        "video" => render_video(settings, editor),
        "signup-form" => render_signup(settings),
        _ => return None,
    })
}

/// A safe URL for media `src` position: http(s), a site path, or an
/// inline data image. Narrower than link `safe_url` — `mailto:` and `#`
/// have no business in an <img>.
fn media_url(raw: &str) -> Option<&str> {
    let v = raw.trim();
    let ok = v.starts_with("https://")
        || v.starts_with("http://")
        || v.starts_with('/') && !v.starts_with("//")
        || v.starts_with("data:image/");
    (ok && !v.is_empty()).then_some(v)
}

fn render_image(s: &MapSettings, editor: bool) -> String {
    let Some(src) = text(s, "src").and_then(|v| media_url(v.as_str()).map(str::to_owned)) else {
        // No image yet: nothing for a visitor, a placeholder box for the
        // author (the empty-leaf path takes over in the editor).
        return String::new();
    };
    let alt = text(s, "alt").unwrap_or_default();
    let caption = text(s, "caption")
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            format!(
                "<figcaption{}>{}</figcaption>",
                edit_marker(editor, "caption"),
                esc(&v)
            )
        })
        .unwrap_or_default();
    format!(
        "<figure class=\"vy-image\"><img src=\"{}\" alt=\"{}\" loading=\"lazy\"{}>{caption}</figure>",
        esc(&src),
        esc(&alt),
        crate::renderer::blocks::responsive_attrs(&src)
    )
}

fn render_video(s: &MapSettings, editor: bool) -> String {
    let Some(src) = text(s, "src").and_then(|v| media_url(v.as_str()).map(str::to_owned)) else {
        return String::new();
    };
    let poster = text(s, "poster")
        .and_then(|v| media_url(v.as_str()).map(str::to_owned))
        .map(|p| format!(" poster=\"{}\"", esc(&p)))
        .unwrap_or_default();
    let caption = text(s, "caption")
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            format!(
                "<figcaption{}>{}</figcaption>",
                edit_marker(editor, "caption"),
                esc(&v)
            )
        })
        .unwrap_or_default();
    format!(
        "<figure class=\"vy-video\"><video controls preload=\"metadata\" src=\"{}\"{poster}></video>{caption}</figure>",
        esc(&src)
    )
}

/// `data-vy-edit` attribute for `key`, or nothing outside the studio.
fn edit_marker(editor: bool, key: &str) -> String {
    if editor {
        format!(" data-vy-edit=\"{key}\"")
    } else {
        String::new()
    }
}

/// Insert-library grouping for a kind. Every kind the registry holds has
/// one; the catch-all keeps an unlisted kind usable rather than invisible.
pub(crate) fn kind_category(kind: &str) -> &'static str {
    match kind {
        "band" | "columns" | "grid" | "group" => "structure",
        "code-tabs" | "execution-flow" | "status-cards" | "hero" | "feature-grid"
        | "stats-band" | "cta-band" | "faq" | "logo-wall" | "signup-form" => "marketing",
        "menu" | "breadcrumbs" | "docs-nav" | "pagination" | "search-box" => "navigation",
        "image" | "video" => "media",
        _ => "content",
    }
}

/// Starter settings for a freshly inserted section: something real on the
/// canvas instead of an empty rectangle, and a working example for the
/// assistants.
pub(crate) fn kind_sample(kind: &str) -> serde_json::Value {
    match kind {
        "code-tabs" => {
            json!({"heading":"Try it yourself","items":[{"title":"CLI","language":"sh","code":"echo hello"},{"title":"Python","language":"python","code":"print(\"hello\")"}]})
        }
        "execution-flow" => {
            json!({"heading":"How it works","items":[{"title":"Receive","label":"Input","body":"Accept a request."},{"title":"Process","label":"Runtime","body":"Run the workload."},{"title":"Return","label":"Output","body":"Deliver the result."}]})
        }
        "status-cards" => {
            json!({"heading":"Feature maturity","items":[{"title":"Core runtime","status":"available","body":"Ready to use."},{"title":"Distributed execution","status":"preview","body":"Evaluate with your workload."}]})
        }
        "hero" => serde_json::json!({
            "eyebrow": "New",
            "headline": "A headline that says the true thing",
            "subhead": "One supporting line, written plainly.",
            "primary_label": "Get started", "primary_url": "/"
        }),
        "cta-band" => serde_json::json!({
            "headline": "Ready when you are",
            "subhead": "One line on why to act now.",
            "label": "Start", "url": "/"
        }),
        "signup-form" => serde_json::json!({
            "mode": "newsletter",
            "heading": "Get new posts by email",
            "subhead": "No spam, one email per post, unsubscribe any time."
        }),
        "feature-grid" => serde_json::json!({
            "columns": 3,
            "items": [
                {"title": "First", "body": "What it does for the reader."},
                {"title": "Second", "body": "What it does for the reader."},
                {"title": "Third", "body": "What it does for the reader."}
            ]
        }),
        "stats-band" => serde_json::json!({
            "items": [
                {"value": "1,200+", "label": "of something"},
                {"value": "98%", "label": "of something else"},
                {"value": "14", "label": "of a third thing"}
            ]
        }),
        "faq" => serde_json::json!({
            "heading": "Questions",
            "items": [
                {"title": "A common question?", "body": "A plain answer."},
                {"title": "Another one?", "body": "Another answer."}
            ]
        }),
        "logo-wall" => serde_json::json!({
            "items": [{"title": "One"}, {"title": "Two"}, {"title": "Three"}]
        }),
        "columns" => serde_json::json!({"count": 2}),
        "latest-posts" => serde_json::json!({"count": 3}),
        "collection" => serde_json::json!({
            "bind": {"source": "post", "sort": "newest", "limit": 3},
            "columns": 3
        }),
        "image" => serde_json::json!({"alt": ""}),
        _ => serde_json::json!({}),
    }
}

/// String settings the canvas edits in place. Only flat text — items and
/// URLs stay in the inspector.
pub(crate) fn kind_inline(kind: &str) -> &'static [&'static str] {
    match kind {
        "hero" => &["eyebrow", "headline", "subhead"],
        "cta-band" => &["headline", "subhead"],
        "image" | "video" => &["caption"],
        _ => &[],
    }
}

/// One resolver per settings-only kind.
///
/// `ResolveFn` is a bare function pointer, so a resolver cannot capture its
/// own kind — each section gets a one-line function instead of a shared
/// closure. Explicit, and it keeps the registry table readable.
macro_rules! section_resolver {
    ($name:ident, $kind:literal) => {
        fn $name<'a>(
            ctx: &'a super::ResolveContext<'a>,
            settings: &'a MapSettings,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<BlockPayload, String>> + Send + 'a>,
        > {
            let html = render($kind, settings, ctx.editor).unwrap_or_default();
            Box::pin(std::future::ready(Ok(BlockPayload::Html(html))))
        }
    };
}

section_resolver!(resolve_hero, "hero");
section_resolver!(resolve_feature_grid, "feature-grid");
section_resolver!(resolve_stats_band, "stats-band");
section_resolver!(resolve_cta_band, "cta-band");
section_resolver!(resolve_faq, "faq");
section_resolver!(resolve_logo_wall, "logo-wall");
section_resolver!(resolve_image, "image");
section_resolver!(resolve_video, "video");
section_resolver!(resolve_signup_form, "signup-form");

/// Containers plus marketing sections, registered alongside the blog kinds.
pub(crate) fn section_kinds() -> Vec<Builtin> {
    let mut all = containers();
    all.extend(marketing());
    all.extend(super::product::kinds());
    all.extend(marketing_more());
    all.extend(media_kinds());
    all.push(Builtin {
        kind: "signup-form",
        description: "Newsletter signup or contact form; submissions land in the admin",
        schema: json!({
            "type": "object",
            "properties": {
                "mode": {"type": "string", "enum": ["newsletter", "contact"]},
                "form": {"type": "string", "maxLength": 60},
                "heading": {"type": "string"},
                "subhead": {"type": "string"},
                "button_label": {"type": "string"}
            },
            "additionalProperties": false
        }),
        check: check_signup,
        resolve_fn: resolve_signup_form,
        container: false,
    });
    all
}

/// Kinds that hold nested sections.
fn containers() -> Vec<Builtin> {
    vec![
        Builtin {
            kind: "band",
            description: "Full-width band; give it a style scope for a dark or tinted stripe",
            schema: json!({
                "type": "object",
                "properties": {
                    "contained": {"type": "boolean"},
                    "align": {"type": "string", "enum": ["start", "center"]}
                },
                "additionalProperties": false
            }),
            check: check_band,
            resolve_fn: super::resolvers::empty,
            container: true,
        },
        Builtin {
            kind: "columns",
            description: "Side-by-side columns that stack on small screens",
            schema: json!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer", "minimum": 1, "maximum": 6},
                    "count_tablet": {"type": "integer", "minimum": 1, "maximum": 6},
                    "count_mobile": {"type": "integer", "minimum": 1, "maximum": 2},
                    "gap": {"type": "integer", "minimum": 0, "maximum": 12},
                    "align": {"type": "string", "enum": ["start", "center", "end", "stretch"]},
                    "ratio": {"type":"string", "enum":["equal", "wide-start", "wide-end"], "description":"Two-column desktop proportions; smaller screens use their column counts"}
                },
                "additionalProperties": false
            }),
            check: check_columns,
            resolve_fn: super::resolvers::empty,
            container: true,
        },
        Builtin {
            kind: "grid",
            description: "Auto-fitting grid of equal cells",
            schema: json!({
                "type": "object",
                "properties": {
                    "min_width": {"type": "integer", "minimum": 80, "maximum": 600},
                    "gap": {"type": "integer", "minimum": 0, "maximum": 12}
                },
                "additionalProperties": false
            }),
            check: |s| only(s, &["min_width", "gap"]),
            resolve_fn: super::resolvers::empty,
            container: true,
        },
        Builtin {
            kind: "group",
            description: "Plain wrapper for grouping sections",
            schema: json!({"type": "object", "additionalProperties": false}),
            check: |s| only(s, &[]),
            resolve_fn: super::resolvers::empty,
            container: true,
        },
    ]
}

/// Settings-only sections that render their own markup.
fn marketing() -> Vec<Builtin> {
    vec![
        Builtin {
            kind: "hero",
            description: "Headline, supporting line and up to two calls to action",
            schema: json!({
                "type": "object",
                "properties": {
                    "eyebrow": {"type": "string"},
                    "headline": {"type": "string"},
                    "subhead": {"type": "string"},
                    "primary_label": {"type": "string"},
                    "primary_url": {"type": "string"},
                    "secondary_label": {"type": "string"},
                    "secondary_url": {"type": "string"},
                    "align": {"type": "string", "enum": ["start", "center"]},
                    "image": {"type": "string", "description": "Picture beside the copy; the hero splits when set"},
                    "image_alt": {"type": "string"},
                    "image_position": {"type": "string", "enum": ["start", "end"], "description": "Side of the hero artwork on desktop; stacks on mobile"}
                },
                "required": ["headline"],
                "additionalProperties": false
            }),
            check: check_hero,
            resolve_fn: resolve_hero,
            container: false,
        },
        Builtin {
            kind: "feature-grid",
            description: "Titled cards describing features or services",
            schema: json!({
                "type": "object",
                "properties": {
                    "heading": {"type": "string"},
                    "intro": {"type": "string"},
                    "columns": {"type": "integer", "minimum": 1, "maximum": 4},
                    "items": {
                        "type": "array", "maxItems": 12,
                        "items": {
                            "type": "object",
                            "properties": {"title": {"type": "string"}, "body": {"type": "string"}, "eyebrow": {"type": "string"}, "url": {"type": "string", "description": "Optional destination for the card title"}},
                            "required": ["title"]
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            }),
            check: check_items,
            resolve_fn: resolve_feature_grid,
            container: false,
        },
        Builtin {
            kind: "stats-band",
            description: "A row of headline numbers",
            schema: json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array", "maxItems": 6,
                        "items": {
                            "type": "object",
                            "properties": {"value": {"type": "string"}, "label": {"type": "string"}},
                            "required": ["value"]
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            }),
            check: check_stats,
            resolve_fn: resolve_stats_band,
            container: false,
        },
    ]
}

/// The rest of the marketing set.
/// The media kinds: an image or a video as a section of its own.
pub(crate) fn media_kinds() -> Vec<Builtin> {
    vec![
        Builtin {
            kind: "image",
            description: "One image, full width of its section, with an optional caption",
            schema: json!({
                "type": "object",
                "properties": {
                    "src": {"type": "string", "maxLength": 2000,
                            "description": "image URL — https, or a site path like /media/…"},
                    "alt": {"type": "string", "maxLength": 300,
                            "description": "what the image shows, for readers who cannot see it"},
                    "caption": {"type": "string", "maxLength": 300}
                },
                "required": ["src"],
                "additionalProperties": false
            }),
            check: check_media_src,
            resolve_fn: resolve_image,
            container: false,
        },
        Builtin {
            kind: "video",
            description: "One video with browser controls, optional poster and caption",
            schema: json!({
                "type": "object",
                "properties": {
                    "src": {"type": "string", "maxLength": 2000,
                            "description": "video URL — https, or a site path"},
                    "poster": {"type": "string", "maxLength": 2000},
                    "caption": {"type": "string", "maxLength": 300}
                },
                "required": ["src"],
                "additionalProperties": false
            }),
            check: check_media_src,
            resolve_fn: resolve_video,
            container: false,
        },
    ]
}

fn check_media_src(s: &MapSettings) -> Vec<String> {
    match s.get("src").and_then(serde_json::Value::as_str) {
        None | Some("") => Vec::new(), // empty = placeholder until chosen
        Some(v) if media_url(v).is_some() => Vec::new(),
        Some(v) => vec![format!(
            "src {v:?} must be https://, a site path starting with /, or a data:image URL"
        )],
    }
}

fn marketing_more() -> Vec<Builtin> {
    vec![
        Builtin {
            kind: "cta-band",
            description: "Closing call to action; pair it with a dark style scope",
            schema: json!({
                "type": "object",
                "properties": {
                    "headline": {"type": "string"},
                    "subhead": {"type": "string"},
                    "label": {"type": "string"},
                    "url": {"type": "string"}
                },
                "required": ["headline"],
                "additionalProperties": false
            }),
            check: check_cta,
            resolve_fn: resolve_cta_band,
            container: false,
        },
        Builtin {
            kind: "faq",
            description: "Expandable question and answer list",
            schema: json!({
                "type": "object",
                "properties": {
                    "heading": {"type": "string"},
                    "items": {
                        "type": "array", "maxItems": 20,
                        "items": {
                            "type": "object",
                            "properties": {"title": {"type": "string"}, "body": {"type": "string"}},
                            "required": ["title"]
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            }),
            check: check_items,
            resolve_fn: resolve_faq,
            container: false,
        },
        Builtin {
            kind: "logo-wall",
            description: "Row of customer or partner names",
            schema: json!({
                "type": "object",
                "properties": {
                    "heading": {"type": "string"},
                    "columns": {"type": "integer", "minimum": 1, "maximum": 4},
                    "items": {
                        "type": "array", "maxItems": 12,
                        "items": {
                            "type": "object",
                            "properties": {"title": {"type": "string"}},
                            "required": ["title"]
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            }),
            check: check_items,
            resolve_fn: resolve_logo_wall,
            container: false,
        },
    ]
}

#[cfg(test)]
mod modern_tests {
    use super::*;

    #[test]
    fn hero_alignment_and_media_side_render_and_validate() {
        let mut settings: MapSettings = serde_json::from_value(json!({
            "headline": "Made with care", "align": "center", "image_position": "start",
            "image": "/theme-assets/images/hero.svg", "image_alt": "Artwork"
        }))
        .expect("settings");
        assert!(check_hero(&settings).is_empty());
        let html = render_hero(&settings, true);
        assert!(html.contains("vy-hero--center"));
        assert!(html.contains("vy-hero--media-start"));
        assert!(html.contains("data-vy-edit"));
        settings.set("align", json!("sideways"));
        assert!(!check_hero(&settings).is_empty());
    }

    #[test]
    fn feature_destinations_are_real_links_and_unsafe_urls_are_rejected() {
        let mut settings: MapSettings = serde_json::from_value(json!({
            "items": [{"title": "Guides & recipes", "eyebrow": "01", "url": "/search?q=guides"}]
        }))
        .expect("settings");
        assert!(check_items(&settings).is_empty());
        let html = render_items(&settings, "features");
        assert!(html.contains("href=\"/search?q=guides\""));
        assert!(html.contains("Guides &amp; recipes"));
        settings.set(
            "items",
            json!([{"title": "Bad", "url": "javascript:alert(1)"}]),
        );
        assert!(!check_items(&settings).is_empty());
        assert!(!render_items(&settings, "features").contains("href="));
    }
}
