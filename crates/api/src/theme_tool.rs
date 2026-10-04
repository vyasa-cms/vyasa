//! `vyasa theme pack`: a theme source tree to a `.vytheme`, optionally
//! signed so a scripted theme can be uploaded or published.

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer as _, SigningKey};

/// Zips `dir`, validates the result as a theme, writes
/// `<name>-<version>.vytheme` (or `out`) and, with a key, `<file>.sig`.
///
/// # Errors
/// A message for the terminal.
pub fn pack(dir: &Path, out: Option<PathBuf>, key_hex: Option<String>) -> Result<(), String> {
    let bytes = pack_bytes(dir)?;
    let parsed = vyasa_themes::package::parse_vytheme(&bytes)
        .map_err(|e| format!("not a valid theme: {e}"))?;
    let out = out.unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}-{}.vytheme",
            parsed.manifest.name, parsed.manifest.version
        ))
    });
    std::fs::write(&out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {}", out.display());
    if let Some(key) = key_hex.or_else(|| std::env::var("VYASA_SIGNING_KEY").ok()) {
        let signature = sign(&bytes, &key)?;
        let sig_path = PathBuf::from(format!("{}.sig", out.display()));
        std::fs::write(&sig_path, &signature.1)
            .map_err(|e| format!("{}: {e}", sig_path.display()))?;
        println!("signed with public key {}", signature.0);
    }
    Ok(())
}

/// The zip of every file under `dir` (hidden files and READMEs left out).
pub fn pack_bytes(dir: &Path) -> Result<Vec<u8>, String> {
    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options = zip::write::SimpleFileOptions::default();
        for rel in files {
            use std::io::Write as _;
            let bytes = std::fs::read(dir.join(&rel)).map_err(|e| format!("{rel}: {e}"))?;
            zip.start_file(&rel, options)
                .map_err(|e| format!("zip {rel}: {e}"))?;
            zip.write_all(&bytes)
                .map_err(|e| format!("zip {rel}: {e}"))?;
        }
        zip.finish().map_err(|e| format!("zip: {e}"))?;
    }
    Ok(buf.into_inner())
}

/// `(public_hex, signature_hex)` over `bytes` by the seed `key_hex`.
fn sign(bytes: &[u8], key_hex: &str) -> Result<(String, String), String> {
    let seed: [u8; 32] = hex::decode(key_hex.trim())
        .map_err(|_| String::from("signing key is not hex"))?
        .try_into()
        .map_err(|_| String::from("signing key must be 32 bytes (64 hex characters)"))?;
    let signing = SigningKey::from_bytes(&seed);
    Ok((
        hex::encode(signing.verifying_key().to_bytes()),
        hex::encode(signing.sign(bytes).to_bytes()),
    ))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') || name == "README.md" {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../themes-starter/blog");

    #[test]
    fn the_blog_starter_packs_into_a_valid_theme() {
        let bytes = pack_bytes(Path::new(BLOG)).expect("pack");
        let parsed = vyasa_themes::package::parse_vytheme(&bytes).expect("valid theme");
        assert_eq!(parsed.manifest.name, "blog");
    }

    #[test]
    fn a_signed_pack_verifies_under_its_public_key() {
        let bytes = pack_bytes(Path::new(BLOG)).expect("pack");
        let (public, signature) = sign(&bytes, &"07".repeat(32)).expect("sign");
        crate::signing::verify_signature(&bytes, &signature, &[public]).expect("verifies");
    }
}
