//! The WebAssembly entry points: the thin layer between the host's
//! `vyasa-plugin-v2` world and the pure modules of this crate.

wit_bindgen::generate!({
    path: "../../crates/plugins/wit",
    world: "vyasa-plugin-v2",
});

use sha2::{Digest, Sha256};
use vyasa::plugin::host;

use crate::settings::Settings;
use crate::sync::{self, Budget, Entry, Env, Managed, Put, Reply};
use crate::{render, routes, sections};

const SETTINGS_KEY: &str = "cfg/settings";
const SIG_KEY: &str = "cfg/sig";
const PENDING_KEY: &str = "sync/pending";

struct Docs;

fn settings() -> Settings {
    host::kv_get(SETTINGS_KEY)
        .ok()
        .flatten()
        .map(|j| Settings::from_json(&j))
        .unwrap_or_default()
}

fn kv_text(key: &str) -> Option<String> {
    host::kv_get(key).ok().flatten()
}

/// The sync algorithm's view of the host.
struct HostEnv;

impl Env for HostEnv {
    fn get(&mut self, url: &str, headers: &[(&str, String)]) -> Result<Reply, String> {
        let r = host::fetch_with(&host::FetchRequest {
            url: url.to_owned(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        })?;
        let etag = r
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
            .map(|(_, v)| v.clone());
        Ok(Reply {
            status: r.status,
            etag,
            body: r.body,
        })
    }

    fn now_ms(&self) -> u64 {
        host::now_ms()
    }

    fn kv_get(&mut self, key: &str) -> Option<String> {
        kv_text(key)
    }

    fn kv_set(&mut self, key: &str, value: &str) -> Result<(), String> {
        host::kv_set(key, value)
    }

    fn kv_delete(&mut self, key: &str) {
        let _ = host::kv_delete(key);
    }

    fn kv_list(&mut self, prefix: &str) -> Vec<String> {
        host::kv_list(prefix, 1000).unwrap_or_default()
    }

    fn put_entry(&mut self, e: &Entry) -> Result<Put, String> {
        let r = host::put_entry(&host::EntryInput {
            post_type: sync::POST_TYPE.to_owned(),
            key: e.key.clone(),
            parent_key: e.parent_key.clone(),
            slug: e.slug.clone(),
            title: e.title.clone(),
            excerpt: e.excerpt.clone(),
            blocks_json: e.blocks_json.clone(),
            position: e.position,
            status: "published".to_owned(),
            terms: e.terms.clone(),
            fields_json: e.fields_json.clone(),
            seo_json: e.seo_json.clone(),
            source_hash: e.source_hash.clone(),
            edit_url: e.edit_url.clone(),
        })?;
        Ok(match r.outcome.as_str() {
            "created" => Put::Created,
            "updated" => Put::Updated,
            _ => Put::Unchanged,
        })
    }

    fn list_managed(&mut self) -> Result<Vec<Managed>, String> {
        Ok(host::list_managed(sync::POST_TYPE)?
            .into_iter()
            .map(|m| Managed {
                key: m.key,
                source_hash: m.source_hash,
            })
            .collect())
    }

    fn delete_entry(&mut self, key: &str) -> Result<(), String> {
        host::delete_entry(key)
    }
}

impl Guest for Docs {
    fn init(config_json: String) -> Result<PluginInfo, String> {
        // Each call is a fresh instance, so the settings live in `kv`.
        let s = Settings::from_json(&config_json);
        let json = s.to_json();
        host::kv_set(SETTINGS_KEY, &json)?;
        // New settings (another repository, branch or folder) mean the next
        // check must not be answered with "nothing changed".
        let sig: String = Sha256::digest(json.as_bytes())
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        if kv_text(SIG_KEY).as_deref() != Some(sig.as_str()) {
            host::kv_set(SIG_KEY, &sig)?;
            host::kv_set("sync/complete", "0")?;
        }
        Ok(PluginInfo {
            name: "vyasa-docs".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }

    fn filter(_stage: FilterStage, content: String) -> String {
        content
    }

    fn action(_kind: EventKind, _payload_json: String) -> Result<(), String> {
        Ok(())
    }

    fn register_blocks() -> String {
        serde_json::json!([
            {"kind": "docs/tabs", "title": "Tabs", "icon": "▭", "container": {"child": "group", "labels": "labels"},
             "attrs-schema": {"type": "object", "properties": {
                 "labels": {"type": "array", "items": {"type": "string"}, "title": "Tab labels"},
                 "group": {"type": "string", "title": "Sync group", "description": "Tab sets with the same name switch together."}}}},
            {"kind": "docs/code-group", "title": "Code group", "icon": "‹›",
             "attrs-schema": {"type": "object", "properties": {
                 "tabs": {"type": "array", "title": "Snippets", "items": {"type": "object", "properties": {
                     "label": {"type": "string"}, "language": {"type": "string"}, "code": {"type": "string", "format": "code"}}}}}}},
            {"kind": "docs/steps", "title": "Steps", "icon": "①", "container": {"child": "group"},
             "attrs-schema": {"type": "object", "properties": {}}},
            {"kind": "docs/params", "title": "Parameters table", "icon": "≣",
             "attrs-schema": {"type": "object", "properties": {
                 "rows": {"type": "array", "title": "Parameters", "items": {"type": "object", "properties": {
                     "name": {"type": "string"}, "type": {"type": "string"}, "required": {"type": "boolean"},
                     "default": {"type": "string"}, "description": {"type": "string", "format": "inline-html"}}}}}}},
            {"kind": "docs/file-tree", "title": "File tree", "icon": "🗀",
             "attrs-schema": {"type": "object", "properties": {
                 "tree": {"type": "string", "format": "code", "title": "Tree", "description": "Two spaces per level; a trailing / marks a folder; ` # note` adds a comment."}}}}
        ])
        .to_string()
    }

    fn register_routes() -> String {
        serde_json::json!([{"method": "POST", "path": routes::WEBHOOK_PATH, "headers": routes::WEBHOOK_HEADERS}]).to_string()
    }

    fn handle_request(req: host::HttpRequest) -> Result<host::HttpResponse, String> {
        let outcome = routes::handle(&req.method, &req.path, &req.headers, &req.body, &settings());
        if let Some(sha) = &outcome.pending {
            host::kv_set(PENDING_KEY, sha)?;
        }
        Ok(host::HttpResponse {
            status: outcome.status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: outcome.body,
        })
    }

    fn render_block(kind: String, attrs_json: String) -> Result<String, String> {
        let attrs: serde_json::Value =
            serde_json::from_str(&attrs_json).map_err(|e| format!("attrs: {e}"))?;
        render::render(&kind, &attrs)
    }

    fn run_task(name: String) -> Result<(), String> {
        if name != "sync" {
            return Ok(());
        }
        let s = settings();
        if !s.configured() {
            return Ok(());
        }
        let cfg = s.sync_config()?;
        let now = host::now_ms();
        let pending = kv_text(PENDING_KEY).filter(|p| !p.is_empty());
        let incomplete = kv_text("sync/complete").as_deref() != Some("1");
        let last: u64 = kv_text("sync/last-run-ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let due =
            pending.is_some() || incomplete || now.saturating_sub(last) >= s.poll_minutes * 60_000;
        if !due {
            return Ok(());
        }
        host::kv_set("sync/last-run-ms", &now.to_string())?;
        // Stop short of the host's own deadline: a trap would disable the plugin.
        let total = u64::from(host::call_budget_ms());
        let usable = total.saturating_sub((total / 5).max(400));
        let long = total > 10_000;
        let budget = Budget {
            deadline_ms: now + usable,
            max_requests: if long { 250 } else { 12 },
            max_writes: if long { 500 } else { 30 },
        };
        let report = sync::run(&mut HostEnv, &cfg, pending.as_deref(), budget)?;
        if report.finished && pending.is_some() {
            host::kv_delete(PENDING_KEY)?;
        }
        let note = format!(
            "docs sync {}: {} created, {} updated, {} unchanged, {} removed, {} requests{}",
            if report.finished {
                "finished"
            } else {
                "paused (will continue)"
            },
            report.created,
            report.updated,
            report.unchanged,
            report.deleted,
            report.requests,
            if report.up_to_date {
                " (up to date)"
            } else {
                ""
            }
        );
        host::log(host::LogLevel::Info, &note);
        for w in report.warnings.iter().take(10) {
            host::log(host::LogLevel::Warn, w);
        }
        Ok(())
    }

    fn register_schedule() -> String {
        serde_json::json!([{"name": "sync", "every-seconds": 60, "budget": "extended"}]).to_string()
    }

    fn register_admin() -> String {
        crate::settings::admin_form().to_string()
    }

    fn register_assets() -> String {
        serde_json::json!({"css": include_str!("../assets/docs.css"), "js": include_str!("../assets/docs.js")}).to_string()
    }

    fn register_post_types() -> String {
        serde_json::json!([{
            "slug": sync::POST_TYPE, "singular": "Doc", "plural": "Docs",
            "public": true, "has-archive": true, "hierarchical": true, "base": sync::BASE,
            "fields": [
                {"key": "section_label", "label": "Section label", "kind": "text", "help": "A heading shown above this page in the sidebar."},
                {"key": "hide_from_nav", "label": "Hide from navigation", "kind": "boolean", "help": "The page stays reachable and searchable."},
                {"key": "edit_url", "label": "Edit URL", "kind": "url", "help": "Where readers can edit this page (set by the importer for synced pages)."},
                {"key": "last_updated", "label": "Last updated", "kind": "date"},
                {"key": "version_root", "label": "Version entry", "kind": "boolean", "help": "Marks the entry an older version's pages hang under."}
            ]
        }])
        .to_string()
    }

    fn register_taxonomies() -> String {
        serde_json::json!([{
            "slug": sync::VERSION_TAXONOMY, "singular": "Docs version", "plural": "Docs versions",
            "hierarchical": false, "public": false
        }])
        .to_string()
    }

    fn filter_at(point: String, payload: String) -> String {
        match point.as_str() {
            "register-sections" => sections::descriptors().to_string(),
            "render-section" => sections::render(&payload, &settings()),
            "register-layouts" => sections::default_layouts().to_string(),
            _ => payload,
        }
    }

    fn on_event(_name: String, _payload_json: String) -> Result<(), String> {
        Ok(())
    }
}

export!(Docs);
