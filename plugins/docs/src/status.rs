//! "Sync now" and the sync status line shown on the plugin's settings page.

use serde_json::{json, Value};

use crate::sync::Report;

/// Where the last status is kept.
pub const STATUS_KEY: &str = "sync/status";

/// Remembers that a `sync_now` request was already acted on, so a setting
/// that stays ticked does not restart the import on every deploy.
pub const NOW_SEEN_KEY: &str = "cfg/sync-now-seen";

/// What the last sync run did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// When the run happened (ms since the epoch); 0 for never.
    pub at_ms: u64,
    /// Pages imported by the last finished run.
    pub pages: u64,
    /// The last run applied everything.
    pub complete: bool,
    /// The last run's error; empty when it had none.
    pub error: String,
}

impl Status {
    /// Reads the stored form; anything unreadable is "never synced".
    #[must_use]
    pub fn parse(text: Option<&str>) -> Self {
        let v: Value = text
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        Self {
            at_ms: v["at_ms"].as_u64().unwrap_or(0),
            pages: v["pages"].as_u64().unwrap_or(0),
            complete: v["complete"].as_bool().unwrap_or(false),
            error: v["error"].as_str().unwrap_or("").to_owned(),
        }
    }

    /// The stored form.
    #[must_use]
    pub fn to_json(&self) -> String {
        json!({"at_ms": self.at_ms, "pages": self.pages, "complete": self.complete, "error": self.error})
            .to_string()
    }

    /// The status after a run. A paused run keeps the page count of the last
    /// finished one; a failed run keeps everything and records the error.
    #[must_use]
    pub fn after(prev: &Self, now_ms: u64, outcome: Result<&Report, &str>) -> Self {
        match outcome {
            Ok(r) => Self {
                at_ms: now_ms,
                pages: if r.finished {
                    u64::from(r.created) + u64::from(r.updated) + u64::from(r.unchanged)
                } else {
                    prev.pages
                },
                complete: r.finished,
                error: String::new(),
            },
            Err(e) => Self {
                at_ms: now_ms,
                pages: prev.pages,
                complete: false,
                error: e.chars().take(300).collect(),
            },
        }
    }

    /// The line the operator reads.
    #[must_use]
    pub fn line(&self, now_ms: u64) -> String {
        let when = if self.at_ms == 0 {
            "never".to_owned()
        } else {
            let secs = now_ms.saturating_sub(self.at_ms) / 1000;
            match secs {
                0..=59 => format!("{secs} s ago"),
                60..=3599 => format!("{} min ago", secs / 60),
                3600..=86399 => format!("{} h ago", secs / 3600),
                _ => format!("{} days ago", secs / 86400),
            }
        };
        let error = if self.error.is_empty() {
            "none".to_owned()
        } else {
            self.error.clone()
        };
        format!(
            "Last sync: {when}. Pages imported: {}. Sync complete: {}. Last error: {error}.",
            self.pages,
            if self.complete { "yes" } else { "no" },
        )
    }

    /// The public, secret-free view served at `GET status`.
    #[must_use]
    pub fn public_json(&self) -> String {
        json!({"last_sync_ms": self.at_ms, "pages": self.pages, "complete": self.complete, "ok": self.error.is_empty()})
            .to_string()
    }
}

/// What `init` does about the `sync_now` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NowAction {
    /// Nothing to do.
    Nothing,
    /// Start a full import and remember that this request was served.
    Trigger,
    /// The box was unticked: forget the served request.
    Rearm,
}

/// Decides from the setting and the remembered mark.
#[must_use]
pub fn now_action(requested: bool, seen: Option<&str>) -> NowAction {
    match (requested, seen == Some("1")) {
        (true, false) => NowAction::Trigger,
        (false, true) => NowAction::Rearm,
        _ => NowAction::Nothing,
    }
}

/// Whether the saved settings ask for a sync now.
#[must_use]
pub fn requested(config_json: &str) -> bool {
    let v: Value = serde_json::from_str(config_json).unwrap_or(Value::Null);
    match &v["sync_now"] {
        Value::Bool(b) => *b,
        Value::String(s) => matches!(s.as_str(), "true" | "1" | "on"),
        _ => false,
    }
}

/// The settings field, with the status in its help text.
#[must_use]
pub fn field(status_line: &str) -> Value {
    json!({
        "key": "sync_now", "label": "Sync now", "type": "boolean", "default": false,
        "help": format!("Tick and save to import everything again within a minute; untick it afterwards. {status_line} The numbers refresh when this page is saved; GET /api/v1/plugin/vyasa-docs/status shows them live."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(finished: bool) -> Report {
        Report {
            created: 2,
            updated: 3,
            unchanged: 5,
            finished,
            ..Report::default()
        }
    }

    #[test]
    fn a_finished_run_counts_pages_and_clears_the_error() {
        let prev = Status {
            error: "boom".into(),
            pages: 1,
            ..Status::default()
        };
        let s = Status::after(&prev, 9, Ok(&report(true)));
        assert_eq!(
            (s.pages, s.complete, s.error.as_str(), s.at_ms),
            (10, true, "", 9)
        );
    }

    #[test]
    fn a_paused_run_keeps_the_last_count_and_is_not_complete() {
        let prev = Status {
            pages: 7,
            complete: true,
            ..Status::default()
        };
        let s = Status::after(&prev, 9, Ok(&report(false)));
        assert_eq!((s.pages, s.complete), (7, false));
    }

    #[test]
    fn a_failed_run_records_the_error_and_keeps_the_count() {
        let prev = Status {
            pages: 7,
            complete: true,
            ..Status::default()
        };
        let s = Status::after(&prev, 9, Err("GitHub said 403"));
        assert_eq!(
            (s.pages, s.complete, s.error.as_str()),
            (7, false, "GitHub said 403")
        );
        assert!(s.line(9).contains("Last error: GitHub said 403"));
    }

    #[test]
    fn the_status_round_trips_and_unreadable_text_is_never() {
        let s = Status {
            at_ms: 5,
            pages: 4,
            complete: true,
            error: "x".into(),
        };
        assert_eq!(Status::parse(Some(&s.to_json())), s);
        assert_eq!(Status::parse(Some("not json")), Status::default());
        assert!(Status::parse(None).line(1).starts_with("Last sync: never."));
    }

    #[test]
    fn the_line_names_all_four_facts() {
        let s = Status {
            at_ms: 1_000,
            pages: 629,
            complete: false,
            error: String::new(),
        };
        assert_eq!(
            s.line(1_000 + 120_000),
            "Last sync: 2 min ago. Pages imported: 629. Sync complete: no. Last error: none."
        );
    }

    #[test]
    fn the_public_view_has_no_error_text() {
        let s = Status {
            error: "secret path".into(),
            ..Status::default()
        };
        assert!(!s.public_json().contains("secret"));
        assert!(s.public_json().contains("\"ok\":false"));
    }

    #[test]
    fn a_ticked_box_triggers_once_and_rearms_when_unticked() {
        assert_eq!(now_action(true, None), NowAction::Trigger);
        assert_eq!(now_action(true, Some("1")), NowAction::Nothing);
        assert_eq!(now_action(false, Some("1")), NowAction::Rearm);
        assert_eq!(now_action(false, None), NowAction::Nothing);
    }

    #[test]
    fn the_setting_reads_as_a_bool_or_a_string() {
        assert!(requested(r#"{"sync_now":true}"#));
        assert!(requested(r#"{"sync_now":"on"}"#));
        assert!(!requested(r#"{"sync_now":false}"#));
        assert!(!requested("{}"));
    }

    #[test]
    fn the_field_is_a_boolean_carrying_the_status() {
        let f = field("Last sync: never.");
        assert_eq!(f["type"], "boolean");
        assert!(f["help"]
            .as_str()
            .is_some_and(|h| h.contains("Last sync: never.")));
    }
}
