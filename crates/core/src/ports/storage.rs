//! Storage port — the contract for saving backup files to storage providers.
//!
//! Provides a unified interface for storing files locally or uploading them to
//! cloud storage providers like AWS S3.

use std::path::Path;

use crate::domain::error::BackupError;

/// Port trait for backup storage providers.
///
/// Implementations define how files are uploaded or copied to a specific
/// destination (local disk, cloud bucket, etc.).
pub trait Storage: Send + Sync {
    /// Upload or copy a local file to the storage destination.
    ///
    /// - `source`: The path to the local file to upload (usually the `.zip` archive).
    /// - `filename`: The target filename or key in the destination storage (e.g. `DB_20260612.zip`).
    ///
    /// Returns a string identifier of the stored resource (e.g., local file path or S3 URI) if successful.
    fn upload(
        &self,
        source: &Path,
        filename: &str,
    ) -> impl std::future::Future<Output = Result<String, BackupError>> + Send;
}
