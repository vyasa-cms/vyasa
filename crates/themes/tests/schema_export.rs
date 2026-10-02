//! Keeps docs/theme-token-schema.json in sync with the derive.

use std::fs;
use std::path::Path;

const SCHEMA_PATH: &str = "../../docs/theme-token-schema.json";

/// Regenerate with: cargo test -p vyasa-themes --test schema_export -- --ignored
#[test]
#[ignore = "generator; run explicitly to refresh the exported schema"]
fn export_schema() {
    let schema =
        serde_json::to_string_pretty(&vyasa_themes::json_schema()).expect("schema serializes");
    fs::write(SCHEMA_PATH, schema + "\n").expect("writes schema file");
}

#[test]
fn committed_schema_is_current() {
    let path = Path::new(SCHEMA_PATH);
    let expected = serde_json::to_string_pretty(&vyasa_themes::json_schema())
        .expect("schema serializes")
        + "\n";
    match fs::read_to_string(path) {
        Ok(actual) => assert_eq!(
            actual, expected,
            "docs/theme-token-schema.json is stale; run: cargo test -p vyasa-themes \
             --test schema_export export_schema -- --ignored"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::write(path, &expected).expect("creates missing schema file");
        }
        Err(e) => panic!("cannot read {SCHEMA_PATH}: {e}"),
    }
}
