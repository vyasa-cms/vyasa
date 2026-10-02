//! Keeps the checked-in OpenAPI document honest.
//!
//! `admin/openapi.json` is what the admin's generated types are built from
//! (`pnpm gen:api` falls back to it). A route change that does not update it
//! leaves the admin typed against an API that no longer exists.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// Regenerate with:
/// `cargo test -p vyasa-api --test openapi_snapshot -- --ignored`,
/// then `cd admin && VYASA_API_URL=http://127.0.0.1:9 pnpm gen:api`.
#[test]
fn the_checked_in_openapi_matches_the_live_document() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../admin/openapi.json");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read admin/openapi.json"))
            .expect("admin/openapi.json is JSON");
    assert_eq!(
        on_disk,
        live(),
        "admin/openapi.json is out of date. Regenerate it with:\n    \
         cargo test -p vyasa-api --test openapi_snapshot -- --ignored\n    \
         cd admin && VYASA_API_URL=http://127.0.0.1:9 pnpm gen:api"
    );
}

#[test]
#[ignore = "writes admin/openapi.json; run deliberately"]
fn regenerate_the_openapi_snapshot() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../admin/openapi.json");
    std::fs::write(&path, serde_json::to_string(&live()).expect("serialise"))
        .expect("write admin/openapi.json");
    println!("wrote {}", path.display());
}

/// The document exactly as the binary reports it (an integration test
/// cannot import the binary crate's modules).
fn live() -> serde_json::Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vyasa"))
        .arg("openapi")
        .output()
        .expect("run vyasa openapi");
    assert!(
        out.status.success(),
        "vyasa openapi failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("vyasa openapi prints JSON")
}
