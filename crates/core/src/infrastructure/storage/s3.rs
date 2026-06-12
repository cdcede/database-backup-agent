//! S3 storage adapter implementation.
//!
//! Enabled via the `s3` cargo feature flag. Uses the official AWS SDK for Rust.

#![cfg(feature = "s3")]

use std::path::Path;

use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use crate::domain::error::BackupError;
use crate::ports::storage::Storage;

/// AWS S3 storage adapter.
pub struct S3Storage {
    bucket: String,
    prefix: Option<String>,
}

impl S3Storage {
    /// Create a new `S3Storage` instance.
    ///
    /// - `bucket`: The S3 bucket name.
    /// - `prefix`: Optional folder prefix (e.g. `backups/sql-server`).
    pub fn new(bucket: String, prefix: Option<String>) -> Self {
        Self { bucket, prefix }
    }
}

impl Storage for S3Storage {
    async fn upload(&self, source: &Path, filename: &str) -> Result<String, BackupError> {
        // ---------------------------------------------------------------------
        // Async Rule: AWS SDK is natively async
        // ---------------------------------------------------------------------
        // The AWS SDK for Rust uses `tokio` under the hood. All network operations
        // and streaming uploads are fully async.
        
        // 1. Load AWS configuration from environment variables / IAM roles
        let config = aws_config::load_from_env().await;
        let client = Client::new(&config);

        // 2. Compute the S3 object key (path inside the bucket)
        let key = match &self.prefix {
            Some(prefix) => {
                let clean_prefix = prefix.trim().trim_matches('/');
                if clean_prefix.is_empty() {
                    filename.to_string()
                } else {
                    format!("{}/{}", clean_prefix, filename)
                }
            }
            None => filename.to_string(),
        };

        // 3. Open file as a ByteStream asynchronously
        let body = ByteStream::from_path(source).await.map_err(|e| {
            BackupError::StorageUpload(format!(
                "Failed to read source file '{}' for S3 upload: {}",
                source.display(),
                e
            ))
        })?;

        tracing::info!(
            bucket = %self.bucket,
            key = %key,
            "Uploading file to S3"
        );

        // 4. Send PUT request to S3
        client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(body)
            .send()
            .await
            .map_err(|e| {
                BackupError::StorageUpload(format!(
                    "Failed to upload object to S3 bucket '{}' with key '{}': {}",
                    self.bucket,
                    key,
                    e
                ))
            })?;

        tracing::info!(
            bucket = %self.bucket,
            key = %key,
            "S3 upload completed"
        );

        // Return the S3 URI as the unique identifier
        Ok(format!("s3://{}/{}", self.bucket, key))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Basic constructor test to verify compilation.
    #[test]
    fn s3_constructor_stores_parameters() {
        let storage = S3Storage::new("my-backups".to_string(), Some("sql/".to_string()));
        assert_eq!(storage.bucket, "my-backups");
        assert_eq!(storage.prefix, Some("sql/".to_string()));
    }
}
