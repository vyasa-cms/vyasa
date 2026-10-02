//! Password hashing and verification (argon2id).
//!
//! Hashes use the default Argon2id parameters (19 MiB memory, 2
//! iterations, parallelism 1) — the OWASP-recommended baseline.

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;

use vyasa_common::AppError;

/// The shortest password any path that sets one accepts.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// The one password rule, shared by every path that sets a password.
///
/// # Errors
///
/// Returns [`AppError::Validation`] for a password shorter than
/// [`MIN_PASSWORD_LENGTH`].
pub fn validate_password(password: &str) -> Result<(), AppError> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(AppError::validation(
            "password must be at least 8 characters",
        ));
    }
    Ok(())
}

/// A pre-computed argon2id hash of an unguessable random string: verifying
/// against it costs what verifying against a real hash costs, which keeps
/// "no such account" and "no password on it" as slow as "wrong password".
pub(crate) const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$OXR4ZXZtcG1vaGt3aGpsdA$k4LtC1a2hEm8i0eSqiKdZYBSkNUPzxJZU8DRnXBJylY";

/// Hashes `password` with argon2id and a random salt.
///
/// # Errors
///
/// Returns [`AppError::Internal`] when the OS entropy source or hashing
/// fails (both are practically impossible).
pub fn hash_password(password: &str) -> Result<String, AppError> {
    // password-hash 0.6 draws the salt from the OS itself.
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::internal_msg(format!("password hashing failed: {err}")))
}

/// Verifies `password` against an argon2 hash string.
///
/// A malformed stored hash counts as a failed verification, not an error.
///
/// # Errors
///
/// Returns [`AppError::Auth`] when the password does not match.
pub fn verify_password(password: &str, hash: &str) -> Result<(), AppError> {
    let parsed = PasswordHash::new(hash);
    let ok = parsed.is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    });
    if ok {
        Ok(())
    } else {
        Err(AppError::auth(super::auth::INVALID_CREDENTIALS))
    }
}

#[cfg(test)]
mod tests {
    use super::{hash_password, verify_password};
    use vyasa_common::AppError;

    #[test]
    fn hash_and_verify_round_trip() {
        let hash = hash_password("correct horse battery staple").expect("hash");
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &hash).is_ok());
    }

    #[test]
    fn wrong_password_fails() {
        let hash = hash_password("hunter2").expect("hash");
        assert!(matches!(
            verify_password("hunter3", &hash),
            Err(AppError::Auth { .. })
        ));
    }

    #[test]
    fn malformed_hash_fails_closed() {
        assert!(matches!(
            verify_password("anything", "not-a-hash"),
            Err(AppError::Auth { .. })
        ));
    }

    #[test]
    fn hashes_are_salted_uniquely() {
        let a = hash_password("same").expect("hash a");
        let b = hash_password("same").expect("hash b");
        assert_ne!(a, b, "same password must salt differently");
    }
}
