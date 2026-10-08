//! Storage backend abstraction.

use async_trait::async_trait;
use vyasa_common::AppError;

/// Abstract storage for media bytes.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Stores `bytes` at `path`.
    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), AppError>;

    /// Retrieves bytes at `path`.
    async fn get(&self, path: &str) -> Result<Vec<u8>, AppError>;

    /// Deletes the object at `path`. Succeeds if already gone.
    async fn delete(&self, path: &str) -> Result<(), AppError>;

    /// Deletes `path` from the store recorded as `kind` only. A plain
    /// backend holds one kind; a router that holds both overrides this so
    /// a file replaced under the same path in a different store loses
    /// its stale copy and keeps the new one.
    async fn delete_from(
        &self,
        kind: vyasa_db::content_models::MediaStorage,
        path: &str,
    ) -> Result<(), AppError> {
        if kind == self.kind() {
            self.delete(path).await
        } else {
            Ok(())
        }
    }

    /// Which kind of store this is, for the `storage` column.
    ///
    /// The service used to write `Local` into every row regardless, so on
    /// an object-storage deployment the database and the API both said
    /// the bytes were on local disk when they were in a bucket.
    fn kind(&self) -> vyasa_db::content_models::MediaStorage;
}

/// Local filesystem backend, sharded as `{media_dir}/{id%1000}/{id}/{file_name}` logically,
/// but `path` is already the sharded relative path (e.g. `123/456/original.png`).
#[derive(Clone, Debug)]
pub struct LocalFsBackend {
    /// Base directory (e.g. `./media` or temp dir for tests).
    pub base: std::path::PathBuf,
}

impl LocalFsBackend {
    /// Creates a backend rooted at `base`.
    #[must_use]
    pub fn new(base: impl Into<std::path::PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Resolves `path` to an absolute filesystem path, rejecting traversal.
    fn resolve(&self, path: &str) -> Result<std::path::PathBuf, AppError> {
        // Path traversal impossible by construction: `path` is always
        // `{shard}/{id}/{sanitized_file_name}` where `id` is numeric and
        // `file_name` is sanitized (no `/` or `..`). Still, defensively
        // reject any `..` or absolute components.
        if path.contains("..") || path.starts_with('/') || path.starts_with('\\') {
            return Err(AppError::validation(format!(
                "invalid storage path: {path:?}"
            )));
        }
        let mut full = self.base.clone();
        for component in path.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(AppError::validation(format!(
                    "invalid path component: {component:?}"
                )));
            }
            // Sanitize: reject `/` and `\` inside components (already split) and control chars.
            if component.contains('\\') || component.contains('\0') {
                return Err(AppError::validation(format!(
                    "invalid path component: {component:?}"
                )));
            }
            full.push(component);
        }
        Ok(full)
    }
}

#[async_trait]
impl StorageBackend for LocalFsBackend {
    fn kind(&self) -> vyasa_db::content_models::MediaStorage {
        vyasa_db::content_models::MediaStorage::Local
    }

    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), AppError> {
        let full = self.resolve(path)?;
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|err| {
                AppError::internal_msg(format!("failed to create media dir: {err}"))
            })?;
        }
        tokio::fs::write(&full, bytes)
            .await
            .map_err(|err| AppError::internal_msg(format!("failed to write media file: {err}")))?;
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, AppError> {
        let full = self.resolve(path)?;
        tokio::fs::read(&full).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found("media", path)
            } else {
                AppError::internal_msg(format!("failed to read media file: {err}"))
            }
        })
    }

    async fn delete(&self, path: &str) -> Result<(), AppError> {
        let full = self.resolve(path)?;
        match tokio::fs::remove_file(&full).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::internal_msg(format!(
                "failed to delete media file: {err}"
            ))),
        }
    }
}

/// The behaviour every backend must share, as one function a backend's
/// own tests can call.
///
/// Media moved from local disk to object storage, and the two must be
/// interchangeable: an upload that behaved differently depending on where
/// bytes landed would be a bug nobody found until they switched.
///
/// # Errors
/// Propagates whatever the backend returned, so a caller sees which step
/// disagreed.
///
/// # Panics
/// When the backend violates the contract — that is the point.
pub async fn assert_backend_contract(backend: &dyn StorageBackend) -> Result<(), AppError> {
    let path = format!("{}/1/contract.bin", vyasa_common::next_id_i64());
    let bytes = b"the quick brown fox".to_vec();

    backend.put(&path, &bytes).await?;
    assert_eq!(backend.get(&path).await?, bytes, "what went in comes out");

    // Overwriting replaces rather than appends.
    backend.put(&path, b"shorter").await?;
    assert_eq!(backend.get(&path).await?, b"shorter");

    backend.delete(&path).await?;
    assert!(
        backend.get(&path).await.is_err(),
        "a deleted object is gone"
    );
    // Deleting what is already gone succeeds, so a retried cleanup is not
    // an error.
    backend.delete(&path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LocalFsBackend, StorageBackend};

    #[tokio::test]
    async fn round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalFsBackend::new(dir.path());
        let path = "123/456/test.txt";
        let bytes = b"hello world";
        backend.put(path, bytes).await.expect("put");
        let got = backend.get(path).await.expect("get");
        assert_eq!(got, bytes);
        backend.delete(path).await.expect("delete");
        assert!(backend.get(path).await.is_err());
    }

    #[tokio::test]
    async fn the_local_backend_honours_the_shared_contract() {
        let dir = tempfile::tempdir().expect("temp dir");
        let backend = LocalFsBackend::new(dir.path());
        super::assert_backend_contract(&backend)
            .await
            .expect("local disk honours it");
    }

    #[tokio::test]
    async fn rejects_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalFsBackend::new(dir.path());
        assert!(backend.put("../evil", b"x").await.is_err());
        assert!(backend.put("/absolute", b"x").await.is_err());
        assert!(backend.put("a/../b", b"x").await.is_err());
    }
}
