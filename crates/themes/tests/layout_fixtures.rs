//! Layout schema fixtures, child-theme merge semantics, canonical snapshot.

#![allow(clippy::pedantic)]

use std::sync::Arc;

use vyasa_themes::{
    builtin_registry, canonical_layout_json, merge_layouts, validate_layout, Layout, MapSettings,
    Section, TemplateType,
};

fn block(id: &str, kind: &str) -> Section {
    Section {
        id: id.to_owned(),
        kind: kind.to_owned(),
        settings: MapSettings::default(),
        children: Vec::new(),
        scope: None,
        hide_on: None,
        width: None,
    }
}

fn parent_layout() -> Layout {
    Layout {
        index: vec![
            block("header", "header"),
            block("nav", "nav"),
            block("hero", "latest-posts"),
            block("main", "content"),
            block("aside", "sidebar-right"),
            block("footer", "footer"),
        ],
        single: vec![
            block("header", "header"),
            block("trail", "breadcrumbs"),
            block("body", "post-content"),
            block("comments", "comments"),
        ],
        ..Layout::default()
    }
}

fn fill_all_templates(layout: &mut Layout) {
    for t in TemplateType::ALL {
        if layout.for_template(t).is_empty() {
            let v = match t {
                TemplateType::NotFound => vec![block("nf", "content")],
                _ => vec![block("main", "content")],
            };
            *match t {
                TemplateType::Index => &mut layout.index,
                TemplateType::Single => &mut layout.single,
                TemplateType::Archive => &mut layout.archive,
                TemplateType::Page => &mut layout.page,
                TemplateType::Search => &mut layout.search,
                TemplateType::NotFound => &mut layout.not_found,
            } = v;
        }
    }
}

#[test]
fn complete_layout_with_settings_validates() {
    let mut layout = parent_layout();
    layout.index[2].settings.set("count", serde_json::json!(8));
    fill_all_templates(&mut layout);
    let diags = validate_layout(&layout, &builtin_registry());
    assert!(diags.is_empty(), "expected clean validation, got {diags:?}");
}

#[test]
fn invalid_layouts_are_rejected() {
    let reg = builtin_registry();
    // Unknown dynamic kind.
    let mut l = parent_layout();
    l.index[2].kind = "no-such-block".into();
    fill_all_templates(&mut l);
    assert!(validate_layout(&l, &reg)
        .iter()
        .any(|d| d.path().contains(".kind")));

    // Bad setting value (count = 500).
    let mut l = parent_layout();
    l.index[2].settings.set("count", serde_json::json!(500));
    fill_all_templates(&mut l);
    assert!(validate_layout(&l, &reg)
        .iter()
        .any(|d| d.path().contains(".settings")));

    // Wrong-typed setting.
    let mut l = parent_layout();
    l.index[2].settings.set("count", serde_json::json!("many"));
    fill_all_templates(&mut l);
    assert!(validate_layout(&l, &reg)
        .iter()
        .any(|d| d.path().contains(".settings")));

    // Duplicate ids within one template.
    let mut l = parent_layout();
    l.single.push(block("body", "post-content"));
    fill_all_templates(&mut l);
    assert!(validate_layout(&l, &reg)
        .iter()
        .any(|d| d.to_string().contains("duplicate section id")));

    // Uppercase / space in id.
    let mut l = parent_layout();
    l.index[0].id = "Bad Id".into();
    fill_all_templates(&mut l);
    assert!(validate_layout(&l, &reg)
        .iter()
        .any(|d| d.path().contains(".id")));

    // Empty template list.
    let l = Layout::default();
    assert!(validate_layout(&l, &reg)
        .iter()
        .any(|d| d.path() == "index" && d.to_string().contains("no sections")));
}

#[test]
fn merge_overrides_template_and_extends_by_id() {
    let mut parent = parent_layout();
    fill_all_templates(&mut parent);
    parent.index[2].settings.set("count", serde_json::json!(5));

    let child = Layout {
        // Override `index`: extend hero settings by id + append a tag cloud.
        index: vec![
            Section {
                id: "hero".into(),
                kind: "latest-posts".into(),
                settings: {
                    let mut s = MapSettings::default();
                    s.set("count", serde_json::json!(12));
                    s.set("category", serde_json::json!("news"));
                    s
                },
                children: Vec::new(),
                scope: None,
                hide_on: None,
                width: None,
            },
            block("tags", "tag-cloud"),
        ],
        // Empty template lists inherit from the parent untouched.
        ..Layout::default()
    };

    let merged = merge_layouts(&parent, &child);
    assert_eq!(
        merged.single.len(),
        parent.single.len(),
        "untouched template inherits"
    );
    assert_eq!(merged.index.len(), 7, "parent 6 blocks + 1 appended");
    let hero = merged.index.iter().find(|b| b.id == "hero").expect("kept");
    assert_eq!(
        hero.settings.get("count"),
        Some(&serde_json::json!(12)),
        "child wins per key"
    );
    assert_eq!(
        hero.settings.get("category"),
        Some(&serde_json::json!("news"))
    );
    assert_eq!(
        merged.index.last().map(|b| b.id.as_str()),
        Some("tags"),
        "new id appended last"
    );
    assert!(validate_layout(&merged, &builtin_registry()).is_empty());
}

#[test]
fn custom_registry_kind_is_accepted_and_validated() {
    struct Always;
    impl vyasa_themes::DynamicBlock for Always {
        fn kind(&self) -> &'static str {
            "custom-widget"
        }
        fn description(&self) -> &'static str {
            "test"
        }
    }
    let mut reg = builtin_registry();
    reg.register(Arc::new(Always));
    let mut l = parent_layout();
    l.page.push(block("widget", "custom-widget"));
    fill_all_templates(&mut l);
    assert!(validate_layout(&l, &reg).is_empty());
    // …but unknown kinds still fail with that same registry.
    let mut l2 = l.clone();
    l2.page.last_mut().expect("widget present").kind = "still-unknown".into();
    assert!(validate_layout(&l2, &reg)
        .iter()
        .any(|d| d.path().contains(".kind")));
}

#[test]
fn snapshot_canonical_layout() {
    let mut layout = parent_layout();
    fill_all_templates(&mut layout);
    layout.search[0]
        .settings
        .set("placeholder", serde_json::json!("Search…"));
    layout.not_found[0]
        .settings
        .set("zzz", serde_json::json!(true));
    insta::assert_snapshot!("canonical_layout", canonical_layout_json(&layout));
}
