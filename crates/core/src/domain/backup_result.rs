//! BackupResult and BackupStatus types.
//!
//! `BackupStatus` — the lifecycle states of a backup operation.
//! `BackupResult` — immutable record of a completed backup (used in history).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle states of a backup operation.
///
/// This is a Rust enum — much more powerful than enums in C# or Java.
/// Each variant can optionally carry data (we keep these simple for now).
///
/// The manual `Display` impl below shows you how to implement a trait yourself,
/// without relying on derive macros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackupStatus {
    /// Job created, waiting to start.
    Pending,
    /// BACKUP DATABASE is running.
    InProgress,
    /// .bak file is being compressed to .zip.
    Compressing,
    /// .zip is being uploaded to remote storage.
    Uploading,
    /// Everything succeeded.
    Completed,
    /// Something went wrong (details in `BackupJob.error_message`).
    Failed,
}

/// Manual implementation of the `Display` trait.
///
/// `Display` is what gets called when you use `{}` in `format!` / `println!`.
/// We implement it manually here (instead of deriving) to show you how traits work:
///
/// 1. `impl TraitName for TypeName` — "I'm implementing this trait for this type"
/// 2. `fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result` — the required method
/// 3. `match self { ... }` — pattern matching on enum variants
/// 4. `write!(f, "...")` — write formatted text to the output
impl std::fmt::Display for BackupStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::InProgress => write!(f, "In Progress"),
            Self::Compressing => write!(f, "Compressing"),
            Self::Uploading => write!(f, "Uploading"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed => write!(f, "Failed"),
        }
    }
}

/// Immutable record of a completed backup.
///
/// Created from a `BackupJob` once it finishes (success or failure).
/// This is what gets stored in the history and displayed in the GUI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackupResult {
    /// Links back to the original `BackupJob`.
    pub job_id: Uuid,
    /// Database that was backed up.
    pub database_name: String,
    /// Final status (Completed or Failed).
    pub status: BackupStatus,
    /// When the backup started.
    pub started_at: DateTime<Utc>,
    /// When the backup finished.
    pub completed_at: DateTime<Utc>,
    /// Total duration in seconds.
    pub duration_secs: u64,
    /// Size of the raw .bak file in bytes.
    pub backup_size_bytes: u64,
    /// Size of the .zip file in bytes. `None` if compression was skipped or failed.
    pub compressed_size_bytes: Option<u64>,
    /// Where the backup was stored (e.g., "local:/path/to/file" or "s3://bucket/key").
    pub storage_destination: String,
    /// Error details if the backup failed.
    pub error_message: Option<String>,
}

impl BackupResult {
    /// Create a `BackupResult` from a completed `BackupJob`.
    ///
    /// Takes a `&BackupJob` (immutable borrow) — we READ from the job, we don't consume it.
    /// `.clone()` creates an explicit copy of owned data (`String`).
    /// `unwrap_or` / `unwrap_or_else` provide fallback values for `Option<T>`.
    pub fn from_completed_job(
        job: &super::backup_job::BackupJob,
        backup_size_bytes: u64,
        compressed_size_bytes: Option<u64>,
        storage_destination: impl Into<String>,
    ) -> Self {
        let started = job.started_at.unwrap_or(job.created_at);
        let completed = job.completed_at.unwrap_or_else(Utc::now);
        let duration = completed.signed_duration_since(started);

        Self {
            job_id: job.id,
            database_name: job.database_name.clone(),
            status: job.status.clone(),
            started_at: started,
            completed_at: completed,
            duration_secs: duration.num_seconds().unsigned_abs(),
            backup_size_bytes,
            compressed_size_bytes,
            storage_destination: storage_destination.into(),
            error_message: job.error_message.clone(),
        }
    }

    /// Human-readable backup size (e.g., "1.2 GB", "340 MB").
    pub fn human_size(&self) -> String {
        humanize_bytes(self.backup_size_bytes)
    }

    /// Human-readable compressed size.
    pub fn human_compressed_size(&self) -> Option<String> {
        self.compressed_size_bytes.map(humanize_bytes)
    }
}

/// Convert bytes to a human-readable string.
pub fn humanize_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_status() {
        assert_eq!(BackupStatus::InProgress.to_string(), "In Progress");
        assert_eq!(BackupStatus::Completed.to_string(), "Completed");
        assert_eq!(BackupStatus::Failed.to_string(), "Failed");
    }

    #[test]
    fn humanize_bytes_formatting() {
        assert_eq!(humanize_bytes(500), "500 B");
        assert_eq!(humanize_bytes(1024), "1.0 KB");
        assert_eq!(humanize_bytes(1_500_000), "1.4 MB");
        assert_eq!(humanize_bytes(2_500_000_000), "2.3 GB");
    }

    #[test]
    fn result_from_completed_job() {
        use super::super::backup_job::BackupJob;

        let mut job = BackupJob::new("TestDB");
        job.start();
        job.complete();

        let result = BackupResult::from_completed_job(
            &job,
            1_000_000,
            Some(500_000),
            "local:C:\\Backups\\test.zip",
        );

        assert_eq!(result.database_name, "TestDB");
        assert_eq!(result.status, BackupStatus::Completed);
        assert_eq!(result.backup_size_bytes, 1_000_000);
        assert_eq!(result.human_size(), "976.6 KB");
    }
}
