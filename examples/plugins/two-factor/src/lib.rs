//! two-factor reference plugin: TOTP challenge on login.

pub mod totp;

/// Settings keys used in plugin_settings.
pub const SECRET_KEY: &str = "totp_secret";
/// Whether 2FA is enabled for the site.
pub const ENABLED_KEY: &str = "enabled";

/// Evaluates a login attempt: returns `Some(expected_flow)` when the
/// plugin demands a second factor.
///
/// `settings` mirrors plugin_settings kv; `code_opt` is the submitted
/// code (absent on first login attempt).
pub fn login_challenge(
    settings: &dyn Fn(&str) -> Option<String>,
    code_opt: Option<&str>,
    now_secs: u64,
) -> Challenge {
    let enabled = settings(ENABLED_KEY).as_deref() == Some("true");
    if !enabled {
        return Challenge::Allow;
    }
    let Some(secret_b64) = settings(SECRET_KEY) else {
        // Enabled but unprovisioned: fail closed.
        return Challenge::Deny(String::from("2FA enabled without secret"));
    };
    let secret = base32_decode(&secret_b64).unwrap_or_else(|| secret_b64.into_bytes());
    match code_opt {
        None => Challenge::RequireCode,
        Some(code) => {
            if totp::verify(&secret, code, now_secs, 1) {
                Challenge::Allow
            } else {
                Challenge::Deny(String::from("invalid or expired code"))
            }
        }
    }
}

/// Login outcome demanded by the auth:challenge stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Challenge {
    /// Proceed with normal authentication.
    Allow,
    /// Stop and present the OTP form.
    RequireCode,
    /// Reject outright with reason.
    Deny(String),
}

fn base32_decode(input: &str) -> Option<Vec<u8>> {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let clean: Vec<u8> = input
        .to_ascii_uppercase()
        .bytes()
        .filter(|b| *b != b'=' && *b != b' ')
        .collect();
    if clean.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut buffer = 0u64;
    let mut bits = 0u32;
    for b in clean {
        let idx = ALPHA.iter().position(|a| *a == b)? as u64;
        buffer = (buffer << 5) | idx;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_reference_vector() {
        // RFC 6238 Appendix B secret "12345678901234567890" (ASCII).
        let secret = b"12345678901234567890";
        // T=59s -> 94287082 truncated to 6 digits = 287082.
        assert_eq!(totp::totp_at(secret, 59 / 30), String::from("287082"));
        // T=1111111109 -> 081804.
        assert_eq!(
            totp::totp_at(secret, 1_111_111_109 / 30),
            String::from("081804")
        );
    }

    #[test]
    fn verify_accepts_current_code_and_rejects_wrong() {
        let secret = b"12345678901234567890";
        let now: u64 = 59;
        let code = totp::totp_now(secret, now);
        assert!(totp::verify(secret, &code, now + 29, 1));
        assert!(!totp::verify(secret, "000000", now, 1));
    }

    #[test]
    fn challenge_flow() {
        let disabled = |_k: &str| None;
        assert_eq!(login_challenge(&disabled, None, 0), Challenge::Allow);

        let settings = |k: &str| match k {
            ENABLED_KEY => Some(String::from("true")),
            SECRET_KEY => Some(String::from("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")),
            _ => None,
        };
        assert_eq!(login_challenge(&settings, None, 59), Challenge::RequireCode);
        let good = totp::totp_now(b"12345678901234567890", 59);
        assert_eq!(
            login_challenge(&settings, Some(&good), 59),
            Challenge::Allow
        );
        assert!(matches!(
            login_challenge(&settings, Some("000000"), 59),
            Challenge::Deny(_)
        ));
    }
}


