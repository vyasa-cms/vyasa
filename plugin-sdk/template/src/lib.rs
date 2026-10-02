//! Vyasa plugin template: implements the `vyasa-plugin-v2` world with
//! pass-through hooks and empty declarations. Copy this crate and fill in
//! the parts you need — every export below is safe to leave as it is.
//!
//! `vyasa-plugin-v2` is a superset of the base `vyasa-plugin` world. If
//! you only want filters and actions you can target `vyasa-plugin`
//! instead and drop the second half of this file; plugins built against
//! either world install and run the same way.

wit_bindgen::generate!({
    path: "../../crates/plugins/wit",
    world: "vyasa-plugin-v2",
});

use vyasa::plugin::host;

struct HelloPlugin;

impl Guest for HelloPlugin {
    fn init(config_json: String) -> Result<PluginInfo, String> {
        // `config_json` is this plugin's stored settings — the object the
        // operator filled in on the form `register_admin` declares.
        let name = serde_json::from_str::<serde_json::Value>(&config_json)
            .ok()
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "hello".to_owned());
        host::log(
            host::LogLevel::Info,
            &format!("hello plugin initialised as {name}"),
        );
        Ok(PluginInfo {
            name,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }

    fn filter(_stage: FilterStage, content: String) -> String {
        // Pass everything through untouched.
        content
    }

    fn action(_kind: EventKind, payload_json: String) -> Result<(), String> {
        host::log(
            host::LogLevel::Debug,
            &format!("event payload: {payload_json}"),
        );
        Ok(())
    }

    /// Declare custom blocks: JSON [{ kind, title, icon? }].
    /// `kind` must be `namespace/name`. Instances are rendered by
    /// `render_block` below.
    fn register_blocks() -> String {
        "[]".to_owned()
    }

    /// Declare extra public routes: JSON [{ method, path }].
    /// They are mounted at `/api/v1/plugin/<plugin name>/<path>` and served
    /// by `handle_request` below. A path may end in `*` to match a tree.
    fn register_routes() -> String {
        "[]".to_owned()
    }

    /// Serve one request to a declared route.
    fn handle_request(_req: host::HttpRequest) -> Result<host::HttpResponse, String> {
        Ok(host::HttpResponse {
            status: 404,
            headers: vec![],
            body: "not found".to_owned(),
        })
    }

    /// Render one instance of a block declared above. The returned HTML is
    /// sanitized by the host, but escaping values that came from the
    /// document is still this plugin's job.
    fn render_block(kind: String, _attrs_json: String) -> Result<String, String> {
        Err(format!("unknown block {kind}"))
    }

    /// Run one task declared in `register_schedule`.
    fn run_task(name: String) -> Result<(), String> {
        Err(format!("unknown task {name}"))
    }

    /// Declare periodic work: JSON [{ name, every-seconds }].
    /// Intervals below 60 seconds are raised to 60.
    fn register_schedule() -> String {
        "[]".to_owned()
    }

    /// Declare an admin settings form. What an operator saves here is what
    /// `init` receives:
    /// { title?, description?, fields: [{ key, label, type, help?,
    ///   options?, default? }] } with type one of
    /// text | textarea | number | boolean | select.
    fn register_admin() -> String {
        "{}".to_owned()
    }

    /// Declare front-end assets: { css?, js? }. Served from
    /// `/plugin-assets/<hash>.css|js` and linked from every page.
    fn register_assets() -> String {
        "{}".to_owned()
    }

    /// Declare custom post types:
    /// JSON [{ slug, singular, plural, public?, has-archive? }].
    /// Entries live at `/<slug>/<entry-slug>` with an archive at `/<slug>`.
    fn register_post_types() -> String {
        "[]".to_owned()
    }

    /// Declare custom taxonomies:
    /// JSON [{ slug, singular, plural, hierarchical?, public? }].
    /// Terms archive at `/<slug>/<term>`.
    fn register_taxonomies() -> String {
        "[]".to_owned()
    }

    /// The named filter points: `seo-title`, `seo-description`,
    /// `post-title`, `excerpt`, `search-query`.
    ///
    /// A point you do not handle must return `payload` unchanged.
    fn filter_at(_point: String, payload: String) -> String {
        payload
    }

    /// The named events — `post-saved`, `user-created`, `login-failed`
    /// and the rest; see `docs/plugin-api.md` for the list and payloads.
    fn on_event(_name: String, _payload_json: String) -> Result<(), String> {
        Ok(())
    }
}

export!(HelloPlugin);
