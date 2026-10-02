//! Packaging and signing for plugin authors.
//!
//! Installing a plugin requires an ed25519 signature over
//! `sha256(manifest.toml) || sha256(plugin.wasm)` from a key the server
//! trusts. The SDK shipped a `sign.sh` that referenced a helper binary
//! that did not exist, so it wrote an empty placeholder and every package
//! it produced was rejected — nobody could install a plugin at all,
//! including their own.
//!
//! These commands are that missing helper: `keygen` mints a keypair and
//! prints the public half to put in `plugin_trusted_keys`, `pack` builds
//! and signs the `.vyplugin` zip.

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest as _, Sha256};

/// The three entries a `.vyplugin` archive holds.
const MANIFEST: &str = "manifest.toml";
const WASM: &str = "plugin.wasm";
const SIGNATURE: &str = "signature.txt";

/// The bytes a signature covers: the two digests, concatenated raw.
///
/// Must stay identical to `vyasa_plugins::package::verify_signature`; a
/// mismatch here produces packages the server rejects with no useful
/// explanation.
#[must_use]
pub fn signing_payload(manifest: &[u8], wasm: &[u8]) -> Vec<u8> {
    let mut msg = Sha256::digest(manifest).to_vec();
    msg.extend_from_slice(&Sha256::digest(wasm));
    msg
}

/// Prints a fresh keypair: the secret to sign with, the public to trust.
pub fn keygen() {
    use rand::RngCore as _;
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    let public = hex::encode(signing.verifying_key().to_bytes());
    println!("secret (keep private, pass as VYASA_SIGNING_KEY):");
    println!("  {}", hex::encode(seed));
    println!();
    println!("public (add to plugin_trusted_keys):");
    println!("  {public}");
    println!();
    println!("Trust it by adding to your .env:");
    println!("  VYASA_PLUGIN_TRUSTED_KEYS={public}");
}

/// Builds `<dir>` into a signed `.vyplugin` package.
///
/// `dir` must contain `manifest.toml` and `plugin.wasm`.
///
/// # Errors
/// A message naming the missing file, the bad key, or the write failure.
pub fn pack(dir: &Path, out: Option<PathBuf>, key_hex: Option<String>) -> Result<(), String> {
    let manifest_path = dir.join(MANIFEST);
    let wasm_path = dir.join(WASM);
    let manifest =
        std::fs::read(&manifest_path).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let wasm = std::fs::read(&wasm_path).map_err(|e| format!("{}: {e}", wasm_path.display()))?;
    if !wasm.starts_with(b"\0asm") {
        return Err(format!(
            "{} is not WebAssembly (build with: cargo build --release --target wasm32-wasip2)",
            wasm_path.display()
        ));
    }

    let key_hex = key_hex
        .or_else(|| std::env::var("VYASA_SIGNING_KEY").ok())
        .ok_or_else(|| {
            String::from(
                "no signing key: pass --key <hex> or set VYASA_SIGNING_KEY \
                 (make one with `vyasa plugin keygen`)",
            )
        })?;
    let seed: [u8; 32] = hex::decode(key_hex.trim())
        .map_err(|_| String::from("signing key is not hex"))?
        .try_into()
        .map_err(|_| String::from("signing key must be 32 bytes (64 hex characters)"))?;
    let signing = SigningKey::from_bytes(&seed);
    let signature = hex::encode(signing.sign(&signing_payload(&manifest, &wasm)).to_bytes());

    let out = out.unwrap_or_else(|| {
        let name = dir
            .file_name()
            .map_or_else(|| String::from("plugin"), |n| n.to_string_lossy().into());
        PathBuf::from(format!("{name}.vyplugin"))
    });
    let file = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in [
        (MANIFEST, manifest.as_slice()),
        (WASM, wasm.as_slice()),
        (SIGNATURE, signature.as_bytes()),
    ] {
        use std::io::Write as _;
        zip.start_file(name, options)
            .map_err(|e| format!("zip {name}: {e}"))?;
        zip.write_all(bytes)
            .map_err(|e| format!("zip {name}: {e}"))?;
    }
    zip.finish().map_err(|e| format!("zip finish: {e}"))?;

    println!("wrote {}", out.display());
    println!(
        "signed with public key {}",
        hex::encode(signing.verifying_key().to_bytes())
    );
    println!("the server must list that key in plugin_trusted_keys to accept it");
    Ok(())
}

/// Installs a package straight into the database, running exactly the
/// validation the REST route runs: signature against the configured
/// trusted keys, capability names, size and zip-safety checks.
///
/// The HTTP route needs a browser session; a server operator deploying
/// from a shell has one anyway (they hold the database URL), and needing
/// to click through the admin to install is a poor fit for scripted
/// deploys.
///
/// # Errors
/// A message describing the rejected package or the database failure.
pub async fn install(
    config: &vyasa_common::VyasaConfig,
    path: &Path,
    enable: bool,
) -> Result<(), String> {
    use ed25519_dalek::VerifyingKey;

    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let keys: Vec<VerifyingKey> = config
        .plugin_trusted_keys
        .iter()
        .filter_map(|raw| {
            let bytes = hex::decode(raw.trim()).ok()?;
            let arr: [u8; 32] = bytes.as_slice().try_into().ok()?;
            VerifyingKey::from_bytes(&arr).ok()
        })
        .collect();
    if keys.is_empty() {
        return Err(String::from(
            "no trusted signing keys configured; set VYASA_PLUGIN_TRUSTED_KEYS              to the public key that signed this package",
        ));
    }
    let parsed = vyasa_plugins::package::parse_rpplugin(&bytes, &keys)
        .map_err(|e| format!("package rejected: {e}"))?;

    let pool = vyasa_db::pool(config)
        .await
        .map_err(|e| format!("database: {e}"))?;
    let repo = vyasa_db::repo::PluginsRepo::new(pool);
    let sha = hex::encode(Sha256::digest(&parsed.wasm));
    let caps = serde_json::json!(parsed
        .capabilities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>());
    let row = vyasa_plugins::lifecycle::install(
        &repo,
        &parsed.manifest.name,
        &parsed.manifest.version,
        &parsed.wasm,
        &sha,
        &caps,
    )
    .await
    .map_err(|e| format!("install failed: {e}"))?;

    if enable {
        repo.set_enabled(row.id, true)
            .await
            .map_err(|e| format!("enable failed: {e}"))?;
    }
    println!(
        "installed {} v{} (id {}){}",
        parsed.manifest.name,
        parsed.manifest.version,
        row.id,
        if enable { ", enabled" } else { "" }
    );
    println!(
        "capabilities: {}",
        parsed
            .capabilities
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    // This wrote to the database from a different process, so a server
    // that is already up has not seen any of it. Enabling through the
    // admin needs no restart; this path does.
    println!("restart the server, or enable it again from Admin → Plugins");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload must match what the server verifies, byte for byte.
    #[test]
    fn payload_is_two_raw_digests_concatenated() {
        let manifest = b"name = \"x\"\n";
        let wasm = b"\0asm\x01\0\0\0";
        let payload = signing_payload(manifest, wasm);
        assert_eq!(payload.len(), 64, "two 32-byte digests");
        assert_eq!(&payload[..32], Sha256::digest(manifest).as_slice());
        assert_eq!(&payload[32..], Sha256::digest(wasm).as_slice());
    }

    /// A package this tool signs verifies against the server's own check.
    #[test]
    fn signed_package_verifies_against_the_host_verifier() {
        use ed25519_dalek::VerifyingKey;
        let seed = [7u8; 32];
        let signing = SigningKey::from_bytes(&seed);
        let manifest = b"name = \"hello\"\nversion = \"0.1.0\"\n";
        let wasm = b"\0asm\x01\0\0\0";
        let sig = hex::encode(signing.sign(&signing_payload(manifest, wasm)).to_bytes());

        let public: VerifyingKey = signing.verifying_key();
        vyasa_plugins::package::verify_signature(&[public], &sig, manifest, wasm)
            .expect("the server accepts what we sign");

        // A different key must not.
        let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key();
        assert!(
            vyasa_plugins::package::verify_signature(&[other], &sig, manifest, wasm).is_err(),
            "any key must not verify"
        );
        // Tampering with either file must break it.
        assert!(
            vyasa_plugins::package::verify_signature(&[public], &sig, manifest, b"\0asm\x02\0\0\0")
                .is_err(),
            "a swapped wasm must not verify"
        );
    }
}
