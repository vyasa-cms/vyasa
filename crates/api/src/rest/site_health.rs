//! `GET /api/v1/site-health` — operational diagnostics for the admin UI.
//!
//! One list feeds both this page and the setup wizard's Welcome screen:
//! the environment checks come from [`crate::setup::checks`], the
//! operational ones are assembled here. Every check is individually
//! bounded, so a wedged dependency shows up as a failed check rather than
//! a hung request.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use vyasa_core::health::{self, Check, Report};

use crate::error::{ApiErrorBody, ApiResult};
use crate::state::AppState;

async fn count(pool: &sqlx::PgPool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

/// Free and total bytes on the volume that holds `path`.
fn volume_space(path: &std::path::Path) -> (Option<u64>, Option<u64>) {
    (fs2::available_space(path).ok(), fs2::total_space(path).ok())
}

/// Which part of the site a check belongs to, by its stable name.
fn group_of(name: &str) -> &'static str {
    match name {
        "database" | "secret_key" | "database_size" | "disk_free" => "core",
        "search_index" | "broken_links" | "link_sweep" | "stale_locks" | "site_url" => "content",
        "storage" | "media_storage" | "object_storage" | "index_dir" | "registry_dir" | "https"
        | "smtp" => "delivery",
        "ai_budget" => "assistants",
        _ => "operations",
    }
}

/// A remedy for checks that have one and did not attach their own.
fn with_remedy(check: Check) -> Check {
    if check.action.is_some() || check.status == health::Status::Ok {
        return check;
    }
    match check.name.as_str() {
        "site_url" | "smtp" => check.link("Open settings", "/admin/settings#settings-identity"),
        "registration" => check.link("Open settings", "/admin/settings#settings-membership"),
        "media_storage" => check.link("Open media", "/admin/media"),
        "broken_links" => check.link("See posts", "/admin/posts?links=broken"),
        "ai_budget" => check.link("Open assistants", "/admin/settings#settings-assistants"),
        "plugins" => check.link("Open plugins", "/admin/plugins"),
        "job_queue" => check.link("Open webhooks", "/admin/webhooks"),
        "search_index" => check.op("Rebuild index", "reindex"),
        "stale_locks" | "expired_sessions" => check.op("Clean up now", "cleanup"),
        _ => check,
    }
}

/// `GET /api/v1/site-health`
#[utoipa::path(
    get, path = "/api/v1/site-health", tag = "operations",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Diagnostic report", body = serde_json::Value),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Report>> {
    let pool = &state.pool;

    let https = crate::feeds::origin_from_headers(&headers).starts_with("https://");
    let mut checks = crate::setup::checks(&state, https).await;

    let published = count(
        pool,
        "SELECT count(*) FROM posts WHERE status = 'published' AND type IN ('post', 'page')",
    )
    .await;
    let indexed = state.search_index.as_ref().map(|i| i.doc_count());
    let dead = count(pool, "SELECT count(*) FROM jobs WHERE status = 'dead'").await;
    let queued = count(pool, "SELECT count(*) FROM jobs WHERE status = 'queued'").await;
    let unhealthy = count(
        pool,
        "SELECT count(*) FROM plugins WHERE status IN ('errored', 'degraded')",
    )
    .await;
    let spent = vyasa_db::repo::AiLogRepo::new(pool.clone())
        .month_spend_total()
        .await
        .unwrap_or(0.0);
    // The same cap every assistant is refused past: the site option, else
    // the environment. It used to read the environment only.
    let cap = crate::ai_registry::month_cap_usd(&state)
        .await
        .unwrap_or(state.config.ai.monthly_budget_usd);
    let configured_url = state.options_service.site_url().await.unwrap_or_default();
    let broken = crate::seo::posts_with_broken_links(pool).await.unwrap_or(0);
    let media_stats = state.media.stats().await.ok();
    let media_cap = crate::rest::media::storage_cap_bytes(&state).await;
    let overdue = count(
        pool,
        "SELECT count(*) FROM posts WHERE status = 'scheduled' AND scheduled_for < now() - interval '2 minutes'",
    )
    .await;
    let last_sweep = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
        "SELECT max(checked_at) FROM link_checks",
    )
    .fetch_one(pool)
    .await
    .ok()
    .flatten();
    let stale_locks = count(
        pool,
        "SELECT count(*) FROM post_locks WHERE seen_at < now() - interval '90 seconds'",
    )
    .await;
    let expired = count(
        pool,
        "SELECT count(*) FROM sessions WHERE expires_at < now()",
    )
    .await;
    let db_bytes = sqlx::query_scalar::<_, i64>("SELECT pg_database_size(current_database())")
        .fetch_one(pool)
        .await
        .ok();
    let (free, total) = volume_space(&state.config.media_dir);
    let registration = registration_check(&state, &configured_url).await;
    let failing_hooks = vyasa_db::repo::WebhooksRepo::new(pool.clone())
        .failing_count()
        .await
        .unwrap_or(0);

    checks.extend([
        health::media_storage(media_stats.map_or(0, |s| s.bytes), media_cap),
        health::broken_links(broken),
        health::search_index(indexed, published),
        health::job_queue(dead, queued),
        health::plugins(unhealthy),
        health::webhooks(failing_hooks),
        health::ai_budget(spent, cap),
        health::site_url(&configured_url),
        health::scheduled_publisher(overdue),
        health::link_sweep(last_sweep, published),
        health::stale_locks(stale_locks),
        health::expired_sessions(expired),
        health::database_size(db_bytes),
        health::disk_free(free, total),
        registration,
    ]);
    let checks = checks
        .into_iter()
        .map(|c| {
            let group = group_of(&c.name);
            with_remedy(c.in_group(group))
        })
        .collect();
    Ok(Json(Report::new(checks)))
}

/// Registration on without a way to mail, or with a default role that no
/// longer applies (phase 98).
async fn registration_check(state: &AppState, site_url: &str) -> Check {
    let enabled = state.registration.status().await.is_ok_and(|s| s.enabled);
    let configured = state
        .options_service
        .registration_default_role()
        .await
        .unwrap_or_else(|_| String::from("subscriber"));
    let effective = state
        .registration
        .effective_default_role()
        .await
        .unwrap_or_else(|_| configured.clone());
    let unconfirmed = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .count_unconfirmed()
        .await
        .unwrap_or_default();
    health::registration(health::RegistrationSetup {
        enabled,
        mail: crate::mail::effective(state).await.is_some(),
        site_url: !site_url.trim().is_empty(),
        configured_role: &configured,
        effective_role: &effective,
        unconfirmed,
    })
}

/// `POST /api/v1/site-health/cleanup` — drops expired sessions and stale
/// editing locks now rather than waiting for their sweeps.
#[utoipa::path(
    post, path = "/api/v1/site-health/cleanup", tag = "operations",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Rows removed", body = serde_json::Value),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn cleanup(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let sessions = sqlx::query("DELETE FROM sessions WHERE expires_at < now()")
        .execute(&state.pool)
        .await
        .map_or(0, |r| r.rows_affected());
    let locks = sqlx::query("DELETE FROM post_locks WHERE seen_at < now() - interval '90 seconds'")
        .execute(&state.pool)
        .await
        .map_or(0, |r| r.rows_affected());
    Ok(Json(
        serde_json::json!({ "sessions": sessions, "locks": locks }),
    ))
}
