//! Config file I/O — loading, saving, and creating default configuration.
//!
//! This is INFRASTRUCTURE because it deals with the filesystem (an external concern).
//! The domain types (AppConfig, etc.) don't know or care about files.

use std::fs;
use std::path::Path;

use crate::domain::config::AppConfig;
use crate::domain::error::BackupError;

/// Load configuration from a TOML file.
///
/// ## Error handling — two patterns in one function
///
/// 1. `fs::read_to_string(path)?` uses the `?` operator.
///    This works because `BackupError` has `#[from] io::Error` — the compiler
///    auto-converts `io::Error` → `BackupError::Io(e)`.
///
/// 2. `toml::from_str(&content).map_err(|e| ...)?` uses `.map_err()`.
///    There's no `From<toml::de::Error>` impl, so we MANUALLY convert
///    the error into `BackupError::Config(String)`.
///
/// Rule of thumb:
/// - Use `#[from]` when there's a 1:1 mapping between error types.
/// - Use `.map_err()` when you need to add context or the mapping isn't direct.
pub fn load(path: &Path) -> Result<AppConfig, BackupError> {
    let content = fs::read_to_string(path).map_err(|e| {
        BackupError::Config(format!("Cannot read '{}': {e}", path.display()))
    })?;

    toml::from_str(&content).map_err(|e| {
        BackupError::Config(format!("Invalid TOML in '{}': {e}", path.display()))
    })
}

/// Save configuration to a TOML file.
///
/// `&AppConfig` — we BORROW the config (read-only). The caller keeps ownership.
/// `toml::to_string_pretty` produces human-readable TOML with alignment.
pub fn save(config: &AppConfig, path: &Path) -> Result<(), BackupError> {
    let content = toml::to_string_pretty(config).map_err(|e| {
        BackupError::Config(format!("Failed to serialize config: {e}"))
    })?;

    // Create parent directories if they don't exist.
    // `if let Some(parent)` — destructure the Option only if it's Some.
    // `path.parent()` returns `None` only for root paths like "/" or "C:\".
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?; // io::Error → BackupError::Io via #[from]
    }

    fs::write(path, content)?; // io::Error → BackupError::Io via #[from]
    Ok(())
}

/// Load config if it exists, or create a default one and save it.
///
/// Useful for first-time setup: the app generates a starter `config.toml`
/// that the user can then edit.
pub fn load_or_create_default(path: &Path) -> Result<AppConfig, BackupError> {
    if path.exists() {
        load(path)
    } else {
        let config = AppConfig::default();
        save(&config, path)?;
        Ok(config)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::{AuthMethod, StorageProviderType};

    /// Create a unique temp file path per test to avoid conflicts.
    /// Uses UUID so parallel test runs never collide.
    fn temp_path() -> std::path::PathBuf {
        let id = uuid::Uuid::new_v4();
        std::env::temp_dir()
            .join("backup-agent-tests")
            .join(format!("{id}.toml"))
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_path();
        let original = AppConfig::default();

        save(&original, &path).expect("save failed");
        let loaded = load(&path).expect("load failed");

        assert_eq!(loaded.sql_server.host, original.sql_server.host);
        assert_eq!(loaded.sql_server.port, original.sql_server.port);
        assert_eq!(loaded.sql_server.auth_method, original.sql_server.auth_method);
        assert_eq!(loaded.tasks, original.tasks);
        assert_eq!(loaded.telegram.enabled, original.telegram.enabled);
        assert_eq!(loaded.storage.provider, original.storage.provider);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_nonexistent_file_returns_error() {
        let path = temp_path();
        let result = load(&path);
        assert!(result.is_err(), "Loading a missing file should fail");
    }

    #[test]
    fn load_invalid_toml_returns_error() {
        let path = temp_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "this is {{ not valid }} toml").unwrap();

        let result = load(&path);
        assert!(result.is_err(), "Invalid TOML should fail to parse");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_or_create_default_creates_file_when_missing() {
        let path = temp_path();
        assert!(!path.exists());

        let config = load_or_create_default(&path).expect("should create default");

        assert!(path.exists(), "File should have been created");
        assert_eq!(config.sql_server.host, "localhost");
        assert_eq!(config.sql_server.port, 1433);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_or_create_default_loads_existing() {
        let path = temp_path();

        // Create a custom config first
        let mut custom = AppConfig::default();
        custom.sql_server.host = "db-prod".to_string();
        custom.sql_server.port = 2433;
        save(&custom, &path).unwrap();

        // load_or_create_default should load the existing file, not overwrite it
        let loaded = load_or_create_default(&path).expect("should load existing");
        assert_eq!(loaded.sql_server.host, "db-prod");
        assert_eq!(loaded.sql_server.port, 2433);

        let _ = fs::remove_file(&path);
    }

    /// This test verifies that our Rust types correctly parse a realistic TOML config.
    /// If we change the types and break compatibility, this test catches it.
    #[test]
    fn parse_full_config_from_toml_string() {
        let toml_str = r#"
[sql_server]
host = "sql-prod.example.com"
port = 1433
auth_method = "sql"
username = "backup_user"
password = "s3cret!"

[backup]
local_path = "D:\\SQLBackups"

[[tasks]]
name = "Daily Backup"
databases = ["OrdersDB", "UsersDB", "AnalyticsDB"]
schedule = "03:30"
retention_days = 14

[telegram]
enabled = true
bot_token = "123456:ABC-DEF"
chat_id = "-1001234567890"

[storage]
provider = "s3"

[storage.s3]
endpoint = "https://s3.amazonaws.com"
bucket = "my-backups"
region = "us-east-1"
access_key = "AKIAIOSFODNN7EXAMPLE"
secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();

        assert_eq!(config.sql_server.host, "sql-prod.example.com");
        assert_eq!(config.sql_server.auth_method, AuthMethod::Sql);
        assert_eq!(config.tasks.len(), 1);
        assert_eq!(config.tasks[0].name, "Daily Backup");
        assert_eq!(config.tasks[0].databases.len(), 3);
        assert_eq!(config.tasks[0].schedule, "03:30");
        assert!(config.telegram.enabled);
        assert_eq!(config.storage.provider, StorageProviderType::S3);
        assert_eq!(config.storage.s3.as_ref().unwrap().bucket, "my-backups");
    }

    /// Verify that optional sections (telegram, storage) get defaults when missing.
    /// This is the `#[serde(default)]` behavior in action.
    #[test]
    fn parse_minimal_config_fills_defaults() {
        let toml_str = r#"
[sql_server]
host = "db-server"
port = 1433
auth_method = "windows"

[backup]
local_path = "D:\\Backups"

[[tasks]]
name = "Prod Tasks"
databases = ["ProdDB"]
schedule = "02:00"
retention_days = 7
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();

        // These were missing in TOML → filled by Default
        assert!(!config.telegram.enabled);
        assert_eq!(config.storage.provider, StorageProviderType::Local);

        // Windows auth → no credentials needed
        assert!(config.sql_server.username.is_none());
        assert!(config.sql_server.password.is_none());
        assert_eq!(config.tasks.len(), 1);
        assert_eq!(config.tasks[0].databases[0], "ProdDB");
    }
}
