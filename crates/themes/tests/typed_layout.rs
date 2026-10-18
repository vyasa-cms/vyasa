//! A theme may carry a section tree per custom post type.
#![allow(clippy::expect_used)]

use vyasa_themes::{
    builtin_registry, canonical_layout_json, merge_layouts, validate_layout, Layout,
};

const BASE: &str = r#""index":[{"id":"posts","kind":"latest-posts"}],
"single":[{"id":"body","kind":"post-content"}],
"archive":[{"id":"list","kind":"latest-posts"}],
"page":[{"id":"body","kind":"post-content"}],
"search":[{"id":"find","kind":"search-box"}],
"not-found":[{"id":"nf","kind":"content"}]"#;

fn layout(extra: &str) -> Layout {
    serde_json::from_str(&format!("{{{BASE}{extra}}}")).expect("parses")
}

const DOC: &str = r#","types":{"doc":[
  {"id":"cols","kind":"columns","settings":{"count":2},"children":[
    {"id":"side","kind":"group","children":[{"id":"nav","kind":"docs/sidebar"}]},
    {"id":"main","kind":"group","children":[{"id":"body2","kind":"post-content"}]}]}]}"#;

fn errors(l: &Layout) -> Vec<String> {
    validate_layout(l, &builtin_registry())
        .into_iter()
        .filter(vyasa_themes::TokenDiagnostic::is_error)
        .map(|d| format!("{d:?}"))
        .collect()
}

#[test]
fn a_theme_without_types_is_unchanged() {
    let l = layout("");
    assert!(l.types.is_empty());
    assert!(l.type_sections("doc").is_none());
    assert!(errors(&l).is_empty());
    assert!(
        !serde_json::to_string(&l)
            .expect("serialises")
            .contains("types"),
        "no key when empty"
    );
}

#[test]
fn a_typed_tree_parses_validates_and_round_trips() {
    let l = layout(DOC);
    let tree = l.type_sections("doc").expect("present");
    assert_eq!(tree[0].kind, "columns");
    // `docs/sidebar` is a plugin kind nobody registered here: a warning, not an error.
    assert!(errors(&l).is_empty(), "{:?}", errors(&l));
    let again: Layout =
        serde_json::from_str(&serde_json::to_string(&l).expect("serialises")).expect("parses");
    assert_eq!(again, l);
    assert!(canonical_layout_json(&l).contains("\"types\""));
}

#[test]
fn a_bad_key_or_an_empty_tree_is_an_error_naming_the_type() {
    let bad_key = layout(r#","types":{"Not A Slug":[{"id":"b","kind":"post-content"}]}"#);
    assert!(
        errors(&bad_key)
            .iter()
            .any(|e| e.contains("types.Not A Slug")),
        "{:?}",
        errors(&bad_key)
    );
    let empty = layout(r#","types":{"doc":[]}"#);
    assert!(
        errors(&empty)
            .iter()
            .any(|e| e.contains("types.doc") && e.contains("no sections")),
        "{:?}",
        errors(&empty)
    );
    assert!(
        empty.type_sections("doc").is_none(),
        "an empty tree is not a tree"
    );
}

#[test]
fn a_typed_tree_is_validated_like_any_other() {
    let dup = layout(
        r#","types":{"doc":[{"id":"a","kind":"post-content"},{"id":"a","kind":"post-content"}]}"#,
    );
    assert!(
        errors(&dup)
            .iter()
            .any(|e| e.contains("duplicate section id")),
        "{:?}",
        errors(&dup)
    );
    let unknown = layout(r#","types":{"doc":[{"id":"a","kind":"no-such-kind"}]}"#);
    assert!(
        errors(&unknown).iter().any(|e| e.contains("types.doc")),
        "{:?}",
        errors(&unknown)
    );
}

#[test]
fn unknown_top_level_keys_are_still_refused() {
    let r: Result<Layout, _> = serde_json::from_str(&format!("{{{BASE},\"typos\":{{}}}}"));
    assert!(r.is_err());
}

#[test]
fn a_child_theme_overrides_a_type_wholesale_and_inherits_the_rest() {
    let parent = layout(&DOC.replace(
        "\"types\":{",
        "\"types\":{\"note\":[{\"id\":\"n\",\"kind\":\"post-content\"}],",
    ));
    let child = layout(r#","types":{"doc":[{"id":"only","kind":"post-content"}]}"#);
    let merged = merge_layouts(&parent, &child);
    assert_eq!(merged.type_sections("doc").expect("doc")[0].id, "only");
    assert_eq!(merged.type_sections("note").expect("note")[0].id, "n");
}
