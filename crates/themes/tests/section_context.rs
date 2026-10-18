//! The page's headings as a section's context sees them.
#![allow(clippy::expect_used)]

use vyasa_core::BlockDocument;
use vyasa_themes::{render_blocks, toc_of};

fn doc() -> BlockDocument {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "blocks": [
            {"kind": "heading", "attrs": {"level": 2, "text": "First"}},
            {"kind": "group", "attrs": {}, "children": [
                {"kind": "heading", "attrs": {"level": 3, "text": "Nested"}}
            ]},
            {"kind": "heading", "attrs": {"level": 2, "text": "Second"}}
        ]
    }))
    .expect("document")
}

#[test]
fn toc_of_numbers_headings_in_document_order_as_the_renderer_does() {
    let document = doc();
    let toc = toc_of(&document.blocks);
    let anchors: Vec<_> = toc.iter().map(|t| t.anchor.as_str()).collect();
    assert_eq!(anchors, ["#vy-h-1", "#vy-h-2", "#vy-h-3"]);
    assert_eq!(
        toc.iter()
            .map(|t| (t.level, t.text.as_str()))
            .collect::<Vec<_>>(),
        [(2, "First"), (3, "Nested"), (2, "Second")]
    );
    let html = render_blocks(&document.blocks).expect("renders");
    for t in &toc {
        let id = t.anchor.trim_start_matches('#');
        assert!(
            html.contains(&format!("id=\"{id}\"")),
            "{id} is not in:\n{html}"
        );
    }
}
