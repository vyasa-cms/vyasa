//! Update endpoints for the admin panel.
//!
//! The apply endpoint cannot hold its own HTTP request: the process it
//! upgrades is the one serving the request. So it spawns a detached
//! `vyasa update apply --yes` and returns immediately; the admin polls
//! [`status`] (which reads the run file written beside the binary) and
//! then `/api/v1/version` until the new build answers.

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use vyasa_common::AppError;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::state::AppState;
use crate::update;

/// `GET /api/v1/updates` — version, channel, deployment and preflight.
///
/// # Errors
/// 403 without `ManageOptions`.
#[utoipa::path(
    get, path = "/api/v1/updates", tag = "updates",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Update status", body = update::UpdateStatus),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn get(State(state): State<AppState>) -> ApiResult<Json<update::UpdateStatus>> {
    // Upgrading is an owner-level act, and the payload names filesystem
    // paths; the route keeps both behind the options capability.
    Ok(Json(update::status(&state).await))
}

/// What to install.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ApplyRequest {
    /// Version to install; defaults to the newest offered.
    #[serde(default)]
    pub version: Option<String>,
    /// Skip the database dump. Migrations are forward-only, so this
    /// discards the only way back.
    #[serde(default)]
    pub skip_backup: bool,
}

/// `POST /api/v1/updates/apply` — start an upgrade in a detached process.
///
/// # Errors
/// 403 without `ManageOptions`, or for a caller who is not a full
/// administrator (the upgrade replaces the binary the site runs); 400
/// when this deployment cannot upgrade itself or the preflight failed.
#[utoipa::path(
    post, path = "/api/v1/updates/apply", tag = "updates",
    security(("session_cookie" = [])),
    request_body = ApplyRequest,
    responses(
        (status = 202, description = "Upgrade started", body = update::apply::Progress),
        (status = 400, description = "Refused", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn apply(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<ApplyRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<update::apply::Progress>)> {
    crate::policy::full_administrator(&principal, "upgrade the site")?;
    let (release, checks, env) = update::plan_for(&state, body.version.as_deref()).await?;
    if !env.mode.can_self_update() || !env.binary_replaceable {
        return Err(ApiError(AppError::validation(format!(
            "this install is upgraded from outside ({:?}); run: {}",
            env.mode,
            env.mode.instructions(&release.version).join(" && ")
        ))));
    }
    if !checks.can_proceed {
        let blockers: Vec<&str> = checks
            .findings
            .iter()
            .filter(|f| f.status == vyasa_core::health::Status::Fail)
            .map(|f| f.detail.as_str())
            .collect();
        return Err(ApiError(AppError::validation(format!(
            "preflight failed: {}",
            blockers.join("; ")
        ))));
    }

    let binary = std::env::current_exe()
        .map_err(|e| ApiError(AppError::internal_msg(format!("no binary path: {e}"))))?;
    let mut command = std::process::Command::new(&binary);
    command.arg("update").arg("apply").arg("--yes");
    command.arg("--to").arg(&release.version);
    // The updater restarts *this* process once the swap lands: SIGTERM,
    // which now drains in-flight requests, then a health gate on the new
    // build with an automatic rollback if it does not come up.
    command
        .arg("--restart-pid")
        .arg(std::process::id().to_string());
    if body.skip_backup {
        command.arg("--no-backup");
    }
    // Detached: this process is about to be replaced, and a child that
    // dies with it would leave a half-applied upgrade.
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
        .spawn()
        .map_err(|e| ApiError(AppError::internal_msg(format!("cannot start updater: {e}"))))?;

    crate::audit::record(
        &state,
        principal.user(),
        "update.apply",
        format!("version:{}", release.version),
        serde_json::json!({"skip_backup": body.skip_backup}),
    );

    let progress = update::apply::Progress::new(
        &release.version,
        update::apply::Stage::Preflight,
        "upgrade started",
    );
    progress.write_beside(&binary);
    Ok((axum::http::StatusCode::ACCEPTED, Json(progress)))
}

/// `GET /api/v1/updates/status` — how a running upgrade is going.
///
/// # Errors
/// 403 without `ManageOptions`.
#[utoipa::path(
    get, path = "/api/v1/updates/status", tag = "updates",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Progress, null when no run is recorded",
         body = Option<update::apply::Progress>),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn run_status(
    _state: State<AppState>,
) -> ApiResult<Json<Option<update::apply::Progress>>> {
    let progress = std::env::current_exe()
        .ok()
        .and_then(|binary| update::apply::Progress::read_beside(&binary));
    Ok(Json(progress))
}
