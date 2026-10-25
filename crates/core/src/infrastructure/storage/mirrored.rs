//! Mirrored storage — stores a backup in a primary destination and then copies
//! it to an optional secondary (cloud) destination.
//!
//! The primary copy is always written first, so a cloud outage never prevents
//! the local backup from existing. A mirror failure is still reported as an
//! error so the job is marked failed and the operator gets notified.

use std::path::Path;

use crate::domain::error::BackupError;
use crate::ports::storage::Storage;

/// Primary storage plus an optional mirror.
pub struct MirroredStorage<P, M> {
    primary: P,
    mirror: Option<M>,
}

impl<P, M> MirroredStorage<P, M> {
    /// Create a new `MirroredStorage`. With `mirror = None` it behaves exactly like `primary`.
    pub fn new(primary: P, mirror: Option<M>) -> Self {
        Self { primary, mirror }
    }
}

impl<P: Storage, M: Storage> Storage for MirroredStorage<P, M> {
    async fn upload(&self, source: &Path, filename: &str) -> Result<String, BackupError> {
        let primary_destination = self.primary.upload(source, filename).await?;

        let Some(mirror) = &self.mirror else {
            return Ok(primary_destination);
        };

        match mirror.upload(source, filename).await {
            Ok(mirror_destination) => Ok(format!("{} | {}", primary_destination, mirror_destination)),
            Err(e) => {
                let reason = match e {
                    BackupError::StorageUpload(msg) => msg,
                    other => other.to_string(),
                };
                Err(BackupError::StorageUpload(format!(
                    "backup saved to '{}' but cloud mirror upload failed: {}",
                    primary_destination, reason
                )))
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct FakeStorage {
        name: &'static str,
        should_fail: bool,
        calls: Arc<AtomicUsize>,
    }

    impl FakeStorage {
        fn new(name: &'static str, should_fail: bool) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (Self { name, should_fail, calls: calls.clone() }, calls)
        }
    }

    impl Storage for FakeStorage {
        async fn upload(&self, _source: &Path, filename: &str) -> Result<String, BackupError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.should_fail {
                return Err(BackupError::StorageUpload(format!("{} offline", self.name)));
            }
            Ok(format!("{}:{}", self.name, filename))
        }
    }

    #[tokio::test]
    async fn without_mirror_returns_primary_destination() {
        let (primary, _) = FakeStorage::new("local", false);
        let storage: MirroredStorage<_, FakeStorage> = MirroredStorage::new(primary, None);
        let result = storage.upload(Path::new("x.zip"), "db.zip").await.unwrap();
        assert_eq!(result, "local:db.zip");
    }

    #[tokio::test]
    async fn with_mirror_returns_both_destinations() {
        let (primary, _) = FakeStorage::new("local", false);
        let (mirror, mirror_calls) = FakeStorage::new("gdrive", false);
        let storage = MirroredStorage::new(primary, Some(mirror));
        let result = storage.upload(Path::new("x.zip"), "db.zip").await.unwrap();
        assert_eq!(result, "local:db.zip | gdrive:db.zip");
        assert_eq!(mirror_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mirror_failure_is_an_error_that_names_the_primary_copy() {
        let (primary, _) = FakeStorage::new("local", false);
        let (mirror, _) = FakeStorage::new("gdrive", true);
        let storage = MirroredStorage::new(primary, Some(mirror));
        let err = storage.upload(Path::new("x.zip"), "db.zip").await.unwrap_err().to_string();
        assert!(err.contains("backup saved to 'local:db.zip'"), "{err}");
        assert!(err.contains("gdrive offline"), "{err}");
    }

    #[tokio::test]
    async fn primary_failure_skips_mirror() {
        let (primary, _) = FakeStorage::new("local", true);
        let (mirror, mirror_calls) = FakeStorage::new("gdrive", false);
        let storage = MirroredStorage::new(primary, Some(mirror));
        assert!(storage.upload(Path::new("x.zip"), "db.zip").await.is_err());
        assert_eq!(mirror_calls.load(Ordering::SeqCst), 0);
    }
}
