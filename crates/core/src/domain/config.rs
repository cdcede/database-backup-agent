//! Application configuration types.
//!
//! These structs define the SHAPE of the configuration.
//! Loading/saving (the HOW) lives in `infrastructure::config_loader`.
//!
//! Each struct maps to a TOML section:
//!   [sql_server]  → SqlServerConfig
//!   [backup]      → BackupConfig
//!   [telegram]    → TelegramConfig
//!   [storage]     → StorageConfig
//!   [storage.s3]  → S3Config
//!   [service]     → ServiceConfig

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::BackupError;

// =============================================================================
// Type Definitions
// =============================================================================

/// Top-level application configuration — mirrors the entire `config.toml`.
///
/// `#[serde(default)]` on a field means: "if this key is missing in the TOML,
/// use `Default::default()` for that type instead of failing."
/// We use it on `telegram` and `storage` because those sections are optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub sql_server: SqlServerConfig,
    pub backup: BackupConfig,
    #[serde(default)]
    pub tasks: Vec<BackupTaskConfig>,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    /// Service runtime tuning (history cap, log rotation threshold).
    /// The entire `[service]` section is optional — omitting it uses defaults.
    #[serde(default)]
    pub service: ServiceConfig,
}

/// Service runtime configuration.
///
/// Maps to the `[service]` TOML section. All fields have serde defaults so
/// existing `config.toml` files that lack this section keep working.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Maximum number of backup history entries to keep on disk.
    /// Oldest entries are removed first when the cap is exceeded.
    /// Default: 500. A value of 0 is treated as unconfigured and falls back to 500.
    #[serde(default = "default_history_max_entries")]
    pub history_max_entries: usize,
    /// Size threshold (bytes) at which `backup-agent.log` is rotated on startup.
    /// Default: 10 MiB (10_485_760 bytes).
    /// NOTE: this field is read during config validation but the actual rotation
    /// in `main.rs` uses the hardcoded 10 MiB default because the config is not
    /// yet loaded at log-rotation time. Configurable threshold is a future enhancement.
    #[serde(default = "default_log_rotate_threshold_bytes")]
    pub log_rotate_threshold_bytes: u64,
}

fn default_history_max_entries() -> usize {
    500
}

fn default_log_rotate_threshold_bytes() -> u64 {
    10 * 1024 * 1024
}

/// SQL Server connection settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SqlServerConfig {
    pub host: String,
    pub port: u16,
    pub auth_method: AuthMethod,
    /// `Option<String>` — can be `Some("sa")` or `None` (absent in TOML).
    /// `#[serde(default)]` makes serde use `None` when the key is missing.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

/// SQL Server authentication method.
///
/// `#[serde(rename_all = "lowercase")]` maps Rust variants to lowercase TOML strings:
///   `AuthMethod::Sql` ↔ `"sql"` in TOML
///   `AuthMethod::Windows` ↔ `"windows"` in TOML
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Sql,
    Windows,
}

/// Backup global local settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupConfig {
    /// `PathBuf` is the OWNED version of `Path` — like `String` is to `&str`.
    /// Use `PathBuf` when you need to store a path. Use `&Path` when borrowing.
    pub local_path: PathBuf,
}

/// Individual backup task/schedule configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupTaskConfig {
    pub name: String,
    pub databases: Vec<String>,
    pub schedule: String,
    pub retention_days: u32,
}

/// Telegram notification settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub bot_token: String,
    pub chat_id: String,
}

/// Storage provider configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageConfig {
    pub provider: StorageProviderType,
    #[serde(default)]
    pub s3: Option<S3Config>,
}

/// Supported storage backends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageProviderType {
    Local,
    S3,
}

/// S3-compatible storage settings (AWS, MinIO, DigitalOcean Spaces, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

// =============================================================================
// Default Implementations
//
// The `Default` trait provides a "zero config" starting point.
// Used by:
//   1. `load_or_create_default()` to generate a starter config.toml
//   2. `#[serde(default)]` to fill missing TOML sections
// =============================================================================

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            sql_server: SqlServerConfig::default(),
            backup: BackupConfig::default(),
            tasks: vec![BackupTaskConfig::default()],
            telegram: TelegramConfig::default(),
            storage: StorageConfig::default(),
            service: ServiceConfig::default(),
        }
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            history_max_entries: default_history_max_entries(),
            log_rotate_threshold_bytes: default_log_rotate_threshold_bytes(),
        }
    }
}

impl Default for SqlServerConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 1433,
            auth_method: AuthMethod::Sql,
            username: Some("sa".to_string()),
            password: Some(String::new()), // Empty — validation will flag this
        }
    }
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            // `cfg!()` is a compile-time check that returns `true` or `false`.
            // Different from `#[cfg()]` which conditionally includes/excludes code.
            local_path: PathBuf::from(if cfg!(windows) {
                r"C:\Backups\BackupAgent"
            } else {
                "/tmp/backup-agent"
            }),
        }
    }
}

impl Default for BackupTaskConfig {
    fn default() -> Self {
        Self {
            name: "Default Backup Task".to_string(),
            databases: vec!["MyDatabase".to_string()],
            schedule: "02:00".to_string(),
            retention_days: 7,
        }
    }
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            chat_id: String::new(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            provider: StorageProviderType::Local,
            s3: None,
        }
    }
}

// =============================================================================
// Validation
//
// Validation is DOMAIN logic — it encodes business rules about what
// constitutes a valid configuration. That's why it lives here, not in
// infrastructure.
// =============================================================================

impl AppConfig {
    /// Validate the entire configuration. Returns a list of human-readable errors.
    ///
    /// Returns an empty `Vec` if everything is valid.
    /// This approach lets the GUI show ALL problems at once instead of
    /// stopping at the first error.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        self.validate_sql_server(&mut errors);
        self.validate_backup(&mut errors);
        self.validate_tasks(&mut errors);
        self.validate_telegram(&mut errors);
        self.validate_storage(&mut errors);
        self.validate_service(&mut errors);
        errors
    }

    /// Like `validate()`, but returns a `Result` for use with the `?` operator.
    ///
    /// Joins all error messages with "; " into a single `BackupError::Config`.
    pub fn ensure_valid(&self) -> Result<(), BackupError> {
        let errors = self.validate();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(BackupError::Config(errors.join("; ")))
        }
    }

    /// `&mut Vec<String>` — we pass a mutable borrow of the error list
    /// so each helper can APPEND to the same collection.
    fn validate_sql_server(&self, errors: &mut Vec<String>) {
        if self.sql_server.auth_method == AuthMethod::Sql {
            // `as_deref()` converts `Option<String>` → `Option<&str>`
            // `unwrap_or("")` gives `""` if `None`
            // So this checks: is it None OR is it an empty string?
            if self.sql_server.username.as_deref().unwrap_or("").is_empty() {
                errors.push("SQL authentication requires a username".into());
            }
            if self.sql_server.password.as_deref().unwrap_or("").is_empty() {
                errors.push("SQL authentication requires a password".into());
            }
        }
    }

    fn validate_backup(&self, errors: &mut Vec<String>) {
        if self.backup.local_path.to_string_lossy().trim().is_empty() {
            errors.push("Local destination path cannot be empty".into());
        }
    }

    fn validate_tasks(&self, errors: &mut Vec<String>) {
        if self.tasks.is_empty() {
            errors.push("At least one backup task must be configured".into());
            return;
        }

        for (idx, task) in self.tasks.iter().enumerate() {
            let prefix = if task.name.trim().is_empty() {
                format!("Task {}:", idx + 1)
            } else {
                format!("Task '{}':", task.name)
            };

            if task.name.trim().is_empty() {
                errors.push(format!("Task {} has an empty name", idx + 1));
            }
            if task.databases.is_empty() {
                errors.push(format!("{} At least one database must be selected", prefix));
            }
            if task.retention_days == 0 {
                errors.push(format!("{} Retention days must be greater than 0", prefix));
            }
            if !is_valid_schedule(&task.schedule) {
                errors.push(format!(
                    "{} Invalid schedule '{}': expected 24h (HH:MM) or a valid cron expression",
                    prefix, task.schedule
                ));
            }
        }
    }

    fn validate_telegram(&self, errors: &mut Vec<String>) {
        if self.telegram.enabled {
            if self.telegram.bot_token.is_empty() {
                errors.push("Telegram is enabled but bot_token is empty".into());
            }
            if self.telegram.chat_id.is_empty() {
                errors.push("Telegram is enabled but chat_id is empty".into());
            }
        }
    }

    fn validate_storage(&self, errors: &mut Vec<String>) {
        if self.storage.provider == StorageProviderType::S3 {
            match &self.storage.s3 {
                None => {
                    errors.push("S3 storage provider selected but storage.s3 section is missing".into());
                }
                Some(s3) => {
                    if s3.endpoint.is_empty() {
                        errors.push("S3 endpoint is empty".into());
                    }
                    if s3.bucket.is_empty() {
                        errors.push("S3 bucket is empty".into());
                    }
                    if s3.region.is_empty() {
                        errors.push("S3 region is empty".into());
                    }
                    if s3.access_key.is_empty() {
                        errors.push("S3 access_key is empty".into());
                    }
                    if s3.secret_key.is_empty() {
                        errors.push("S3 secret_key is empty".into());
                    }
                }
            }
        }
    }

    fn validate_service(&self, errors: &mut Vec<String>) {
        // history_max_entries = 0 is a misconfiguration; callers should treat it
        // as the default (500). We report it so users know the field was ignored.
        if self.service.history_max_entries == 0 {
            errors.push(
                "service.history_max_entries is 0 — must be > 0 (defaulting to 500)".into(),
            );
        }
        if self.service.log_rotate_threshold_bytes < 1024 {
            errors.push(
                "service.log_rotate_threshold_bytes must be >= 1024 (1 KiB)".into(),
            );
        }
    }
}

/// Validate schedule format: must be "HH:MM" or a cron expression.
fn is_valid_schedule(schedule: &str) -> bool {
    let schedule = schedule.trim();
    if schedule.is_empty() {
        return false;
    }

    // 1. Try to parse as HH:MM
    if schedule.contains(':') && !schedule.contains(' ') {
        let Some((hours_str, minutes_str)) = schedule.split_once(':') else {
            return false;
        };
        if hours_str.len() < 1 || hours_str.len() > 2 || minutes_str.len() != 2 {
            return false;
        }
        let Ok(hours) = hours_str.parse::<u32>() else {
            return false;
        };
        let Ok(minutes) = minutes_str.parse::<u32>() else {
            return false;
        };
        hours < 24 && minutes < 60
    } else {
        // 2. Try to parse as a cron expression (contains 5 to 7 whitespace-separated fields)
        let fields: Vec<&str> = schedule.split_whitespace().collect();
        fields.len() >= 5 && fields.len() <= 7
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a valid config (all checks pass).
    fn valid_config() -> AppConfig {
        let mut config = AppConfig::default();
        config.sql_server.password = Some("real_password".to_string());
        config
    }

    #[test]
    fn default_config_warns_about_empty_password() {
        let config = AppConfig::default();
        let errors = config.validate();
        assert!(
            errors.iter().any(|e| e.contains("password")),
            "Should flag empty password, got: {errors:?}"
        );
    }

    #[test]
    fn valid_config_passes_all_checks() {
        let errors = valid_config().validate();
        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
    }

    #[test]
    fn sql_auth_without_username_fails() {
        let mut config = valid_config();
        config.sql_server.username = None;
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("username")));
    }

    #[test]
    fn empty_databases_fails() {
        let mut config = valid_config();
        config.tasks[0].databases.clear();
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("database")));
    }

    #[test]
    fn invalid_schedule_format_fails() {
        let mut config = valid_config();
        config.tasks[0].schedule = "25:00".to_string();
        assert!(config.validate().iter().any(|e| e.contains("schedule")));

        config.tasks[0].schedule = "not-a-time".to_string();
        assert!(config.validate().iter().any(|e| e.contains("schedule")));
    }

    #[test]
    fn windows_auth_skips_credential_check() {
        let mut config = valid_config();
        config.sql_server.auth_method = AuthMethod::Windows;
        config.sql_server.username = None;
        config.sql_server.password = None;
        let errors = config.validate();
        assert!(errors.is_empty(), "Windows auth shouldn't require credentials: {errors:?}");
    }

    #[test]
    fn s3_provider_without_config_fails() {
        let mut config = valid_config();
        config.storage.provider = StorageProviderType::S3;
        config.storage.s3 = None;
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("S3")));
    }

    #[test]
    fn telegram_enabled_without_token_fails() {
        let mut config = valid_config();
        config.telegram.enabled = true;
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("bot_token")));
    }

    #[test]
    fn ensure_valid_returns_result() {
        let config = valid_config();
        assert!(config.ensure_valid().is_ok());

        let bad_config = AppConfig::default();
        assert!(bad_config.ensure_valid().is_err());
    }

    #[test]
    fn schedule_validation_edge_cases() {
        assert!(is_valid_schedule("00:00")); // Midnight
        assert!(is_valid_schedule("23:59")); // Last minute
        assert!(is_valid_schedule("2:00")); // Single digit hour (permissive)
        assert!(!is_valid_schedule("24:00")); // Hour too high
        assert!(!is_valid_schedule("12:60")); // Minute too high
        assert!(!is_valid_schedule("12")); // No colon
        assert!(!is_valid_schedule("")); // Empty
    }

    // -------------------------------------------------------------------------
    // T5.1 — ServiceConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn service_config_default_values() {
        let cfg = ServiceConfig::default();
        assert_eq!(cfg.history_max_entries, 500);
        assert_eq!(cfg.log_rotate_threshold_bytes, 10 * 1024 * 1024);
    }

    #[test]
    fn app_config_default_includes_service_defaults() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.service.history_max_entries, 500);
        assert_eq!(cfg.service.log_rotate_threshold_bytes, 10_485_760);
    }

    #[test]
    fn toml_without_service_section_loads_defaults() {
        // A minimal TOML that does not include [service] must still deserialize
        // with default ServiceConfig values (backward-compatibility).
        let toml = r#"
            [sql_server]
            host = "localhost"
            port = 1433
            auth_method = "sql"
            username = "sa"
            password = "secret"

            [backup]
            local_path = "/tmp/test"

            [storage]
            provider = "local"
        "#;
        let cfg: AppConfig = toml::from_str(toml).expect("Should parse without [service]");
        assert_eq!(cfg.service.history_max_entries, 500);
        assert_eq!(cfg.service.log_rotate_threshold_bytes, 10_485_760);
    }

    #[test]
    fn toml_with_service_section_respected() {
        let toml = r#"
            [sql_server]
            host = "localhost"
            port = 1433
            auth_method = "sql"
            username = "sa"
            password = "secret"

            [backup]
            local_path = "/tmp/test"

            [storage]
            provider = "local"

            [service]
            history_max_entries = 200
            log_rotate_threshold_bytes = 5242880
        "#;
        let cfg: AppConfig = toml::from_str(toml).expect("Should parse with [service]");
        assert_eq!(cfg.service.history_max_entries, 200);
        assert_eq!(cfg.service.log_rotate_threshold_bytes, 5_242_880);
    }

    #[test]
    fn validate_rejects_history_max_entries_zero() {
        let mut config = valid_config();
        config.service.history_max_entries = 0;
        let errors = config.validate();
        assert!(
            errors.iter().any(|e| e.contains("history_max_entries")),
            "Should flag history_max_entries = 0, got: {errors:?}"
        );
    }

    #[test]
    fn validate_rejects_log_rotate_threshold_below_1kib() {
        let mut config = valid_config();
        config.service.log_rotate_threshold_bytes = 512;
        let errors = config.validate();
        assert!(
            errors.iter().any(|e| e.contains("log_rotate_threshold_bytes")),
            "Should flag threshold < 1024, got: {errors:?}"
        );
    }

    #[test]
    fn validate_accepts_valid_service_config() {
        let config = valid_config();
        // Default ServiceConfig should pass validation
        let errors = config.validate();
        assert!(
            !errors.iter().any(|e| e.contains("service")),
            "Default service config should pass validation, got: {errors:?}"
        );
    }
}
