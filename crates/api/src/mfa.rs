//! A second factor: time-based one-time codes (RFC 6238) with recovery
//! codes. The secret is sealed with the server secret like provider keys;
//! recovery codes are hashed and spent on use. A sign-in that clears the
//! password but has a second factor pending gets a short signed challenge
//! instead of a session.

use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};
use vyasa_common::AppError;
use vyasa_core::ai_models::KeyVault;

use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

const CHALLENGE_TTL_SECS: i64 = 300;
const RECOVERY_CODES: usize = 8;

fn vault(state: &AppState) -> KeyVault {
    KeyVault::new(
        state
            .config
            .secret_key
            .as_ref()
            .map(|k| k.expose().as_bytes()),
    )
}

async fn issuer(state: &AppState) -> String {
    state
        .options_service
        .site_identity()
        .await
        .ok()
        .map(|i| i.title)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| String::from("Vyasa"))
}

/// Where an account stands.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MfaStatus {
    /// A second factor is required at sign-in.
    pub enabled: bool,
    /// Setup was started but the first code not yet confirmed.
    pub pending: bool,
    /// Unused recovery codes left.
    pub recovery_codes_left: usize,
}

/// What setup hands back: scan or type, then confirm with a code.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MfaSetup {
    /// The otpauth URL an authenticator app understands.
    pub otpauth_url: String,
    /// The same URL as an SVG QR code.
    pub qr_svg: String,
    /// The base32 secret, for typing by hand.
    pub secret: String,
}

/// Recovery codes, shown once.
#[derive(Serialize, utoipa::ToSchema)]
pub struct RecoveryCodes {
    pub codes: Vec<String>,
}

struct Row {
    secret: String,
    enabled: bool,
    codes: Vec<String>,
}

async fn row(state: &AppState, user_id: i64) -> Result<Option<Row>, AppError> {
    let r: Option<(
        String,
        Option<chrono::DateTime<chrono::Utc>>,
        serde_json::Value,
    )> = sqlx::query_as(
        "SELECT secret, enabled_at, recovery_codes FROM user_mfa WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("mfa: {e}")))?;
    Ok(r.map(|(secret, enabled_at, codes)| Row {
        secret,
        enabled: enabled_at.is_some(),
        codes: codes
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    }))
}

/// Whether sign-in must ask for a code.
///
/// # Errors
/// Database errors must abort sign-in rather than bypass the second factor.
pub async fn enabled(state: &AppState, user_id: i64) -> Result<bool, AppError> {
    Ok(row(state, user_id).await?.is_some_and(|r| r.enabled))
}

/// Every user id with a second factor on, for the users list.
pub async fn enabled_ids(state: &AppState) -> Vec<i64> {
    sqlx::query_scalar::<_, i64>("SELECT user_id FROM user_mfa WHERE enabled_at IS NOT NULL")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
}

/// # Errors
/// Database errors.
pub async fn status(state: &AppState, user_id: i64) -> Result<MfaStatus, AppError> {
    Ok(match row(state, user_id).await? {
        None => MfaStatus {
            enabled: false,
            pending: false,
            recovery_codes_left: 0,
        },
        Some(r) => MfaStatus {
            enabled: r.enabled,
            pending: !r.enabled,
            recovery_codes_left: r.codes.len(),
        },
    })
}

async fn totp_for(
    state: &AppState,
    secret_b32: &str,
    account: &str,
) -> Result<totp_rs::Totp, AppError> {
    let secret = totp_rs::Secret::try_from_base32(secret_b32)
        .map_err(|e| AppError::internal_msg(format!("totp secret: {e}")))?;
    totp_rs::Builder::new()
        .with_algorithm(totp_rs::Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(secret)
        .with_account_name(account)
        .with_issuer(Some(issuer(state).await))
        .build()
        .map_err(|e| AppError::internal_msg(format!("totp: {e}")))
}

/// Starts (or restarts) setup: a fresh secret, not yet enabled.
///
/// # Errors
/// Database or cipher errors.
pub async fn begin(state: &AppState, user_id: i64, email: &str) -> Result<MfaSetup, AppError> {
    let b32 = totp_rs::Secret::generate().to_base32();
    let sealed = vault(state).seal(&b32)?;
    let result = sqlx::query(
        "INSERT INTO user_mfa (user_id, secret, enabled_at, recovery_codes)
         VALUES ($1, $2, NULL, '[]'::jsonb)
         ON CONFLICT (user_id) DO UPDATE SET secret = EXCLUDED.secret, enabled_at = NULL, recovery_codes = '[]'::jsonb WHERE user_mfa.enabled_at IS NULL",
    )
    .bind(user_id)
    .bind(&sealed)
    .execute(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("mfa begin: {e}")))?;
    if result.rows_affected() == 0 {
        return Err(AppError::validation(
            "disable the existing second factor with a valid code before restarting setup",
        ));
    }
    let totp = totp_for(state, &b32, email).await?;
    let url = totp
        .to_url()
        .map_err(|e| AppError::internal_msg(format!("otpauth: {e}")))?;
    let qr_svg = qrcode::QrCode::new(url.as_bytes())
        .map(|code| {
            code.render::<qrcode::render::svg::Color>()
                .min_dimensions(180, 180)
                .quiet_zone(true)
                .build()
        })
        .unwrap_or_default();
    Ok(MfaSetup {
        otpauth_url: url,
        qr_svg,
        secret: b32,
    })
}

fn open_secret(state: &AppState, sealed: &str) -> Result<String, AppError> {
    vault(state).open(sealed)
}

fn hash_code(code: &str) -> String {
    hex::encode(Sha256::digest(
        code.trim().to_ascii_lowercase().replace('-', ""),
    ))
}

fn fresh_codes() -> Vec<String> {
    use rand::RngCore as _;
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut b = [0u8; 5];
            rand::thread_rng().fill_bytes(&mut b);
            let hex = hex::encode(b);
            format!("{}-{}", &hex[..5], &hex[5..])
        })
        .collect()
}

/// Confirms setup with the first code, turns the factor on, and returns
/// the recovery codes once.
///
/// # Errors
/// [`AppError::Validation`] when the code is wrong or setup was not begun.
pub async fn confirm(
    state: &AppState,
    user_id: i64,
    email: &str,
    code: &str,
) -> Result<RecoveryCodes, AppError> {
    let Some(r) = row(state, user_id).await? else {
        return Err(AppError::validation("start setup first"));
    };
    let secret = open_secret(state, &r.secret)?;
    let totp = totp_for(state, &secret, email).await?;
    if totp.check_current(code.trim()).is_none() {
        return Err(AppError::validation(
            "that code is not right; codes change every 30 seconds",
        ));
    }
    let codes = fresh_codes();
    let hashed: Vec<String> = codes.iter().map(|c| hash_code(c)).collect();
    sqlx::query("UPDATE user_mfa SET enabled_at = now(), recovery_codes = $2 WHERE user_id = $1")
        .bind(user_id)
        .bind(serde_json::json!(hashed))
        .execute(&state.pool)
        .await
        .map_err(|e| AppError::db(format!("mfa confirm: {e}")))?;
    Ok(RecoveryCodes { codes })
}

/// Checks a code, or spends a recovery code.
///
/// # Errors
/// [`AppError::Auth`] when neither matches.
pub async fn verify(
    state: &AppState,
    user_id: i64,
    email: &str,
    code: &str,
) -> Result<(), AppError> {
    let Some(r) = row(state, user_id).await? else {
        return Err(AppError::auth("no second factor is set up"));
    };
    let secret = open_secret(state, &r.secret)?;
    if totp_for(state, &secret, email)
        .await?
        .check_current(code.trim())
        .is_some()
    {
        return Ok(());
    }
    let h = hash_code(code);
    // The membership test and removal must happen in one UPDATE. PostgreSQL
    // rechecks the predicate after a concurrent writer commits, so exactly
    // one request can spend a code and other codes cannot be restored.
    let spent = sqlx::query(
        "UPDATE user_mfa SET recovery_codes = recovery_codes - $2::text
         WHERE user_id = $1 AND secret = $3 AND enabled_at IS NOT NULL
           AND recovery_codes ? $2::text",
    )
    .bind(user_id)
    .bind(&h)
    .bind(&r.secret)
    .execute(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("mfa spend: {e}")))?;
    if spent.rows_affected() == 1 {
        return Ok(());
    }
    Err(AppError::auth("that code is not right"))
}

/// Turns the factor off after one last valid code.
///
/// # Errors
/// [`AppError::Auth`] on a wrong code.
pub async fn disable(
    state: &AppState,
    user_id: i64,
    email: &str,
    code: &str,
) -> Result<(), AppError> {
    verify(state, user_id, email, code).await?;
    reset(state, user_id).await
}

/// Removes the factor outright; an administrator's escape hatch.
///
/// # Errors
/// Database errors.
pub async fn reset(state: &AppState, user_id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM user_mfa WHERE user_id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(|e| AppError::db(format!("mfa reset: {e}")))?;
    Ok(())
}

// Keyed by the server's token secret: `VYASA_SECRET_KEY` when set, else
// the random per-boot key (a restart only voids challenges under five
// minutes old). It used to fall back to the constant `vyasa-insecure`, so
// on an install without the variable anyone could sign a challenge for any
// user id and skip the password step entirely.
fn challenge_key(state: &AppState) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(b"vyasa/mfa-challenge/v1");
    h.update(&state.private_token_secret);
    h.finalize().to_vec()
}

/// A short signed token that says "this user passed the password step".
#[must_use]
pub fn challenge(state: &AppState, user_id: i64) -> String {
    let exp = chrono::Utc::now().timestamp() + CHALLENGE_TTL_SECS;
    let payload = format!("{user_id}:{exp}");
    let mut mac = HmacSha256::new_from_slice(&challenge_key(state))
        .unwrap_or_else(|_| unreachable!("any key size"));
    mac.update(payload.as_bytes());
    format!("{payload}:{}", hex::encode(mac.finalize().into_bytes()))
}

/// The user id behind a challenge token, if it is genuine and fresh.
///
/// # Errors
/// [`AppError::Auth`] otherwise.
pub fn open_challenge(state: &AppState, token: &str) -> Result<i64, AppError> {
    let parts: Vec<&str> = token.split(':').collect();
    let [uid, exp, sig] = parts.as_slice() else {
        return Err(AppError::auth("bad challenge"));
    };
    let payload = format!("{uid}:{exp}");
    let mut mac = HmacSha256::new_from_slice(&challenge_key(state))
        .unwrap_or_else(|_| unreachable!("any key size"));
    mac.update(payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());
    if expected.len() != sig.len() || !constant_eq(expected.as_bytes(), sig.as_bytes()) {
        return Err(AppError::auth("bad challenge"));
    }
    let exp: i64 = exp.parse().map_err(|_| AppError::auth("bad challenge"))?;
    if chrono::Utc::now().timestamp() > exp {
        return Err(AppError::auth("that sign-in took too long; start again"));
    }
    uid.parse().map_err(|_| AppError::auth("bad challenge"))
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A fresh session token, the same shape the auth service issues.
#[must_use]
pub fn session_token() -> String {
    use rand::RngCore as _;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}
