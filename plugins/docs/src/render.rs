//! `render-block` for the plugin's own block kinds.
//!
//! Plain HTML with `docs-*` classes; behaviour (tab switching, copy
//! buttons) is progressive enhancement from the plugin's script, so the
//! page reads correctly with scripts off: every tab panel is visible and
//! labelled until the script marks the group as enhanced.
//!
//! Container blocks (tabs, steps) mark where their children go with the
//! token `[[vy-child-N]]`; the host swaps in the rendered child after
//! sanitising, so a plugin can wrap rich content it never sees.

use serde_json::Value;

use crate::blocks::esc;

/// The block kinds this plugin renders.
pub const KINDS: &[&str] = &[
    "docs/tabs",
    "docs/code-group",
    "docs/steps",
    "docs/params",
    "docs/file-tree",
];

/// Renders one block.
///
/// # Errors
/// For a kind this plugin does not own, or attributes of the wrong shape.
pub fn render(kind: &str, attrs: &Value) -> Result<String, String> {
    match kind {
        "docs/tabs" => Ok(tabs(attrs)),
        "docs/code-group" => Ok(code_group(attrs)),
        "docs/steps" => Ok(steps(attrs)),
        "docs/params" => Ok(params(attrs)),
        "docs/file-tree" => file_tree(attrs),
        other => Err(format!("this plugin does not render {other}")),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn tabs(attrs: &Value) -> String {
    let labels = strings(&attrs["labels"]);
    let group = attrs["group"]
        .as_str()
        .filter(|g| !g.is_empty())
        .map(|g| format!(" data-group=\"{}\"", esc(g)))
        .unwrap_or_default();
    let mut list = String::new();
    let mut panels = String::new();
    for (i, label) in labels.iter().enumerate() {
        let active = i == 0;
        list.push_str(&format!(
            "<button type=\"button\" class=\"docs-tabs__tab{}\" role=\"tab\" aria-selected=\"{active}\" data-tab=\"{i}\" data-label=\"{}\">{}</button>",
            if active { " is-active" } else { "" },
            esc(label),
            esc(label)
        ));
        panels.push_str(&format!(
            "<div class=\"docs-tabs__panel{}\" role=\"tabpanel\" data-panel=\"{i}\"><p class=\"docs-tabs__label\">{}</p>[[vy-child-{i}]]</div>",
            if active { " is-active" } else { "" },
            esc(label)
        ));
    }
    format!("<div class=\"docs-tabs\" data-docs-tabs{group}><div class=\"docs-tabs__list\" role=\"tablist\">{list}</div>{panels}</div>")
}

fn code_group(attrs: &Value) -> String {
    let tabs = attrs["tabs"].as_array().cloned().unwrap_or_default();
    let mut list = String::new();
    let mut panels = String::new();
    for (i, t) in tabs.iter().enumerate() {
        let label = t["label"].as_str().unwrap_or("Code");
        let lang = t["language"].as_str().unwrap_or("");
        let active = i == 0;
        list.push_str(&format!(
            "<button type=\"button\" class=\"docs-tabs__tab{}\" role=\"tab\" aria-selected=\"{active}\" data-tab=\"{i}\" data-label=\"{}\">{}</button>",
            if active { " is-active" } else { "" },
            esc(label),
            esc(label)
        ));
        panels.push_str(&format!(
            "<div class=\"docs-tabs__panel{}\" role=\"tabpanel\" data-panel=\"{i}\"><p class=\"docs-tabs__label\">{}</p><pre class=\"docs-code\"><code class=\"language-{}\">{}</code></pre></div>",
            if active { " is-active" } else { "" },
            esc(label),
            esc(&lang.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '#')).collect::<String>()),
            esc(t["code"].as_str().unwrap_or(""))
        ));
    }
    format!("<div class=\"docs-tabs docs-codegroup\" data-docs-tabs data-group=\"code\"><div class=\"docs-tabs__list\" role=\"tablist\">{list}</div>{panels}</div>")
}

fn steps(attrs: &Value) -> String {
    // The number of steps is the number of children; the host passes it as
    // `__children`, and the importer also records it as `count`.
    let n = attrs["__children"]
        .as_u64()
        .or_else(|| attrs["count"].as_u64())
        .unwrap_or(0);
    let mut out = String::from("<ol class=\"docs-steps\">");
    for i in 0..n {
        out.push_str(&format!(
            "<li class=\"docs-steps__item\">[[vy-child-{i}]]</li>"
        ));
    }
    out.push_str("</ol>");
    out
}

fn params(attrs: &Value) -> String {
    let rows = attrs["rows"].as_array().cloned().unwrap_or_default();
    let mut out = String::from("<table class=\"docs-params\"><thead><tr><th>Name</th><th>Type</th><th>Default</th><th>Description</th></tr></thead><tbody>");
    for r in rows {
        let required = r["required"].as_bool().unwrap_or(false);
        let default = r["default"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(|d| format!("<code>{}</code>", esc(d)))
            .unwrap_or_else(|| "&mdash;".to_owned());
        out.push_str(&format!(
            "<tr><td><code class=\"docs-params__name\">{}</code>{}</td><td><code>{}</code></td><td>{default}</td><td>{}</td></tr>",
            esc(r["name"].as_str().unwrap_or("")),
            if required { " <span class=\"docs-params__req\">required</span>" } else { "" },
            esc(r["type"].as_str().unwrap_or("")),
            // Inline HTML produced by the importer from Markdown; the host's
            // sanitiser has the final say.
            r["description"].as_str().unwrap_or("")
        ));
    }
    out.push_str("</tbody></table>");
    out
}

/// A tree of files from indented text: two spaces (or a tab) per level,
/// a trailing `/` marks a folder, ` # note` trails a comment.
fn file_tree(attrs: &Value) -> Result<String, String> {
    let text = attrs["tree"]
        .as_str()
        .ok_or("docs/file-tree needs attrs.tree")?;
    let mut items: Vec<(usize, String, bool, String)> = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let indent = raw
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| if c == '\t' { 2 } else { 1 })
            .sum::<usize>()
            / 2;
        let body = raw.trim();
        let (name, note) = match body.split_once(" # ") {
            Some((n, c)) => (n.trim(), c.trim().to_owned()),
            None => (body, String::new()),
        };
        let is_dir = name.ends_with('/');
        items.push((indent, name.trim_end_matches('/').to_owned(), is_dir, note));
    }
    if items.len() > 500 {
        return Err("docs/file-tree: more than 500 lines".to_owned());
    }
    let mut out = String::from("<div class=\"docs-tree\"><ul>");
    let mut depth = 0usize;
    for (i, (raw_indent, name, is_dir, note)) in items.iter().enumerate() {
        let indent = (*raw_indent).min(depth);
        while depth > indent {
            out.push_str("</ul></li>");
            depth -= 1;
        }
        let next_deeper = items.get(i + 1).is_some_and(|n| n.0 > indent);
        let note = if note.is_empty() {
            String::new()
        } else {
            format!(" <span class=\"docs-tree__note\">{}</span>", esc(note))
        };
        out.push_str(&format!(
            "<li class=\"docs-tree__{}\"><span class=\"docs-tree__name\">{}{}</span>{note}",
            if *is_dir || next_deeper {
                "dir"
            } else {
                "file"
            },
            esc(name),
            if *is_dir || next_deeper { "/" } else { "" }
        ));
        if next_deeper {
            out.push_str("<ul>");
            depth += 1;
        } else {
            out.push_str("</li>");
        }
    }
    while depth > 0 {
        out.push_str("</ul></li>");
        depth -= 1;
    }
    out.push_str("</ul></div>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tabs_have_a_button_a_labelled_panel_and_a_child_slot_each() {
        let html = render(
            "docs/tabs",
            &json!({"labels": ["npm", "p<n>pm"], "group": "pkg"}),
        )
        .unwrap();
        assert!(html.contains("data-group=\"pkg\""));
        assert_eq!(html.matches("role=\"tab\"").count(), 2);
        assert!(html.contains("[[vy-child-0]]") && html.contains("[[vy-child-1]]"));
        assert!(html.contains("p&lt;n&gt;pm"));
        assert_eq!(
            html.matches("is-active").count(),
            2,
            "first button and first panel only: {html}"
        );
    }

    #[test]
    fn a_code_group_escapes_code_and_language() {
        let html = render(
            "docs/code-group",
            &json!({"tabs": [{"label": "a", "language": "ts\"><script>", "code": "1 < 2 && x"}]}),
        )
        .unwrap();
        assert!(html.contains("1 &lt; 2 &amp;&amp; x"));
        assert!(!html.contains("<script"));
        assert!(html.contains("language-ts"));
    }

    #[test]
    fn steps_numbers_come_from_the_child_count() {
        let html = render("docs/steps", &json!({"__children": 3})).unwrap();
        assert_eq!(html.matches("docs-steps__item").count(), 3);
        assert!(html.contains("[[vy-child-2]]"));
        assert_eq!(
            render("docs/steps", &json!({}))
                .unwrap()
                .matches("<li")
                .count(),
            0
        );
    }

    #[test]
    fn params_render_required_default_and_description() {
        let html = render("docs/params", &json!({"rows": [
            {"name": "id", "type": "string", "required": true, "default": "", "description": "The <em>id</em>."},
            {"name": "n<", "type": "number", "required": false, "default": "10", "description": ""}]})).unwrap();
        assert!(html.contains("docs-params__req"));
        assert!(html.contains("The <em>id</em>."));
        assert!(html.contains("<code>10</code>"));
        assert!(html.contains("n&lt;"));
        assert!(html.contains("&mdash;"));
    }

    #[test]
    fn a_file_tree_nests_by_indent_and_marks_folders() {
        let html = render(
            "docs/file-tree",
            &json!({"tree": "app/\n  page.tsx # the page\n  api/\n    route.ts\nREADME.md"}),
        )
        .unwrap();
        assert_eq!(
            html.matches("<ul>").count(),
            html.matches("</ul>").count(),
            "{html}"
        );
        assert_eq!(
            html.matches("<li").count(),
            html.matches("</li>").count(),
            "{html}"
        );
        assert!(html.contains("docs-tree__dir"));
        assert!(html.contains("docs-tree__note\">the page"));
        assert!(html.contains("route.ts"));
    }

    #[test]
    fn unknown_kinds_and_missing_attributes_are_errors() {
        assert!(render("docs/nope", &json!({})).is_err());
        assert!(render("docs/file-tree", &json!({})).is_err());
    }
}
