//! The marketplace and update channel this build trusts.
//!
//! Compiled in, deliberately: whoever can change where packages and
//! releases come from, and whose signature makes them trusted, can take
//! the site over. Only the server's own configuration may point at a
//! mirror (same keys) or switch a source off.

use serde::Serialize;
use vyasa_common::VyasaConfig;

/// The official marketplace index.
pub const MARKETPLACE_URL: &str = "https://marketplace.vyasa.site/index.json";
/// The official release manifest.
pub const UPDATES_URL: &str = "https://updates.vyasa.site/stable.json";
/// Keys that sign marketplace packages: the live key, then the spare
/// kept offline for rotation.
pub const MARKETPLACE_KEYS: &[&str] = &[
    "5018b58f3167bed3f08fba4680f7ddf84a17c96aed43ae147f0a30e2d2f188ff",
    "c6e47c76bc3680ded88ff4aa14d02d4ab2a475d3dd8209c76f84385d248026d5",
];
/// Keys that sign core release tarballs: live, then spare.
pub const RELEASE_KEYS: &[&str] = &[
    "134772c6aa4faefc8bb225a70b96b5f82ec583114904bad55483ecd9d2c536e9",
    "a96a59d91d79c40e1716aa1e4822fb0bbef1aa57e13302add7ed00018776323f",
];

/// Where a source points, as the admin is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// The compiled-in address.
    Official,
    /// An operator's mirror of it.
    Mirror,
    /// Switched off by the operator.
    Off,
}

/// A resolved source: address (empty when off) and trusted keys.
#[derive(Debug, Clone)]
pub struct Source {
    pub state: SourceState,
    pub url: String,
    pub keys: Vec<String>,
}

fn resolve(
    config: &vyasa_common::config::SourceConfig,
    official: &str,
    keys: &[&str],
    test_var: &str,
    bind_port: u16,
) -> Source {
    let keys: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
    if !config.enabled {
        return Source {
            state: SourceState::Off,
            url: String::new(),
            keys,
        };
    }
    if let Some(over) = test_override(test_var, bind_port) {
        return over;
    }
    let mirror = config.mirror_url.trim();
    if mirror.is_empty() {
        Source {
            state: SourceState::Official,
            url: official.to_owned(),
            keys,
        }
    } else {
        Source {
            state: SourceState::Mirror,
            url: mirror.to_owned(),
            keys,
        }
    }
}

/// The marketplace this install uses.
#[must_use]
pub fn marketplace(config: &VyasaConfig) -> Source {
    resolve(
        &config.marketplace,
        MARKETPLACE_URL,
        MARKETPLACE_KEYS,
        "MARKETPLACE",
        config.bind_addr_port(),
    )
}

/// The update channel this install uses.
#[must_use]
pub fn updates(config: &VyasaConfig) -> Source {
    resolve(
        &config.updates,
        UPDATES_URL,
        RELEASE_KEYS,
        "UPDATES",
        config.bind_addr_port(),
    )
}

/// Whether `url` may be fetched: https, or — in debug builds running a
/// test override only — plain http to the test server.
#[must_use]
pub fn transport_ok(url: &str) -> bool {
    url.starts_with("https://") || (cfg!(debug_assertions) && test_override_active())
}

/// Test-only source, honoured in debug builds alone so no release binary
/// can be pointed elsewhere by its environment. `VYASA_TESTONLY_<NAME>_URL`
/// starting with `/` is relative to this server's own address (its
/// `registry_dir` hosting); `VYASA_TESTONLY_<NAME>_KEYS` is comma-separated.
#[cfg(debug_assertions)]
fn test_override(name: &str, bind_port: u16) -> Option<Source> {
    let url = std::env::var(format!("VYASA_TESTONLY_{name}_URL")).ok()?;
    let url = if url.starts_with('/') {
        format!("http://127.0.0.1:{bind_port}{url}")
    } else {
        url
    };
    let keys = std::env::var(format!("VYASA_TESTONLY_{name}_KEYS"))
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    Some(Source {
        state: SourceState::Official,
        url,
        keys,
    })
}

#[cfg(not(debug_assertions))]
fn test_override(_name: &str, _bind_port: u16) -> Option<Source> {
    None
}

fn test_override_active() -> bool {
    cfg!(debug_assertions)
        && (std::env::var_os("VYASA_TESTONLY_MARKETPLACE_URL").is_some()
            || std::env::var_os("VYASA_TESTONLY_UPDATES_URL").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> VyasaConfig {
        let path = std::env::temp_dir().join(format!(
            "vyasa-official-{}-{}.toml",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(
            &path,
            format!("database_url = \"postgres://u:p@localhost/x\"\n{toml}"),
        )
        .unwrap();
        VyasaConfig::load_from_file(path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn the_compiled_in_keys_are_valid_ed25519_keys() {
        for key in MARKETPLACE_KEYS.iter().chain(RELEASE_KEYS) {
            let bytes: [u8; 32] = hex::decode(key).unwrap().try_into().unwrap();
            ed25519_dalek::VerifyingKey::from_bytes(&bytes).unwrap();
        }
        assert_eq!(MARKETPLACE_KEYS.len(), 2);
        assert_eq!(RELEASE_KEYS.len(), 2);
    }

    #[test]
    fn official_by_default() {
        let m = marketplace(&config(""));
        assert_eq!(m.state, SourceState::Official);
        assert_eq!(m.url, MARKETPLACE_URL);
        assert_eq!(
            m.keys,
            MARKETPLACE_KEYS
                .iter()
                .map(|k| (*k).to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(updates(&config("")).url, UPDATES_URL);
    }

    #[test]
    fn a_mirror_changes_the_address_not_the_keys() {
        let m = marketplace(&config(
            "[marketplace]\nmirror_url = \"https://m.example/index.json\"\n",
        ));
        assert_eq!(m.state, SourceState::Mirror);
        assert_eq!(m.url, "https://m.example/index.json");
        assert_eq!(m.keys.len(), 2);
    }

    #[test]
    fn off_means_no_address() {
        let u = updates(&config("[updates]\nenabled = false\n"));
        assert_eq!(u.state, SourceState::Off);
        assert!(u.url.is_empty());
    }
}
