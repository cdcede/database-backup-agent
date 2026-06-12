//! Retention cleanup service implementation.
//!
//! Scans backup directories and removes backup files that exceed the retention
//! period configured by the user. Employs strict filename pattern matching to
//! prevent accidental deletion of unrelated files.

use std::path::Path;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use tokio::fs;

use crate::domain::error::BackupError;

/// Parse the timestamp from a standard backup filename.
///
/// Pattern expected: `<database_name>_YYYYMMDD_HHMMSS.<ext>`
///
/// Returns the parsed UTC DateTime if successful, or `None` if the filename
/// does not match the expected structure.
pub fn parse_backup_timestamp(filename: &str, db_name: &str) -> Option<DateTime<Utc>> {
    // 1. Safety check: Filename must start with the database name followed by an underscore
    if !filename.starts_with(&format!("{}_", db_name)) {
        return None;
    }

    // 2. Remove file extension to isolate the base name
    let path = Path::new(filename);
    let stem = path.file_stem()?.to_str()?;

    // 3. Extract the date/time suffix
    // Splitting by '_' handles database names containing underscores (e.g. "My_Prod_DB")
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() < 3 {
        return None;
    }

    let time_str = parts[parts.len() - 1]; // "HHMMSS"
    let date_str = parts[parts.len() - 2]; // "YYYYMMDD"

    // 4. Parse the concatenated date and time parts using chrono
    let datetime_str = format!("{}{}", date_str, time_str);
    let naive = NaiveDateTime::parse_from_str(&datetime_str, "%Y%m%d%H%M%S").ok()?;
    
    Some(Utc.from_utc_datetime(&naive))
}

/// Scan a directory and delete backup files older than `retention_days` for a database.
///
/// - `directory`: The folder containing the backup files.
/// - `database_name`: The database to filter files for.
/// - `retention_days`: Expire backups older than this number of days (0 disables cleanup).
///
/// Returns the number of files deleted if successful.
pub async fn clean_old_backups(
    directory: &Path,
    database_name: &str,
    retention_days: u32,
) -> Result<u32, BackupError> {
    if retention_days == 0 {
        tracing::info!(
            database = database_name,
            "Retention days set to 0; retention cleanup skipped"
        );
        return Ok(0);
    }

    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
    let mut deleted_count = 0;

    tracing::info!(
        database = database_name,
        directory = %directory.display(),
        retention_days = retention_days,
        cutoff = %cutoff.format("%Y-%m-%d %H:%M:%S UTC"),
        "Starting retention cleanup scan"
    );

    // Read the directory contents
    let mut entries = fs::read_dir(directory).await.map_err(|e| {
        BackupError::Io(e)
    })?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Parse and verify timestamp matches our database prefix
                if let Some(timestamp) = parse_backup_timestamp(filename, database_name) {
                    if timestamp < cutoff {
                        tracing::info!(
                            file = %filename,
                            timestamp = %timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                            "Deleting expired backup file"
                        );
                        
                        fs::remove_file(&path).await.map_err(|e| {
                            BackupError::Io(e)
                        })?;
                        
                        deleted_count += 1;
                    }
                }
            }
        }
    }

    if deleted_count > 0 {
        tracing::info!(
            database = database_name,
            deleted_count = deleted_count,
            "Retention cleanup completed"
        );
    } else {
        tracing::debug!(
            database = database_name,
            "No expired backup files found to delete"
        );
    }

    Ok(deleted_count)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn parse_timestamp_handles_simple_name() {
        let ts = parse_backup_timestamp("ProdDB_20260612_143000.zip", "ProdDB");
        assert!(ts.is_some());
        let dt = ts.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 6);
        assert_eq!(dt.day(), 12);
        assert_eq!(dt.hour(), 14);
        assert_eq!(dt.minute(), 30);
    }

    #[test]
    fn parse_timestamp_handles_underscores_in_db_name() {
        let ts = parse_backup_timestamp("My_Prod_DB_20260612_143000.bak", "My_Prod_DB");
        assert!(ts.is_some());
        assert_eq!(ts.unwrap().year(), 2026);
    }

    #[test]
    fn parse_timestamp_ignores_wrong_database() {
        let ts = parse_backup_timestamp("OtherDB_20260612_143000.zip", "ProdDB");
        assert!(ts.is_none());
    }

    #[test]
    fn parse_timestamp_ignores_invalid_structure() {
        let ts1 = parse_backup_timestamp("ProdDB_20260612.zip", "ProdDB");
        let ts2 = parse_backup_timestamp("ProdDB_20260612_143000_extra.zip", "ProdDB");
        let ts3 = parse_backup_timestamp("ProdDB_notdate_time.zip", "ProdDB");
        
        assert!(ts1.is_none());
        assert!(ts2.is_none());
        assert!(ts3.is_none());
    }

    #[tokio::test]
    async fn retention_cleanup_deletes_expired_files_only() {
        let temp_dir = std::env::temp_dir().join(format!("retention_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).await.unwrap();

        // 1. Create mock files with varying ages
        // Cutoff for 2 days ago:
        // - Expired (3 days ago): 20260609_120000
        // - Recent (1 day ago): 20260611_120000
        // - Expired but different database name (3 days ago)
        // - Invalid name pattern
        
        let now = Utc::now();
        let three_days_ago = now - chrono::Duration::days(3);
        let one_day_ago = now - chrono::Duration::days(1);

        let expired_name = format!("ProdDB_{}.zip", three_days_ago.format("%Y%m%d_%H%M%S"));
        let recent_name = format!("ProdDB_{}.zip", one_day_ago.format("%Y%m%d_%H%M%S"));
        let other_db_name = format!("OtherDB_{}.zip", three_days_ago.format("%Y%m%d_%H%M%S"));
        let invalid_name = "ProdDB_some_random_text.zip".to_string();

        let expired_path = temp_dir.join(&expired_name);
        let recent_path = temp_dir.join(&recent_name);
        let other_db_path = temp_dir.join(&other_db_name);
        let invalid_path = temp_dir.join(&invalid_name);

        fs::write(&expired_path, b"expired").await.unwrap();
        fs::write(&recent_path, b"recent").await.unwrap();
        fs::write(&other_db_path, b"other").await.unwrap();
        fs::write(&invalid_path, b"invalid").await.unwrap();

        // 2. Execute cleanup with retention = 2 days
        let deleted = clean_old_backups(&temp_dir, "ProdDB", 2).await.unwrap();
        
        // 3. Verify assertions
        assert_eq!(deleted, 1, "Should have deleted exactly 1 file");
        assert!(!expired_path.exists(), "Expired file should be deleted");
        assert!(recent_path.exists(), "Recent file should be preserved");
        assert!(other_db_path.exists(), "Other database's file should be preserved");
        assert!(invalid_path.exists(), "Non-matching filename pattern should be preserved");

        // Clean up temp dir
        let _ = fs::remove_dir_all(&temp_dir).await;
    }
}
