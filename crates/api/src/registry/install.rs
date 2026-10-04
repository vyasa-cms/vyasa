//! Installing from the marketplace.
//!
//! Downloaded packages go through the *same* parser and the same
//! persistence as a file an operator uploads by hand — the registry adds
//! discovery and distribution trust, never a second install path with a
//! second set of rules.

use serde::Serialize;
use vyasa_common::AppError;

use super::index::{Kind, Listing, Version};
use crate::state::AppState;

/// Largest package the marketplace may hand us.
const MAX_PACKAGE_BYTES: usize = 25 * 1024 * 1024;

/// What an install did.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Installed {
    /// What was installed.
    pub kind: Kind,
    /// Package name.
    pub name: String,
    /// Version installed.
    pub version: String,
    /// Capabilities the package now holds (plugins only).
    pub capabilities: Vec<String>,
    /// True when a plugin needs enabling before it does anything.
    pub needs_enabling: bool,
}

/// Downloads and verifies one package.
///
/// # Errors
/// [`AppError::Validation`] when the digest or signature does not match;
/// [`AppError::Internal`] on transport failure.
pub async fn download(state: &AppState, version: &Version) -> Result<Vec<u8>, AppError> {
    if !crate::official::transport_ok(&version.url) {
        return Err(AppError::validation(
            "marketplace packages must be served over https",
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| AppError::internal_msg(e.to_string()))?;
    let response = client
        .get(&version.url)
        .send()
        .await
        .map_err(|e| AppError::internal_msg(format!("download failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::internal_msg(format!("download failed: {e}")))?;
    // Counted while it streams: a package past the cap is abandoned at
    // the cap, not read whole into memory and measured afterwards.
    let bytes = crate::net_guard::read_capped(response, MAX_PACKAGE_BYTES)
        .await
        .map_err(|e| match e {
            crate::net_guard::BodyError::TooLarge => AppError::validation(format!(
                "package is larger than the {MAX_PACKAGE_BYTES}-byte limit"
            )),
            crate::net_guard::BodyError::Transport(e) => {
                AppError::internal_msg(format!("download truncated: {e}"))
            }
        })?;
    let keys = super::source(state).keys;
    crate::signing::verify_registry_download(
        &bytes,
        &version.sha256,
        version.signature.as_deref(),
        &keys,
        &version.url,
    )?;
    Ok(bytes)
}

/// Installs a verified package.
///
/// The listing's declared name must match the name inside the package:
/// a registry that could rename a package on the way in could shadow one
/// an operator already trusts.
///
/// `accepted` is what the operator agreed to grant. The index's own
/// capability list is only a claim; what gets stored is what the package
/// declares, so that is what must fall within `accepted`.
///
/// # Errors
/// [`AppError::Validation`] on a package that does not parse, a name
/// mismatch, or (for plugins) a signature no configured key vouches for
/// or a capability the operator did not accept.
pub async fn install(
    state: &AppState,
    listing: &Listing,
    version: &Version,
    bytes: &[u8],
    accepted: &[String],
) -> Result<Installed, AppError> {
    match listing.kind {
        Kind::Plugin => install_plugin(state, listing, version, bytes, accepted).await,
        Kind::Theme => install_theme(state, listing, bytes).await,
    }
}

async fn install_plugin(
    state: &AppState,
    listing: &Listing,
    version: &Version,
    bytes: &[u8],
    accepted: &[String],
) -> Result<Installed, AppError> {
    // The author signature is checked against the key the listing names;
    // the marketplace signature already vouched for the bytes, so no key
    // of the author's needs to be on this site.
    let author_keys: Vec<String> = listing
        .author_key
        .clone()
        .map_or_else(|| super::source(state).keys, |k| vec![k]);
    let keys = crate::signing::parse_keys(&author_keys);
    let parsed = vyasa_plugins::package::parse_rpplugin(bytes, &keys).map_err(|e| {
        AppError::validation(format!("the plugin's author signature did not verify: {e}"))
    })?;
    if parsed.manifest.name != listing.name {
        return Err(AppError::validation(format!(
            "the marketplace lists this as \"{}\" but the package is \"{}\"",
            listing.name, parsed.manifest.name
        )));
    }
    if parsed.manifest.version != version.version {
        return Err(AppError::validation(format!(
            "the marketplace offers {} but the package is {}",
            version.version, parsed.manifest.version
        )));
    }
    let unaccepted = unaccepted_capabilities(&parsed.capabilities, accepted);
    if !unaccepted.is_empty() {
        return Err(AppError::validation(format!(
            "the package asks for capabilities you did not accept: {}",
            unaccepted.join(", ")
        )));
    }
    let capabilities: Vec<String> = parsed
        .capabilities
        .iter()
        .map(ToString::to_string)
        .collect();
    let caps_json = serde_json::json!(capabilities);
    let sha = vyasa_plugins::host::sha256_hex(&parsed.wasm);
    let row = vyasa_plugins::lifecycle::install(
        &state.plugins_repo,
        &parsed.manifest.name,
        &parsed.manifest.version,
        &parsed.wasm,
        &sha,
        &caps_json,
    )
    .await?;
    let _ =
        crate::rest::plugins::refresh(state, row.id, crate::plugin_surface::Collect::Boot).await;
    Ok(Installed {
        kind: Kind::Plugin,
        name: row.name,
        version: row.version,
        capabilities,
        // Installed plugins stay inert until an operator enables them:
        // seeing the capability list and then choosing is the point.
        needs_enabling: !row.enabled,
    })
}

/// The package's capabilities that nothing in `accepted` covers. A
/// capability is covered by an accepted one that implies it (the same rule
/// the broker enforces at run time), so accepting `net:fetch:*.example.com`
/// covers a package asking for `net:fetch:api.example.com`.
fn unaccepted_capabilities(
    declared: &[vyasa_plugins::capabilities::Capability],
    accepted: &[String],
) -> Vec<String> {
    let accepted: Vec<vyasa_plugins::capabilities::Capability> = accepted
        .iter()
        .filter_map(|a| vyasa_plugins::capabilities::Capability::parse(a).ok())
        .collect();
    declared
        .iter()
        .filter(|c| !accepted.iter().any(|a| a.implies(c)))
        .map(ToString::to_string)
        .collect()
}

async fn install_theme(
    state: &AppState,
    listing: &Listing,
    bytes: &[u8],
) -> Result<Installed, AppError> {
    let parsed = vyasa_themes::package::parse_vytheme(bytes)
        .map_err(|e| AppError::validation(e.to_string()))?;
    if parsed.manifest.name != listing.name {
        return Err(AppError::validation(format!(
            "the marketplace lists this as \"{}\" but the package is \"{}\"",
            listing.name, parsed.manifest.name
        )));
    }
    let templates_json = if parsed.templates.is_empty() {
        None
    } else {
        Some(
            serde_json::to_value(&parsed.templates)
                .map_err(|e| AppError::internal_msg(format!("serialize templates: {e}")))?,
        )
    };
    let package_version = i32::try_from(parsed.manifest.version).unwrap_or(i32::MAX);
    if state
        .themes
        .has_package_version(&parsed.manifest.name, package_version)
        .await?
    {
        return Err(AppError::conflict(format!(
            "theme {} v{package_version} already installed",
            parsed.manifest.name
        )));
    }
    let version = state.themes.latest_version(&parsed.manifest.name).await? + 1;
    let row = state
        .themes
        .insert_version(
            &parsed.manifest.name,
            version,
            parsed.tokens_json,
            parsed.layout_json,
            templates_json,
            parsed.assets_json,
            None,
        )
        .await?;
    state
        .themes
        .set_package_version(row.id, package_version)
        .await?;
    crate::theme_assets::store_files(state, row.id, &parsed.files).await?;
    Ok(Installed {
        kind: Kind::Theme,
        name: row.name,
        version: row.version.to_string(),
        capabilities: Vec::new(),
        // A theme is inert until someone activates it, which is a
        // separate, reversible act with its own rollback.
        needs_enabling: !row.is_active,
    })
}

#[cfg(test)]
mod tests {
    use super::unaccepted_capabilities;
    use vyasa_plugins::capabilities::Capability;

    fn caps(raw: &[&str]) -> Vec<Capability> {
        raw.iter().map(|c| Capability::parse(c).unwrap()).collect()
    }

    fn strings(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_package_not_the_index_decides_what_needs_consent() {
        // The index claimed only `log:write`, so that is what the operator
        // accepted; the package itself also asks for posts and the network.
        let declared = caps(&["log:write", "db:read:posts", "net:fetch:evil.example"]);
        let missing = unaccepted_capabilities(&declared, &strings(&["log:write"]));
        assert_eq!(
            missing,
            strings(&["db:read:posts", "net:fetch:evil.example"])
        );
    }

    #[test]
    fn accepted_capabilities_cover_by_implication() {
        let declared = caps(&["net:fetch:api.example.com", "kv:read"]);
        let accepted = strings(&["net:fetch:*.example.com", "kv:write"]);
        assert!(unaccepted_capabilities(&declared, &accepted).is_empty());
        assert_eq!(
            unaccepted_capabilities(&declared, &[]),
            strings(&["net:fetch:api.example.com", "kv:read"])
        );
    }
}
