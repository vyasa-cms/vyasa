//! Capability broker: resolve(plugin, capability) -> allow/deny, per-plugin
//! quotas, and audit logging. Deny is loud: audit row + telemetry counter.

use std::collections::HashMap;
use std::sync::Arc;

use vyasa_common::AppError;
use vyasa_db::repo::PluginsRepo;

use crate::capabilities::Capability;

/// Per-window quotas for one plugin.
#[derive(Debug, Clone, Copy)]
pub struct Quotas {
    /// db reads allowed per minute.
    pub db_reads_per_min: u32,
    /// outbound fetches allowed per minute.
    pub fetches_per_min: u32,
    /// total bytes fetched per minute.
    pub bytes_per_min: u64,
    /// mutations (kv writes, post writes) allowed per minute.
    pub writes_per_min: u32,
    /// text-model completions allowed per minute.
    pub ai_calls_per_min: u32,
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            db_reads_per_min: 600,
            fetches_per_min: 60,
            bytes_per_min: 10 * 1024 * 1024,
            // Deliberately far below the read budget. A plugin that needs
            // to write more than twice a second is doing a bulk import,
            // which is a job, not a hook.
            writes_per_min: 120,
            // Every completion is a paid provider call made on the site
            // owner's key. One every few seconds covers any honest use
            // from a hook, route or task; a plugin that wants more is
            // spending someone else's money in a loop.
            ai_calls_per_min: 20,
        }
    }
}

/// Sliding counters (fixed 60 s window keyed by wall-clock minute).
#[derive(Default)]
struct Counters {
    window: u64,
    db_reads: u32,
    fetches: u32,
    bytes: u64,
    writes: u32,
    ai_calls: u32,
}

impl Counters {
    fn roll(&mut self) -> &mut Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 60);
        if self.window != now {
            self.window = now;
            self.db_reads = 0;
            self.fetches = 0;
            self.bytes = 0;
            self.writes = 0;
            self.ai_calls = 0;
        }
        self
    }
}

/// Broker outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Capability granted and within quota.
    Allowed,
    /// Missing grant or quota exhausted.
    Denied,
}

/// The capability broker.
#[derive(Clone)]
pub struct Broker {
    repo: PluginsRepo,
    audit_pool: sqlx::PgPool,
    grants: Arc<tokio::sync::RwLock<HashMap<i64, Vec<Capability>>>>,
    counters: Arc<tokio::sync::Mutex<HashMap<i64, Counters>>>,
    quotas: Quotas,
}

impl std::fmt::Debug for Broker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Broker")
    }
}

impl Broker {
    /// Builds a broker over the plugins repository and audit pool.
    #[must_use]
    pub fn new(repo: PluginsRepo, pool: sqlx::PgPool) -> Self {
        Self {
            repo,
            audit_pool: pool,
            grants: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            counters: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            quotas: Quotas::default(),
        }
    }

    async fn granted_for(&self, plugin_id: i64) -> Result<Vec<Capability>, AppError> {
        if let Some(hit) = self.grants.read().await.get(&plugin_id) {
            return Ok(hit.clone());
        }
        let row = self
            .repo
            .list()
            .await?
            .into_iter()
            .find(|p| p.id == plugin_id)
            .ok_or_else(|| AppError::not_found("plugin", plugin_id))?;
        let caps = Capability::parse_list(&row.capabilities).unwrap_or_default();
        self.grants.write().await.insert(plugin_id, caps.clone());
        Ok(caps)
    }

    async fn audit(&self, plugin_id: i64, kind: &str, capability: &Capability, detail: &str) {
        let written = sqlx::query(
            "INSERT INTO plugin_audit (id, plugin_id, kind, capability, detail)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(plugin_id)
        .bind(kind)
        .bind(capability.to_string())
        .bind(detail)
        .execute(&self.audit_pool)
        .await;
        // The audit trail is a security control, so a failure to write it
        // is worth a line of its own. Discarding this result hid the fact
        // that `kind = 'write'` violated the column's constraint and every
        // write audit was being thrown away.
        if let Err(e) = written {
            tracing::error!(plugin_id, kind, "could not write the plugin audit row: {e}");
        }
        // A refusal is something an operator should see; a permitted call
        // is not. Logging every allowed write at `warn` filled the log
        // with lines reporting that a plugin did exactly what it was
        // granted permission to do.
        if matches!(kind, "deny" | "quota") {
            tracing::warn!(plugin_id, kind, capability = %capability, "broker {kind}: {detail}");
        } else {
            tracing::debug!(plugin_id, kind, capability = %capability, "broker {kind}: {detail}");
        }
    }

    /// Records a refusal a caller made on its own.
    ///
    /// Most denials come from the broker, but some rules live at the call
    /// site — `send-mail` refuses an address the site does not know, and
    /// that attempt belongs in the same log an operator already reads.
    pub async fn audit_denial(&self, plugin_id: i64, cap: &Capability, detail: &str) {
        self.audit(plugin_id, "deny", cap, detail).await;
    }

    /// Resolves one capability with no quota accounting.
    ///
    /// The named `check_*` helpers exist because most call sites also
    /// consume a quota; this is the plain grant question, used where the
    /// quota belongs to a different counter.
    pub async fn check_capability(&self, plugin_id: i64, cap: &Capability) -> Decision {
        let granted = match self.granted_for(plugin_id).await {
            Ok(g) => g,
            Err(e) => {
                self.audit(plugin_id, "deny", cap, &e.to_string()).await;
                return Decision::Denied;
            }
        };
        if granted.iter().any(|g| g.implies(cap)) {
            Decision::Allowed
        } else {
            self.audit(plugin_id, "deny", cap, "not granted").await;
            Decision::Denied
        }
    }

    /// Gate a mutation whose effect is confined to the plugin's own
    /// private storage: the grant plus one slot from the write quota, and
    /// no audit row.
    ///
    /// A plugin writing its own key/value namespace is not doing anything
    /// an operator would reconstruct later, and at the quota ceiling it
    /// would have written 172 800 audit rows a day — burying the entries
    /// that do matter.
    pub async fn check_private_write(&self, plugin_id: i64, cap: &Capability) -> Decision {
        self.gate_write(plugin_id, cap, None).await
    }

    /// Gate a mutation with an effect outside the plugin: the grant, one
    /// slot from the write quota, and an audit row.
    ///
    /// A denied read is a packaging mistake; a *successful* write that
    /// touches content, post meta or someone's inbox is the thing an
    /// operator needs to be able to reconstruct afterwards.
    pub async fn check_write(&self, plugin_id: i64, cap: &Capability, detail: &str) -> Decision {
        self.gate_write(plugin_id, cap, Some(detail)).await
    }

    async fn gate_write(&self, plugin_id: i64, cap: &Capability, detail: Option<&str>) -> Decision {
        if self.check_capability(plugin_id, cap).await != Decision::Allowed {
            return Decision::Denied;
        }
        let mut counters = self.counters.lock().await;
        let c = counters.entry(plugin_id).or_default().roll();
        if c.writes >= self.quotas.writes_per_min {
            drop(counters);
            self.audit(plugin_id, "quota", cap, "writes/min exceeded")
                .await;
            return Decision::Denied;
        }
        c.writes += 1;
        drop(counters);
        if let Some(detail) = detail {
            self.audit(plugin_id, "write", cap, detail).await;
        }
        Decision::Allowed
    }

    /// Gate a database read.
    pub async fn check_db_read(&self, plugin_id: i64, cap: Capability) -> Decision {
        let granted = match self.granted_for(plugin_id).await {
            Ok(g) => g,
            Err(e) => {
                self.audit(plugin_id, "deny", &cap, &e.to_string()).await;
                return Decision::Denied;
            }
        };
        if !granted.iter().any(|g| g.implies(&cap)) {
            self.audit(plugin_id, "deny", &cap, "not granted").await;
            return Decision::Denied;
        }
        let mut counters = self.counters.lock().await;
        let c = counters.entry(plugin_id).or_default().roll();
        if c.db_reads >= self.quotas.db_reads_per_min {
            drop(counters);
            self.audit(plugin_id, "quota", &cap, "db reads/min exceeded")
                .await;
            return Decision::Denied;
        }
        c.db_reads += 1;
        Decision::Allowed
    }

    /// Gate an event emission.
    pub async fn check_event_emit(&self, plugin_id: i64) -> Decision {
        let granted = match self.granted_for(plugin_id).await {
            Ok(g) => g,
            Err(e) => {
                self.audit(plugin_id, "deny", &Capability::EventEmit, &e.to_string())
                    .await;
                return Decision::Denied;
            }
        };
        if granted.iter().any(|g| g.implies(&Capability::EventEmit)) {
            Decision::Allowed
        } else {
            self.audit(plugin_id, "deny", &Capability::EventEmit, "not granted")
                .await;
            Decision::Denied
        }
    }

    /// Drops the cached capability set for a plugin.
    ///
    /// Grants were cached for the life of the process, so reinstalling a
    /// plugin with a different capability list kept enforcing the old one
    /// until the next restart.
    pub async fn evict(&self, plugin_id: i64) {
        self.grants.write().await.remove(&plugin_id);
    }

    /// Whether the plugin's capability set is cached.
    pub async fn has_cached_grants(&self, plugin_id: i64) -> bool {
        self.grants.read().await.contains_key(&plugin_id)
    }

    /// Checks the log capability.
    pub async fn check_log(&self, plugin_id: i64) -> Decision {
        let Ok(granted) = self.granted_for(plugin_id).await else {
            return Decision::Denied;
        };
        if granted.iter().any(|g| g.implies(&Capability::LogWrite)) {
            Decision::Allowed
        } else {
            // Not audited: a plugin logging without the capability is a
            // packaging mistake, and one audit row per log line would
            // drown the table it is meant to make readable.
            Decision::Denied
        }
    }

    /// Whether `plugin_id` holds `cap`, with no audit row and no quota.
    ///
    /// For gates evaluated on every page render (`html:page`), where a
    /// denial is the ordinary state of a plugin that never asked for the
    /// power and one audit row per render would bury the table.
    pub async fn holds(&self, plugin_id: i64, cap: &Capability) -> bool {
        self.granted_for(plugin_id)
            .await
            .is_ok_and(|granted| granted.iter().any(|g| g.implies(cap)))
    }

    /// Gate a text-model completion: the `ai:text` grant plus one slot
    /// from the per-minute AI quota.
    pub async fn check_ai(&self, plugin_id: i64) -> Decision {
        let granted = match self.granted_for(plugin_id).await {
            Ok(g) => g,
            Err(e) => {
                self.audit(plugin_id, "deny", &Capability::AiText, &e.to_string())
                    .await;
                return Decision::Denied;
            }
        };
        if !granted.iter().any(|g| g.implies(&Capability::AiText)) {
            self.audit(plugin_id, "deny", &Capability::AiText, "not granted")
                .await;
            return Decision::Denied;
        }
        let mut counters = self.counters.lock().await;
        let c = counters.entry(plugin_id).or_default().roll();
        if c.ai_calls >= self.quotas.ai_calls_per_min {
            drop(counters);
            self.audit(
                plugin_id,
                "quota",
                &Capability::AiText,
                "ai calls/min exceeded",
            )
            .await;
            return Decision::Denied;
        }
        c.ai_calls += 1;
        Decision::Allowed
    }

    /// Gate an outbound fetch to `url`, consuming one slot from the fetch +
    /// byte quotas when the response size is known upfront (unknown sizes
    /// consume after download via [`Broker::record_bytes`]).
    pub async fn check_fetch(&self, plugin_id: i64, url: &str) -> Decision {
        let host = url_host(url);
        let Some(host) = host else {
            self.audit(
                plugin_id,
                "deny",
                &Capability::NetFetch(String::new()),
                "unparseable url",
            )
            .await;
            return Decision::Denied;
        };
        let wanted = Capability::NetFetch(host.clone());
        let granted = match self.granted_for(plugin_id).await {
            Ok(g) => g,
            Err(e) => {
                self.audit(plugin_id, "deny", &wanted, &e.to_string()).await;
                return Decision::Denied;
            }
        };
        if !granted.iter().any(|g| g.implies(&wanted)) {
            self.audit(plugin_id, "deny", &wanted, "pattern not granted")
                .await;
            return Decision::Denied;
        }
        let mut counters = self.counters.lock().await;
        let c = counters.entry(plugin_id).or_default().roll();
        if c.fetches >= self.quotas.fetches_per_min {
            drop(counters);
            self.audit(plugin_id, "quota", &wanted, "fetches/min exceeded")
                .await;
            return Decision::Denied;
        }
        c.fetches += 1;
        drop(counters);
        self.audit(plugin_id, "fetch", &wanted, url).await;
        Decision::Allowed
    }

    /// Records fetched bytes against the byte quota; over-budget callers
    /// should abort the transfer.
    pub async fn record_bytes(&self, plugin_id: i64, n: u64) -> bool {
        let mut counters = self.counters.lock().await;
        let c = counters.entry(plugin_id).or_default().roll();
        if c.bytes + n > self.quotas.bytes_per_min {
            return false;
        }
        c.bytes += n;
        true
    }
}

/// Parses a URL a plugin asked to fetch, or refuses it.
///
/// This is the single parser for plugin egress: the broker checks the
/// host this returns, and the fetch backend must request the very `Url`
/// this returns, so the two can never disagree about where a request
/// goes. Refused: anything but `https`, any userinfo
/// (`https://good.example@evil.example` names `evil.example`), and IP
/// literals (a `net:fetch` grant names a domain, never an address).
#[must_use]
pub fn parse_fetch_url(raw: &str) -> Option<url::Url> {
    let parsed = url::Url::parse(raw).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    match parsed.host()? {
        url::Host::Domain(d) if !d.is_empty() => Some(parsed),
        _ => None,
    }
}

fn url_host(url: &str) -> Option<String> {
    let parsed = parse_fetch_url(url)?;
    parsed.host_str().map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod url_host_tests {
    use super::url_host;
    use crate::capabilities::host_matches;

    #[test]
    fn userinfo_cannot_smuggle_a_granted_host() {
        // Before: the part before '@' was taken as the host, so this
        // passed a `net:fetch:api.good.com` grant while going to evil.com.
        assert_eq!(url_host("https://api.good.com@evil.com/x"), None);
        assert_eq!(url_host("https://api.good.com:pw@evil.com/x"), None);
    }

    #[test]
    fn authority_delimiters_do_not_fool_wildcards() {
        // `\`, `?` and `#` end the authority; the host is what precedes them.
        let host = url_host("https://evil.com\\.a.example.com/").unwrap_or_default();
        assert!(!host_matches("*.example.com", &host), "{host}");
        let host = url_host("https://evil.com?.a.example.com").unwrap_or_default();
        assert!(!host_matches("*.example.com", &host), "{host}");
        let host = url_host("https://evil.com#.a.example.com").unwrap_or_default();
        assert!(!host_matches("*.example.com", &host), "{host}");
        assert_eq!(
            url_host("https://A.Example.com:8443/p?q#f").as_deref(),
            Some("a.example.com")
        );
    }

    #[test]
    fn non_https_and_ip_literals_are_refused() {
        for bad in [
            "http://a.example.com/",
            "ftp://a.example.com/",
            "https://127.0.0.1/",
            "https://2130706433/",
            "https://[::1]/",
            "not a url",
        ] {
            assert_eq!(url_host(bad), None, "{bad}");
        }
    }
}
