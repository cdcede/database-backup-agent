//! BackupJob entity — represents a backup operation in progress.
//!
//! This is the "live" object during execution. It tracks state transitions
//! as the backup moves through its lifecycle: Pending → InProgress → Compressing → ...
//!
//! Once completed, a `BackupResult` (see backup_result.rs) is created from it
//! for permanent storage in the history.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::backup_result::BackupStatus;

/// A single backup operation.
///
/// Created when a backup is triggered (scheduled or manual).
/// Mutated as the backup progresses through its lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackupJob {
    /// Unique identifier for this job. Generated once, never changes.
    pub id: Uuid,
    /// The SQL Server database being backed up.
    pub database_name: String,
    /// Current lifecycle state.
    pub status: BackupStatus,
    /// When this job was created (always UTC — convert to local only for display).
    pub created_at: DateTime<Utc>,
    /// When execution actually started. `None` while still `Pending`.
    pub started_at: Option<DateTime<Utc>>,
    /// When execution finished (success or failure). `None` while running.
    pub completed_at: Option<DateTime<Utc>>,
    /// Error details if the job failed. `None` on success.
    pub error_message: Option<String>,
}

impl BackupJob {
    /// Create a new backup job in `Pending` state.
    ///
    /// `Uuid::new_v4()` generates a random UUID.
    /// `Utc::now()` captures the current UTC timestamp.
    pub fn new(database_name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            database_name: database_name.into(),
            status: BackupStatus::Pending,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error_message: None,
        }
    }

    /// Transition to `InProgress` — the backup has started executing.
    ///
    /// `&mut self` means this method borrows the job MUTABLY —
    /// only one mutable reference can exist at a time (Rust's borrow checker).
    pub fn start(&mut self) {
        self.status = BackupStatus::InProgress;
        self.started_at = Some(Utc::now());
    }

    /// Mark the backup as successfully completed.
    pub fn complete(&mut self) {
        self.status = BackupStatus::Completed;
        self.completed_at = Some(Utc::now());
    }

    /// Mark the backup as failed with a reason.
    pub fn fail(&mut self, reason: impl Into<String>) {
        self.status = BackupStatus::Failed;
        self.completed_at = Some(Utc::now());
        self.error_message = Some(reason.into());
    }

    /// Update the status (used for intermediate states like Compressing, Uploading).
    pub fn set_status(&mut self, status: BackupStatus) {
        self.status = status;
    }

    /// Calculate how long this job has been running (or ran in total).
    /// Returns `None` if the job hasn't started yet.
    pub fn elapsed_secs(&self) -> Option<u64> {
        let started = self.started_at?; // `?` on Option: return None if None
        let end = self.completed_at.unwrap_or_else(Utc::now);
        let duration = end.signed_duration_since(started);
        Some(duration.num_seconds().unsigned_abs())
    }
}

// =============================================================================
// Tests — run with `cargo test -p backup-agent-core`
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_job_starts_as_pending() {
        let job = BackupJob::new("TestDB");
        assert_eq!(job.status, BackupStatus::Pending);
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());
        assert!(job.error_message.is_none());
    }

    #[test]
    fn job_lifecycle_success() {
        let mut job = BackupJob::new("TestDB");

        job.start();
        assert_eq!(job.status, BackupStatus::InProgress);
        assert!(job.started_at.is_some());

        job.set_status(BackupStatus::Compressing);
        assert_eq!(job.status, BackupStatus::Compressing);

        job.complete();
        assert_eq!(job.status, BackupStatus::Completed);
        assert!(job.completed_at.is_some());
        assert!(job.elapsed_secs().is_some());
    }

    #[test]
    fn job_failure_captures_reason() {
        let mut job = BackupJob::new("TestDB");
        job.start();
        job.fail("Disk full");

        assert_eq!(job.status, BackupStatus::Failed);
        assert_eq!(job.error_message.as_deref(), Some("Disk full"));
    }

    #[test]
    fn elapsed_secs_is_none_before_start() {
        let job = BackupJob::new("TestDB");
        assert!(job.elapsed_secs().is_none());
    }
}
