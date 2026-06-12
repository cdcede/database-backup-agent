//! File compression port — the contract for compressing backup files.
//!
//! Exposes a clean interface for compressing files (typically SQL Server `.bak` files)
//! into ZIP format, reducing storage size and network transfer times.

use std::path::{Path, PathBuf};

use crate::domain::error::BackupError;

/// Port trait for compressing files.
///
/// Any compression adapter (Zip, TarGz, 7z) must satisfy this trait.
/// By returning `impl std::future::Future`, we support native async/await
/// without external crates like `async-trait`.
pub trait FileCompressor: Send + Sync {
    /// Compress a single file at `source` into a ZIP archive at `destination`.
    ///
    /// - `source`: The file to compress (e.g., `C:\Backups\DB_20260612.bak`).
    /// - `destination`: The path to the ZIP file to create (e.g., `C:\Backups\DB_20260612.zip`).
    ///
    /// Returns the path to the created archive and its size in bytes if successful.
    fn compress_file(
        &self,
        source: &Path,
        destination: &Path,
    ) -> impl std::future::Future<Output = Result<(PathBuf, u64), BackupError>> + Send;
}
