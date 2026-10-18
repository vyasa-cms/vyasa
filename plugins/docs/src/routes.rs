//! The plugin's one public route: the GitHub webhook.

use crate::settings::Settings;
use crate::webhook::{self, Delivery};

/// The route path, below the plugin's mount point.
pub const WEBHOOK_PATH: &str = "webhook";

/// The request headers the route asks the host to forward.
pub const WEBHOOK_HEADERS: &[&str] = &["x-hub-signature-256", "x-github-event"];

/// What the route decided.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    /// HTTP status.
    pub status: u16,
    /// JSON body.
    pub body: String,
    /// A commit to sync soon (the host stores it for the next task run).
    pub pending: Option<String>,
}

fn reply(status: u16, body: &str) -> Outcome {
    Outcome {
        status,
        body: body.to_owned(),
        pending: None,
    }
}

/// Handles `POST webhook`.
#[must_use]
pub fn handle(
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &str,
    settings: &Settings,
) -> Outcome {
    if path != WEBHOOK_PATH {
        return reply(404, r#"{"error":"no such endpoint"}"#);
    }
    if method != "POST" {
        return reply(405, r#"{"error":"POST only"}"#);
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    // Authentication before anything else, and before the body is read as JSON.
    if !webhook::verify(
        &settings.webhook_secret,
        body.as_bytes(),
        header("x-hub-signature-256").unwrap_or(""),
    ) {
        return reply(401, r#"{"error":"bad signature"}"#);
    }
    match webhook::interpret(
        header("x-github-event").unwrap_or(""),
        body,
        &settings.branch,
    ) {
        Delivery::Ping => reply(200, r#"{"pong":true}"#),
        Delivery::Ignored => reply(200, r#"{"ignored":true}"#),
        Delivery::Push { sha } => Outcome {
            status: 202,
            body: r#"{"queued":true}"#.to_owned(),
            pending: Some(sha),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::from_json(
            r#"{"repo_url":"https://github.com/o/r","webhook_secret":"shh","branch":"main"}"#,
        )
    }

    fn signed(body: &str, event: &str) -> Vec<(String, String)> {
        vec![
            (
                "x-hub-signature-256".into(),
                webhook::sign("shh", body.as_bytes()),
            ),
            ("x-github-event".into(), event.into()),
        ]
    }

    const PUSH: &str =
        r#"{"ref":"refs/heads/main","after":"0123456789abcdef0123456789abcdef01234567"}"#;

    #[test]
    fn a_signed_push_is_queued_with_its_commit() {
        let o = handle("POST", "webhook", &signed(PUSH, "push"), PUSH, &settings());
        assert_eq!(o.status, 202);
        assert_eq!(
            o.pending.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn an_unsigned_wrong_or_secretless_delivery_is_refused() {
        let s = settings();
        assert_eq!(handle("POST", "webhook", &[], PUSH, &s).status, 401);
        let bad = vec![
            (
                "x-hub-signature-256".into(),
                webhook::sign("other", PUSH.as_bytes()),
            ),
            ("x-github-event".into(), "push".into()),
        ];
        assert_eq!(handle("POST", "webhook", &bad, PUSH, &s).status, 401);
        let mut none = settings();
        none.webhook_secret.clear();
        let h = vec![(
            "x-hub-signature-256".into(),
            webhook::sign("", PUSH.as_bytes()),
        )];
        assert_eq!(handle("POST", "webhook", &h, PUSH, &none).status, 401);
    }

    #[test]
    fn ping_other_events_and_other_branches_are_acknowledged_without_work() {
        let s = settings();
        assert_eq!(
            handle("POST", "webhook", &signed("{}", "ping"), "{}", &s).body,
            r#"{"pong":true}"#
        );
        let other =
            r#"{"ref":"refs/heads/dev","after":"0123456789abcdef0123456789abcdef01234567"}"#;
        let o = handle("POST", "webhook", &signed(other, "push"), other, &s);
        assert_eq!((o.status, o.pending), (200, None));
        assert_eq!(
            handle("POST", "webhook", &signed("{}", "issues"), "{}", &s).pending,
            None
        );
    }

    #[test]
    fn wrong_method_or_path() {
        assert_eq!(handle("GET", "webhook", &[], "", &settings()).status, 405);
        assert_eq!(handle("POST", "other", &[], "", &settings()).status, 404);
    }

    #[test]
    fn header_names_match_case_insensitively() {
        let h = vec![
            (
                "X-Hub-Signature-256".into(),
                webhook::sign("shh", PUSH.as_bytes()),
            ),
            ("X-GitHub-Event".into(), "push".into()),
        ];
        assert_eq!(handle("POST", "webhook", &h, PUSH, &settings()).status, 202);
    }
}
