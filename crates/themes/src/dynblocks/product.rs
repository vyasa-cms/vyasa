//! Editable product-site components with progressively enhanced code examples.
use std::fmt::Write as _;

use serde_json::{json, Value};

use super::{BlockPayload, Builtin, ResolveContext};
use crate::layout::MapSettings;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn validate(s: &MapSettings, kind: &str) -> Vec<String> {
    let mut errors = Vec::new();
    for (key, value) in s.iter() {
        match key {
            "heading" | "intro" if value.as_str().is_some_and(|v| v.len() <= 4000) => {}
            "items" => {}
            _ => errors.push(format!("unknown or invalid setting: {key}")),
        }
    }
    let Some(items) = s.get("items").and_then(Value::as_array) else {
        errors.push("items must be an array".into());
        return errors;
    };
    if items.is_empty() || items.len() > 12 {
        errors.push("provide between 1 and 12 items".into());
    }
    let fields: &[&str] = if kind == "code-tabs" {
        &["title", "language", "code"]
    } else if kind == "status-cards" {
        &["title", "body", "status"]
    } else {
        &["title", "body", "label"]
    };
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            errors.push(format!("items[{index}] must be an object"));
            continue;
        };
        for (key, value) in object {
            let max = if key == "code" { 16000 } else { 4000 };
            if !fields.contains(&key.as_str()) || value.as_str().is_none_or(|v| v.len() > max) {
                errors.push(format!(
                    "items[{index}].{key} is unknown, not text, or too long"
                ));
            }
        }
        if field(item, "title").trim().is_empty() {
            errors.push(format!("items[{index}].title is required"));
        }
        if kind == "code-tabs" && field(item, "code").trim().is_empty() {
            errors.push(format!("items[{index}].code is required"));
        }
        if kind == "status-cards"
            && !["available", "preview", "experimental", "coming-soon"]
                .contains(&field(item, "status"))
        {
            errors.push(format!(
                "items[{index}].status must be available, preview, experimental, or coming-soon"
            ));
        }
    }
    errors
}
fn check_code(s: &MapSettings) -> Vec<String> {
    validate(s, "code-tabs")
}
fn check_flow(s: &MapSettings) -> Vec<String> {
    validate(s, "execution-flow")
}
fn check_status(s: &MapSettings) -> Vec<String> {
    validate(s, "status-cards")
}

/// Render static content. All examples remain readable when JavaScript is disabled.
#[must_use]
pub fn render(kind: &str, s: &MapSettings) -> String {
    let heading = s.get("heading").and_then(Value::as_str).unwrap_or("");
    let intro = s.get("intro").and_then(Value::as_str).unwrap_or("");
    let mut out = format!("<div class=\"vy-product vy-{kind}\">");
    if !heading.is_empty() {
        let _ = write!(out, "<h2 class=\"vy-section-title\">{}</h2>", esc(heading));
    }
    if !intro.is_empty() {
        let _ = write!(out, "<p class=\"vy-section-intro\">{}</p>", esc(intro));
    }
    if kind == "code-tabs" {
        out.push_str("<div class=\"vy-code-examples\">");
    } else {
        out.push_str("<ol class=\"vy-product-items\">");
    }
    for (i, item) in s
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let title = esc(field(item, "title"));
        if kind == "code-tabs" {
            let _ = write!(out, "<section class=\"vy-code-example\"><h3>{title}</h3><span class=\"vy-code-language\">{}</span><pre tabindex=\"0\"><code>{}</code></pre></section>", esc(field(item, "language")), esc(field(item, "code")));
        } else {
            let _ = write!(
                out,
                "<li><span class=\"vy-step-number\" aria-hidden=\"true\">{:02}</span>",
                i + 1
            );
            if kind == "status-cards" {
                let status = field(item, "status");
                let label = match status {
                    "available" => "Available",
                    "preview" => "Preview",
                    "experimental" => "Experimental",
                    _ => "Coming soon",
                };
                let _ = write!(out, "<span class=\"vy-status\">{label}</span>");
            } else {
                let _ = write!(
                    out,
                    "<span class=\"vy-flow-label\">{}</span>",
                    esc(field(item, "label"))
                );
            }
            let _ = write!(
                out,
                "<h3>{title}</h3><p>{}</p></li>",
                esc(field(item, "body"))
            );
        }
    }
    out.push_str(if kind == "code-tabs" {
        "</div></div>"
    } else {
        "</ol></div>"
    });
    out
}
macro_rules! resolver {
    ($name:ident, $kind:literal) => {
        fn $name<'a>(
            _: &'a ResolveContext<'a>,
            s: &'a MapSettings,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<BlockPayload, String>> + Send + 'a>,
        > {
            Box::pin(std::future::ready(Ok(BlockPayload::Html(render($kind, s)))))
        }
    };
}
resolver!(resolve_code, "code-tabs");
resolver!(resolve_flow, "execution-flow");
resolver!(resolve_status, "status-cards");

fn schema(fields: &Value, required: &[&str]) -> Value {
    json!({"type":"object", "additionalProperties":false,"required":["items"],"properties":{
        "heading":{"type":"string","maxLength":4000},"intro":{"type":"string","maxLength":4000},
        "items":{"type":"array","minItems":1,"maxItems":12,"items":{"type":"object","additionalProperties":false,"properties":fields,"required":required}}
    }})
}
pub(super) fn kinds() -> Vec<Builtin> {
    vec![
        Builtin {
            kind: "code-tabs",
            description: "Keyboard-accessible code examples with language tabs and copy buttons",
            schema: schema(
                &json!({"title":{"type":"string","maxLength":4000},"language":{"type":"string","maxLength":4000},"code":{"type":"string","format":"multiline","maxLength":16000}}),
                &["title", "code"],
            ),
            check: check_code,
            resolve_fn: resolve_code,
            container: false,
        },
        Builtin {
            kind: "execution-flow",
            description: "An ordered architecture or execution diagram, stacked on phones",
            schema: schema(
                &json!({"title":{"type":"string","maxLength":4000},"label":{"type":"string","maxLength":4000},"body":{"type":"string","format":"multiline","maxLength":4000}}),
                &["title"],
            ),
            check: check_flow,
            resolve_fn: resolve_flow,
            container: false,
        },
        Builtin {
            kind: "status-cards",
            description: "Product capabilities with explicit maturity labels",
            schema: schema(
                &json!({"title":{"type":"string","maxLength":4000},"body":{"type":"string","format":"multiline","maxLength":4000},"status":{"type":"string","enum":["available","preview","experimental","coming-soon"]}}),
                &["title", "status"],
            ),
            check: check_status,
            resolve_fn: resolve_status,
            container: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn examples_validate_and_escape_code_and_labels() {
        for kind in kinds() {
            let settings: MapSettings =
                serde_json::from_value(super::super::sections::kind_sample(kind.kind))
                    .expect("sample");
            assert!((kind.check)(&settings).is_empty(), "{}", kind.kind);
            assert!(render(kind.kind, &settings).contains("vy-product"));
        }
        let s = serde_json::from_value(
            json!({"items":[{"title":"<script>","code":"</code><script>alert(1)</script>"}]}),
        )
        .expect("settings");
        let html = render("code-tabs", &s);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;/code&gt;"));
        assert!(html.contains("tabindex=\"0\""));
    }
    #[test]
    fn malformed_or_unbounded_items_are_rejected() {
        for value in [
            json!({"items":[]}),
            json!({"items":[{}]}),
            json!({"items":[{"title":"x","status":"stable"}]}),
            json!({"items":[{"title":"x","status":"preview","body":42}]}),
            json!({"items":vec![json!({"title":"x","status":"preview"});13]}),
        ] {
            let s = serde_json::from_value(value).expect("settings");
            assert!(!check_status(&s).is_empty());
        }
    }
}
