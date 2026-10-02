//! The `.vyplugin` format: zip safety, manifest rules and signatures.
//!
//! This is the code that decides whether a stranger's WebAssembly runs on
//! someone's site, so each rule it enforces gets a case that would pass if
//! the rule were removed.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest as _, Sha256};
use vyasa_plugins::package::{parse_rpplugin, verify_signature, MAX_PACKAGE_BYTES};

const MANIFEST: &str = "name = \"demo\"\nversion = \"1.0.0\"\ncapabilities = [\"log:write\"]\n";
/// The shortest byte string `parse_rpplugin` accepts as a component.
const WASM: &[u8] = b"\0asm\x01\0\0\0";

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn signature(signing: &SigningKey, manifest: &[u8], wasm: &[u8]) -> String {
    let mut msg = Sha256::digest(manifest).to_vec();
    msg.extend_from_slice(&Sha256::digest(wasm));
    hex::encode(signing.sign(&msg).to_bytes())
}

/// Builds a zip from `(name, bytes)` pairs, exactly as given — including
/// hostile names and duplicates, which is the point.
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, bytes) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
    }
    buf.into_inner()
}

/// A well-formed package signed by `signing`.
fn package(signing: &SigningKey, manifest: &str, wasm: &[u8]) -> Vec<u8> {
    let sig = signature(signing, manifest.as_bytes(), wasm);
    zip_of(&[
        ("manifest.toml", manifest.as_bytes()),
        ("plugin.wasm", wasm),
        ("signature.txt", sig.as_bytes()),
    ])
}

fn message_of(bytes: &[u8], keys: &[VerifyingKey]) -> String {
    parse_rpplugin(bytes, keys)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_else(|| String::from("<accepted>"))
}

#[test]
fn a_well_formed_signed_package_parses() {
    let signing = key(1);
    let parsed = parse_rpplugin(
        &package(&signing, MANIFEST, WASM),
        &[signing.verifying_key()],
    )
    .expect("accepted");
    assert_eq!(parsed.manifest.name, "demo");
    assert_eq!(parsed.manifest.version, "1.0.0");
    assert_eq!(parsed.wasm, WASM);
    assert_eq!(parsed.capabilities.len(), 1);
}

#[test]
fn any_trusted_key_verifies_which_is_what_makes_rotation_possible() {
    let old = key(1);
    let new = key(2);
    let signed_by_new = package(&new, MANIFEST, WASM);
    // Both keys trusted: a package signed by either is accepted, so an
    // operator can add the new key before re-signing anything.
    assert!(parse_rpplugin(&signed_by_new, &[old.verifying_key(), new.verifying_key()]).is_ok());
    // The old key alone is not enough.
    let msg = message_of(&signed_by_new, &[old.verifying_key()]);
    assert!(msg.contains("signature verification failed"), "{msg}");
}

#[test]
fn tampering_with_either_file_invalidates_the_signature() {
    let signing = key(1);
    let trusted = [signing.verifying_key()];
    let sig = signature(&signing, MANIFEST.as_bytes(), WASM);

    // The signature covers both digests concatenated, so changing either
    // one breaks it — that is what binds the manifest to the binary and
    // stops a capability list being swapped onto someone else's wasm.
    let swapped_manifest = zip_of(&[
        (
            "manifest.toml",
            "name = \"demo\"\nversion = \"1.0.0\"\ncapabilities = [\"net:fetch:*\"]\n".as_bytes(),
        ),
        ("plugin.wasm", WASM),
        ("signature.txt", sig.as_bytes()),
    ]);
    let msg = message_of(&swapped_manifest, &trusted);
    assert!(msg.contains("signature verification failed"), "{msg}");

    let swapped_wasm = zip_of(&[
        ("manifest.toml", MANIFEST.as_bytes()),
        ("plugin.wasm", b"\0asm\x02\0\0\0"),
        ("signature.txt", sig.as_bytes()),
    ]);
    let msg = message_of(&swapped_wasm, &trusted);
    assert!(msg.contains("signature verification failed"), "{msg}");
}

#[test]
fn hostile_entry_paths_are_refused_before_anything_is_read() {
    let signing = key(1);
    let trusted = [signing.verifying_key()];
    let sig = signature(&signing, MANIFEST.as_bytes(), WASM);
    for hostile in [
        "../escape.txt",
        "/absolute.txt",
        "a/../../b.txt",
        "dir\\windows.txt",
        "./here.txt",
    ] {
        let bytes = zip_of(&[
            ("manifest.toml", MANIFEST.as_bytes()),
            ("plugin.wasm", WASM),
            ("signature.txt", sig.as_bytes()),
            (hostile, b"payload"),
        ]);
        let msg = message_of(&bytes, &trusted);
        assert!(msg.contains("unsafe entry path"), "{hostile}: {msg}");
    }
}

#[test]
fn a_duplicate_entry_is_refused_rather_than_resolved() {
    // Two `manifest.toml` entries would let the signature be checked
    // against one and the install read the other.
    //
    // The `zip` writer refuses to emit a duplicate name, so the archive is
    // built with a decoy entry and renamed afterwards — which is what an
    // attacker would do too, since this shape can only be hand-crafted.
    // The name appears in the local header and again in the central
    // directory, and the replacement is the same length, so no offset in
    // the file moves.
    let signing = key(1);
    let sig = signature(&signing, MANIFEST.as_bytes(), WASM);
    let mut bytes = zip_of(&[
        ("manifest.toml", MANIFEST.as_bytes()),
        ("Xanifest.toml", b"name = \"other\"\nversion = \"9\"\n"),
        ("plugin.wasm", WASM),
        ("signature.txt", sig.as_bytes()),
    ]);
    let decoy = b"Xanifest.toml";
    let real = b"manifest.toml";
    let mut renamed = 0;
    for i in 0..bytes.len().saturating_sub(decoy.len()) {
        if &bytes[i..i + decoy.len()] == decoy {
            bytes[i..i + real.len()].copy_from_slice(real);
            renamed += 1;
        }
    }
    assert_eq!(renamed, 2, "local header and central directory");

    let msg = message_of(&bytes, &[signing.verifying_key()]);
    assert!(msg.contains("declares 4 entries"), "{msg}");
}

#[test]
fn every_required_file_must_be_present() {
    let signing = key(1);
    let trusted = [signing.verifying_key()];
    let sig = signature(&signing, MANIFEST.as_bytes(), WASM);
    let all: [(&str, &[u8]); 3] = [
        ("manifest.toml", MANIFEST.as_bytes()),
        ("plugin.wasm", WASM),
        ("signature.txt", sig.as_bytes()),
    ];
    for missing in 0..3 {
        let kept: Vec<(&str, &[u8])> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != missing)
            .map(|(_, e)| *e)
            .collect();
        let msg = message_of(&zip_of(&kept), &trusted);
        assert!(msg.contains("missing required file"), "{missing}: {msg}");
    }
}

#[test]
fn a_package_that_is_not_a_zip_is_refused() {
    let msg = message_of(b"this is not a zip file at all", &[key(1).verifying_key()]);
    assert!(msg.contains("not a valid .vyplugin zip"), "{msg}");
}

#[test]
fn an_oversized_package_is_refused_without_being_parsed() {
    let huge = vec![0u8; MAX_PACKAGE_BYTES + 1];
    let msg = message_of(&huge, &[key(1).verifying_key()]);
    assert!(msg.contains("max"), "{msg}");
}

#[test]
fn manifest_names_are_constrained_because_the_name_becomes_a_url() {
    let signing = key(1);
    let trusted = [signing.verifying_key()];
    for bad_name in [
        "",
        "Upper",
        "has space",
        "under_score",
        "sl/ash",
        &"x".repeat(61),
    ] {
        let manifest = format!("name = \"{bad_name}\"\nversion = \"1.0.0\"\n");
        let msg = message_of(&package(&signing, &manifest, WASM), &trusted);
        assert!(
            msg.contains("manifest name must match"),
            "{bad_name:?}: {msg}"
        );
    }
    // The plugin name is its mount point at /api/v1/plugin/<name>/…, so
    // this grammar is what keeps a package from claiming a path.
    let ok = "name = \"a-good-name-9\"\nversion = \"1.0.0\"\n";
    assert!(parse_rpplugin(&package(&signing, ok, WASM), &trusted).is_ok());
}

#[test]
fn a_capability_typo_is_caught_at_install_not_at_first_call() {
    let signing = key(1);
    let manifest = "name = \"demo\"\nversion = \"1.0.0\"\ncapabilities = [\"db:reed:posts\"]\n";
    let msg = message_of(
        &package(&signing, manifest, WASM),
        &[signing.verifying_key()],
    );
    assert!(msg.contains("unknown capability"), "{msg}");
}

#[test]
fn a_malformed_manifest_is_a_message_not_a_panic() {
    let signing = key(1);
    let trusted = [signing.verifying_key()];
    let msg = message_of(&package(&signing, "this is not toml {{{", WASM), &trusted);
    assert!(msg.contains("manifest.toml"), "{msg}");

    // Missing a required field.
    let msg = message_of(&package(&signing, "version = \"1.0.0\"\n", WASM), &trusted);
    assert!(msg.contains("manifest.toml"), "{msg}");
}

#[test]
fn the_payload_must_actually_be_webassembly() {
    let signing = key(1);
    let trusted = [signing.verifying_key()];
    for not_wasm in [b"".as_slice(), b"MZ".as_slice(), b"#!/bin/sh\n".as_slice()] {
        let msg = message_of(&package(&signing, MANIFEST, not_wasm), &trusted);
        assert!(msg.contains("not a WebAssembly binary"), "{msg}");
    }
}

#[test]
fn signature_shape_is_checked_before_it_is_verified() {
    let manifest = MANIFEST.as_bytes();
    let trusted = [key(1).verifying_key()];

    assert!(verify_signature(&[], "00", manifest, WASM)
        .unwrap_err()
        .to_string()
        .contains("no trusted signing keys"));

    let short = hex::encode([0u8; 32]);
    assert!(verify_signature(&trusted, &short, manifest, WASM)
        .unwrap_err()
        .to_string()
        .contains("64 bytes"));

    // Whitespace and newlines around the hex are tolerated: the file is
    // written by shell tooling that likes to add a trailing newline.
    let signing = key(1);
    let sig = signature(&signing, manifest, WASM);
    let padded = format!("  {sig}\n");
    verify_signature(&[signing.verifying_key()], &padded, manifest, WASM).expect("tolerates space");
}

#[test]
fn a_signature_of_the_wrong_length_after_cleaning_is_refused() {
    // Non-hex characters are stripped, so a signature made of them alone
    // must not decode to an empty-and-therefore-accepted value.
    let trusted = [key(1).verifying_key()];
    let msg = verify_signature(&trusted, "zz zz", MANIFEST.as_bytes(), WASM)
        .unwrap_err()
        .to_string();
    assert!(msg.contains("64 bytes"), "{msg}");

    // An odd number of hex digits is a decode failure, not a length one.
    let msg = verify_signature(&trusted, "abc", MANIFEST.as_bytes(), WASM)
        .unwrap_err()
        .to_string();
    assert!(msg.contains("not valid hex"), "{msg}");
}
