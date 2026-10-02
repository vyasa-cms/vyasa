//! Typed application configuration.
//!
//! Sources, lowest to highest precedence:
//! 1. built-in defaults (see [`Default`] impls),
//! 2. optional `vyasa.toml` file in the working directory,
//! 3. `VYASA_*` environment variables (`__` separates nesting, e.g.
//!    `VYASA_LOG__LEVEL=debug`).
//!
//! Secrets ([`Secret`]) never leak through `Debug` or `Display`.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

use serde::Deserialize;

/// A string whose value is redacted from `Debug` and `Display` output.
///
/// Use this for database URLs, API keys, SMTP passwords, and anything else
/// that must never reach logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps `value` as a secret.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Reveals the secret value. Only call this where the value is
    /// actually needed (opening a connection, sending a request).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret").field("value", &"***").finish()
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Secret::new)
    }
}

/// Log output format.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable output (development default).
    #[default]
    Pretty,
    /// Structured JSON lines (production/ingestion).
    Json,
}

/// Tracing/logging settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LogConfig {
    /// Log level or filter directive, e.g. `"info"` or `"vyasa=debug"`.
    pub level: String,
    /// Output format.
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: LogFormat::default(),
        }
    }
}

/// Background job settings.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct JobsConfig {
    /// Number of queue worker tasks (1..=64).
    pub workers: u32,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self { workers: 2 }
    }
}

/// Public sandbox mode (`VYASA_DEMO__ENABLED=true`).
///
/// Visitors sign in with the shared account below and may edit freely,
/// but nothing that runs code, sends mail, reaches other servers, stores
/// credentials or locks the next visitor out is allowed. Every such
/// refusal is decided in the API's `policy` module.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DemoConfig {
    /// Whether this install is a public demo.
    pub enabled: bool,
    /// The shared account's email (what the sign-in form asks for), shown
    /// on every page of the demo.
    pub email: String,
    /// The shared account's password, shown on the sign-in page. Not a
    /// secret: the whole point is that everyone has it.
    pub password: String,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            email: "demo@vyasa.site".to_string(),
            password: "demo".to_string(),
        }
    }
}

/// Cloud AI provider settings (keys optional until the AI features run).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// Anthropic API key (primary provider).
    pub anthropic_key: Option<Secret>,
    /// OpenAI API key (fallback provider).
    pub openai_key: Option<Secret>,
    /// Total monthly AI spend cap in USD; AI features degrade gracefully
    /// when exceeded.
    pub monthly_budget_usd: f64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            anthropic_key: None,
            openai_key: None,
            monthly_budget_usd: 25.0,
        }
    }
}

/// Object storage for media; `None` keeps files on local disk.
///
/// Any S3-compatible service: AWS S3, Cloudflare R2, Backblaze B2, MinIO.
/// Local disk is the default and is correct for one server; it is the
/// wrong answer for several, because each would hold different files.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// `local` or `s3`.
    pub provider: String,
    /// Bucket name.
    pub bucket: String,
    /// Region; `auto` suits R2 and most MinIO deployments.
    pub region: String,
    /// Service endpoint, e.g. `https://s3.eu-west-1.amazonaws.com` or an
    /// R2/MinIO address. Required: guessing it from the region would only
    /// ever be right for AWS.
    pub endpoint: String,
    /// Access key id.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: Option<Secret>,
    /// Address objects as `{endpoint}/{bucket}/{key}` rather than as a
    /// virtual host. MinIO needs this; R2 and S3 accept it.
    pub path_style: bool,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            provider: String::from("local"),
            bucket: String::new(),
            region: String::from("auto"),
            endpoint: String::new(),
            access_key_id: String::new(),
            secret_access_key: None,
            path_style: true,
        }
    }
}

impl StorageConfig {
    /// Whether this asks for object storage and has everything it needs.
    #[must_use]
    pub fn is_s3(&self) -> bool {
        self.provider == "s3"
            && !self.bucket.is_empty()
            && !self.endpoint.is_empty()
            && !self.access_key_id.is_empty()
            && self.secret_access_key.is_some()
    }
}

/// CDN purge settings; `None` leaves the edge alone.
///
/// Only meaningful alongside a positive `edge_cache_seconds` site option:
/// caching HTML at an edge without a way to purge it means a published
/// correction sits behind a stale copy until the TTL runs out.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct CdnConfig {
    /// Provider name; `cloudflare` is the only one implemented.
    pub provider: String,
    /// Cloudflare zone id.
    pub zone_id: String,
    /// API token with the *Cache Purge* permission and nothing else.
    pub api_token: Option<Secret>,
}

impl Default for CdnConfig {
    fn default() -> Self {
        Self {
            provider: String::from("cloudflare"),
            zone_id: String::new(),
            api_token: None,
        }
    }
}

impl CdnConfig {
    /// Whether this is complete enough to purge with.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !self.zone_id.is_empty() && self.api_token.is_some()
    }
}

/// SMTP relay settings; `None` disables outbound email.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SmtpConfig {
    /// Relay hostname.
    pub host: String,
    /// Relay port (e.g. 587).
    pub port: u16,
    /// Username (if the relay requires auth).
    pub username: Option<String>,
    /// Password (if the relay requires auth).
    pub password: Option<Secret>,
    /// Envelope-from address.
    pub from: String,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 587,
            username: None,
            password: None,
            from: "vyasa@localhost".to_string(),
        }
    }
}

/// Database pool settings.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DbConfig {
    /// Maximum pool connections (1..=1000).
    pub max_connections: u32,
    /// Seconds to wait when acquiring a connection (includes establishing
    /// new connections) before failing.
    pub acquire_timeout_secs: u64,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            acquire_timeout_secs: 5,
        }
    }
}

/// The complete, validated application configuration.
///
/// `Debug` output is safe to log: every secret field redacts itself.
#[derive(Clone, Debug, Deserialize)]
pub struct VyasaConfig {
    /// PostgreSQL connection string (required).
    pub database_url: Secret,
    /// HTTP listen address, default `0.0.0.0:3000`.
    pub bind_addr: String,
    /// Directory for uploaded media files, default `./media`.
    pub media_dir: PathBuf,
    /// Directory for the search index, default `./index`.
    pub index_dir: PathBuf,
    /// Directory a self-hosted marketplace is served from, default
    /// `./registry`. Nothing is served unless the files exist, so an
    /// install that hosts no marketplace gains no public surface.
    pub registry_dir: PathBuf,
    /// Whether the playground and other dev-only endpoints are enabled.
    #[serde(default)]
    pub debug: bool,
    /// Trusted plugin signing keys (hex ed25519 public keys).
    #[serde(default)]
    pub plugin_trusted_keys: Vec<String>,
    /// Database pool settings.
    pub db: DbConfig,
    /// Logging settings.
    pub log: LogConfig,
    /// Background job settings.
    pub jobs: JobsConfig,
    /// AI provider settings.
    pub ai: AiConfig,
    /// SMTP settings; `None` disables outbound email.
    pub smtp: Option<SmtpConfig>,
    /// CDN purge settings, when an edge cache is in front of the site.
    pub cdn: Option<CdnConfig>,
    /// Where media bytes live; local disk unless told otherwise.
    pub storage: Option<StorageConfig>,
    /// Long-lived server secret (`VYASA_SECRET_KEY`). Signs preview and
    /// private-post tokens so they survive a restart, and encrypts stored
    /// AI provider keys at rest. Without it tokens are signed with a
    /// per-boot random key and provider keys are stored plain.
    pub secret_key: Option<Secret>,
    /// Reverse proxies whose forwarding headers are believed
    /// (`VYASA_TRUSTED_PROXIES`, comma-separated addresses or CIDR blocks,
    /// e.g. `127.0.0.1,::1`). Empty by default: the socket peer is the
    /// client, and `X-Forwarded-For` from anyone is ignored. From a listed
    /// peer the rightmost `X-Forwarded-For` entry that is not itself a
    /// listed proxy is the client.
    #[serde(default)]
    pub trusted_proxies: Vec<ProxyNet>,
    /// Believe `CF-Connecting-IP` when it arrives from a trusted proxy
    /// (`VYASA_TRUST_CF_CONNECTING_IP=true`). Only for a Cloudflare tunnel
    /// or edge in front of the site: Cloudflare overwrites the header, but
    /// anything else passes a client's own value straight through. A
    /// cloudflared tunnel to this host wants
    /// `VYASA_TRUSTED_PROXIES=127.0.0.1,::1` plus this.
    #[serde(default)]
    pub trust_cf_connecting_ip: bool,
    /// Public sandbox mode; off unless configured.
    pub demo: DemoConfig,
}

/// One trusted proxy: an address (`127.0.0.1`) or a CIDR block
/// (`10.0.0.0/8`, `fd00::/8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxyNet {
    addr: IpAddr,
    prefix: u8,
}

impl ProxyNet {
    /// Whether `ip` falls inside this block. IPv4-mapped IPv6 addresses
    /// (`::ffff:127.0.0.1`) are compared as the IPv4 address they carry.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr.to_canonical(), ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl FromStr for ProxyNet {
    type Err = crate::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || crate::AppError::validation(format!("invalid trusted proxy: {s:?}"));
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p.parse::<u8>().map_err(|_| bad())?)),
            None => (s, None),
        };
        let addr = IpAddr::from_str(addr).map_err(|_| bad())?.to_canonical();
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return Err(bad());
        }
        Ok(Self { addr, prefix })
    }
}

impl<'de> Deserialize<'de> for ProxyNet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Intermediate, everything-optional representation used while merging
/// configuration sources.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    database_url: Option<String>,
    bind_addr: Option<String>,
    media_dir: Option<PathBuf>,
    index_dir: Option<PathBuf>,
    registry_dir: Option<PathBuf>,
    debug: Option<bool>,
    plugin_trusted_keys: Option<Vec<String>>,
    db: Option<DbConfig>,
    log: Option<LogConfig>,
    jobs: Option<JobsConfig>,
    ai: Option<AiConfig>,
    smtp: Option<SmtpConfig>,
    cdn: Option<CdnConfig>,
    storage: Option<StorageConfig>,
    secret_key: Option<String>,
    trusted_proxies: Option<Vec<String>>,
    trust_cf_connecting_ip: Option<bool>,
    demo: Option<DemoConfig>,
}

impl TryFrom<RawConfig> for VyasaConfig {
    type Error = crate::AppError;

    fn try_from(raw: RawConfig) -> Result<Self, Self::Error> {
        let database_url = raw.database_url.ok_or_else(|| {
            crate::AppError::validation(
                "missing required configuration: database_url \
                 (set VYASA_DATABASE_URL or database_url in vyasa.toml)",
            )
        })?;
        let bind_addr = raw.bind_addr.unwrap_or_else(|| "0.0.0.0:3000".to_string());
        SocketAddr::from_str(&bind_addr)
            .map_err(|_| crate::AppError::validation(format!("invalid bind_addr: {bind_addr}")))?;
        let jobs = raw.jobs.unwrap_or_default();
        if !(1..=64).contains(&jobs.workers) {
            return Err(crate::AppError::validation(format!(
                "jobs.workers must be between 1 and 64, got {}",
                jobs.workers
            )));
        }
        let db = raw.db.unwrap_or_default();
        if !(1..=1000).contains(&db.max_connections) {
            return Err(crate::AppError::validation(format!(
                "db.max_connections must be between 1 and 1000, got {}",
                db.max_connections
            )));
        }
        if db.acquire_timeout_secs == 0 {
            return Err(crate::AppError::validation(
                "db.acquire_timeout_secs must be at least 1",
            ));
        }
        let trusted_proxies = raw
            .trusted_proxies
            .unwrap_or_default()
            .iter()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.parse())
            .collect::<Result<Vec<ProxyNet>, _>>()?;
        Ok(Self {
            database_url: Secret::new(database_url),
            bind_addr,
            media_dir: raw.media_dir.unwrap_or_else(|| PathBuf::from("media")),
            index_dir: raw.index_dir.unwrap_or_else(|| PathBuf::from("index")),
            registry_dir: raw
                .registry_dir
                .unwrap_or_else(|| PathBuf::from("registry")),
            debug: raw.debug.unwrap_or(false),
            plugin_trusted_keys: raw.plugin_trusted_keys.unwrap_or_default(),
            db,
            log: raw.log.unwrap_or_default(),
            jobs,
            ai: raw.ai.unwrap_or_default(),
            smtp: raw.smtp,
            cdn: raw.cdn,
            storage: raw.storage,
            secret_key: raw
                .secret_key
                .filter(|s| !s.trim().is_empty())
                .map(Secret::new),
            trusted_proxies,
            trust_cf_connecting_ip: raw.trust_cf_connecting_ip.unwrap_or(false),
            demo: raw.demo.unwrap_or_default(),
        })
    }
}

impl VyasaConfig {
    /// Loads configuration from an optional `vyasa.toml` in the
    /// current directory plus `VYASA_*` environment overrides.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AppError::Validation`] when required fields are
    /// missing or values are invalid.
    pub fn load() -> Result<Self, crate::AppError> {
        Self::load_from_file("vyasa.toml")
    }

    /// Loads configuration from the file at `path` (if it exists) plus
    /// `VYASA_*` environment overrides.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AppError::Validation`] when required fields are
    /// missing or values are invalid.
    pub fn load_from_file(path: &str) -> Result<Self, crate::AppError> {
        let merged = config::Config::builder()
            .add_source(config::File::with_name(path).required(false))
            .add_source(
                config::Environment::with_prefix("VYASA")
                    // Required: without an explicit prefix separator it
                    // would default to the `separator` value ("__") and
                    // stop recognizing `VYASA_*` variables.
                    .prefix_separator("_")
                    .separator("__")
                    .try_parsing(true)
                    // Without a list separator a `Vec<String>` field can
                    // never be built from the environment, so
                    // `VYASA_PLUGIN_TRUSTED_KEYS` was silently unusable
                    // and trusted keys could only come from a config file
                    // — one this project does not ship.
                    .list_separator(",")
                    .with_list_parse_key("plugin_trusted_keys")
                    .with_list_parse_key("trusted_proxies"),
            )
            .build()
            .map_err(|err| {
                crate::AppError::validation(format!("failed to load configuration: {err}"))
            })?;
        let raw: RawConfig = merged
            .try_deserialize()
            .map_err(|err| crate::AppError::validation(format!("invalid configuration: {err}")))?;
        raw.try_into()
    }
}

#[cfg(test)]
mod tests {
    use super::{LogFormat, VyasaConfig};
    use crate::AppError;

    fn temp_config_path() -> String {
        std::env::temp_dir()
            .join(format!("vyasa-config-test-{}.toml", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    /// All environment-mutating assertions live in this single test so
    /// parallel tests never race on the process environment.
    #[test]
    fn config_load_defaults_file_and_env_precedence() {
        let path = temp_config_path();
        // 1. Nothing set at all: missing database_url is a validation error.
        std::env::remove_var("VYASA_DATABASE_URL");
        std::env::remove_var("VYASA_BIND_ADDR");
        std::env::remove_var("VYASA_LOG__LEVEL");
        std::env::remove_var("VYASA_JOBS__WORKERS");
        std::env::remove_var("VYASA_DB__MAX_CONNECTIONS");
        let missing = VyasaConfig::load_from_file(&path);
        assert!(
            matches!(missing, Err(AppError::Validation { .. })),
            "{missing:?}"
        );
        let message = missing.err().map_or(String::new(), |e| e.to_string());
        assert!(message.contains("database_url"), "message: {message}");

        // 2. File provides values; defaults fill the rest.
        std::fs::write(
            &path,
            "database_url = \"postgres://user:secretpw@localhost/rp\"\n\
             bind_addr = \"127.0.0.1:9999\"\n",
        )
        .expect("write temp config");
        let cfg = VyasaConfig::load_from_file(&path).expect("load from file");
        assert_eq!(
            cfg.database_url.expose(),
            "postgres://user:secretpw@localhost/rp"
        );
        assert!(cfg.plugin_trusted_keys.is_empty(), "none configured");

        // 2b. A list-valued field from the environment. Without an
        // explicit list separator this field is unbuildable from env at
        // all, so trusted plugin keys could only come from a config file
        // — and installing a plugin needs at least one trusted key.
        std::env::set_var("VYASA_PLUGIN_TRUSTED_KEYS", "aa11,bb22");
        let keyed = VyasaConfig::load_from_file(&path).expect("load with keys");
        assert_eq!(keyed.plugin_trusted_keys, vec!["aa11", "bb22"]);
        std::env::remove_var("VYASA_PLUGIN_TRUSTED_KEYS");
        assert_eq!(cfg.bind_addr, "127.0.0.1:9999");
        assert_eq!(cfg.media_dir, std::path::PathBuf::from("media"));
        assert_eq!(cfg.index_dir, std::path::PathBuf::from("index"));
        assert_eq!(cfg.registry_dir, std::path::PathBuf::from("registry"));
        assert_eq!(cfg.log.level, "info");
        assert_eq!(cfg.log.format, LogFormat::Pretty);
        assert_eq!(cfg.jobs.workers, 2);
        assert_eq!(cfg.db.max_connections, 10);
        assert_eq!(cfg.db.acquire_timeout_secs, 5);
        assert!((cfg.ai.monthly_budget_usd - 25.0).abs() < 1e-9);
        assert!(cfg.smtp.is_none());
        assert!(!cfg.demo.enabled, "demo mode is off unless asked for");

        // 2c. Demo mode from the environment, with the default account.
        std::env::set_var("VYASA_DEMO__ENABLED", "true");
        let demo = VyasaConfig::load_from_file(&path).expect("load with demo");
        assert!(demo.demo.enabled);
        assert_eq!(demo.demo.email, "demo@vyasa.site");
        assert_eq!(demo.demo.password, "demo");
        std::env::remove_var("VYASA_DEMO__ENABLED");

        // 3. Environment overrides the file; `__` reaches nested fields.
        std::env::set_var("VYASA_BIND_ADDR", "0.0.0.0:1");
        std::env::set_var("VYASA_LOG__LEVEL", "debug");
        std::env::set_var("VYASA_JOBS__WORKERS", "4");
        std::env::set_var("VYASA_DB__MAX_CONNECTIONS", "32");
        let cfg = VyasaConfig::load_from_file(&path).expect("load with env overrides");
        assert_eq!(cfg.bind_addr, "0.0.0.0:1");
        assert_eq!(cfg.log.level, "debug");
        assert_eq!(cfg.jobs.workers, 4);
        assert_eq!(cfg.db.max_connections, 32);

        // 4. Debug output never leaks secrets.
        let debug = format!("{cfg:?}");
        assert!(debug.contains("Secret"), "debug: {debug}");
        assert!(!debug.contains("secretpw"), "debug: {debug}");

        // 5. Invalid values fail fast with a validation error.
        std::env::set_var("VYASA_BIND_ADDR", "not-an-addr");
        let invalid = VyasaConfig::load_from_file(&path);
        assert!(
            matches!(invalid, Err(AppError::Validation { .. })),
            "{invalid:?}"
        );
        std::env::set_var("VYASA_BIND_ADDR", "0.0.0.0:2");
        std::env::set_var("VYASA_DB__MAX_CONNECTIONS", "0");
        let invalid = VyasaConfig::load_from_file(&path);
        assert!(
            matches!(invalid, Err(AppError::Validation { .. })),
            "{invalid:?}"
        );

        // Cleanup.
        std::env::remove_var("VYASA_BIND_ADDR");
        std::env::remove_var("VYASA_LOG__LEVEL");
        std::env::remove_var("VYASA_JOBS__WORKERS");
        std::env::remove_var("VYASA_DB__MAX_CONNECTIONS");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn secret_display_is_redacted() {
        let secret = crate::config::Secret::new("hunter2");
        assert_eq!(format!("{secret}"), "***");
        assert_eq!(format!("{secret:?}"), "Secret { value: \"***\" }");
    }
    #[test]
    fn trusted_proxy_blocks_match_what_they_cover() {
        use super::ProxyNet;
        use std::net::IpAddr;
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let one: ProxyNet = "127.0.0.1".parse().unwrap();
        assert!(one.contains(ip("127.0.0.1")));
        assert!(one.contains(ip("::ffff:127.0.0.1")), "mapped v4 is v4");
        assert!(!one.contains(ip("127.0.0.2")));
        let block: ProxyNet = "10.0.0.0/8".parse().unwrap();
        assert!(block.contains(ip("10.200.3.4")));
        assert!(!block.contains(ip("11.0.0.1")));
        let v6: ProxyNet = "::1".parse().unwrap();
        assert!(v6.contains(ip("::1")));
        assert!(!v6.contains(ip("127.0.0.1")));
        let all: ProxyNet = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains(ip("203.0.113.9")));
        for bad in ["", "localhost", "10.0.0.0/33", "::1/129", "1.2.3.4/x"] {
            assert!(bad.parse::<ProxyNet>().is_err(), "{bad}");
        }
    }
}
