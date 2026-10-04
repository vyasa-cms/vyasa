//! Wasmtime host: loads plugin components with fuel + memory + epoch
//! limits, pools linked components, and isolates crashes.
//!
//! Every host import is gated by the capability broker, and every call
//! gets its own store and instance so one visitor's guest state cannot
//! reach the next.
#![allow(missing_docs)] // wasmtime bindgen output

use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use vyasa_common::AppError;
use vyasa_db::repo::{PluginRow, PluginsRepo};
use wasmtime::component::HasSelf;
use wasmtime::component::{bindgen, Component, Linker};
use wasmtime::{Config, Engine, Store as WasmStore};

use crate::host::vyasa::plugin::host;

bindgen!({
    path: "wit/host.wit",
    world: "vyasa-plugin",
    imports: { default: async },
    exports: { default: async },
    require_store_data_send: true,
});

/// Bindings for the optional superset world.
///
/// Generated separately from the base world, with `host` mapped onto the
/// bindings above so the import side is not duplicated. Resolving these
/// indices is how the host asks "does this component implement v2?": a
/// plugin built against the base world simply fails the lookup once, at
/// load, and is served by the v1 path forever after.
pub mod v2 {
    wasmtime::component::bindgen!({
        path: "wit/host.wit",
        world: "vyasa-plugin-v2",
        imports: { default: async },
        exports: { default: async },
        require_store_data_send: true,
        with: {
            "vyasa:plugin/host": crate::host::vyasa::plugin::host,
        },
    });
}

/// Per-call limits applied to instantiation and invocation.
#[derive(Debug, Clone)]
pub struct HostLimits {
    /// Fuel burned per hook call (wasmtime fuel units).
    pub fuel_per_call: u64,
    /// Maximum linear memory in bytes per instance.
    pub max_memory_bytes: usize,
    /// Epoch ticks a call may span before interruption.
    pub epoch_deadline_ticks: u64,
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            // Roughly half a second of guest compute on commodity
            // hardware. The previous 10 billion was ~5 seconds of CPU,
            // and the epoch deadline of 10 000 ticks (50 ms each) allowed
            // over eight minutes — so neither limit bounded anything a
            // request would survive anyway.
            fuel_per_call: 1_000_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
            // 40 ticks x 50 ms = a 2 second wall-clock ceiling, which
            // also catches a guest blocked on a host call rather than
            // burning fuel.
            epoch_deadline_ticks: 40,
        }
    }
}

/// The text model, as the API crate provides it. Plugins never see keys or
/// providers; they get a reply from whatever the site's Models page says is
/// the default, logged under their own name.
pub trait AiBackend: Send + Sync {
    /// One plain-text completion for `plugin_name`.
    fn complete<'a>(
        &'a self,
        plugin_name: &'a str,
        system: String,
        user: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Outbound HTTP, as the API crate provides it. Keeping the client out of
/// this crate preserves the dependency direction and leaves exactly one
/// implementation to audit at the sandbox boundary.
pub trait FetchBackend: Send + Sync {
    /// GETs `url`, returning the body as text or a message.
    fn get<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Longest `user` prompt a plugin may send, in characters.
pub const MAX_AI_PROMPT_CHARS: usize = 20_000;

/// Longest `system` prompt a plugin may send, in characters.
///
/// The user prompt was capped and the system prompt was not, so the cap
/// was one argument away from meaningless.
pub const MAX_AI_SYSTEM_CHARS: usize = 8_000;

/// How long a text-model call may hold a render-path hook.
///
/// Below the store's 2 s epoch deadline: a guest parked on a host call
/// is only interrupted once the call returns, so a reply that arrives
/// after the deadline both held the page and trapped the plugin.
pub const RENDER_AI_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1_500);

/// Whether, and for how long, a text-model call may run in the call a
/// store is serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiWindow {
    /// Routes, actions, events and scheduled tasks: the provider's own
    /// timeout applies.
    Unbounded,
    /// Render-path hooks other than `page-html`: bounded by
    /// [`RENDER_AI_TIMEOUT`].
    Render,
    /// The `page-html` filter. Refused outright: its output is cached
    /// and shared, it runs on every page of the site, and a model reply
    /// spliced into it is text nobody reviewed going to every visitor.
    Forbidden,
}

/// Who is signed in for the request this call is serving.
///
/// Deliberately not the whole user row: a plugin gets what it needs to
/// personalise a page, never an email address or anything it could sign
/// in with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    /// User id.
    pub id: i64,
    /// Display name.
    pub display_name: String,
    /// A built-in role's name, or the slug of the custom role the
    /// account holds.
    pub role: String,
}

/// Host-side state visible to imported functions.
pub struct HostCtx {
    /// Plugin display name (used as the tracing target).
    pub plugin_name: String,
    /// Plugin row id (broker subject).
    pub plugin_id: i64,
    /// Per-instance resource limits (set at instantiation).
    pub limits: Option<wasmtime::StoreLimits>,
    /// WASI context required by the component's implicit `wasi:io/poll`.
    pub wasi: wasmtime_wasi::WasiCtx,
    /// Resource table required by the p2 IoView trait.
    pub table: wasmtime_wasi::ResourceTable,
    /// Capability broker + destination pool for brokered reads.
    pub broker: std::sync::Arc<crate::broker::Broker>,
    /// Destination database pool backing brokered reads.
    pub dest: sqlx::PgPool,
    /// The text model, when the application wired one.
    pub ai: Option<std::sync::Arc<dyn AiBackend>>,
    /// Outbound HTTP, when the application wired it.
    pub fetch: Option<std::sync::Arc<dyn FetchBackend>>,
    /// The signed-in visitor, when this call is serving one request whose
    /// output nothing caches.
    pub viewer: Option<Viewer>,
    /// How long `ai-complete` may run for this call.
    pub ai_window: AiWindow,
}

/// Broker + pool accessor bundle shared across host calls.
pub struct HostEnv {
    /// The capability broker.
    pub broker: std::sync::Arc<crate::broker::Broker>,
    /// Destination pool.
    pub dest: sqlx::PgPool,
    /// Who is signed in, when this call serves one uncached request.
    ///
    /// `None` for boot-time calls, scheduled tasks, and — deliberately —
    /// page renders: those are cached and the result is shared, so a
    /// plugin personalising there would leak one visitor's page to the
    /// next. Only [`HostEnv::for_viewer`] sets it, and only the plugin
    /// route calls that.
    pub viewer: Option<Viewer>,
}

impl HostEnv {
    /// An environment for work whose output is cached or serves nobody:
    /// boot, scheduled tasks, and every page render.
    #[must_use]
    pub fn background(broker: std::sync::Arc<crate::broker::Broker>, dest: sqlx::PgPool) -> Self {
        Self {
            broker,
            dest,
            viewer: None,
        }
    }

    /// The same environment, serving `viewer`.
    #[must_use]
    pub fn for_viewer(mut self, viewer: Option<Viewer>) -> Self {
        self.viewer = viewer;
        self
    }
}

impl HostCtx {
    /// The subject + destination bundle the data host functions take.
    fn data_ctx(&self) -> crate::hostdata::Ctx<'_> {
        crate::hostdata::Ctx {
            broker: &self.broker,
            pool: &self.dest,
            plugin_id: self.plugin_id,
        }
    }
}

impl wasmtime_wasi::WasiView for HostCtx {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Options readable by plugins without extra grants.
const PUBLIC_OPTIONS: &[&str] = &[
    "site_title",
    "site_tagline",
    "permalink_pattern",
    "posts_per_page",
];

impl host::Host for HostCtx {
    async fn log(&mut self, level: host::LogLevel, message: String) {
        // `log:write` was declarable and never enforced, so the
        // capability told an operator nothing about what a plugin does.
        if self.broker.check_log(self.plugin_id).await != crate::broker::Decision::Allowed {
            return;
        }
        let target = format!("plugin:{}", self.plugin_name);
        match level {
            host::LogLevel::Trace => tracing::trace!(target, "{message}"),
            host::LogLevel::Debug => tracing::debug!(target, "{message}"),
            host::LogLevel::Info => tracing::info!(target, "{message}"),
            host::LogLevel::Warn => tracing::warn!(target, "{message}"),
            host::LogLevel::Error => tracing::error!(target, "{message}"),
        }
    }

    async fn get_post(&mut self, id: u64) -> Result<host::PostSummary, String> {
        if self
            .broker
            .check_db_read(self.plugin_id, crate::capabilities::Capability::DbReadPosts)
            .await
            != crate::broker::Decision::Allowed
        {
            return Err(String::from("capability-denied"));
        }
        let Ok(Some(row)) = sqlx::query_as::<
            _,
            (
                i64,
                String,
                String,
                String,
                String,
                Option<chrono::DateTime<chrono::Utc>>,
            ),
        >(
            "SELECT id, type::text AS t, slug, title, COALESCE(excerpt,''), published_at \
             FROM posts WHERE id = $1 AND status = 'published'",
        )
        .bind(i64::try_from(id).unwrap_or(-1))
        .fetch_optional(&self.dest)
        .await
        else {
            return Err(String::from("not found"));
        };
        let (rid, ptype, slug, title, excerpt, published) = row;
        Ok(host::PostSummary {
            id: u64::try_from(rid).unwrap_or(0),
            post_type: ptype,
            slug,
            title,
            excerpt,
            author_id: 0,
            published_at: published.map(|d| d.timestamp_millis().unsigned_abs()),
        })
    }

    async fn query_posts(
        &mut self,
        filter_post_type: Option<String>,
        limit: u32,
    ) -> Result<Vec<host::PostSummary>, String> {
        if self
            .broker
            .check_db_read(self.plugin_id, crate::capabilities::Capability::DbReadPosts)
            .await
            != crate::broker::Decision::Allowed
        {
            return Err(String::from("capability-denied"));
        }
        let lim = i64::from(limit.clamp(1, 100));
        let rows = sqlx::query_as::<_, (i64, String, String, String, String)>(
            "SELECT id, type::text, slug, title, COALESCE(excerpt,'') FROM posts \
             WHERE status = 'published' AND ($1::text IS NULL OR type::text = $1) \
             ORDER BY published_at DESC NULLS LAST LIMIT $2",
        )
        .bind(filter_post_type)
        .bind(lim)
        .fetch_all(&self.dest)
        .await
        .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .map(|(id, ptype, slug, title, excerpt)| host::PostSummary {
                id: u64::try_from(id).unwrap_or(0),
                post_type: ptype,
                slug,
                title,
                excerpt,
                author_id: 0,
                published_at: None,
            })
            .collect())
    }

    async fn get_option(&mut self, key: String) -> Result<Option<String>, String> {
        if self
            .broker
            .check_db_read(
                self.plugin_id,
                crate::capabilities::Capability::DbReadOptions,
            )
            .await
            != crate::broker::Decision::Allowed
        {
            return Err(String::from("capability-denied"));
        }
        if !PUBLIC_OPTIONS.contains(&key.as_str()) {
            return Err(String::from("capability-denied"));
        }
        match sqlx::query_scalar::<_, serde_json::Value>("SELECT value FROM options WHERE key = $1")
            .bind(&key)
            .fetch_optional(&self.dest)
            .await
        {
            Ok(Some(v)) => Ok(Some(
                v.as_str().map_or_else(|| v.to_string(), str::to_owned),
            )),
            _ => Ok(None),
        }
    }

    async fn emit_event(&mut self, kind: host::EventKind, payload: String) -> Result<(), String> {
        if self.broker.check_event_emit(self.plugin_id).await != crate::broker::Decision::Allowed {
            return Err(String::from("capability-denied"));
        }
        if payload.len() > 64 * 1024 {
            return Err(String::from("payload too large"));
        }
        // A plugin may only announce things about content that exists,
        // and only by id: it cannot forge an event about arbitrary state.
        let id = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.get("id").and_then(serde_json::Value::as_i64))
            .ok_or_else(|| String::from("payload must be an object with a numeric \"id\""))?;
        let event = match kind {
            host::EventKind::PostPublished => {
                vyasa_core::events::Event::Published(vyasa_core::events::PostPublished {
                    post_id: id,
                    author_id: 0,
                })
            }
            host::EventKind::PostUpdated => {
                vyasa_core::events::Event::Updated(vyasa_core::events::PostUpdated { post_id: id })
            }
            host::EventKind::PostTrashed => {
                vyasa_core::events::Event::Trashed(vyasa_core::events::PostTrashed { post_id: id })
            }
            // A plugin cannot fabricate a comment or a render request:
            // those originate from a visitor, not from a plugin.
            host::EventKind::CommentAdded | host::EventKind::RequestRender => {
                return Err(String::from(
                    "this event kind cannot be emitted by a plugin",
                ));
            }
        };
        vyasa_core::events::emit(event);
        Ok(())
    }

    async fn fetch(&mut self, url: String) -> Result<String, String> {
        // https only: `check_fetch` cannot parse anything else, and a
        // plaintext request from inside the sandbox is never what an
        // operator meant to grant.
        if !url.starts_with("https://") {
            return Err(String::from("only https:// URLs may be fetched"));
        }
        if self.broker.check_fetch(self.plugin_id, &url).await != crate::broker::Decision::Allowed {
            return Err(String::from("capability-denied"));
        }
        let Some(fetch) = self.fetch.clone() else {
            return Err(String::from(
                "outbound fetch is not configured on this site",
            ));
        };
        let body = fetch.get(&url).await?;
        // Charge the byte quota after the fact; an over-budget plugin
        // gets the body it already paid for and is refused next time.
        if !self
            .broker
            .record_bytes(self.plugin_id, body.len() as u64)
            .await
        {
            return Err(String::from("byte quota exceeded"));
        }
        Ok(body)
    }

    async fn kv_get(&mut self, key: String) -> Result<Option<String>, String> {
        crate::hostdata::kv_get(&self.data_ctx(), key).await
    }

    async fn kv_set(&mut self, key: String, value: String) -> Result<(), String> {
        crate::hostdata::kv_set(&self.data_ctx(), key, value).await
    }

    async fn kv_delete(&mut self, key: String) -> Result<(), String> {
        crate::hostdata::kv_delete(&self.data_ctx(), key).await
    }

    async fn kv_list(&mut self, prefix: String, limit: u32) -> Result<Vec<String>, String> {
        crate::hostdata::kv_list(&self.data_ctx(), prefix, limit).await
    }

    async fn list_comments(
        &mut self,
        post_id: u64,
        limit: u32,
    ) -> Result<Vec<host::CommentSummary>, String> {
        let rows = crate::hostdata::list_comments(&self.data_ctx(), post_id, limit).await?;
        Ok(rows
            .into_iter()
            .map(|c| host::CommentSummary {
                id: c.id.unsigned_abs(),
                post_id: c.post_id.unsigned_abs(),
                author_name: c.author_name,
                body_html: c.body_html,
                created_at: c.created_at.unsigned_abs(),
            })
            .collect())
    }

    async fn create_post(&mut self, input: host::NewPost) -> Result<u64, String> {
        crate::hostdata::create_post(
            &self.data_ctx(),
            crate::hostdata::NewPost {
                post_type: input.post_type,
                title: input.title,
                slug: input.slug,
                body_html: input.body_html,
                excerpt: input.excerpt,
                status: input.status,
            },
        )
        .await
    }

    async fn update_post(&mut self, id: u64, patch: host::PostPatch) -> Result<(), String> {
        crate::hostdata::update_post(
            &self.data_ctx(),
            id,
            crate::hostdata::PostPatch {
                title: patch.title,
                body_html: patch.body_html,
                excerpt: patch.excerpt,
                status: patch.status,
            },
        )
        .await
    }

    async fn current_viewer(&mut self) -> Result<Option<host::Viewer>, String> {
        if self
            .broker
            .check_capability(self.plugin_id, &crate::capabilities::Capability::ViewerRead)
            .await
            != crate::broker::Decision::Allowed
        {
            return Err(String::from("capability-denied"));
        }
        Ok(self.viewer.as_ref().map(|v| host::Viewer {
            id: v.id.unsigned_abs(),
            display_name: v.display_name.clone(),
            role: v.role.clone(),
        }))
    }

    async fn get_post_meta(&mut self, post_id: u64, key: String) -> Result<Option<String>, String> {
        crate::hostdata::get_post_meta(&self.data_ctx(), &self.plugin_name, post_id, key).await
    }

    async fn set_post_meta(
        &mut self,
        post_id: u64,
        key: String,
        value: String,
    ) -> Result<(), String> {
        crate::hostdata::set_post_meta(&self.data_ctx(), &self.plugin_name, post_id, key, value)
            .await
    }

    async fn send_mail(&mut self, to: String, subject: String, body: String) -> Result<(), String> {
        crate::hostdata::send_mail(&self.data_ctx(), &self.plugin_name, to, subject, body).await
    }

    async fn ai_complete(&mut self, system: String, user: String) -> Result<String, String> {
        // Refusals that need no grant come first, so they spend no quota.
        if self.ai_window == AiWindow::Forbidden {
            return Err(String::from(
                "ai-complete is not available in the page-html filter",
            ));
        }
        if user.chars().count() > MAX_AI_PROMPT_CHARS {
            return Err(String::from("prompt too long"));
        }
        if system.chars().count() > MAX_AI_SYSTEM_CHARS {
            return Err(String::from("system prompt too long"));
        }
        if self.broker.check_ai(self.plugin_id).await != crate::broker::Decision::Allowed {
            return Err(String::from("capability-denied"));
        }
        let Some(ai) = self.ai.clone() else {
            return Err(String::from("no text model is set up on this site"));
        };
        let reply = ai.complete(&self.plugin_name, system, user);
        match self.ai_window {
            AiWindow::Render => tokio::time::timeout(RENDER_AI_TIMEOUT, reply)
                .await
                .unwrap_or_else(|_| Err(String::from("ai-complete timed out"))),
            _ => reply.await,
        }
    }
}

/// One pooled plugin: instance + its store behind a mutex.
pub struct PooledPlugin {
    /// Component linked against the host imports, ready to instantiate.
    ///
    /// Deliberately *not* a live instance: every call gets its own store
    /// and instance. Sharing one instance behind a mutex made a plugin a
    /// site-wide serialization point (eight concurrent renders through a
    /// 670 ms plugin took 5 seconds instead of 700 ms) and let a plugin's
    /// globals persist from one visitor's request into the next.
    pub pre: VyasaPluginPre<HostCtx>,
    /// Plugin name, for host-side logging and audit.
    pub name: String,
    /// Whether the component also implements the superset world.
    ///
    /// Resolved once, at load: asking per call would pay an export lookup
    /// on every request for an answer that cannot change while the
    /// component is cached.
    pub v2: bool,
}

/// Instance pool keyed by (plugin_id, version).
#[allow(missing_docs)]
pub type InstancePool = HashMap<i64, Arc<PooledPlugin>>;

/// The execution substrate.
#[derive(Clone)]
pub struct WasmtimeHost {
    engine: Engine,
    linker: Arc<Linker<HostCtx>>,
    repo: PluginsRepo,
    instances: Arc<tokio::sync::Mutex<InstancePool>>,
    limits: HostLimits,
    ai: Arc<std::sync::RwLock<Option<Arc<dyn AiBackend>>>>,
    fetch: Arc<std::sync::RwLock<Option<Arc<dyn FetchBackend>>>>,
}

impl WasmtimeHost {
    /// Wires the text model plugins may call through `ai-complete`.
    pub fn set_ai_backend(&self, backend: Arc<dyn AiBackend>) {
        if let Ok(mut slot) = self.ai.write() {
            *slot = Some(backend);
        }
    }

    /// Wires outbound HTTP for `fetch`. Without it the capability exists
    /// but every call is refused, which is how it shipped.
    pub fn set_fetch_backend(&self, backend: Arc<dyn FetchBackend>) {
        if let Ok(mut slot) = self.fetch.write() {
            *slot = Some(backend);
        }
    }

    /// Builds an engine with epoch interruption enabled and spawns the
    /// ticker advancing deadlines.
    ///
    /// # Errors
    /// [`AppError::Internal`] when the engine cannot be configured.
    pub fn new(repo: PluginsRepo) -> Result<Self, AppError> {
        Self::with_limits(repo, HostLimits::default())
    }

    /// Same as [`WasmtimeHost::new`] with explicit limits.
    ///
    /// # Errors
    /// See [`WasmtimeHost::new`].
    pub fn with_limits(repo: PluginsRepo, limits: HostLimits) -> Result<Self, AppError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)
            .map_err(|e| AppError::internal_msg(format!("wasmtime engine: {e}")))?;

        // Advance epochs every 50 ms so deadline checks fire promptly.
        let ticker = engine.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
            loop {
                tick.tick().await;
                ticker.increment_epoch();
            }
        });

        let mut linker: Linker<HostCtx> = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async::<HostCtx>(&mut linker)
            .map_err(|e| AppError::internal_msg(format!("wasi linker: {e}")))?;
        VyasaPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |ctx| ctx)
            .map_err(|e| AppError::internal_msg(format!("linker: {e}")))?;

        Ok(Self {
            engine,
            linker: Arc::new(linker),
            repo,
            instances: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            limits,
            ai: Arc::new(std::sync::RwLock::new(None)),
            fetch: Arc::new(std::sync::RwLock::new(None)),
        })
    }

    /// Loads (or returns the pooled) instance for the plugin's active
    /// version. Disabled/errored plugins are refused; artifact hashes are
    /// verified before compile.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for unknown plugins; hash mismatch is
    /// [`AppError::validation`]; compile/instantiate failures mark the
    /// plugin errored and surface as internal errors.
    pub async fn load(&self, plugin_id: i64, env: &HostEnv) -> Result<Arc<PooledPlugin>, AppError> {
        // Cache hit is the hot path: a page render calls every registered
        // plugin, and this used to `SELECT` the whole plugins table each
        // time before doing anything else. Enable/disable and version
        // changes take effect at the next boot (that is already true of
        // hook registration), and a trap evicts the entry here and now,
        // so a stale entry cannot keep a broken plugin running.
        if let Some(hit) = self.instances.lock().await.get(&plugin_id) {
            return Ok(Arc::clone(hit));
        }
        let row: PluginRow = self
            .repo
            .list()
            .await?
            .into_iter()
            .find(|p| p.id == plugin_id)
            .ok_or_else(|| AppError::not_found("plugin", plugin_id))?;
        if !row.enabled || row.status == "errored" {
            return Err(AppError::validation(format!(
                "plugin {} is disabled or errored",
                row.name
            )));
        }
        let inst = self.instantiate(&row, env).await?;
        let mut pool = self.instances.lock().await;
        // A linked component is cheap to hold (no instance, no store), so
        // the bound only guards against an unbounded install list.
        if pool.len() >= 64 && !pool.contains_key(&plugin_id) {
            if let Some(victim) = pool.keys().copied().next() {
                drop(pool.remove(&victim));
            }
        }
        pool.insert(plugin_id, Arc::clone(&inst));
        drop(pool);
        self.repo.set_status(plugin_id, "loaded").await.ok();
        Ok(inst)
    }

    /// Drops a cached component so the next call reloads it from the
    /// database. Call after install, enable, disable or rollback.
    pub async fn evict(&self, plugin_id: i64) {
        self.instances.lock().await.remove(&plugin_id);
    }

    /// Whether a compiled component for the plugin is held in the pool.
    pub async fn is_loaded(&self, plugin_id: i64) -> bool {
        self.instances.lock().await.contains_key(&plugin_id)
    }

    /// Compiles and links the component. No instance is created here:
    /// instances are per call.
    async fn instantiate(
        &self,
        row: &PluginRow,
        _env: &HostEnv,
    ) -> Result<Arc<PooledPlugin>, AppError> {
        let (bytes, sha) = self.repo.wasm_bytes(row.id, &row.version).await?;
        if sha256_hex(&bytes) != sha {
            self.repo.set_status(row.id, "errored").await.ok();
            return Err(AppError::validation(format!(
                "plugin {} artifact hash mismatch",
                row.name
            )));
        }
        let component = match Component::from_binary(&self.engine, &bytes) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("component {}: compile failed: {e}", row.name);
                self.mark_errored(row.id).await;
                return Err(AppError::internal_msg(msg));
            }
        };
        let pre = match self
            .linker
            .instantiate_pre(&component)
            .and_then(VyasaPluginPre::new)
        {
            Ok(pre) => pre,
            Err(e) => {
                let msg = format!("link {}: {e}", row.name);
                self.mark_errored(row.id).await;
                return Err(AppError::internal_msg(msg));
            }
        };
        // A base-world plugin fails this and is served by the v1 path;
        // nothing about that is an error, so it is not logged as one.
        let v2 = v2::VyasaPluginV2Pre::new(pre.instance_pre().clone()).is_ok();
        tracing::debug!(plugin = %row.name, v2, "plugin linked");
        Ok(Arc::new(PooledPlugin {
            pre,
            name: row.name.clone(),
            v2,
        }))
    }

    /// A store for one call: fresh guest state, fresh fuel, its own
    /// deadline. Creating one per call is what makes plugin calls
    /// concurrent and keeps one request's guest state out of the next.
    fn call_store(
        &self,
        pooled: &PooledPlugin,
        plugin_id: i64,
        env: &HostEnv,
    ) -> WasmStore<HostCtx> {
        let limits = Some(
            wasmtime::StoreLimitsBuilder::new()
                .memory_size(self.limits.max_memory_bytes)
                .build(),
        );
        let mut store = WasmStore::new(
            &self.engine,
            HostCtx {
                plugin_name: pooled.name.clone(),
                plugin_id,
                limits,
                wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
                table: wasmtime_wasi::ResourceTable::new(),
                broker: env.broker.clone(),
                dest: env.dest.clone(),
                ai: self.ai.read().ok().and_then(|g| g.clone()),
                fetch: self.fetch.read().ok().and_then(|g| g.clone()),
                viewer: env.viewer.clone(),
                ai_window: AiWindow::Unbounded,
            },
        );
        // Both budgets are best-effort: a store that refuses them still
        // runs, just unbounded, which the epoch ticker still interrupts.
        let _ = store.set_fuel(self.limits.fuel_per_call);
        store.limiter(|ctx: &mut HostCtx| {
            ctx.limits
                .as_mut()
                .unwrap_or_else(|| unreachable!("limits set at instantiation"))
        });
        store.set_epoch_deadline(self.limits.epoch_deadline_ticks);
        store
    }

    /// Instantiates for one call: the raw instance plus its store.
    ///
    /// Kept separate from the binding construction so that a v2 call binds
    /// the *same* instance through the superset world rather than paying
    /// for a second instantiation.
    async fn begin(
        &self,
        plugin_id: i64,
        env: &HostEnv,
    ) -> Result<
        (
            Arc<PooledPlugin>,
            wasmtime::component::Instance,
            WasmStore<HostCtx>,
        ),
        AppError,
    > {
        let pooled = self.load(plugin_id, env).await?;
        let mut store = self.call_store(&pooled, plugin_id, env);
        let instance = pooled
            .pre
            .instance_pre()
            .instantiate_async(&mut store)
            .await
            .map_err(|e| AppError::internal_msg(format!("instantiate: {e}")))?;
        Ok((pooled, instance, store))
    }

    /// Instantiates for one call and hands back the bindings plus store.
    async fn begin_call(
        &self,
        plugin_id: i64,
        env: &HostEnv,
    ) -> Result<(VyasaPlugin, WasmStore<HostCtx>), AppError> {
        let (_, instance, mut store) = self.begin(plugin_id, env).await?;
        let bindings = VyasaPlugin::new(&mut store, &instance)
            .map_err(|e| AppError::internal_msg(format!("bind: {e}")))?;
        Ok((bindings, store))
    }

    /// The same, through the superset world.
    ///
    /// `Ok(None)` means the plugin implements the base world only — an
    /// ordinary answer, not a failure: it is how a plugin built before v2
    /// existed keeps working.
    async fn begin_call_v2(
        &self,
        plugin_id: i64,
        env: &HostEnv,
    ) -> Result<Option<(v2::VyasaPluginV2, WasmStore<HostCtx>)>, AppError> {
        let (pooled, instance, mut store) = self.begin(plugin_id, env).await?;
        if !pooled.v2 {
            return Ok(None);
        }
        let bindings = v2::VyasaPluginV2::new(&mut store, &instance)
            .map_err(|e| AppError::internal_msg(format!("bind v2: {e}")))?;
        Ok(Some((bindings, store)))
    }

    /// Whether this plugin implements the superset world.
    ///
    /// # Errors
    /// Propagates load failures.
    pub async fn supports_v2(&self, plugin_id: i64, env: &HostEnv) -> Result<bool, AppError> {
        Ok(self.load(plugin_id, env).await?.v2)
    }

    /// Calls the plugin's `init` export with admin config.
    ///
    /// Returns `Err(msg)` when the plugin itself rejected the config and
    /// [`AppError::internal`] on traps (which also mark the plugin errored).
    ///
    /// # Errors
    /// Propagates load failures.
    pub async fn call_init(
        &self,
        plugin_id: i64,
        config_json: &str,
        env: &HostEnv,
    ) -> Result<Result<(), String>, AppError> {
        let (bindings, mut store) = self.begin_call(plugin_id, env).await?;
        match bindings.call_init(&mut store, config_json).await {
            Ok(result) => Ok(result.map(|_| ())),
            Err(trap) => {
                let msg = Self::trap_reason(&trap);
                self.mark_errored(plugin_id).await;
                Err(AppError::internal_msg(format!("init trap: {msg}")))
            }
        }
    }

    /// Invokes the `filter` export with fuel/epoch enforcement.
    ///
    /// Traps never propagate: they mark the plugin errored+disabled and
    /// return [`InvokeOutcome::Failed`] so request handling proceeds.
    pub async fn invoke_filter(
        &self,
        plugin_id: i64,
        stage: FilterStage,
        content: String,
        env: &HostEnv,
    ) -> InvokeOutcome {
        let page_html = matches!(stage, FilterStage::PageHtml);
        // `page-html` rewrites every public page, script included, so it
        // is a granted power rather than something every installed plugin
        // gets. Registration is shared with the post-trashed action, so
        // the gate lives here; a plugin without the grant is skipped, not
        // degraded — not asking for a power is not a failure.
        if page_html
            && !env
                .broker
                .holds(plugin_id, &crate::capabilities::Capability::HtmlPage)
                .await
        {
            return InvokeOutcome::Ok(content);
        }
        let (bindings, mut store) = match self.begin_call(plugin_id, env).await {
            Ok(pair) => pair,
            Err(e) => return InvokeOutcome::Failed(e.to_string()),
        };
        store.data_mut().ai_window = if page_html {
            AiWindow::Forbidden
        } else {
            AiWindow::Render
        };
        match bindings.call_filter(&mut store, stage, &content).await {
            Ok(out) => InvokeOutcome::Ok(out),
            Err(trap) => {
                let msg = Self::trap_reason(&trap);
                self.on_trap(plugin_id, &msg).await;
                InvokeOutcome::Failed(msg)
            }
        }
    }

    async fn mark_errored(&self, plugin_id: i64) {
        // Evict first: the cached component must not answer another call
        // now that the plugin is disabled.
        self.evict(plugin_id).await;
        self.repo.set_status(plugin_id, "errored").await.ok();
        self.repo.set_enabled(plugin_id, false).await.ok();
    }

    async fn on_trap(&self, plugin_id: i64, message: &str) {
        tracing::warn!(plugin_id, "plugin trap: {message}");
        self.mark_errored(plugin_id).await;
    }

    /// A trap's real reason.
    ///
    /// `Display` on a wasmtime error prints only the backtrace, so a
    /// production trap logged as "error while executing at wasm
    /// backtrace" with no cause — which is unactionable. Fuel
    /// exhaustion, an epoch deadline and an explicit unreachable all
    /// live in the source chain.
    fn trap_reason(err: &wasmtime::Error) -> String {
        // `{:#}` on a wasmtime error renders the whole cause chain,
        // which is where "all fuel consumed" and "epoch deadline" live.
        let mut reason = format!("{err:#}");
        if let Some(trap) = err.downcast_ref::<wasmtime::Trap>() {
            reason = format!("{reason} (trap: {trap})");
        }
        reason
    }
}

/// Outcome of invoking a filter hook.
#[derive(Debug, Clone)]
pub enum InvokeOutcome {
    /// Content returned by the plugin (or its error text).
    Ok(String),
    /// The plugin trapped or exceeded limits; message recorded.
    Failed(String),
}

/// Hex sha256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl WasmtimeHost {
    /// Invokes the `action` export (fire-and-forget semantics handled by
    /// the caller).
    pub async fn invoke_action(
        &self,
        plugin_id: i64,
        kind: host::EventKind,
        payload_json: &str,
        env: &HostEnv,
    ) -> InvokeOutcome {
        let (bindings, mut store) = match self.begin_call(plugin_id, env).await {
            Ok(pair) => pair,
            Err(e) => return InvokeOutcome::Failed(e.to_string()),
        };
        match bindings.call_action(&mut store, kind, payload_json).await {
            Ok(Ok(())) => InvokeOutcome::Ok(String::new()),
            Ok(Err(e)) => InvokeOutcome::Failed(e),
            Err(trap) => {
                let msg = Self::trap_reason(&trap);
                self.on_trap(plugin_id, &msg).await;
                InvokeOutcome::Failed(msg)
            }
        }
    }

    /// Calls `register_blocks`, returning the raw JSON declaration.
    ///
    /// # Errors
    /// Returns [`AppError::internal`] on traps; load errors propagate.
    pub async fn call_register_blocks(
        &self,
        plugin_id: i64,
        env: &HostEnv,
    ) -> Result<String, AppError> {
        let (bindings, mut store) = self.begin_call(plugin_id, env).await?;
        match bindings.call_register_blocks(&mut store).await {
            Ok(s) => Ok(s),
            Err(trap) => Err(AppError::internal_msg(format!(
                "register_blocks trap: {}",
                Self::trap_reason(&trap)
            ))),
        }
    }

    /// Calls `register_routes`, returning the raw JSON declaration.
    ///
    /// # Errors
    /// Returns [`AppError::internal`] on traps; load errors propagate.
    pub async fn call_register_routes(
        &self,
        plugin_id: i64,
        env: &HostEnv,
    ) -> Result<String, AppError> {
        let (bindings, mut store) = self.begin_call(plugin_id, env).await?;
        match bindings.call_register_routes(&mut store).await {
            Ok(s) => Ok(s),
            Err(trap) => Err(AppError::internal_msg(format!(
                "register_routes trap: {}",
                Self::trap_reason(&trap)
            ))),
        }
    }
}

/// Which optional declaration to ask a plugin for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declaration {
    /// Periodic work: `[{ name, every-seconds }]`.
    Schedule,
    /// Admin settings form.
    Admin,
    /// Front-end CSS/JS.
    Assets,
    /// Custom post types.
    PostTypes,
    /// Custom taxonomies.
    Taxonomies,
}

impl Declaration {
    /// Name used in log lines and error messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "register-schedule",
            Self::Admin => "register-admin",
            Self::Assets => "register-assets",
            Self::PostTypes => "register-post-types",
            Self::Taxonomies => "register-taxonomies",
        }
    }
}

/// Outcome of a superset-world call.
///
/// `Unsupported` is not a failure: it is the answer for every plugin built
/// against the base world, and callers treat it as "this plugin declares
/// none of that" rather than as something to report or degrade.
#[derive(Debug, Clone)]
pub enum V2Outcome<T> {
    /// The component does not implement the superset world.
    Unsupported,
    /// The plugin answered.
    Ok(T),
    /// The plugin refused, trapped or could not be loaded.
    Failed(String),
}

impl<T> V2Outcome<T> {
    /// The value, or `None` for both "not implemented" and "failed".
    pub fn ok(self) -> Option<T> {
        match self {
            Self::Ok(v) => Some(v),
            _ => None,
        }
    }
}

impl WasmtimeHost {
    /// Serves one request through a plugin's `handle-request` export.
    pub async fn call_handle_request(
        &self,
        plugin_id: i64,
        req: &host::HttpRequest,
        env: &HostEnv,
    ) -> V2Outcome<host::HttpResponse> {
        let (bindings, mut store) = match self.begin_call_v2(plugin_id, env).await {
            Ok(Some(pair)) => pair,
            Ok(None) => return V2Outcome::Unsupported,
            Err(e) => return V2Outcome::Failed(e.to_string()),
        };
        match bindings.call_handle_request(&mut store, req).await {
            Ok(Ok(response)) => V2Outcome::Ok(response),
            Ok(Err(message)) => V2Outcome::Failed(message),
            Err(trap) => {
                let msg = Self::trap_reason(&trap);
                self.on_trap(plugin_id, &msg).await;
                V2Outcome::Failed(msg)
            }
        }
    }

    /// Renders one plugin block instance.
    pub async fn call_render_block(
        &self,
        plugin_id: i64,
        kind: &str,
        attrs_json: &str,
        env: &HostEnv,
    ) -> V2Outcome<String> {
        let (bindings, mut store) = match self.begin_call_v2(plugin_id, env).await {
            Ok(Some(pair)) => pair,
            Ok(None) => return V2Outcome::Unsupported,
            Err(e) => return V2Outcome::Failed(e.to_string()),
        };
        store.data_mut().ai_window = AiWindow::Render;
        match bindings
            .call_render_block(&mut store, kind, attrs_json)
            .await
        {
            Ok(Ok(html)) => V2Outcome::Ok(html),
            Ok(Err(message)) => V2Outcome::Failed(message),
            Err(trap) => {
                let msg = Self::trap_reason(&trap);
                self.on_trap(plugin_id, &msg).await;
                V2Outcome::Failed(msg)
            }
        }
    }

    /// Runs one declared scheduled task.
    pub async fn call_run_task(&self, plugin_id: i64, name: &str, env: &HostEnv) -> V2Outcome<()> {
        let (bindings, mut store) = match self.begin_call_v2(plugin_id, env).await {
            Ok(Some(pair)) => pair,
            Ok(None) => return V2Outcome::Unsupported,
            Err(e) => return V2Outcome::Failed(e.to_string()),
        };
        match bindings.call_run_task(&mut store, name).await {
            Ok(Ok(())) => V2Outcome::Ok(()),
            Ok(Err(message)) => V2Outcome::Failed(message),
            Err(trap) => {
                let msg = Self::trap_reason(&trap);
                self.on_trap(plugin_id, &msg).await;
                V2Outcome::Failed(msg)
            }
        }
    }

    /// Asks a plugin for one of its optional declarations.
    ///
    /// One method rather than four near-identical ones: the only thing
    /// that varies is which export to call.
    pub async fn call_declaration(
        &self,
        plugin_id: i64,
        which: Declaration,
        env: &HostEnv,
    ) -> V2Outcome<String> {
        let (bindings, mut store) = match self.begin_call_v2(plugin_id, env).await {
            Ok(Some(pair)) => pair,
            Ok(None) => return V2Outcome::Unsupported,
            Err(e) => return V2Outcome::Failed(e.to_string()),
        };
        let called = match which {
            Declaration::Schedule => bindings.call_register_schedule(&mut store).await,
            Declaration::Admin => bindings.call_register_admin(&mut store).await,
            Declaration::Assets => bindings.call_register_assets(&mut store).await,
            Declaration::PostTypes => bindings.call_register_post_types(&mut store).await,
            Declaration::Taxonomies => bindings.call_register_taxonomies(&mut store).await,
        };
        match called {
            Ok(json) => V2Outcome::Ok(json),
            Err(trap) => {
                let msg = format!("{}: {}", which.as_str(), Self::trap_reason(&trap));
                self.on_trap(plugin_id, &msg).await;
                V2Outcome::Failed(msg)
            }
        }
    }
}

impl WasmtimeHost {
    /// Runs one string-keyed filter point.
    ///
    /// The payload comes back unchanged when the plugin does not handle the
    /// point, so a caller can chain plugins without knowing which of them
    /// cares about what.
    pub async fn call_filter_at(
        &self,
        plugin_id: i64,
        point: &str,
        payload: &str,
        env: &HostEnv,
    ) -> V2Outcome<String> {
        let (bindings, mut store) = match self.begin_call_v2(plugin_id, env).await {
            Ok(Some(pair)) => pair,
            Ok(None) => return V2Outcome::Unsupported,
            Err(e) => return V2Outcome::Failed(e.to_string()),
        };
        // Filter points are titles, excerpts and sections of a page being
        // rendered for someone.
        store.data_mut().ai_window = AiWindow::Render;
        match bindings.call_filter_at(&mut store, point, payload).await {
            Ok(out) => V2Outcome::Ok(out),
            Err(trap) => {
                let msg = format!("filter-at {point}: {}", Self::trap_reason(&trap));
                self.on_trap(plugin_id, &msg).await;
                V2Outcome::Failed(msg)
            }
        }
    }

    /// Fires one string-keyed event.
    pub async fn call_on_event(
        &self,
        plugin_id: i64,
        name: &str,
        payload_json: &str,
        env: &HostEnv,
    ) -> V2Outcome<()> {
        let (bindings, mut store) = match self.begin_call_v2(plugin_id, env).await {
            Ok(Some(pair)) => pair,
            Ok(None) => return V2Outcome::Unsupported,
            Err(e) => return V2Outcome::Failed(e.to_string()),
        };
        match bindings.call_on_event(&mut store, name, payload_json).await {
            Ok(Ok(())) => V2Outcome::Ok(()),
            Ok(Err(message)) => V2Outcome::Failed(message),
            Err(trap) => {
                let msg = format!("on-event {name}: {}", Self::trap_reason(&trap));
                self.on_trap(plugin_id, &msg).await;
                V2Outcome::Failed(msg)
            }
        }
    }
}
