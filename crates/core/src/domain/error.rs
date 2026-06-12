//! Domain error types.
//!
//! These errors describe WHAT went wrong from a business perspective,
//! not HOW (that's infrastructure's concern).
//!
//! Uses `thiserror` to auto-implement `std::error::Error` and `Display`.

use std::io;

use thiserror::Error;

/// All errors that can occur during backup operations.
///
/// Each variant maps to a distinct failure mode in the backup pipeline.
/// The `#[error("...")]` attribute generates the `Display` implementation —
/// you never write `impl Display` manually when using `thiserror`.
#[derive(Debug, Error)]
pub enum BackupError {
    /// SQL Server connection failed (bad credentials, unreachable host, etc.)
    #[error("Failed to connect to SQL Server: {0}")]
    DatabaseConnection(String),

    /// The BACKUP DATABASE command failed.
    /// Uses named fields in the variant — Rust enums can hold structured data.
    #[error("Backup failed for database '{database}': {reason}")]
    BackupExecution { database: String, reason: String },

    /// ZIP compression failed.
    #[error("Compression failed: {0}")]
    Compression(String),

    /// Upload to storage provider failed.
    #[error("Storage upload failed: {0}")]
    StorageUpload(String),

    /// Telegram or other notification delivery failed.
    #[error("Notification failed: {0}")]
    Notification(String),

    /// Configuration file is invalid or missing.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Not enough disk space for the backup.
    #[error("Insufficient disk space: {available_bytes} bytes available, {required_bytes} bytes required")]
    InsufficientDiskSpace {
        required_bytes: u64,
        available_bytes: u64,
    },

    /// Wraps any `std::io::Error`.
    /// The `#[from]` attribute lets you use the `?` operator to auto-convert:
    ///   `std::fs::read("file")?`  →  automatically becomes `BackupError::Io(e)`
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}
