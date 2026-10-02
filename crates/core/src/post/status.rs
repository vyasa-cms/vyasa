//! Post status transition validation.
//!
//! User-initiated transitions are a strict subset of system transitions.
//! In particular, `Scheduled -> Published` is system-only (the publisher
//! worker), so users must unschedule to draft first if they want to
//! publish early via a different flow (or the worker can be nudged).

use vyasa_common::AppError;
use vyasa_db::content_models::PostStatus;

/// User-allowed transitions (via REST).
const USER_TRANSITIONS: &[(PostStatus, PostStatus)] = &[
    (PostStatus::Draft, PostStatus::Draft),
    (PostStatus::Draft, PostStatus::Published),
    (PostStatus::Draft, PostStatus::Scheduled),
    (PostStatus::Draft, PostStatus::Private),
    (PostStatus::Draft, PostStatus::Trash),
    (PostStatus::Scheduled, PostStatus::Scheduled),
    (PostStatus::Scheduled, PostStatus::Draft),
    (PostStatus::Scheduled, PostStatus::Trash),
    (PostStatus::Published, PostStatus::Published),
    (PostStatus::Published, PostStatus::Draft),
    (PostStatus::Published, PostStatus::Private),
    (PostStatus::Published, PostStatus::Trash),
    (PostStatus::Private, PostStatus::Private),
    (PostStatus::Private, PostStatus::Published),
    (PostStatus::Private, PostStatus::Draft),
    (PostStatus::Private, PostStatus::Trash),
    (PostStatus::Trash, PostStatus::Trash),
    (PostStatus::Trash, PostStatus::Draft),
];

/// System-allowed transitions (publisher worker, etc.) — superset of user.
const SYSTEM_TRANSITIONS: &[(PostStatus, PostStatus)] = &[
    (PostStatus::Draft, PostStatus::Draft),
    (PostStatus::Draft, PostStatus::Published),
    (PostStatus::Draft, PostStatus::Scheduled),
    (PostStatus::Draft, PostStatus::Private),
    (PostStatus::Draft, PostStatus::Trash),
    (PostStatus::Scheduled, PostStatus::Scheduled),
    (PostStatus::Scheduled, PostStatus::Published),
    (PostStatus::Scheduled, PostStatus::Draft),
    (PostStatus::Scheduled, PostStatus::Trash),
    (PostStatus::Published, PostStatus::Published),
    (PostStatus::Published, PostStatus::Draft),
    (PostStatus::Published, PostStatus::Private),
    (PostStatus::Published, PostStatus::Trash),
    (PostStatus::Private, PostStatus::Private),
    (PostStatus::Private, PostStatus::Published),
    (PostStatus::Private, PostStatus::Draft),
    (PostStatus::Private, PostStatus::Trash),
    (PostStatus::Trash, PostStatus::Trash),
    (PostStatus::Trash, PostStatus::Draft),
];

/// Whether a user-initiated transition is legal.
#[must_use]
pub fn transition_allowed(from: PostStatus, to: PostStatus) -> bool {
    USER_TRANSITIONS.contains(&(from, to))
}

/// Whether a system-initiated transition is legal.
#[must_use]
pub fn system_transition_allowed(from: PostStatus, to: PostStatus) -> bool {
    SYSTEM_TRANSITIONS.contains(&(from, to))
}

/// Ensures a user transition is legal.
///
/// # Errors
///
/// Returns [`AppError::Validation`] naming the rejected transition.
pub fn ensure_transition(from: PostStatus, to: PostStatus) -> Result<(), AppError> {
    if transition_allowed(from, to) {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "illegal status transition {} -> {} (trash restores to draft first)",
            from.as_str(),
            to.as_str()
        )))
    }
}

/// Ensures a system transition is legal.
///
/// # Errors
///
/// Returns [`AppError::Validation`] naming the rejected transition.
pub fn ensure_system_transition(from: PostStatus, to: PostStatus) -> Result<(), AppError> {
    if system_transition_allowed(from, to) {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "illegal system transition {} -> {}",
            from.as_str(),
            to.as_str()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_transition, system_transition_allowed, transition_allowed, PostStatus};

    #[test]
    fn user_legal_transitions() {
        assert!(transition_allowed(PostStatus::Draft, PostStatus::Published));
        assert!(transition_allowed(PostStatus::Draft, PostStatus::Scheduled));
        assert!(transition_allowed(PostStatus::Published, PostStatus::Draft));
        assert!(transition_allowed(PostStatus::Trash, PostStatus::Draft));
        assert!(transition_allowed(PostStatus::Draft, PostStatus::Draft));
        // Scheduled -> Published is system-only
        assert!(!transition_allowed(
            PostStatus::Scheduled,
            PostStatus::Published
        ));
    }

    #[test]
    fn system_allows_scheduled_publish() {
        assert!(system_transition_allowed(
            PostStatus::Scheduled,
            PostStatus::Published
        ));
    }

    #[test]
    fn illegal_transitions() {
        assert!(!transition_allowed(
            PostStatus::Trash,
            PostStatus::Published
        ));
        assert!(!transition_allowed(
            PostStatus::Trash,
            PostStatus::Scheduled
        ));
        let err = ensure_transition(PostStatus::Trash, PostStatus::Published)
            .err()
            .map_or(String::new(), |e| e.to_string());
        assert!(err.contains("trash -> published"), "err: {err}");
    }
}
