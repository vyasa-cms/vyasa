//! The registry index document, and reading it.

use serde::{Deserialize, Serialize};
use vyasa_common::AppError;

/// What a listing installs into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A `.vyplugin` package.
    Plugin,
    /// A `.vytheme` package.
    Theme,
}

impl Kind {
    /// The capability an operator needs to install this kind.
    #[must_use]
    pub const fn capability(self) -> vyasa_core::user::Capability {
        match self {
            Self::Plugin => vyasa_core::user::Capability::ManagePlugins,
            Self::Theme => vyasa_core::user::Capability::ManageThemes,
        }
    }
}

/// One downloadable version of a listing.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
// `version` reads as `Version::version` here on purpose: the field is the
// version *string*, and renaming it would break the published index format.
#[allow(clippy::struct_field_names)]
pub struct Version {
    /// Version string. Plugins are semver; themes are integers, so this
    /// is compared as semver when it parses and as a string otherwise.
    pub version: String,
    /// Where the package lives.
    pub url: String,
    /// Hex SHA-256 of the package, always checked.
    pub sha256: String,
    /// Hex ed25519 signature over the package bytes, when the registry
    /// signs its artifacts.
    #[serde(default)]
    pub signature: Option<String>,
    /// Host API a plugin needs (`PluginManifest::min_host_api`).
    #[serde(default)]
    pub min_host_api: Option<u32>,
    /// Token schema a theme targets (`ThemeManifest::required_api`).
    #[serde(default)]
    pub required_api: Option<u32>,
    /// Capabilities a plugin requests — shown *before* installing, and
    /// diffed against what is already installed before updating.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Publication date, informational.
    #[serde(default)]
    pub released_at: Option<String>,
}

/// One plugin or theme on offer.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Listing {
    /// `plugin` or `theme`.
    pub kind: Kind,
    /// Package name, matching the name inside the package.
    pub name: String,
    /// Display title.
    #[serde(default)]
    pub title: String,
    /// One-line description.
    #[serde(default)]
    pub summary: String,
    /// Author display string.
    #[serde(default)]
    pub author: String,
    /// Project or documentation URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Hex ed25519 key the plugin's author signs with. The marketplace
    /// signature over the bytes vouches for it; absent, the package must
    /// be author-signed by a marketplace key.
    #[serde(default)]
    pub author_key: Option<String>,
    /// Versions, newest first after [`Listing::sorted`].
    #[serde(default)]
    pub versions: Vec<Version>,
}

impl Listing {
    /// The newest version on offer.
    #[must_use]
    pub fn latest(&self) -> Option<&Version> {
        self.versions.first()
    }

    /// The named version.
    #[must_use]
    pub fn version(&self, version: &str) -> Option<&Version> {
        self.versions.iter().find(|v| v.version == version)
    }

    /// Display title, falling back to the package name.
    #[must_use]
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            &self.name
        } else {
            &self.title
        }
    }
}

/// The whole registry document.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Index {
    /// Document schema version; unknown majors are refused.
    #[serde(default = "one")]
    pub schema: u32,
    /// When the index was generated, informational.
    #[serde(default)]
    pub generated_at: Option<String>,
    /// Everything on offer.
    #[serde(default)]
    pub listings: Vec<Listing>,
}

const fn one() -> u32 {
    1
}

/// The schema this build understands.
pub const SCHEMA_VERSION: u32 = 1;

impl Index {
    /// Listings of one kind, newest-version-first inside each.
    #[must_use]
    pub fn of_kind(&self, kind: Kind) -> Vec<Listing> {
        let mut out: Vec<Listing> = self
            .listings
            .iter()
            .filter(|l| l.kind == kind)
            .cloned()
            .map(sorted)
            .collect();
        out.sort_by(|a, b| {
            a.display_title()
                .to_lowercase()
                .cmp(&b.display_title().to_lowercase())
        });
        out
    }

    /// One listing by kind and name.
    #[must_use]
    pub fn find(&self, kind: Kind, name: &str) -> Option<Listing> {
        self.listings
            .iter()
            .find(|l| l.kind == kind && l.name == name)
            .cloned()
            .map(sorted)
    }
}

/// Newest version first. Semver where both parse, string order otherwise
/// — theme versions are integers, which orders correctly as numbers of
/// equal width and is only ever a display concern besides.
fn sorted(mut listing: Listing) -> Listing {
    listing.versions.sort_by(|a, b| {
        match (
            semver::Version::parse(&a.version),
            semver::Version::parse(&b.version),
        ) {
            (Ok(x), Ok(y)) => y.cmp(&x),
            _ => match (a.version.parse::<u64>(), b.version.parse::<u64>()) {
                (Ok(x), Ok(y)) => y.cmp(&x),
                _ => b.version.cmp(&a.version),
            },
        }
    });
    listing
}

/// True when `candidate` is a later version than `installed`.
///
/// Falls back to integer then string comparison, so theme packages
/// (whose versions are plain integers) compare correctly too.
#[must_use]
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(installed),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => match (candidate.parse::<u64>(), installed.parse::<u64>()) {
            (Ok(a), Ok(b)) => a > b,
            _ => candidate > installed,
        },
    }
}

/// Maximum index size; a marketplace document is text, not a payload.
const MAX_INDEX_BYTES: usize = 4 * 1024 * 1024;

/// Fetches and parses the index.
///
/// # Errors
/// [`AppError::Validation`] when no registry is configured, the document
/// does not parse, or its schema is newer than this build understands;
/// [`AppError::Internal`] on transport failures.
pub async fn fetch(url: &str) -> Result<Index, AppError> {
    if url.trim().is_empty() {
        return Err(AppError::validation(
            "the marketplace is turned off on this server",
        ));
    }
    if !crate::official::transport_ok(url.trim_start()) {
        return Err(AppError::validation(
            "the marketplace index must be an https URL",
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| AppError::internal_msg(e.to_string()))?;
    let response = client
        .get(url)
        .header("user-agent", format!("vyasa/{}", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .map_err(|e| AppError::internal_msg(format!("marketplace unreachable: {e}")))?;
    if !response.status().is_success() {
        return Err(AppError::internal_msg(format!(
            "marketplace answered {}",
            response.status()
        )));
    }
    let body = crate::net_guard::read_capped(response, MAX_INDEX_BYTES)
        .await
        .map_err(|e| match e {
            crate::net_guard::BodyError::TooLarge => {
                AppError::validation("marketplace index is implausibly large")
            }
            crate::net_guard::BodyError::Transport(e) => AppError::internal_msg(e.to_string()),
        })?;
    let index: Index = serde_json::from_slice(&body)
        .map_err(|e| AppError::validation(format!("marketplace index is not readable: {e}")))?;
    if index.schema > SCHEMA_VERSION {
        return Err(AppError::validation(format!(
            "this marketplace speaks index schema {} and this build understands {SCHEMA_VERSION} \
             — upgrade Vyasa to install from it",
            index.schema
        )));
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::{is_newer, Index, Kind, Listing, Version};

    fn version(v: &str) -> Version {
        Version {
            version: v.to_owned(),
            url: format!("https://reg.test/{v}.pkg"),
            sha256: "0".repeat(64),
            signature: None,
            min_host_api: None,
            required_api: None,
            capabilities: Vec::new(),
            released_at: None,
        }
    }

    fn listing(kind: Kind, name: &str, title: &str, versions: Vec<Version>) -> Listing {
        Listing {
            kind,
            name: name.to_owned(),
            title: title.to_owned(),
            summary: String::new(),
            author: String::new(),
            homepage: None,
            author_key: None,
            versions,
        }
    }

    fn index() -> Index {
        Index {
            schema: 1,
            generated_at: None,
            listings: vec![
                listing(
                    Kind::Plugin,
                    "storefront",
                    "Storefront",
                    vec![version("1.0.0"), version("1.10.0"), version("1.2.0")],
                ),
                listing(Kind::Theme, "aurora", "", vec![version("2"), version("10")]),
            ],
        }
    }

    #[test]
    fn versions_come_back_newest_first_for_both_schemes() {
        let plugins = index().of_kind(Kind::Plugin);
        assert_eq!(plugins.len(), 1);
        let versions: Vec<&str> = plugins[0]
            .versions
            .iter()
            .map(|v| v.version.as_str())
            .collect();
        assert_eq!(
            versions,
            ["1.10.0", "1.2.0", "1.0.0"],
            "semver, not string order"
        );

        let themes = index().of_kind(Kind::Theme);
        let versions: Vec<&str> = themes[0]
            .versions
            .iter()
            .map(|v| v.version.as_str())
            .collect();
        assert_eq!(versions, ["10", "2"], "theme versions are integers");
    }

    #[test]
    fn kinds_are_kept_apart_and_findable() {
        let idx = index();
        assert!(idx.find(Kind::Plugin, "storefront").is_some());
        assert!(
            idx.find(Kind::Theme, "storefront").is_none(),
            "a plugin must not be installable as a theme"
        );
        assert_eq!(
            idx.find(Kind::Theme, "aurora")
                .expect("theme")
                .display_title(),
            "aurora",
            "a listing with no title falls back to its package name"
        );
    }

    #[test]
    fn newer_understands_semver_integers_and_neither() {
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(!is_newer("1.9.0", "1.10.0"));
        assert!(is_newer("10", "2"), "theme versions are integers");
        assert!(!is_newer("2", "10"));
        assert!(!is_newer("1.0.0", "1.0.0"));
    }
}

#[cfg(test)]
mod fetch_tests {
    #[tokio::test]
    async fn a_plain_http_index_is_refused_before_any_request() {
        let err = super::fetch("http://marketplace.example.com/index.json")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("https"), "{err}");
    }
}
