//! One place that decides whether downloaded bytes may be trusted.
//!
//! Core releases, marketplace plugins and marketplace themes all arrive
//! the same way — over https, from a host that is not the authority —
//! so they are all checked the same way: a mandatory SHA-256 against the
//! digest the signed index published, and an ed25519 signature over the
//! bytes when the source signs them.
//!
//! The policy that matters is in [`verify_download`]: **silence is never
//! trust**. Once an install lists trusted keys, an unsigned artifact is
//! refused rather than waved through; and a signed artifact is refused
//! when no key is configured to check it against. Anything else lets an
//! attacker downgrade security by simply omitting a field.

use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use sha2::{Digest as _, Sha256};
use vyasa_common::AppError;

/// Checks `bytes` against a published digest and (when either side
/// offers one) a signature.
///
/// # Errors
/// [`AppError::Validation`] on a digest mismatch, a bad signature, or a
/// trust configuration the artifact does not satisfy.
pub fn verify_download(
    bytes: &[u8],
    sha256_hex: &str,
    signature_hex: Option<&str>,
    trusted_keys: &[String],
) -> Result<(), AppError> {
    let digest = hex::encode(Sha256::digest(bytes));
    if !digest.eq_ignore_ascii_case(sha256_hex) {
        return Err(AppError::validation(format!(
            "checksum mismatch: expected {sha256_hex}, got {digest}"
        )));
    }
    match signature_hex {
        None if trusted_keys.is_empty() => Ok(()),
        None => Err(AppError::validation(
            "this download is unsigned but the install trusts only signed artifacts",
        )),
        Some(_) if trusted_keys.is_empty() => Err(AppError::validation(
            "this download is signed but no trusted key is configured to check it",
        )),
        Some(signature) => verify_signature(bytes, signature, trusted_keys),
    }
}

/// Hex keys to verifying keys, skipping anything that is not one.
#[must_use]
pub fn parse_keys(raws: &[String]) -> Vec<VerifyingKey> {
    raws.iter()
        .filter_map(|raw| {
            let bytes = hex::decode(raw.trim()).ok()?;
            let arr: [u8; 32] = bytes.as_slice().try_into().ok()?;
            VerifyingKey::from_bytes(&arr).ok()
        })
        .collect()
}

/// [`verify_download`] for a marketplace package, where silence is never
/// trust *for code*.
///
/// A package that can run code in a visitor's browser or on the server —
/// every plugin, and any theme shipping `assets/theme.js` or a template
/// with script in it — must carry a signature from a trusted key, even on
/// an install that has configured none (it is then refused). Only a theme
/// that is pure data may install unsigned, and only when it came over
/// https and matched the digest the index published.
///
/// # Errors
/// [`AppError::Validation`] on a digest mismatch, a bad signature, an
/// unsigned package that carries code, or an unsigned download not
/// fetched over https.
pub fn verify_registry_download(
    bytes: &[u8],
    sha256_hex: &str,
    signature_hex: Option<&str>,
    trusted_keys: &[String],
    url: &str,
) -> Result<(), AppError> {
    verify_download(bytes, sha256_hex, signature_hex, trusted_keys)?;
    if signature_hex.is_some() {
        // `verify_download` refused a signature it could not check, so
        // reaching here means a trusted key vouched for these bytes.
        return Ok(());
    }
    if !is_code_free_theme(bytes) {
        return Err(AppError::validation(
            "this marketplace package carries code (a plugin, or a theme with a script) \
             and is unsigned; every such package must be signed by the marketplace key",
        ));
    }
    if !url.starts_with("https://") {
        return Err(AppError::validation(
            "an unsigned theme is only accepted when downloaded over https",
        ));
    }
    Ok(())
}

/// The rule for a theme an administrator uploads by hand, the same one
/// the marketplace applies: a theme that is only data needs nothing; a
/// theme that can run script in visitors' browsers needs a signature over
/// the exact package bytes from a trusted key.
///
/// # Errors
/// [`AppError::Validation`] when a scripted theme is unsigned or signed by
/// a key not in `keys`.
pub fn verify_theme_upload(
    bytes: &[u8],
    signature_hex: Option<&str>,
    keys: &[String],
) -> Result<(), AppError> {
    if is_code_free_theme(bytes) {
        return Ok(());
    }
    let refuse = || {
        AppError::validation(
            "this theme carries a script (assets/theme.js or a template that writes \
             one), so it must be signed by the official marketplace key or a key in \
             package_trusted_keys",
        )
    };
    let signature = signature_hex.ok_or_else(refuse)?;
    verify_signature(bytes, signature.trim(), keys).map_err(|_| refuse())
}

/// Whether `bytes` are a valid theme package that cannot run code: no
/// `assets/theme.js`, and no template that writes a script tag, a
/// `javascript:` URL or an inline event handler.
///
/// Anything that does not parse as a theme — a plugin included — is not
/// code-free.
#[must_use]
pub fn is_code_free_theme(bytes: &[u8]) -> bool {
    let Ok(parsed) = vyasa_themes::package::parse_vytheme(bytes) else {
        return false;
    };
    let has_js = parsed
        .assets_json
        .as_ref()
        .and_then(|a| a.get("js"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|js| !js.trim().is_empty());
    !has_js && !parsed.templates.values().any(|t| template_has_script(t))
}

/// A conservative scan: false positives cost an unsigned theme its
/// install, false negatives would cost a visitor.
fn template_has_script(src: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    if lower.contains("<script") || lower.contains("javascript:") {
        return true;
    }
    // ` onclick=`, `\tonload =`, ... : whitespace, `on`, letters, `=`.
    let b = lower.as_bytes();
    let mut i = 0;
    while let Some(off) = lower[i..].find("on") {
        let at = i + off;
        let preceded = at > 0 && (b[at - 1].is_ascii_whitespace() || b[at - 1] == b'/');
        let mut j = at + 2;
        while j < b.len() && b[j].is_ascii_lowercase() {
            j += 1;
        }
        let named = j > at + 2;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if preceded && named && b.get(j) == Some(&b'=') {
            return true;
        }
        i = at + 2;
    }
    false
}

/// ed25519 over the raw bytes, against any of the trusted public keys.
///
/// # Errors
/// [`AppError::Validation`] when the signature is malformed or matches
/// no configured key.
pub fn verify_signature(
    bytes: &[u8],
    signature_hex: &str,
    keys: &[String],
) -> Result<(), AppError> {
    let raw =
        hex::decode(signature_hex).map_err(|_| AppError::validation("signature is not hex"))?;
    let signature =
        Signature::from_slice(&raw).map_err(|_| AppError::validation("signature is malformed"))?;
    for key in keys {
        let Ok(key_bytes) = hex::decode(key) else {
            continue;
        };
        let Ok(array) = <[u8; 32]>::try_from(key_bytes.as_slice()) else {
            continue;
        };
        let Ok(verifying) = VerifyingKey::from_bytes(&array) else {
            continue;
        };
        if verifying.verify(bytes, &signature).is_ok() {
            return Ok(());
        }
    }
    Err(AppError::validation(
        "signature does not match any trusted key",
    ))
}

/// Hex SHA-256 of `bytes`. Test-only today; the production paths take
/// digests from a signed index rather than computing their own.
#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::{sha256_hex, verify_download, verify_registry_download};

    fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write as _;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            for (name, data) in entries {
                w.start_file((*name).to_owned(), zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(data.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    /// The technology starter (no template) plus whatever `extra` adds.
    fn theme(extra: &[(&str, &str)]) -> Vec<u8> {
        let mut entries = vec![
            (
                "manifest.toml",
                include_str!("../../../themes-starter/technology/manifest.toml"),
            ),
            (
                "tokens.json",
                include_str!("../../../themes-starter/technology/tokens.json"),
            ),
            (
                "layout.json",
                include_str!("../../../themes-starter/technology/layout.json"),
            ),
        ];
        entries.extend_from_slice(extra);
        zip(&entries)
    }

    #[test]
    fn a_data_only_theme_uploads_unsigned() {
        assert!(super::verify_theme_upload(&theme(&[]), None, &[]).is_ok());
    }

    #[test]
    fn scripted_theme_unsigned_is_refused() {
        let err = super::verify_theme_upload(&theme(&[("assets/theme.js", "x()")]), None, &[])
            .unwrap_err();
        assert!(err.to_string().contains("carries a script"), "{err}");
    }

    #[test]
    fn scripted_theme_signed_by_unknown_key_is_refused() {
        use ed25519_dalek::{Signer as _, SigningKey};
        let bytes = theme(&[("assets/theme.js", "x()")]);
        let sig = hex::encode(SigningKey::from_bytes(&[7; 32]).sign(&bytes).to_bytes());
        let trusted = hex::encode(SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes());
        let err = super::verify_theme_upload(&bytes, Some(&sig), &[trusted]).unwrap_err();
        assert!(err.to_string().contains("carries a script"), "{err}");
    }

    #[test]
    fn scripted_theme_signed_by_a_trusted_key_uploads() {
        use ed25519_dalek::{Signer as _, SigningKey};
        let bytes = theme(&[("assets/theme.js", "x()")]);
        let key = SigningKey::from_bytes(&[9; 32]);
        let sig = hex::encode(key.sign(&bytes).to_bytes());
        let trusted = hex::encode(key.verifying_key().to_bytes());
        assert!(super::verify_theme_upload(&bytes, Some(&sig), &[trusted]).is_ok());
    }

    #[test]
    fn unsigned_code_is_refused_even_with_no_keys_configured() {
        let url = "https://registry.example/p.vytheme";
        // A theme that is pure data may install unsigned over https.
        let plain = theme(&[("assets/theme.css", "body{color:red}")]);
        assert!(super::is_code_free_theme(&plain));
        assert!(verify_registry_download(&plain, &sha256_hex(&plain), None, &[], url).is_ok());
        // ...but not over plain http.
        assert!(verify_registry_download(
            &plain,
            &sha256_hex(&plain),
            None,
            &[],
            "http://registry.example/p.vytheme"
        )
        .is_err());
        // ...nor with a wrong digest.
        assert!(verify_registry_download(&plain, &"0".repeat(64), None, &[], url).is_err());

        // A theme with a script does not: silence is never trust.
        let scripted = theme(&[("assets/theme.js", "fetch('/api/v1/users')")]);
        assert!(!super::is_code_free_theme(&scripted));
        let err = verify_registry_download(&scripted, &sha256_hex(&scripted), None, &[], url)
            .unwrap_err()
            .to_string();
        assert!(err.contains("carries code"), "{err}");

        // Nor does anything that is not a theme — a plugin package.
        let plugin = zip(&[("manifest.toml", "name = \"p\""), ("plugin.wasm", "\0asm")]);
        assert!(verify_registry_download(&plugin, &sha256_hex(&plugin), None, &[], url).is_err());
    }

    #[test]
    fn signed_code_from_a_trusted_key_is_accepted() {
        use ed25519_dalek::{Signer as _, SigningKey};
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let public = hex::encode(key.verifying_key().to_bytes());
        let scripted = theme(&[("assets/theme.js", "console.log(1)")]);
        let signature = hex::encode(key.sign(&scripted).to_bytes());
        assert!(verify_registry_download(
            &scripted,
            &sha256_hex(&scripted),
            Some(&signature),
            &[public],
            "https://registry.example/p.vytheme",
        )
        .is_ok());
    }

    #[test]
    fn templates_that_write_script_count_as_code() {
        for bad in [
            "<SCRIPT>alert(1)</SCRIPT>",
            "<a href=\"javascript:alert(1)\">x</a>",
            "<img src=x onerror=alert(1)>",
            "<body\tonload = \"x()\">",
            "<svg/onload=alert(1)>",
        ] {
            assert!(super::template_has_script(bad), "{bad}");
        }
        for fine in [
            "<p>{{ post.title }}</p>",
            "<p>common onto online</p>",
            "<div class=\"button\">online</div>",
        ] {
            assert!(!super::template_has_script(fine), "{fine}");
        }
    }

    #[test]
    fn a_wrong_digest_is_refused_however_it_is_signed() {
        assert!(verify_download(b"payload", &"0".repeat(64), None, &[]).is_err());
    }

    #[test]
    fn silence_is_never_trust_in_either_direction() {
        let bytes = b"payload";
        let digest = sha256_hex(bytes);
        // No keys configured and nothing signed: an operator who has not
        // opted into signing may still install.
        assert!(verify_download(bytes, &digest, None, &[]).is_ok());
        // Keys configured, artifact unsigned: refused.
        assert!(verify_download(bytes, &digest, None, &["aa".repeat(32)]).is_err());
        // Artifact signed, no key to check it with: refused.
        assert!(verify_download(bytes, &digest, Some(&"ab".repeat(64)), &[]).is_err());
        // Signed by a key that is not trusted: refused.
        assert!(
            verify_download(bytes, &digest, Some(&"ab".repeat(64)), &["aa".repeat(32)]).is_err()
        );
    }

    #[test]
    fn a_real_signature_from_a_trusted_key_passes() {
        use ed25519_dalek::{Signer as _, SigningKey};
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let bytes = b"payload";
        let signature = hex::encode(key.sign(bytes).to_bytes());
        let public = hex::encode(key.verifying_key().to_bytes());
        assert!(verify_download(bytes, &sha256_hex(bytes), Some(&signature), &[public]).is_ok());
    }
}
