//! Application state: the composition root's shared handle.

use std::sync::Arc;

use rand::RngCore;
use sqlx::PgPool;
use tokio::sync::Notify;
use vyasa_common::VyasaConfig;
use vyasa_core::comment::CommentService;
use vyasa_core::media::{LocalFsBackend, MediaService, StorageRouter};
use vyasa_core::post::revisions::RevisionService;
use vyasa_core::post::PostService;
use vyasa_core::taxonomy::TermService;
use vyasa_core::user::{AuthService, RolesService, UsersService};
use vyasa_db::repo::{
    ApiKeysRepo, CommentsRepo, MediaRepo, OptionsRepo, PostsRepo, RevisionsRepo, SessionsRepo,
    TermsRepo, UsersRepo,
};

/// Shared state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    /// Bounded, short-lived CSS/JS for authenticated unsaved theme previews.
    pub preview_assets: moka::sync::Cache<String, crate::theme_assets::Asset>,
    /// Loaded configuration.
    pub config: VyasaConfig,
    /// Database pool (health checks and later services).
    #[allow(dead_code)]
    pub pool: PgPool,
    /// Authentication service.
    pub auth: AuthService,
    /// User management service.
    pub users: UsersService,
    /// Custom role management.
    pub roles: RolesService,
    /// Public registration: accounts, confirmation links (phase 98).
    pub registration: vyasa_core::user::RegistrationService,
    /// API key repository (headless auth).
    pub api_keys: ApiKeysRepo,
    /// Post service.
    pub posts: PostService,
    /// Revision service.
    pub revisions: RevisionService,
    /// Term service.
    pub terms: TermService,
    /// Comment service.
    pub comments: CommentService,
    /// Media service.
    pub media: MediaService,
    /// Where media bytes go: local disk plus the object store active now
    /// (from the environment, or from the settings saved in the admin).
    pub media_storage: Arc<StorageRouter>,
    /// Options repository (public option reads for GraphQL `option`).
    pub options: OptionsRepo,
    /// Theme storage (install/activate/rollback).
    pub themes: vyasa_db::repo::ThemesRepo,
    /// Navigation menu service (phase 25).
    pub menus: vyasa_db::repo::MenusRepo,
    /// Audience data: view rollups, form submissions, subscribers
    /// (phase 61).
    pub audience: vyasa_db::repo::AudienceRepo,
    /// Entry languages / translation groups (phase 62).
    pub translations: vyasa_db::repo::TranslationsRepo,
    /// Typed site options (phase 27).
    pub options_service: vyasa_core::options::OptionsService,
    /// Wasmtime host (phase 31) + registries (phase 33).
    pub plugin_host: std::sync::Arc<vyasa_plugins::host::WasmtimeHost>,
    pub hook_registry: vyasa_plugins::hooks::HookRegistry,
    pub plugin_blocks: vyasa_plugins::blocks::PluginBlockRegistry,
    /// Plugins repository (lifecycle REST).
    pub plugins_repo: vyasa_db::repo::PluginsRepo,
    /// Routes, assets, admin forms, post types and tasks plugins declared.
    pub plugin_surface: std::sync::Arc<crate::plugin_surface::PluginSurface>,
    /// Outbound client for embed previews: short timeout, no redirects,
    /// only ever pointed at a provider's oEmbed endpoint.
    pub embed_client: reqwest::Client,
    /// Outbound client for link checks: follows a few redirects, gives up
    /// after eight seconds.
    pub site_url_client: reqwest::Client,
    /// The first-run setup token, while no administrator exists.
    pub setup_token: Arc<std::sync::Mutex<Option<String>>>,
    /// A nonce minted at boot; the setup wizard's site-address check looks
    /// for it to prove an address reaches this server.
    pub instance_nonce: String,
    /// Bumped whenever the plugin surface changes.
    ///
    /// Part of every cached page's key, so enabling or disabling a plugin
    /// makes the old entries unreachable instead of purging the whole
    /// cache: a page with no plugin output in it stays warm.
    pub surface_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// One circuit breaker per model, shared by every request.
    ///
    /// Keyed `"{provider}/{model}"`. A breaker only means something if it
    /// outlives the request that trips it.
    /// The active theme, built once and shared by every public request.
    pub theme_bundle: crate::public::routes::BundleCache,
    pub breakers: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, vyasa_ai::CircuitBreaker>>,
    >,
    /// Capability broker gating every plugin host call.
    pub broker: std::sync::Arc<vyasa_plugins::broker::Broker>,
    /// Full-text search index manager (None when index dir unavailable).
    #[allow(dead_code)] // consumed by the tantivy subscriber spawned in main
    pub search_index: Option<std::sync::Arc<vyasa_search::IndexManager>>,
    /// Data seam for dynamic blocks / feeds. A trait object so the
    /// studio agent's eyes can render previews with an overlay (staged
    /// menus) in front of the same live queries.
    pub content_queries: std::sync::Arc<dyn vyasa_themes::ContentQueries>,
    /// Public-site render cache (phase 23); `None` until routes land.
    pub render_cache: Option<std::sync::Arc<crate::render_cache::RenderCache>>,
    /// Per-IP rate limiter shared across requests.
    pub rate_limiter: std::sync::Arc<crate::middleware::rate_limit::RateLimiter>,
    /// Progressive lockout tracker for repeated failed logins.
    pub lockout: std::sync::Arc<crate::middleware::lockout::Lockout>,
    /// Edge-cache purger; a no-op when no CDN is configured.
    pub cdn: std::sync::Arc<dyn crate::cdn::Purger>,
    /// Prometheus metric registry.
    pub metrics: std::sync::Arc<crate::metrics::Metrics>,
    /// Secret for HMAC-signing private-post and preview tokens.
    pub private_token_secret: Vec<u8>,
    /// The AI model registry: providers, keys, models per kind.
    pub ai_models: vyasa_core::ai_models::AiModelsService,
    /// Notifier to nudge the scheduled publisher.
    pub publisher_notify: Arc<Notify>,
}

/// Local disk, with object storage active when the environment configures
/// it. Settings saved in the admin are applied after construction (see
/// `media_storage::apply_saved`), so the environment always wins.
///
/// Local disk is right for one server and wrong for several: each node
/// would hold different files, so an upload that landed on one would 404
/// from the other.
fn storage_router(config: &VyasaConfig) -> Arc<StorageRouter> {
    let object = config
        .storage
        .as_ref()
        .and_then(crate::media_s3::S3Backend::new)
        .map(|s3| {
            tracing::info!("media: object storage (environment)");
            Arc::new(s3) as Arc<dyn vyasa_core::media::StorageBackend>
        });
    Arc::new(StorageRouter::new(
        LocalFsBackend::new(config.media_dir.clone()),
        object,
    ))
}

/// Outbound client for embed previews: short timeout, no redirects, only
/// ever pointed at a provider's oEmbed endpoint.
fn embed_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Vyasa/1.0 (+embed preview)")
        .build()
        .unwrap_or_default()
}

/// Outbound client for testing a site address: guarded, because anyone
/// holding the setup token or the options capability chooses the address.
fn site_url_client() -> reqwest::Client {
    crate::net_guard::guarded_builder(5)
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("Mozilla/5.0 (compatible; Vyasa link check)")
        .build()
        .unwrap_or_default()
}

impl AppState {
    /// Wires all services over `pool` and `config`.
    #[must_use]
    // One field per service, wired once: the length is the number of
    // services, not the number of ideas.
    #[allow(clippy::too_many_lines)]
    pub fn new(config: VyasaConfig, pool: PgPool) -> Self {
        let index_dir = config.index_dir.clone();
        let users_repo = UsersRepo::new(pool.clone());
        let sessions = SessionsRepo::new(pool.clone());
        let api_keys = ApiKeysRepo::new(pool.clone());
        let auth = AuthService::new(users_repo.clone(), sessions);
        let roles = RolesService::new(
            vyasa_db::repo::RolesRepo::new(pool.clone()),
            users_repo.clone(),
        );
        let registration_users = users_repo.clone();
        let users = UsersService::new(users_repo);
        let posts = PostService::new(PostsRepo::new(pool.clone()));
        let revisions = RevisionService::new(
            RevisionsRepo::new(pool.clone()),
            PostsRepo::new(pool.clone()),
        );
        let terms = TermService::new(TermsRepo::new(pool.clone()), PostsRepo::new(pool.clone()));
        let comments = CommentService::new(
            CommentsRepo::new(pool.clone()),
            OptionsRepo::new(pool.clone()),
            PostsRepo::new(pool.clone()),
        );
        let media_storage = storage_router(&config);
        let media = MediaService::new(
            MediaRepo::new(pool.clone()),
            media_storage.clone(),
            pool.clone(),
        );
        let options = OptionsRepo::new(pool.clone());
        let menus = vyasa_db::repo::MenusRepo::new(pool.clone());
        let options_service = vyasa_core::options::OptionsService::new(options.clone());
        let registration = vyasa_core::user::RegistrationService::new(
            registration_users,
            vyasa_db::repo::TokensRepo::new(pool.clone()),
            vyasa_db::repo::RolesRepo::new(pool.clone()),
            options_service.clone(),
        );
        let comments_repo = vyasa_db::repo::CommentsRepo::new(pool.clone());
        let posts_repo = vyasa_db::repo::PostsRepo::new(pool.clone());
        let terms_repo = vyasa_db::repo::TermsRepo::new(pool.clone());
        let plugins_repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
        let plugins_repo2 = plugins_repo.clone();
        let plugins_repo_shared = plugins_repo.clone();
        let broker_pool = pool.clone();
        let plugin_host = match vyasa_plugins::host::WasmtimeHost::new(plugins_repo) {
            Ok(h) => std::sync::Arc::new(h),
            Err(e) => {
                tracing::error!("wasmtime host init failed: {e}; retrying fresh engine");
                let fresh = vyasa_plugins::host::WasmtimeHost::with_limits(
                    vyasa_db::repo::PluginsRepo::new(pool.clone()),
                    vyasa_plugins::host::HostLimits::default(),
                );
                match fresh {
                    Ok(h) => std::sync::Arc::new(h),
                    Err(fresh_err) => {
                        panic!("wasmtime engine unusable: {e}; {fresh_err}")
                    }
                }
            }
        };
        let (plugin_surface, content_queries) =
            content_wiring(&pool, posts_repo, terms_repo, comments_repo, &menus);
        // Render cache. Enabled now that cache_invalidator subscribes to the
        // domain event bus and purges on every content change; it was left
        // disabled while nothing did, because a stale page is worse than a
        // re-render. 32 MB, 5-minute TTL.
        let render_cache = Some(std::sync::Arc::new(crate::render_cache::RenderCache::new(
            32 * 1024 * 1024,
            300,
        )));
        let secret = token_secret(&config);
        let ai_models = ai_registry(&config, &pool);
        Self {
            cdn: crate::cdn::from_config(&config),
            config,
            audience: vyasa_db::repo::AudienceRepo::new(pool.clone()),
            translations: vyasa_db::repo::TranslationsRepo::new(pool.clone()),
            themes: vyasa_db::repo::ThemesRepo::new(pool.clone()),
            pool,
            auth,
            users,
            roles,
            registration,
            api_keys,
            posts,
            revisions,
            terms,
            comments,
            media,
            media_storage,
            options,
            menus,
            options_service,
            plugin_host: std::sync::Arc::clone(&plugin_host),
            hook_registry: vyasa_plugins::hooks::HookRegistry::new(),
            plugin_blocks: vyasa_plugins::blocks::PluginBlockRegistry::new(),
            plugins_repo: plugins_repo_shared,
            plugin_surface,
            embed_client: embed_client(),
            site_url_client: site_url_client(),
            setup_token: Arc::new(std::sync::Mutex::new(None)),
            instance_nonce: hex::encode({
                use rand::RngCore as _;
                let mut b = [0u8; 12];
                rand::thread_rng().fill_bytes(&mut b);
                b
            }),
            surface_epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            theme_bundle: crate::public::routes::BundleCache::default(),
            breakers: std::sync::Arc::default(),
            broker: std::sync::Arc::new(vyasa_plugins::broker::Broker::new(
                plugins_repo2,
                broker_pool,
            )),
            preview_assets: moka::sync::Cache::builder()
                .max_capacity(128)
                .time_to_idle(std::time::Duration::from_secs(600))
                .build(),
            search_index: vyasa_search::IndexManager::open(&index_dir)
                .ok()
                .map(std::sync::Arc::new),
            content_queries,
            render_cache,
            rate_limiter: std::sync::Arc::new(crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::DEFAULT_LIMITS.to_vec(),
            )),
            lockout: std::sync::Arc::new(crate::middleware::lockout::Lockout::new()),
            metrics: std::sync::Arc::new(crate::metrics::Metrics::new()),
            private_token_secret: secret,
            ai_models,
            publisher_notify: Arc::new(Notify::new()),
        }
    }
}

/// Token-signing secret: the configured `VYASA_SECRET_KEY` when there is
/// one (preview links survive restarts), else 32 random bytes that live
/// only in memory.
fn token_secret(config: &VyasaConfig) -> Vec<u8> {
    if let Some(key) = &config.secret_key {
        return key.expose().as_bytes().to_vec();
    }
    let mut secret = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    secret
}

/// The AI model registry, sealing stored keys under the same server secret.
fn ai_registry(config: &VyasaConfig, pool: &PgPool) -> vyasa_core::ai_models::AiModelsService {
    vyasa_core::ai_models::AiModelsService::new(
        vyasa_db::repo::AiModelsRepo::new(pool.clone()),
        vyasa_core::ai_models::KeyVault::new(
            config.secret_key.as_ref().map(|k| k.expose().as_bytes()),
        ),
    )
}

/// The theme engine's view of site content, over the repositories — and
/// the plugin surface both it and the router consult, created here so the
/// two share one instance.
fn content_wiring(
    pool: &PgPool,
    posts: PostsRepo,
    terms: TermsRepo,
    comments: vyasa_db::repo::CommentsRepo,
    menus: &vyasa_db::repo::MenusRepo,
) -> (
    std::sync::Arc<crate::plugin_surface::PluginSurface>,
    std::sync::Arc<crate::content_queries::ApiContentQueries>,
) {
    let plugin_surface = std::sync::Arc::new(crate::plugin_surface::PluginSurface::new());
    let queries = std::sync::Arc::new(crate::content_queries::ApiContentQueries::new(
        posts,
        terms,
        comments,
        menus.clone(),
        vyasa_db::repo::AiDataRepo::new(pool.clone()),
        OptionsRepo::new(pool.clone()),
        std::sync::Arc::clone(&plugin_surface),
        pool.clone(),
    ));
    (plugin_surface, queries)
}
