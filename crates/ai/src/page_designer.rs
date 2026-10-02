//! The page designer: a request in words becomes one page's section tree.
//!
//! The theme assistant in [`crate::theme_studio`] edits a *theme* — the
//! shape every page on the site inherits. This edits a single page, which
//! is the other half of the job and the one authors ask for by name: "make
//! me a landing page for the launch".
//!
//! It composes with exactly the vocabulary the validator accepts, and a
//! tree that fails validation goes back to the model with the diagnostics
//! rather than reaching an author's editor.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use vyasa_themes::{Section, TokenDiagnostic, TokenSet};

/// Characters of the page's own prose shown to the model.
const MAX_PROSE_CHARS: usize = 3000;

/// Chrome the page renderer places from the theme, never from a page tree.
///
/// Naming them in the prompt costs a line and saves a repair round: a model
/// that has seen theme layouts will otherwise open with a `header`.
const THEME_CHROME: [&str; 5] = ["header", "nav", "footer", "sidebar-left", "sidebar-right"];

/// Everything the designer is told about the page it is composing.
pub struct DesignInput<'a> {
    /// The vocabulary: block kinds with settings schemas, static regions
    /// (`vyasa_themes::registry_schema()`).
    pub vocabulary: &'a Value,
    /// Site title and tagline, so a page suits the site it lives on.
    pub site: &'a str,
    /// The brand kit as prose: who the site is for, how it sounds, what to
    /// avoid. Empty when nobody has written one.
    ///
    /// This is the countermeasure to output that reads like the median
    /// website for a category — without it a model has only the page's own
    /// words to go on, and every site in a category has similar words.
    pub brand: &'a str,
    /// The page's title.
    pub title: &'a str,
    /// The page's own writing, as plain text.
    pub prose: &'a str,
    /// The tree as it stands; empty when the page has never been composed.
    pub current: &'a [Section],
    /// The request, in the author's words.
    pub message: &'a str,
    /// Model identifier for the audit row.
    pub model: &'a str,
}

/// What the model returns: prose for the author plus the whole new tree.
///
/// Sections arrive as raw JSON so a malformed one is reported back by index
/// (`sections[2].kind: unknown kind "banner"`) rather than as a parse
/// failure with no position.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Design {
    /// What was composed and why, for the author. Plain text.
    pub reply: String,
    /// The complete tree for this page, replacing whatever was there.
    pub sections: Vec<Value>,
}

/// System prompt; the vocabulary is appended by [`system_prompt`].
pub const SYSTEM: &str = "You are the page designer inside Vyasa. You compose one page by \
returning its sections, never by writing HTML or CSS.\n\
\n\
A page's sections are an ordered tree. Each is \
{id, kind, settings?, children?, scope?}:\n\
- `kind` is a static region or a registered kind from the vocabulary below.\n\
- `settings` must satisfy that kind's schema.\n\
- `id` is lowercase [a-z0-9:_-] and unique across the whole tree.\n\
- `children` is only legal on kinds marked [container] (band, columns, \
grid, group). A leaf kind with children is rejected. Nest at most 5 deep.\n\
- `hide_on` withholds a section from one screen size: \"mobile\" for \
something that is noise on a phone, \"desktop\" for something meant only \
for one. Omit it and the section is shown everywhere, which is usually \
right — reach for it rather than composing the page twice.\n\
- `scope` restyles one section and everything inside it: \
{bg, surface, text, text_muted, border, primary, on_primary, padding_y, \
contained}. Each colour is a hex literal or a reference to a theme role \
written \"$role\" — a dark band is {\"bg\": \"$text\", \"text\": \"$bg\"}. \
Prefer references: they keep tracking the palette when it changes, \
literals do not. A scoped section spans the page unless you set \
\"contained\": true. Overriding `bg` and `text` alone leaves secondary \
text at its old colour on the new background: set `text_muted` too, or \
the sub-headings become unreadable.\n\\n\
A `collection` section draws entries from the site's content at render \
time: its settings carry {\"bind\": {\"source\", \"term\"?, \"sort\"?, \
\"limit\"?}}. Bind only to a source listed under `sources` in the \
vocabulary — never invent one; an unknown source renders as nothing, \
which reads as a broken page.\n\
Kinds written namespace/name come from plugins. Use one only when the \
vocabulary lists it — never invent a namespaced kind, and never guess at \
one from a plugin's name: an unlisted kind renders as nothing.\n\
Every section may carry `width`: \"narrow\" for prose, \"content\" \
(the default column), \"wide\", or \"full\" edge-to-edge. Prefer \
narrow for long text and wide/full for imagery and bands; width wins \
over `contained`.\n\
An `image` or `video` section shows one piece of media (settings: src, \
alt/caption); a hero with an `image` setting splits copy and picture. \
Always write alt text that says what the image shows.\n\
\n\
What composing a page means here:\n\
- These sections replace this page's body, and only this page's. The \
theme still supplies the header, navigation and footer, so never emit \
them — they are dropped.\n\
- The author has already written this page. Include a `content` section \
wherever their writing belongs, and place it deliberately: between a hero \
and a call to action reads better than above everything. Omit it only if \
the page genuinely has no prose.\n\
- Write real copy from the page's own title and words. Never \
placeholder text, never invented facts, figures, names or testimonials.\n\
- A marketing page usually reads: `hero`, then `feature-grid` or \
`columns` of substance, optionally `stats-band` or `logo-wall`, and \
closes with a `cta-band` in a scoped `band`.\n\
- A repeating section takes one list: a `feature-grid` with three items \
is one section, not three grids inside `columns`.\n\
\n\
Rules:\n\
- Return the complete tree every time, not a patch. To leave the page \
alone, return the tree you were given.\n\
- Scale the change to the request. A request naming one property -- a \
wider hero, a darker band -- changes that and nothing else. A request \
naming a whole look -- \"magazine\", \"editorial\", \"brutalist\", \
\"like a portfolio\" -- is a request to restyle, and type and colour \
alone will never deliver one: rebuild the section tree to match. Either \
way, do not redesign what the request did not reach.\n\
- An editorial or magazine look is a shape, not a typeface: a `hero` for \
the lead item, a `grid` or `feature-grid` for the rest, `columns` where a \
sidebar earns its place, and rules and open space in place of boxes.\n\
- Keep text readable on its background: inside a scope as well as at the \
root, aim for WCAG AA (4.5:1).\n\
- When you reply to the author: one to three short sentences, what you \
composed and anything they should know. Plain text, no markdown.";

/// The system prompt with the vocabulary embedded, so the model composes
/// with exactly the kinds and fields the validator accepts.
#[must_use]
pub fn system_prompt(vocabulary: &Value) -> String {
    let blocks = vocabulary
        .get("blocks")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|b| {
                    let holds = if b["container"].as_bool() == Some(true) {
                        " [container: may hold children]"
                    } else {
                        ""
                    };
                    format!(
                        "- {}{} — {} settings: {}",
                        b["kind"].as_str().unwrap_or(""),
                        holds,
                        b["description"].as_str().unwrap_or(""),
                        compact(&b["settings_schema"])
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    // Only the regions a page may actually use: the rest are theme chrome
    // and listing them as available would invite a wasted repair round.
    let regions = vocabulary
        .get("static_regions")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|r| {
                    r["kind"]
                        .as_str()
                        .is_some_and(|k| !THEME_CHROME.contains(&k))
                })
                .map(|r| {
                    format!(
                        "- {} — {}",
                        r["kind"].as_str().unwrap_or(""),
                        r["description"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    format!("{SYSTEM}\n\nRegions a page may use:\n{regions}\n\nSection kinds:\n{blocks}")
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// The colour roles as they currently resolve, so `$role` is a value the
/// model can reason about rather than a name it is guessing at.
fn palette_line(tokens: &TokenSet) -> String {
    let c = &tokens.colors;
    format!(
        "Theme colours (light): $bg {}, $surface {}, $text {}, $text_muted {}, \
         $border {}, $primary {}, $on_primary {}",
        c.bg.light.normalized(),
        c.surface.light.normalized(),
        c.text.light.normalized(),
        c.text_muted.light.normalized(),
        c.border.light.normalized(),
        c.primary.light.normalized(),
        c.on_primary.light.normalized(),
    )
}

/// The user turn: the site, the page, its current shape, the request.
#[must_use]
pub fn user_prompt(input: &DesignInput<'_>, tokens: &TokenSet) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Site: {}", input.site);
    let _ = writeln!(out, "{}\n", palette_line(tokens));
    if !input.brand.trim().is_empty() {
        out.push_str("The site's brand kit — follow it over any house style of your own:\n");
        out.push_str(input.brand.trim());
        out.push_str("\n\n");
    }
    let _ = writeln!(out, "Page title: {}", input.title);

    out.push_str("\nThe page's own writing:\n");
    if input.prose.trim().is_empty() {
        out.push_str("(empty — the author has not written the body yet)\n");
    } else {
        let prose: String = input.prose.chars().take(MAX_PROSE_CHARS).collect();
        out.push_str(prose.trim());
        out.push('\n');
    }

    out.push_str("\nCurrent sections:\n");
    if input.current.is_empty() {
        out.push_str("(none — this page renders through the theme's page template)\n");
    } else {
        out.push_str(&compact(
            &serde_json::to_value(input.current).unwrap_or(Value::Null),
        ));
        out.push('\n');
    }

    let _ = writeln!(out, "\nRequest:\n{}", input.message);
    out
}

/// Parses and validates a plan's sections against the live registry.
/// What the api layer lends the page toolbox: rendering eyes.
pub trait PageEyes: Send {
    /// Renders the page with the sandbox sections over its real content
    /// and returns a readable digest.
    fn look<'a>(
        &'a mut self,
        sandbox: &'a [Section],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

/// The page agent's toolbox: a sandbox section tree and the same
/// validation a hand edit passes. The caller reads `sandbox` back as the
/// proposal; nothing here persists.
pub struct PageToolbox<'t, E> {
    /// The working tree; starts as the page's current sections.
    pub sandbox: Vec<Section>,
    /// Contrast notes and kin from the latest accepted edit.
    pub warnings: Vec<TokenDiagnostic>,
    tokens: &'t TokenSet,
    registry: vyasa_themes::DynBlockRegistry,
    eyes: Option<E>,
}

impl<'t, E: PageEyes> PageToolbox<'t, E> {
    /// A toolbox over a copy of `current`.
    #[must_use]
    pub fn new(
        current: &[Section],
        tokens: &'t TokenSet,
        registry: vyasa_themes::DynBlockRegistry,
        eyes: Option<E>,
    ) -> Self {
        Self {
            sandbox: current.to_vec(),
            warnings: Vec::new(),
            tokens,
            registry,
            eyes,
        }
    }

    fn edit(&mut self, input: &Value) -> Result<String, String> {
        let raw = input
            .get("sections")
            .and_then(Value::as_array)
            .ok_or("edit takes {\"sections\": [..]} — the page's whole tree, in order")?;
        let plan = Design {
            reply: String::new(),
            sections: raw.clone(),
        };
        let sections = check(&plan, self.tokens, Some(&self.registry)).map_err(|diags| {
            diags
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        self.warnings = vyasa_themes::validate_sections(
            &sections,
            &self.registry,
            Some(self.tokens),
            "sections",
        );
        let summary = format!(
            "Replaced the tree: {} top-level section(s) ({}).{}",
            sections.len(),
            sections
                .iter()
                .map(|s| s.kind.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if self.warnings.is_empty() {
                String::new()
            } else {
                format!(
                    "\nWarnings (fix before finishing):\n{}",
                    self.warnings
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
        );
        self.sandbox = sections;
        Ok(summary)
    }
}

impl<E: PageEyes> crate::agent::Toolbox for PageToolbox<'_, E> {
    fn tools(&self) -> Vec<crate::agent::ToolDef> {
        let mut tools = vec![
            crate::agent::ToolDef {
                name: "inspect",
                description: "Read the page's current section tree as JSON.",
                input_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            },
            crate::agent::ToolDef {
                name: "edit",
                description: "Replace the page's section tree. Send the WHOLE tree in order; \
                              invalid sections come back with the validator's diagnostics.",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"sections": {"type": "array", "items": {"type": "object"}}},
                    "required": ["sections"],
                    "additionalProperties": false
                }),
            },
        ];
        if self.eyes.is_some() {
            tools.push(crate::agent::ToolDef {
                name: "look",
                description: "Render this page with the current sandbox over its real \
                              content and read it back. Use after meaningful edits.",
                input_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            });
        }
        tools
    }

    fn call<'a>(
        &'a mut self,
        name: &str,
        input: &Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        match name {
            "edit" => {
                let out = self.edit(input);
                Box::pin(async move { out })
            }
            "inspect" => {
                let out = serde_json::to_value(&self.sandbox)
                    .map(|v| v.to_string())
                    .map_err(|e| e.to_string());
                Box::pin(async move { out })
            }
            "look" => Box::pin(async move {
                let Some(mut eyes) = self.eyes.take() else {
                    return Err("this run has no eyes".to_owned());
                };
                let out = eyes.look(&self.sandbox).await;
                self.eyes = Some(eyes);
                out
            }),
            other => {
                let msg = format!("no tool named {other:?}");
                Box::pin(async move { Err(msg) })
            }
        }
    }
}

fn check(
    plan: &Design,
    tokens: &TokenSet,
    registry: Option<&vyasa_themes::DynBlockRegistry>,
) -> Result<Vec<Section>, Vec<TokenDiagnostic>> {
    let built;
    let registry = if let Some(r) = registry {
        r
    } else {
        built = vyasa_themes::builtin_registry();
        &built
    };
    let mut sections = Vec::with_capacity(plan.sections.len());
    let mut errors = Vec::new();
    for (i, raw) in plan.sections.iter().enumerate() {
        match serde_json::from_value::<Section>(raw.clone()) {
            Ok(s) => sections.push(s),
            Err(e) => errors.push(TokenDiagnostic::Error {
                path: format!("sections[{i}]"),
                message: e.to_string(),
            }),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let diags = vyasa_themes::validate_sections(&sections, registry, Some(tokens), "sections");
    let errors: Vec<_> = diags
        .into_iter()
        .filter(TokenDiagnostic::is_error)
        .collect();
    if errors.is_empty() {
        Ok(sections)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vyasa_themes::MapSettings;

    fn check_builtin(
        plan: &Design,
        tokens: &TokenSet,
    ) -> Result<Vec<Section>, Vec<TokenDiagnostic>> {
        check(plan, tokens, None)
    }

    fn plan(sections: &serde_json::Value) -> Design {
        Design {
            reply: "done".into(),
            sections: sections.as_array().cloned().unwrap_or_default(),
        }
    }

    fn settings(pairs: &[(&str, Value)]) -> MapSettings {
        let mut s = MapSettings::default();
        for (k, v) in pairs {
            s.set((*k).to_owned(), v.clone());
        }
        s
    }

    #[test]
    fn the_prompt_carries_every_kind_the_validator_accepts() {
        // The output schema only says "object", so the prompt is the whole
        // contract for what a section may be.
        let prompt = system_prompt(&vyasa_themes::registry_schema());
        for kind in vyasa_themes::builtin_registry().kinds() {
            assert!(prompt.contains(&format!("- {kind}")), "missing {kind}");
        }
    }

    #[test]
    fn the_prompt_marks_containers() {
        // Whether `children` is legal is not inferable from a settings
        // schema, so the prompt has to state it or the model will guess.
        let prompt = system_prompt(&vyasa_themes::registry_schema());
        for container in ["band", "columns", "grid", "group"] {
            assert!(
                prompt.contains(&format!("- {container} [container")),
                "{container} not marked as a container"
            );
        }
        assert!(!prompt.contains("- hero [container"), "hero is a leaf");
    }

    #[test]
    fn the_prompt_does_not_offer_chrome_the_renderer_drops() {
        // A page tree's header is skipped so the page does not end up with
        // two; listing it as available would buy a wasted repair round.
        let prompt = system_prompt(&vyasa_themes::registry_schema());
        let regions = prompt
            .split("Regions a page may use:")
            .nth(1)
            .and_then(|s| s.split("Section kinds:").next())
            .expect("regions section");
        for chrome in THEME_CHROME {
            assert!(
                !regions.contains(&format!("- {chrome} ")),
                "offered {chrome}"
            );
        }
        assert!(regions.contains("- content "), "content must be offered");
    }

    #[test]
    fn the_prompt_still_teaches_the_content_section() {
        // The response-shape wording moved to the agent protocol and the
        // edit tool's schema; what must stay here is the composition rule
        // a model cannot guess — where the author's own writing goes.
        let prompt = system_prompt(&vyasa_themes::registry_schema());
        assert!(
            prompt.contains("`content` section"),
            "the model is not told how to place the author's writing"
        );
    }

    #[test]
    fn the_user_turn_shows_the_page_and_what_the_roles_resolve_to() {
        let tokens = TokenSet::default();
        let current = vec![Section::new("lead", "hero")];
        let prompt = user_prompt(
            &DesignInput {
                vocabulary: &Value::Null,
                site: "Vyasa · a CMS",
                brand: "",
                title: "Launch week",
                prose: "Everything we shipped.",
                current: &current,
                message: "add a call to action",
                model: "m",
            },
            &tokens,
        );
        assert!(prompt.contains("Launch week"));
        assert!(prompt.contains("Everything we shipped."));
        assert!(prompt.contains("add a call to action"));
        assert!(prompt.contains("\"lead\""), "current tree shown: {prompt}");
        // `$text` has to mean something concrete for contrast to be judged.
        assert!(prompt.contains(&tokens.colors.text.light.normalized()));
    }

    #[test]
    fn an_empty_page_says_so_rather_than_showing_nothing() {
        let prompt = user_prompt(
            &DesignInput {
                vocabulary: &Value::Null,
                site: "s",
                brand: "",
                title: "Untitled",
                prose: "   ",
                current: &[],
                message: "make a landing page",
                model: "m",
            },
            &TokenSet::default(),
        );
        assert!(prompt.contains("the author has not written the body yet"));
        assert!(prompt.contains("renders through the theme's page template"));
    }

    #[test]
    fn a_valid_tree_passes_the_check() {
        let out = check_builtin(
            &plan(&serde_json::json!([
                {"id": "lead", "kind": "hero", "settings": {"headline": "Hi"}},
                {"id": "words", "kind": "content"},
                {"id": "closing", "kind": "band", "children": [
                    {"id": "signup", "kind": "cta-band",
                     "settings": {"headline": "Ready?", "label": "Start", "url": "/go"},
                     "scope": {"bg": "$text", "text": "$bg"}}
                ]}
            ])),
            &TokenSet::default(),
        )
        .expect("valid");
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].children.len(), 1);
    }

    #[test]
    fn a_malformed_section_is_reported_by_index_not_as_a_parse_failure() {
        // The whole point of taking raw values: the model has to be told
        // *which* section it got wrong.
        let err = check_builtin(
            &plan(&serde_json::json!([
                {"id": "ok", "kind": "hero"},
                {"kind": "hero"}
            ])),
            &TokenSet::default(),
        )
        .expect_err("missing id");
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].path(), "sections[1]");
    }

    #[test]
    fn the_checks_that_matter_all_fire() {
        let cases: [(&str, Value, &str); 4] = [
            (
                "unknown kind",
                serde_json::json!([{"id": "x", "kind": "banner"}]),
                "sections[0].kind",
            ),
            (
                "children on a leaf",
                serde_json::json!([
                    {"id": "x", "kind": "hero", "children": [{"id": "y", "kind": "cta-band"}]}
                ]),
                "sections[0].children",
            ),
            (
                "duplicate ids",
                serde_json::json!([
                    {"id": "x", "kind": "hero"}, {"id": "x", "kind": "cta-band"}
                ]),
                "sections[1].id",
            ),
            (
                "a role that does not exist",
                serde_json::json!([{
                    "id": "x", "kind": "cta-band",
                    "settings": {"headline": "Ready?", "label": "Start", "url": "/go"},
                    "scope": {"bg": "$nope"}
                }]),
                "sections[0].scope.bg",
            ),
        ];
        for (name, tree, path) in cases {
            let err = check_builtin(&plan(&tree), &TokenSet::default()).expect_err(name);
            assert!(
                err.iter().any(|d| d.path() == path),
                "{name}: expected {path}, got {err:?}"
            );
        }
    }

    #[test]
    fn settings_are_checked_against_the_kinds_own_schema() {
        let mut bad = Section::new("cols", "columns");
        bad.settings = settings(&[("count", serde_json::json!(99))]);
        let err = check_builtin(
            &plan(&serde_json::json!([
                serde_json::to_value(&bad).expect("value")
            ])),
            &TokenSet::default(),
        )
        .expect_err("99 columns");
        assert!(
            err.iter().any(|d| d.path() == "sections[0].settings"),
            "{err:?}"
        );
    }
}
