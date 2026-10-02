//! Provider keys at rest.
//!
//! With a server secret (`VYASA_SECRET_KEY`) keys are sealed with
//! ChaCha20-Poly1305 under a key derived from it; without one they are
//! stored as they are. The stored string says which (`v1:` or `plain:`),
//! so a deployment that adds a secret later keeps working, and the admin
//! page can tell the operator which state they are in.

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore as _;
use sha2::{Digest, Sha256};
use vyasa_common::AppError;

const SEALED: &str = "v1:";
const PLAIN: &str = "plain:";
const DERIVE_LABEL: &[u8] = b"vyasa/ai-provider-keys/v1";

/// Seals and opens stored credentials.
#[derive(Clone)]
pub struct KeyVault {
    cipher: Option<ChaCha20Poly1305>,
}

impl std::fmt::Debug for KeyVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyVault")
            .field("encrypting", &self.cipher.is_some())
            .finish()
    }
}

impl KeyVault {
    /// A vault keyed from the server secret, or a pass-through vault when
    /// there is none.
    #[must_use]
    pub fn new(server_secret: Option<&[u8]>) -> Self {
        let cipher = server_secret.filter(|s| !s.is_empty()).map(|secret| {
            let mut hasher = Sha256::new();
            hasher.update(DERIVE_LABEL);
            hasher.update(secret);
            let derived = hasher.finalize();
            ChaCha20Poly1305::new(&Key::from(derived))
        });
        Self { cipher }
    }

    /// Whether stored keys are encrypted.
    #[must_use]
    pub fn encrypting(&self) -> bool {
        self.cipher.is_some()
    }

    /// Prepares a key for storage.
    ///
    /// # Errors
    /// Cipher failure (should not happen).
    pub fn seal(&self, plaintext: &str) -> Result<String, AppError> {
        let Some(cipher) = &self.cipher else {
            return Ok(format!("{PLAIN}{plaintext}"));
        };
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let sealed = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| AppError::internal_msg("could not seal provider key"))?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&sealed);
        Ok(format!(
            "{SEALED}{}",
            base64::engine::general_purpose::STANDARD.encode(blob)
        ))
    }

    /// Recovers a stored key.
    ///
    /// # Errors
    /// A sealed key with no (or a different) server secret, or a corrupt
    /// value.
    pub fn open(&self, stored: &str) -> Result<String, AppError> {
        if let Some(plain) = stored.strip_prefix(PLAIN) {
            return Ok(plain.to_owned());
        }
        let Some(encoded) = stored.strip_prefix(SEALED) else {
            // Pre-vault rows, if any, are plain.
            return Ok(stored.to_owned());
        };
        let Some(cipher) = &self.cipher else {
            return Err(AppError::validation(
                "this provider key is encrypted; set VYASA_SECRET_KEY to the value it was \
                 saved with, or enter the key again",
            ));
        };
        let blob = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| AppError::validation("stored provider key is corrupt"))?;
        if blob.len() < 12 {
            return Err(AppError::validation("stored provider key is corrupt"));
        }
        let (nonce, sealed) = blob.split_at(12);
        let plain = cipher
            .decrypt(
                &Nonce::try_from(nonce).map_err(|_| AppError::validation("bad nonce"))?,
                sealed,
            )
            .map_err(|_| {
                AppError::validation(
                    "this provider key was encrypted with a different VYASA_SECRET_KEY; \
                     enter the key again",
                )
            })?;
        String::from_utf8(plain).map_err(|_| AppError::validation("stored provider key is corrupt"))
    }

    /// Whether a stored value is sealed.
    #[must_use]
    pub fn is_sealed(stored: &str) -> bool {
        stored.starts_with(SEALED)
    }
}

/// The last four characters, for "sk-…abcd" displays. Never more.
#[must_use]
pub fn hint(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if key.chars().count() <= 4 {
        return "••••".to_owned();
    }
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_keys_round_trip_and_are_not_readable_without_the_secret() {
        let vault = KeyVault::new(Some(b"server-secret"));
        let stored = vault.seal("sk-live-1234").unwrap();
        assert!(KeyVault::is_sealed(&stored));
        assert!(!stored.contains("sk-live"));
        assert_eq!(vault.open(&stored).unwrap(), "sk-live-1234");

        let other = KeyVault::new(Some(b"another-secret"));
        assert!(other.open(&stored).is_err());
        let none = KeyVault::new(None);
        assert!(none.open(&stored).is_err());
    }

    #[test]
    fn without_a_secret_keys_are_stored_plain_and_still_open() {
        let vault = KeyVault::new(None);
        assert!(!vault.encrypting());
        let stored = vault.seal("sk-1").unwrap();
        assert_eq!(stored, "plain:sk-1");
        assert_eq!(vault.open(&stored).unwrap(), "sk-1");
        // A secret added later still opens plain rows.
        assert_eq!(KeyVault::new(Some(b"x")).open(&stored).unwrap(), "sk-1");
    }

    #[test]
    fn hints_show_only_the_tail() {
        assert_eq!(hint("sk-abcdef1234"), "…1234");
        assert_eq!(hint("abc"), "••••");
    }
}
