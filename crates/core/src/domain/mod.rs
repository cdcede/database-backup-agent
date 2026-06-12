//! Domain layer — the heart of the application.
//!
//! Contains entities, value objects, and error types.
//! This module has NO knowledge of infrastructure (SQL Server, S3, filesystem, etc.).

pub mod backup_job;
pub mod backup_result;
pub mod config;
pub mod error;

// Re-export core types so consumers can write:
//   use backup_agent_core::domain::BackupJob;
// instead of:
//   use backup_agent_core::domain::backup_job::BackupJob;
pub use backup_job::BackupJob;
pub use backup_result::{BackupResult, BackupStatus};
pub use config::{AppConfig, BackupConfig, BackupTaskConfig};
pub use error::BackupError;
