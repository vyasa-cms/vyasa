//! `.vyplugin` package format + ed25519 signature verification.
//!
//! Archive layout:
//!
//! ```text
//! manifest.toml          # name, version, min_host_api, capabilities[]
//! plugin.wasm            # the component binary
//! signature.txt          # hex ed25519 sig over sha256(manifest || wasm)
//! ```
//!
//! Canonical form signed: `sha256(manifest.toml bytes) || sha256(plugin.wasm
//! bytes)` — both digests concatenated, then signed. This avoids any TOML
//! canonicalization debate while binding both artifacts together.

use std::io::Read;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;

use vyasa_common::AppError;

use crate::capabilities::Capability;

/// Maximum package size (wasm components are small; keep headroom).
pub const MAX_PACKAGE_BYTES: usize = 10 * 1024 * 1024;

/// `manifest.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    /// Package name `[a-z0-9-]{1,60}`.
    pub name: String,
    /// Semver-ish version string.
    pub version: String,
    /// Minimum host API this plugin requires.
    #[serde(default)]
    pub min_host_api: u32,
    /// Requested capability strings.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// One or two sentences for the admin page.
    #[serde(default)]
    pub description: String,
    /// Who made it.
    #[serde(default)]
    pub author: String,
    /// Where to read more or report problems.
    #[serde(default)]
    pub homepage: String,
    /// SPDX identifier or free text.
    #[serde(default)]
    pub license: String,
}

/// What an operator sees before deciding to install: the package parsed
/// and its signature checked, but nothing written.
pub struct Inspected {
    /// Validated manifest.
    pub manifest: PluginManifest,
    /// Capabilities parsed from the manifest.
    pub capabilities: Vec<Capability>,
    /// Whether a trusted key vouches for the signature.
    pub trusted: bool,
    /// The first 16 hex characters of the signature, to talk about it.
    pub signature_prefix: String,
    /// Bytes of wasm.
    pub wasm_bytes: usize,
}

/// Parses a package and reports whether it is trusted, without failing
/// on an untrusted signature.
///
/// # Errors
/// [`AppError::Validation`] for a malformed package; an untrusted
/// signature is reported, not an error.
pub fn inspect_rpplugin(
    bytes: &[u8],
    trusted_keys: &[VerifyingKey],
) -> Result<Inspected, AppError> {
    match parse_rpplugin(bytes, trusted_keys) {
        Ok(parsed) => {
            let sig = signature_prefix(bytes);
            Ok(Inspected {
                wasm_bytes: parsed.wasm.len(),
                capabilities: parsed.capabilities,
                manifest: parsed.manifest,
                trusted: true,
                signature_prefix: sig,
            })
        }
        Err(e) if e.to_string().contains("signature verification failed") => {
            // Parse again with a key that matches nothing to get the
            // manifest out; the signature check is the only thing that
            // failed, so everything before it held.
            let parsed = parse_rpplugin_unchecked(bytes)?;
            let sig = signature_prefix(bytes);
            Ok(Inspected {
                wasm_bytes: parsed.wasm.len(),
                capabilities: parsed.capabilities,
                manifest: parsed.manifest,
                trusted: false,
                signature_prefix: sig,
            })
        }
        Err(e) => Err(e),
    }
}

fn signature_prefix(bytes: &[u8]) -> String {
    let reader = std::io::Cursor::new(bytes);
    let Ok(mut archive) = zip::ZipArchive::new(reader) else {
        return String::new();
    };
    let Ok(mut f) = archive.by_name("signature.txt") else {
        return String::new();
    };
    let mut s = String::new();
    let _ = std::io::Read::read_to_string(&mut f, &mut s);
    s.trim().chars().take(16).collect()
}

/// A verified package.
pub struct ParsedPlugin {
    /// Validated manifest.
    pub manifest: PluginManifest,
    /// Component bytes.
    pub wasm: Vec<u8>,
    /// Capabilities parsed from the manifest.
    pub capabilities: Vec<Capability>,
}

fn err(msg: impl Into<String>) -> AppError {
    AppError::validation(msg)
}

/// Parses and verifies a package against a trusted public key.
///
/// # Errors
/// Returns [`AppError::validation`] with an operator-friendly message for
/// bad zips, hostile entries, malformed manifests, or bad signatures.
pub fn parse_rpplugin(
    bytes: &[u8],
    trusted_keys: &[VerifyingKey],
) -> Result<ParsedPlugin, AppError> {
    parse_inner(bytes, Some(trusted_keys))
}

fn parse_inner(
    bytes: &[u8],
    trusted_keys: Option<&[VerifyingKey]>,
) -> Result<ParsedPlugin, AppError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(err(format!(
            "package is {} bytes; max {MAX_PACKAGE_BYTES}",
            bytes.len()
        )));
    }
    let reader = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| err(format!("not a valid .vyplugin zip: {e}")))?;

    // Entry safety: no traversal/duplicates.
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let f = archive
            .by_index_raw(i)
            .map_err(|e| err(format!("corrupt entry #{i}: {e}")))?;
        let name = f.name().to_owned();
        if name.starts_with('/')
            || name.contains('\\')
            || name.split('/').any(|s| s == ".." || s == ".")
        {
            return Err(err(format!("unsafe entry path \"{name}\"")));
        }
        names.push(name);
    }
    // Duplicate names, checked against the archive's own declared entry
    // count rather than against `names`.
    //
    // `ZipArchive` keys entries by name, so a second `manifest.toml`
    // silently replaces the first and this list never contains a
    // duplicate — the guard that looked for one here could not fire. The
    // server itself was never confused (it reads every file by name, so it
    // verifies and installs the same bytes), but an operator extracting
    // the package with `unzip` gets *both* entries and can end up
    // reviewing a manifest the server never saw. A package with two
    // entries of one name is malformed; refuse it.
    if let Some(declared) = declared_entry_count(bytes) {
        if declared != names.len() {
            return Err(err(format!(
                "archive declares {declared} entries but holds {} distinct names",
                names.len()
            )));
        }
    }

    let mut read_entry = |name: &str| -> Result<Vec<u8>, AppError> {
        match archive.by_name(name) {
            Ok(f) => {
                // Through a limit, not up to the declared size: the header
                // is the packager's claim, and a bomb claims small. No
                // single entry can legitimately exceed the whole-package
                // cap, so that is the ceiling.
                let mut buf = Vec::with_capacity(usize::try_from(f.size()).unwrap_or(0));
                let mut limited = f.take(u64::try_from(MAX_PACKAGE_BYTES).unwrap_or(u64::MAX) + 1);
                limited
                    .read_to_end(&mut buf)
                    .map_err(|e| err(format!("cannot read {name}: {e}")))?;
                if buf.len() > MAX_PACKAGE_BYTES {
                    return Err(err(format!(
                        "{name} inflates past {MAX_PACKAGE_BYTES} bytes"
                    )));
                }
                Ok(buf)
            }
            Err(zip::result::ZipError::FileNotFound) => {
                Err(err(format!("package missing required file \"{name}\"")))
            }
            Err(e) => Err(err(format!("cannot open {name}: {e}"))),
        }
    };

    let manifest_bytes = read_entry("manifest.toml")?;
    let manifest: PluginManifest = toml::from_str(
        std::str::from_utf8(&manifest_bytes).map_err(|_| err("manifest not utf-8"))?,
    )
    .map_err(|e| err(format!("manifest.toml: {e}")))?;
    if manifest.name.is_empty()
        || manifest.name.len() > 60
        || !manifest
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(err("manifest name must match [a-z0-9-]{1,60}"));
    }

    // Validate capability grammar early so install surfaces typos.
    let capabilities = manifest
        .capabilities
        .iter()
        .map(|c| Capability::parse(c))
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;

    let wasm = read_entry("plugin.wasm")?;
    if wasm.len() < 4 || !wasm.starts_with(b"\0asm") {
        return Err(err("plugin.wasm is not a WebAssembly binary"));
    }

    // Signature: hex ed25519 over sha256(manifest) || sha256(wasm).
    let sig_hex = String::from_utf8(read_entry("signature.txt")?)
        .map_err(|_| err("signature.txt is not utf-8"))?;
    if let Some(keys) = trusted_keys {
        verify_signature(keys, &sig_hex, &manifest_bytes, &wasm)?;
    }

    Ok(ParsedPlugin {
        manifest,
        wasm,
        capabilities,
    })
}

/// Parses without checking the signature. Only for inspection; nothing
/// that installs may call it.
fn parse_rpplugin_unchecked(bytes: &[u8]) -> Result<ParsedPlugin, AppError> {
    parse_inner(bytes, None)
}

/// How many entries the zip's end-of-central-directory record declares.
///
/// Read directly rather than inferred, because that count is the only
/// place a duplicate name survives: every other reader collapses them.
/// Returns `None` when the record cannot be located, in which case the
/// caller simply skips the check — `ZipArchive::new` will have failed
/// already for anything genuinely unreadable.
fn declared_entry_count(bytes: &[u8]) -> Option<usize> {
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    // The record is 22 bytes plus a comment of up to 65 535, and it is the
    // last thing in the file, so scan backwards from the end.
    let start = bytes.len().checked_sub(22)?;
    let floor = start.saturating_sub(u16::MAX as usize);
    let at = (floor..=start).rev().find(|i| bytes[*i..*i + 4] == EOCD)?;
    let count = u16::from_le_bytes([bytes[at + 10], bytes[at + 11]]);
    Some(count as usize)
}

/// Verifies the detached signature over the canonical double digest.
///
/// Accepts when **any** trusted key verifies (key rotation support).
///
/// # Errors
/// [`AppError::validation`] naming the failure reason.
pub fn verify_signature(
    trusted_keys: &[VerifyingKey],
    sig_hex: &str,
    manifest: &[u8],
    wasm: &[u8],
) -> Result<(), AppError> {
    if trusted_keys.is_empty() {
        return Err(err("no trusted signing keys configured"));
    }
    let clean: String = sig_hex.chars().filter(char::is_ascii_hexdigit).collect();
    let raw = hex::decode(&clean).map_err(|_| err("signature is not valid hex"))?;
    let sig_bytes: [u8; 64] = raw
        .try_into()
        .map_err(|_| err("signature must be 64 bytes"))?;
    let sig = Signature::from_bytes(&sig_bytes);

    let mut msg = Sha256::digest(manifest).to_vec();
    msg.extend_from_slice(&Sha256::digest(wasm));

    for key in trusted_keys {
        if key.verify(&msg, &sig).is_ok() {
            return Ok(());
        }
    }
    Err(err(
        "signature verification failed against all trusted keys",
    ))
}

use sha2::{Digest as _, Sha256};
