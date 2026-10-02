//! Recording destructive administrative actions.
//!
//! Deliberately fire-and-forget: the action has already been authorised and
//! performed, so a logging failure must not turn a successful deletion into
//! an error the operator will retry.
//!
//! What counts as worth recording is "hard to undo, and someone may later
//! ask who did it" — user creation, deletion and role changes, theme and plugin
//! lifecycle, option writes, webhook and API-key removal. Ordinary content
//! edits are not audited here: revisions already carry that history.

use crate::state::AppState;

/// Best-effort audit write.
///
/// Takes the actor's row rather than an extractor, because the endpoints
/// worth auditing are split between the `CurrentUser` and `Principal`
/// extractors and neither is more correct than the other here.
///
/// Spawned rather than awaited so the response is not held up by the log.
pub fn record(
    state: &AppState,
    actor: &vyasa_db::models::UserRow,
    action: &str,
    target: impl Into<String>,
    detail: serde_json::Value,
) {
    let repo = vyasa_db::repo::AuditRepo::new(state.pool.clone());
    let actor_id = actor.id;
    let actor_name = actor.display_name.clone();
    let action = action.to_owned();
    let target = target.into();
    tokio::spawn(async move {
        if let Err(err) = repo
            .record(Some(actor_id), &actor_name, &action, &target, detail, "")
            .await
        {
            tracing::warn!(action = %action, "audit write failed: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    /// The action vocabulary, kept in one place so a reader of the log can
    /// know what they might encounter.
    const ACTIONS: [&str; 14] = [
        "user.create",
        "user.register",
        "user.confirm",
        "user.resend_confirmation",
        "ai.provider.update",
        "user.delete",
        "user.role_change",
        "theme.activate",
        "theme.rollback",
        "plugin.install",
        "plugin.enable",
        "plugin.disable",
        "plugin.delete",
        "options.write",
    ];

    #[test]
    fn action_names_are_namespaced_and_lowercase() {
        // A log that mixes "UserDelete" with "user.delete" is a log nobody
        // can filter.
        for action in ACTIONS {
            assert!(action.contains('.'), "{action} needs a namespace");
            assert_eq!(action, action.to_lowercase(), "{action} must be lowercase");
        }
    }
}
