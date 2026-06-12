//! SQL Server backup implementation using tiberius.
//!
//! This is the "Adapter" that satisfies the `DatabaseBackup` port.
//! It knows about SQL Server specifics (T-SQL syntax, TDS protocol).
//! The rest of the application only sees the `DatabaseBackup` trait.
//!
//! ## Assumption
//!
//! The backup agent runs on the same machine as SQL Server (or has
//! access to the same filesystem via UNC path). This is the standard
//! setup for on-premises SQL Server backups.

use std::path::Path;

use chrono::Utc;
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

use crate::domain::config::{AuthMethod, SqlServerConfig};
use crate::domain::error::BackupError;
use crate::ports::database::{BackupInfo, DatabaseBackup};

/// Type alias — the full tiberius client type is verbose.
///
/// `Compat<TcpStream>` is a wrapper that adapts tokio's `AsyncWrite`
/// to the `futures::io::AsyncWrite` that tiberius expects.
/// tiberius is runtime-agnostic; this shim bridges the two ecosystems.
type TcpClient = tiberius::Client<tokio_util::compat::Compat<TcpStream>>;

/// SQL Server backup implementation.
///
/// Each `execute_backup` call creates a fresh connection — no pooling needed
/// since backups are infrequent, long-running operations (minutes, not milliseconds).
pub struct MssqlBackup {
    config: SqlServerConfig,
}

impl MssqlBackup {
    /// Create a new instance from SQL Server config.
    pub fn new(config: SqlServerConfig) -> Self {
        Self { config }
    }

    /// Establish a TDS connection to SQL Server.
    ///
    /// Connection flow:
    /// 1. Build tiberius `Config` with auth settings
    /// 2. Open a raw TCP socket
    /// 3. Wrap with `.compat_write()` for tiberius compatibility
    /// 4. TDS protocol handshake
    async fn connect(&self) -> Result<TcpClient, BackupError> {
        let mut tib_config = tiberius::Config::new();
        tib_config.host(&self.config.host);
        tib_config.port(self.config.port);

        // Map domain AuthMethod → tiberius AuthMethod.
        // This is where the Adapter pattern shines: the mapping between
        // our domain types and the library types happens HERE, not in domain.
        match &self.config.auth_method {
            AuthMethod::Sql => {
                let username = self.config.username.as_deref().ok_or_else(|| {
                    BackupError::DatabaseConnection(
                        "SQL authentication requires a username".into(),
                    )
                })?;
                let password = self.config.password.as_deref().ok_or_else(|| {
                    BackupError::DatabaseConnection(
                        "SQL authentication requires a password".into(),
                    )
                })?;
                tib_config
                    .authentication(tiberius::AuthMethod::sql_server(username, password));
            }
            AuthMethod::Windows => {
                // Uses NTLM on Windows, requires GSSAPI feature on Linux/macOS.
                #[cfg(any(windows, feature = "integrated-auth-gssapi"))]
                {
                    tib_config.authentication(tiberius::AuthMethod::Integrated);
                }
                #[cfg(not(any(windows, feature = "integrated-auth-gssapi")))]
                {
                    return Err(BackupError::DatabaseConnection(
                        "Windows authentication is only supported on Windows or when the integrated-auth-gssapi feature is enabled".into(),
                    ));
                }
            }
        }

        // Accept any TLS certificate (standard for on-premises SQL Server).
        tib_config.trust_cert();

        // TCP connection
        let addr = format!("{}:{}", self.config.host, self.config.port);
        tracing::debug!(addr = %addr, "Connecting to SQL Server");

        let tcp = TcpStream::connect(&addr).await.map_err(|e| {
            BackupError::DatabaseConnection(format!("Cannot reach SQL Server at {addr}: {e}"))
        })?;
        tcp.set_nodelay(true)?;

        // TDS handshake — `.compat_write()` bridges tokio ↔ futures I/O
        let client = tiberius::Client::connect(tib_config, tcp.compat_write())
            .await
            .map_err(|e| {
                BackupError::DatabaseConnection(format!(
                    "TDS handshake failed with {addr}: {e}"
                ))
            })?;

        tracing::debug!(addr = %addr, "Connected to SQL Server");
        Ok(client)
    }

    /// Retrieve the list of non-system online databases.
    pub async fn list_databases(&self) -> Result<Vec<String>, BackupError> {
        let mut client = self.connect().await?;
        let query = "SELECT name FROM sys.databases WHERE name NOT IN ('master', 'tempdb', 'model', 'msdb') AND state = 0 ORDER BY name ASC";
        let stream = client.query(query, &[]).await.map_err(|e| {
            BackupError::DatabaseConnection(format!("Database query failed: {e}"))
        })?;
        
        let rows = stream.into_first_result().await.map_err(|e| {
            BackupError::DatabaseConnection(format!("Database response parsing failed: {e}"))
        })?;
        
        let mut databases = Vec::new();
        for row in rows {
            if let Some(name) = row.get::<&str, _>(0) {
                databases.push(name.to_string());
            }
        }
        Ok(databases)
    }

    /// Build the T-SQL `BACKUP DATABASE` command.
    ///
    /// - `[database]` — brackets handle names with spaces or reserved words
    /// - `N'path'` — N prefix enables Unicode paths
    /// - `FORMAT, INIT` — overwrite any existing backup set in the file
    /// - `COMPRESSION` — native SQL Server compression (60-80% size reduction)
    pub fn build_backup_sql(database: &str, backup_path: &Path) -> String {
        format!(
            "BACKUP DATABASE [{database}] \
             TO DISK = N'{path}' \
             WITH FORMAT, INIT, COMPRESSION, \
             NAME = N'{database} Full Backup'",
            database = database,
            path = backup_path.display(),
        )
    }
}

impl DatabaseBackup for MssqlBackup {
    async fn execute_backup(
        &self,
        database: &str,
        backup_dir: &Path,
    ) -> Result<BackupInfo, BackupError> {
        // Ensure backup directory exists
        std::fs::create_dir_all(backup_dir)?;

        // Generate unique filename with timestamp
        // e.g. "OrdersDB_20260612_020000.bak"
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = format!("{database}_{timestamp}.bak");
        let backup_path = backup_dir.join(&filename);

        tracing::info!(
            database = database,
            path = %backup_path.display(),
            "Starting BACKUP DATABASE"
        );

        // Connect and execute the backup command
        let mut client = self.connect().await?;
        let sql = Self::build_backup_sql(database, &backup_path);

        client.execute(sql.as_str(), &[]).await.map_err(|e| {
            BackupError::BackupExecution {
                database: database.to_string(),
                reason: e.to_string(),
            }
        })?;

        tracing::info!(database = database, "BACKUP DATABASE completed");

        // Get file size (assumes agent runs on the SQL Server machine).
        // If the file is on a remote server, this returns 0 gracefully.
        let size_bytes = std::fs::metadata(&backup_path)
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(BackupInfo {
            database_name: database.to_string(),
            backup_path,
            size_bytes,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_includes_required_clauses() {
        let sql = MssqlBackup::build_backup_sql(
            "OrdersDB",
            Path::new(r"C:\Backups\OrdersDB_20260612.bak"),
        );

        assert!(sql.contains("[OrdersDB]"), "Name should be bracketed");
        assert!(sql.contains("COMPRESSION"), "Must use COMPRESSION");
        assert!(sql.contains("FORMAT"), "Must use FORMAT");
        assert!(sql.contains("INIT"), "Must use INIT");
        assert!(
            sql.contains(r"C:\Backups\OrdersDB_20260612.bak"),
            "Path should be in the SQL"
        );
    }

    #[test]
    fn sql_handles_name_with_spaces() {
        let sql = MssqlBackup::build_backup_sql(
            "My Database",
            Path::new(r"C:\Backups\test.bak"),
        );
        assert!(
            sql.contains("[My Database]"),
            "Brackets should protect names with spaces"
        );
    }

    #[test]
    fn constructor_stores_config() {
        let config = SqlServerConfig {
            host: "sql-prod".to_string(),
            port: 2433,
            auth_method: AuthMethod::Sql,
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
        };

        let backup = MssqlBackup::new(config);
        assert_eq!(backup.config.host, "sql-prod");
        assert_eq!(backup.config.port, 2433);
    }

    /// Verify our trait compiles and can be used generically.
    /// This is a compile-time test — if it compiles, it passes.
    #[allow(dead_code)]
    async fn accepts_any_database_backup(db: &impl DatabaseBackup) {
        let _result = db
            .execute_backup("TestDB", Path::new("/tmp"))
            .await;
    }
}
