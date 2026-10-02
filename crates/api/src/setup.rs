//! First-run setup: the token that guards a fresh install, the checks, and
//! the eight steps. Each step is a plain function over `AppState` so the
//! browser wizard and `vyasa setup --answers` run the same code.

use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::health::{Check, Status};

use crate::state::AppState;

/// The option that records how far setup got; `"done"` once finished.
pub const PROGRESS_KEY: &str = "setup_progress";

/// A fresh token, unless the environment fixed one (`VYASA_SETUP_TOKEN`,
/// which scripted installs and tests use).
#[must_use]
pub fn mint_token() -> String {
    use rand::RngCore as _;
    if let Ok(fixed) = std::env::var("VYASA_SETUP_TOKEN") {
        if fixed.len() >= 12 {
            return fixed;
        }
    }
    let mut bytes = [0u8; 18];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("stp_{}", hex::encode(bytes))
}

/// Whether the site still needs its first administrator.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn needs_admin(state: &AppState) -> Result<bool, AppError> {
    Ok(vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .count()
        .await?
        == 0)
}

/// Where setup stands: `None` never started, a step name, or `"done"`.
pub async fn progress(state: &AppState) -> Option<String> {
    state
        .options
        .get(PROGRESS_KEY)
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|s| !s.is_empty())
}

async fn set_progress(state: &AppState, step: &str) -> Result<(), AppError> {
    state
        .options_service
        .put(PROGRESS_KEY, serde_json::Value::String(step.to_owned()))
        .await
}

/// What the Welcome screen shows: the environment, tested.
pub async fn checks(state: &AppState, arrived_over_https: bool) -> Vec<Check> {
    let mut out = vec![vyasa_core::health::database(&state.pool).await];
    out.push(Check::new(
        "secret_key",
        if state.config.secret_key.is_some() {
            Status::Ok
        } else {
            Status::Warn
        },
        if state.config.secret_key.is_some() {
            "VYASA_SECRET_KEY is set"
        } else {
            "VYASA_SECRET_KEY is not set: sessions and previews reset on every restart"
        },
    ));
    let s3 = state
        .config
        .storage
        .as_ref()
        .is_some_and(vyasa_common::StorageConfig::is_s3);
    // With a bucket, the bucket probe below is the storage check; the local
    // directory is only a cache then.
    if !s3 {
        out.push(vyasa_core::health::storage(&state.config.media_dir).await);
    }
    for (name, dir) in [
        ("index_dir", &state.config.index_dir),
        ("registry_dir", &state.config.registry_dir),
    ] {
        let writable = tokio::fs::create_dir_all(dir).await.is_ok()
            && tokio::fs::write(dir.join(".vyasa-probe"), b"ok")
                .await
                .is_ok();
        let _ = tokio::fs::remove_file(dir.join(".vyasa-probe")).await;
        out.push(Check::new(
            name,
            if writable { Status::Ok } else { Status::Warn },
            format!(
                "{} is {}",
                dir.display(),
                if writable { "writable" } else { "not writable" }
            ),
        ));
    }
    out.push(if s3 {
        // A real round trip, so a wrong key shows here and not at the first upload.
        let probe = format!("setup-probe/{}", vyasa_common::next_id_i64());
        let storage = state.media.storage();
        match storage.put(&probe, b"ok").await {
            Ok(()) => {
                let _ = storage.delete(&probe).await;
                Check::new(
                    "object_storage",
                    Status::Ok,
                    "bucket answers; a probe object was written and removed",
                )
            }
            Err(e) => Check::new(
                "object_storage",
                Status::Warn,
                format!("bucket refused a probe write: {e}"),
            ),
        }
    } else {
        Check::new(
            "object_storage",
            Status::Ok,
            "local disk; right for one server",
        )
    });
    out.push(match crate::mail::effective(state).await {
        Some(smtp) => Check::new(
            "smtp",
            Status::Ok,
            format!("{}:{} from {}", smtp.host, smtp.port, smtp.from),
        ),
        None => Check::new(
            "smtp",
            Status::Warn,
            "no SMTP relay: mail is logged, not sent",
        ),
    });
    out.push(Check::new(
        "https",
        if arrived_over_https {
            Status::Ok
        } else {
            Status::Warn
        },
        if arrived_over_https {
            "you arrived over HTTPS"
        } else {
            "you arrived over HTTP; set the site address to its https form"
        },
    ));
    out
}

/// Step 1.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct AccountInput {
    pub email: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub password: String,
    /// IANA zone from the browser, e.g. `Europe/Stockholm`.
    pub timezone: Option<String>,
}

/// Creates the first administrator. Refused once any user exists.
///
/// # Errors
/// [`AppError::Conflict`] when a user already exists; validation errors
/// from the user service.
pub async fn account(
    state: &AppState,
    input: AccountInput,
) -> Result<vyasa_db::models::UserRow, AppError> {
    if !needs_admin(state).await? {
        return Err(AppError::conflict(
            "an administrator already exists; sign in instead",
        ));
    }
    if input.password.chars().count() < 12 {
        return Err(AppError::validation(
            "use a password of twelve characters or more",
        ));
    }
    let user = state
        .users
        .create(vyasa_core::user::CreateUser {
            email: input.email.trim().to_owned(),
            username: input
                .username
                .map(|u| u.trim().to_owned())
                .filter(|u| !u.is_empty()),
            display_name: input
                .display_name
                .map(|d| d.trim().to_owned())
                .filter(|d| !d.is_empty()),
            password: Some(input.password),
            role: vyasa_db::models::Role::Admin,
        })
        .await?;
    if let Some(tz) = input.timezone.filter(|t| !t.trim().is_empty()) {
        state
            .options_service
            .put("timezone", serde_json::Value::String(tz.trim().to_owned()))
            .await?;
    }
    set_progress(state, "site").await?;
    Ok(user)
}

/// Step 2.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct SiteInput {
    pub site_title: String,
    pub site_tagline: Option<String>,
    pub site_url: String,
    pub site_language: Option<String>,
    pub date_format: Option<String>,
    pub timezone: Option<String>,
}

/// # Errors
/// Validation from the options service.
pub async fn site(state: &AppState, input: SiteInput) -> Result<(), AppError> {
    let put = |k: &'static str, v: String| async move {
        state
            .options_service
            .put(k, serde_json::Value::String(v))
            .await
    };
    put("site_title", input.site_title.trim().to_owned()).await?;
    put(
        "site_tagline",
        input.site_tagline.unwrap_or_default().trim().to_owned(),
    )
    .await?;
    put(
        "site_url",
        input.site_url.trim().trim_end_matches('/').to_owned(),
    )
    .await?;
    if let Some(v) = input.site_language.filter(|v| !v.trim().is_empty()) {
        put("site_language", v.trim().to_owned()).await?;
    }
    if let Some(v) = input.date_format.filter(|v| !v.trim().is_empty()) {
        put("date_format", v.trim().to_owned()).await?;
    }
    if let Some(v) = input.timezone.filter(|v| !v.trim().is_empty()) {
        put("timezone", v.trim().to_owned()).await?;
    }
    set_progress(state, "content").await
}

/// Whether `site_url` reaches this very server: the status endpoint answers
/// with the instance nonce minted at boot.
pub async fn site_url_reaches_us(state: &AppState, site_url: &str) -> bool {
    let target = format!("{}/api/v1/setup/status", site_url.trim_end_matches('/'));
    let Ok(response) = state.link_client.get(&target).send().await else {
        return false;
    };
    let Ok(body) = response.json::<serde_json::Value>().await else {
        return false;
    };
    body.get("instance").and_then(serde_json::Value::as_str) == Some(state.instance_nonce.as_str())
}

/// Step 3.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct ContentInput {
    /// Starter to activate: `blog`, `docs`, `portfolio`, `storefront-lite`.
    pub theme: Option<String>,
    #[serde(default)]
    pub sample_content: bool,
    pub permalink_pattern: Option<String>,
    pub posts_per_page: Option<u32>,
}

/// # Errors
/// Unknown theme name; validation from the options and post services.
pub async fn content(
    state: &AppState,
    author_id: i64,
    input: ContentInput,
) -> Result<(), AppError> {
    if let Some(name) = input.theme.filter(|n| !n.trim().is_empty()) {
        let name = name.trim();
        let version = state.themes.latest_version(name).await?;
        if version == 0 {
            return Err(AppError::validation(format!(
                "no theme named \"{name}\" is installed"
            )));
        }
        state.themes.set_active(name, version).await?;
        crate::rest::themes::after_activate(state, name, version);
    }
    if let Some(p) = input.permalink_pattern.filter(|p| !p.trim().is_empty()) {
        state
            .options_service
            .put(
                "permalink_pattern",
                serde_json::Value::String(p.trim().to_owned()),
            )
            .await?;
    }
    if let Some(n) = input.posts_per_page {
        state
            .options_service
            .put("posts_per_page", serde_json::Value::from(n.clamp(1, 100)))
            .await?;
    }
    if input.sample_content {
        sample_content(state, author_id).await?;
    }
    set_progress(state, "delivery").await
}

/// A welcome post and an About page, so the site is not blank. Skipped
/// when either slug already exists.
async fn sample_content(state: &AppState, author_id: i64) -> Result<(), AppError> {
    use vyasa_core::block::{Block, BlockDocument, BlockKind};
    use vyasa_core::post::CreatePost;
    use vyasa_db::content_models::{PostStatus, PostType};
    let para = |text: &str| Block {
        kind: BlockKind::Paragraph,
        plugin_kind: None,
        attrs: serde_json::json!({ "text": text }),
        children: Vec::new(),
    };
    let heading = |text: &str| Block {
        kind: BlockKind::Heading,
        plugin_kind: None,
        attrs: serde_json::json!({ "level": 2, "text": text }),
        children: Vec::new(),
    };
    let entries = [
        (
            PostType::Post,
            "Welcome to your new site",
            "welcome",
            BlockDocument::new(vec![
                para("This is your first post. It was written by the setup wizard so the site is not empty on day one; edit it, or delete it from Posts."),
                heading("What to try first"),
                para("Open this post in the editor and press <code>/</code> to see every block. Drop a photo onto the page to add it. The SEO card in the sidebar tells you what a search engine will make of it."),
            ]),
            Some("Your first post, written by the setup wizard."),
        ),
        (
            PostType::Page,
            "About",
            "about",
            BlockDocument::new(vec![para("A page about you or your site. Pages sit outside the posting timeline; this one is linked from the main menu.")]),
            None,
        ),
    ];
    for (post_type, title, slug, content, excerpt) in entries {
        if state.posts.get_by_slug(post_type, slug).await.is_ok() {
            continue;
        }
        state
            .posts
            .create(CreatePost {
                post_type,
                status: PostStatus::Published,
                title: title.to_owned(),
                slug: Some(slug.to_owned()),
                content,
                excerpt: excerpt.map(str::to_owned),
                author_id,
                parent_id: None,
                scheduled_for: None,
                password: None,
                term_ids: None,
                layout: None,
            })
            .await?;
    }
    // The About page joins the main menu the first boot created.
    let menus = vyasa_core::menu::MenuService::new(state.menus.clone());
    if let Ok(existing) = menus.list().await {
        if let Some(main) = existing.iter().find(|m| m.slug == "main") {
            let rows = state.menus.items(main.id).await.unwrap_or_default();
            let mut items = vyasa_core::menu::MenuDraft::from_rows(main, &rows).items;
            if !items.iter().any(|i| i.url == "/about") {
                items.push(vyasa_core::menu::MenuDraftItem {
                    label: "About".into(),
                    url: "/about".into(),
                    children: Vec::new(),
                });
                let _ = menus
                    .apply_draft(&vyasa_core::menu::MenuDraft {
                        slug: "main".into(),
                        name: main.name.clone(),
                        location: main.location.clone(),
                        items,
                    })
                    .await;
            }
        }
    }
    Ok(())
}

/// Step 4.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct DeliveryInput {
    pub edge_cache_seconds: Option<u32>,
    pub media_storage_cap_mb: Option<u32>,
}

/// # Errors
/// Validation from the options service.
pub async fn delivery(state: &AppState, input: DeliveryInput) -> Result<(), AppError> {
    if let Some(s) = input.edge_cache_seconds {
        state
            .options_service
            .put("edge_cache_seconds", serde_json::Value::from(s))
            .await?;
    }
    if let Some(mb) = input.media_storage_cap_mb {
        state
            .options_service
            .put("media_storage_cap_mb", serde_json::Value::from(mb))
            .await?;
    }
    set_progress(state, "mail").await
}

/// Step 5.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct MailInput {
    /// `auto_approve`, `require_first`, or `require_all`.
    pub comment_moderation: Option<String>,
    #[serde(default)]
    pub newsletter_enabled: bool,
    /// A relay to save first, so the newsletter can be switched on in
    /// the same step.
    pub smtp: Option<crate::mail::MailInput>,
}

/// # Errors
/// Validation from the options service.
pub async fn mail(state: &AppState, input: MailInput) -> Result<(), AppError> {
    if let Some(m) = input.comment_moderation.filter(|m| !m.trim().is_empty()) {
        state
            .options_service
            .put(
                "comment_moderation",
                serde_json::Value::String(m.trim().to_owned()),
            )
            .await?;
    }
    if let Some(smtp) = input.smtp {
        crate::mail::save(state, smtp).await?;
    }
    let can_send = crate::mail::effective(state).await.is_some();
    state
        .options_service
        .put(
            "newsletter_enabled",
            serde_json::Value::Bool(input.newsletter_enabled && can_send),
        )
        .await?;
    set_progress(state, "assistants").await
}

/// Sends one test message through the configured relay, right now.
///
/// # Errors
/// [`AppError::Validation`] with no relay; the relay's reason on failure.
pub async fn mail_test(state: &AppState, to: &str) -> Result<(), AppError> {
    crate::mail::send_test(state, to).await
}

/// Step 6.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct AssistantsInput {
    /// `anthropic`, `openai` or `openrouter`.
    pub provider: Option<String>,
    pub api_key: Option<String>,
    /// Model id for the text kind; a sensible default per provider otherwise.
    pub text_model: Option<String>,
    pub monthly_cap_usd: Option<f64>,
    #[serde(default = "default_true")]
    pub alt_text: bool,
    #[serde(default)]
    pub comment_screening: bool,
    #[serde(default)]
    pub related_posts: bool,
    pub audience: Option<String>,
}
fn default_true() -> bool {
    true
}

/// # Errors
/// Unknown provider; validation from the model registry and options.
pub async fn assistants(state: &AppState, input: AssistantsInput) -> Result<(), AppError> {
    use vyasa_core::ai_models::{ModelKind, ProviderId};
    if let (Some(provider), Some(key)) = (input.provider.as_deref(), input.api_key.as_deref()) {
        let provider = ProviderId::parse(provider)?;
        state
            .ai_models
            .save_provider(provider, Some(key.trim()), "", true)
            .await?;
        let model = input
            .text_model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .or_else(|| provider.recommended(ModelKind::Text).map(|r| r.model))
            .ok_or_else(|| {
                AppError::validation("that provider has no text model to start with; name one")
            })?;
        for kind in [ModelKind::Text, ModelKind::Vision] {
            if !provider.supports(kind) {
                continue;
            }
            let rec = provider.recommended(kind);
            let row = state
                .ai_models
                .register(vyasa_core::ai_models::NewModelSpec {
                    provider,
                    kind,
                    model,
                    label: rec.filter(|r| r.model == model).map_or(model, |r| r.label),
                    settings: serde_json::json!({}),
                    input_cost_per_mtok: rec.and_then(|r| r.input_cost_per_mtok),
                    output_cost_per_mtok: rec.and_then(|r| r.output_cost_per_mtok),
                })
                .await?;
            let _ = state.ai_models.set_default(row.id).await;
        }
    }
    if let Some(cap) = input.monthly_cap_usd {
        state
            .options_service
            .put("ai_month_cap_usd", serde_json::Value::from(cap.max(0.0)))
            .await?;
    }
    for (key, on) in [
        ("ai_alt_text", input.alt_text),
        ("ai_related_posts", input.related_posts),
        ("ai_embeddings", input.related_posts),
    ] {
        state
            .options_service
            .put(key, serde_json::Value::Bool(on))
            .await?;
    }
    state
        .options_service
        .put(
            "ai_comment_screening",
            serde_json::Value::String(if input.comment_screening {
                "flag".into()
            } else {
                "off".into()
            }),
        )
        .await?;
    if let Some(audience) = input.audience.filter(|a| !a.trim().is_empty()) {
        state
            .options_service
            .put(
                "brand_kit",
                serde_json::json!({ "audience": audience.trim() }),
            )
            .await?;
    }
    set_progress(state, "updates").await
}

/// Step 7.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct UpdatesInput {
    pub update_channel_url: Option<String>,
    pub registry_url: Option<String>,
    pub trusted_keys: Option<Vec<String>>,
}

/// # Errors
/// Validation from the options service.
pub async fn updates(state: &AppState, input: UpdatesInput) -> Result<(), AppError> {
    if let Some(u) = input.update_channel_url.filter(|u| !u.trim().is_empty()) {
        state
            .options_service
            .put(
                "update_channel_url",
                serde_json::Value::String(u.trim().to_owned()),
            )
            .await?;
    }
    if let Some(u) = input.registry_url.filter(|u| !u.trim().is_empty()) {
        state
            .options_service
            .put(
                "registry_url",
                serde_json::Value::String(u.trim().to_owned()),
            )
            .await?;
    }
    if let Some(keys) = input.trusted_keys {
        let keys: Vec<serde_json::Value> = keys
            .into_iter()
            .map(|k| serde_json::Value::String(k.trim().to_owned()))
            .filter(|k| k.as_str().is_some_and(|s| !s.is_empty()))
            .collect();
        state
            .options_service
            .put(
                "update_trusted_keys",
                serde_json::Value::Array(keys.clone()),
            )
            .await?;
        state
            .options_service
            .put("registry_trusted_keys", serde_json::Value::Array(keys))
            .await?;
    }
    set_progress(state, "finish").await
}

/// A fresh ed25519 keypair for signing this site's own plugin packages.
/// The private half is returned once and stored nowhere.
#[derive(Serialize, utoipa::ToSchema)]
pub struct Keypair {
    pub public_key: String,
    pub private_key: String,
}

#[must_use]
pub fn keypair() -> Keypair {
    use ed25519_dalek::SigningKey;
    use rand::RngCore as _;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    Keypair {
        public_key: hex::encode(signing.verifying_key().to_bytes()),
        private_key: hex::encode(signing.to_bytes()),
    }
}

/// Step 8: marks setup done and rebuilds the search index.
///
/// # Errors
/// [`AppError::Db`] on failure to record progress.
pub async fn finish(state: &AppState) -> Result<usize, AppError> {
    set_progress(state, "done").await?;
    let indexed = crate::search_indexer::reindex_all(state).await.unwrap_or(0);
    Ok(indexed)
}

/// Everything the wizard wrote, as TOML, for `vyasa setup --print`.
pub async fn print_answers(state: &AppState) -> String {
    use std::fmt::Write as _;
    let mut out =
        String::from("# Vyasa setup answers; feed back with `vyasa setup --answers this.toml`\n");
    for key in [
        "site_title",
        "site_tagline",
        "site_url",
        "site_language",
        "date_format",
        "timezone",
        "permalink_pattern",
        "posts_per_page",
        "edge_cache_seconds",
        "media_storage_cap_mb",
        "comment_moderation",
        "newsletter_enabled",
        "ai_month_cap_usd",
        "ai_alt_text",
        "ai_comment_screening",
        "ai_related_posts",
        "update_channel_url",
        "registry_url",
    ] {
        if let Ok(v) = state.options.get(key).await {
            if !v.is_null() {
                let _ = writeln!(out, "{key} = {}", toml_value(&v));
            }
        }
    }
    out
}

fn toml_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    }
}

/// The answers file `vyasa setup --answers` reads: every step's input,
/// each optional so a file can cover only what it wants to.
#[derive(Deserialize, Default)]
pub struct Answers {
    pub account: Option<AccountInput>,
    pub site: Option<SiteInput>,
    pub content: Option<ContentInput>,
    pub delivery: Option<DeliveryInput>,
    pub mail: Option<MailInput>,
    pub assistants: Option<AssistantsInput>,
    pub updates: Option<UpdatesInput>,
    #[serde(default = "default_true")]
    pub finish: bool,
}

/// Runs the answers in step order. The account step runs only when no
/// administrator exists; everything else applies to whoever exists.
///
/// # Errors
/// The first step that fails, named.
pub async fn run_answers(state: &AppState, answers: Answers) -> Result<Vec<String>, AppError> {
    let mut done = Vec::new();
    let mut author_id: Option<i64> = None;
    if let Some(acc) = answers.account {
        if needs_admin(state).await? {
            author_id = Some(account(state, acc).await?.id);
            done.push("account".into());
        }
    }
    if author_id.is_none() {
        author_id = vyasa_db::repo::UsersRepo::new(state.pool.clone())
            .list(100, 0)
            .await?
            .into_iter()
            .find(|u| u.role == vyasa_db::models::Role::Admin)
            .map(|u| u.id);
    }
    let Some(author_id) = author_id else {
        return Err(AppError::validation(
            "no administrator exists and the answers have no [account] section",
        ));
    };
    if let Some(s) = answers.site {
        site(state, s).await?;
        done.push("site".into());
    }
    if let Some(c) = answers.content {
        content(state, author_id, c).await?;
        done.push("content".into());
    }
    if let Some(d) = answers.delivery {
        delivery(state, d).await?;
        done.push("delivery".into());
    }
    if let Some(m) = answers.mail {
        mail(state, m).await?;
        done.push("mail".into());
    }
    if let Some(a) = answers.assistants {
        assistants(state, a).await?;
        done.push("assistants".into());
    }
    if let Some(u) = answers.updates {
        updates(state, u).await?;
        done.push("updates".into());
    }
    if answers.finish {
        finish(state).await?;
        done.push("finish".into());
    }
    Ok(done)
}
