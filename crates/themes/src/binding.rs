//! The `bind` contract: how a repeating section names its data source.
//!
//! A binding is settings, not a new tree shape — `Section` did not change.
//! A section whose settings carry a `bind` object draws its entries from a
//! post type at render time instead of from hand-entered items, which is
//! what makes a products grid or a forum topic list an ordinary section:
//! the plugin registers the *type*, and any binding kind can show it.
//!
//! Validation here is shape only. Whether the named source actually exists
//! is a live question the themes crate cannot answer (types come and go
//! with plugins), so an unknown source renders as an empty section — the
//! same survival rule a disabled plugin's post type follows — and the
//! places that do know the live list (the studio, the assistants) warn
//! instead of break.

use crate::dynblocks::EntrySort;
use crate::layout::MapSettings;

/// Most entries one bound section may pull.
pub const MAX_BOUND_ENTRIES: u32 = 24;

/// Entries a binding shows when it does not say.
pub const DEFAULT_BOUND_ENTRIES: u32 = 6;

/// Most `where` conditions one binding may carry.
pub const MAX_BOUND_FILTERS: usize = 4;

/// Longest string a `where` condition may compare against.
const MAX_FILTER_TEXT: usize = 200;

/// Ordering by a custom field's value: `"sort": "field:price"` (lowest
/// first) or `"field:price:desc"`. Entries without a value come last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldOrder {
    /// The field key.
    pub key: String,
    /// Highest first.
    pub descending: bool,
}

/// A parsed `bind` settings object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Post type slug: `post`, `page`, or a registered type.
    pub source: String,
    /// Term filter: `"news"` (a category) or `"shelf:fiction"`.
    pub term: Option<String>,
    /// Order of entries (ignored when `field_sort` is set).
    pub sort: EntrySort,
    /// Order by a custom field instead.
    pub field_sort: Option<FieldOrder>,
    /// `where`: entries whose field equals the value (or, for a field
    /// holding several choices, includes it), all conditions together.
    /// A field that no longer exists matches nothing; it never errors.
    pub filters: Vec<(String, serde_json::Value)>,
    /// How many entries, clamped to [`MAX_BOUND_ENTRIES`].
    pub limit: u32,
}

/// The grammar of a field key: `^[a-z][a-z0-9_]{0,39}$`.
fn is_field_key(v: &str) -> bool {
    (1..=40).contains(&v.len())
        && v.starts_with(|c: char| c.is_ascii_lowercase())
        && v.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Reads `field:<key>` or `field:<key>:desc`.
fn field_order(s: &str) -> Option<FieldOrder> {
    let rest = s.strip_prefix("field:")?;
    let (key, descending) = match rest.strip_suffix(":desc") {
        Some(key) => (key, true),
        None => (rest.strip_suffix(":asc").unwrap_or(rest), false),
    };
    is_field_key(key).then(|| FieldOrder {
        key: key.to_owned(),
        descending,
    })
}

/// Reads the `where` object.
fn filters(raw: &serde_json::Value, errors: &mut Vec<String>) -> Vec<(String, serde_json::Value)> {
    let Some(obj) = raw.as_object() else {
        errors.push("bind.where must be an object of field: value".to_owned());
        return Vec::new();
    };
    if obj.len() > MAX_BOUND_FILTERS {
        errors.push(format!(
            "bind.where takes at most {MAX_BOUND_FILTERS} conditions"
        ));
        return Vec::new();
    }
    let mut out = Vec::new();
    for (key, value) in obj {
        if !is_field_key(key) {
            errors.push(format!("bind.where.{key} is not a field key"));
            continue;
        }
        let ok = match value {
            serde_json::Value::String(s) => s.chars().count() <= MAX_FILTER_TEXT,
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) => true,
            _ => false,
        };
        if ok {
            out.push((key.clone(), value.clone()));
        } else {
            errors.push(format!(
                "bind.where.{key} must be text (at most {MAX_FILTER_TEXT} characters), a number \
                 or true/false"
            ));
        }
    }
    out
}

/// The grammar post types and term slugs share — the same one the
/// `posts.type` column enforces.
fn is_slug(v: &str) -> bool {
    v.len() >= 2
        && v.len() <= 32
        && v.starts_with(|c: char| c.is_ascii_lowercase())
        && v.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Binding {
    /// Reads the `bind` object out of a section's settings.
    ///
    /// `Ok(None)` when there is no `bind` key at all; `Err` lists every
    /// problem with one that is present, phrased for the editor.
    ///
    /// # Errors
    /// One message per malformed field.
    pub fn from_settings(settings: &MapSettings) -> Result<Option<Self>, Vec<String>> {
        let Some(raw) = settings.get("bind") else {
            return Ok(None);
        };
        let Some(obj) = raw.as_object() else {
            return Err(vec!["bind must be an object".to_owned()]);
        };
        let mut errors = Vec::new();

        let source = match obj.get("source").and_then(serde_json::Value::as_str) {
            Some(s) if is_slug(s) => s.to_owned(),
            Some(s) => {
                errors.push(format!(
                    "bind.source {s:?} is not a post type name (lowercase letters, digits, hyphens)"
                ));
                String::new()
            }
            None => {
                errors.push("bind.source is required: the post type to draw from".to_owned());
                String::new()
            }
        };

        let term = match obj.get("term") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(t)) => {
                let ok = match t.split_once(':') {
                    Some((tax, slug)) => is_slug(tax) && is_slug(slug),
                    None => is_slug(t),
                };
                if ok {
                    Some(t.clone())
                } else {
                    errors.push(format!(
                        "bind.term {t:?} is not a term slug (or taxonomy:slug)"
                    ));
                    None
                }
            }
            Some(_) => {
                errors.push("bind.term must be a string".to_owned());
                None
            }
        };

        let mut field_sort = None;
        let sort = match obj.get("sort") {
            None | Some(serde_json::Value::Null) => EntrySort::default(),
            Some(serde_json::Value::String(s)) => {
                if let Some(sort) = EntrySort::parse(s) {
                    sort
                } else if let Some(order) = field_order(s) {
                    field_sort = Some(order);
                    EntrySort::default()
                } else {
                    errors.push(format!(
                        "bind.sort {s:?} is not one of: {}, or field:<key> (field:<key>:desc)",
                        EntrySort::NAMES.join(", ")
                    ));
                    EntrySort::default()
                }
            }
            Some(_) => {
                errors.push("bind.sort must be a string".to_owned());
                EntrySort::default()
            }
        };

        let limit = match obj.get("limit") {
            None | Some(serde_json::Value::Null) => DEFAULT_BOUND_ENTRIES,
            Some(v) => match v.as_u64() {
                Some(n) if (1..=u64::from(MAX_BOUND_ENTRIES)).contains(&n) => {
                    u32::try_from(n).unwrap_or(DEFAULT_BOUND_ENTRIES)
                }
                _ => {
                    errors.push(format!("bind.limit must be 1..={MAX_BOUND_ENTRIES}"));
                    DEFAULT_BOUND_ENTRIES
                }
            },
        };

        let filters = match obj.get("where") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(raw) => filters(raw, &mut errors),
        };

        for key in obj.keys() {
            if !matches!(key.as_str(), "source" | "term" | "sort" | "limit" | "where") {
                errors.push(format!("bind.{key} is not a binding field"));
            }
        }

        if errors.is_empty() {
            Ok(Some(Self {
                source,
                term,
                sort,
                field_sort,
                filters,
                limit,
            }))
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(bind: serde_json::Value) -> MapSettings {
        let mut s = MapSettings::default();
        s.set("bind".to_owned(), bind);
        s
    }

    #[test]
    fn absent_means_unbound() {
        assert_eq!(Binding::from_settings(&MapSettings::default()), Ok(None));
    }

    #[test]
    fn a_full_binding_parses() {
        let b = Binding::from_settings(&settings(serde_json::json!({
            "source": "product", "term": "shelf:fiction", "sort": "title", "limit": 12
        })))
        .expect("valid")
        .expect("present");
        assert_eq!(b.source, "product");
        assert_eq!(b.term.as_deref(), Some("shelf:fiction"));
        assert_eq!(b.sort, EntrySort::Title);
        assert_eq!(b.limit, 12);
    }

    #[test]
    fn defaults_fill_what_is_not_said() {
        let b = Binding::from_settings(&settings(serde_json::json!({"source": "post"})))
            .expect("valid")
            .expect("present");
        assert_eq!(b.sort, EntrySort::Newest);
        assert_eq!(b.limit, DEFAULT_BOUND_ENTRIES);
        assert_eq!(b.term, None);
    }

    #[test]
    fn a_binding_may_sort_and_filter_by_a_field() {
        let b = Binding::from_settings(&settings(serde_json::json!({
            "source": "product", "sort": "field:price:desc",
            "where": {"colour": "red", "in_stock": true, "size": 3}
        })))
        .expect("valid")
        .expect("present");
        assert_eq!(
            b.field_sort,
            Some(FieldOrder {
                key: "price".into(),
                descending: true
            })
        );
        assert_eq!(b.filters.len(), 3);
        let asc = Binding::from_settings(&settings(serde_json::json!({
            "source": "product", "sort": "field:price"
        })))
        .expect("valid")
        .expect("present");
        assert_eq!(asc.field_sort.map(|o| o.descending), Some(false));

        let errs = Binding::from_settings(&settings(serde_json::json!({
            "source": "product", "sort": "field:Price",
            "where": {"Bad-Key": 1, "ok": {"nested": 1}}
        })))
        .expect_err("invalid");
        assert_eq!(errs.len(), 3, "{errs:?}");
    }

    #[test]
    fn every_problem_is_named_at_once() {
        let errs = Binding::from_settings(&settings(serde_json::json!({
            "source": "Not A Type", "sort": "sideways", "limit": 500, "extra": 1
        })))
        .expect_err("invalid");
        assert_eq!(errs.len(), 4, "{errs:?}");
    }
}
