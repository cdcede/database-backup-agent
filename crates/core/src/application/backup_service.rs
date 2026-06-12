//! Backup orchestrator implementation.
//!
//! Coordinates the database backup, file compression, remote storage,
//! and notification ports into a single transactional execution pipeline.

use std::path::Path;

use crate::domain::backup_job::BackupJob;
use crate::domain::backup_result::{BackupResult, BackupStatus};
use crate::ports::database::DatabaseBackup;
use crate::ports::compression::FileCompressor;
use crate::ports::storage::Storage;
use crate::ports::notifier::Notifier;

/// Orchestrates the execution of a backup pipeline.
pub struct BackupOrchestrator<D, C, S, N> {
    db: D,
    compressor: C,
    storage: S,
    notifier: N,
}

impl<D, C, S, N> BackupOrchestrator<D, C, S, N>
where
    D: DatabaseBackup,
    C: FileCompressor,
    S: Storage,
    N: Notifier,
{
    /// Create a new `BackupOrchestrator` with its required port dependencies.
    pub fn new(db: D, compressor: C, storage: S, notifier: N) -> Self {
        Self {
            db,
            compressor,
            storage,
            notifier,
        }
    }

    /// Execute the backup pipeline for a single database.
    ///
    /// The pipeline follows these sequential stages:
    /// 1. Create a `BackupJob` entity tracking progress.
    /// 2. Perform raw database backup (`DatabaseBackup`) -> produces a `.bak` file.
    /// 3. Compress the `.bak` file to a `.zip` archive (`FileCompressor`).
    /// 4. Eagerly clean up the temporary `.bak` file to conserve disk space.
    /// 5. Upload the `.zip` file to remote storage (`Storage`).
    /// 6. Clean up the temporary `.zip` file.
    /// 7. Send notification (success or failure) via the `Notifier`.
    /// 8. Return the final `BackupResult` for storage in job history.
    pub async fn run_backup(
        &self,
        database: &str,
        temp_dir: &Path,
        storage_filename: &str,
    ) -> BackupResult {
        let mut job = BackupJob::new(database);
        job.start();

        tracing::info!(
            job_id = %job.id,
            database = database,
            "Starting scheduled backup operation"
        );

        // ---------------------------------------------------------------------
        // Stage 1: Database Backup
        // ---------------------------------------------------------------------
        let backup_info = match self.db.execute_backup(database, temp_dir).await {
            Ok(info) => info,
            Err(e) => {
                let err_msg = format!("Database backup failed: {}", e);
                tracing::error!(job_id = %job.id, error = %err_msg);
                
                job.fail(&err_msg);
                let _ = self.notifier.send_failure(database, &err_msg).await;
                
                return BackupResult::from_completed_job(&job, 0, None, "");
            }
        };

        let raw_backup_path = backup_info.backup_path.clone();
        let raw_size_bytes = backup_info.size_bytes;

        // ---------------------------------------------------------------------
        // Stage 2: Compression
        // ---------------------------------------------------------------------
        job.set_status(BackupStatus::Compressing);
        
        let zip_filename = format!("{}.zip", uuid::Uuid::new_v4());
        let zip_path = temp_dir.join(zip_filename);

        let (compressed_path, compressed_size_bytes) = match self
            .compressor
            .compress_file(&raw_backup_path, &zip_path)
            .await
        {
            Ok((path, size)) => (path, size),
            Err(e) => {
                let err_msg = format!("ZIP compression failed: {}", e);
                tracing::error!(job_id = %job.id, error = %err_msg);
                
                // Cleanup intermediate raw file
                let _ = tokio::fs::remove_file(&raw_backup_path).await;
                
                job.fail(&err_msg);
                let _ = self.notifier.send_failure(database, &err_msg).await;
                
                return BackupResult::from_completed_job(&job, raw_size_bytes, None, "");
            }
        };

        // Eager cleanup: remove raw .bak file since compression succeeded
        let _ = tokio::fs::remove_file(&raw_backup_path).await;

        // ---------------------------------------------------------------------
        // Stage 3: Storage Upload
        // ---------------------------------------------------------------------
        job.set_status(BackupStatus::Uploading);

        let storage_destination = match self.storage.upload(&compressed_path, storage_filename).await {
            Ok(destination) => destination,
            Err(e) => {
                let err_msg = format!("Storage upload failed: {}", e);
                tracing::error!(job_id = %job.id, error = %err_msg);
                
                // Cleanup intermediate compressed zip file
                let _ = tokio::fs::remove_file(&compressed_path).await;
                
                job.fail(&err_msg);
                let _ = self.notifier.send_failure(database, &err_msg).await;
                
                return BackupResult::from_completed_job(
                    &job,
                    raw_size_bytes,
                    Some(compressed_size_bytes),
                    "",
                );
            }
        };

        // Cleanup intermediate compressed zip file after successful upload
        let _ = tokio::fs::remove_file(&compressed_path).await;

        // ---------------------------------------------------------------------
        // Stage 4: Completion & Notification
        // ---------------------------------------------------------------------
        job.complete();
        
        let elapsed = job.elapsed_secs().unwrap_or(0);
        tracing::info!(
            job_id = %job.id,
            destination = %storage_destination,
            duration_secs = elapsed,
            "Backup operation completed successfully"
        );

        let _ = self
            .notifier
            .send_success(database, compressed_size_bytes, &storage_destination, elapsed)
            .await;

        BackupResult::from_completed_job(
            &job,
            raw_size_bytes,
            Some(compressed_size_bytes),
            storage_destination,
        )
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::Arc;
    use crate::ports::database::BackupInfo;
    use crate::domain::error::BackupError;

    // -------------------------------------------------------------------------
    // Mock Implementations for testing the orchestrator
    // -------------------------------------------------------------------------

    struct MockDb {
        should_fail: bool,
    }

    impl DatabaseBackup for MockDb {
        async fn execute_backup(
            &self,
            database: &str,
            backup_dir: &Path,
        ) -> Result<BackupInfo, BackupError> {
            if self.should_fail {
                return Err(BackupError::BackupExecution {
                    database: database.to_string(),
                    reason: "Database offline".into(),
                });
            }
            
            let mock_path = backup_dir.join("mock.bak");
            // Create a dummy file on disk for the compressor to read
            tokio::fs::write(&mock_path, b"mock database data").await.unwrap();

            Ok(BackupInfo {
                database_name: database.to_string(),
                backup_path: mock_path,
                size_bytes: 18,
            })
        }
    }

    struct MockCompressor {
        should_fail: bool,
    }

    impl FileCompressor for MockCompressor {
        async fn compress_file(
            &self,
            _source: &Path,
            destination: &Path,
        ) -> Result<(std::path::PathBuf, u64), BackupError> {
            if self.should_fail {
                return Err(BackupError::Compression("Disk space exhausted".into()));
            }

            // Write dummy ZIP file to destination
            tokio::fs::write(destination, b"mock zip data").await.unwrap();
            Ok((destination.to_path_buf(), 13))
        }
    }

    struct MockStorage {
        should_fail: bool,
    }

    impl Storage for MockStorage {
        async fn upload(&self, _source: &Path, filename: &str) -> Result<String, BackupError> {
            if self.should_fail {
                return Err(BackupError::StorageUpload("S3 Access Denied".into()));
            }
            Ok(format!("s3://test-bucket/{}", filename))
        }
    }

    #[derive(Default, Clone)]
    struct MockNotifierSharedState {
        successes: Vec<(String, u64, String)>,
        failures: Vec<(String, String)>,
    }

    struct MockNotifier {
        state: Arc<Mutex<MockNotifierSharedState>>,
    }

    impl Notifier for MockNotifier {
        async fn send_success(
            &self,
            database: &str,
            size_bytes: u64,
            destination: &str,
            _elapsed_secs: u64,
        ) -> Result<(), BackupError> {
            let mut state = self.state.lock().unwrap();
            state.successes.push((database.to_string(), size_bytes, destination.to_string()));
            Ok(())
        }

        async fn send_failure(&self, database: &str, reason: &str) -> Result<(), BackupError> {
            let mut state = self.state.lock().unwrap();
            state.failures.push((database.to_string(), reason.to_string()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn pipeline_success_flow() {
        let temp_dir = std::env::temp_dir().join(format!("orchestrator_success_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let db = MockDb { should_fail: false };
        let compressor = MockCompressor { should_fail: false };
        let storage = MockStorage { should_fail: false };
        
        let notifier_state = Arc::new(Mutex::new(MockNotifierSharedState::default()));
        let notifier = MockNotifier {
            state: notifier_state.clone(),
        };

        let orchestrator = BackupOrchestrator::new(db, compressor, storage, notifier);
        
        let result = orchestrator
            .run_backup("ProdDB", &temp_dir, "ProdDB_backup.zip")
            .await;

        // Verify result matches success expectations
        assert_eq!(result.database_name, "ProdDB");
        assert_eq!(result.status, BackupStatus::Completed);
        assert_eq!(result.backup_size_bytes, 18);
        assert_eq!(result.compressed_size_bytes, Some(13));
        assert_eq!(result.storage_destination, "s3://test-bucket/ProdDB_backup.zip");
        assert!(result.error_message.is_none());

        // Verify notifications
        let state = notifier_state.lock().unwrap();
        assert_eq!(state.successes.len(), 1);
        assert_eq!(state.successes[0].0, "ProdDB");
        assert_eq!(state.successes[0].1, 13);
        assert_eq!(state.successes[0].2, "s3://test-bucket/ProdDB_backup.zip");
        assert_eq!(state.failures.len(), 0);

        // Verify temporary files cleaned up
        let mock_bak = temp_dir.join("mock.bak");
        let mock_zip = temp_dir.join("mock.zip");
        assert!(!mock_bak.exists());
        assert!(!mock_zip.exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn pipeline_fails_on_database_error() {
        let temp_dir = std::env::temp_dir().join(format!("orchestrator_fail_db_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let db = MockDb { should_fail: true };
        let compressor = MockCompressor { should_fail: false };
        let storage = MockStorage { should_fail: false };
        
        let notifier_state = Arc::new(Mutex::new(MockNotifierSharedState::default()));
        let notifier = MockNotifier {
            state: notifier_state.clone(),
        };

        let orchestrator = BackupOrchestrator::new(db, compressor, storage, notifier);
        
        let result = orchestrator
            .run_backup("ProdDB", &temp_dir, "backup.zip")
            .await;

        assert_eq!(result.status, BackupStatus::Failed);
        assert!(result.error_message.unwrap().contains("Database backup failed"));

        let state = notifier_state.lock().unwrap();
        assert_eq!(state.successes.len(), 0);
        assert_eq!(state.failures.len(), 1);
        assert!(state.failures[0].1.contains("Database backup failed"));

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn pipeline_fails_on_compression_error() {
        let temp_dir = std::env::temp_dir().join(format!("orchestrator_fail_zip_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let db = MockDb { should_fail: false };
        let compressor = MockCompressor { should_fail: true };
        let storage = MockStorage { should_fail: false };
        
        let notifier_state = Arc::new(Mutex::new(MockNotifierSharedState::default()));
        let notifier = MockNotifier {
            state: notifier_state.clone(),
        };

        let orchestrator = BackupOrchestrator::new(db, compressor, storage, notifier);
        
        let result = orchestrator
            .run_backup("ProdDB", &temp_dir, "backup.zip")
            .await;

        assert_eq!(result.status, BackupStatus::Failed);
        assert!(result.error_message.unwrap().contains("ZIP compression failed"));

        // Verify intermediate raw file cleaned up
        let mock_bak = temp_dir.join("mock.bak");
        assert!(!mock_bak.exists(), "Raw backup file should be eagerly cleaned up");

        let state = notifier_state.lock().unwrap();
        assert_eq!(state.successes.len(), 0);
        assert_eq!(state.failures.len(), 1);
        assert!(state.failures[0].1.contains("ZIP compression failed"));

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
