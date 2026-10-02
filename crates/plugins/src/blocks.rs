//! Plugin-declared custom block kinds.
//!
//! A plugin declares kinds from `register-blocks` and renders instances of
//! them through the superset world's `render-block` export. Both halves
//! matter: before, declarations were collected and then consulted by
//! nothing, and the core block enum could not represent an instance
//! anyway, so a declared kind was unreachable from the editor and
//! unstorable in a document.

use std::collections::BTreeMap;
use std::sync::Arc;

/// One declared block kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDecl {
    /// `namespace/name`, as it appears in a document's `kind`.
    pub kind: String,
    /// Label for the editor's inserter.
    pub title: String,
    /// Optional emoji or short glyph for the inserter.
    pub icon: Option<String>,
    /// Plugin that owns (and renders) it.
    pub plugin_id: i64,
    /// Plugin name, for the editor and for error messages.
    pub plugin_name: String,
}

/// Registry of plugin-contributed block kinds.
#[derive(Default, Clone)]
pub struct PluginBlockRegistry {
    /// kind → declaration.
    kinds: Arc<tokio::sync::RwLock<BTreeMap<String, BlockDecl>>>,
}

impl PluginBlockRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the declarations in a `register-blocks()` payload.
    ///
    /// Malformed entries are skipped rather than failing the boot: one bad
    /// declaration should cost that block, not the plugin and not the
    /// site. A kind another plugin already claimed is skipped too — first
    /// declaration wins, so installing a second plugin cannot silently
    /// take over the rendering of blocks already in published documents.
    pub async fn register_from_json(
        &self,
        plugin_id: i64,
        plugin_name: &str,
        json: &str,
    ) -> Vec<String> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            return Vec::new();
        };
        let Some(items) = value.as_array() else {
            return Vec::new();
        };
        let mut map = self.kinds.write().await;
        let mut accepted = Vec::new();
        for item in items {
            let Some(kind) = item.get("kind").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if !vyasa_core::block::validate::is_plugin_kind_name(kind) {
                tracing::warn!(
                    plugin = plugin_name,
                    "ignoring block kind {kind:?}: not `namespace/name`"
                );
                continue;
            }
            if let Some(existing) = map.get(kind) {
                if existing.plugin_id != plugin_id {
                    tracing::warn!(
                        plugin = plugin_name,
                        "block kind {kind:?} already declared by {}",
                        existing.plugin_name
                    );
                    continue;
                }
            }
            let title = item
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(kind);
            map.insert(
                kind.to_owned(),
                BlockDecl {
                    kind: kind.to_owned(),
                    title: title.to_owned(),
                    icon: item
                        .get("icon")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    plugin_id,
                    plugin_name: plugin_name.to_owned(),
                },
            );
            accepted.push(kind.to_owned());
        }
        accepted
    }

    /// The declaration for `kind`, when one exists.
    pub async fn get(&self, kind: &str) -> Option<BlockDecl> {
        self.kinds.read().await.get(kind).cloned()
    }

    /// The plugin owning `kind`, when declared.
    pub async fn owner(&self, kind: &str) -> Option<i64> {
        self.kinds.read().await.get(kind).map(|d| d.plugin_id)
    }

    /// Every declaration, sorted by kind.
    pub async fn all(&self) -> Vec<BlockDecl> {
        self.kinds.read().await.values().cloned().collect()
    }

    /// Drops every kind a plugin declared (on disable, upgrade or delete).
    pub async fn forget(&self, plugin_id: i64) {
        self.kinds
            .write()
            .await
            .retain(|_, d| d.plugin_id != plugin_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn declarations_are_namespaced_and_first_wins() {
        let reg = PluginBlockRegistry::new();
        let accepted = reg
            .register_from_json(
                1,
                "charts",
                r#"[{"kind": "charts/bar", "title": "Bar chart", "icon": "B"},
                    {"kind": "nope", "title": "Unnamespaced"},
                    {"title": "No kind at all"}]"#,
            )
            .await;
        assert_eq!(accepted, vec![String::from("charts/bar")]);
        assert_eq!(reg.get("charts/bar").await.unwrap().title, "Bar chart");
        assert!(reg.get("nope").await.is_none());

        // A second plugin cannot take over a kind already in documents.
        let stolen = reg
            .register_from_json(
                2,
                "evil",
                r#"[{"kind": "charts/bar", "title": "Mine now"}]"#,
            )
            .await;
        assert!(stolen.is_empty());
        assert_eq!(reg.owner("charts/bar").await, Some(1));

        reg.forget(1).await;
        assert!(reg.all().await.is_empty());
    }
}
