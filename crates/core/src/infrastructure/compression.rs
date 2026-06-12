//! Zip compression adapter implementation.
//!
//! Satisfies the `FileCompressor` port. Uses the `zip` crate to create
//! deflate-compressed ZIP archives. Offloads CPU-bound compression to
//! Tokio's blocking thread pool.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;
use zip::CompressionMethod;

use crate::domain::error::BackupError;
use crate::ports::compression::FileCompressor;

/// ZIP compression adapter.
pub struct ZipCompressor;

impl ZipCompressor {
    /// Create a new `ZipCompressor` instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for ZipCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl FileCompressor for ZipCompressor {
    async fn compress_file(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(PathBuf, u64), BackupError> {
        let source_path = source.to_path_buf();
        let dest_path = destination.to_path_buf();

        // ---------------------------------------------------------------------
        // Async Rule: Offload CPU-Bound / Sync I/O tasks
        // ---------------------------------------------------------------------
        // Zip compression is CPU-intensive (deflate) and uses synchronous
        // std::fs I/O operations. Running this directly in an async task would
        // block the Tokio executor threads.
        // `spawn_blocking` runs the closure in a separate thread pool managed by
        // Tokio designed specifically for blocking calls.
        let (dest_path, size_bytes) = tokio::task::spawn_blocking(move || -> Result<(PathBuf, u64), BackupError> {
            let src_file = File::open(&source_path).map_err(|e| {
                BackupError::Compression(format!(
                    "Failed to open source file '{}': {}",
                    source_path.display(),
                    e
                ))
            })?;
            let mut reader = BufReader::new(src_file);

            // Ensure the parent directory for the destination exists
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    BackupError::Compression(format!(
                        "Failed to create destination directory '{}': {}",
                        parent.display(),
                        e
                    ))
                })?;
            }

            let dest_file = File::create(&dest_path).map_err(|e| {
                BackupError::Compression(format!(
                    "Failed to create destination zip file '{}': {}",
                    dest_path.display(),
                    e
                ))
            })?;
            let writer = BufWriter::new(dest_file);
            let mut zip = ZipWriter::new(writer);

            // Get the plain filename to store inside the ZIP
            let entry_name = source_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    BackupError::Compression(format!(
                        "Invalid source path filename: {}",
                        source_path.display()
                    ))
                })?;

            // Deflate compression options
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated);

            zip.start_file(entry_name, options).map_err(|e| {
                BackupError::Compression(format!("Failed to start zip file entry: {}", e))
            })?;

            // Stream file contents into ZIP entry
            std::io::copy(&mut reader, &mut zip).map_err(|e| {
                BackupError::Compression(format!("Failed to write data to zip: {}", e))
            })?;

            // Finalize ZIP directory structure
            zip.finish().map_err(|e| {
                BackupError::Compression(format!("Failed to finalize zip archive: {}", e))
            })?;

            // Fetch the compressed file size
            let metadata = std::fs::metadata(&dest_path).map_err(|e| {
                BackupError::Compression(format!(
                    "Failed to read destination metadata: {}",
                    e
                ))
            })?;

            Ok((dest_path, metadata.len()))
        })
        .await
        .map_err(|e| {
            BackupError::Compression(format!("Tokio thread join failure: {}", e))
        })??;

        Ok((dest_path, size_bytes))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    #[tokio::test]
    async fn zip_compression_success() {
        let temp_dir = std::env::temp_dir();
        let unique_id = uuid::Uuid::new_v4();
        
        let src_path = temp_dir.join(format!("test_src_{}.txt", unique_id));
        let dest_path = temp_dir.join(format!("test_dest_{}.zip", unique_id));

        // 1. Create a dummy file with mock database contents
        let test_content = b"SQL Server Backup Content Simulation. Deflate this!";
        std::fs::write(&src_path, test_content).unwrap();

        // 2. Perform compression
        let compressor = ZipCompressor::new();
        let (output_path, size) = compressor
            .compress_file(&src_path, &dest_path)
            .await
            .expect("Compression failed");

        assert_eq!(output_path, dest_path);
        assert!(size > 0, "ZIP file should have non-zero size");

        // 3. Read it back and verify it matches original
        let file = File::open(&dest_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        
        assert_eq!(archive.len(), 1, "ZIP should contain exactly one file");
        
        let mut entry = archive.by_index(0).unwrap();
        assert_eq!(
            entry.name(),
            src_path.file_name().unwrap().to_str().unwrap(),
            "ZIP entry name should match source file name"
        );

        let mut compressed_data = Vec::new();
        entry.read_to_end(&mut compressed_data).unwrap();
        assert_eq!(compressed_data, test_content, "Decompressed content mismatch");

        // 4. Clean up temporary files
        let _ = std::fs::remove_file(&src_path);
        let _ = std::fs::remove_file(&dest_path);
    }

    #[tokio::test]
    async fn non_existent_source_returns_error() {
        let temp_dir = std::env::temp_dir();
        let unique_id = uuid::Uuid::new_v4();

        let src_path = temp_dir.join(format!("does_not_exist_{}.txt", unique_id));
        let dest_path = temp_dir.join(format!("output_{}.zip", unique_id));

        let compressor = ZipCompressor::new();
        let result = compressor.compress_file(&src_path, &dest_path).await;

        assert!(result.is_err(), "Compression of non-existent file should fail");
        
        if let Err(BackupError::Compression(msg)) = result {
            assert!(msg.contains("Failed to open source file"));
        } else {
            panic!("Expected BackupError::Compression");
        }
    }
}
