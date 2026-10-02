//! The layout schema changed shape in place rather than behind a version
//! gate, so the one thing that must never regress is this: a layout written
//! by the previous schema still loads, still validates, and still renders the
//! way it did before.
//!
//! The fixture is the exact JSON the live site had stored when the change
//! landed.

use vyasa_themes::{builtin_registry, composes, validate_layout, Layout, TemplateType};

const STORED: &str = include_str!("fixtures/live_layout.json");

#[test]
fn a_layout_from_the_old_schema_still_loads() {
    let layout: Layout = serde_json::from_str(STORED).expect("old layout must deserialize");
    let diags = validate_layout(&layout, &builtin_registry());
    let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn an_old_layout_keeps_the_slot_rendering_path() {
    // Nothing in a pre-existing theme nests or uses a marketing section, so
    // it must not be routed through the new composing renderer — that is what
    // keeps the live site rendering byte-for-byte as before.
    let layout: Layout = serde_json::from_str(STORED).expect("deserialize");
    for t in TemplateType::ALL {
        assert!(
            !composes(layout.for_template(t)),
            "{} unexpectedly composes",
            t.as_str()
        );
    }
}

#[test]
fn sections_default_to_no_children_and_no_scope() {
    let layout: Layout = serde_json::from_str(STORED).expect("deserialize");
    for section in layout.for_template(TemplateType::Index) {
        assert!(section.children.is_empty());
        assert!(section.scope.is_none());
    }
}
