//! Notifier port — the contract for sending notifications.
//!
//! Provides a unified interface for notifying users about backup job outcomes
//! (successes or failures) via notification channels like Telegram.

use crate::domain::error::BackupError;

/// Port trait for backup notification channels.
///
/// Implementations define how success and failure alerts are formatted and
/// transmitted to external notification systems.
pub trait Notifier: Send + Sync {
    /// Send a notification indicating a backup job completed successfully.
    ///
    /// - `database`: The database that was backed up.
    /// - `size_bytes`: The size of the backup file in bytes.
    /// - `destination`: The storage location where the backup is saved.
    /// - `elapsed_secs`: The time taken to execute the backup.
    fn send_success(
        &self,
        database: &str,
        size_bytes: u64,
        destination: &str,
        elapsed_secs: u64,
    ) -> impl std::future::Future<Output = Result<(), BackupError>> + Send;

    /// Send a notification indicating a backup job failed.
    ///
    /// - `database`: The database that failed to back up.
    /// - `reason`: The error reason details.
    fn send_failure(
        &self,
        database: &str,
        reason: &str,
    ) -> impl std::future::Future<Output = Result<(), BackupError>> + Send;
}
