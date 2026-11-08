//! Verifying a GitHub webhook delivery.
//!
//! GitHub signs the raw body with HMAC-SHA256 under the secret the
//! operator typed into both places, and sends `X-Hub-Signature-256:
//! sha256=<hex>`. The check is constant-time (`verify_slice`), runs before
//! the body is parsed, and a delivery without a configured secret is
//! always refused: an unauthenticated "sync now" endpoint is a free way to
//! make someone's site spend its fetch quota.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Whether `header` is the signature of `body` under `secret`.
#[must_use]
pub fn verify(secret: &str, body: &[u8], header: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Some(hex) = header.trim().strip_prefix("sha256=") else {
        return false;
    };
    let Some(expected) = decode_hex(hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// The signature GitHub would send, for tests and for the docs.
#[must_use]
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC takes any key length"));
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut out = String::from("sha256=");
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.is_empty() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// What a push delivery says, reduced to what a sync needs.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    /// A push to this branch, now at this commit.
    Push {
        /// The commit the branch points at.
        sha: String,
    },
    /// GitHub's "ping" on creating the hook.
    Ping,
    /// Anything else: acknowledged and ignored.
    Ignored,
}

/// Interprets a verified delivery.
#[must_use]
pub fn interpret(event: &str, body: &str, branch: &str) -> Delivery {
    match event {
        "ping" => Delivery::Ping,
        "push" => {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
                return Delivery::Ignored;
            };
            let on_branch = v["ref"].as_str() == Some(&format!("refs/heads/{branch}"));
            match (on_branch, v["after"].as_str()) {
                (true, Some(sha))
                    if sha.len() >= 7
                        && sha.chars().all(|c| c.is_ascii_hexdigit())
                        && !sha.chars().all(|c| c == '0') =>
                {
                    Delivery::Push {
                        sha: sha.to_owned(),
                    }
                }
                _ => Delivery::Ignored,
            }
        }
        _ => Delivery::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_good_signature_verifies() {
        let body = br#"{"zen":"Keep it logically awesome."}"#;
        let header = sign("s3cret", body);
        assert!(verify("s3cret", body, &header));
    }

    #[test]
    fn known_vector_from_github_docs() {
        // The worked example in GitHub's "Validating webhook deliveries".
        let header = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify(
            "It's a Secret to Everybody",
            b"Hello, World!",
            header
        ));
    }

    #[test]
    fn wrong_secret_body_or_shape_is_refused() {
        let body = b"{}";
        let header = sign("a", body);
        assert!(!verify("b", body, &header));
        assert!(!verify("a", b"{ }", &header));
        assert!(!verify("a", body, "sha1=abcd"));
        assert!(!verify("a", body, "sha256=zz"));
        assert!(!verify("a", body, "sha256="));
        assert!(!verify("a", body, ""));
    }

    #[test]
    fn no_configured_secret_refuses_everything() {
        let header = sign("", b"x");
        assert!(!verify("", b"x", &header));
    }

    #[test]
    fn push_events_on_the_branch_carry_the_commit() {
        let body =
            r#"{"ref":"refs/heads/main","after":"0123456789abcdef0123456789abcdef01234567"}"#;
        assert_eq!(
            interpret("push", body, "main"),
            Delivery::Push {
                sha: "0123456789abcdef0123456789abcdef01234567".into()
            }
        );
        assert_eq!(interpret("push", body, "dev"), Delivery::Ignored);
        assert_eq!(interpret("ping", "{}", "main"), Delivery::Ping);
        assert_eq!(interpret("issues", "{}", "main"), Delivery::Ignored);
        let deleted =
            r#"{"ref":"refs/heads/main","after":"0000000000000000000000000000000000000000"}"#;
        assert_eq!(interpret("push", deleted, "main"), Delivery::Ignored);
        assert_eq!(interpret("push", "not json", "main"), Delivery::Ignored);
    }
}
