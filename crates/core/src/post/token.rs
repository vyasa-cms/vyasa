//! Signed read tokens for private posts.
//!
//! Token format: `base64url(payload).base64url(sig)` where payload is
//! `post_id:exp` and sig is HMAC-SHA256(payload, secret). Expiry is
//! 15 minutes by default (caller chooses TTL).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use vyasa_common::AppError;

type HmacSha256 = Hmac<Sha256>;

/// Default TTL for private-post read tokens (15 minutes).
pub const DEFAULT_TTL_SECS: u64 = 15 * 60;

/// Default TTL for preview tokens (24 hours).
pub const PREVIEW_TTL_SECS: u64 = 24 * 60 * 60;

/// Generates a signed token bound to `post_id`.
///
/// # Errors
///
/// Returns [`AppError::Internal`] if HMAC initialization fails (should not
/// happen).
pub fn generate_token(post_id: i64, secret: &[u8], ttl_secs: u64) -> Result<String, AppError> {
    let exp = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0) + ttl_secs;
    let payload = format!("{post_id}:{exp}");
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|err| AppError::internal_msg(format!("hmac init failed: {err}")))?;
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    Ok(format!("{payload_b64}.{sig_b64}"))
}

/// Verifies a token for `expected_post_id`.
///
/// # Errors
///
/// Returns [`AppError::Auth`] when the token is malformed, signature
/// invalid, expired, or bound to a different post.
pub fn verify_token(token: &str, secret: &[u8], expected_post_id: i64) -> Result<(), AppError> {
    let (payload_b64, sig_b64) = token
        .split_once('.')
        .ok_or_else(|| AppError::auth("invalid token format"))?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| AppError::auth("invalid token encoding"))?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| AppError::auth("invalid token signature encoding"))?;
    let payload =
        String::from_utf8(payload_bytes).map_err(|_| AppError::auth("invalid token payload"))?;
    let (post_id_str, exp_str) = payload
        .split_once(':')
        .ok_or_else(|| AppError::auth("invalid token payload"))?;
    let post_id: i64 = post_id_str
        .parse()
        .map_err(|_| AppError::auth("invalid token post id"))?;
    if post_id != expected_post_id {
        return Err(AppError::auth("token post mismatch"));
    }
    let exp: i64 = exp_str
        .parse()
        .map_err(|_| AppError::auth("invalid token expiry"))?;
    let now = chrono::Utc::now().timestamp();
    if now > exp {
        return Err(AppError::auth("token expired"));
    }
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|err| AppError::internal_msg(format!("hmac init failed: {err}")))?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&sig_bytes)
        .map_err(|_| AppError::auth("invalid token signature"))?;
    Ok(())
}

/// Generates a signed preview token bound to `post_id`.
///
/// Uses a `preview:` prefix to avoid confusion with private read tokens.
///
/// # Errors
///
/// Returns [`AppError::Internal`] if HMAC initialization fails.
pub fn generate_preview_token(post_id: i64, secret: &[u8]) -> Result<String, AppError> {
    generate_token_with_prefix("preview", post_id, secret, PREVIEW_TTL_SECS)
}

/// Verifies a preview token for `expected_post_id`.
///
/// # Errors
///
/// Returns [`AppError::Auth`] when the token is malformed, signature
/// invalid, expired, or bound to a different post.
pub fn verify_preview_token(
    token: &str,
    secret: &[u8],
    expected_post_id: i64,
) -> Result<(), AppError> {
    verify_token_with_prefix("preview", token, secret, expected_post_id)
}

fn generate_token_with_prefix(
    prefix: &str,
    post_id: i64,
    secret: &[u8],
    ttl_secs: u64,
) -> Result<String, AppError> {
    let exp = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0) + ttl_secs;
    let payload = format!("{prefix}:{post_id}:{exp}");
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|err| AppError::internal_msg(format!("hmac init failed: {err}")))?;
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    Ok(format!("{payload_b64}.{sig_b64}"))
}

fn verify_token_with_prefix(
    prefix: &str,
    token: &str,
    secret: &[u8],
    expected_post_id: i64,
) -> Result<(), AppError> {
    let (payload_b64, sig_b64) = token
        .split_once('.')
        .ok_or_else(|| AppError::auth("invalid token format"))?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| AppError::auth("invalid token encoding"))?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| AppError::auth("invalid token signature encoding"))?;
    let payload =
        String::from_utf8(payload_bytes).map_err(|_| AppError::auth("invalid token payload"))?;
    // Expected: "prefix:post_id:exp"
    let mut parts = payload.splitn(3, ':');
    let got_prefix = parts
        .next()
        .ok_or_else(|| AppError::auth("invalid token payload"))?;
    let post_id_str = parts
        .next()
        .ok_or_else(|| AppError::auth("invalid token payload"))?;
    let exp_str = parts
        .next()
        .ok_or_else(|| AppError::auth("invalid token payload"))?;
    if got_prefix != prefix {
        return Err(AppError::auth("invalid token prefix"));
    }
    let post_id: i64 = post_id_str
        .parse()
        .map_err(|_| AppError::auth("invalid token post id"))?;
    if post_id != expected_post_id {
        return Err(AppError::auth("token post mismatch"));
    }
    let exp: i64 = exp_str
        .parse()
        .map_err(|_| AppError::auth("invalid token expiry"))?;
    let now = chrono::Utc::now().timestamp();
    if now > exp {
        return Err(AppError::auth("token expired"));
    }
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|err| AppError::internal_msg(format!("hmac init failed: {err}")))?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&sig_bytes)
        .map_err(|_| AppError::auth("invalid token signature"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        generate_preview_token, generate_token, verify_preview_token, verify_token,
        DEFAULT_TTL_SECS,
    };

    #[test]
    fn round_trip() {
        let secret = b"test-secret-32-bytes-long-for-hmac!!";
        let post_id = 12345;
        let token = generate_token(post_id, secret, DEFAULT_TTL_SECS).expect("generate");
        verify_token(&token, secret, post_id).expect("verify");
    }

    #[test]
    fn wrong_post_id_fails() {
        let secret = b"test-secret-32-bytes-long-for-hmac!!";
        let token = generate_token(1, secret, 3600).expect("generate");
        assert!(verify_token(&token, secret, 2).is_err());
    }

    #[test]
    fn expired_fails() {
        let secret = b"test-secret-32-bytes-long-for-hmac!!";
        // TTL 0 means exp = now, but immediate verify may still pass if clock hasn't advanced; use past exp
        // Generate with 0 then sleep? Instead craft payload with past exp.
        let token = generate_token(1, secret, 0).expect("generate");
        // Sleep 1 sec to ensure expiry
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(verify_token(&token, secret, 1).is_err());
    }

    #[test]
    fn tampered_fails() {
        let secret = b"test-secret-32-bytes-long-for-hmac!!";
        let mut token = generate_token(1, secret, 3600).expect("generate");
        token.push('x');
        assert!(verify_token(&token, secret, 1).is_err());
    }

    #[test]
    fn wrong_secret_fails() {
        let secret = b"test-secret-32-bytes-long-for-hmac!!";
        let other = b"other-secret-32-bytes-long-for-hmac!!";
        let token = generate_token(1, secret, 3600).expect("generate");
        assert!(verify_token(&token, other, 1).is_err());
    }

    #[test]
    fn preview_round_trip() {
        let secret = b"test-secret-32-bytes-long-for-hmac!!";
        let token = generate_preview_token(42, secret).expect("generate");
        verify_preview_token(&token, secret, 42).expect("verify");
    }

    #[test]
    fn preview_cannot_be_used_as_private() {
        let secret = b"test-secret-32-bytes-long-for-hmac!!";
        let preview = generate_preview_token(1, secret).expect("generate");
        assert!(verify_token(&preview, secret, 1).is_err());
        let private = generate_token(1, secret, 3600).expect("generate");
        assert!(verify_preview_token(&private, secret, 1).is_err());
    }
}
