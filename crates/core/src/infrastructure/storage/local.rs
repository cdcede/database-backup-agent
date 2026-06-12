//! Local storage adapter implementation.
//!
//! Satisfies the `Storage` port. Copies backup files to a designated local directory
//! (or a network share accessible via UNC path). Uses Tokio's asynchronous
//! filesystem APIs.

use std::path::{Path, PathBuf};

use crate::domain::error::BackupError;
use crate::ports::storage::Storage;

/// Local directory storage adapter.
pub struct LocalStorage {
    target_dir: PathBuf,
}

impl LocalStorage {
    /// Create a new `LocalStorage` instance with the specified target directory.
    pub fn new(target_dir: PathBuf) -> Self {
        Self { target_dir }
    }
}

impl Storage for LocalStorage {
    async fn upload(&self, source: &Path, filename: &str) -> Result<String, BackupError> {
        // ---------------------------------------------------------------------
        // Async Rule: Use Tokio's native async fs APIs where available
        // ---------------------------------------------------------------------
        // Unlike ZIP compression which does heavy CPU calculation, copying files
        // is purely I/O bound. Tokio provides `tokio::fs` which runs on its
        // internal blocking helper pool under the hood, giving us a clean,
        // non-blocking async API.
        
        // 1. Ensure target directory exists
        tokio::fs::create_dir_all(&self.target_dir)
            .await
            .map_err(|e| {
                BackupError::StorageUpload(format!(
                    "Failed to create local storage directory '{}': {}",
                    self.target_dir.display(),
                    e
                ))
            })?;

        let target_path = self.target_dir.join(filename);

        // 2. Perform the copy operation asynchronously
        tokio::fs::copy(source, &target_path)
            .await
            .map_err(|e| {
                BackupError::StorageUpload(format!(
                    "Failed to copy backup file from '{}' to '{}': {}",
                    source.display(),
                    target_path.display(),
                    e
                ))
            })?;

        // 3. Obtain the full absolute path as the identifier
        let absolute_path = tokio::fs::canonicalize(&target_path)
            .await
            .unwrap_or(target_path);

        Ok(absolute_path.to_string_lossy().to_string())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_storage_saves_file_successfully() {
        let temp_dir = std::env::temp_dir();
        let unique_id = uuid::Uuid::new_v4();

        let source_dir = temp_dir.join(format!("local_storage_src_{}", unique_id));
        let target_dir = temp_dir.join(format!("local_storage_tgt_{}", unique_id));

        tokio::fs::create_dir_all(&source_dir).await.unwrap();

        let src_file = source_dir.join("db_backup.zip");
        let content = b"Dummy ZIP File Content";
        tokio::fs::write(&src_file, content).await.unwrap();

        let storage = LocalStorage::new(target_dir.clone());
        let result_path_str = storage
            .upload(&src_file, "db_backup_stored.zip")
            .await
            .expect("Upload failed");

        let result_path = Path::new(&result_path_str);
        assert!(result_path.exists(), "Stored file should exist");
        assert_eq!(
            result_path.file_name().unwrap().to_str().unwrap(),
            "db_backup_stored.zip"
        );

        // Verify content matches
        let saved_content = tokio::fs::read(result_path).await.unwrap();
        assert_eq!(saved_content, content, "File content corrupted during storage copy");

        // Clean up
        let _ = tokio::fs::remove_dir_all(&source_dir).await;
        let _ = tokio::fs::remove_dir_all(&target_dir).await;
    }

    #[tokio::test]
    async fn local_storage_fails_if_source_missing() {
        let temp_dir = std::env::temp_dir();
        let unique_id = uuid::Uuid::new_v4();
        let target_dir = temp_dir.join(format!("local_storage_tgt_{}", unique_id));

        let missing_src = temp_dir.join(format!("non_existent_{}.zip", unique_id));
        let storage = LocalStorage::new(target_dir.clone());

        let result = storage.upload(&missing_src, "db.zip").await;
        assert!(result.is_err(), "Copy of non-existent source file should fail");

        if let Err(BackupError::StorageUpload(msg)) = result {
            assert!(msg.contains("Failed to copy backup file"));
        } else {
            panic!("Expected BackupError::StorageUpload");
        }

        // Clean up target dir if created
        let _ = tokio::fs::remove_dir_all(&target_dir).await;
    }
}
