//! Marketplace endpoints: browse, install, and see what has updates.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_common::AppError;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::registry::index::{self, Kind, Listing};
use crate::state::AppState;

/// One listing as the browse UI needs it: the catalogue entry plus what
/// this site already has.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct BrowseEntry {
    /// The catalogue entry.
    #[serde(flatten)]
    pub listing: Listing,
    /// Installed version, when this site has one.
    pub installed_version: Option<String>,
    /// True when the newest offered version is later than the installed one.
    pub update_available: bool,
    /// Capabilities an update would add that the installed version does
    /// not already hold. Empty for a theme, and empty for an update that
    /// asks for nothing new.
    pub new_capabilities: Vec<String>,
}

/// Browse filter.
#[derive(Debug, Deserialize, utoipa::IntoParams, Default)]
pub struct BrowseQuery {
    /// `plugin` or `theme`; both when absent.
    pub kind: Option<String>,
    /// Case-insensitive substring over title, name and summary.
    pub q: Option<String>,
}

/// What the browse endpoint returns.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct BrowseResponse {
    /// Whether a marketplace is configured at all.
    pub configured: bool,
    /// Official, an operator's mirror, or off.
    pub state: crate::official::SourceState,
    /// Why the marketplace could not be read, when it could not.
    pub error: Option<String>,
    /// Matching listings.
    pub entries: Vec<BrowseEntry>,
}

/// Installed plugins as `name -> (version, capabilities)`.
async fn installed_plugins(
    state: &AppState,
) -> std::collections::HashMap<String, (String, Vec<String>)> {
    state
        .plugins_repo
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let caps = p
                .capabilities
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|c| c.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            (p.name, (p.version, caps))
        })
        .collect()
}

/// Installed themes as `name -> newest installed version`.
async fn installed_themes(state: &AppState) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in state.themes.list().await.unwrap_or_default() {
        // Compare what the package said, not the site's own sequence.
        let version = row.package_version.unwrap_or(row.version).to_string();
        out.entry(row.name)
            .and_modify(|current| {
                if index::is_newer(&version, current) {
                    current.clone_from(&version);
                }
            })
            .or_insert(version);
    }
    out
}

/// `GET /api/v1/registry` — what the marketplace offers.
///
/// # Errors
/// 403 without `ManagePlugins` or `ManageThemes`.
#[utoipa::path(
    get, path = "/api/v1/registry", tag = "registry",
    security(("session_cookie" = [])),
    params(BrowseQuery),
    responses(
        (status = 200, description = "Marketplace listings", body = BrowseResponse),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn browse(
    State(state): State<AppState>,
    Query(query): Query<BrowseQuery>,
) -> ApiResult<Json<BrowseResponse>> {
    let source = crate::registry::source(&state);
    if source.state == crate::official::SourceState::Off {
        return Ok(Json(BrowseResponse {
            configured: false,
            state: source.state,
            error: None,
            entries: Vec::new(),
        }));
    }
    let doc = match index::fetch(&source.url).await {
        Ok(doc) => doc,
        Err(err) => {
            return Ok(Json(BrowseResponse {
                configured: true,
                state: source.state,
                error: Some(err.to_string()),
                entries: Vec::new(),
            }))
        }
    };

    let want: Option<Kind> = match query.kind.as_deref() {
        Some("plugin") => Some(Kind::Plugin),
        Some("theme") => Some(Kind::Theme),
        _ => None,
    };
    let mut listings = Vec::new();
    if want != Some(Kind::Theme) {
        listings.extend(doc.of_kind(Kind::Plugin));
    }
    if want != Some(Kind::Plugin) {
        listings.extend(doc.of_kind(Kind::Theme));
    }
    let needle = query.q.unwrap_or_default().to_lowercase();
    if !needle.is_empty() {
        listings.retain(|l| {
            l.name.to_lowercase().contains(&needle)
                || l.title.to_lowercase().contains(&needle)
                || l.summary.to_lowercase().contains(&needle)
        });
    }

    let plugins = installed_plugins(&state).await;
    let themes = installed_themes(&state).await;
    let entries = listings
        .into_iter()
        .map(|listing| {
            let (installed_version, installed_caps) = match listing.kind {
                Kind::Plugin => plugins
                    .get(&listing.name)
                    .map(|(v, caps)| (Some(v.clone()), caps.clone()))
                    .unwrap_or_default(),
                Kind::Theme => (themes.get(&listing.name).cloned(), Vec::new()),
            };
            let latest = listing.latest().cloned();
            let update_available = match (&installed_version, &latest) {
                (Some(installed), Some(offered)) => index::is_newer(&offered.version, installed),
                _ => false,
            };
            // The capability diff is the point of the whole panel: an
            // update that wants powers the installed version never had is
            // a decision, not a routine upgrade.
            let new_capabilities = if update_available {
                latest.as_ref().map_or_else(Vec::new, |offered| {
                    offered
                        .capabilities
                        .iter()
                        .filter(|c| !installed_caps.contains(c))
                        .cloned()
                        .collect()
                })
            } else {
                Vec::new()
            };
            BrowseEntry {
                listing,
                installed_version,
                update_available,
                new_capabilities,
            }
        })
        .collect();

    Ok(Json(BrowseResponse {
        configured: true,
        state: source.state,
        error: None,
        entries,
    }))
}

/// What to install.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InstallRequest {
    /// `plugin` or `theme`.
    pub kind: String,
    /// Package name as the marketplace lists it.
    pub name: String,
    /// Version to install; the newest offered when absent.
    #[serde(default)]
    pub version: Option<String>,
    /// Capabilities the operator saw and accepted. When present, the
    /// install refuses if the package asks for anything not in this list
    /// — so a catalogue edited between the browse and the click cannot
    /// grant powers nobody agreed to.
    #[serde(default)]
    pub accept_capabilities: Option<Vec<String>>,
}

/// `POST /api/v1/registry/install` — install from the marketplace.
///
/// # Errors
/// 403 without the capability the listing's kind needs; 400 when the
/// listing is unknown, the download fails verification, or the package
/// asks for capabilities the caller did not accept.
#[utoipa::path(
    post, path = "/api/v1/registry/install", tag = "registry",
    security(("session_cookie" = [])),
    request_body = InstallRequest,
    responses(
        (status = 201, description = "Installed", body = crate::registry::install::Installed),
        (status = 400, description = "Refused", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn install(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<InstallRequest>,
) -> ApiResult<(
    axum::http::StatusCode,
    Json<crate::registry::install::Installed>,
)> {
    let kind = match body.kind.as_str() {
        "plugin" => Kind::Plugin,
        "theme" => Kind::Theme,
        other => {
            return Err(ApiError(AppError::validation(format!(
                "unknown kind {other:?}; expected \"plugin\" or \"theme\""
            ))))
        }
    };
    principal.ensure(kind.capability())?;
    // A plugin install must say what the caller agreed to — before anything
    // is fetched. The index's claim is checked against it below as a fast
    // refusal; the install re-checks what the package itself declares.
    if kind == Kind::Plugin && body.accept_capabilities.is_none() {
        return Err(ApiError(AppError::validation(
            "installing a plugin requires accept_capabilities: the capabilities you agree to grant",
        )));
    }

    let source = crate::registry::source(&state);
    if source.state == crate::official::SourceState::Off {
        return Err(ApiError(AppError::validation(
            "the marketplace is turned off on this server",
        )));
    }
    let doc = index::fetch(&source.url).await?;
    let listing = doc.find(kind, &body.name).ok_or_else(|| {
        ApiError(AppError::validation(format!(
            "the marketplace lists no {} called \"{}\"",
            body.kind, body.name
        )))
    })?;
    let version = match body.version.as_deref() {
        Some(v) => listing.version(v).cloned().ok_or_else(|| {
            ApiError(AppError::validation(format!(
                "\"{}\" has no version {v}",
                body.name
            )))
        })?,
        None => listing.latest().cloned().ok_or_else(|| {
            ApiError(AppError::validation(format!(
                "\"{}\" has no versions to install",
                body.name
            )))
        })?,
    };

    if let Some(accepted) = &body.accept_capabilities {
        let unexpected: Vec<&String> = version
            .capabilities
            .iter()
            .filter(|c| !accepted.contains(c))
            .collect();
        if !unexpected.is_empty() {
            return Err(ApiError(AppError::validation(format!(
                "this package asks for capabilities you did not accept: {}",
                unexpected
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))));
        }
    }

    let bytes = crate::registry::install::download(&state, &version).await?;
    let accepted = body.accept_capabilities.as_deref().unwrap_or_default();
    let installed =
        crate::registry::install::install(&state, &listing, &version, &bytes, accepted).await?;

    crate::audit::record(
        &state,
        principal.user(),
        "registry.install",
        format!("{}:{}@{}", body.kind, installed.name, installed.version),
        serde_json::json!({ "capabilities": installed.capabilities }),
    );
    Ok((axum::http::StatusCode::CREATED, Json(installed)))
}
