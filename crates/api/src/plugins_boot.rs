//! Plugin startup: init calls, declaration collection, event dispatcher.

use vyasa_plugins::hooks::Registration;
use vyasa_plugins::host::HostEnv;

use crate::state::AppState;

/// Runs plugin `action` exports over the wasmtime host.
///
/// This replaces an earlier placeholder that returned an error for every
/// call: because `dispatch_action` degrades a plugin whenever its executor
/// fails, the placeholder marked every enabled plugin degraded on every
/// domain event, and no plugin action ever ran.
struct HostActions {
    host: std::sync::Arc<vyasa_plugins::host::WasmtimeHost>,
    broker: std::sync::Arc<vyasa_plugins::broker::Broker>,
    pool: sqlx::PgPool,
}

/// Translates the transport-free hook enum into the WIT binding enum.
const fn wit_kind(
    kind: vyasa_plugins::hooks::ActionKind,
) -> vyasa_plugins::host::vyasa::plugin::host::EventKind {
    use vyasa_plugins::hooks::ActionKind as A;
    use vyasa_plugins::host::vyasa::plugin::host::EventKind as W;
    match kind {
        A::PostPublished => W::PostPublished,
        A::PostUpdated => W::PostUpdated,
        A::PostTrashed => W::PostTrashed,
        A::CommentAdded => W::CommentAdded,
    }
}

impl vyasa_plugins::hooks::ActionExecutor for HostActions {
    fn invoke_action(
        &self,
        plugin_id: i64,
        kind: vyasa_plugins::hooks::ActionKind,
        payload: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        let host = std::sync::Arc::clone(&self.host);
        let env = vyasa_plugins::host::HostEnv::background(
            std::sync::Arc::clone(&self.broker),
            self.pool.clone(),
        );
        Box::pin(async move {
            match host
                .invoke_action(plugin_id, wit_kind(kind), &payload, &env)
                .await
            {
                vyasa_plugins::host::InvokeOutcome::Ok(_) => Ok(()),
                vyasa_plugins::host::InvokeOutcome::Failed(reason) => Err(reason),
            }
        })
    }
}

/// Marks a plugin degraded in the database after a failed action, so the
/// admin plugin list reflects reality instead of only the log.
struct DegradedStatus {
    pool: sqlx::PgPool,
    metrics: std::sync::Arc<crate::metrics::Metrics>,
}

impl vyasa_plugins::hooks::DegradedSink for DegradedStatus {
    fn mark_degraded(&self, plugin_id: i64, reason: &str) {
        tracing::warn!(plugin_id, "plugin action failed: {reason}");
        self.metrics.inc_plugin_failure();
        let repo = vyasa_db::repo::PluginsRepo::new(self.pool.clone());
        let reason = reason.to_owned();
        tokio::spawn(async move {
            if let Err(e) = repo.set_status_reason(plugin_id, "degraded", &reason).await {
                tracing::warn!(plugin_id, "could not persist degraded status: {e}");
            }
        });
    }
}

/// A plugin's stored settings as the JSON object handed to `init`.
async fn settings_json(repo: &vyasa_db::repo::PluginsRepo, plugin_id: i64) -> String {
    match repo.settings(plugin_id).await {
        Ok(pairs) if !pairs.is_empty() => {
            let map: serde_json::Map<String, serde_json::Value> = pairs.into_iter().collect();
            serde_json::to_string(&map).unwrap_or_else(|_| String::from("{}"))
        }
        _ => String::from("{}"),
    }
}

/// Loads every enabled plugin, runs `init`, records everything it
/// declares, starts the task runner and spawns the domain-event
/// dispatcher.
///
/// # Errors
/// Propagates only a failure to list the enabled plugins: per-plugin
/// problems are logged and that plugin is marked errored, so one bad
/// plugin never keeps the site from booting.
/// Brings one enabled plugin into the running process: `init` with its
/// settings, then everything it declares.
///
/// Extracted from the boot loop so that enabling a plugin can call it too.
/// Enabling used to record a flag and nothing else, so the plugin did
/// nothing at all until the next restart — while *disabling* took effect
/// at once, which made the pair behave asymmetrically for no reason a
/// person could see.
///
/// Returns `Ok(false)` when the plugin refused its configuration or
/// trapped; the caller decides whether that is worth reporting.
///
/// # Errors
/// With [`crate::plugin_surface::Collect::Enable`], a Conflict when the
/// plugin declares a post type or taxonomy whose slug is an
/// administrator's content type. Everything it contributed has been
/// withdrawn and it is marked errored with that reason; the caller turns
/// it off. At boot such a declaration is skipped instead, and the plugin
/// is marked degraded with the reason (its health on the plugins page).
pub async fn activate(
    state: &AppState,
    row: &vyasa_db::repo::PluginRow,
    mode: crate::plugin_surface::Collect,
) -> Result<bool, vyasa_common::AppError> {
    let repo = vyasa_db::repo::PluginsRepo::new(state.pool.clone());
    let env = HostEnv::background(std::sync::Arc::clone(&state.broker), state.pool.clone());
    // Its own settings, not an empty object: the settings kv, its REST
    // endpoints and the whole per-plugin configuration story were
    // disconnected from the guest, which always saw `{}`.
    let config = settings_json(&repo, row.id).await;
    match state.plugin_host.call_init(row.id, &config, &env).await {
        Ok(Ok(())) => {
            // A clean start heals a degraded mark; one earlier hiccup is
            // no longer permanent damage on the admin page.
            repo.set_status_reason(row.id, "loaded", "").await.ok();
        }
        Ok(Err(reject)) => {
            tracing::warn!("plugin {} rejected config: {reject}", row.name);
            repo.set_status_reason(
                row.id,
                "errored",
                &format!("rejected its settings: {reject}"),
            )
            .await
            .ok();
            return Ok(false);
        }
        Err(e) => {
            tracing::warn!("plugin {} init trap: {e}", row.name);
            repo.set_status_reason(row.id, "errored", &format!("crashed during init: {e}"))
                .await
                .ok();
            return Ok(false);
        }
    }
    // Declarations (best-effort; failures leave registries empty).
    if let Ok(blocks_json) = state.plugin_host.call_register_blocks(row.id, &env).await {
        let kinds = state
            .plugin_blocks
            .register_from_json(row.id, &row.name, &blocks_json)
            .await;
        if !kinds.is_empty() {
            tracing::info!(plugin = %row.name, "blocks: {}", kinds.join(", "));
        }
    }
    let routes_json = state
        .plugin_host
        .call_register_routes(row.id, &env)
        .await
        .unwrap_or_else(|_| String::from("[]"));
    // Routes, assets, admin forms, post types, taxonomies and schedules.
    // Everything past routes comes from the superset world, so a plugin
    // built against the base world simply contributes none of it.
    match crate::plugin_surface::collect(state, row.id, &row.name, &routes_json, mode).await {
        Ok(skipped) if !skipped.is_empty() => {
            repo.set_status_reason(row.id, "degraded", &skipped.join("; "))
                .await
                .ok();
        }
        Ok(_) => {}
        Err(refused) => {
            // Nothing half-registered: withdraw whatever was collected
            // before the refusal.
            state.plugin_surface.forget(row.id).await;
            state.plugin_blocks.forget(row.id).await;
            // And the compiled instance and grants its init call cached:
            // a refused plugin holds no memory and no stale grant here.
            state.plugin_host.evict(row.id).await;
            state.broker.evict(row.id).await;
            repo.set_status_reason(row.id, "errored", &refused.client_message())
                .await
                .ok();
            return Err(refused);
        }
    }
    // Every hook point the host dispatches. `Content`, `FeedItem` and
    // `SeoTitle` once had no registrants at all, so a plugin could declare
    // interest in them and never be called — including the post-published
    // and post-updated actions, which are addressed to the `Content`
    // audience.
    //
    // Registering on `PageHtml` does not grant the page-html *filter*: that
    // needs `html:page`, checked in `invoke_filter` on every call. The
    // registration stays unconditional because the post-trashed action is
    // addressed to the `PageHtml` audience, and a plugin should not lose an
    // event for not asking to rewrite pages.
    for hook in [
        vyasa_plugins::hooks::HookPoint::PageHtml,
        vyasa_plugins::hooks::HookPoint::CommentBody,
        vyasa_plugins::hooks::HookPoint::AuthChallenge,
        vyasa_plugins::hooks::HookPoint::Content,
        vyasa_plugins::hooks::HookPoint::FeedItem,
        // SeoTitle is deliberately absent: the WIT has no stage for it, so
        // it arrives at a guest as `content` and a plugin cannot tell the
        // two apart. Registering it would call every plugin twice with the
        // same stage.
    ] {
        state
            .hook_registry
            .register(Registration {
                plugin_id: row.id,
                hook,
                priority: 100,
            })
            .await;
    }
    Ok(true)
}

pub async fn bootstrap_plugins(state: &AppState) -> Result<(), vyasa_common::AppError> {
    let repo = vyasa_db::repo::PluginsRepo::new(state.pool.clone());
    // Administrators' content types first: one holds its slug against a
    // plugin declaring the same one, which is then skipped and reported
    // in that plugin's health rather than taking the type over.
    match vyasa_core::content::ContentTypesService::new(state.pool.clone())
        .load_into_registry()
        .await
    {
        Ok(skipped) => {
            for (slug, reason) in skipped {
                tracing::error!(slug, "content type not loaded: {reason}");
            }
        }
        Err(e) => tracing::error!("content types could not be loaded: {e}"),
    }
    for row in repo.list_enabled().await? {
        // Boot never refuses a plugin outright: a clash is skipped.
        let _ = activate(state, &row, crate::plugin_surface::Collect::Boot).await;
    }

    // Scheduled work: reconcile the declared tasks against the table so a
    // task keeps its last-run time across restarts, then start the runner.
    if let Err(e) = crate::plugin_tasks::reconcile(state).await {
        tracing::warn!("could not record plugin tasks: {e}");
    }
    crate::plugin_tasks::spawn_runner(state.clone());

    // Domain-event fan-out.
    vyasa_plugins::dispatch::spawn_event_dispatcher(
        state.hook_registry.clone(),
        std::sync::Arc::new(HostActions {
            host: std::sync::Arc::clone(&state.plugin_host),
            broker: std::sync::Arc::clone(&state.broker),
            pool: state.pool.clone(),
        }),
        std::sync::Arc::new(DegradedStatus {
            pool: state.pool.clone(),
            metrics: std::sync::Arc::clone(&state.metrics),
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use vyasa_testkit::TestDb;

    use super::activate;
    use crate::authz::tests::test_state;
    use crate::plugin_surface::Collect;

    const BOOKSHELF_WASM: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugin-sdk/examples/bookshelf/bookshelf-component.wasm"
    );

    /// A refused enable leaves nothing of the plugin behind in this
    /// process: not its declarations, and not the compiled instance or the
    /// capability grants its init call cached.
    #[tokio::test]
    async fn a_refused_enable_evicts_the_plugins_instance() {
        let db = TestDb::new().await;
        let state = test_state(&db);
        // An administrator's type holds the slug the plugin declares as
        // its post type (`book`).
        sqlx::query(
            "INSERT INTO content_types (slug, singular, plural, description, public, has_archive)
             VALUES ('book', 'Volume', 'Volumes', '', true, true)",
        )
        .execute(&state.pool)
        .await
        .expect("admin type");
        let types = vyasa_core::content::ContentTypesService::new(state.pool.clone());
        types.load_into_registry().await.expect("registry");

        let wasm = std::fs::read(BOOKSHELF_WASM).expect("bookshelf fixture");
        let repo = vyasa_db::repo::PluginsRepo::new(state.pool.clone());
        let row = repo
            .create(
                "bookshelf",
                "0.1.0",
                &wasm,
                &vyasa_plugins::host::sha256_hex(&wasm),
                &serde_json::json!(["log:write", "db:read:posts", "kv:read", "kv:write"]),
            )
            .await
            .expect("install plugin");
        repo.set_enabled(row.id, true).await.expect("enable");
        let row = repo
            .list()
            .await
            .expect("list")
            .into_iter()
            .find(|p| p.id == row.id)
            .expect("row");

        let refused = activate(&state, &row, Collect::Enable).await;
        let outcome = format!("{refused:?}");
        types
            .delete("book")
            .await
            .expect("leave the registry as it was");
        assert!(refused.is_err(), "{outcome}");
        assert!(
            !state.plugin_host.is_loaded(row.id).await,
            "the instance its init loaded is gone"
        );
        assert!(
            !state.broker.has_cached_grants(row.id).await,
            "its cached grants are gone"
        );
    }
}
