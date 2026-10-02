//! Working-copy operations for the theme studio.
//!
//! A draft is the three documents a theme is made of — tokens, layout and
//! optional Tera overrides — held in memory. Every change, whether a person
//! dragged a block or the assistant proposed a palette, arrives as a
//! [`StudioOp`] and goes through [`apply`], which runs the *same* validators
//! the package installer runs. Nothing invalid ever becomes a revision, and
//! there is exactly one edit path for both producers.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{builtin_sources, Engine};
use crate::layout::{validate_layout, Layout, Section, TemplateType, STATIC_REGIONS};
use crate::tokens::{parse_token_set, validate_token_set, TokenDiagnostic, TokenSet};

/// The template files a theme may override, in the engine's naming.
pub const TEMPLATE_NAMES: [&str; 7] = [
    "base.html",
    "index.html",
    "single.html",
    "archive.html",
    "page.html",
    "search.html",
    "not-found.html",
];

/// Whether `name` is a legal template file name: one of the fixed seven,
/// or a per-type override (`single-book.html`, `archive-book.html`).
///
/// The type part is checked for shape only — types come and go with
/// plugins, so existence is not a compile-time question. A template for a
/// type that never exists simply never renders, which is harmless; a
/// typo'd *stem* would silently shadow nothing, which is why the stem set
/// stays closed.
#[must_use]
pub fn is_template_name(name: &str) -> bool {
    if TEMPLATE_NAMES.contains(&name) {
        return true;
    }
    let Some(stem) = name.strip_suffix(".html") else {
        return false;
    };
    let Some(type_part) = stem
        .strip_prefix("single-")
        .or_else(|| stem.strip_prefix("archive-"))
    else {
        return false;
    };
    type_part.len() >= 2
        && type_part.len() <= 32
        && type_part.starts_with(|c: char| c.is_ascii_lowercase())
        && type_part
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// What each static region is for, shown in the layout editor and handed
/// to the assistant so it composes with the same vocabulary.
pub const STATIC_REGION_DESCRIPTIONS: [(&str, &str); 7] = [
    ("header", "Site name and tagline"),
    (
        "nav",
        "Plain navigation bar (use the `menu` block for a named menu)",
    ),
    ("content", "Main content column"),
    ("post-content", "Body of the current post or page"),
    ("sidebar-left", "Left sidebar column"),
    ("sidebar-right", "Right sidebar column"),
    ("footer", "Site footer"),
];

/// The three documents of a theme, parsed and ready to edit.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftState {
    /// Design tokens.
    pub tokens: TokenSet,
    /// Block composition per template.
    pub layout: Layout,
    /// Tera overrides keyed by template name (`single.html`).
    pub templates: BTreeMap<String, String>,
    /// Stylesheet and script the theme ships with itself.
    pub assets: ThemeAssets,
}

/// CSS and JavaScript a theme carries.
///
/// Separate from the site's custom CSS: that belongs to the site and
/// survives a theme change, this belongs to the theme and is versioned,
/// previewed and rolled back with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThemeAssets {
    /// Stylesheet source, or empty for none.
    #[serde(default)]
    pub css: String,
    /// Script source, or empty for none.
    #[serde(default)]
    pub js: String,
}

/// Largest stylesheet a theme may ship.
pub const MAX_THEME_CSS: usize = 256 * 1024;
/// Largest script a theme may ship.
pub const MAX_THEME_JS: usize = 256 * 1024;

impl ThemeAssets {
    /// Whether either half is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.css.trim().is_empty() && self.js.trim().is_empty()
    }

    /// Checks the sizes.
    ///
    /// # Errors
    /// A message naming the half that is too large.
    pub fn validate(&self) -> Result<(), String> {
        if self.css.len() > MAX_THEME_CSS {
            return Err(format!(
                "theme CSS is {} bytes; the maximum is {MAX_THEME_CSS}",
                self.css.len()
            ));
        }
        if self.js.len() > MAX_THEME_JS {
            return Err(format!(
                "theme JavaScript is {} bytes; the maximum is {MAX_THEME_JS}",
                self.js.len()
            ));
        }
        Ok(())
    }
}

impl DraftState {
    /// Parses stored JSON documents. Unknown template keys are kept so a
    /// stored theme never loses data on the way through the studio.
    ///
    /// # Errors
    /// A message naming the document that failed to parse.
    pub fn from_json(
        tokens: &Value,
        layout: &Value,
        templates: Option<&Value>,
    ) -> Result<Self, String> {
        let tokens =
            parse_token_set(&tokens.to_string()).map_err(|d| format!("tokens: {}", join(&d)))?;
        let layout: Layout =
            serde_json::from_value(layout.clone()).map_err(|e| format!("layout: {e}"))?;
        let mut map = BTreeMap::new();
        for (name, src) in templates.and_then(Value::as_object).into_iter().flatten() {
            if let Some(src) = src.as_str() {
                map.insert(name.clone(), src.to_owned());
            }
        }
        Ok(Self {
            tokens,
            layout,
            templates: map,
            assets: ThemeAssets::default(),
        })
    }

    /// The same, with the theme's own assets document.
    ///
    /// A separate constructor rather than a fourth parameter on
    /// [`DraftState::from_json`], so every existing caller keeps working
    /// and a theme stored before assets existed reads as having none.
    ///
    /// # Errors
    /// A message naming the document that failed to parse.
    pub fn from_json_with_assets(
        tokens: &Value,
        layout: &Value,
        templates: Option<&Value>,
        assets: Option<&Value>,
    ) -> Result<Self, String> {
        let mut state = Self::from_json(tokens, layout, templates)?;
        if let Some(assets) = assets {
            state.assets =
                serde_json::from_value(assets.clone()).map_err(|e| format!("assets: {e}"))?;
            state.assets.validate()?;
        }
        Ok(state)
    }

    /// The assets as the JSON stored on a theme or draft row, or `None`
    /// when the theme ships neither.
    #[must_use]
    pub fn assets_json(&self) -> Option<Value> {
        (!self.assets.is_empty()).then(|| serde_json::json!(self.assets))
    }

    /// Tokens as the JSON stored in the database.
    #[must_use]
    pub fn tokens_json(&self) -> Value {
        serde_json::to_value(&self.tokens).unwrap_or(Value::Null)
    }

    /// Layout as the JSON stored in the database.
    #[must_use]
    pub fn layout_json(&self) -> Value {
        serde_json::to_value(&self.layout).unwrap_or(Value::Null)
    }

    /// Templates as stored: `None` when there are no overrides, matching
    /// what the package installer writes.
    #[must_use]
    pub fn templates_json(&self) -> Option<Value> {
        if self.templates.is_empty() {
            None
        } else {
            serde_json::to_value(&self.templates).ok()
        }
    }
}

/// One edit to a draft. Both the editor and the assistant speak this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum StudioOp {
    /// Replace the whole token document.
    SetTokens {
        /// A complete token document.
        tokens: Value,
    },
    /// Merge a partial token document (RFC 7386 merge patch: objects
    /// merge recursively, `null` removes a key).
    PatchTokens {
        /// The partial document.
        patch: Value,
    },
    /// Replace the section tree of one template.
    SetLayout {
        /// Which template.
        template: TemplateType,
        /// Its new sections, in order.
        ///
        /// Accepts `blocks` too: that was the field's name before layouts
        /// became trees, and both the admin and any saved plan still use it.
        /// The assistant is told to write `sections`, and a prompt that
        /// disagreed with the schema is exactly how a good plan got rejected.
        #[serde(alias = "blocks")]
        sections: Vec<Section>,
    },
    /// Add or replace a Tera override.
    SetTemplate {
        /// One of [`TEMPLATE_NAMES`].
        name: String,
        /// Template source.
        source: String,
    },
    /// Drop a Tera override so the built-in template renders again.
    RemoveTemplate {
        /// One of [`TEMPLATE_NAMES`].
        name: String,
    },
    /// Replace the theme's own stylesheet and script.
    SetAssets {
        /// Stylesheet source; empty removes it.
        css: String,
        /// Script source; empty removes it.
        js: String,
    },
    /// Prose only — the assistant explaining itself. Changes nothing.
    Explain {
        /// What to tell the author.
        text: String,
    },
}

/// The result of applying ops: the new state plus what changed.
#[derive(Debug, Clone)]
pub struct Applied {
    /// State after every op.
    pub state: DraftState,
    /// Human-readable change notes, one per effective op.
    pub changes: Vec<String>,
    /// Non-blocking validator observations (contrast, dark palette…).
    pub warnings: Vec<TokenDiagnostic>,
}

/// Applies `ops` to `state`, validating the result as a whole.
///
/// All ops are applied before validation, so a batch that moves through an
/// invalid intermediate state (say, removing a template another one
/// extends, then removing that one too) still succeeds when the end state
/// is valid.
///
/// # Errors
/// Every hard diagnostic found in the end state. The caller's state is
/// untouched.
/// A sentence describing what changed about the theme's own assets.
fn describe_assets(before: &ThemeAssets, after: &ThemeAssets) -> String {
    let part =
        |was: &str, now: &str, what: &str| match (was.trim().is_empty(), now.trim().is_empty()) {
            (true, false) => Some(format!("added {what}")),
            (false, true) => Some(format!("removed {what}")),
            (false, false) if was != now => Some(format!("edited {what}")),
            _ => None,
        };
    let mut parts: Vec<String> = Vec::new();
    parts.extend(part(&before.css, &after.css, "the theme stylesheet"));
    parts.extend(part(&before.js, &after.js, "the theme script"));
    if parts.is_empty() {
        return String::from("Left the theme assets unchanged");
    }
    let mut sentence = parts.join(" and ");
    if let Some(first) = sentence.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    sentence
}

/// Applies `ops` to `state`, validating the result as a whole.
///
/// All ops are applied before validation, so a batch that moves through an
/// invalid intermediate state (say, removing a template another one
/// extends, then removing that one too) still succeeds when the end state
/// is valid.
///
/// # Errors
/// Every hard diagnostic found in the end state. The caller's state is
/// untouched.
pub fn apply(state: &DraftState, ops: &[StudioOp]) -> Result<Applied, Vec<TokenDiagnostic>> {
    apply_with(state, ops, &crate::dynblocks::builtin_registry())
}

/// [`apply`] against a caller-supplied registry.
///
/// The host passes the live one — built-ins plus enabled plugins'
/// sections — so a plugin kind validates with its real schema while its
/// plugin is on, and softens to the survival warning while it is off.
///
/// # Errors
/// See [`apply`].
pub fn apply_with(
    state: &DraftState,
    ops: &[StudioOp],
    registry: &crate::dynblocks::DynBlockRegistry,
) -> Result<Applied, Vec<TokenDiagnostic>> {
    let mut next = state.clone();
    let mut changes = Vec::new();
    let mut errors = Vec::new();

    for op in ops {
        match op {
            StudioOp::SetTokens { tokens } => match parse_tokens(tokens) {
                Ok(parsed) => {
                    changes.push(describe_tokens(&next.tokens, &parsed));
                    next.tokens = parsed;
                }
                Err(mut d) => errors.append(&mut d),
            },
            StudioOp::PatchTokens { patch } => {
                let mut doc = next.tokens_json();
                merge_patch(&mut doc, patch);
                match parse_tokens(&doc) {
                    Ok(parsed) => {
                        changes.push(describe_tokens(&next.tokens, &parsed));
                        next.tokens = parsed;
                    }
                    Err(mut d) => errors.append(&mut d),
                }
            }
            StudioOp::SetLayout { template, sections } => {
                changes.push(format!("Changed the {} layout", template.as_str()));
                next.layout.for_template_mut(*template).clone_from(sections);
            }
            StudioOp::SetTemplate { name, source } => {
                if !is_template_name(name) {
                    errors.push(TokenDiagnostic::Error {
                        path: format!("templates.{name}"),
                        message: format!(
                            "not a template a theme can override (expected one of: {}, \
                             or a per-type single-<type>.html / archive-<type>.html)",
                            TEMPLATE_NAMES.join(", ")
                        ),
                    });
                    continue;
                }
                let verb = if next.templates.contains_key(name) {
                    "Edited"
                } else {
                    "Added"
                };
                changes.push(format!("{verb} the {name} template"));
                next.templates.insert(name.clone(), source.clone());
            }
            StudioOp::RemoveTemplate { name } => {
                if next.templates.remove(name).is_some() {
                    changes.push(format!("Removed the {name} override"));
                }
            }
            StudioOp::SetAssets { css, js } => {
                let next_assets = ThemeAssets {
                    css: css.clone(),
                    js: js.clone(),
                };
                match next_assets.validate() {
                    Ok(()) => {
                        changes.push(describe_assets(&next.assets, &next_assets));
                        next.assets = next_assets;
                    }
                    Err(message) => errors.push(TokenDiagnostic::Error {
                        path: String::from("assets"),
                        message,
                    }),
                }
            }
            StudioOp::Explain { .. } => {}
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut diags = validate_token_set(&next.tokens);
    diags.extend(validate_layout(&next.layout, registry));
    if let Err(e) = compile_templates(&next.templates) {
        diags.push(e);
    }
    let (errors, warnings): (Vec<_>, Vec<_>) =
        diags.into_iter().partition(TokenDiagnostic::is_error);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Applied {
        state: next,
        changes,
        warnings,
    })
}

/// Compiles every override together, the way the renderer will load them,
/// so a template that `include`s a sibling is checked against that sibling.
///
/// # Errors
/// One diagnostic pointing at the failing template.
pub fn compile_templates(templates: &BTreeMap<String, String>) -> Result<(), TokenDiagnostic> {
    let mut engine = Engine::builtin().map_err(|e| TokenDiagnostic::Error {
        path: "templates".to_owned(),
        message: e.to_string(),
    })?;
    let pairs: Vec<(&str, &str)> = templates
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    engine.install_theme_templates(&pairs).map_err(|e| {
        let name = templates
            .keys()
            .find(|k| e.0.contains(&format!("\"{k}\"")))
            .cloned()
            .unwrap_or_default();
        TokenDiagnostic::Error {
            path: if name.is_empty() {
                "templates".to_owned()
            } else {
                format!("templates.{name}")
            },
            message: e.to_string(),
        }
    })
}

fn parse_tokens(doc: &Value) -> Result<TokenSet, Vec<TokenDiagnostic>> {
    parse_token_set(&doc.to_string())
}

fn describe_tokens(before: &TokenSet, after: &TokenSet) -> String {
    let paths = diff_paths(
        &serde_json::to_value(before).unwrap_or(Value::Null),
        &serde_json::to_value(after).unwrap_or(Value::Null),
    );
    match paths.len() {
        0 => "No token changes".to_owned(),
        1..=3 => format!("Changed {}", paths.join(", ")),
        n => format!("Changed {} and {} more", paths[..2].join(", "), n - 2),
    }
}

fn join(diags: &[TokenDiagnostic]) -> String {
    diags
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// RFC 7386 JSON merge patch, in place.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(patch_map) => {
            if !target.is_object() {
                *target = Value::Object(serde_json::Map::new());
            }
            let Some(map) = target.as_object_mut() else {
                return;
            };
            for (key, value) in patch_map {
                if value.is_null() {
                    map.remove(key);
                } else {
                    let slot = map.entry(key.clone()).or_insert(Value::Null);
                    merge_patch(slot, value);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Dot-separated paths whose values differ between two documents. Arrays
/// are compared whole, so a reordered list reports as one change.
#[must_use]
pub fn diff_paths(a: &Value, b: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_diff("", a, b, &mut out);
    out
}

fn walk_diff(prefix: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(ma), Value::Object(mb)) => {
            let mut keys: Vec<&String> = ma.keys().chain(mb.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match (ma.get(key), mb.get(key)) {
                    (Some(x), Some(y)) => walk_diff(&path, x, y, out),
                    _ => out.push(path),
                }
            }
        }
        _ if a != b => out.push(if prefix.is_empty() {
            "document".to_owned()
        } else {
            prefix.to_owned()
        }),
        _ => {}
    }
}

/// Everything an editor (or the assistant) needs to compose a layout and
/// author tokens: the block vocabulary with settings schemas, the template
/// names and their built-in sources, and the token schema. Generated from
/// the same registry the validator uses, so it cannot drift.
#[must_use]
pub fn registry_schema() -> Value {
    registry_schema_for(&crate::dynblocks::builtin_registry())
}

/// [`registry_schema`] over a caller-supplied registry, so the host can
/// serve a vocabulary that includes enabled plugins' sections. Everything
/// downstream — the insert library, the inspector forms, both assistants —
/// reads this one document, which is how a plugin's section shows up in
/// all of them with no host special cases.
#[must_use]
pub fn registry_schema_for(registry: &crate::dynblocks::DynBlockRegistry) -> Value {
    let blocks: Vec<Value> = registry
        .kinds()
        .into_iter()
        .filter_map(|kind| registry.get(kind))
        .map(|b| {
            serde_json::json!({
                "kind": b.kind(),
                "description": b.description(),
                "settings_schema": b.settings_schema(),
                // The assistant needs to know where nesting is legal;
                // without this it has to guess, and a guess costs a
                // rejected op and a wasted turn.
                "container": b.container(),
                // Studio 2.0: insert-library grouping, starter settings
                // for a fresh instance, and the keys the canvas edits in
                // place. The assistants read the same three fields.
                "category": b.category(),
                "sample": b.sample(),
                "inline": b.inline_editable(),
            })
        })
        .collect();
    let statics: Vec<Value> = STATIC_REGIONS
        .iter()
        .map(|kind| {
            let description = STATIC_REGION_DESCRIPTIONS
                .iter()
                .find(|(k, _)| k == kind)
                .map_or("", |(_, d)| *d);
            serde_json::json!({ "kind": kind, "description": description })
        })
        .collect();
    let template_files: Vec<Value> = builtin_sources()
        .iter()
        .map(|(name, src)| serde_json::json!({ "name": name, "builtin_source": src }))
        .collect();
    serde_json::json!({
        "static_regions": statics,
        "blocks": blocks,
        "templates": TemplateType::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
        "template_files": template_files,
        "token_schema": crate::tokens::json_schema(),
        // The assistant composes with the same vocabulary an author has,
        // so it needs to know assets exist and how large they may be.
        "assets": {
            "description": "CSS and JavaScript the theme ships with itself, \
                            versioned and rolled back with it",
            "max_css_bytes": MAX_THEME_CSS,
            "max_js_bytes": MAX_THEME_JS,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn blog() -> DraftState {
        let tokens: Value =
            serde_json::from_str(include_str!("../../../themes-starter/blog/tokens.json")).unwrap();
        let layout: Value =
            serde_json::from_str(include_str!("../../../themes-starter/blog/layout.json")).unwrap();
        DraftState::from_json(&tokens, &layout, None).unwrap()
    }

    #[test]
    fn merge_patch_follows_rfc_7386() {
        let mut doc = json!({"a": {"b": 1, "c": 2}, "d": [1, 2]});
        merge_patch(&mut doc, &json!({"a": {"b": null, "e": 3}, "d": [9]}));
        assert_eq!(doc, json!({"a": {"c": 2, "e": 3}, "d": [9]}));
    }

    #[test]
    fn patch_tokens_changes_one_colour_and_names_it() {
        let state = blog();
        let applied = apply(
            &state,
            &[StudioOp::PatchTokens {
                patch: json!({"colors": {"primary": {"light": "#123456"}}}),
            }],
        )
        .unwrap();
        assert_eq!(applied.state.tokens.colors.primary.light.0, "#123456");
        assert_eq!(applied.changes, vec!["Changed colors.primary.light"]);
    }

    #[test]
    fn invalid_hex_is_rejected_with_a_path() {
        let state = blog();
        let err = apply(
            &state,
            &[StudioOp::PatchTokens {
                patch: json!({"colors": {"primary": {"light": "blue"}}}),
            }],
        )
        .unwrap_err();
        assert!(
            err.iter()
                .any(|d| d.path().contains("colors.primary.light")),
            "{err:?}"
        );
    }

    #[test]
    fn low_contrast_is_a_warning_not_a_failure() {
        let state = blog();
        let applied = apply(
            &state,
            &[StudioOp::PatchTokens {
                patch: json!({"colors": {"text": {"light": "#eeeeee"}}}),
            }],
        )
        .unwrap();
        assert!(applied.warnings.iter().any(|w| w.path() == "colors.text"));
    }

    #[test]
    fn layout_with_unknown_kind_is_rejected() {
        let state = blog();
        let err = apply(
            &state,
            &[StudioOp::SetLayout {
                template: TemplateType::Index,
                sections: vec![Section {
                    id: "x".into(),
                    kind: "carousel".into(),
                    settings: crate::layout::MapSettings::default(),
                    children: Vec::new(),
                    scope: None,
                    hide_on: None,
                    width: None,
                }],
            }],
        )
        .unwrap_err();
        assert!(err.iter().any(|d| d.path() == "index[0].kind"));
    }

    #[test]
    fn templates_compile_together_and_sandbox_holds() {
        let state = blog();
        let ok = apply(
            &state,
            &[StudioOp::SetTemplate {
                name: "single.html".into(),
                source: "{% extends \"base.html\" %}{% block body %}<p>{{ page.title }}</p>{% endblock %}"
                    .into(),
            }],
        )
        .unwrap();
        assert_eq!(ok.state.templates.len(), 1);
        assert_eq!(ok.changes, vec!["Added the single.html template"]);

        let escape = apply(
            &state,
            &[StudioOp::SetTemplate {
                name: "single.html".into(),
                source: "{{ get_env(name=\"HOME\") }}".into(),
            }],
        )
        .unwrap_err();
        assert_eq!(escape[0].path(), "templates.single.html");

        let unknown = apply(
            &state,
            &[StudioOp::SetTemplate {
                name: "evil.html".into(),
                source: "x".into(),
            }],
        )
        .unwrap_err();
        assert_eq!(unknown[0].path(), "templates.evil.html");
    }

    #[test]
    fn a_batch_is_validated_as_a_whole() {
        let mut state = blog();
        state
            .templates
            .insert("index.html".into(), "{% include \"single.html\" %}".into());
        state.templates.insert("single.html".into(), "hi".into());
        // Removing both in one batch is fine even though removing only the
        // included one would leave a dangling include.
        let ok = apply(
            &state,
            &[
                StudioOp::RemoveTemplate {
                    name: "single.html".into(),
                },
                StudioOp::RemoveTemplate {
                    name: "index.html".into(),
                },
            ],
        )
        .unwrap();
        assert!(ok.state.templates.is_empty());
    }

    #[test]
    fn explain_changes_nothing() {
        let state = blog();
        let applied = apply(
            &state,
            &[StudioOp::Explain {
                text: "thinking".into(),
            }],
        )
        .unwrap();
        assert_eq!(applied.state, state);
        assert!(applied.changes.is_empty());
    }

    #[test]
    fn set_layout_accepts_both_field_names() {
        // `sections` is what the assistant is told to write; `blocks` is what
        // the admin and every saved plan already send. Both must parse, or a
        // rename silently breaks one of the two producers.
        let with_sections: StudioOp = serde_json::from_value(serde_json::json!({
            "op": "set_layout",
            "template": "page",
            "sections": [{"id": "a", "kind": "header"}]
        }))
        .expect("sections form");
        let with_blocks: StudioOp = serde_json::from_value(serde_json::json!({
            "op": "set_layout",
            "template": "page",
            "blocks": [{"id": "a", "kind": "header"}]
        }))
        .expect("blocks form");
        assert_eq!(
            serde_json::to_value(&with_sections).ok(),
            serde_json::to_value(&with_blocks).ok()
        );
    }

    #[test]
    fn registry_schema_matches_the_registry() {
        let schema = registry_schema();
        let kinds: Vec<&str> = schema["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["kind"].as_str().unwrap())
            .collect();
        let registry = crate::dynblocks::builtin_registry();
        assert_eq!(kinds, registry.kinds());
        assert_eq!(schema["template_files"].as_array().unwrap().len(), 7);
        assert!(schema["token_schema"]["properties"]["colors"].is_object());
    }

    #[test]
    fn ops_round_trip_through_json() {
        let op = StudioOp::SetLayout {
            template: TemplateType::NotFound,
            sections: vec![],
        };
        let json = serde_json::to_value(&op).unwrap();
        assert_eq!(json["op"], "set_layout");
        assert_eq!(json["template"], "not-found");
        assert_eq!(serde_json::from_value::<StudioOp>(json).unwrap(), op);
    }
}

#[cfg(test)]
mod asset_tests {
    use super::{apply, DraftState, StudioOp, ThemeAssets, MAX_THEME_CSS, MAX_THEME_JS};
    use serde_json::json;

    /// A layout with every template filled, since `apply` validates the
    /// whole draft and an empty template is a hard diagnostic.
    fn layout() -> serde_json::Value {
        let blocks = json!([{"id": "main", "kind": "post-content"}]);
        json!({
            "index": blocks, "single": blocks, "archive": blocks,
            "page": blocks, "search": blocks, "not-found": blocks,
        })
    }

    fn state() -> DraftState {
        DraftState::from_json(&json!({"version": 1}), &layout(), None).expect("a starting state")
    }

    #[test]
    fn a_theme_starts_with_no_assets_and_stores_none() {
        let state = state();
        assert!(state.assets.is_empty());
        // Nothing to store means nothing stored, so a token-only theme
        // does not carry an empty assets document around.
        assert!(state.assets_json().is_none());
    }

    #[test]
    fn setting_assets_is_an_op_like_any_other() {
        let applied = apply(
            &state(),
            &[StudioOp::SetAssets {
                css: String::from(".vy-card{border-radius:0}"),
                js: String::new(),
            }],
        )
        .expect("valid");
        assert_eq!(applied.state.assets.css, ".vy-card{border-radius:0}");
        assert!(applied.state.assets.js.is_empty());
        // The change note is what the studio shows in history and what the
        // assistant reads back, so it has to say what happened.
        assert_eq!(applied.changes, vec!["Added the theme stylesheet"]);
        assert!(applied.state.assets_json().is_some());

        // Editing, then removing, each read correctly.
        let edited = apply(
            &applied.state,
            &[StudioOp::SetAssets {
                css: String::from(".vy-card{border-radius:8px}"),
                js: String::from("console.log(1)"),
            }],
        )
        .expect("valid");
        assert_eq!(
            edited.changes,
            vec!["Edited the theme stylesheet and added the theme script"]
        );
        let removed = apply(
            &edited.state,
            &[StudioOp::SetAssets {
                css: String::new(),
                js: String::new(),
            }],
        )
        .expect("valid");
        assert_eq!(
            removed.changes,
            vec!["Removed the theme stylesheet and removed the theme script"]
        );
        assert!(removed.state.assets_json().is_none());
    }

    #[test]
    fn oversized_assets_are_refused_as_a_diagnostic_not_a_panic() {
        for op in [
            StudioOp::SetAssets {
                css: "x".repeat(MAX_THEME_CSS + 1),
                js: String::new(),
            },
            StudioOp::SetAssets {
                css: String::new(),
                js: "x".repeat(MAX_THEME_JS + 1),
            },
        ] {
            let diags = apply(&state(), &[op]).expect_err("too large");
            assert!(
                format!("{diags:?}").contains("maximum"),
                "a size message: {diags:?}"
            );
        }
    }

    #[test]
    fn assets_round_trip_through_the_stored_document() {
        let assets = ThemeAssets {
            css: String::from("body{}"),
            js: String::from("void 0"),
        };
        let stored = serde_json::json!(assets);
        let back = DraftState::from_json_with_assets(
            &json!({"version": 1}),
            &layout(),
            None,
            Some(&stored),
        )
        .expect("parses");
        assert_eq!(back.assets, assets);

        // A theme stored before assets existed reads as having none rather
        // than failing to load.
        let older =
            DraftState::from_json_with_assets(&json!({"version": 1}), &layout(), None, None)
                .expect("parses");
        assert!(older.assets.is_empty());
    }

    #[test]
    fn the_studio_vocabulary_mentions_assets_so_the_assistant_can_use_them() {
        let schema = super::registry_schema();
        assert!(schema["assets"]["max_css_bytes"].is_number(), "{schema}");
        // And the new viewer block is in the palette, because the palette
        // is generated from the registry rather than listed by hand.
        let kinds: Vec<&str> = schema["blocks"]
            .as_array()
            .expect("blocks")
            .iter()
            .filter_map(|b| b["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"viewer-greeting"), "{kinds:?}");
    }
}
