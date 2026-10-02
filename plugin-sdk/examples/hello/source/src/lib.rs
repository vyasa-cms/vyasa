//! Vyasa plugin template: implements the `vyasa:plugin` world
//! with pass-through hooks. Copy this crate and edit the marked sections.

wit_bindgen::generate!({
    path: "wit",
    world: "vyasa-plugin",
});

struct HelloPlugin;

impl Guest for HelloPlugin {
    fn init(config_json: String) -> Result<PluginInfo, String> {
        let name = serde_json::from_str::<serde_json::Value>(&config_json)
            .ok()
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "hello".to_owned());
        vyasa::plugin::host::log(
            vyasa::plugin::host::LogLevel::Info,
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

export!(HelloPlugin);
