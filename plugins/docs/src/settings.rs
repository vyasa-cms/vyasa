//! The plugin's settings form and what the importer makes of it.

use serde_json::{json, Value};

use crate::github;
use crate::sync::Config;

/// Most older versions one site may declare.
pub const MAX_VERSIONS: usize = 12;

/// The values the operator saved, with defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `https://github.com/owner/name`.
    pub repo_url: String,
    /// Branch to follow.
    pub branch: String,
    /// Folder holding the docs.
    pub folder: String,
    /// Access token (write-only in the admin).
    pub token: String,
    /// Shared secret for the webhook (write-only in the admin).
    pub webhook_secret: String,
    /// Minutes between checks when no webhook arrives.
    pub poll_minutes: u64,
    /// Label of the version served at `/docs`; empty when unversioned.
    pub latest_version: String,
    /// What the version switcher calls the latest version.
    pub latest_label: String,
    /// Older versions, one `label = folder` per line.
    pub versions: String,
    /// Breadcrumb root label.
    pub docs_title: String,
    /// Search box placeholder.
    pub search_placeholder: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            repo_url: String::new(),
            branch: "main".to_owned(),
            folder: "docs".to_owned(),
            token: String::new(),
            webhook_secret: String::new(),
            poll_minutes: 60,
            latest_version: String::new(),
            latest_label: "Latest".to_owned(),
            versions: String::new(),
            docs_title: "Docs".to_owned(),
            search_placeholder: "Search the docs".to_owned(),
        }
    }
}

impl Settings {
    /// Reads the JSON object `init` receives; missing or mistyped values
    /// fall back to the defaults.
    #[must_use]
    pub fn from_json(json: &str) -> Self {
        let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
        let d = Self::default();
        let text = |k: &str, fallback: &str| {
            v[k].as_str()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fallback.to_owned())
        };
        let raw = |k: &str| v[k].as_str().map(str::to_owned).unwrap_or_default();
        Self {
            repo_url: raw("repo_url").trim().to_owned(),
            branch: text("branch", &d.branch),
            folder: v["folder"]
                .as_str()
                .map_or(d.folder.clone(), |s| s.trim().to_owned()),
            token: raw("token").trim().to_owned(),
            webhook_secret: raw("webhook_secret"),
            poll_minutes: v["poll_minutes"]
                .as_u64()
                .or_else(|| v["poll_minutes"].as_f64().map(|f| f as u64))
                .unwrap_or(d.poll_minutes)
                .clamp(5, 1440),
            latest_version: raw("latest_version").trim().to_owned(),
            latest_label: text("latest_label", &d.latest_label),
            versions: raw("versions"),
            docs_title: text("docs_title", &d.docs_title),
            search_placeholder: text("search_placeholder", &d.search_placeholder),
        }
    }

    /// Stored form (kv), the inverse of [`Settings::from_json`].
    #[must_use]
    pub fn to_json(&self) -> String {
        json!({
            "repo_url": self.repo_url, "branch": self.branch, "folder": self.folder, "token": self.token,
            "webhook_secret": self.webhook_secret, "poll_minutes": self.poll_minutes,
            "latest_version": self.latest_version, "latest_label": self.latest_label, "versions": self.versions,
            "docs_title": self.docs_title, "search_placeholder": self.search_placeholder,
        })
        .to_string()
    }

    /// Whether a repository has been set at all.
    #[must_use]
    pub fn configured(&self) -> bool {
        !self.repo_url.is_empty()
    }

    /// The importer's configuration.
    ///
    /// # Errors
    /// A message for the operator naming the setting that is wrong.
    pub fn sync_config(&self) -> Result<Config, String> {
        let repo = github::parse_repo(&self.repo_url).map_err(|e| format!("Repository: {e}"))?;
        let folder = github::folder_segments(&self.folder).map_err(|e| format!("Folder: {e}"))?;
        if self.branch.is_empty()
            || self.branch.len() > 200
            || self
                .branch
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("Branch: must be a branch name without spaces".to_owned());
        }
        let versions = parse_versions(&self.versions)?;
        Ok(Config {
            repo,
            branch: self.branch.clone(),
            folder,
            token: Some(self.token.clone()).filter(|t| !t.is_empty()),
            latest_version: self.latest_version.clone(),
            versions,
        })
    }
}

/// `label = folder` lines.
///
/// # Errors
/// A message naming the bad line.
pub fn parse_versions(text: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (label, folder) = line
            .split_once('=')
            .ok_or_else(|| format!("Older versions: `{line}` is not `label = folder`"))?;
        let label = label.trim();
        if label.is_empty()
            || label.len() > 40
            || !label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return Err(format!(
                "Older versions: `{label}` is not a usable version label (letters, digits, . - _)"
            ));
        }
        let folder = github::folder_segments(folder).map_err(|e| format!("Older versions: {e}"))?;
        if out.iter().any(|(l, _)| l == label) {
            return Err(format!("Older versions: {label} appears twice"));
        }
        out.push((label.to_owned(), folder));
    }
    if out.len() > MAX_VERSIONS {
        return Err(format!("Older versions: at most {MAX_VERSIONS}"));
    }
    Ok(out)
}

/// The `register-admin` form.
#[must_use]
pub fn admin_form() -> Value {
    json!({
        "title": "Documentation",
        "description": "Imports Markdown from a GitHub repository into the Docs section. Pages that come from the repository are read-only here: edit them on GitHub and they follow within the polling interval, or at once through the webhook.",
        "fields": [
            {"key": "repo_url", "label": "Repository", "type": "text", "help": "https://github.com/owner/name"},
            {"key": "branch", "label": "Branch", "type": "text", "default": "main"},
            {"key": "folder", "label": "Folder", "type": "text", "default": "docs", "help": "The folder with the Markdown files; leave empty for the repository root."},
            {"key": "token", "label": "Access token", "type": "secret", "help": "Only for a private repository, or to raise GitHub's rate limit. A fine-grained token with read access to Contents is enough."},
            {"key": "webhook_secret", "label": "Webhook secret", "type": "secret", "help": "Add a GitHub webhook (push events, content type application/json) to /api/v1/plugin/vyasa-docs/webhook with this secret to sync on every push."},
            {"key": "poll_minutes", "label": "Check every (minutes)", "type": "number", "default": 60, "help": "Between 5 and 1440. A check with nothing new costs one conditional request."},
            {"key": "latest_version", "label": "Latest version label", "type": "text", "help": "Leave empty for unversioned docs. Pages without a version belong to this one and are served at /docs."},
            {"key": "latest_label", "label": "Name for the latest version", "type": "text", "default": "Latest"},
            {"key": "versions", "label": "Older versions", "type": "textarea", "help": "One per line: `v1 = docs-v1` (label = folder in the same repository). They are served at /docs/<label>/ and carry a notice pointing to the latest."},
            {"key": "docs_title", "label": "Breadcrumb root", "type": "text", "default": "Docs"},
            {"key": "search_placeholder", "label": "Search placeholder", "type": "text", "default": "Search the docs"}
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_an_empty_or_broken_config() {
        for j in ["", "{}", "null", "[1]"] {
            let s = Settings::from_json(j);
            assert_eq!(s.branch, "main");
            assert_eq!(s.folder, "docs");
            assert_eq!(s.poll_minutes, 60);
            assert!(!s.configured());
        }
    }

    #[test]
    fn values_round_trip_and_the_interval_is_clamped() {
        let s = Settings::from_json(
            r#"{"repo_url":" https://github.com/o/r ","branch":"dev","folder":"","token":"t","webhook_secret":"w","poll_minutes":1,"latest_version":"v2","versions":"v1 = docs-v1"}"#,
        );
        assert_eq!(s.repo_url, "https://github.com/o/r");
        assert_eq!(s.folder, "");
        assert_eq!(s.poll_minutes, 5);
        assert_eq!(Settings::from_json(&s.to_json()), s);
        assert_eq!(
            Settings::from_json(r#"{"poll_minutes": 99999}"#).poll_minutes,
            1440
        );
        assert_eq!(
            Settings::from_json(r#"{"poll_minutes": 30.0}"#).poll_minutes,
            30
        );
    }

    #[test]
    fn the_sync_config_validates_each_setting_by_name() {
        let ok = Settings::from_json(
            r#"{"repo_url":"https://github.com/o/r","folder":"docs/site","token":"t","versions":"v1 = old\n# comment\nv0.9 = older/x"}"#,
        );
        let c = ok.sync_config().unwrap();
        assert_eq!(c.folder, ["docs", "site"]);
        assert_eq!(c.token.as_deref(), Some("t"));
        assert_eq!(c.versions.len(), 2);
        assert!(
            Settings::from_json(r#"{"repo_url":"https://example.com/o/r"}"#)
                .sync_config()
                .unwrap_err()
                .starts_with("Repository")
        );
        assert!(
            Settings::from_json(r#"{"repo_url":"https://github.com/o/r","folder":"../x"}"#)
                .sync_config()
                .unwrap_err()
                .starts_with("Folder")
        );
        assert!(
            Settings::from_json(r#"{"repo_url":"https://github.com/o/r","branch":"a b"}"#)
                .sync_config()
                .unwrap_err()
                .starts_with("Branch")
        );
        assert!(Settings::from_json(
            r#"{"repo_url":"https://github.com/o/r","versions":"nonsense"}"#
        )
        .sync_config()
        .unwrap_err()
        .starts_with("Older versions"));
        assert!(
            Settings::from_json(r#"{"repo_url":"https://github.com/o/r"}"#)
                .sync_config()
                .unwrap()
                .token
                .is_none()
        );
    }

    #[test]
    fn versions_reject_duplicates_and_odd_labels() {
        assert!(parse_versions("v1 = a\nv1 = b").is_err());
        assert!(parse_versions("v 1 = a").is_err());
        assert!(parse_versions("v1 = ../a").is_err());
        assert_eq!(parse_versions("").unwrap(), vec![]);
    }

    #[test]
    fn the_form_declares_secrets_as_secrets_and_every_key_is_read_back() {
        let form = admin_form();
        let fields = form["fields"].as_array().unwrap();
        let secret: Vec<_> = fields
            .iter()
            .filter(|f| f["type"] == "secret")
            .map(|f| f["key"].as_str().unwrap())
            .collect();
        assert_eq!(secret, ["token", "webhook_secret"]);
        let mut all = serde_json::Map::new();
        for f in fields {
            all.insert(f["key"].as_str().unwrap().to_owned(), json!("x"));
        }
        let parsed: Value =
            serde_json::from_str(&Settings::from_json(&Value::Object(all).to_string()).to_json())
                .unwrap();
        for f in fields {
            assert!(
                parsed.get(f["key"].as_str().unwrap()).is_some(),
                "{} is not read by Settings",
                f["key"]
            );
        }
    }
}
