//! Serving a marketplace *from* this install.
//!
//! A Vyasa site can host a registry for other sites — including itself.
//! The whole marketplace is one directory, `registry_dir` (default
//! `registry/`, beside `media/` and `index/`):
//!
//! ```text
//! registry/index.json              the catalogue
//! registry/packages/*.vyplugin     the packages it points at
//! registry/packages/*.vytheme
//! ```
//!
//! Nothing is served unless those files exist, so an install that never
//! wanted to host a marketplace gains no surface. This is the answer for
//! a registry whose source repository is private: the index and the
//! packages have to be *readable* by every install, but the listings,
//! review history and CI that produce them do not.
//!
//! Filenames are checked against an allowlist rather than scanned for
//! traversal. This is a public, unauthenticated surface, and the cheapest
//! way to be certain a path cannot escape its directory is never to build
//! one out of caller input that has not been vetted byte by byte.

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, State};
use axum::http::{header, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Largest file served, matching the cap the installer enforces on the
/// way in. A registry that offers something bigger offers something no
/// site can install.
const MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Longest package filename accepted.
const MAX_NAME: usize = 100;

/// `GET /registry/index.json` — the catalogue this site publishes.
///
/// Cached briefly rather than immutably: the index is the one document
/// here whose bytes are meant to change.
pub async fn index(State(state): State<AppState>) -> Response {
    serve(
        state.config.registry_dir.join("index.json"),
        "application/json",
        "public, max-age=60",
    )
    .await
}

/// `GET /registry/packages/{file}` — one package the catalogue points at.
///
/// Immutable: package filenames carry their version, and the bytes behind
/// a published URL must never change. A site that installed a version
/// recorded its digest.
pub async fn package(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    if !is_package_name(&file) {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve(
        state.config.registry_dir.join("packages").join(&file),
        "application/octet-stream",
        "public, max-age=31536000, immutable",
    )
    .await
}

/// Whether `name` may be joined onto the packages directory.
///
/// Lowercase ASCII, digits, dot, dash and underscore only, ending in a
/// known package extension. That excludes `/` and `\` by construction,
/// so no traversal sequence can survive; `..` is refused explicitly
/// anyway, and a leading dot keeps hidden files out.
fn is_package_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME || name.starts_with('.') || name.contains("..") {
        return false;
    }
    if !(name.ends_with(".vyplugin") || name.ends_with(".vytheme")) {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_'))
}

/// Reads a file and answers with it, or 404.
///
/// Every failure is a 404, deliberately: a missing file, a directory, an
/// oversized one and a symlink are indistinguishable from outside, so
/// this leaks nothing about what the directory holds.
async fn serve(path: PathBuf, content_type: &'static str, cache: &'static str) -> Response {
    if !readable(&path).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

/// A real file, within the size cap, and not a symlink.
///
/// `symlink_metadata` rather than `metadata`: the operator owns this
/// directory, but a symlink planted in it would otherwise let the server
/// read anything the process can, and refusing them costs nothing.
async fn readable(path: &FsPath) -> bool {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) => meta.is_file() && meta.len() <= MAX_BYTES,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_package_names() {
        assert!(is_package_name("storefront-1.2.0.vyplugin"));
        assert!(is_package_name("aurora-3.vytheme"));
        assert!(is_package_name("a.vyplugin"));
        assert!(is_package_name("two_words-0.1.0-rc1.vyplugin"));
    }

    #[test]
    fn refuses_traversal() {
        assert!(!is_package_name("../.env.vyplugin"));
        assert!(!is_package_name("..%2f.env.vyplugin"));
        assert!(!is_package_name("a/../../etc/passwd.vyplugin"));
        assert!(!is_package_name("/etc/passwd.vyplugin"));
        assert!(!is_package_name("..\\windows.vyplugin"));
    }

    #[test]
    fn refuses_hidden_files_and_odd_characters() {
        assert!(!is_package_name(".hidden.vyplugin"));
        assert!(!is_package_name("Storefront.vyplugin"), "uppercase");
        assert!(!is_package_name("store front.vyplugin"), "space");
        assert!(!is_package_name("store\0front.vyplugin"), "nul");
        assert!(!is_package_name("café.vyplugin"), "non-ascii");
    }

    #[test]
    fn refuses_anything_that_is_not_a_package() {
        assert!(!is_package_name("index.json"));
        assert!(!is_package_name("id_rsa"));
        assert!(!is_package_name(""));
        assert!(!is_package_name("x.vyplugin.txt"));
    }

    #[test]
    fn refuses_an_overlong_name() {
        let long = format!("{}.vyplugin", "a".repeat(MAX_NAME));
        assert!(long.len() > MAX_NAME);
        assert!(!is_package_name(&long));
    }

    #[tokio::test]
    async fn readable_accepts_a_plain_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.json");
        tokio::fs::write(&path, b"{}").await.expect("write");
        assert!(readable(&path).await);
    }

    #[tokio::test]
    async fn readable_refuses_a_missing_file_and_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!readable(&dir.path().join("nope.json")).await);
        assert!(!readable(dir.path()).await, "a directory is not a file");
    }

    #[tokio::test]
    async fn readable_refuses_a_symlink_out_of_the_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = dir.path().join("secret");
        tokio::fs::write(&outside, b"secret").await.expect("write");
        let link = dir.path().join("packages").join("evil.vyplugin");
        tokio::fs::create_dir_all(link.parent().expect("parent"))
            .await
            .expect("mkdir");
        // A name that passes validation but resolves elsewhere: the
        // allowlist governs the path, this governs what it points at.
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");
        assert!(is_package_name("evil.vyplugin"));
        assert!(!readable(&link).await, "symlinks are refused");
    }

    #[tokio::test]
    async fn readable_refuses_an_oversized_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.vyplugin");
        let file = std::fs::File::create(&path).expect("create");
        file.set_len(MAX_BYTES + 1).expect("grow");
        assert!(!readable(&path).await);
    }
}
