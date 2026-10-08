#![allow(clippy::too_many_arguments)]
//! Media service: upload with sniffing, allowlist, size caps, and storage.

use std::sync::Arc;

use vyasa_common::AppError;
use vyasa_db::content_models::MediaRow;
use vyasa_db::repo::MediaRepo;

use super::storage::StorageBackend;

/// Maximum upload size (10 MiB).
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

/// Allowlist of MIME types (sniffed, not just extension). SVG is rejected.
const ALLOWLIST: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/avif",
    "image/gif",
    "video/mp4",
    "video/webm",
    "audio/mpeg",
    "audio/wav",
    "audio/ogg",
    "application/pdf",
];

/// Media operations.
#[derive(Clone)]
pub struct MediaService {
    repo: MediaRepo,
    storage: Arc<dyn StorageBackend>,
    pool: sqlx::PgPool,
}

impl std::fmt::Debug for MediaService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaService").finish_non_exhaustive()
    }
}

/// A crop as fractions of the picture, top-left origin.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct CropBox {
    /// Left edge, 0..=1.
    pub x: f64,
    /// Top edge, 0..=1.
    pub y: f64,
    /// Width as a fraction of the picture.
    pub w: f64,
    /// Height as a fraction of the picture.
    pub h: f64,
}

/// What [`MediaService::edit_image`] applies: crop first, then rotation
/// (0, 90, 180 or 270 degrees clockwise), then flips.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ImageEdit {
    /// Degrees clockwise: 0, 90, 180 or 270.
    #[serde(default)]
    pub rotate: u32,
    /// Mirror left-to-right.
    #[serde(default)]
    pub flip_h: bool,
    /// Mirror top-to-bottom.
    #[serde(default)]
    pub flip_v: bool,
    /// The part to keep, applied before rotation.
    #[serde(default)]
    pub crop: Option<CropBox>,
}

impl MediaService {
    /// The backend the bytes live in, for work that runs outside a request.
    ///
    /// The derivatives worker needs the same store the upload wrote to;
    /// reconstructing one from the environment gave it a different store
    /// on every deployment that did not happen to match.
    #[must_use]
    pub fn storage(&self) -> Arc<dyn StorageBackend> {
        self.storage.clone()
    }

    /// Creates a service over `repo` and `storage`.
    #[must_use]
    pub fn new(repo: MediaRepo, storage: Arc<dyn StorageBackend>, pool: sqlx::PgPool) -> Self {
        Self {
            repo,
            storage,
            pool,
        }
    }

    /// Creates a service without pool (for tests without enqueue).
    #[cfg(test)]
    #[must_use]
    pub fn new_without_pool(repo: MediaRepo, storage: Arc<dyn StorageBackend>) -> Self {
        let pool = repo.pool();
        Self {
            repo,
            storage,
            pool,
        }
    }

    /// Returns the underlying repo (for listing).
    #[must_use]
    pub fn repo(&self) -> &MediaRepo {
        &self.repo
    }

    /// Uploads `bytes` as `file_name` for `owner_id`.
    ///
    /// Sniffs MIME via magic bytes, rejects SVG and non-allowlisted types,
    /// enforces size caps, shards storage path, and inserts the DB row.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Validation` for bad MIME/size, `AppError::Db` for DB failures,
    /// `AppError::Internal` for storage failures.
    pub async fn upload(
        &self,
        owner_id: i64,
        file_name: &str,
        bytes: Vec<u8>,
        alt: Option<String>,
        caption: Option<String>,
    ) -> Result<MediaRow, AppError> {
        if bytes.len() > MAX_BYTES {
            return Err(AppError::too_large(format!(
                "The file is {} MB; the limit is {} MB.",
                bytes.len() / (1024 * 1024),
                MAX_BYTES / (1024 * 1024)
            )));
        }
        if bytes.is_empty() {
            return Err(AppError::validation("file is empty"));
        }
        // Sanitize file name: keep only basename, replace risky chars.
        let file_name = sanitize_file_name(file_name);
        if file_name.is_empty() {
            return Err(AppError::validation(
                "file name is empty after sanitization",
            ));
        }
        // Reject SVG by extension (case-insensitive).
        let ext = std::path::Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if ext == "svg" || ext == "svgz" {
            return Err(AppError::validation("SVG files are not allowed"));
        }
        // Sniff MIME via magic bytes.
        let mime = sniff_mime(&bytes, &file_name);
        if mime == "image/svg+xml" {
            return Err(AppError::validation("SVG files are not allowed"));
        }
        if mime == "image/heic" || mime == "image/heif" {
            return Err(AppError::validation(
                "HEIC photos are not supported. The editor converts them on browsers that can \
                 read them; otherwise set the camera to Most Compatible, or export as JPEG.",
            ));
        }
        if !ALLOWLIST.contains(&mime.as_str()) {
            return Err(AppError::validation(format!(
                "unsupported media type: {mime}"
            )));
        }
        let sha256 = sha256_hex(&bytes);
        let id = vyasa_common::next_id_i64();
        // Sharded path: `{id % 1000}/{id}/{file_name}` – no user input in dir components.
        let shard = (id % 1000).abs();
        let path = format!("{shard}/{id}/{file_name}");
        // Store bytes.
        self.storage.put(&path, &bytes).await?;
        // Insert DB row. Width/height/blurhash are phase 16; leave null for now.
        let row = self
            .repo
            .insert(&vyasa_db::repo::media::NewMedia {
                id,
                owner_id,
                file_name: &file_name,
                mime: &mime,
                byte_size: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
                storage: self.storage.kind(),
                path: &path,
                width: None,
                height: None,
                blurhash: None,
                alt: alt.as_deref(),
                caption: caption.as_deref(),
                derivatives: serde_json::json!({}),
                sha256: Some(&sha256),
            })
            .await
            .inspect_err(|_| {
                // Best-effort cleanup of stored file on DB failure.
                let storage = self.storage.clone();
                let path_clone = path.clone();
                tokio::spawn(async move {
                    let _ = storage.delete(&path_clone).await;
                });
            })?;
        // Enqueue derivatives + blurhash job (fire-and-forget, non-blocking).
        let pool = self.pool.clone();
        let media_id = row.id;
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT INTO jobs (id, kind, payload, run_at, status) VALUES ($1, 'media_derivatives', $2, now(), 'queued')",
            )
            .bind(vyasa_common::next_id_i64())
            .bind(serde_json::json!({"media_id": media_id}))
            .execute(&pool)
            .await;
        });
        Ok(row)
    }

    /// Fetches a media row by id.
    ///
    /// # Errors
    ///
    /// Returns `AppError::NotFound` when missing.
    pub async fn get(&self, id: i64) -> Result<MediaRow, AppError> {
        self.repo.get(id).await
    }

    /// The row that already holds these exact bytes, if any.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Db` on failure.
    pub async fn duplicate_of(&self, bytes: &[u8]) -> Result<Option<MediaRow>, AppError> {
        self.repo.find_by_sha256(&sha256_hex(bytes)).await
    }

    /// Rotates, flips and crops an image in place. The file keeps its id
    /// and name, so every post that embeds it shows the new version;
    /// derivatives are regenerated.
    ///
    /// # Errors
    /// [`AppError::Validation`] for a non-image or an empty crop;
    /// storage and database errors.
    pub async fn edit_image(&self, id: i64, edit: &ImageEdit) -> Result<MediaRow, AppError> {
        let row = self.repo.get(id).await?;
        if !row.mime.starts_with("image/") || row.mime == "image/svg+xml" || row.mime == "image/gif"
        {
            return Err(AppError::validation(
                "only raster images can be edited (SVG and GIF are kept as they are)",
            ));
        }
        let bytes = self.storage.get(&row.path).await?;
        let mut img = image::load_from_memory(&bytes)
            .map_err(|e| AppError::validation(format!("could not decode the image: {e}")))?;
        if let Some(c) = &edit.crop {
            let (w, h) = (f64::from(img.width()), f64::from(img.height()));
            let clamp = |v: f64| v.clamp(0.0, 1.0);
            let x = clamp(c.x);
            let y = clamp(c.y);
            let cw = clamp(c.w).min(1.0 - x);
            let ch = clamp(c.h).min(1.0 - y);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (px, py, pw, ph) = (
                (x * w) as u32,
                (y * h) as u32,
                (cw * w) as u32,
                (ch * h) as u32,
            );
            if pw < 8 || ph < 8 {
                return Err(AppError::validation("that crop would leave almost nothing"));
            }
            img = img.crop_imm(px, py, pw, ph);
        }
        img = match edit.rotate {
            90 => img.rotate90(),
            180 => img.rotate180(),
            270 => img.rotate270(),
            _ => img,
        };
        if edit.flip_h {
            img = img.fliph();
        }
        if edit.flip_v {
            img = img.flipv();
        }
        let (out, _ext) = super::derivatives::encode_like(&img, &row.file_name)
            .map_err(|e| AppError::internal_msg(format!("encode: {e}")))?;
        let name = row.file_name.clone();
        self.replace(id, &name, out).await
    }

    /// Replaces a file's bytes under the same id: the old original and
    /// its derivatives go, the new bytes are stored, and the derivatives
    /// job runs again. Every post that embeds the id keeps working.
    ///
    /// # Errors
    ///
    /// Same rules as `upload`; `AppError::NotFound` when the id is unknown.
    pub async fn replace(
        &self,
        id: i64,
        file_name: &str,
        bytes: Vec<u8>,
    ) -> Result<MediaRow, AppError> {
        let old = self.repo.get(id).await?;
        let (file_name, mime) = check_upload(file_name, &bytes)?;
        let sha256 = sha256_hex(&bytes);
        let shard = (id % 1000).abs();
        let path = format!("{shard}/{id}/{file_name}");
        let kind = self.storage.kind();
        self.storage.put(&path, &bytes).await?;
        let row = self
            .repo
            .replace_file(
                id,
                &file_name,
                &mime,
                i64::try_from(bytes.len()).unwrap_or(i64::MAX),
                &path,
                &sha256,
                kind,
            )
            .await?;
        if old.path == path {
            // Same path, different store (the admin switched since the
            // upload): the stale copy would otherwise be "moved" over the
            // new bytes later.
            if old.storage != kind {
                if let Err(err) = self.storage.delete_from(old.storage, &path).await {
                    tracing::warn!(
                        media_id = id,
                        "could not delete the replaced original's old copy: {err}"
                    );
                }
            }
        } else if let Err(err) = self.storage.delete(&old.path).await {
            tracing::warn!(media_id = id, "could not delete replaced original: {err}");
        }
        super::derivatives::delete_derivatives(self.storage.as_ref(), id, &old.derivatives).await;
        self.enqueue_derivatives(id);
        Ok(row)
    }

    /// Queues the derivatives job for `media_id`.
    fn enqueue_derivatives(&self, media_id: i64) {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT INTO jobs (id, kind, payload, run_at, status) VALUES ($1, 'media_derivatives', $2, now(), 'queued')",
            )
            .bind(vyasa_common::next_id_i64())
            .bind(serde_json::json!({"media_id": media_id}))
            .execute(&pool)
            .await;
        });
    }

    /// Library totals.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Db` on failure.
    pub async fn stats(&self) -> Result<vyasa_db::repo::MediaStats, AppError> {
        self.repo.stats().await
    }

    /// Lists media, newest first.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Db` on failure.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<MediaRow>, AppError> {
        self.repo.list(limit, offset).await
    }

    /// Moves a file to the trash. It leaves the library and the stats;
    /// the bytes stay until it is purged, so a mistake can be undone.
    ///
    /// # Errors
    /// Returns `AppError::NotFound` when missing.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        self.repo.trash(id).await
    }

    /// Brings a file back from the trash.
    ///
    /// # Errors
    /// Returns `AppError::NotFound` when missing.
    pub async fn restore(&self, id: i64) -> Result<MediaRow, AppError> {
        self.repo.restore(id).await?;
        self.repo.get(id).await
    }

    /// Removes a file and its derivatives for good.
    ///
    /// # Errors
    /// Returns `AppError::NotFound` when missing.
    pub async fn purge(&self, id: i64) -> Result<(), AppError> {
        let row = self.repo.get(id).await?;
        self.repo.delete(id).await?;
        if let Err(err) = self.storage.delete(&row.path).await {
            tracing::warn!(media_id = id, path = %row.path, "could not delete media original: {err}");
        }
        super::derivatives::delete_derivatives(self.storage.as_ref(), id, &row.derivatives).await;
        Ok(())
    }

    /// Purges everything in the trash; returns how many.
    ///
    /// # Errors
    /// Database errors.
    pub async fn empty_trash(&self) -> Result<usize, AppError> {
        let rows = self.repo.trashed().await?;
        let mut n = 0;
        for row in rows {
            if self.purge(row.id).await.is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Retrieves the raw bytes for a media row.
    ///
    /// # Errors
    ///
    /// Returns `AppError::NotFound` when file missing.
    pub async fn get_bytes(&self, row: &MediaRow) -> Result<Vec<u8>, AppError> {
        self.storage.get(&row.path).await
    }

    /// Retrieves bytes for an arbitrary storage path (for variants).
    ///
    /// # Errors
    ///
    /// Returns `AppError::NotFound` when file missing.
    pub async fn get_bytes_for_path(&self, path: &str) -> Result<Vec<u8>, AppError> {
        self.storage.get(path).await
    }

    /// Generates a strong ETag for `bytes` (hex of sha256).
    #[must_use]
    pub fn etag_for(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(bytes);
        format!("\"{}\"", hex::encode(hash))
    }
}

/// Hex SHA-256 of a file's bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// The checks an upload passes: size, name, type. Shared with replace.
///
/// # Errors
///
/// `AppError::TooLarge` past the cap, `AppError::Validation` for an empty
/// file, an unsafe name, or a type outside the allowlist.
pub fn check_upload(file_name: &str, bytes: &[u8]) -> Result<(String, String), AppError> {
    if bytes.len() > MAX_BYTES {
        return Err(AppError::too_large(format!(
            "The file is {} MB; the limit is {} MB.",
            bytes.len() / (1024 * 1024),
            MAX_BYTES / (1024 * 1024)
        )));
    }
    if bytes.is_empty() {
        return Err(AppError::validation("file is empty"));
    }
    let file_name = sanitize_file_name(file_name);
    if file_name.is_empty() {
        return Err(AppError::validation(
            "file name is empty after sanitization",
        ));
    }
    let ext = std::path::Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext == "svg" || ext == "svgz" {
        return Err(AppError::validation("SVG files are not allowed"));
    }
    let mime = sniff_mime(bytes, &file_name);
    if mime == "image/svg+xml" {
        return Err(AppError::validation("SVG files are not allowed"));
    }
    if mime == "image/heic" || mime == "image/heif" {
        return Err(AppError::validation(
            "HEIC photos are not supported. The editor converts them on browsers that can \
             read them; otherwise set the camera to Most Compatible, or export as JPEG.",
        ));
    }
    if !ALLOWLIST.contains(&mime.as_str()) {
        return Err(AppError::validation(format!(
            "unsupported media type: {mime}"
        )));
    }
    Ok((file_name, mime))
}

/// Sanitizes a file name: keeps basename, replaces path separators, trims.
#[must_use]
pub fn sanitize_file_name(input: &str) -> String {
    // Take basename (after last `/` or `\`), then keep only safe chars.
    let base = input
        .rsplit('/')
        .next()
        .unwrap_or(input)
        .rsplit('\\')
        .next()
        .unwrap_or(input);
    let mut out = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else if ch.is_ascii_whitespace() || ch == '-' {
            out.push('-');
        } else {
            out.push('_');
        }
    }
    // Collapse repeated `-`/`_`, trim dots/dashes.
    let mut cleaned = String::with_capacity(out.len());
    let mut last_dash = false;
    for ch in out.chars() {
        let is_dash = ch == '-' || ch == '_';
        if is_dash && last_dash {
            continue;
        }
        cleaned.push(ch);
        last_dash = is_dash;
    }
    cleaned
        .trim_matches(|c| c == '.' || c == '-' || c == '_')
        .to_string()
}

/// Sniffs MIME via `infer` (magic bytes), falling back to extension guess.
#[must_use]
pub fn sniff_mime(bytes: &[u8], file_name: &str) -> String {
    if let Some(kind) = infer::get(bytes) {
        let mime = kind.mime_type();
        // infer may return `text/xml` for SVG; treat as SVG.
        if mime == "image/svg+xml" || mime == "text/xml" && bytes.windows(4).any(|w| w == b"<svg") {
            return "image/svg+xml".to_string();
        }
        // Normalize `image/jpg` to `image/jpeg` etc.
        let mime = match mime {
            "image/jpg" => "image/jpeg",
            other => other,
        };
        return mime.to_string();
    }
    // Fallback to extension guess (still validated against allowlist).
    mime_guess::from_path(file_name)
        .first()
        .map_or_else(|| "application/octet-stream".to_string(), |m| m.to_string())
}

#[cfg(test)]
mod tests {
    use super::{sanitize_file_name, sniff_mime, ALLOWLIST, MAX_BYTES};
    use std::sync::Arc;

    #[test]
    fn sanitize_keeps_basename() {
        assert_eq!(sanitize_file_name("/tmp/../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("a/b/c.png"), "c.png");
        assert_eq!(sanitize_file_name("my photo.jpg"), "my-photo.jpg");
        assert_eq!(sanitize_file_name(""), "");
    }

    #[test]
    fn sniff_rejects_svg() {
        let svg = b"<?xml version=\"1.0\"?><svg></svg>";
        let mime = sniff_mime(svg, "image.svg");
        assert_eq!(mime, "image/svg+xml");
        assert!(!ALLOWLIST.contains(&mime.as_str()));
    }

    #[test]
    fn sniff_png() {
        // Minimal PNG magic: 89 50 4E 47 0D 0A 1A 0A
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        let mime = sniff_mime(&png, "x.png");
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn allowlist_enforced() {
        assert!(ALLOWLIST.contains(&"image/png"));
        assert!(!ALLOWLIST.contains(&"image/svg+xml"));
        assert!(!ALLOWLIST.contains(&"application/x-msdownload"));
    }

    #[test]
    fn size_cap() {
        const { assert!(MAX_BYTES == 10 * 1024 * 1024) }
    }

    // Storage round-trip is tested in storage.rs; mime spoof is tested via integration (see below).
    #[tokio::test]
    async fn storage_round_trip_via_service() {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage: Arc<dyn super::super::storage::StorageBackend> =
            Arc::new(super::super::storage::LocalFsBackend::new(dir.path()));
        // Use a dummy repo that will fail DB, but we test storage directly.
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mime = sniff_mime(&png, "a.png");
        assert_eq!(mime, "image/png");
        // Ensure storage works (already tested in storage.rs)
        storage.put("1/2/a.png", &png).await.expect("put");
        let got = storage.get("1/2/a.png").await.expect("get");
        assert_eq!(got, png);
    }
}

#[cfg(test)]
mod library_tests {
    use super::{check_upload, sha256_hex};

    #[test]
    fn the_hash_is_of_the_bytes_and_stable() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_hex(b"abc"), sha256_hex(b"abc"));
        assert_ne!(sha256_hex(b"abc"), sha256_hex(b"abd"));
    }

    #[test]
    fn replace_is_held_to_the_same_rules_as_upload() {
        let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        let (name, mime) = check_upload("../My Photo.PNG", &png).expect("a png");
        assert_eq!(name, "My-Photo.PNG");
        assert_eq!(mime, "image/png");
        assert!(check_upload("x.png", b"").is_err(), "empty");
        assert!(check_upload("x.svg", b"<svg/>").is_err(), "svg by name");
        assert!(
            check_upload("x.txt", b"hello").is_err(),
            "outside the allowlist"
        );
    }
}
