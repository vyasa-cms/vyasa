//! Unified error taxonomy for Vyasa.
//!
//! Every crate converts its failures into [`AppError`]. Each error carries a
//! stable machine code (see [`AppError::error_code`]) so the REST and
//! GraphQL adapters can produce structured responses without inspecting
//! human-readable messages. Messages stored in the variants must be safe to
//! show to clients: never put SQL, DSNs, or stack traces in them — use
//! [`AppError::internal`] which keeps the underlying cause for server-side
//! logs only.

use std::error::Error as StdError;
use std::fmt;

/// Boxed dynamic error source for internal errors.
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// The unified error type used across all Vyasa crates.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The requested resource does not exist.
    #[error("{resource} not found: {ident}")]
    NotFound {
        /// Machine-readable resource name, e.g. `"post"`.
        resource: &'static str,
        /// Identifier of the missing instance (id, slug, ...).
        ident: String,
    },
    /// The request input failed validation.
    #[error("validation failed: {message}")]
    Validation {
        /// Human-readable description of the validation failure.
        message: String,
    },
    /// Authentication is missing or invalid.
    #[error("authentication failed: {message}")]
    Auth {
        /// Human-readable description of the failure.
        message: String,
    },
    /// The caller is authenticated but lacks the required capability.
    #[error("permission denied: {message}")]
    Forbidden {
        /// Human-readable description of what was denied.
        message: String,
    },
    /// The password step passed; a second factor is needed. Answers 202
    /// with the challenge so the client can ask for a code.
    #[error("a second factor is needed")]
    MfaRequired {
        /// The signed challenge to send back with the code.
        challenge: String,
    },
    /// The credentials were right, but the account was made by public
    /// registration and its address has not been confirmed yet, so there
    /// is no session. Only ever answered after the password checked out:
    /// it must not tell a stranger which addresses have accounts.
    #[error("the email address has not been confirmed")]
    EmailUnconfirmed,
    /// The request conflicts with existing state (e.g. duplicate slug).
    #[error("conflict: {message}")]
    Conflict {
        /// Human-readable description of the conflict.
        message: String,
    },
    /// The caller exceeded a rate limit.
    #[error("rate limited: {message}")]
    RateLimited {
        /// Human-readable description of the exceeded limit.
        message: String,
    },
    /// Unexpected internal failure; details are logged server-side only.
    #[error("internal error: {message}")]
    Internal {
        /// Sanitized summary safe for client display.
        message: String,
        /// Optional underlying cause, preserved for server-side logs.
        #[source]
        source: Option<BoxError>,
    },
    /// A database operation failed.
    #[error("database error: {message}")]
    Db {
        /// Sanitized description (no SQL text or DSNs).
        message: String,
    },
    /// An external upstream service failed.
    #[error("external service error ({provider}): {message}")]
    External {
        /// Provider name, e.g. `"anthropic"`.
        provider: &'static str,
        /// Sanitized failure description.
        message: String,
    },
    /// The request body is bigger than the endpoint accepts.
    ///
    /// Distinct from validation: a 400 says "fix the request", a 413 says
    /// "send less". Media uploads documented a 413 for years while the
    /// only path that could produce one returned 400 with an opaque
    /// message, so clients could not tell an oversize file from a bad
    /// field.
    #[error("payload too large: {message}")]
    TooLarge {
        /// What was too big and by how much, safe for display.
        message: String,
    },
}

impl AppError {
    /// Builds a [`AppError::NotFound`] for `resource` identified by `ident`.
    #[must_use]
    pub fn not_found(resource: &'static str, ident: impl fmt::Display) -> Self {
        Self::NotFound {
            resource,
            ident: ident.to_string(),
        }
    }

    /// Builds a [`AppError::Validation`] with `message`.
    #[must_use]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    /// Builds a [`AppError::Auth`] with `message`.
    #[must_use]
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth {
            message: message.into(),
        }
    }

    /// Builds a [`AppError::Forbidden`] with `message`.
    #[must_use]
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden {
            message: message.into(),
        }
    }

    /// Builds a [`AppError::Conflict`] with `message`.
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }

    /// Builds a [`AppError::RateLimited`] with `message`.
    #[must_use]
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::RateLimited {
            message: message.into(),
        }
    }

    /// Builds a [`AppError::Internal`] wrapping `source` with a sanitized
    /// client-safe `message`.
    #[must_use]
    pub fn internal(source: impl Into<BoxError>, message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            source: Some(source.into()),
        }
    }

    /// Builds a [`AppError::Internal`] without an underlying source.
    #[must_use]
    pub fn internal_msg(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            source: None,
        }
    }

    /// Builds a [`AppError::Db`] with a sanitized `message`.
    #[must_use]
    pub fn db(message: impl Into<String>) -> Self {
        Self::Db {
            message: message.into(),
        }
    }

    /// Builds a [`AppError::External`] for `provider` with a sanitized
    /// `message`.
    #[must_use]
    pub fn external(provider: &'static str, message: impl Into<String>) -> Self {
        Self::External {
            provider,
            message: message.into(),
        }
    }

    /// Builds a [`AppError::TooLarge`] with `message`.
    #[must_use]
    pub fn too_large(message: impl Into<String>) -> Self {
        Self::TooLarge {
            message: message.into(),
        }
    }

    /// Returns the stable machine code for this error, e.g.
    /// `"post_not_found"`, `"unauthorized"`. Adapters expose this verbatim
    /// to API clients; never change existing codes (only add new ones).
    #[must_use]
    pub fn error_code(&self) -> String {
        match self {
            Self::NotFound { resource, .. } => format!("{resource}_not_found"),
            Self::Validation { .. } => "validation_failed".to_string(),
            Self::Auth { .. } => "unauthorized".to_string(),
            Self::Forbidden { .. } => "forbidden".to_string(),
            Self::Conflict { .. } => "conflict".to_string(),
            Self::MfaRequired { .. } => "mfa_required".to_string(),
            Self::EmailUnconfirmed => "email_unconfirmed".to_string(),
            Self::RateLimited { .. } => "rate_limited".to_string(),
            Self::Internal { .. } => "internal_error".to_string(),
            Self::Db { .. } => "db_error".to_string(),
            Self::External { .. } => "external_error".to_string(),
            Self::TooLarge { .. } => "payload_too_large".to_string(),
        }
    }

    /// The human sentence, without the category in front of it.
    ///
    /// [`fmt::Display`] prefixes the taxonomy — "external service error
    /// (openrouter): rate limited…" — which is what a log line wants and
    /// the opposite of what an author wants. They already have the category
    /// from [`Self::error_code`], and there is nothing they can do with the
    /// word "external"; it only pushes the part that matters off the end of
    /// whatever is showing it.
    #[must_use]
    pub fn client_message(&self) -> String {
        match self {
            // Carries no separate message: "post not found: 42" already
            // reads as a sentence.
            Self::NotFound { .. } => self.to_string(),
            Self::MfaRequired { .. } => "a second factor is needed".to_string(),
            Self::EmailUnconfirmed => {
                "confirm your email address first: follow the link we sent you".to_string()
            }
            Self::Validation { message }
            | Self::Auth { message }
            | Self::Forbidden { message }
            | Self::Conflict { message }
            | Self::RateLimited { message }
            | Self::Internal { message, .. }
            | Self::Db { message }
            | Self::External { message, .. }
            | Self::TooLarge { message } => message.clone(),
        }
    }

    /// Returns the HTTP status code this error maps to. The api crate
    /// converts this `u16` into the framework's status type.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound { .. } => 404,
            Self::Validation { .. } => 400,
            Self::Auth { .. } => 401,
            Self::Forbidden { .. } | Self::EmailUnconfirmed => 403,
            Self::Conflict { .. } => 409,
            Self::MfaRequired { .. } => 202,
            Self::RateLimited { .. } => 429,
            Self::Internal { .. } | Self::Db { .. } => 500,
            Self::External { .. } => 502,
            Self::TooLarge { .. } => 413,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppError;
    use std::collections::HashSet;

    fn one_of_each() -> Vec<AppError> {
        vec![
            AppError::not_found("post", 42),
            AppError::validation("bad input"),
            AppError::auth("no session"),
            AppError::forbidden("missing cap"),
            AppError::conflict("slug taken"),
            AppError::rate_limited("too fast"),
            AppError::internal_msg("boom"),
            AppError::db("query failed"),
            AppError::external("anthropic", "timeout"),
            AppError::too_large("12 MB, max 10 MB"),
            AppError::EmailUnconfirmed,
        ]
    }

    #[test]
    fn error_codes_are_unique_per_category() {
        let codes: Vec<String> = one_of_each().iter().map(AppError::error_code).collect();
        let set: HashSet<&String> = codes.iter().collect();
        assert_eq!(codes.len(), set.len(), "duplicate codes: {codes:?}");
    }

    #[test]
    fn the_client_message_drops_the_category_but_keeps_the_sentence() {
        // Display is for logs and prefixes the taxonomy. Clients get the
        // category in `code`, so repeating it in prose only pushes the part
        // that matters off the end of whatever renders it.
        let e = AppError::external("openrouter", "Rate limited. Try again shortly.");
        assert_eq!(
            e.to_string(),
            "external service error (openrouter): Rate limited. Try again shortly."
        );
        assert_eq!(e.client_message(), "Rate limited. Try again shortly.");

        // Every variant must give back something non-empty, or a surface
        // shows a blank alert box.
        for err in one_of_each() {
            let msg = err.client_message();
            assert!(!msg.trim().is_empty(), "{err:?} has no client message");
            assert!(!msg.contains("error ("), "{err:?} kept its category: {msg}");
        }

        // The one with no message field still reads as a sentence.
        assert_eq!(
            AppError::not_found("post", 42).client_message(),
            "post not found: 42"
        );
    }

    #[test]
    fn not_found_code_embeds_resource() {
        assert_eq!(
            AppError::not_found("post", 7).error_code(),
            "post_not_found"
        );
        assert_eq!(
            AppError::not_found("media", "x.png").error_code(),
            "media_not_found"
        );
    }

    #[test]
    fn http_status_mapping() {
        let expected = [404, 400, 401, 403, 409, 429, 500, 500, 502, 413];
        for (err, status) in one_of_each().into_iter().zip(expected) {
            assert_eq!(err.http_status(), status, "for {err}");
        }
    }

    #[test]
    fn internal_preserves_source() {
        let err = AppError::internal(std::io::Error::other("disk on fire"), "upload failed");
        let source = std::error::Error::source(&err).expect("source must be present");
        assert_eq!(source.to_string(), "disk on fire");
        assert!(err.to_string().contains("upload failed"));
    }
}
