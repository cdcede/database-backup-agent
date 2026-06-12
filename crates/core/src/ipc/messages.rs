//! IPC request and response message types.
//!
//! Serves as the binary communication contract between the GUI presentation layer
//! and the background Windows Service.

use serde::{Deserialize, Serialize};

use crate::domain::config::{AppConfig, SqlServerConfig};
use crate::domain::backup_job::BackupJob;
use crate::domain::backup_result::BackupResult;

/// Requests sent from the GUI client to the background Windows Service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IpcRequest {
    /// Retrieve the service's active configuration.
    GetConfig,
    /// Update and persist a new configuration TOML.
    UpdateConfig(AppConfig),
    /// Trigger an immediate manual backup of a database.
    TriggerBackup {
        /// Name of the database to backup.
        database_name: String,
    },
    /// Fetch the status of current active backup operations.
    GetStatus,
    /// Fetch the historical records of completed backups.
    GetHistory,
    /// Test database connection settings and discover database names.
    TestConnection(SqlServerConfig),
}

/// Responses sent from the Windows Service back to the GUI client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IpcResponse {
    /// Generic confirmation indicating successful processing.
    Ok,
    /// Failure message containing detail logs.
    Error(String),
    /// The service's current configuration.
    Config(AppConfig),
    /// Statuses of the currently active backup jobs.
    Status(Vec<BackupJob>),
    /// Historical list of completed backup results.
    History(Vec<BackupResult>),
    /// List of databases discovered on connection test.
    Databases(Vec<String>),
}
