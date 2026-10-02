//! Fixture-driven tests: valid themes compile (snapshot-checked), malformed
//! documents are rejected with precise field-path errors.

#![allow(clippy::pedantic)]

use std::fs;
use std::path::Path;

use vyasa_themes::{
    compile_tokens, parse_token_set, tokens_to_css, validate_token_set, TokenDiagnostic,
};

fn fixture(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn blog_fixture_compiles_with_no_diagnostics() {
    let json = fixture("valid/blog.json");
    let (css, warnings) = compile_tokens(&json).expect("blog.json is valid");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert!(css.contains("--vy-font-heading: \"Inter\", system-ui, sans-serif;"));
    assert!(css.contains("@font-face"));
    assert!(css.contains("--vy-color-primary: #0f766e;"));
}

#[test]
fn minimal_fixture_falls_back_to_defaults() {
    let json = fixture("valid/minimal.json");
    let (css, warnings) = compile_tokens(&json).expect("minimal.json is valid");
    assert!(warnings.is_empty());
    assert_eq!(
        css,
        tokens_to_css(&vyasa_themes::TokenSet::default()),
        "empty token set must equal compiled defaults"
    );
}

const INVALID_FIXTURES: &[(&str, &str)] = &[
    ("unknown_key.json", ""),
    ("unknown_nested_key.json", "colors.primaryy"),
    ("bad_hex_prefix.json", "colors.bg.light"),
    ("bad_hex_length.json", "colors.text.light"),
    ("bad_version.json", "version"),
    ("ratio_out_of_range.json", "typography.scale_ratio"),
    ("unit_too_big.json", "spacing.unit_px"),
    ("font_src_not_https.json", "typography.font_faces[0].src"),
    ("font_family_quotes.json", "typography.font_faces[0].family"),
    ("radius_negative.json", "radius_px"),
    ("content_width_small.json", "layout.content_width_px"),
    ("section_scale_empty.json", "spacing.section_scale"),
    ("section_scale_unsorted.json", "spacing.section_scale[1]"),
    ("wrong_type.json", ""),
];

#[test]
fn malformed_fixtures_are_rejected_with_field_paths() {
    for (name, expected_path) in INVALID_FIXTURES {
        let json = fixture(&format!("invalid/{name}"));
        match parse_token_set(&json) {
            Ok(_) => {
                // Parsed but must fail validation with an error at the path.
                let tokens = parse_token_set(&json).expect("checked");
                let diags = validate_token_set(&tokens);
                assert!(
                    diags.iter().any(TokenDiagnostic::is_error),
                    "{name}: expected a hard error, got {diags:?}"
                );
                if !expected_path.is_empty() {
                    assert!(
                        diags.iter().any(|d| d.path() == *expected_path),
                        "{name}: expected path \"{expected_path}\", got {diags:?}"
                    );
                }
            }
            Err(diags) => {
                assert!(
                    diags.iter().all(TokenDiagnostic::is_error),
                    "{name}: parse failures must be errors"
                );
                if !expected_path.is_empty() {
                    assert!(
                        diags.iter().any(|d| d.path() == *expected_path),
                        "{name}: expected path \"{expected_path}\", got {diags:?}"
                    );
                }
            }
        }
    }
}

// --- CSS snapshots -----------------------------------------------------------

#[test]
fn snapshot_blog_css() {
    let json = fixture("valid/blog.json");
    let (css, _) = compile_tokens(&json).expect("valid");
    insta::assert_snapshot!("blog_css", css);
}

#[test]
fn snapshot_default_css() {
    insta::assert_snapshot!("default_css", tokens_to_css(&Default::default()));
}
