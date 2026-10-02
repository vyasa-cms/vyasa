//! The release manifest: what versions exist and how to verify one.
//!
//! A small signed JSON document rather than a package index. It carries
//! enough to answer "may I upgrade, and to what" *before* downloading
//! anything: the migration count, the plugin contract revision, and the
//! oldest version each release can be reached from.

use serde::{Deserialize, Serialize};
use vyasa_common::AppError;

/// The build's own version, stamped at compile time.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the manifest lives; empty disables update checks entirely.
pub const CHANNEL_OPTION: &str = "update_channel_url";

/// One downloadable build.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Artifact {
    /// Rust target triple, e.g. `x86_64-unknown-linux-gnu`.
    pub platform: String,
    /// Tarball holding the binary and the matching admin bundle.
    pub url: String,
    /// Hex SHA-256 of the tarball. Always checked.
    pub sha256: String,
    /// Hex ed25519 signature over the tarball bytes, when the channel
    /// signs its artifacts.
    #[serde(default)]
    pub signature: Option<String>,
}

/// One release.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Release {
    /// Semantic version.
    pub version: String,
    /// Human-readable notes.
    #[serde(default)]
    pub notes_url: Option<String>,
    /// One-line summary for the admin panel.
    #[serde(default)]
    pub summary: String,
    /// Oldest version that may upgrade straight to this one.
    #[serde(default)]
    pub min_upgrade_from: Option<String>,
    /// Highest migration this release carries, so the preflight can say
    /// how far the schema moves before anything is downloaded.
    #[serde(default)]
    pub migration_version: Option<i64>,
    /// Plugin WIT contract this release speaks. A change here means
    /// installed plugins must be rebuilt.
    #[serde(default)]
    pub plugin_contract: Option<String>,
    /// Whether this release requires operator attention beyond the
    /// upgrade itself (a breaking change, a manual step).
    #[serde(default)]
    pub requires_attention: bool,
    /// Builds, by platform.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

impl Release {
    /// The artifact for this machine, if the release ships one.
    #[must_use]
    pub fn artifact_for_host(&self) -> Option<&Artifact> {
        let host = host_target();
        self.artifacts.iter().find(|a| a.platform == host)
    }
}

/// The whole channel document.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Manifest {
    /// Newest version the channel offers.
    pub latest: String,
    /// Every release still described, newest first.
    #[serde(default)]
    pub releases: Vec<Release>,
}

impl Manifest {
    /// The release matching `version`.
    #[must_use]
    pub fn release(&self, version: &str) -> Option<&Release> {
        self.releases.iter().find(|r| r.version == version)
    }

    /// Releases strictly newer than `from`, newest first. An unparsable
    /// version on either side yields nothing rather than a wrong answer.
    #[must_use]
    pub fn newer_than(&self, from: &str) -> Vec<&Release> {
        let Ok(current) = semver::Version::parse(from) else {
            return Vec::new();
        };
        let mut out: Vec<&Release> = self
            .releases
            .iter()
            .filter(|r| semver::Version::parse(&r.version).is_ok_and(|v| v > current))
            .collect();
        out.sort_by(|a, b| b.version.cmp(&a.version));
        out
    }
}

/// The target triple this binary was built for.
#[must_use]
pub fn host_target() -> String {
    // Stamped by build.rs; falls back to the compile-time arch/os pair so
    // a build without the stamp still matches the common Linux artifact.
    option_env!("VYASA_TARGET").map_or_else(
        || {
            format!(
                "{}-unknown-{}-gnu",
                std::env::consts::ARCH,
                std::env::consts::OS
            )
        },
        ToOwned::to_owned,
    )
}

/// Largest channel document accepted: 16 MiB. A manifest lists releases
/// and their digests; anything near this size is not one.
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

/// Fetches and parses the channel document.
///
/// # Errors
/// [`AppError::Validation`] when the URL is unset or the document does
/// not parse; [`AppError::Internal`] on transport failures.
pub async fn fetch(url: &str) -> Result<Manifest, AppError> {
    if url.trim().is_empty() {
        return Err(AppError::validation(
            "no update channel is configured (set update_channel_url)",
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| AppError::internal_msg(e.to_string()))?;
    let response = client
        .get(url)
        .header("user-agent", format!("vyasa/{CURRENT_VERSION}"))
        .send()
        .await
        .map_err(|e| AppError::internal_msg(format!("update channel unreachable: {e}")))?;
    if !response.status().is_success() {
        return Err(AppError::internal_msg(format!(
            "update channel answered {}",
            response.status()
        )));
    }
    let body = crate::net_guard::read_capped(response, MAX_MANIFEST_BYTES)
        .await
        .map_err(|e| match e {
            crate::net_guard::BodyError::TooLarge => {
                AppError::validation("update manifest is implausibly large")
            }
            crate::net_guard::BodyError::Transport(e) => AppError::internal_msg(e.to_string()),
        })?;
    serde_json::from_slice(&body)
        .map_err(|e| AppError::validation(format!("update manifest is not readable: {e}")))
}

/// Compares two versions, `None` when either is unparsable.
#[must_use]
pub fn is_newer(candidate: &str, than: &str) -> Option<bool> {
    let a = semver::Version::parse(candidate).ok()?;
    let b = semver::Version::parse(than).ok()?;
    Some(a > b)
}

/// Whether the jump from `from` to `release` is one the release allows.
#[must_use]
pub fn reachable_from(release: &Release, from: &str) -> bool {
    let Some(min) = release.min_upgrade_from.as_deref() else {
        return true;
    };
    match (semver::Version::parse(from), semver::Version::parse(min)) {
        (Ok(current), Ok(minimum)) => current >= minimum,
        // An unreadable bound is not a licence to ignore it.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_newer, reachable_from, Manifest, Release};

    fn release(version: &str, min_from: Option<&str>) -> Release {
        Release {
            version: version.to_owned(),
            notes_url: None,
            summary: String::new(),
            min_upgrade_from: min_from.map(ToOwned::to_owned),
            migration_version: None,
            plugin_contract: None,
            requires_attention: false,
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn newer_releases_are_listed_newest_first() {
        let manifest = Manifest {
            latest: "1.3.0".into(),
            releases: vec![
                release("1.2.0", None),
                release("1.3.0", None),
                release("1.0.0", None),
            ],
        };
        let newer: Vec<&str> = manifest
            .newer_than("1.1.0")
            .iter()
            .map(|r| r.version.as_str())
            .collect();
        assert_eq!(newer, ["1.3.0", "1.2.0"]);
        assert!(manifest.newer_than("9.9.9").is_empty());
        assert!(
            manifest.newer_than("not-a-version").is_empty(),
            "an unreadable current version offers nothing rather than everything"
        );
    }

    #[test]
    fn upgrade_floors_are_enforced_and_unreadable_ones_refuse() {
        let gated = release("2.0.0", Some("1.5.0"));
        assert!(reachable_from(&gated, "1.5.0"));
        assert!(reachable_from(&gated, "1.9.2"));
        assert!(!reachable_from(&gated, "1.4.9"));
        assert!(!reachable_from(&gated, "nonsense"));
        assert!(reachable_from(&release("1.1.0", None), "1.0.0"));
    }

    #[test]
    fn version_comparison_is_semantic_not_lexical() {
        assert_eq!(is_newer("1.10.0", "1.9.0"), Some(true));
        assert_eq!(is_newer("1.9.0", "1.10.0"), Some(false));
        assert_eq!(is_newer("1.0.0", "1.0.0"), Some(false));
        assert_eq!(is_newer("x", "1.0.0"), None);
    }
}
