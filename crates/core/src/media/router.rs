//! One storage handle that can change backends while the server runs.
//!
//! Media rows record where their bytes went (`storage` column), but the
//! service and the job workers hold a single backend. The router keeps
//! that shape: writes go to the active backend, reads fall back to the
//! other one, so files uploaded before a switch keep serving without the
//! caller knowing which store holds them.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use vyasa_common::AppError;
use vyasa_db::content_models::MediaStorage;

use super::storage::{LocalFsBackend, StorageBackend};

/// Local disk plus an optional object store, swappable at runtime.
///
/// `active` is where new uploads go; the other store stays readable, so
/// switching back to local disk does not orphan what sits in the bucket.
pub struct StorageRouter {
    local: LocalFsBackend,
    object: RwLock<Option<Arc<dyn StorageBackend>>>,
    active: RwLock<MediaStorage>,
}

impl std::fmt::Debug for StorageRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageRouter")
            .field("local", &self.local.base)
            .field("object", &self.object().is_some())
            .field("active", &self.active_kind())
            .finish()
    }
}

impl StorageRouter {
    /// A router over `local`, with `object` active when given.
    #[must_use]
    pub fn new(local: LocalFsBackend, object: Option<Arc<dyn StorageBackend>>) -> Self {
        let active = if object.is_some() {
            MediaStorage::S3
        } else {
            MediaStorage::Local
        };
        Self {
            local,
            object: RwLock::new(object),
            active: RwLock::new(active),
        }
    }

    /// Replaces (or removes) the object store and makes it the target of
    /// new uploads when given. Takes effect for the next call on every
    /// holder of this router.
    pub fn set_object(&self, object: Option<Arc<dyn StorageBackend>>) {
        let active = if object.is_some() {
            MediaStorage::S3
        } else {
            MediaStorage::Local
        };
        *self
            .object
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = object;
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = active;
    }

    /// Chooses where new uploads go without forgetting the other store.
    /// `S3` without an object store falls back to local.
    pub fn set_active(&self, kind: MediaStorage) {
        let kind = if kind == MediaStorage::S3 && self.object().is_none() {
            MediaStorage::Local
        } else {
            kind
        };
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = kind;
    }

    /// The object store currently active, if any.
    #[must_use]
    pub fn object(&self) -> Option<Arc<dyn StorageBackend>> {
        self.object
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Where new uploads go.
    #[must_use]
    pub fn active_kind(&self) -> MediaStorage {
        let active = *self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active == MediaStorage::S3 && self.object().is_none() {
            MediaStorage::Local
        } else {
            active
        }
    }

    fn active_backend(&self) -> Arc<dyn StorageBackend> {
        match self.active_kind() {
            MediaStorage::S3 => self
                .object()
                .unwrap_or_else(|| Arc::new(self.local.clone())),
            MediaStorage::Local => Arc::new(self.local.clone()),
        }
    }

    /// The backend that holds files recorded under `kind`.
    #[must_use]
    pub fn backend_for(&self, kind: MediaStorage) -> Option<Arc<dyn StorageBackend>> {
        match kind {
            MediaStorage::Local => Some(Arc::new(self.local.clone())),
            MediaStorage::S3 => self.object(),
        }
    }

    /// The local disk backend.
    #[must_use]
    pub fn local(&self) -> &LocalFsBackend {
        &self.local
    }
}

#[async_trait]
impl StorageBackend for StorageRouter {
    fn kind(&self) -> MediaStorage {
        self.active_kind()
    }

    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), AppError> {
        self.active_backend().put(path, bytes).await
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, AppError> {
        let (first, second): (Arc<dyn StorageBackend>, Option<Arc<dyn StorageBackend>>) =
            match self.active_kind() {
                MediaStorage::S3 => (self.active_backend(), Some(Arc::new(self.local.clone()))),
                MediaStorage::Local => (Arc::new(self.local.clone()), self.object()),
            };
        // Any failure of the active store falls through: a bucket that
        // answers 403 for a missing key, or is down, must not hide files
        // that sit on local disk.
        match first.get(path).await {
            Ok(bytes) => Ok(bytes),
            Err(err) => match second {
                Some(second) => second.get(path).await.map_err(|_| err),
                None => Err(err),
            },
        }
    }

    async fn delete(&self, path: &str) -> Result<(), AppError> {
        self.local.delete(path).await?;
        if let Some(object) = self.object() {
            object.delete(path).await?;
        }
        Ok(())
    }

    async fn delete_from(&self, kind: MediaStorage, path: &str) -> Result<(), AppError> {
        match self.backend_for(kind) {
            Some(backend) => backend.delete(path).await,
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> (
        StorageRouter,
        LocalFsBackend,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
        let a = tempfile::tempdir().expect("tempdir");
        let b = tempfile::tempdir().expect("tempdir");
        let object = LocalFsBackend::new(b.path());
        let router = StorageRouter::new(
            LocalFsBackend::new(a.path()),
            Some(Arc::new(object.clone())),
        );
        (router, object, a, b)
    }

    #[tokio::test]
    async fn writes_go_to_the_active_backend_and_reads_fall_back() {
        let (router, object, a, _b) = two();
        assert_eq!(router.kind(), MediaStorage::S3);
        router.put("1/1/new.txt", b"new").await.unwrap();
        assert!(object.get("1/1/new.txt").await.is_ok());
        assert!(router.local().get("1/1/new.txt").await.is_err());

        // A file from before the switch lives on local disk only.
        router.local().put("1/2/old.txt", b"old").await.unwrap();
        assert_eq!(router.get("1/2/old.txt").await.unwrap(), b"old");
        assert!(matches!(
            router.get("1/3/missing.txt").await,
            Err(AppError::NotFound { .. })
        ));
        // Deleting from one store leaves the other's copy.
        router.local().put("1/6/both.txt", b"a").await.unwrap();
        object.put("1/6/both.txt", b"b").await.unwrap();
        router
            .delete_from(MediaStorage::Local, "1/6/both.txt")
            .await
            .unwrap();
        assert!(router.local().get("1/6/both.txt").await.is_err());
        assert_eq!(object.get("1/6/both.txt").await.unwrap(), b"b");
        drop(a);
    }

    #[tokio::test]
    async fn delete_clears_both_and_swapping_changes_the_target() {
        let (router, object, _a, _b) = two();
        router.local().put("1/1/f.txt", b"x").await.unwrap();
        object.put("1/1/f.txt", b"x").await.unwrap();
        router.delete("1/1/f.txt").await.unwrap();
        assert!(router.local().get("1/1/f.txt").await.is_err());
        assert!(object.get("1/1/f.txt").await.is_err());
        router.delete("1/1/f.txt").await.unwrap();

        // Back to local for new uploads: the bucket stays readable.
        object.put("1/5/kept.txt", b"k").await.unwrap();
        router.set_active(MediaStorage::Local);
        assert_eq!(router.kind(), MediaStorage::Local);
        router.put("1/4/l.txt", b"l").await.unwrap();
        assert!(router.local().get("1/4/l.txt").await.is_ok());
        assert_eq!(router.get("1/5/kept.txt").await.unwrap(), b"k");
        assert!(router.backend_for(MediaStorage::S3).is_some());

        router.set_object(None);
        assert!(router.backend_for(MediaStorage::S3).is_none());
        assert!(router.get("1/5/kept.txt").await.is_err());
    }

    #[tokio::test]
    async fn honours_the_shared_contract() {
        let (router, _o, _a, _b) = two();
        super::super::storage::assert_backend_contract(&router)
            .await
            .unwrap();
    }
}
