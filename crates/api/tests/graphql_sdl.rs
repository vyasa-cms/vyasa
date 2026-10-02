//! Keeps the checked-in GraphQL schema honest.
//!
//! `docs/graphql-schema.graphql` is the reference a headless client reads
//! before writing a query. A schema change that does not update it turns
//! that file into confident misinformation, which is worse than having no
//! file at all — so this test fails the build instead.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// Regenerate with: `cargo test -p vyasa-api --test graphql_sdl -- --ignored`
#[test]
fn the_checked_in_sdl_matches_the_live_schema() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/graphql-schema.graphql")
        .canonicalize()
        .expect("docs/graphql-schema.graphql must exist; run the regenerate test");
    let on_disk = std::fs::read_to_string(&path).expect("read schema file");
    let live = sdl();
    assert_eq!(
        on_disk.trim(),
        live.trim(),
        "docs/graphql-schema.graphql is out of date. Regenerate it with:\n    \
         cargo test -p vyasa-api --test graphql_sdl -- --ignored --nocapture"
    );
}

#[test]
#[ignore = "writes docs/graphql-schema.graphql; run deliberately"]
fn regenerate_the_sdl() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/graphql-schema.graphql");
    std::fs::write(&path, sdl()).expect("write schema file");
    println!("wrote {}", path.display());
}

/// The SDL exactly as the running server reports it.
///
/// Shelling out to the binary rather than rebuilding the schema here: an
/// integration test cannot import a binary crate's modules, and rebuilding
/// the roots by hand would let the test and the server drift apart while
/// still agreeing with each other.
fn sdl() -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vyasa"))
        .args(["graphql", "sdl"])
        .output()
        .expect("run vyasa graphql sdl");
    assert!(
        out.status.success(),
        "vyasa graphql sdl failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("sdl is utf-8")
}
