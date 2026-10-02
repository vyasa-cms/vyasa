//! The theme studio's draft lifecycle: start a working copy, apply
//! operations to it, walk its history, publish it as a theme version.
//!
//! Every edit — from the editor or the assistant — goes through
//! [`apply_ops`], so there is one validation path and one revision log.

use serde_json::{json, Value};
use vyasa_common::AppError;
use vyasa_db::repo::themes::ThemeRow;
use vyasa_db::repo::{DraftDocuments, ThemeDraftRow, ThemeDraftsRepo};
use vyasa_themes::{DraftState, StudioOp, TokenDiagnostic};

use crate::state::AppState;

fn repo(state: &AppState) -> ThemeDraftsRepo {
    ThemeDraftsRepo::new(state.pool.clone())
}

/// Where a new draft's documents come from.
pub enum Start {
    /// Copy an installed theme version.
    Theme(i64),
    /// Copy whatever is live; fall back to the built-in defaults when no
    /// theme is active yet.
    Active,
}

/// Starts a working copy.
///
/// # Errors
/// [`AppError::NotFound`] for an unknown base theme, [`AppError::Db`] on
/// failure.
pub async fn create_draft(
    state: &AppState,
    name: Option<&str>,
    start: Start,
    created_by: Option<i64>,
) -> Result<ThemeDraftRow, AppError> {
    let base: Option<ThemeRow> = match start {
        Start::Theme(id) => Some(state.themes.get(id).await?),
        Start::Active => state.themes.get_active().await?,
    };
    let (tokens, layout, templates, assets, base_id, default_name) = if let Some(row) = &base {
        (
            row.tokens.clone(),
            row.layout.clone(),
            row.templates.clone(),
            row.assets.clone(),
            Some(row.id),
            format!("{} (draft)", row.name),
        )
    } else {
        let starter = crate::theme_defaults::embedded_packages()
            .into_iter()
            .next()
            .ok_or_else(|| AppError::internal_msg("no starter theme embedded"))?;
        let parsed = vyasa_themes::package::parse_vytheme(&starter.bytes)
            .map_err(|e| AppError::internal_msg(format!("starter: {e}")))?;
        (
            parsed.tokens_json,
            parsed.layout_json,
            Some(json!(parsed.templates)),
            parsed.assets_json,
            None,
            "New theme".to_owned(),
        )
    };
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or(default_name, str::to_owned);
    repo(state)
        .create(
            &name,
            base_id,
            DraftDocuments {
                tokens: &tokens,
                layout: &layout,
                templates: templates.as_ref(),
                assets: assets.as_ref(),
            },
            created_by,
        )
        .await
}

/// Applies `ops` to a draft as one revision.
///
/// # Errors
/// [`AppError::Validation`] carrying every diagnostic when the result is
/// invalid (the draft is untouched); [`AppError::NotFound`] for an unknown
/// draft; [`AppError::Db`] on failure.
pub async fn apply_ops(
    state: &AppState,
    draft_id: i64,
    ops: &[StudioOp],
    source: &str,
    note: Option<&str>,
) -> Result<(ThemeDraftRow, Vec<TokenDiagnostic>), AppError> {
    let repo = repo(state);
    let row = repo.get(draft_id).await?;
    let current = DraftState::from_json_with_assets(
        &row.tokens,
        &row.layout,
        row.templates.as_ref(),
        row.assets.as_ref(),
    )
    .map_err(|e| AppError::internal_msg(format!("stored draft is unreadable: {e}")))?;
    let registry = crate::plugin_sections::registry_with_plugins(state).await;
    let applied = vyasa_themes::apply_studio_ops_with(&current, ops, &registry)
        .map_err(|diags| AppError::validation(diagnostics_message(&diags)))?;
    if applied.changes.is_empty() {
        // Nothing effective (an explanation, or a no-op): no revision.
        return Ok((row, applied.warnings));
    }
    let note = note.map_or_else(|| applied.changes.join("; "), str::to_owned);
    let tokens = applied.state.tokens_json();
    let layout = applied.state.layout_json();
    let templates = applied.state.templates_json();
    let assets = applied.state.assets_json();
    let row = repo
        .commit_revision(
            draft_id,
            DraftDocuments {
                tokens: &tokens,
                layout: &layout,
                templates: templates.as_ref(),
                assets: assets.as_ref(),
            },
            &note,
            source,
        )
        .await?;
    Ok((row, applied.warnings))
}

/// Makes an earlier revision current, as a new revision (history is
/// append-only, so a revert can itself be undone).
///
/// # Errors
/// [`AppError::NotFound`] for an unknown draft or sequence.
pub async fn revert(state: &AppState, draft_id: i64, seq: i32) -> Result<ThemeDraftRow, AppError> {
    let repo = repo(state);
    let target = repo.revision(draft_id, seq).await?;
    repo.commit_revision(
        draft_id,
        DraftDocuments {
            tokens: &target.tokens,
            layout: &target.layout,
            templates: target.templates.as_ref(),
            // History carries assets too, so going back to a revision
            // restores the stylesheet that shipped with it.
            assets: target.assets.as_ref(),
        },
        &format!("Went back to revision {seq}"),
        "revert",
    )
    .await
}

/// Installs the draft as the next version of theme `name`, optionally
/// making it live. The draft stays, so publishing is repeatable.
///
/// # Errors
/// [`AppError::Validation`] for a bad name or an invalid draft,
/// [`AppError::Db`] on failure.
pub async fn publish(
    state: &AppState,
    draft_id: i64,
    name: &str,
    activate: bool,
) -> Result<ThemeRow, AppError> {
    if !vyasa_themes::package::valid_name(name) {
        return Err(AppError::validation(format!(
            "theme name \"{name}\" must be 1-60 lowercase letters, digits or dashes"
        )));
    }
    let row = repo(state).get(draft_id).await?;
    // Re-run the installer's checks: the draft was validated on every
    // write, but the registry may have changed since.
    let current = DraftState::from_json_with_assets(
        &row.tokens,
        &row.layout,
        row.templates.as_ref(),
        row.assets.as_ref(),
    )
    .map_err(|e| AppError::validation(format!("draft is no longer valid: {e}")))?;
    vyasa_themes::apply_studio_ops(&current, &[])
        .map_err(|diags| AppError::validation(diagnostics_message(&diags)))?;
    let version = state.themes.latest_version(name).await? + 1;
    let mut theme = state
        .themes
        .insert_version(
            name,
            version,
            row.tokens.clone(),
            row.layout.clone(),
            row.templates.clone(),
            // Publishing carries the draft's own stylesheet and script
            // with it, so what was previewed is what goes live.
            row.assets.clone(),
            row.base_theme_id,
        )
        .await?;
    // The pictures and fonts the base version bundled go with the new
    // version; the studio edits tokens, layout, templates and the two
    // text assets, never the files, so the base's set is the right one.
    // A draft whose base version was deleted since (the column nulls on
    // delete) still belongs to a named theme: its latest installed version
    // is the closest thing to the base it lost.
    let source = match row.base_theme_id {
        Some(base) => Some(base),
        None => state
            .themes
            .latest_below(name, version)
            .await
            .ok()
            .map(|r| r.id),
    };
    if let Some(source) = source {
        state.themes.copy_files(source, theme.id).await?;
        // An edit of an installed package is still that package, for the
        // marketplace's update check.
        if let Ok(base) = state.themes.get(source).await {
            if let Some(pv) = base.package_version {
                state.themes.set_package_version(theme.id, pv).await?;
                theme.package_version = Some(pv);
            }
        }
    }
    if activate {
        state.themes.set_active(name, version).await?;
        crate::rest::themes::after_activate(state, name, version);
    }
    Ok(theme)
}

/// Every diagnostic on one line each, for a validation error body.
#[must_use]
pub fn diagnostics_message(diags: &[TokenDiagnostic]) -> String {
    diags
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Diagnostics as JSON for the editor to place inline.
#[must_use]
pub fn diagnostics_json(diags: &[TokenDiagnostic]) -> Value {
    Value::Array(
        diags
            .iter()
            .map(|d| match d {
                TokenDiagnostic::Error { path, message } => {
                    json!({"level": "error", "path": path, "message": message})
                }
                TokenDiagnostic::Warning { path, message } => {
                    json!({"level": "warning", "path": path, "message": message})
                }
            })
            .collect(),
    )
}

/// The draft as the API presents it.
#[must_use]
pub fn draft_json(row: &ThemeDraftRow, warnings: &[TokenDiagnostic]) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "base_theme_id": row.base_theme_id,
        "status": row.status,
        "status_note": row.status_note,
        "tokens": row.tokens,
        "layout": row.layout,
        "templates": row.templates.clone().unwrap_or_else(|| json!({})),
        // Null rather than an empty object: a theme that ships no assets
        // is different from one whose stylesheet is empty, and the studio
        // reads the difference.
        "assets": row.assets,
        "revision": row.revision,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "warnings": diagnostics_json(warnings),
    })
}

/// The lightweight shape for lists.
#[must_use]
pub fn draft_summary_json(row: &ThemeDraftRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "base_theme_id": row.base_theme_id,
        "status": row.status,
        "status_note": row.status_note,
        "revision": row.revision,
        "updated_at": row.updated_at,
        "colors": row.tokens.get("colors").cloned().unwrap_or(Value::Null),
    })
}

/// Warnings for the stored documents, so a freshly loaded draft shows the
/// same badges an edit would.
#[must_use]
pub fn current_warnings(row: &ThemeDraftRow) -> Vec<TokenDiagnostic> {
    DraftState::from_json_with_assets(
        &row.tokens,
        &row.layout,
        row.templates.as_ref(),
        row.assets.as_ref(),
    )
    .ok()
    .and_then(|s| vyasa_themes::apply_studio_ops(&s, &[]).ok())
    .map(|a| a.warnings)
    .unwrap_or_default()
}
