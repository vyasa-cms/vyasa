//! Sandboxed Tera template engine.
//!
//! Templates are data: only strings registered through this module ever
//! compile, and every theme-supplied source passes a construct scan before
//! registration (`include`/`extends` targets must resolve to templates in
//! the same package; environment access and dunder probes are rejected).
//! Autoescape stays on for all HTML templates; a custom `t()` filter stub
//! provides the i18n seam.

use std::collections::HashMap;

use tera::Tera;

/// Engine setup or template-compilation failure.
///
/// Messages are written for theme authors at install time: they name the
/// template and the offending construct.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct EngineError(pub String);

/// English strings for the keys the bundled templates use.
///
/// There is no locale negotiation yet, so this is deliberately a single
/// table rather than a catalogue format: it exists so that `{{ "k"|t }}`
/// renders prose instead of leaking the key to visitors, which is what the
/// previous identity stub did — the 404 page rendered a link reading
/// literally "back_home".
const STRINGS: [(&str, &str); 2] = [
    ("back_home", "Back to home"),
    ("search_results", "Search results"),
];

/// Turns an unknown key into something presentable: `no_comments_yet`
/// becomes `No comments yet`.
///
/// A theme may use a key this table has never heard of, and showing a
/// visitor raw snake_case is worse than showing humanised text.
fn humanise(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for (i, ch) in key.replace(['_', '-'], " ").chars().enumerate() {
        if i == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

// The Result shape is dictated by Tera's filter signature.
#[allow(clippy::unnecessary_wraps)]
fn translate(
    value: &tera::Value,
    _args: &HashMap<String, tera::Value>,
) -> tera::Result<tera::Value> {
    // Non-strings pass through untouched; `|t` on a number is a template
    // bug, but breaking the render over it helps nobody.
    let Some(key) = value.as_str() else {
        return Ok(value.clone());
    };
    let text = STRINGS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or_else(|| humanise(key), |(_, v)| (*v).to_owned());
    Ok(tera::Value::String(text))
}

fn register_filters(tera: &mut Tera) {
    tera.register_filter("t", translate);
}

/// Flattens a Tera error's cause chain into one readable line.
fn format_causes(e: &tera::Error) -> String {
    use std::fmt::Write as _;
    let mut msg = e.to_string();
    let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e);
    while let Some(s) = src {
        let _ = write!(msg, "; caused by: {s}");
        src = s.source();
    }
    msg
}

/// Constructs that must never appear in theme-supplied templates.
// Bare "__" cannot be banned outright: BEM class names like
// `blog-single__header` are legitimate markup.
const FORBIDDEN_SUBSTRINGS: [&str; 10] = [
    "get_env",
    "__tera_context",
    "self.__",
    "__class__",
    "__init__",
    "__globals__",
    ".constructor",
    "eval(",
    "exec(",
    "{% import",
];

/// Scans a template source for sandbox escapes.
fn scan_source(name: &str, src: &str) -> Result<(), EngineError> {
    for bad in FORBIDDEN_SUBSTRINGS {
        if src.contains(bad) {
            return Err(EngineError(format!(
                "template \"{name}\": forbidden construct {bad:?} — templates cannot access \
                 the environment or engine internals"
            )));
        }
    }
    Ok(())
}

/// Collects `include`/`extends` target names from a template body.
fn referenced_templates(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tag in ["include", "extends"] {
        let needle = format!("{tag} \"");
        let mut rest = src;
        while let Some(pos) = rest.find(&needle) {
            let after = &rest[pos + needle.len()..];
            if let Some(end) = after.find('"') {
                out.push(after[..end].to_owned());
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    out
}

/// Every template a source names with `include`, `extends` or `import`,
/// in double or single quotes — what a scan of the values a page reads
/// must follow, macros included.
fn every_referenced_template(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tag in ["include", "extends", "import"] {
        for quote in ['"', '\''] {
            let needle = format!("{tag} {quote}");
            let mut rest = src;
            while let Some(pos) = rest.find(&needle) {
                let after = &rest[pos + needle.len()..];
                let Some(end) = after.find(quote) else { break };
                out.push(after[..end].to_owned());
                rest = &after[end + 1..];
            }
        }
    }
    out
}

/// A compiled set of templates: the built-in page set plus any installed
/// theme overrides.
pub struct Engine {
    tera: Tera,
    /// Registered sources by name, so the renderer can ask which region
    /// ids a template (and its parents) actually read.
    sources: HashMap<String, String>,
}

impl Engine {
    /// Creates an engine with only the built-in template set.
    ///
    /// # Errors
    /// Returns [`EngineError`] if a built-in template fails to compile
    /// (a release-blocking bug, never user input).
    pub fn builtin() -> Result<Self, EngineError> {
        let mut tera = Tera::default();
        tera.autoescape_suffixes = vec!["html"];
        for (name, src) in builtin_templates() {
            tera.add_raw_template(name, src)
                .map_err(|e| EngineError(format!("builtin template {name}: {e}")))?;
        }
        register_filters(&mut tera);
        let sources = builtin_templates()
            .into_iter()
            .map(|(n, src)| (n.to_owned(), src.to_owned()))
            .collect();
        Ok(Self { tera, sources })
    }

    /// Installs theme-supplied templates, overriding built-ins by name.
    ///
    /// Validation happens **here**, once at install time — rendering
    /// assumes everything registered is trusted.
    ///
    /// # Errors
    /// Returns [`EngineError`] when a source contains forbidden constructs,
    /// references templates outside the package, or fails to compile.
    pub fn install_theme_templates(
        &mut self,
        templates: &[(&str, &str)],
    ) -> Result<(), EngineError> {
        // Validate all sources first so installation is atomic.
        for (name, src) in templates {
            scan_source(name, src)?;
            for reference in referenced_templates(src) {
                let known = self.tera.templates.contains_key(&reference)
                    || templates.iter().any(|(n, _)| *n == reference)
                    || reference.starts_with("partials/");
                if !known {
                    return Err(EngineError(format!(
                        "template \"{name}\": references unknown template \"{reference}\" \
                         (includes/extends are limited to this theme's files)"
                    )));
                }
            }
        }
        for (name, src) in templates {
            self.tera
                .add_raw_template(name, src)
                .map_err(|e| EngineError(format!("template \"{name}\": {e}")))?;
            self.sources.insert((*name).to_owned(), (*src).to_owned());
        }
        Ok(())
    }

    /// Whether this template — or anything it extends or includes —
    /// prints the composed `sections` body.
    ///
    /// The loose-block rescue in `finish_regions` exists for templates
    /// that place regions by hand; a template that defers to the composed
    /// tree has already placed everything, and appending the same blocks
    /// again doubled the page.
    #[must_use]
    pub fn prints_sections(&self, name: &str) -> bool {
        let mut queue = vec![name.to_owned()];
        let mut seen = std::collections::HashSet::new();
        while let Some(current) = queue.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let Some(src) = self.sources.get(&current) else {
                continue;
            };
            if src.contains("{{ sections")
                || src.contains("{{- sections")
                || src.contains("{% if sections")
                || src.contains("{%- if sections")
            {
                return true;
            }
            for parent in referenced_templates(src) {
                queue.push(parent);
            }
        }
        false
    }

    /// The region ids a template reads, following `extends`.
    ///
    /// A layout block whose HTML is already placed by the template must not
    /// also be appended as a loose block, and only the template itself
    /// knows which slots it addresses.
    #[must_use]
    pub fn region_ids(&self, name: &str) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        let mut queue = vec![name.to_owned()];
        let mut seen = std::collections::HashSet::new();
        while let Some(current) = queue.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let Some(src) = self.sources.get(&current) else {
                continue;
            };
            collect_region_ids(src, &mut out);
            for parent in referenced_templates(src) {
                queue.push(parent);
            }
        }
        out
    }
}

impl Engine {
    /// The custom-field keys a template (and anything it extends or
    /// includes) reads as `….fields.<key>`, each with the sub-keys it reads
    /// off that value (`fields.cover.url` → `cover: {url}`).
    ///
    /// The renderer fills every one of them with an empty value when the
    /// entry has none, so a template that names a field which no longer
    /// exists — or was never filled in — renders nothing rather than
    /// failing the page on an undefined variable.
    #[must_use]
    pub fn field_refs(
        &self,
        name: &str,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut out = std::collections::BTreeMap::new();
        let mut queue = vec![name.to_owned()];
        let mut seen = std::collections::HashSet::new();
        while let Some(current) = queue.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let Some(src) = self.sources.get(&current) else {
                continue;
            };
            collect_field_refs(src, &mut out);
            for parent in every_referenced_template(src) {
                queue.push(parent);
            }
        }
        out
    }
}

/// An identifier at the start of `s`: ASCII letters, digits, underscores.
fn ident_at(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

/// Collects `fields.<key>` and `fields.<key>.<sub>` references.
fn collect_field_refs(
    src: &str,
    out: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) {
    let needle = "fields.";
    let mut rest = src;
    while let Some(pos) = rest.find(needle) {
        // `fields` must be a whole word (`entry.fields.x`, `p.fields.x`),
        // not the tail of `myfields.x`.
        let whole = rest[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let after = &rest[pos + needle.len()..];
        rest = after;
        if !whole {
            continue;
        }
        let key = ident_at(after);
        if key.is_empty() {
            continue;
        }
        let subs = out.entry(key.to_owned()).or_default();
        if let Some(tail) = after[key.len()..].strip_prefix('.') {
            let sub = ident_at(tail);
            if !sub.is_empty() {
                subs.insert(sub.to_owned());
            }
        }
    }
}

/// Collects `regions.foo` and `regions["foo"]` identifiers from a source.
fn collect_region_ids(src: &str, out: &mut std::collections::HashSet<String>) {
    let needle = "regions";
    let mut rest = src;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        rest = after;
        let mut chars = after.chars();
        match chars.next() {
            Some('.') => {
                let ident: String = after[1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !ident.is_empty() {
                    out.insert(ident);
                }
            }
            // `regions["with-dashes"]`
            Some('[') => {
                if let Some(open) = after.find('"') {
                    if let Some(close) = after[open + 1..].find('"') {
                        out.insert(after[open + 1..open + 1 + close].to_owned());
                    }
                }
            }
            _ => {}
        }
    }
}

impl Engine {
    /// Validates a theme-supplied source against this engine's sandbox
    /// rules **without** installing it (used by the package parser).
    ///
    /// # Errors
    /// Same as [`Self::install_theme_templates`] but nothing is mutated.
    pub fn validate_theme_source(&self, name: &str, src: &str) -> Result<(), EngineError> {
        scan_source(name, src)?;
        for reference in referenced_templates(src) {
            let known =
                self.tera.templates.contains_key(&reference) || reference.starts_with("partials/");
            if !known {
                return Err(EngineError(format!(
                    "template \"{name}\": references unknown template \"{reference}\" \
                     (includes/extends are limited to this theme's files)"
                )));
            }
        }
        // Compile against the built-in set to catch syntax errors and
        // missing parents without registering anything.
        let mut probe = probe_tera();
        probe
            .add_raw_template(name, src)
            .map_err(|e| EngineError(format!("template \"{name}\": {e}")))?;
        Ok(())
    }

    /// Renders a registered template with a JSON context.
    ///
    /// # Errors
    /// Returns [`EngineError`] (install-time-friendly message) when the
    /// template name is unknown or rendering fails.
    pub fn render_json(&self, name: &str, ctx: &serde_json::Value) -> Result<String, EngineError> {
        let ctx = tera::Context::from_serialize(ctx)
            .map_err(|e| EngineError(format!("context for {name}: {e}")))?;
        self.tera
            .render(name, &ctx)
            .map_err(|e| EngineError(format!("rendering {name}: {}", format_causes(&e))))
    }

    /// Names of all registered templates.
    #[must_use]
    pub fn template_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tera.templates.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

const BASE: &str = include_str!("../templates/base.html");
const INDEX: &str = include_str!("../templates/index.html");
const SINGLE: &str = include_str!("../templates/single.html");
const ARCHIVE: &str = include_str!("../templates/archive.html");
const PAGE: &str = include_str!("../templates/page.html");
const SEARCH: &str = include_str!("../templates/search.html");
const NOT_FOUND: &str = include_str!("../templates/not-found.html");

/// A throwaway engine preloaded with only the built-in templates.
fn probe_tera() -> Tera {
    let mut tera = Tera::default();
    for (name, src) in builtin_templates() {
        // Built-ins are compile-checked in `Engine::builtin` too; a failure
        // here is a release-blocking bug, not user input.
        if let Err(e) = tera.add_raw_template(name, src) {
            unreachable!("builtin template {name} failed to compile: {e}");
        }
    }
    tera
}

/// The built-in template sources by engine name, so editors can show an
/// author what they are overriding.
#[must_use]
pub fn builtin_sources() -> [(&'static str, &'static str); 7] {
    builtin_templates()
}

fn builtin_templates() -> [(&'static str, &'static str); 7] {
    [
        ("base.html", BASE),
        ("index.html", INDEX),
        ("single.html", SINGLE),
        ("archive.html", ARCHIVE),
        ("page.html", PAGE),
        ("search.html", SEARCH),
        ("not-found.html", NOT_FOUND),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str) -> String {
        let args = HashMap::new();
        translate(&tera::Value::String(key.to_owned()), &args)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn known_keys_render_prose() {
        assert_eq!(t("back_home"), "Back to home");
        assert_eq!(t("search_results"), "Search results");
    }

    /// The bug this filter was written for: the 404 page rendered a link
    /// whose text was the literal key.
    #[test]
    fn no_key_ever_reaches_the_page_verbatim() {
        for (key, _) in STRINGS {
            assert_ne!(t(key), key, "{key} rendered as its own key");
        }
    }

    #[test]
    fn unknown_keys_are_humanised() {
        assert_eq!(t("no_comments_yet"), "No comments yet");
        assert_eq!(t("read-more"), "Read more");
    }

    #[test]
    fn non_strings_pass_through() {
        let args = HashMap::new();
        let n = tera::Value::from(7);
        assert_eq!(translate(&n, &args).unwrap(), n);
    }

    /// Every `|t` key in a bundled template must be in the table, or the
    /// humanised fallback is silently shipping to visitors.
    #[test]
    fn bundled_templates_only_use_known_keys() {
        for (name, src) in builtin_templates() {
            let mut rest = src;
            while let Some(i) = rest.find("|t }}") {
                let before = &rest[..i];
                let key = before
                    .rsplit('"')
                    .nth(1)
                    .expect("malformed translation call");
                assert!(
                    STRINGS.iter().any(|(k, _)| *k == key),
                    "template {name} uses untranslated key {key:?}"
                );
                rest = &rest[i + 5..];
            }
        }
    }
}

#[cfg(test)]
impl Engine {
    /// An engine over raw sources, with none of the theme sandbox checks —
    /// for tests of constructs a theme may not use (`import`).
    pub(crate) fn from_raw_for_test(templates: &[(&str, &str)]) -> Self {
        let mut tera = Tera::default();
        tera.autoescape_suffixes = vec!["html"];
        tera.add_raw_templates(templates.iter().copied())
            .expect("test templates compile");
        register_filters(&mut tera);
        let sources = templates
            .iter()
            .map(|(n, s)| ((*n).to_owned(), (*s).to_owned()))
            .collect();
        Self { tera, sources }
    }
}

#[cfg(test)]
mod field_ref_tests {
    use super::*;

    #[test]
    fn field_references_are_followed_through_imports_and_single_quotes() {
        let engine = Engine::from_raw_for_test(&[
            (
                "m.html",
                "{% macro show(e) %}[{{ e.fields.gone }}]{% endmacro %}",
            ),
            ("part.html", "{{ entry.fields.partial }}"),
            (
                "t.html",
                "{% import 'm.html' as m %}{{ m::show(e=entry) }}{% include 'part.html' %}",
            ),
        ]);
        let refs = engine.field_refs("t.html");
        assert!(refs.contains_key("gone"), "{refs:?}");
        assert!(refs.contains_key("partial"), "{refs:?}");
    }

    #[test]
    fn field_references_are_found_with_their_sub_keys() {
        let mut out = std::collections::BTreeMap::new();
        collect_field_refs(
            "{{ entry.fields.price }} {{ entry.fields.cover.url }} \
             {% for p in posts %}{{ p.fields.colour }}{% endfor %} {{ myfields.nope }} \
             {{ entry.fields.cover.alt }}",
            &mut out,
        );
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(out["price"].is_empty());
        assert_eq!(
            out["cover"].iter().map(String::as_str).collect::<Vec<_>>(),
            ["alt", "url"]
        );
        assert!(out.contains_key("colour"));
    }
}
