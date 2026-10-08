//! Vyasa API server: composition root wiring the database, domain
//! services, theme engine, plugin runtime, search, and jobs into the
//! Axum HTTP server (REST + GraphQL + public site rendering).

use std::process::ExitCode;

use axum::response::IntoResponse;
use clap::{Parser, Subcommand};
use vyasa_common::VyasaConfig;
use vyasa_db::{health, pool, run_migrations};

mod admin_assets;
mod agent_eyes;
mod ai_events;
mod ai_features;
mod ai_jobs;
mod ai_registry;
mod analytics;
mod audit;
mod authz;
mod boot;
mod cache_invalidator;
mod cdn;
mod client_ip;
mod comment_notifier;
mod content_queries;
mod entry_fields;
mod error;
mod event_bridge;
mod export;
mod feeds;
mod flexible_id;
mod forms;
mod graphql;
#[cfg(feature = "importer")]
mod importer;
mod indexnow;
mod locks;
mod mail;
mod mcp;
mod media_jobs;
mod media_s3;
mod media_storage;
mod metrics;
mod mfa;
mod middleware;
mod net_guard;
mod newsletter;
mod official;
mod patterns;
mod permalinks;
mod plugin_blocks;
mod plugin_fetch;
mod plugin_hooks;
mod plugin_sections;
mod plugin_surface;
mod plugin_tasks;
mod plugin_tool;
mod plugins_boot;
mod policy;
mod post_workflow;
mod privacy;
mod public;
mod registry;
mod render_cache;
#[cfg(test)]
mod reserved_slug_drift;
mod rest;
mod search_indexer;
mod seed;
mod seo;
mod setup;
mod signing;
mod state;
mod theme_assets;
mod theme_assistant;
mod theme_defaults;
mod theme_studio;
mod theme_tool;
mod update;
mod webhook_dispatcher;

use state::AppState;

/// Vyasa content management system.
#[derive(Parser)]
#[command(name = "vyasa", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Operational subcommands.
#[derive(Subcommand)]
enum Command {
    /// Apply all pending database migrations (idempotent).
    Migrate {
        /// List what would run and change nothing. Migrations are
        /// forward-only, so this is the only preview there is.
        #[arg(long)]
        plan: bool,
    },
    /// Probe the database and report connectivity + migration version.
    Health,
    /// Start the HTTP server.
    Serve,
    /// Import content from another CMS (feature `importer`).
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
    /// Search index maintenance.
    Search {
        #[command(subcommand)]
        command: SearchCommand,
    },
    /// Administrative operations.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Write the site's content as a Vyasa JSON archive or WordPress WXR.
    Export {
        /// `json` or `wxr`.
        #[arg(long, default_value = "json")]
        format: String,
        /// Where to write; stdout when omitted.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// First-run setup without a browser: apply an answers file, or print
    /// what the wizard wrote.
    Setup {
        /// A TOML file with [account], [site], [content], [delivery],
        /// [mail], [assistants], [updates] sections; each optional.
        #[arg(long)]
        answers: Option<std::path::PathBuf>,
        /// Print the current answers as TOML.
        #[arg(long)]
        print: bool,
    },
    /// Model Context Protocol server over stdio (set VYASA_API_KEY;
    /// VYASA_URL defaults to http://127.0.0.1:3000).
    Mcp,
    /// Check for, verify and apply upgrades.
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Development helpers.
    Dev {
        #[command(subcommand)]
        command: DevCommand,
    },
    /// Plugin packaging: mint a signing key, sign a package.
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    /// Theme packaging: zip and sign a theme source tree.
    Theme {
        #[command(subcommand)]
        command: ThemeCommand,
    },
    /// GraphQL schema tooling.
    Graphql {
        #[command(subcommand)]
        command: GraphqlCommand,
    },
    /// Print the REST API's OpenAPI document as JSON.
    ///
    /// Needs no database: the document is built from types, not data.
    Openapi,
}

/// Theme authoring subcommands. None touches the database.
#[derive(Subcommand)]
enum ThemeCommand {
    /// Pack a theme source tree into a `.vytheme`, signed when a key is
    /// given (needed for a theme that carries a script).
    Pack {
        /// Directory holding manifest.toml, tokens.json and layout.json.
        dir: std::path::PathBuf,
        /// Output path; defaults to `<name>-<version>.vytheme`.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Signing key as 64 hex characters; defaults to VYASA_SIGNING_KEY.
        #[arg(long)]
        key: Option<String>,
    },
}

/// Plugin authoring subcommands. Neither touches the database.
#[derive(Subcommand)]
enum PluginCommand {
    /// Generate an ed25519 signing keypair for plugin packages.
    Keygen,
    /// Pack a directory (manifest.toml + plugin.wasm) into a signed
    /// `.vyplugin` archive.
    Pack {
        /// Directory holding manifest.toml and plugin.wasm.
        dir: std::path::PathBuf,
        /// Output path; defaults to `<dir>.vyplugin`.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Signing key as 64 hex characters; defaults to VYASA_SIGNING_KEY.
        #[arg(long)]
        key: Option<String>,
    },
    /// Install a signed package into the database (headless equivalent of
    /// Admin -> Plugins -> Install).
    Install {
        /// Path to the `.vyplugin` package.
        package: std::path::PathBuf,
        /// Enable it immediately. A server that is already running
        /// still needs a restart: this writes to the database from a
        /// separate process.
        #[arg(long)]
        enable: bool,
    },
}

/// Upgrade subcommands.
#[derive(Subcommand)]
enum UpdateCommand {
    /// Report the running version, what the channel offers, and every
    /// preflight finding. Exits 10 when an upgrade is available, so a
    /// cron job can act on it.
    Check,
    /// Run the preflight for a target version without downloading it.
    Verify {
        /// Version to check; defaults to the newest offered.
        #[arg(long)]
        to: Option<String>,
    },
    /// Download, verify, back up, swap, migrate and restart.
    Apply {
        /// Version to install; defaults to the newest offered.
        #[arg(long)]
        to: Option<String>,
        /// Pid of the server to restart once the swap lands. The admin
        /// passes its own; on the command line the operator restarts.
        #[arg(long)]
        restart_pid: Option<u32>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Report every step without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Skip the database dump. Migrations are forward-only, so this
        /// discards the only way back.
        #[arg(long)]
        no_backup: bool,
    },
    /// Put the previous binary back. Safe only when the new build never
    /// served a request; otherwise restore the dump instead.
    Rollback,
}

/// GraphQL subcommands.
#[derive(Subcommand)]
enum GraphqlCommand {
    /// Print the schema in SDL form.
    ///
    /// Needs no database: the schema is built from types, not data.
    Sdl,
}

/// Development-only helpers.
#[derive(Subcommand)]
enum DevCommand {
    /// Generate deterministic demo content for benchmarking and screenshots.
    Seed {
        /// How many published posts to create.
        #[arg(long, default_value_t = 100)]
        count: u32,
        /// Author id; defaults to the lowest-numbered admin.
        #[arg(long)]
        author: Option<i64>,
    },
}

/// Import sources.
#[derive(Subcommand)]
enum ImportCommand {
    /// A Vyasa JSON archive from `vyasa export`.
    Json {
        /// The archive file.
        file: std::path::PathBuf,
        /// Also apply the archive's site options.
        #[arg(long, default_value_t = false)]
        options: bool,
    },
    /// WordPress database import.
    Wp {
        /// Source WP database URL (postgres://… schema via pgloader).
        #[arg(long)]
        database_url: String,
        /// Base URL of the source uploads directory.
        #[arg(long)]
        media_url: Option<String>,
        /// Parse and report without writing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

/// Search subcommands.
#[derive(Subcommand)]
enum SearchCommand {
    /// Rebuild the full-text index from the database.
    ///
    /// Needed after restoring a backup, after the indexer reports dropped
    /// events, or the first time search is enabled on existing content.
    Reindex,
}

/// Admin subcommands.
#[derive(Subcommand)]
enum AdminCommand {
    /// Create the first administrator user.
    Create {
        /// Admin email address.
        #[arg(long)]
        email: String,
        /// Admin password (prompted when omitted).
        #[arg(long)]
        password: Option<String>,
        /// Admin username (derived from email when omitted).
        #[arg(long)]
        username: Option<String>,
    },
}

/// `vyasa theme …`: no database, no configuration file.
fn run_theme_command(command: &ThemeCommand) -> ExitCode {
    let ThemeCommand::Pack { dir, out, key } = command;
    match theme_tool::pack(dir, out.clone(), key.clone()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Handled before the config loads: the schema is built from types, so
    // requiring a database URL to print it would make the command unusable
    // in CI, which is exactly where the drift check runs.
    if let Command::Graphql { command } = &cli.command {
        let GraphqlCommand::Sdl = command;
        println!("{}", graphql::build_schema().sdl());
        return ExitCode::SUCCESS;
    }

    // Same reasoning as the SDL: CI checks the snapshot without a database.
    if let Command::Openapi = &cli.command {
        let doc = <rest::docs::ApiDoc as utoipa::OpenApi>::openapi();
        match serde_json::to_string(&doc) {
            Ok(json) => {
                println!("{json}");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("could not serialise the OpenAPI document: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // The MCP server is a stdio courier to a *running* site's REST API:
    // it needs an API key and a URL, never the database or the config.
    if let Command::Mcp = &cli.command {
        return mcp::run();
    }

    // Theme packaging is an author's tool as well: files on disk only.
    if let Command::Theme { command } = &cli.command {
        return run_theme_command(command);
    }

    // Plugin packaging is an author's tool: it signs files on disk and
    // needs neither a database nor a configuration file.
    if let Command::Plugin { command } = &cli.command {
        // Keygen and pack are offline file operations; install needs the
        // configured database and trusted keys, so it runs below.
        let outcome = match command {
            PluginCommand::Keygen => {
                plugin_tool::keygen();
                Some(Ok(()))
            }
            PluginCommand::Pack { dir, out, key } => {
                Some(plugin_tool::pack(dir, out.clone(), key.clone()))
            }
            PluginCommand::Install { .. } => None,
        };
        if let Some(outcome) = outcome {
            return match outcome {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("vyasa plugin: {err}");
                    ExitCode::FAILURE
                }
            };
        }
    }

    let config = match VyasaConfig::load() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("vyasa: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    vyasa_common::telemetry::init_from_config(&config);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("vyasa: could not start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move {
        match cli.command {
            Command::Migrate { plan } => cmd_migrate(&config, plan).await,
            Command::Health => cmd_health(&config).await,
            Command::Serve => cmd_serve(&config).await,
            Command::Import { command } => cmd_import(&config, command).await,
            Command::Search { command } => cmd_search(&config, command).await,
            Command::Admin { command } => cmd_admin(&config, command).await,
            Command::Setup { answers, print } => cmd_setup(&config, answers, print).await,
            Command::Export { format, out } => cmd_export(&config, &format, out).await,
            Command::Dev { command } => cmd_dev(&config, command).await,
            Command::Mcp | Command::Theme { .. } => unreachable!("handled before config load"),
            Command::Update { command } => cmd_update(&config, command).await,
            Command::Plugin { command } => match command {
                PluginCommand::Install { package, enable } => {
                    match plugin_tool::install(&config, &package, enable).await {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(err) => {
                            eprintln!("vyasa plugin: {err}");
                            ExitCode::FAILURE
                        }
                    }
                }
                // Handled before the runtime starts; see `main`.
                PluginCommand::Keygen | PluginCommand::Pack { .. } => ExitCode::SUCCESS,
            },
            // Handled before the runtime starts; see `main`.
            Command::Graphql { .. } | Command::Openapi => ExitCode::SUCCESS,
        }
    })
}

async fn cmd_admin(config: &VyasaConfig, admin: AdminCommand) -> ExitCode {
    let pg_pool = match pool(config).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("vyasa admin: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = run_migrations(&pg_pool).await {
        eprintln!("vyasa admin: {err}");
        return ExitCode::FAILURE;
    }
    match admin {
        AdminCommand::Create {
            email,
            password,
            username,
        } => {
            let password = match password {
                Some(password) => password,
                None => match rpassword::prompt_password("Admin password: ") {
                    Ok(password) => password,
                    Err(err) => {
                        eprintln!("vyasa admin: could not read password: {err}");
                        return ExitCode::FAILURE;
                    }
                },
            };
            let users = vyasa_core::user::UsersService::new(vyasa_db::repo::UsersRepo::new(
                pg_pool.clone(),
            ));
            match users
                .create(vyasa_core::user::CreateUser {
                    email,
                    username,
                    display_name: None,
                    password: Some(password),
                    role: vyasa_db::models::Role::Admin,
                })
                .await
            {
                Ok(user) => {
                    println!(
                        "vyasa admin: created admin {} (username: {})",
                        user.email, user.username
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("vyasa admin: {err}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// The site's content as an archive.
async fn cmd_export(
    config: &VyasaConfig,
    format: &str,
    out: Option<std::path::PathBuf>,
) -> ExitCode {
    let pg_pool = match pool(config).await {
        Ok(p) => p,
        Err(err) => {
            eprintln!("vyasa export: {err}");
            return ExitCode::FAILURE;
        }
    };
    let state = AppState::new(config.clone(), pg_pool);
    let site_url = state.options_service.site_url().await.unwrap_or_default();
    let archive = match export::build(&state, &site_url).await {
        Ok(a) => a,
        Err(err) => {
            eprintln!("vyasa export: {err}");
            return ExitCode::FAILURE;
        }
    };
    let body = if format == "wxr" {
        export::to_wxr(&archive, &site_url)
    } else {
        serde_json::to_string_pretty(&archive).unwrap_or_default()
    };
    let Some(path) = out else {
        println!("{body}");
        return ExitCode::SUCCESS;
    };
    match std::fs::write(&path, body) {
        Ok(()) => {
            println!("vyasa export: wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vyasa export: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The wizard's steps from a file, or the answers it wrote, without a browser.
async fn cmd_setup(
    config: &VyasaConfig,
    answers: Option<std::path::PathBuf>,
    print: bool,
) -> ExitCode {
    let pg_pool = match pool(config).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("vyasa setup: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = run_migrations(&pg_pool).await {
        eprintln!("vyasa setup: {err}");
        return ExitCode::FAILURE;
    }
    let state = AppState::new(config.clone(), pg_pool);
    if let Err(err) = theme_defaults::bootstrap_starters(&state.themes, &state.options, None).await
    {
        eprintln!("vyasa setup: starter themes: {err}");
    }
    let _ = theme_defaults::bootstrap_menu(&state.menus).await;
    if print {
        println!("{}", setup::print_answers(&state).await);
        return ExitCode::SUCCESS;
    }
    let Some(path) = answers else {
        eprintln!("vyasa setup: pass --answers file.toml or --print");
        return ExitCode::FAILURE;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("vyasa setup: cannot read {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let parsed: setup::Answers = match toml::from_str(&text) {
        Ok(a) => a,
        Err(err) => {
            eprintln!("vyasa setup: {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match setup::run_answers(&state, parsed).await {
        Ok(done) => {
            println!("vyasa setup: applied {}", done.join(", "));
            println!("  a running `vyasa serve` caches site options; restart it to pick these up");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vyasa setup: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Makes the stored content types live in this process (what `serve`
/// does at boot), for one-shot commands that read or write entries.
async fn load_content_types(state: &AppState) {
    match vyasa_core::content::ContentTypesService::new(state.pool.clone())
        .load_into_registry()
        .await
    {
        Ok(skipped) => {
            for (slug, reason) in skipped {
                eprintln!("vyasa: content type {slug:?} not loaded: {reason}");
            }
        }
        Err(e) => eprintln!("vyasa: content types could not be loaded: {e}"),
    }
}

/// Rebuilds the search index from stored content.
async fn cmd_search(config: &VyasaConfig, command: SearchCommand) -> ExitCode {
    let SearchCommand::Reindex = command;
    let pg_pool = match pool(config).await {
        Ok(pg_pool) => pg_pool,
        Err(err) => {
            eprintln!("vyasa search: database unavailable: {err}");
            return ExitCode::FAILURE;
        }
    };
    let state = AppState::new(config.clone(), pg_pool);
    // Administrators' content types are searchable like any public type;
    // a one-shot command has no boot to load them.
    load_content_types(&state).await;
    match search_indexer::reindex_all(&state).await {
        Ok(count) => {
            println!("vyasa search: indexed {count} published posts");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vyasa search: reindex failed: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn cmd_dev(config: &VyasaConfig, command: DevCommand) -> ExitCode {
    let DevCommand::Seed { count, author } = command;
    let pg_pool = match pool(config).await {
        Ok(pg_pool) => pg_pool,
        Err(err) => {
            eprintln!("vyasa dev seed: database unavailable: {err}");
            return ExitCode::FAILURE;
        }
    };
    // Seeding needs a real author: posts carry a foreign key, and inventing
    // an id would fail at the database rather than here.
    let author_id = match author {
        Some(id) => id,
        None => {
            match sqlx::query_scalar::<_, i64>(
                "SELECT id FROM users WHERE role = 'admin' ORDER BY id LIMIT 1",
            )
            .fetch_optional(&pg_pool)
            .await
            {
                Ok(Some(id)) => id,
                Ok(None) => {
                    eprintln!(
                        "vyasa dev seed: no admin user exists. Create one with \
                         `vyasa admin create`, or pass --author."
                    );
                    return ExitCode::FAILURE;
                }
                Err(err) => {
                    eprintln!("vyasa dev seed: could not find an author: {err}");
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    let state = AppState::new(config.clone(), pg_pool);
    match seed::run(&state, count, author_id).await {
        Ok(made) => {
            println!("vyasa dev seed: created {made} published posts");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vyasa dev seed: failed: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Serves the admin SPA index.html.
///
/// Explicitly not cached. The assets it points at are content-hashed and
/// cached hard; this document is the only thing that says which hashes are
/// current, and a stale copy either shows the previous build or references
/// bundles the last deploy deleted.
async fn admin_index() -> impl IntoResponse {
    match tokio::fs::read_to_string("admin/dist/index.html").await {
        Ok(html) => (
            [(
                axum::http::header::CACHE_CONTROL,
                "no-cache, must-revalidate",
            )],
            axum::response::Html(html),
        )
            .into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "admin not built").into_response(),
    }
}

async fn cmd_migrate(config: &VyasaConfig, plan_only: bool) -> ExitCode {
    let pool = match pool(config).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("vyasa migrate: {err}");
            return ExitCode::FAILURE;
        }
    };
    if plan_only {
        return match vyasa_db::pending_migrations(&pool).await {
            Ok(pending) if pending.is_empty() => {
                println!("vyasa migrate: nothing pending; the database is up to date");
                ExitCode::SUCCESS
            }
            Ok(pending) => {
                println!(
                    "vyasa migrate: {} migration(s) would run, in this order:",
                    pending.len()
                );
                for (version, description) in &pending {
                    println!("  {version}  {description}");
                }
                println!(
                    "\nMigrations are forward-only. Back the database up first — \
                     that dump is the only way back."
                );
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("vyasa migrate: {err}");
                ExitCode::FAILURE
            }
        };
    }
    match run_migrations(&pool).await {
        Ok(()) => {
            let report = health(&pool).await;
            println!(
                "vyasa migrate: database is up to date (migration version {})",
                report.migration_version.unwrap_or(0)
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vyasa migrate: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `vyasa update …` — the CLI half of the upgrade feature.
///
/// It builds the same `AppState` the server uses so the preflight sees
/// exactly what a running install would: the same options, the same
/// plugin rows, the same configured media directory.
async fn cmd_update(config: &VyasaConfig, command: UpdateCommand) -> ExitCode {
    let pool = match pool(config).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("vyasa update: {err}");
            return ExitCode::FAILURE;
        }
    };
    let state = AppState::new(config.clone(), pool);
    match command {
        UpdateCommand::Check => {
            let status = update::status(&state).await;
            print_status(&status);
            // A distinct code so `vyasa update check` can drive a cron
            // job or a monitor without parsing prose.
            if status.update_available {
                ExitCode::from(10)
            } else {
                ExitCode::SUCCESS
            }
        }
        UpdateCommand::Verify { to } => match update::plan_for(&state, to.as_deref()).await {
            Ok((release, checks, env)) => {
                println!(
                    "vyasa update: {} → {} on a {:?} deployment\n",
                    update::manifest::CURRENT_VERSION,
                    release.version,
                    env.mode
                );
                print_findings(&checks);
                if checks.can_proceed {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(err) => {
                eprintln!("vyasa update: {err}");
                ExitCode::FAILURE
            }
        },
        UpdateCommand::Apply {
            to,
            restart_pid,
            yes,
            dry_run,
            no_backup,
        } => cmd_update_apply(&state, to, restart_pid, yes, dry_run, no_backup).await,
        UpdateCommand::Rollback => cmd_update_rollback(),
    }
}

/// Renders an update status for a terminal.
fn print_status(status: &update::UpdateStatus) {
    println!("current      {}", status.current);
    match (&status.latest, &status.channel_error) {
        (Some(latest), _) => println!("latest       {latest}"),
        (None, Some(err)) => println!("latest       unknown ({err})"),
        (None, None) => println!("latest       unknown"),
    }
    println!(
        "deployment   {:?}{}",
        status.environment.mode,
        if status.environment.mode.can_self_update() {
            if status.environment.binary_replaceable {
                " (can upgrade itself)"
            } else {
                " (binary not writable here)"
            }
        } else {
            " (upgraded from outside)"
        }
    );
    if status.update_available {
        println!("\nAvailable:");
        for release in &status.releases {
            let flag = if release.requires_attention {
                "  ← needs attention"
            } else {
                ""
            };
            println!("  {} {}{flag}", release.version, release.summary);
        }
        if !status.instructions.is_empty() && !status.environment.mode.can_self_update() {
            println!("\nTo upgrade this install:");
            for line in &status.instructions {
                println!("  {line}");
            }
        }
    } else if status.channel_error.is_none() {
        println!("\nUp to date.");
    }
    println!();
    print_findings(&status.preflight);
}

/// Renders preflight findings.
fn print_findings(checks: &update::preflight::Preflight) {
    for finding in &checks.findings {
        let mark = match finding.status {
            vyasa_core::health::Status::Ok => "ok  ",
            vyasa_core::health::Status::Warn => "warn",
            vyasa_core::health::Status::Fail => "FAIL",
        };
        println!("  [{mark}] {:<14} {}", finding.name, finding.detail);
    }
    if !checks.can_proceed {
        println!("\nBlocked: fix the failures above before upgrading.");
    }
}

/// The apply path, kept apart so the dispatcher stays readable.
async fn cmd_update_apply(
    state: &AppState,
    to: Option<String>,
    restart_pid: Option<u32>,
    yes: bool,
    dry_run: bool,
    no_backup: bool,
) -> ExitCode {
    let (release, checks, env) = match update::plan_for(state, to.as_deref()).await {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!("vyasa update: {err}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "vyasa update: {} → {}\n",
        update::manifest::CURRENT_VERSION,
        release.version
    );
    print_findings(&checks);
    if !checks.can_proceed {
        return ExitCode::FAILURE;
    }
    if !env.mode.can_self_update() {
        println!(
            "\nThis is a {:?} deployment: the image is the unit of upgrade, so \
             nothing here can swap it safely. Run:",
            env.mode
        );
        for line in env.mode.instructions(&release.version) {
            println!("  {line}");
        }
        return ExitCode::FAILURE;
    }
    let Some(artifact) = release.artifact_for_host() else {
        eprintln!(
            "vyasa update: no build for {} in {}",
            update::manifest::host_target(),
            release.version
        );
        return ExitCode::FAILURE;
    };
    if dry_run {
        println!(
            "\nDry run. Would download {}\n  verify sha256 {}\n  {}\n  swap the binary, \
             migrate, restart, then health-check.",
            artifact.url,
            artifact.sha256,
            if no_backup {
                "skip the database dump (no way back)"
            } else {
                "dump the database first"
            }
        );
        return ExitCode::SUCCESS;
    }
    if !yes && !confirm(&release.version) {
        println!("Cancelled.");
        return ExitCode::SUCCESS;
    }
    run_apply(state, &release, artifact, no_backup, restart_pid).await
}

/// Asks for a typed confirmation on a terminal.
fn confirm(version: &str) -> bool {
    use std::io::Write as _;
    print!("Upgrade to {version}? Type the version to confirm: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    line.trim() == version
}

/// The irreversible half, in the order that keeps it recoverable.
async fn run_apply(
    state: &AppState,
    release: &update::manifest::Release,
    artifact: &update::manifest::Artifact,
    no_backup: bool,
    restart_pid: Option<u32>,
) -> ExitCode {
    use update::apply::{self, Progress, Stage};

    let Ok(binary) = std::env::current_exe() else {
        eprintln!("vyasa update: cannot locate the running binary");
        return ExitCode::FAILURE;
    };
    let dir = binary
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let step = |stage: Stage, detail: &str| {
        println!("  {detail}");
        Progress::new(&release.version, stage, detail).write_beside(&binary);
    };

    let prepared = match prepare_release(state, release, artifact, no_backup, &binary, &dir).await {
        Ok(prepared) => prepared,
        Err(code) => return code,
    };
    let Prepared {
        new_binary,
        admin,
        backup,
        staging,
    } = prepared;

    step(
        Stage::Swapping,
        "installing the new binary and admin bundle",
    );
    let previous = match apply::swap_binary(&binary, &new_binary) {
        Ok(path) => path,
        Err(err) => return fail(&binary, &release.version, &format!("{err}")),
    };
    if let Some(new_dist) = &admin {
        // The live bundle sits at <cwd>/admin/dist, where the server reads
        // it from; binary and bundle must move together or the admin talks
        // to a server that no longer matches it.
        let live = std::path::Path::new("admin").join("dist");
        if let Err(err) = apply::swap_admin(new_dist, &live) {
            let _ = apply::restore_binary(&binary, &previous);
            return fail(
                &binary,
                &release.version,
                &format!("{err} (the previous binary was restored)"),
            );
        }
    }

    step(Stage::Migrating, "applying migrations");
    if let Err(err) = run_migrations(&state.pool).await {
        // The schema is the one thing a binary swap cannot undo: put the
        // old binary back so the site keeps serving, and say plainly that
        // the dump is the recovery path if the schema did move.
        let _ = apply::restore_binary(&binary, &previous);
        eprintln!("vyasa update: migration failed: {err}");
        if let Some(path) = &backup {
            eprintln!(
                "  previous binary restored; restore {} if the schema moved",
                path.display()
            );
        }
        Progress::new(
            &release.version,
            Stage::RolledBack,
            format!("migration failed: {err}"),
        )
        .write_beside(&binary);
        let _ = std::fs::remove_dir_all(&staging);
        return ExitCode::FAILURE;
    }
    let _ = std::fs::remove_dir_all(&staging);

    if let Some(pid) = restart_pid {
        step(Stage::Restarting, "asking the server to restart");
        if let Err(code) = restart_and_verify(state, release, pid, &binary, &previous).await {
            return code;
        }
    }

    let mut done = Progress::new(
        &release.version,
        Stage::Done,
        format!("upgraded to {}", release.version),
    );
    done.backup_path = backup.as_ref().map(|p| p.display().to_string());
    done.write_beside(&binary);
    println!(
        "\nInstalled {}.{}",
        release.version,
        if restart_pid.is_some() {
            " The server is serving the new build."
        } else {
            " Restart the server to run it."
        }
    );
    ExitCode::SUCCESS
}

/// What [`prepare_release`] produced: everything needed for the swap,
/// with nothing irreversible done yet.
struct Prepared {
    new_binary: Vec<u8>,
    admin: Option<std::path::PathBuf>,
    backup: Option<std::path::PathBuf>,
    staging: std::path::PathBuf,
}

/// Download, verify, unpack and back up — the whole reversible half.
///
/// Every failure here leaves the install exactly as it was, which is why
/// it all happens before the first `rename`.
async fn prepare_release(
    state: &AppState,
    release: &update::manifest::Release,
    artifact: &update::manifest::Artifact,
    no_backup: bool,
    binary: &std::path::Path,
    dir: &std::path::Path,
) -> Result<Prepared, ExitCode> {
    use update::apply::{self, Progress, Stage};
    let step = |stage: Stage, detail: &str| {
        println!("  {detail}");
        Progress::new(&release.version, stage, detail).write_beside(binary);
    };

    step(Stage::Downloading, "downloading the release");
    let keys = update::source(state).keys;
    let bytes = match apply::download(artifact, &keys).await {
        Ok(bytes) => bytes,
        Err(err) => return Err(fail(binary, &release.version, &format!("{err}"))),
    };
    step(Stage::Verifying, "checksum and signature verified");

    // Staged beside the binary so the final rename is atomic — a staging
    // directory in /tmp would cross filesystems and lose that.
    let staging = dir.join(".vyasa-update-staging");
    let _ = std::fs::remove_dir_all(&staging);
    let unpacked = match apply::unpack_release(&bytes, &staging) {
        Ok(unpacked) => unpacked,
        Err(err) => return Err(fail(binary, &release.version, &format!("{err}"))),
    };
    let new_binary = match std::fs::read(&unpacked.binary) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Err(fail(
                binary,
                &release.version,
                &format!("cannot read the new binary: {err}"),
            ))
        }
    };

    let mut backup = None;
    if no_backup {
        println!("  skipping the database dump (--no-backup)");
    } else {
        step(Stage::BackingUp, "dumping the database");
        match apply::back_up(
            state.config.database_url.expose(),
            dir,
            update::manifest::CURRENT_VERSION,
        ) {
            Ok(path) => {
                println!("    saved {}", path.display());
                backup = Some(path);
            }
            Err(err) => return Err(fail(binary, &release.version, &format!("{err}"))),
        }
    }
    Ok(Prepared {
        new_binary,
        admin: unpacked.admin,
        backup,
        staging,
    })
}

/// Restarts the server and waits for the new build to answer.
///
/// A failure here is the one moment an automatic rollback is safe: the
/// new build has served nothing, so the old binary is still correct for
/// the schema. Once it *has* served a request, going back means restoring
/// the dump — see the module comment on `update::apply`.
async fn restart_and_verify(
    state: &AppState,
    release: &update::manifest::Release,
    pid: u32,
    binary: &std::path::Path,
    previous: &std::path::Path,
) -> Result<(), ExitCode> {
    use update::apply::{self, Progress, Stage};

    if !apply::signal_restart(pid) {
        println!("  the server was not running; start it when you are ready");
        return Ok(());
    }
    let base = base_url_for(&state.config.bind_addr);
    match apply::await_health(&base, &release.version).await {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = apply::restore_binary(binary, previous);
            eprintln!("vyasa update: {err}");
            eprintln!("  the previous binary is back; start the server again");
            Progress::new(
                &release.version,
                Stage::RolledBack,
                format!("new build did not come up: {err}"),
            )
            .write_beside(binary);
            Err(ExitCode::FAILURE)
        }
    }
}

/// A loopback URL for the configured bind address, for health polling.
fn base_url_for(bind_addr: &str) -> String {
    let port = bind_addr.rsplit(':').next().unwrap_or("3000");
    format!("http://127.0.0.1:{port}")
}

/// Records a failed run and reports it.
fn fail(binary: &std::path::Path, version: &str, detail: &str) -> ExitCode {
    eprintln!("vyasa update: {detail}");
    update::apply::Progress::new(version, update::apply::Stage::Failed, detail)
        .write_beside(binary);
    ExitCode::FAILURE
}

/// `vyasa update rollback` — put the kept binary back.
fn cmd_update_rollback() -> ExitCode {
    let Ok(binary) = std::env::current_exe() else {
        eprintln!("vyasa update: cannot locate the running binary");
        return ExitCode::FAILURE;
    };
    let previous = binary
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("vyasa.previous");
    match update::apply::restore_binary(&binary, &previous) {
        Ok(()) => {
            println!(
                "Previous binary restored. If migrations ran since the upgrade, \
                 restore the pre-upgrade dump too — an older build cannot read \
                 documents a newer one has written."
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vyasa update: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn cmd_health(config: &VyasaConfig) -> ExitCode {
    let pool = match pool(config).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("vyasa health: {err}");
            return ExitCode::FAILURE;
        }
    };
    let report = health(&pool).await;
    if report.ok {
        println!(
            "vyasa health: ok (migration version {})",
            report.migration_version.unwrap_or(0)
        );
        ExitCode::SUCCESS
    } else {
        println!(
            "vyasa health: failing ({})",
            report.error.unwrap_or_else(|| "unknown".to_string())
        );
        ExitCode::FAILURE
    }
}

#[allow(clippy::too_many_lines)] // server startup is necessarily verbose
async fn cmd_serve(config: &VyasaConfig) -> ExitCode {
    let pg_pool = match pool(config).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("vyasa serve: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = run_migrations(&pg_pool).await {
        eprintln!("vyasa serve: {err}");
        return ExitCode::FAILURE;
    }
    // A container platform can hand us the first administrator.
    match boot::first_admin_from_env(&pg_pool).await {
        Ok(boot::FirstAdmin::Created(email)) => {
            println!("vyasa serve: created the first administrator {email} from VYASA_ADMIN_EMAIL");
        }
        Ok(boot::FirstAdmin::UsersExist) => {
            eprintln!(
                "vyasa serve: VYASA_ADMIN_EMAIL/VYASA_ADMIN_PASSWORD ignored: users already exist (remove the variables)"
            );
        }
        Ok(boot::FirstAdmin::HalfConfigured) => {
            eprintln!(
                "vyasa serve: VYASA_ADMIN_EMAIL/VYASA_ADMIN_PASSWORD ignored: set both or neither"
            );
        }
        Ok(boot::FirstAdmin::NotConfigured) => {}
        Err(err) => {
            eprintln!("vyasa serve: VYASA_ADMIN_PASSWORD/VYASA_ADMIN_EMAIL: {err}");
            return ExitCode::FAILURE;
        }
    }
    // First-run: no users yet → mint the setup token the wizard asks for.
    let mut first_boot_token: Option<String> = None;
    match vyasa_db::repo::UsersRepo::new(pg_pool.clone())
        .count()
        .await
    {
        Ok(0) => {
            let token = setup::mint_token();
            let run_dir = config.run_dir.as_path();
            let _ = std::fs::create_dir_all(run_dir);
            let _ = std::fs::write(run_dir.join("setup-token"), &token);
            println!(
                "vyasa serve: no users exist yet. Open /admin/setup in a browser and enter this token:"
            );
            println!("    {token}");
            println!(
                "    (also written to {}/setup-token; or run `vyasa admin create`)",
                run_dir.display()
            );
            first_boot_token = Some(token);
        }
        Ok(_) => {}
        Err(err) => eprintln!("vyasa serve: could not count users: {err}"),
    }
    let state = AppState::new(config.clone(), pg_pool.clone());
    // Object storage chosen in the admin, unless the environment set one.
    media_storage::apply_saved(&state).await;
    if let Some(token) = first_boot_token {
        *state
            .setup_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(token);
    }
    // Spawn the scheduled-publisher worker (direct, phase 11).
    let _publisher = vyasa_jobs::publisher::spawn(pg_pool.clone(), state.publisher_notify.clone());
    // Outbound link sweep: a batch of published entries an hour.
    // Outbound work a visitor could aim anywhere stays off in a demo.
    let outbound = policy::outbound_jobs_allowed(&state);
    let _link_sweep = outbound.then(|| seo::spawn_sweep(pg_pool.clone()));
    // Accounts that registered and never confirmed their address go after
    // a week (phase 98).
    let _registration_purge = vyasa_jobs::registration::spawn(pg_pool.clone());
    // Spawn the job queue workers for media derivatives etc.
    let _jobs = vyasa_jobs::runner::spawn(
        pg_pool.clone(),
        config.jobs.workers,
        state.publisher_notify.clone(),
        config.smtp.clone(),
        // Media first: it owns the configured storage backend, which the
        // jobs crate's own worker can only guess at from the environment.
        Some(std::sync::Arc::new(media_jobs::Handlers(vec![
            media_jobs::MediaJobs::handler(&state),
            ai_jobs::AiJobs::handler(state.clone()),
            // Mail resolves its relay per job: Settings can change it live.
            std::sync::Arc::new(mail::MailJobs(state.clone())),
            std::sync::Arc::new(media_storage::MigrateJobs(state.clone())),
        ]))),
    );

    // Offer missing starter themes on upgrades; preserve installed versions and activation.
    if let Err(e) = theme_defaults::bootstrap_starters(
        &state.themes,
        &state.options,
        state.render_cache.as_deref(),
    )
    .await
    {
        tracing::warn!("theme bootstrap failed: {e}");
    }
    // The starter themes all address the menu `main`; without it their
    // navigation renders empty on a brand-new site.
    if let Err(e) = theme_defaults::bootstrap_menu(&state.menus).await {
        tracing::warn!("menu bootstrap failed: {e}");
    }

    // Plugin runtime bootstrapping (phase 33): load enabled plugins, run
    // init, collect block/route declarations, and start the event fan-out.
    if let Err(e) = plugins_boot::bootstrap_plugins(&state).await {
        tracing::warn!("plugin bootstrap failed: {e}");
    }

    // Keeps the search index in step with published content.
    search_indexer::spawn(&state);
    // The bus carries what happens next, not what happened before this
    // instance booted, so a fresh or restarted node rebuilds its own copy
    // when it does not match the database.
    search_indexer::converge_on_boot(&state);

    // Purges cached pages when content changes.
    cache_invalidator::spawn(&state);

    // Relay events between instances before anything subscribes, so a
    // second server's caches hear about this one's publishes. Without it
    // the bus is process-local and two servers behind a load balancer
    // serve divergent pages indefinitely.
    event_bridge::spawn(&state.pool);

    // Fans post/comment lifecycle events out to subscribed webhooks.
    if outbound {
        webhook_dispatcher::spawn(&state);
    }

    // Emails post authors when a comment lands on their post.
    comment_notifier::spawn(&state);
    // Announces publishes/updates to IndexNow-participating engines —
    // inert until site_url points at a public address.
    if outbound {
        indexnow::spawn(&state);
    }
    // Emails confirmed subscribers on publish; off by default.
    newsletter::spawn(&state);
    // AI integrations: embeddings, autofill, screening, read-aloud — all
    // gated by options that default to off.
    ai_events::spawn(&state);
    state
        .plugin_host
        .set_ai_backend(ai_registry::PluginAi::new(state.clone()));
    state
        .plugin_host
        .set_fetch_backend(plugin_fetch::PluginFetch::new());

    let app = app_router(&state);

    let listener = match tokio::net::TcpListener::bind(&config.bind_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("vyasa serve: cannot bind {}: {err}", config.bind_addr);
            return ExitCode::FAILURE;
        }
    };
    println!("vyasa serve: listening on http://{}", config.bind_addr);
    match axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    {
        Ok(()) => {
            // The pool closes explicitly: dropping it does not wait, and a
            // connection killed mid-statement is what turns a restart into
            // a rolled-back write the author thought had landed.
            pg_pool.close().await;
            println!("vyasa serve: stopped cleanly");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vyasa serve: server error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The whole application: the admin shell, the public site, `/api/v1`,
/// GraphQL and the API docs, under the layers every request passes.
///
/// A function of its own so a test can build it: two routers claiming the
/// same path panic when they are merged, and that must happen in the test
/// suite rather than when the server starts.
fn app_router(state: &AppState) -> axum::Router {
    let swagger = utoipa_swagger_ui::SwaggerUi::new("/api/docs").url(
        "/api/openapi.json",
        <rest::docs::ApiDoc as utoipa::OpenApi>::openapi(),
    );

    let graphql_router = axum::Router::new()
        .route(
            "/graphql",
            axum::routing::get(graphql::graphql_handler).post(graphql::graphql_handler),
        )
        .route(
            "/graphql/playground",
            axum::routing::get(graphql::playground_handler),
        )
        .route(
            "/graphql/ws",
            axum::routing::get(graphql::graphql_ws_handler),
        );

    axum::Router::new()
        // Admin panel at /admin (and /login for DX).
        //
        // Every /admin/* URL is routed here explicitly, not left to the
        // fallback. The public site registers /{type}/{slug}, which matches
        // /admin/appearance as happily as /book/dune -- so a deep link into
        // the admin used to be answered by the public site's themed 404,
        // and a refresh on any two-segment admin page dumped the operator
        // onto a page painted in the site's theme. A fallback cannot fix
        // that: it only runs when nothing else matched.
        //
        // All three forms are needed. `{*path}` wants at least one segment,
        // so it does not cover the bare `/admin/` that the SPA's own index
        // route puts in the address bar -- reloading the dashboard fell
        // through to the public site while reloading any sub-page worked.
        .route("/admin", axum::routing::get(admin_index))
        .route("/admin/", axum::routing::get(admin_index))
        .route("/admin/{*path}", axum::routing::get(admin_index))
        .route(
            "/login",
            axum::routing::get(|| async { axum::response::Redirect::permanent("/admin/login") }),
        )
        .route("/assets/{*path}", axum::routing::get(admin_assets::serve))
        .route("/metrics", axum::routing::get(metrics::scrape))
        .merge(public::router(state))
        .nest("/api/v1", rest::router())
        .nest("/api", graphql_router)
        .merge(swagger)
        .layer(axum::middleware::from_fn(
            middleware::headers::security_headers,
        ))
        .layer(axum::middleware::from_fn(middleware::csrf::csrf_check))
        .layer(axum::middleware::from_fn(middleware::guards::body_limit))
        .layer(axum::middleware::from_fn(middleware::guards::timeout))
        .layer(axum::middleware::from_fn(middleware::request_context))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            analytics::middleware,
        ))
        // Demo mode refuses some routes before their bodies are read, so it
        // sits outside the body limits and handlers. A no-op otherwise.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::demo::demo,
        ))
        // Outermost so the recorded duration is what the client actually
        // waited, including time spent in the layers below.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            metrics::track_metrics,
        ))
        .with_state(state.clone())
}

/// Resolves on SIGTERM or Ctrl-C.
///
/// Without this the process died the instant a deploy sent SIGTERM, in the
/// middle of whatever it was serving: a visitor got a connection reset, and
/// a comment or form POST could be cut between its database write and its
/// response — which reads, to the person who submitted it, as the site
/// having silently lost their message. Every restart goes through here now,
/// so in-flight requests finish before the socket closes.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::warn!("cannot listen for SIGTERM: {err}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received; finishing in-flight requests");
}

#[cfg(feature = "importer")]
async fn cmd_import(config: &VyasaConfig, command: ImportCommand) -> ExitCode {
    match command {
        ImportCommand::Json { file, options } => {
            let pg_pool = match pool(config).await {
                Ok(p) => p,
                Err(err) => {
                    eprintln!("vyasa import: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let state = AppState::new(config.clone(), pg_pool);
            // Entries of the site's own content types import as those
            // types, not as posts.
            load_content_types(&state).await;
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(err) => {
                    eprintln!("vyasa import: cannot read {}: {err}", file.display());
                    return ExitCode::FAILURE;
                }
            };
            let archive: export::Archive = match serde_json::from_str(&text) {
                Ok(a) => a,
                Err(err) => {
                    eprintln!("vyasa import: {}: {err}", file.display());
                    return ExitCode::FAILURE;
                }
            };
            match export::import(&state, &archive, options, None).await {
                Ok(r) => {
                    println!(
                        "vyasa import: {} roles, {} users, {} terms, {} posts, {} comments, {} menus, {} options; {} skipped",
                        r.roles, r.users, r.terms, r.posts, r.comments, r.menus, r.options, r.skipped
                    );
                    for w in r.warnings {
                        eprintln!("  warning: {w}");
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("vyasa import: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        ImportCommand::Wp {
            database_url,
            media_url,
            dry_run,
        } => match crate::importer::run_cli(config, &database_url, media_url, dry_run).await {
            Ok(report) => {
                if report.warnings.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    eprintln!(
                        "{} posts need manual review (see warnings above)",
                        report.warnings.len()
                    );
                    ExitCode::SUCCESS
                }
            }
            Err(err) => {
                eprintln!("vyasa import wp: {err}");
                ExitCode::FAILURE
            }
        },
    }
}

#[cfg(not(feature = "importer"))]
#[allow(clippy::unused_async)] // signature parity with the feature-gated version
async fn cmd_import(_config: &VyasaConfig, _command: ImportCommand) -> ExitCode {
    eprintln!("vyasa: built without the `importer` feature");
    ExitCode::FAILURE
}
