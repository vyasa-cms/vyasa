//! Serving the CSS and JavaScript a theme ships with itself.
//!
//! Themes could set design tokens and compose layouts, but could not
//! style anything the tokens did not already describe, nor add a line of
//! behaviour — the largest practical limit on how far a theme could go. A
//! plugin could ship assets; a theme could not.
//!
//! Content-addressed like plugin assets: the URL is the hash of the bytes,
//! so republishing identical CSS does not invalidate a visitor's cache and
//! a change is picked up the instant it lands.
//!
//! Unlike a plugin package, a `.vytheme` carries no signature — an
//! administrator with `manage_themes` uploaded it, the same trust that
//! already lets them write site-wide custom CSS. Script is a step beyond
//! that and is bounded, served from this origin, and versioned with the
//! theme, so rolling a theme back rolls its script back too.

use sha2::{Digest as _, Sha256};

use crate::state::AppState;

/// One served theme asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// File contents.
    pub body: String,
    /// MIME type.
    pub content_type: &'static str,
}

/// Stores the pictures and fonts a parsed package bundles against the
/// theme version that was just inserted.
///
/// # Errors
/// [`vyasa_common::AppError::Db`] on failure.
pub async fn store_files(
    state: &AppState,
    theme_id: i64,
    files: &[vyasa_themes::package::ThemeFile],
) -> Result<(), vyasa_common::AppError> {
    if files.is_empty() {
        return Ok(());
    }
    let inputs: Vec<vyasa_db::repo::ThemeFileInput> = files
        .iter()
        .map(|f| vyasa_db::repo::ThemeFileInput {
            path: f.path.clone(),
            content_type: f.content_type.to_owned(),
            sha256: hex::encode(Sha256::digest(&f.bytes)),
            bytes: f.bytes.clone(),
        })
        .collect();
    state.themes.put_files(theme_id, &inputs).await
}

/// A bundled file of the active theme, by its path under `assets/`.
///
/// Path-addressed rather than content-addressed, because a stylesheet
/// refers to `url(images/hero.jpg)` relative to its own URL and a font
/// face to `/theme-assets/fonts/…`; the ETag carries the content hash
/// so a revalidation after a theme change is cheap.
pub async fn lookup_file(state: &AppState, path: &str) -> Option<vyasa_db::repo::ThemeFileRow> {
    if !vyasa_themes::package::valid_bundled_path(path) {
        return None;
    }
    if let Some(active) = state.themes.get_active().await.ok().flatten() {
        if let Ok(Some(row)) = state.themes.file(active.id, path).await {
            return Some(row);
        }
    }
    None
}

/// Give preview images and fonts an explicit version, avoiding collisions
/// with the active theme. Also used on CSS before caching preview styles.
#[must_use]
pub fn preview_urls(html: &str, theme_id: Option<i64>) -> String {
    let Some(id) = theme_id else {
        return html.to_owned();
    };
    html.replace(
        "/theme-assets/images/",
        &format!("/theme-assets/version/{id}/images/"),
    )
    .replace(
        "/theme-assets/fonts/",
        &format!("/theme-assets/version/{id}/fonts/"),
    )
}

/// Serve the exact unsaved CSS and JS through authenticated, bounded preview
/// storage. Live asset lookup deliberately knows only the active version.
///
/// # Errors
/// Invalid or oversized asset documents.
pub fn preview_head(
    state: &AppState,
    assets: Option<&serde_json::Value>,
    theme_id: Option<i64>,
) -> Result<String, String> {
    use std::fmt::Write as _;
    let assets: vyasa_themes::studio::ThemeAssets = assets
        .cloned()
        .filter(|v| !v.is_null())
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("assets: {e}"))?
        .unwrap_or_default();
    assets.validate()?;
    let mut head = String::new();
    for (ext, body, content_type) in [
        (
            "css",
            preview_urls(&assets.css, theme_id),
            "text/css; charset=utf-8",
        ),
        ("js", assets.js, "text/javascript; charset=utf-8"),
    ] {
        if body.trim().is_empty() {
            continue;
        }
        let hash = hex::encode(Sha256::digest(body.as_bytes()));
        let name = format!("{hash}.{ext}");
        state
            .preview_assets
            .insert(name.clone(), Asset { body, content_type });
        if ext == "css" {
            // Keep relative url(images/...) and url(fonts/...) references
            // under the same version as the preview stylesheet.
            let path = theme_id.map_or_else(
                || format!("/theme-assets/preview/{name}"),
                |id| format!("/theme-assets/version/{id}/preview-{name}"),
            );
            let _ = write!(head, "<link rel=\"stylesheet\" href=\"{path}\">");
        } else {
            let _ = write!(
                head,
                "<script src=\"/theme-assets/preview/{name}\" defer></script>"
            );
        }
    }
    Ok(head)
}

/// The `<head>` tags for a theme's assets document.
///
/// Returns the empty string when the theme ships neither, so a theme that
/// uses only tokens costs nothing.
#[must_use]
pub fn head_tags(assets: Option<&serde_json::Value>) -> String {
    use std::fmt::Write as _;

    let Some(assets) = assets else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(hash) = hash_of(assets, "css") {
        let _ = write!(
            out,
            "\n<link rel=\"stylesheet\" href=\"/theme-assets/{hash}.css\">"
        );
    }
    if let Some(hash) = hash_of(assets, "js") {
        let _ = write!(
            out,
            "\n<script src=\"/theme-assets/{hash}.js\" defer></script>"
        );
    }
    out
}

/// The content hash of one half, when it holds anything.
fn hash_of(assets: &serde_json::Value, key: &str) -> Option<String> {
    let body = assets.get(key)?.as_str()?;
    (!body.trim().is_empty()).then(|| hex::encode(&Sha256::digest(body.as_bytes())[..8]))
}

/// Looks up a theme asset by the hash in its URL.
///
/// Reads the active theme rather than keeping a registry: a theme change
/// is rare, the row is already cached by the render path, and a stale
/// registry would serve the previous theme's stylesheet.
pub async fn lookup(state: &AppState, hash: &str, ext: &str) -> Option<Asset> {
    let active = state.themes.get_active().await.ok()??;
    let assets = active.assets?;
    let key = match ext {
        "css" => "css",
        "js" => "js",
        _ => return None,
    };
    if hash_of(&assets, key)? != hash {
        return None;
    }
    Some(Asset {
        body: assets.get(key)?.as_str()?.to_owned(),
        content_type: if key == "css" {
            "text/css; charset=utf-8"
        } else {
            "text/javascript; charset=utf-8"
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{hash_of, head_tags};
    use serde_json::json;

    #[test]
    fn a_theme_with_no_assets_costs_nothing() {
        assert_eq!(head_tags(None), "");
        assert_eq!(head_tags(Some(&json!({}))), "");
        // Whitespace is not a stylesheet.
        assert_eq!(head_tags(Some(&json!({"css": "  \n"}))), "");
    }

    #[test]
    fn tags_are_content_addressed_and_styles_precede_scripts() {
        let head = head_tags(Some(&json!({"css": "body{}", "js": "console.log(1)"})));
        let css_at = head.find("stylesheet").expect("a stylesheet");
        let js_at = head.find("<script").expect("a script");
        assert!(css_at < js_at, "{head}");
        assert!(head.contains("defer"), "{head}");

        // The same bytes give the same URL, so republishing an unchanged
        // stylesheet does not bust a visitor's year-long cache entry.
        let again = head_tags(Some(&json!({"css": "body{}", "js": "console.log(1)"})));
        assert_eq!(head, again);
        let changed = head_tags(Some(&json!({"css": "body{color:red}"})));
        assert_ne!(head, changed);
    }

    #[test]
    fn each_half_is_hashed_on_its_own() {
        let assets = json!({"css": "a{}", "js": "b()"});
        assert_ne!(hash_of(&assets, "css"), hash_of(&assets, "js"));
        assert!(hash_of(&assets, "nope").is_none());
        assert!(hash_of(&json!({"css": ""}), "css").is_none());
    }
}
