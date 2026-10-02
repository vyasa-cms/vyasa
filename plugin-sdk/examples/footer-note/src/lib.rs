//! A real, installable example: appends a note before `</body>` on every
//! page the site renders, and logs the posts it can see through the host.
//!
//! It exercises the parts of the contract that actually fire today — the
//! `page-html` filter and the `db:read:posts` host call — so installing it
//! is a genuine end-to-end check of the plugin runtime.

wit_bindgen::generate!({
    path: "../../../crates/plugins/wit",
    world: "vyasa-plugin",
});

struct FooterNote;

impl Guest for FooterNote {
    fn init(config_json: String) -> Result<PluginInfo, String> {
        let name = serde_json::from_str::<serde_json::Value>(&config_json)
            .ok()
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "footer-note".to_owned());
        vyasa::plugin::host::log(
            vyasa::plugin::host::LogLevel::Info,
            &format!("footer-note initialised as {name}"),
        );
        Ok(PluginInfo {
            name,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }

    fn filter(stage: FilterStage, content: String) -> String {
        // Only the page-html stage is dispatched today; leave anything
        // else untouched rather than guessing what it wraps.
        if !matches!(stage, FilterStage::PageHtml) {
            return content;
        }
        // Ask the host how much content the site has, proving the
        // capability-gated data path works from inside the sandbox.
        let count = vyasa::plugin::host::query_posts(None, 5)
            .map(|posts| posts.len())
            .unwrap_or(0);
        let note = format!(
            "<!-- footer-note plugin --><p class=\"vy-plugin-note\">\
             Served by a Vyasa plugin. It can see {count} recent posts.</p>"
        );
        match content.rfind("</body>") {
            Some(at) => {
                let mut out = String::with_capacity(content.len() + note.len());
                out.push_str(&content[..at]);
                out.push_str(&note);
                out.push_str(&content[at..]);
                out
            }
            None => content,
        }
    }

    fn action(_kind: EventKind, payload_json: String) -> Result<(), String> {
        vyasa::plugin::host::log(
            vyasa::plugin::host::LogLevel::Debug,
            &format!("event payload: {payload_json}"),
        );
        Ok(())
    }

    /// Declare custom blocks: JSON [{ kind, title, icon?, attrs-schema }].
    fn register_blocks() -> String {
        "[]".to_owned()
    }

    /// Declare extra public routes: JSON [{ method, path }].
    fn register_routes() -> String {
        "[]".to_owned()
    }
}

export!(FooterNote);
