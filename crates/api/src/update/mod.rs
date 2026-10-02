//! Upgrading a running install.
//!
//! Two rules shape everything here:
//!
//! 1. **The CMS always knows, and only acts where it safely can.** A
//!    container cannot replace its own image, so in Docker and Kubernetes
//!    this feature detects, checks and *instructs*. A self-managed binary
//!    can replace itself, so there it does the work.
//! 2. **Nothing irreversible happens until everything reversible has
//!    succeeded.** Verify, then back up, then swap, then migrate — and
//!    keep the outgoing binary until the new one has answered.
//!
//! What is *not* at risk is worth stating, because it is the reason this
//! is safer than the CMS everyone compares it to: themes and plugins are
//! rows in Postgres, not files in a web root. An upgrade cannot half-
//! overwrite them. The state that can genuinely be lost lives on disk —
//! uploads and the search index — which is what [`preflight`] guards.

pub mod apply;
pub mod manifest;
pub mod mode;
pub mod preflight;

use serde::Serialize;
use vyasa_common::AppError;

use crate::state::AppState;

/// Option holding hex ed25519 public keys that may sign releases.
pub const TRUSTED_KEYS_OPTION: &str = "update_trusted_keys";

/// What the admin panel and `vyasa update check` both render.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UpdateStatus {
    /// The running version.
    pub current: String,
    /// Newest offered by the channel, when one is configured and reachable.
    pub latest: Option<String>,
    /// True when `latest` is newer than `current`.
    pub update_available: bool,
    /// Releases between the two, newest first.
    pub releases: Vec<manifest::Release>,
    /// How this install is deployed, and what may be done to it.
    pub environment: mode::Environment,
    /// Commands to run when this install upgrades from outside.
    pub instructions: Vec<String>,
    /// Preflight for the newest release (or general readiness).
    pub preflight: preflight::Preflight,
    /// Why no channel answered, when none did.
    pub channel_error: Option<String>,
}

/// Reads an option holding a list of strings (or one comma-separated).
async fn string_list(state: &AppState, key: &str) -> Vec<String> {
    match state.options.get(key).await {
        Ok(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Ok(serde_json::Value::String(one)) => one
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Release keys this install trusts.
pub async fn trusted_keys(state: &AppState) -> Vec<String> {
    string_list(state, TRUSTED_KEYS_OPTION).await
}

/// Gathers everything without changing anything.
///
/// A missing or unreachable channel is reported in `channel_error` rather
/// than failing the call: an operator still wants the preflight and the
/// deployment facts when the network is down.
pub async fn status(state: &AppState) -> UpdateStatus {
    let environment = mode::detect();
    let channel = state
        .options
        .get(manifest::CHANNEL_OPTION)
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();

    let (manifest, channel_error) = match manifest::fetch(&channel).await {
        Ok(m) => (Some(m), None),
        Err(err) => (None, Some(err.to_string())),
    };

    let releases: Vec<manifest::Release> = manifest
        .as_ref()
        .map(|m| {
            m.newer_than(manifest::CURRENT_VERSION)
                .into_iter()
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let latest = manifest.as_ref().map(|m| m.latest.clone());
    // Trust the comparison, not the channel's own claim: a manifest whose
    // `latest` is older than what is installed must not read as an update.
    let latest_is_newer = latest
        .as_deref()
        .and_then(|l| manifest::is_newer(l, manifest::CURRENT_VERSION))
        .unwrap_or(false);
    let target = releases.first();
    let checks = preflight::run(state, &environment, target).await;
    let instructions = target.map_or_else(Vec::new, |r| environment.mode.instructions(&r.version));

    UpdateStatus {
        current: manifest::CURRENT_VERSION.to_owned(),
        update_available: latest_is_newer && !releases.is_empty(),
        latest,
        releases,
        environment,
        instructions,
        preflight: checks,
        channel_error,
    }
}

/// Reads what a target release would mean, refusing early when the step
/// is not allowed or this install cannot perform it.
///
/// # Errors
/// [`AppError::Validation`] when the version is unknown, unreachable from
/// here, has no build for this host, or the preflight failed.
pub async fn plan_for(
    state: &AppState,
    version: Option<&str>,
) -> Result<(manifest::Release, preflight::Preflight, mode::Environment), AppError> {
    let environment = mode::detect();
    let channel = state
        .options
        .get(manifest::CHANNEL_OPTION)
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let doc = manifest::fetch(&channel).await?;
    let release = match version {
        Some(v) => doc
            .release(v)
            .cloned()
            .ok_or_else(|| AppError::validation(format!("no release {v} in this channel")))?,
        None => doc
            .newer_than(manifest::CURRENT_VERSION)
            .first()
            .map(|r| (*r).clone())
            .ok_or_else(|| AppError::validation("already up to date"))?,
    };
    let checks = preflight::run(state, &environment, Some(&release)).await;
    Ok((release, checks, environment))
}
