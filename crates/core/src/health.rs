//! Site-health diagnostics.
//!
//! Each check is read-only and individually time-bounded, so one wedged
//! dependency cannot hang the health page that exists to tell you it is
//! wedged.
//!
//! Checks report three states rather than a boolean: a search index that is
//! merely behind is a different situation from one that cannot be opened,
//! and collapsing them loses the distinction an operator needs.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long any single check may take before it is reported as failed.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Working as intended.
    Ok,
    /// Working, but something needs attention.
    Warn,
    /// Not working.
    Fail,
}

/// What an operator can do about a check, from the page itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Action {
    /// Button text.
    pub label: String,
    /// An admin page to open, e.g. `/admin/settings#settings-delivery`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// An operation the page knows how to run, e.g. `reindex`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
}

/// One named diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    /// Stable machine name, e.g. `database`.
    pub name: String,
    /// Result.
    pub status: Status,
    /// One line an operator can act on.
    pub detail: String,
    /// Which part of the site it belongs to: `core`, `content`,
    /// `delivery`, `assistants`, `operations`.
    #[serde(default)]
    pub group: String,
    /// The remedy, when the page can offer one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
}

impl Check {
    /// Builds a check result.
    #[must_use]
    pub fn new(name: &str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status,
            detail: detail.into(),
            group: String::new(),
            action: None,
        }
    }

    /// Files the check under a group.
    #[must_use]
    pub fn in_group(mut self, group: &str) -> Self {
        group.clone_into(&mut self.group);
        self
    }

    /// Attaches a page to open.
    #[must_use]
    pub fn link(mut self, label: &str, href: &str) -> Self {
        self.action = Some(Action {
            label: label.to_owned(),
            href: Some(href.to_owned()),
            op: None,
        });
        self
    }

    /// Attaches an operation the page can run.
    #[must_use]
    pub fn op(mut self, label: &str, op: &str) -> Self {
        self.action = Some(Action {
            label: label.to_owned(),
            href: None,
            op: Some(op.to_owned()),
        });
        self
    }
}

/// The whole report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Worst status across all checks.
    pub status: Status,
    /// Individual results, worst first, stable within a status.
    pub checks: Vec<Check>,
    /// When the checks ran, RFC 3339.
    #[serde(default)]
    pub checked_at: String,
    /// How many checks warn.
    #[serde(default)]
    pub warnings: usize,
    /// How many checks fail.
    #[serde(default)]
    pub failures: usize,
}

impl Report {
    /// Rolls individual checks up into one overall status and puts the
    /// ones that need attention first.
    #[must_use]
    pub fn new(mut checks: Vec<Check>) -> Self {
        let failures = checks.iter().filter(|c| c.status == Status::Fail).count();
        let warnings = checks.iter().filter(|c| c.status == Status::Warn).count();
        let status = if failures > 0 {
            Status::Fail
        } else if warnings > 0 {
            Status::Warn
        } else {
            Status::Ok
        };
        let rank = |s: Status| match s {
            Status::Fail => 0,
            Status::Warn => 1,
            Status::Ok => 2,
        };
        checks.sort_by_key(|c| rank(c.status));
        Self {
            status,
            checks,
            checked_at: chrono::Utc::now().to_rfc3339(),
            warnings,
            failures,
        }
    }
}

/// Runs `check` under the shared timeout, reporting a timeout as a failure.
async fn bounded<F>(name: &str, check: F) -> Check
where
    F: std::future::Future<Output = Check>,
{
    match tokio::time::timeout(CHECK_TIMEOUT, check).await {
        Ok(result) => result,
        Err(_) => Check::new(
            name,
            Status::Fail,
            format!("timed out after {}s", CHECK_TIMEOUT.as_secs()),
        ),
    }
}

/// Checks the database is reachable and reports the migration version.
pub async fn database(pool: &sqlx::PgPool) -> Check {
    bounded("database", async {
        match sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(max(version), 0) FROM _sqlx_migrations WHERE success",
        )
        .fetch_one(pool)
        .await
        {
            Ok(version) => Check::new(
                "database",
                Status::Ok,
                format!("reachable, migration version {version}"),
            ),
            Err(err) => Check::new("database", Status::Fail, err.to_string()),
        }
    })
    .await
}

/// Checks the media directory exists and accepts a write.
pub async fn storage(media_dir: &std::path::Path) -> Check {
    let dir = media_dir.to_path_buf();
    bounded("storage", async move {
        // Actually write: a directory can exist and still be read-only, and
        // an upload failing at 3am is a worse way to find out.
        let probe = dir.join(".vyasa-health-probe");
        match tokio::fs::write(&probe, b"ok").await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&probe).await;
                Check::new(
                    "storage",
                    Status::Ok,
                    format!("{} is writable", dir.display()),
                )
            }
            Err(err) => Check::new("storage", Status::Fail, format!("{}: {err}", dir.display())),
        }
    })
    .await
}

/// Compares indexed documents against published entries (posts and
/// pages both live in the index). Only an index that is *behind* warns:
/// one that is ahead just has entries the next publish will reconcile.
#[must_use]
pub fn search_index(indexed: Option<u64>, published: i64) -> Check {
    let published = u64::try_from(published).unwrap_or(0);
    match indexed {
        None => Check::new(
            "search_index",
            Status::Warn,
            "index unavailable; search falls back to SQL matching",
        ),
        Some(count) if count >= published => Check::new(
            "search_index",
            Status::Ok,
            format!("{count} documents indexed for {published} published entries"),
        ),
        Some(count) => Check::new(
            "search_index",
            Status::Warn,
            format!("{count} documents indexed but {published} entries published; run a reindex"),
        )
        .op("Rebuild index", "reindex"),
    }
}

/// Free space where media lives; the disk that fills first takes the
/// site down with it.
#[must_use]
#[allow(clippy::cast_precision_loss)] // gigabytes for display; precision past 2^53 bytes is irrelevant
pub fn disk_free(free_bytes: Option<u64>, total_bytes: Option<u64>) -> Check {
    let (Some(free), Some(total)) = (free_bytes, total_bytes) else {
        return Check::new(
            "disk_free",
            Status::Ok,
            "free space not reported on this platform",
        );
    };
    let gb = |b: u64| b as f64 / 1_073_741_824.0;
    let pct = if total == 0 {
        100.0
    } else {
        free as f64 * 100.0 / total as f64
    };
    let status = if free < 512 * 1024 * 1024 || pct < 5.0 {
        Status::Fail
    } else if free < 2 * 1024 * 1024 * 1024 || pct < 10.0 {
        Status::Warn
    } else {
        Status::Ok
    };
    Check::new(
        "disk_free",
        status,
        format!("{:.1} GB free of {:.1} GB ({pct:.0}%)", gb(free), gb(total)),
    )
}

/// How big the database has grown; informational, warns past 20 GB so a
/// runaway table is noticed before the disk is.
#[must_use]
#[allow(clippy::cast_precision_loss)] // display only
pub fn database_size(bytes: Option<i64>) -> Check {
    match bytes {
        None => Check::new("database_size", Status::Ok, "size not available"),
        Some(b) => {
            let gb = b as f64 / 1_073_741_824.0;
            let status = if gb > 20.0 { Status::Warn } else { Status::Ok };
            let shown = if gb >= 1.0 {
                format!("{gb:.2} GB")
            } else {
                format!("{:.1} MB", b as f64 / 1_048_576.0)
            };
            Check::new("database_size", status, format!("{shown} on disk"))
        }
    }
}

/// Scheduled posts whose time has passed without being published: the
/// publisher is behind or not running.
#[must_use]
pub fn scheduled_publisher(overdue: i64) -> Check {
    if overdue > 0 {
        Check::new(
            "scheduled_publisher",
            Status::Fail,
            format!("{overdue} scheduled {} past due and still unpublished; the publisher is not running", if overdue == 1 { "post is" } else { "posts are" }),
        )
        .link("Open posts", "/admin/posts?status=scheduled")
    } else {
        Check::new(
            "scheduled_publisher",
            Status::Ok,
            "no scheduled post is past due",
        )
    }
}

/// When the nightly outbound-link sweep last ran.
#[must_use]
pub fn link_sweep(last: Option<chrono::DateTime<chrono::Utc>>, published: i64) -> Check {
    match last {
        None if published == 0 => {
            Check::new("link_sweep", Status::Ok, "nothing published to sweep yet")
        }
        None => Check::new(
            "link_sweep",
            Status::Warn,
            "no outbound links have been checked yet; the nightly sweep runs after the first day",
        ),
        Some(at) => {
            let age = chrono::Utc::now() - at;
            if age > chrono::Duration::hours(48) {
                Check::new(
                    "link_sweep",
                    Status::Warn,
                    format!("last sweep {} ago; it runs nightly", human_age(age)),
                )
            } else {
                Check::new(
                    "link_sweep",
                    Status::Ok,
                    format!("last sweep {} ago", human_age(age)),
                )
            }
        }
    }
}

/// Editing locks nobody has renewed: harmless, but a count in the
/// hundreds means the cleanup is not running.
#[must_use]
pub fn stale_locks(stale: i64) -> Check {
    if stale > 100 {
        Check::new(
            "stale_locks",
            Status::Warn,
            format!("{stale} expired editing locks have not been cleared"),
        )
    } else {
        Check::new(
            "stale_locks",
            Status::Ok,
            format!(
                "{stale} expired editing {}",
                if stale == 1 { "lock" } else { "locks" }
            ),
        )
    }
}

/// Enabled webhooks whose newest delivery failed or was given up on.
#[must_use]
pub fn webhooks(failing: i64) -> Check {
    if failing > 0 {
        Check::new(
            "webhooks",
            Status::Warn,
            format!(
                "{failing} enabled {} last delivery failed",
                if failing == 1 {
                    "webhook's"
                } else {
                    "webhooks'"
                }
            ),
        )
        .link("Open webhooks", "/admin/webhooks")
    } else {
        Check::new(
            "webhooks",
            Status::Ok,
            "every enabled webhook's last delivery succeeded",
        )
    }
}

/// Sessions past their expiry that are still on disk.
#[must_use]
pub fn expired_sessions(expired: i64) -> Check {
    if expired > 10_000 {
        Check::new(
            "expired_sessions",
            Status::Warn,
            format!("{expired} expired sessions are still stored"),
        )
    } else {
        Check::new(
            "expired_sessions",
            Status::Ok,
            format!(
                "{expired} expired {}",
                if expired == 1 { "session" } else { "sessions" }
            ),
        )
    }
}

fn human_age(d: chrono::Duration) -> String {
    let mins = d.num_minutes();
    if mins < 60 {
        format!("{mins} min")
    } else if mins < 48 * 60 {
        format!("{} h", mins / 60)
    } else {
        format!("{} days", mins / (60 * 24))
    }
}

/// Reports dead-lettered jobs, which never retry again on their own.
#[must_use]
pub fn job_queue(dead: i64, queued: i64) -> Check {
    if dead > 0 {
        Check::new(
            "job_queue",
            Status::Warn,
            format!("{dead} dead-lettered job(s) need attention; {queued} queued"),
        )
    } else {
        Check::new(
            "job_queue",
            Status::Ok,
            format!("{queued} queued, none dead"),
        )
    }
}

/// Reports plugins the runtime has marked errored or degraded.
#[must_use]
pub fn plugins(unhealthy: i64) -> Check {
    if unhealthy > 0 {
        Check::new(
            "plugins",
            Status::Warn,
            format!("{unhealthy} plugin(s) errored or degraded"),
        )
    } else {
        Check::new("plugins", Status::Ok, "all enabled plugins healthy")
    }
}

/// Reports the library's size against a configured cap.
#[must_use]
pub fn media_storage(used_bytes: i64, cap_bytes: Option<i64>) -> Check {
    let mb = |b: i64| b / (1024 * 1024);
    match cap_bytes {
        None => Check::new(
            "media_storage",
            Status::Ok,
            format!("{} MB of originals, no cap set", mb(used_bytes)),
        ),
        Some(cap) if used_bytes * 10 >= cap * 9 => Check::new(
            "media_storage",
            Status::Warn,
            format!(
                "{} of {} MB used; uploads stop at the cap",
                mb(used_bytes),
                mb(cap)
            ),
        ),
        Some(cap) => Check::new(
            "media_storage",
            Status::Ok,
            format!("{} of {} MB used", mb(used_bytes), mb(cap)),
        ),
    }
}

/// Reports published entries whose outbound links did not answer on the
/// last sweep.
#[must_use]
pub fn broken_links(posts: i64) -> Check {
    if posts > 0 {
        Check::new(
            "broken_links",
            Status::Warn,
            format!(
                "{posts} published entr{} with a broken outbound link",
                if posts == 1 { "y" } else { "ies" }
            ),
        )
    } else {
        Check::new("broken_links", Status::Ok, "no broken outbound links found")
    }
}

/// Reports remaining AI budget for the month.
#[must_use]
pub fn ai_budget(spent_usd: f64, budget_usd: f64) -> Check {
    if budget_usd <= 0.0 {
        return Check::new("ai_budget", Status::Ok, "no budget configured");
    }
    let remaining = budget_usd - spent_usd;
    if remaining <= 0.0 {
        Check::new(
            "ai_budget",
            Status::Warn,
            format!("monthly budget of ${budget_usd:.2} is exhausted; AI features will refuse"),
        )
    } else {
        Check::new(
            "ai_budget",
            Status::Ok,
            format!("${remaining:.2} of ${budget_usd:.2} remaining"),
        )
    }
}

/// Warns when the public site URL is unset, since absolute links depend on it.
#[must_use]
pub fn site_url(configured: &str) -> Check {
    if configured.is_empty() {
        Check::new(
            "site_url",
            Status::Warn,
            "site_url is not set; feeds and emails fall back to the request host",
        )
    } else {
        Check::new("site_url", Status::Ok, configured)
    }
}

/// What the registration check looks at.
#[derive(Debug, Clone, Copy)]
pub struct RegistrationSetup<'a> {
    /// `registration_enabled`.
    pub enabled: bool,
    /// A mail relay is configured, so confirmation links can go out.
    pub mail: bool,
    /// `site_url` is set, so the links have an address to point at.
    pub site_url: bool,
    /// The stored `registration_default_role`.
    pub configured_role: &'a str,
    /// The role a registration made now would actually get.
    pub effective_role: &'a str,
    /// Accounts that registered and have not confirmed their address yet.
    pub unconfirmed: i64,
}

/// "no account awaits confirmation", "1 account awaits ...", "N accounts
/// await ...".
fn awaiting_confirmation(n: i64) -> String {
    match n {
        0 => String::from("no account awaits confirmation"),
        1 => String::from("1 account awaits confirmation"),
        n => format!("{n} accounts await confirmation"),
    }
}

/// Warns when registration is on but cannot work (no mail relay or no
/// site address: every attempt is refused), or when the chosen default
/// role can no longer be given and new accounts silently become
/// subscribers. Every detail ends with how many accounts await
/// confirmation.
#[must_use]
pub fn registration(setup: RegistrationSetup<'_>) -> Check {
    let check = registration_state(setup);
    let detail = format!(
        "{}; {}",
        check.detail,
        awaiting_confirmation(setup.unconfirmed)
    );
    Check::new("registration", check.status, detail)
}

fn registration_state(setup: RegistrationSetup<'_>) -> Check {
    if !setup.enabled {
        return Check::new("registration", Status::Ok, "visitors cannot register");
    }
    let mut missing = Vec::new();
    if !setup.mail {
        missing.push("a mail relay");
    }
    if !setup.site_url {
        missing.push("the site address");
    }
    if !missing.is_empty() {
        return Check::new(
            "registration",
            Status::Warn,
            format!(
                "registration is on, but {} {} not configured, so every registration is \
                 refused until {} set",
                missing.join(" and "),
                if missing.len() == 1 { "is" } else { "are" },
                if missing.len() == 1 {
                    "it is"
                } else {
                    "they are"
                },
            ),
        );
    }
    if setup.configured_role != setup.effective_role {
        return Check::new(
            "registration",
            Status::Warn,
            format!(
                "the default role \"{}\" is missing or can do more than a stranger may be \
                 given (editor-level or administrative capabilities), so new accounts become \
                 {}s; choose another role",
                setup.configured_role, setup.effective_role
            ),
        );
    }
    Check::new(
        "registration",
        Status::Ok,
        format!(
            "visitors can register; new accounts get the role \"{}\"",
            setup.effective_role
        ),
    )
}

#[cfg(test)]
mod registration_tests {
    use super::{registration, RegistrationSetup, Status};

    const READY: RegistrationSetup<'static> = RegistrationSetup {
        enabled: true,
        mail: true,
        site_url: true,
        configured_role: "subscriber",
        effective_role: "subscriber",
        unconfirmed: 0,
    };

    #[test]
    fn registration_says_how_many_accounts_await_confirmation() {
        let detail = |unconfirmed, enabled| {
            registration(RegistrationSetup {
                enabled,
                unconfirmed,
                ..READY
            })
            .detail
        };
        assert!(detail(0, true).contains("no account awaits confirmation"));
        assert!(detail(1, true).contains("1 account awaits confirmation"));
        assert!(detail(7, true).contains("7 accounts await confirmation"));
        // Still worth knowing once registration is off again.
        assert!(detail(3, false).contains("3 accounts await confirmation"));
        // And beside a warning.
        let warned = registration(RegistrationSetup {
            mail: false,
            unconfirmed: 2,
            ..READY
        });
        assert_eq!(warned.status, Status::Warn);
        assert!(warned.detail.contains("2 accounts await confirmation"));
    }

    #[test]
    fn registration_warns_only_when_it_is_on_and_cannot_work_as_set() {
        assert_eq!(registration(READY).status, Status::Ok);
        let off = RegistrationSetup {
            enabled: false,
            mail: false,
            site_url: false,
            ..READY
        };
        assert_eq!(registration(off).status, Status::Ok);
        let no_mail = registration(RegistrationSetup {
            mail: false,
            ..READY
        });
        assert_eq!(no_mail.status, Status::Warn);
        assert!(
            no_mail.detail.contains("a mail relay is not"),
            "{}",
            no_mail.detail
        );
        let neither = registration(RegistrationSetup {
            mail: false,
            site_url: false,
            ..READY
        });
        assert!(
            neither
                .detail
                .contains("a mail relay and the site address are"),
            "{}",
            neither.detail
        );
        let fallback = registration(RegistrationSetup {
            configured_role: "moderators",
            ..READY
        });
        assert_eq!(fallback.status, Status::Warn);
        assert!(
            fallback.detail.contains("\"moderators\"") && fallback.detail.contains("subscriber"),
            "{}",
            fallback.detail
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{ai_budget, job_queue, plugins, search_index, site_url, Report, Status};

    #[test]
    fn the_report_takes_the_worst_status() {
        let report = Report::new(vec![plugins(0), job_queue(2, 0), search_index(Some(5), 5)]);
        assert_eq!(report.status, Status::Warn);
    }

    #[test]
    fn a_single_failure_outranks_any_number_of_warnings() {
        let mut checks = vec![job_queue(1, 0), plugins(1)];
        checks.push(super::Check::new("database", Status::Fail, "down"));
        assert_eq!(Report::new(checks).status, Status::Fail);
    }

    #[test]
    fn an_all_clear_report_is_ok() {
        let report = Report::new(vec![plugins(0), job_queue(0, 3), search_index(Some(2), 2)]);
        assert_eq!(report.status, Status::Ok);
    }

    #[test]
    fn an_index_ahead_of_the_database_is_fine() {
        // Pages are indexed too, so "more documents than posts" was a
        // false alarm on every clean install.
        let check = super::search_index(Some(6), 1);
        assert_eq!(check.status, Status::Ok);
    }

    #[test]
    fn disk_space_warns_then_fails_as_it_runs_out() {
        let gb = 1024 * 1024 * 1024;
        assert_eq!(
            super::disk_free(Some(50 * gb), Some(100 * gb)).status,
            Status::Ok
        );
        assert_eq!(
            super::disk_free(Some(gb + gb / 2), Some(10 * gb)).status,
            Status::Warn
        );
        assert_eq!(
            super::disk_free(Some(100 * 1024 * 1024), Some(100 * gb)).status,
            Status::Fail
        );
    }

    #[test]
    fn a_report_puts_failures_first() {
        let report = super::Report::new(vec![
            super::Check::new("a", Status::Ok, ""),
            super::Check::new("b", Status::Fail, ""),
            super::Check::new("c", Status::Warn, ""),
        ]);
        let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["b", "c", "a"]);
        assert_eq!((report.failures, report.warnings), (1, 1));
    }

    #[test]
    fn an_index_behind_the_database_warns_rather_than_fails() {
        // Search still works via the SQL fallback, so this is not an outage.
        let check = search_index(Some(3), 10);
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("reindex"), "must say what to do");
    }

    #[test]
    fn a_missing_index_is_distinguished_from_a_stale_one() {
        assert_eq!(search_index(None, 10).status, Status::Warn);
        assert!(search_index(None, 10).detail.contains("unavailable"));
    }

    #[test]
    fn an_exhausted_ai_budget_warns() {
        assert_eq!(ai_budget(10.0, 10.0).status, Status::Warn);
        assert_eq!(ai_budget(1.0, 10.0).status, Status::Ok);
        // No budget configured is a deliberate choice, not a problem.
        assert_eq!(ai_budget(500.0, 0.0).status, Status::Ok);
    }

    #[test]
    fn an_unset_site_url_warns_because_absolute_links_depend_on_it() {
        assert_eq!(site_url("").status, Status::Warn);
        assert_eq!(site_url("https://example.test").status, Status::Ok);
    }
}

#[cfg(test)]
mod media_storage_tests {
    use super::{media_storage, Status};

    #[test]
    fn storage_warns_near_the_cap_and_is_calm_without_one() {
        let mb = 1024 * 1024;
        assert_eq!(media_storage(500 * mb, None).status, Status::Ok);
        assert_eq!(media_storage(500 * mb, Some(1000 * mb)).status, Status::Ok);
        assert_eq!(
            media_storage(950 * mb, Some(1000 * mb)).status,
            Status::Warn
        );
    }
}
