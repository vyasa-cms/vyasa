//! `.vytheme` package format: zip archive with manifest + tokens + layout +
//! optional Tera overrides.
//!
//! Archive layout:
//!
//! ```text
//! manifest.toml      # name, version, author, required_api = 1
//! tokens.json        # design-token document (schema v1)
//! layout.json        # layout composition (phase 21 schema)
//! templates/*.tera   # optional Tera overrides
//! assets/theme.css   # optional stylesheet the theme ships with itself
//! assets/theme.js    # optional script
//! assets/images/*    # optional pictures, served at /theme-assets/images/…
//! assets/fonts/*     # optional web fonts, served at /theme-assets/fonts/…
//! screenshot.png     # optional, <=1 MiB
//! ```
//!
//! Hardening: total size and entry-count caps; path-traversal rejection;
//! duplicate-entry rejection; everything validated at install time so the
//! renderer can trust stored themes.

use std::collections::BTreeMap;
use std::io::Read;

use serde::Deserialize;

use crate::engine::{Engine, EngineError};
use crate::layout::{validate_layout, Layout};
use crate::tokens::{parse_token_set, validate_token_set, TokenDiagnostic, TOKEN_SCHEMA_VERSION};

/// Maximum compressed package size. Raised from 5 MiB when packages
/// gained pictures and fonts; the text parts are still bounded on their
/// own, so the headroom is for the files.
pub const MAX_PACKAGE_BYTES: usize = 25 * 1024 * 1024;
/// Maximum number of zip entries.
pub const MAX_ENTRIES: usize = 200;
/// Maximum uncompressed size of a single entry.
pub const MAX_ENTRY_BYTES: usize = 5 * 1024 * 1024;
/// Maximum uncompressed total of everything under `assets/images/` and
/// `assets/fonts/`, so a package cannot inflate past what a row of BYTEA
/// per file should hold.
pub const MAX_FILES_TOTAL_BYTES: usize = 20 * 1024 * 1024;
/// Maximum number of bundled files.
pub const MAX_FILES: usize = 100;
/// Maximum screenshot size.
pub const MAX_SCREENSHOT_BYTES: usize = 1024 * 1024;

/// `manifest.toml` at the package root.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ThemeManifest {
    /// Package name (`[a-z0-9-]{1,60}`).
    pub name: String,
    /// Integer package version >= 1.
    pub version: u32,
    /// Author display string (informational).
    #[serde(default)]
    pub author: String,
    /// API compatibility level; must equal the token schema version.
    pub required_api: u32,
}

/// One picture or font a theme bundles: `assets/images/hero.jpg` becomes
/// path `images/hero.jpg`, served at `/theme-assets/images/hero.jpg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeFile {
    /// Path under `assets/`, forward slashes, no leading slash.
    pub path: String,
    /// MIME type decided by extension at parse time; never sniffed later.
    pub content_type: &'static str,
    /// File contents.
    pub bytes: Vec<u8>,
}

/// The MIME type a bundled file is served with, by extension, when the
/// extension is one a theme may ship. SVG is here because icons and
/// logos are SVG; the route serves it under a CSP that disables script.
#[must_use]
pub fn bundled_content_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    let dir = path.split_once('/')?.0;
    Some(match (dir, ext.as_str()) {
        ("images", "png") => "image/png",
        ("images", "jpg" | "jpeg") => "image/jpeg",
        ("images", "gif") => "image/gif",
        ("images", "webp") => "image/webp",
        ("images", "avif") => "image/avif",
        ("images", "svg") => "image/svg+xml",
        ("fonts", "woff2") => "font/woff2",
        ("fonts", "woff") => "font/woff",
        ("fonts", "ttf") => "font/ttf",
        ("fonts", "otf") => "font/otf",
        _ => return None,
    })
}

/// Whether `path` (already stripped of `assets/`) is a legal bundled-file
/// path: `images/` or `fonts/`, at most three segments, each of
/// `[a-z0-9._-]`, no segment empty or starting with a dot.
#[must_use]
pub fn valid_bundled_path(path: &str) -> bool {
    let segs: Vec<&str> = path.split('/').collect();
    if !(2..=3).contains(&segs.len()) || !matches!(segs[0], "images" | "fonts") {
        return false;
    }
    if path.len() > 120 {
        return false;
    }
    segs.iter().all(|seg| {
        !seg.is_empty()
            && !seg.starts_with('.')
            && seg.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            })
    })
}

/// A fully parsed and validated theme package, ready to store.
#[derive(Debug)]
pub struct ParsedTheme {
    /// Validated manifest.
    pub manifest: ThemeManifest,
    /// Validated token set (JSON kept for storage).
    pub tokens_json: serde_json::Value,
    /// Validated layout (JSON kept for storage).
    pub layout_json: serde_json::Value,
    /// `assets/theme.css` and `assets/theme.js`, when the package has them.
    pub assets_json: Option<serde_json::Value>,
    /// Tera overrides keyed by registered template name
    /// (`templates/single.tera` → key `single.html`).
    pub templates: BTreeMap<String, String>,
    /// Optional screenshot bytes (PNG magic verified).
    pub screenshot_png: Option<Vec<u8>>,
    /// Pictures and fonts under `assets/images/` and `assets/fonts/`.
    pub files: Vec<ThemeFile>,
}

/// Package-level failure with an author-facing message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PackageError(pub String);

fn err(msg: impl Into<String>) -> PackageError {
    PackageError(msg.into())
}

/// Whether `name` is a legal package name: `[a-z0-9-]{1,60}`.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 60
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Validates every entry name up-front: path safety + duplicate rejection.
///
/// Public so hostile-archive tests can exercise the rules without
/// hand-crafting exotic zip files.
///
/// # Errors
/// Returns [`PackageError`] naming the offending entry.
pub fn check_entry_names(names: &[String]) -> Result<(), PackageError> {
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        safe_entry_name(name)?;
        if !seen.insert(name.clone()) {
            return Err(err(format!("duplicate entry \"{name}\" in package")));
        }
    }
    Ok(())
}

/// Rejects absolute paths, backslashes, dot-segments and NULs.
fn safe_entry_name(name: &str) -> Result<(), PackageError> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.split('/').any(|seg| seg == ".." || seg == ".")
    {
        return Err(err(format!("unsafe entry path \"{name}\" in package")));
    }
    Ok(())
}

fn diagnostics_message(prefix: &str, diags: &[TokenDiagnostic]) -> Option<String> {
    let errors: Vec<String> = diags
        .iter()
        .filter(|d| d.is_error())
        .map(ToString::to_string)
        .collect();
    if errors.is_empty() {
        None
    } else {
        Some(format!("{prefix}:\n  - {}", errors.join("\n  - ")))
    }
}

/// Parses a `.vytheme` archive.
///
/// # Errors
/// Returns [`PackageError`] with an install-time-friendly message for bad
/// zips, hostile entries, invalid manifests/tokens/layout or broken Tera.
pub fn parse_vytheme(bytes: &[u8]) -> Result<ParsedTheme, PackageError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(err(format!(
            "package is {} bytes; maximum is {MAX_PACKAGE_BYTES}",
            bytes.len()
        )));
    }
    let reader = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| err(format!("not a valid .vytheme zip: {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(err(format!(
            "package contains {} entries; maximum is {MAX_ENTRIES}",
            archive.len()
        )));
    }

    // Pass 1: name safety + duplicates (before reading any content).
    let mut names: Vec<String> = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let file = archive
            .by_index_raw(i)
            .map_err(|e| err(format!("corrupt package entry #{i}: {e}")))?;
        names.push(file.name().to_owned());
    }
    check_entry_names(&names)?;

    // Pass 2: read required files.
    let manifest_toml = read_required(&mut archive, "manifest.toml")?;
    let manifest: ThemeManifest = toml::from_str(
        std::str::from_utf8(&manifest_toml).map_err(|_| err("manifest.toml is not valid UTF-8"))?,
    )
    .map_err(|e| err(format!("manifest.toml: {e}")))?;
    if !valid_name(&manifest.name) {
        return Err(err(format!(
            "manifest.toml: name \"{}\" must match [a-z0-9-]{{1,60}}",
            manifest.name
        )));
    }
    if manifest.version == 0 {
        return Err(err("manifest.toml: version must be >= 1".to_owned()));
    }
    if manifest.required_api != TOKEN_SCHEMA_VERSION {
        return Err(err(format!(
            "manifest.toml: required_api = {} but this server speaks {}",
            manifest.required_api, TOKEN_SCHEMA_VERSION
        )));
    }

    let tokens_bytes = read_required(&mut archive, "tokens.json")?;
    let tokens_str =
        String::from_utf8(tokens_bytes).map_err(|_| err("tokens.json is not valid UTF-8"))?;
    let token_set = parse_token_set(&tokens_str).map_err(|diags| {
        err(format!(
            "tokens.json failed validation:\n  - {}",
            diags
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n  - ")
        ))
    })?;
    if let Some(msg) = diagnostics_message("tokens.json", &validate_token_set(&token_set)) {
        return Err(err(msg));
    }
    let tokens_json: serde_json::Value =
        serde_json::to_value(&token_set).map_err(|e| err(format!("tokens.json: {e}")))?;

    let layout_bytes = read_required(&mut archive, "layout.json")?;
    let layout_str =
        String::from_utf8(layout_bytes).map_err(|_| err("layout.json is not valid UTF-8"))?;
    let layout: Layout = serde_json::from_str(&layout_str)
        .map_err(|e| err(format!("layout.json is not a valid layout: {e}")))?;
    let registry = crate::dynblocks::builtin_registry();
    if let Some(msg) = diagnostics_message("layout.json", &validate_layout(&layout, &registry)) {
        return Err(err(msg));
    }
    let layout_json: serde_json::Value =
        serde_json::to_value(&layout).map_err(|e| err(format!("layout.json: {e}")))?;

    // Pass 3: optional Tera overrides (validated against the builtin engine
    // so extends/include targets must exist there or under partials/).
    let engine = Engine::builtin().map_err(|e| err(e.to_string()))?;
    let mut templates = BTreeMap::new();
    for name in &names {
        let Some(rest) = name.strip_prefix("templates/") else {
            continue;
        };
        if !rest.to_ascii_lowercase().ends_with(".tera") {
            return Err(err(format!(
                "unexpected file \"{name}\" — only templates/*.tera may live under templates/"
            )));
        }
        if rest.contains('/') {
            return Err(err(format!(
                "\"{name}\": nested directories under templates/ are not supported"
            )));
        }
        let src_bytes = read_required(&mut archive, name)?;
        let src = String::from_utf8(src_bytes)
            .map_err(|_| err(format!("\"{name}\" is not valid UTF-8")))?;
        // Register name: strip .tera → the template it overrides.
        let target = rest
            .strip_suffix(".tera")
            .map_or_else(|| rest.to_owned(), |s| format!("{s}.html"));
        engine
            .validate_theme_source(&target, &src)
            .map_err(|e| package_engine_error(name, &e))?;
        templates.insert(target.clone(), src);
    }

    // Pass 4: optional assets the theme ships with itself.
    //
    // Bounded, and read as text: a theme's stylesheet and script are
    // stored on the row so they version and roll back with everything
    // else, which means an unbounded one would sit in every query that
    // reads the theme.
    let mut assets = serde_json::Map::new();
    for (entry, key, limit) in [
        ("assets/theme.css", "css", crate::studio::MAX_THEME_CSS),
        ("assets/theme.js", "js", crate::studio::MAX_THEME_JS),
    ] {
        if !names.iter().any(|n| n == entry) {
            continue;
        }
        let bytes = read_required(&mut archive, entry)?;
        if bytes.len() > limit {
            return Err(err(format!(
                "{entry} is {} bytes; the maximum is {limit}",
                bytes.len()
            )));
        }
        let src =
            String::from_utf8(bytes).map_err(|_| err(format!("\"{entry}\" is not valid UTF-8")))?;
        if !src.trim().is_empty() {
            assets.insert(key.to_owned(), serde_json::Value::String(src));
        }
    }
    let assets_json = (!assets.is_empty()).then_some(serde_json::Value::Object(assets));

    // Pass 4b: bundled pictures and fonts. Kept out of the JSON row: they
    // are bytes, they can be large, and the render path must never load
    // them. Anything else under assets/ is a mistake worth naming rather
    // than silently ignoring.
    let mut files = Vec::new();
    let mut files_total = 0usize;
    for name in &names {
        let Some(rest) = name.strip_prefix("assets/") else {
            continue;
        };
        if rest == "theme.css" || rest == "theme.js" || name.ends_with('/') {
            continue;
        }
        if !valid_bundled_path(rest) {
            return Err(err(format!(
                "unexpected file \"{name}\" — a theme may bundle assets/theme.css, \
                 assets/theme.js, and files under assets/images/ or assets/fonts/ \
                 named with lowercase letters, digits, dots, dashes and underscores"
            )));
        }
        let Some(content_type) = bundled_content_type(rest) else {
            return Err(err(format!(
                "\"{name}\": images may be png, jpg, gif, webp, avif or svg; \
                 fonts may be woff2, woff, ttf or otf"
            )));
        };
        if files.len() >= MAX_FILES {
            return Err(err(format!(
                "package bundles more than {MAX_FILES} files under assets/"
            )));
        }
        let bytes = read_required(&mut archive, name)?;
        files_total = files_total.saturating_add(bytes.len());
        if files_total > MAX_FILES_TOTAL_BYTES {
            return Err(err(format!(
                "bundled files inflate past {MAX_FILES_TOTAL_BYTES} bytes in total"
            )));
        }
        if content_type == "image/svg+xml" && !looks_like_svg(&bytes) {
            return Err(err(format!(
                "\"{name}\" does not look like an SVG document"
            )));
        }
        files.push(ThemeFile {
            path: rest.to_owned(),
            content_type,
            bytes,
        });
    }

    // Pass 5: optional screenshot.
    let screenshot_png = match archive.by_name("screenshot.png") {
        Ok(f) => {
            if usize::try_from(f.size()).unwrap_or(usize::MAX) > MAX_SCREENSHOT_BYTES {
                return Err(err(format!(
                    "screenshot.png is {} bytes; maximum is {MAX_SCREENSHOT_BYTES}",
                    f.size()
                )));
            }
            let mut buf = Vec::with_capacity(usize::try_from(f.size()).unwrap_or(0));
            let mut limited = f.take(u64::try_from(MAX_SCREENSHOT_BYTES).unwrap_or(u64::MAX) + 1);
            limited
                .read_to_end(&mut buf)
                .map_err(|e| err(format!("cannot read screenshot.png: {e}")))?;
            if buf.len() > MAX_SCREENSHOT_BYTES {
                return Err(err(format!(
                    "screenshot.png inflates past {MAX_SCREENSHOT_BYTES} bytes"
                )));
            }
            if buf.len() < 8 || !buf.starts_with(&[0x89, b'P', b'N', b'G']) {
                return Err(err("screenshot.png does not look like a PNG".to_owned()));
            }
            Some(buf)
        }
        Err(zip::result::ZipError::FileNotFound) => None,
        Err(e) => return Err(err(format!("cannot open screenshot.png: {e}"))),
    };

    Ok(ParsedTheme {
        manifest,
        tokens_json,
        layout_json,
        assets_json,
        templates,
        screenshot_png,
        files,
    })
}

/// An SVG served from this origin is a document, and a document can
/// carry script. The route neutralises that with a CSP; this check only
/// keeps a renamed binary from being served under an SVG type.
#[must_use]
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    head.contains("<svg")
}

fn package_engine_error(entry: &str, e: &EngineError) -> PackageError {
    err(format!("{entry}: {e}"))
}

fn read_required(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    wanted: &str,
) -> Result<Vec<u8>, PackageError> {
    match archive.by_name(wanted) {
        Ok(file) => {
            if usize::try_from(file.size()).unwrap_or(usize::MAX) > MAX_ENTRY_BYTES {
                return Err(err(format!(
                    "entry \"{wanted}\" is {} bytes; maximum is {MAX_ENTRY_BYTES}",
                    file.size()
                )));
            }
            // Read through a limit, not up to the declared size: the size
            // in the header is the packager's claim, and a bomb claims
            // small and inflates large.
            let mut buf = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
            let mut limited = file.take(u64::try_from(MAX_ENTRY_BYTES).unwrap_or(u64::MAX) + 1);
            limited
                .read_to_end(&mut buf)
                .map_err(|e| err(format!("cannot read \"{wanted}\": {e}")))?;
            if buf.len() > MAX_ENTRY_BYTES {
                return Err(err(format!(
                    "entry \"{wanted}\" inflates past {MAX_ENTRY_BYTES} bytes"
                )));
            }
            Ok(buf)
        }
        Err(zip::result::ZipError::FileNotFound) => Err(err(format!(
            "package is missing required file \"{wanted}\""
        ))),
        Err(e) => Err(err(format!("cannot open \"{wanted}\": {e}"))),
    }
}

/// Convenience for tests/tools: validate a tokens+layout pair without a
/// full package (same rules the installer applies).
///
/// # Errors
/// Returns [`PackageError`] when either document fails its validation.
pub fn package_parse_tokens_layout(
    tokens_json: &serde_json::Value,
    layout_json: &serde_json::Value,
) -> Result<(), PackageError> {
    let tokens_str = serde_json::to_string(tokens_json).map_err(|e| err(e.to_string()))?;
    let token_set = parse_token_set(&tokens_str).map_err(|diags| {
        err(format!(
            "tokens failed validation:\n  - {}",
            diags
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n  - ")
        ))
    })?;
    if let Some(msg) = diagnostics_message("tokens", &validate_token_set(&token_set)) {
        return Err(err(msg));
    }
    let layout: Layout = serde_json::from_value(layout_json.clone())
        .map_err(|e| err(format!("invalid layout: {e}")))?;
    let registry = crate::dynblocks::builtin_registry();
    if let Some(msg) = diagnostics_message("layout", &validate_layout(&layout, &registry)) {
        return Err(err(msg));
    }
    Ok(())
}
