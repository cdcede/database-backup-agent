//! Database backup port — the contract for executing database backups.
//!
//! Any database engine (SQL Server, PostgreSQL, etc.) must satisfy this trait.
//! The application layer only sees this trait — it never knows which engine
//! is behind it.

use std::path::{Path, PathBuf};

use crate::domain::error::BackupError;

/// Information about a completed backup file.
///
/// Returned by `DatabaseBackup::execute_backup` after the database engine
/// writes the backup file to disk.
#[derive(Debug, Clone)]
pub struct BackupInfo {
    /// Name of the database that was backed up.
    pub database_name: String,
    /// Full path to the generated backup file (.bak).
    pub backup_path: PathBuf,
    /// Size of the backup file in bytes.
    pub size_bytes: u64,
}

/// Port trait for database backup operations.
///
/// ## `async fn` in traits (Rust 1.75+)
///
/// Before Rust 1.75, you needed the `async-trait` crate to write async methods
/// in traits. Now it's native syntax. The only limitation: you can't use
/// `dyn DatabaseBackup` (trait objects) with async methods directly.
/// For that you'd still need `async-trait` or manual boxing. We use generics.
///
/// ## Why `Send + Sync`?
///
/// Tokio is a multi-threaded runtime. `Send` means the type can be
/// transferred between threads. `Sync` means it can be shared (via `&`)
/// across threads. Without these bounds, the trait couldn't be used
/// from async tasks.
pub trait DatabaseBackup: Send + Sync {
    /// Execute a backup of the specified database.
    ///
    /// - `database`: Name of the database to back up.
    /// - `backup_dir`: Directory where the backup file should be created.
    ///
    /// The implementation generates the filename (usually with a timestamp)
    /// and returns `BackupInfo` with the full path and size.
    fn execute_backup(
        &self,
        database: &str,
        backup_dir: &Path,
    ) -> impl std::future::Future<Output = Result<BackupInfo, BackupError>> + Send;
}
