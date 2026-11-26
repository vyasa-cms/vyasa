//! Operator overrides of plugin quotas (`plugin_quotas`, env
//! `VYASA_PLUGIN_QUOTAS`).
//!
//! The capability broker gives every plugin the same per-minute budgets.
//! An operator who trusts a plugin with more (a documentation importer
//! syncing several versions, say) raises its limits here, by plugin name:
//!
//! ```text
//! vyasa-docs:reads=3000,task_reads=20000;*:reads=1200
//! ```
//!
//! Entries are separated by `;`. Each names a plugin (`*` for every
//! plugin) and a comma-separated list of `limit=value`. A limit prefixed
//! with `task_` applies to extended tasks (a scheduled sync), the bare
//! limit to every other call. A named plugin's entries apply on top of
//! `*`.

use serde::{Deserialize, Serialize};

/// Which per-minute budget an override sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaLimit {
    /// Database and key-value reads.
    Reads,
    /// Outbound fetches.
    Fetches,
    /// Bytes fetched.
    Bytes,
    /// Mutations (key-value and content writes).
    Writes,
    /// Text-model completions.
    AiCalls,
}

impl QuotaLimit {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "reads" => Self::Reads,
            "fetches" => Self::Fetches,
            "bytes" => Self::Bytes,
            "writes" => Self::Writes,
            "ai_calls" => Self::AiCalls,
            _ => return None,
        })
    }
}

/// One operator override: `plugin`'s `limit` is `value` per minute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginQuotaOverride {
    /// Plugin name, or `*` for every plugin.
    pub plugin: String,
    /// The budget it sets.
    pub limit: QuotaLimit,
    /// Whether it sets the extended-task budget rather than the ordinary one.
    pub task: bool,
    /// Allowed per minute (bytes per minute for [`QuotaLimit::Bytes`]).
    pub value: u64,
}

/// Parses a `plugin_quotas` value. Empty means no overrides.
///
/// # Errors
/// A message naming the malformed entry: a missing plugin name, an unknown
/// limit, or a value that is not a whole number.
pub fn parse_plugin_quotas(spec: &str) -> Result<Vec<PluginQuotaOverride>, String> {
    let mut out = Vec::new();
    for entry in spec.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let (plugin, limits) = entry
            .split_once(':')
            .ok_or_else(|| format!("plugin_quotas entry `{entry}` needs `plugin:limit=value`"))?;
        let plugin = plugin.trim();
        if plugin.is_empty() {
            return Err(format!("plugin_quotas entry `{entry}` names no plugin"));
        }
        for item in limits.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            let (name, value) = item
                .split_once('=')
                .ok_or_else(|| format!("plugin_quotas `{item}` needs `limit=value`"))?;
            let name = name.trim();
            let (task, base) = name
                .strip_prefix("task_")
                .map_or((false, name), |rest| (true, rest));
            let limit = QuotaLimit::parse(base).ok_or_else(|| {
                format!(
                    "plugin_quotas limit `{name}` is unknown (reads, fetches, bytes, writes, \
                     ai_calls, each optionally prefixed with task_)"
                )
            })?;
            let value = value
                .trim()
                .parse::<u64>()
                .map_err(|_| format!("plugin_quotas `{item}`: the value must be a whole number"))?;
            out.push(PluginQuotaOverride {
                plugin: plugin.to_owned(),
                limit,
                task,
                value,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_name_a_plugin_a_limit_and_a_budget() {
        let parsed =
            parse_plugin_quotas(" vyasa-docs: reads=3000, task_reads=20000 ; *:writes=300 ;")
                .unwrap();
        assert_eq!(
            parsed,
            vec![
                PluginQuotaOverride {
                    plugin: "vyasa-docs".into(),
                    limit: QuotaLimit::Reads,
                    task: false,
                    value: 3000
                },
                PluginQuotaOverride {
                    plugin: "vyasa-docs".into(),
                    limit: QuotaLimit::Reads,
                    task: true,
                    value: 20000
                },
                PluginQuotaOverride {
                    plugin: "*".into(),
                    limit: QuotaLimit::Writes,
                    task: false,
                    value: 300
                },
            ]
        );
        assert!(parse_plugin_quotas("").unwrap().is_empty());
    }

    #[test]
    fn a_malformed_entry_is_refused_with_its_text() {
        for (spec, needle) in [
            ("reads=10", "reads=10"),
            (":reads=10", "names no plugin"),
            ("p:speed=10", "speed"),
            ("p:reads=lots", "whole number"),
            ("p:reads", "limit=value"),
        ] {
            let e = parse_plugin_quotas(spec).unwrap_err();
            assert!(e.contains(needle), "{spec}: {e}");
        }
    }
}
