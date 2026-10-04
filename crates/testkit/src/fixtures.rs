//! Signed packages for tests: a plugin built the way `vyasa plugin pack`
//! builds one, and a theme that is only data. Keys are fresh per call.

use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest as _, Sha256};
use std::io::Write as _;

/// The smallest component in the tree, enough for a package to parse.
const HELLO_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/hello/hello-component.wasm"
));

/// A fresh ed25519 keypair as `(secret_hex, public_hex)`.
#[must_use]
pub fn keypair() -> (String, String) {
    use rand::RngCore as _;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let key = SigningKey::from_bytes(&seed);
    (
        hex::encode(seed),
        hex::encode(key.verifying_key().to_bytes()),
    )
}

fn signing_key(secret_hex: &str) -> SigningKey {
    let seed: [u8; 32] = hex::decode(secret_hex)
        .expect("secret is hex")
        .try_into()
        .expect("secret is 32 bytes");
    SigningKey::from_bytes(&seed)
}

/// Hex ed25519 signature over `bytes` by `secret_hex`: the detached form
/// a marketplace listing carries.
#[must_use]
pub fn sign_hex(secret_hex: &str, bytes: &[u8]) -> String {
    hex::encode(signing_key(secret_hex).sign(bytes).to_bytes())
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, options).expect("zip entry");
            zip.write_all(bytes).expect("zip write");
        }
        zip.finish().expect("zip finish");
    }
    buf.into_inner()
}

/// A `.vyplugin` named `name`, requesting `caps`, author-signed by
/// `author_secret` (`signature.txt` = hex ed25519 over
/// `sha256(manifest) || sha256(wasm)`).
#[must_use]
pub fn plugin_package(name: &str, version: &str, caps: &[&str], author_secret: &str) -> Vec<u8> {
    let caps = caps
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest = format!(
        "name = \"{name}\"\nversion = \"{version}\"\nmin_host_api = 0\n\
         capabilities = [{caps}]\ndescription = \"test plugin\"\n"
    );
    let mut msg = Sha256::digest(manifest.as_bytes()).to_vec();
    msg.extend_from_slice(&Sha256::digest(HELLO_WASM));
    let signature = hex::encode(signing_key(author_secret).sign(&msg).to_bytes());
    zip_of(&[
        ("manifest.toml", manifest.as_bytes()),
        ("plugin.wasm", HELLO_WASM),
        ("signature.txt", signature.as_bytes()),
    ])
}

/// A `.vytheme` that is only data: tokens and the blog starter's layout.
#[must_use]
pub fn data_theme_package(name: &str, version: u32) -> Vec<u8> {
    theme_package(name, version, &[])
}

/// A `.vytheme` with extra entries, for example `assets/theme.js`.
#[must_use]
pub fn theme_package(name: &str, version: u32, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let manifest =
        format!("name = \"{name}\"\nversion = {version}\nauthor = \"t\"\nrequired_api = 1\n");
    let layout = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../themes-starter/blog/layout.json"
    ));
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("manifest.toml", manifest.as_bytes()),
        ("tokens.json", br#"{"version":1}"#),
        ("layout.json", layout),
    ];
    entries.extend_from_slice(extra);
    zip_of(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_plugin_parses_under_its_author_key() {
        let (secret, public) = keypair();
        let bytes = plugin_package("fx", "1.0.0", &["log:write"], &secret);
        let key_bytes: [u8; 32] = hex::decode(public).unwrap().try_into().unwrap();
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes).unwrap();
        let parsed = vyasa_plugins::package::parse_rpplugin(&bytes, &[key]).expect("parses");
        assert_eq!(parsed.manifest.name, "fx");
    }

    #[test]
    fn a_data_theme_parses() {
        vyasa_themes::package::parse_vytheme(&data_theme_package("fx", 1)).expect("parses");
    }
}
