//! Image derivatives and blurhash generation.
//!
//! Derivatives are *real*: each size is decoded, resized with Lanczos,
//! and re-encoded — plus a lossy WebP twin per size, so the renderer can
//! offer `<picture>` sources that are actually smaller than the
//! original. (The MVP shipped byte-for-byte copies under these names;
//! every "optimized" image was a lie the exact size of the original.)
//! Blurhash is generated once at upload via the `blurhash` crate when
//! possible, otherwise a deterministic placeholder is used.

use vyasa_common::AppError;

/// Desired derivative sizes.
pub const SIZES: &[(&str, u32)] = &[("thumb", 320), ("medium", 768), ("large", 1280)];

/// Lossy WebP quality: visually clean for photography, roughly half the
/// bytes of an 85-quality JPEG.
const WEBP_QUALITY: f32 = 80.0;

/// Generates a blurhash for `bytes` (image data). Returns a deterministic
/// placeholder if the image cannot be decoded.
#[must_use]
pub fn blurhash_for(bytes: &[u8]) -> String {
    // Try to decode with `image` and encode blurhash; fall back to placeholder.
    if let Ok(img) = decode_upright(bytes) {
        let (w, h) = (img.width(), img.height());
        // Use 4x3 components, as recommended for thumbnails.
        let rgba = img.to_rgba8();
        if let Ok(hash) = blurhash::encode(4, 3, w, h, &rgba.into_raw()) {
            return hash;
        }
    }
    // Deterministic placeholder based on byte length.
    format!("LEHV6nWB2yk8pyo0adR*.kWQBkCM{:02x}", bytes.len() % 256)
}

/// Returns the dimensions of an image, if decodable.
#[must_use]
pub fn dimensions_of(bytes: &[u8]) -> Option<(u32, u32)> {
    decode_upright(bytes).ok().map(|img| {
        let (w, h) = (img.width(), img.height());
        (w, h)
    })
}

/// Lowercased file extension, `""` when there is none.
fn ext_of(file_name: &str) -> String {
    std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Formats that keep their original bytes untouched: resizing a GIF
/// drops its animation and an SVG has no pixels to resize. They get no
/// derivatives; the renderer serves the original.
fn resizable(file_name: &str) -> bool {
    !matches!(ext_of(file_name).as_str(), "gif" | "svg" | "avif")
}

/// Re-encodes `img` in the family it arrived in — a JPEG stays a JPEG,
/// everything else becomes a PNG (already-webp originals too: their
/// `webp_*` twin is the WebP).
pub(crate) fn encode_like(
    img: &image::DynamicImage,
    file_name: &str,
) -> Result<(Vec<u8>, &'static str), String> {
    let mut out = std::io::Cursor::new(Vec::new());
    let jpeg = matches!(ext_of(file_name).as_str(), "jpg" | "jpeg");
    if jpeg {
        // JPEG has no alpha; flatten before encoding.
        img.to_rgb8()
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .map_err(|e| e.to_string())?;
        Ok((out.into_inner(), "jpg"))
    } else {
        img.write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok((out.into_inner(), "png"))
    }
}

/// Lossy WebP bytes for `img`.
fn encode_webp(img: &image::DynamicImage) -> Vec<u8> {
    let rgba = img.to_rgba8();
    let encoder = webp::Encoder::from_rgba(&rgba, img.width(), img.height());
    encoder.encode(WEBP_QUALITY).to_vec()
}

/// Generates derivative files for `bytes` under `base_path` via `storage`.
/// Returns a map of `variant -> {path, width}` for `media.derivatives`.
///
/// Each entry in [`SIZES`] no wider than the original is resized
/// (Lanczos, aspect preserved — never upscaled) and stored twice: once
/// in the original's format family and once as lossy WebP
/// (`webp_<variant>_…`). A full-size WebP is stored as `webp`.
/// Undecodable or unresizable files (GIF, SVG) yield an empty map and
/// the original serves as-is.
///
/// # Errors
///
/// Returns `AppError` if storage fails; encode failures skip the
/// variant rather than failing the upload.
pub async fn generate_derivatives(
    storage: &dyn super::storage::StorageBackend,
    base_path: &str,
    file_name: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, AppError> {
    let mut map = serde_json::Map::new();
    if !resizable(file_name) {
        return Ok(serde_json::Value::Object(map));
    }
    let Ok(img) = decode_upright(bytes) else {
        return Ok(serde_json::Value::Object(map));
    };
    // Extract shard/id prefix from base_path: `{shard}/{id}/{file_name}` -> `{shard}/{id}`
    let prefix = base_path.rsplit_once('/').map_or("", |(p, _)| p);
    let at = |name: String| {
        if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        }
    };

    for (variant, width) in SIZES {
        if *width >= img.width() {
            continue; // never upscale; smaller originals just have fewer sources
        }
        let resized = img.resize(*width, u32::MAX, image::imageops::FilterType::Lanczos3);
        if let Ok((encoded, ext)) = encode_like(&resized, file_name) {
            let path = at(format!("{variant}_{}.{ext}", stem(file_name)));
            storage.put(&path, &encoded).await?;
            map.insert(
                (*variant).to_owned(),
                serde_json::json!({"path": path, "width": resized.width()}),
            );
        }
        let encoded = encode_webp(&resized);
        let path = at(format!("webp_{variant}_{}.webp", stem(file_name)));
        storage.put(&path, &encoded).await?;
        map.insert(
            format!("webp_{variant}"),
            serde_json::json!({"path": path, "width": resized.width()}),
        );
    }
    // Full-size WebP: the format saves bytes even without a resize.
    let encoded = encode_webp(&img);
    let path = at(format!("webp_{}.webp", stem(file_name)));
    storage.put(&path, &encoded).await?;
    map.insert(
        "webp".to_owned(),
        serde_json::json!({"path": path, "width": img.width()}),
    );
    Ok(serde_json::Value::Object(map))
}

/// Decodes an image the way a browser shows it.
///
/// A phone stores a portrait photo as landscape pixels plus an EXIF
/// orientation tag, and browsers rotate it on display. A resize that
/// ignores the tag writes the landscape pixels without it, so every
/// derived size of a portrait photo came out on its side while the
/// original looked fine — the worst kind of bug, because it only shows on
/// the public site at the widths that pick a variant.
fn decode_upright(bytes: &[u8]) -> image::ImageResult<image::DynamicImage> {
    use image::ImageDecoder as _;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// `photo.jpeg` → `photo`, so variants carry their own honest extension.
fn stem(file_name: &str) -> &str {
    file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem)
}

/// The storage paths recorded in a `media.derivatives` value.
///
/// Tolerates `{}` — the derivatives job has not run yet — and any entry
/// without a string `path`, so a row from before the shape settled still
/// deletes cleanly.
#[must_use]
pub fn derivative_paths(derivatives: &serde_json::Value) -> Vec<&str> {
    derivatives.as_object().map_or_else(Vec::new, |map| {
        map.values()
            .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
            .collect()
    })
}

/// Deletes every derivative object a row recorded, best-effort.
///
/// `MediaService::delete` removed the row and the original and left the
/// four resized copies behind in local disk or the bucket, for every
/// deletion after the derivatives job had run. A failure on one path is
/// logged with the media id and the rest are still attempted; both
/// backends treat an already-missing object as success, so this is safe
/// to run twice.
pub async fn delete_derivatives(
    storage: &dyn super::storage::StorageBackend,
    media_id: i64,
    derivatives: &serde_json::Value,
) {
    for path in derivative_paths(derivatives) {
        if let Err(err) = storage.delete(path).await {
            tracing::warn!(media_id, path, "could not delete media derivative: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{blurhash_for, decode_upright, dimensions_of, SIZES};

    /// 8×4 pixels, red on the left, tagged "rotate 90° clockwise to
    /// display": shown correctly it is 4×8 with red on top.
    const ORIENTED: &[u8] = include_bytes!("fixtures/oriented.jpg");

    #[test]
    fn exif_orientation_is_applied_before_anything_reads_the_pixels() {
        assert_eq!(
            dimensions_of(ORIENTED),
            Some((4, 8)),
            "dimensions are as displayed"
        );
        let img = decode_upright(ORIENTED).expect("decodes").to_rgb8();
        let top = img.get_pixel(1, 1);
        let bottom = img.get_pixel(1, 6);
        assert!(top[0] > 200 && top[2] < 80, "top is red: {top:?}");
        assert!(
            bottom[2] > 200 && bottom[0] < 80,
            "bottom is blue: {bottom:?}"
        );
    }

    #[test]
    fn sizes_are_sorted() {
        let mut last = 0;
        for (_, w) in SIZES {
            assert!(*w > last);
            last = *w;
        }
    }

    #[test]
    fn blurhash_deterministic() {
        let bytes = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 1, 2, 3,
        ];
        let a = blurhash_for(&bytes);
        let b = blurhash_for(&bytes);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    /// A real 1600×900 gradient, encoded as PNG in memory.
    fn photo_png() -> Vec<u8> {
        let img = image::ImageBuffer::from_fn(1600, 900, |x, y| {
            image::Rgb([
                u8::try_from(x % 256).unwrap_or(0),
                u8::try_from(y % 256).unwrap_or(0),
                128,
            ])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode");
        out.into_inner()
    }

    #[tokio::test]
    async fn derivatives_are_real_resizes_not_copies() {
        use crate::media::storage::{LocalFsBackend, StorageBackend};
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = LocalFsBackend::new(dir.path());
        let png = photo_png();
        let map = super::generate_derivatives(&storage, "1/1/photo.png", "photo.png", &png)
            .await
            .expect("derivatives");

        // Every configured size fits inside 1600px, so all three exist,
        // at their real widths, in both formats.
        for (variant, width) in SIZES {
            let entry = &map[*variant];
            assert_eq!(entry["width"], *width, "{variant}");
            let bytes = storage
                .get(entry["path"].as_str().expect("path"))
                .await
                .expect("stored");
            let resized = image::load_from_memory(&bytes).expect("decodable");
            assert_eq!(resized.width(), *width);
            assert!(
                bytes.len() < png.len(),
                "{variant} should be smaller than the original"
            );
            let twin = &map[&format!("webp_{variant}")];
            let webp_bytes = storage
                .get(twin["path"].as_str().expect("webp path"))
                .await
                .expect("webp stored");
            assert_eq!(&webp_bytes[..4], b"RIFF", "{variant} twin is webp");
            assert_eq!(&webp_bytes[8..12], b"WEBP");
        }
        // Plus a full-size webp.
        assert_eq!(map["webp"]["width"], 1600);
    }

    #[tokio::test]
    async fn small_originals_are_never_upscaled_and_gifs_stay_whole() {
        use crate::media::storage::LocalFsBackend;
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = LocalFsBackend::new(dir.path());

        // 1x1: no size variant qualifies; only the webp twin appears.
        let tiny = {
            let img = image::ImageBuffer::from_pixel(1, 1, image::Rgb([1u8, 2, 3]));
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut out, image::ImageFormat::Png)
                .expect("encode");
            out.into_inner()
        };
        let map = super::generate_derivatives(&storage, "2/2/dot.png", "dot.png", &tiny)
            .await
            .expect("derivatives");
        let keys: Vec<&String> = map.as_object().expect("map").keys().collect();
        assert_eq!(keys, ["webp"], "no upscales: {keys:?}");

        // A GIF is left alone entirely — resizing drops animation.
        let map = super::generate_derivatives(&storage, "3/3/anim.gif", "anim.gif", &tiny)
            .await
            .expect("gif");
        assert_eq!(map, serde_json::json!({}));
    }

    #[test]
    fn dimensions_of_png() {
        use base64::Engine;
        let png =
            base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").unwrap();
        let dims = dimensions_of(&png).expect("dims");
        assert_eq!(dims, (1, 1));
    }
}

#[cfg(test)]
mod delete_tests {
    use super::{delete_derivatives, derivative_paths, generate_derivatives};
    use crate::media::storage::{LocalFsBackend, StorageBackend};

    #[test]
    fn derivative_paths_reads_the_recorded_shape() {
        assert!(derivative_paths(&serde_json::json!({})).is_empty());
        assert!(derivative_paths(&serde_json::json!(null)).is_empty());
        let recorded = serde_json::json!({
            "thumb": {"path": "1/2/thumb_a.png", "width": 320},
            "broken": {"width": 1},
        });
        assert_eq!(derivative_paths(&recorded), ["1/2/thumb_a.png"]);
    }

    #[tokio::test]
    async fn deleting_derivatives_removes_every_recorded_object() {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = LocalFsBackend::new(dir.path());
        // A real decodable PNG, so the pipeline has an image to work with.
        let png = {
            let img = image::ImageBuffer::from_pixel(400, 300, image::Rgb([9u8, 9, 9]));
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut out, image::ImageFormat::Png)
                .expect("encode");
            out.into_inner()
        };
        storage.put("7/7/a.png", &png).await.expect("original");
        let derivatives = generate_derivatives(&storage, "7/7/a.png", "a.png", &png)
            .await
            .expect("derivatives");
        let paths: Vec<String> = derivative_paths(&derivatives)
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert!(!paths.is_empty(), "nothing was generated: {derivatives}");
        for p in &paths {
            assert!(
                storage.get(p).await.is_ok(),
                "{p} should exist before delete"
            );
        }

        delete_derivatives(&storage, 7, &derivatives).await;

        for p in &paths {
            assert!(storage.get(p).await.is_err(), "{p} survived the delete");
        }
        // The original is not this function's to remove.
        assert!(storage.get("7/7/a.png").await.is_ok());
    }
}
