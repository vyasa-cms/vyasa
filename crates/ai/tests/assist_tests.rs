//! Extraction + gating tests for editor assist.

#![allow(clippy::pedantic, clippy::unwrap_used)]

use vyasa_ai::assist::{enough_content, extract_text, AssistKind};
use vyasa_core::block::Block;

fn block(kind: &str, attrs: serde_json::Value) -> Block {
    Block {
        kind: serde_json::from_value(serde_json::json!(kind)).unwrap(),
        plugin_kind: None,
        attrs,
        children: Vec::new(),
    }
}

#[test]
fn extraction_collects_paragraphs_and_headings() {
    let doc = vec![
        block("heading", serde_json::json!({"text": "Intro", "level": 2})),
        block("paragraph", serde_json::json!({"text": "Body text here."})),
        block("image", serde_json::json!({"url": "https://x/y.png"})),
        block("code", serde_json::json!({"code": "ignored()"})),
    ];
    let text = extract_text(&doc, 4000);
    assert!(text.contains("Intro"));
    assert!(text.contains("Body text here."));
    assert!(!text.contains("ignored"));
}

#[test]
fn empty_and_image_only_posts_fail_enough_content() {
    assert!(!enough_content(""));
    let image_only = vec![block(
        "image",
        serde_json::json!({"url": "https://cdn.example.com/a.png"}),
    )];
    let text = extract_text(&image_only, 4000);
    assert!(!enough_content(&text), "image-only has no text");
}

#[test]
fn long_text_is_truncated_to_budget_with_ellipsis() {
    let long = "word ".repeat(2000);
    let doc = vec![block("paragraph", serde_json::json!({"text": long}))];
    let text = extract_text(&doc, 100);
    assert!(text.chars().count() <= 101);
    assert!(text.ends_with('…'));
}

#[test]
fn kinds_parse_and_purpose_map() {
    assert_eq!(
        AssistKind::parse("title").unwrap().purpose(),
        "assist-title"
    );
    assert_eq!(AssistKind::parse("tags").unwrap().purpose(), "assist-tags");
    assert!(AssistKind::parse("bogus").is_err());
}
