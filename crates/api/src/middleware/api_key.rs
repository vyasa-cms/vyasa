//! API-key authentication: `Authorization: Bearer vy_...` for headless
//! consumers. Only the SHA-256 of the key is stored; capabilities are
//! granted per key and intersected with the owner's.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use sha2::{Digest, Sha256};

use vyasa_common::AppError;
use vyasa_db::models::ApiKeyRow;

use crate::state::AppState;

/// The authenticated principal from an API key.
#[derive(Clone, Debug)]
pub struct ApiKeyPrincipal {
    /// The api_keys row.
    pub key: ApiKeyRow,
    /// The owning user row.
    pub user: vyasa_db::models::UserRow,
}
impl ApiKeyPrincipal {
    /// Hashes a raw key for lookup (storage stores this hash).
    #[must_use]
    pub fn hash_raw(raw: &str) -> String {
        let digest = Sha256::digest(raw.as_bytes());
        hex_encode(&digest)
    }

    /// Whether this key grants `cap` (intersected with owner's caps).
    #[must_use]
    pub fn can(&self, cap: vyasa_core::user::Capability) -> bool {
        self.grants(cap) && vyasa_core::user::can(&self.user, cap)
    }

    /// Whether the key itself declares `cap`.
    #[must_use]
    pub fn grants(&self, cap: vyasa_core::user::Capability) -> bool {
        let name = vyasa_core::user::cap_name(cap);
        self.key
            .capabilities
            .as_array()
            .is_some_and(|caps| caps.iter().any(|v| v.as_str() == Some(name)))
    }

    /// Ensures the key (and its owner) grants `cap`.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Forbidden`] otherwise.
    pub fn ensure(&self, cap: vyasa_core::user::Capability) -> Result<(), AppError> {
        if self.can(cap) {
            Ok(())
        } else {
            Err(AppError::forbidden(format!(
                "this api key does not grant the {} capability",
                vyasa_core::user::cap_name(cap)
            )))
        }
    }
}

/// Extractor: authenticates via `Authorization: Bearer <raw key>`.
///
/// Resolves the key hash through the db repo and intersects the key's
/// grants with the owner's effective capabilities.
impl FromRequestParts<AppState> for ApiKeyPrincipal {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = bearer_token(parts)
            .ok_or_else(|| AppError::auth("api key required (Authorization: Bearer vy_...)"))?;
        let (key, user) = state
            .api_keys
            .resolve(&Self::hash_raw(raw))
            .await
            .map_err(|_| AppError::auth("invalid or revoked api key"))?;
        Ok(ApiKeyPrincipal { key, user })
    }
}

/// Pulls the bearer token out of the Authorization header.
fn bearer_token(parts: &Parts) -> Option<&str> {
    let header = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let rest = header.strip_prefix("Bearer ")?;
    (!rest.is_empty()).then_some(rest)
}

/// Lowercase hex encoding of a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{bearer_token, hex_encode};

    #[test]
    fn hex_encoding() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0x10]), "00ff10");
    }

    #[test]
    fn bearer_token_extraction() {
        let mut parts = {
            let request: axum::http::Request<()> = axum::http::Request::default();
            let (parts, ()) = request.into_parts();
            parts
        };
        parts.headers.insert(
            "authorization",
            "Bearer vy_abc123".parse().expect("valid header"),
        );
        assert_eq!(bearer_token(&parts), Some("vy_abc123"));

        parts.headers.insert(
            "authorization",
            "Basic dXNlcjpwYXNz".parse().expect("valid header"),
        );
        assert_eq!(bearer_token(&parts), None);
    }
}
