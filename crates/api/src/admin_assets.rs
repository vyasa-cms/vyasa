//! The admin SPA's built assets, served from `admin/dist/assets`.
//!
//! The route's path segment arrives percent-decoded, so
//! `/assets/..%2f..%2fvyasa.toml` used to become a read of `vyasa.toml`
//! with the database password in it. A request names a file under the
//! assets directory or nothing.

use std::path::{Component, Path, PathBuf};

use axum::response::{IntoResponse, Response};

const ASSETS_DIR: &str = "admin/dist/assets";

/// Whether `path` is a plain relative path of ordinary names: no `..`,
/// no root or drive prefix, no backslashes, no NUL.
fn is_plain_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0'])
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// The file `path` names under `root`, after resolving symlinks, if it
/// stays inside `root`.
async fn resolve_under(root: &Path, path: &str) -> Option<PathBuf> {
    if !is_plain_relative(path) {
        return None;
    }
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let file = tokio::fs::canonicalize(root.join(path)).await.ok()?;
    file.starts_with(&root).then_some(file)
}

/// `GET /assets/{*path}`.
pub async fn serve(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let not_found = || (axum::http::StatusCode::NOT_FOUND, "asset not found").into_response();
    let Some(file) = resolve_under(Path::new(ASSETS_DIR), &path).await else {
        return not_found();
    };
    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let ct = match file.extension().and_then(|e| e.to_str()) {
                Some("js") => "application/javascript",
                Some("css") => "text/css",
                Some("svg") => "image/svg+xml",
                Some("woff2") => "font/woff2",
                _ => "application/octet-stream",
            };
            ([(axum::http::header::CONTENT_TYPE, ct)], bytes).into_response()
        }
        Err(_) => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_names_pass() {
        assert!(is_plain_relative("index-abc123.js"));
        assert!(is_plain_relative("fonts/inter.woff2"));
        for bad in [
            "",
            "../vyasa.toml",
            "../../vyasa.toml",
            "fonts/../../../etc/passwd",
            "/etc/passwd",
            "..\\..\\vyasa.toml",
            "a\\b.js",
            "a\0.js",
            "./index.js",
            "..",
        ] {
            assert!(!is_plain_relative(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn traversal_never_leaves_the_assets_directory() {
        let base = std::env::temp_dir().join(format!("vyasa-assets-{}", std::process::id()));
        let root = base.join("assets");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("app.js"), b"ok").unwrap();
        std::fs::write(base.join("vyasa.toml"), b"secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("vyasa.toml"), root.join("link.js")).unwrap();

        assert!(resolve_under(&root, "app.js").await.is_some());
        // What `..%2fvyasa.toml` decodes to, and friends.
        for bad in [
            "../vyasa.toml",
            "..%2fvyasa.toml",
            "/etc/passwd",
            "missing.js",
        ] {
            assert!(resolve_under(&root, bad).await.is_none(), "{bad}");
        }
        #[cfg(unix)]
        assert!(
            resolve_under(&root, "link.js").await.is_none(),
            "a symlink out of the directory is refused"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
