//! Google Drive storage adapter implementation.
//!
//! Uploads backups with the Drive v3 *resumable* protocol: the `.zip` archive
//! is sent in fixed-size chunks (never fully loaded in memory) and a dropped
//! connection only retries from the last byte Google acknowledged.
//!
//! Built on `reqwest` only (no Google SDK) so it stays compatible with the
//! Rust 1.77.2 toolchain pinned for Windows Server 2012 R2.

use std::io::SeekFrom;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::domain::config::GoogleDriveConfig;
use crate::domain::error::BackupError;
use crate::ports::storage::Storage;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";

/// 8 MiB. Drive requires every non-final chunk to be a multiple of 256 KiB.
const DEFAULT_CHUNK_SIZE: usize = 32 * 256 * 1024;

/// Consecutive failed attempts (without upload progress) before giving up.
const MAX_ATTEMPTS: u32 = 3;

/// Upper bound for any single HTTP request (an 8 MiB chunk needs ~4.5 min at 256 kbit/s).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Google Drive storage adapter (OAuth2 refresh-token credentials).
pub struct GoogleDriveStorage {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    folder_id: Option<String>,
    token_url: String,
    upload_url: String,
    chunk_size: usize,
    retry_delay: Duration,
    http: reqwest::Client,
}

/// What the server reported after receiving (part of) the upload.
enum UploadProgress {
    /// More bytes are expected; the value is the next offset to send.
    Incomplete(u64),
    /// Upload finished; the value is the Drive file ID.
    Done(String),
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
}

impl GoogleDriveStorage {
    /// Create a new `GoogleDriveStorage` from the `[storage.google_drive]` settings.
    pub fn new(config: &GoogleDriveConfig) -> Self {
        let http = build_http_client(REQUEST_TIMEOUT);

        Self {
            client_id: config.client_id.trim().to_string(),
            client_secret: config.client_secret.trim().to_string(),
            refresh_token: config.refresh_token.trim().to_string(),
            folder_id: config
                .folder_id
                .as_deref()
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(str::to_string),
            token_url: TOKEN_URL.to_string(),
            upload_url: UPLOAD_URL.to_string(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            retry_delay: Duration::from_secs(2),
            http,
        }
    }

    /// Exchange the long-lived refresh token for a short-lived access token.
    async fn access_token(&self) -> Result<String, BackupError> {
        let response = self
            .http
            .post(&self.token_url)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| upload_error(format!("Google OAuth token request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(upload_error(format!(
                "Google OAuth token request rejected ({}): {}",
                status, body
            )));
        }

        let token: TokenResponse = response
            .json()
            .await
            .map_err(|e| upload_error(format!("Invalid Google OAuth token response: {}", e)))?;
        Ok(token.access_token)
    }

    /// Open a resumable upload session and return its session URI.
    async fn start_session(
        &self,
        access_token: &str,
        filename: &str,
        total: u64,
    ) -> Result<String, BackupError> {
        let mut metadata = serde_json::json!({ "name": filename });
        if let Some(folder) = &self.folder_id {
            metadata["parents"] = serde_json::json!([folder]);
        }

        let response = self
            .http
            .post(format!(
                "{}?uploadType=resumable&supportsAllDrives=true",
                self.upload_url
            ))
            .bearer_auth(access_token)
            .header("X-Upload-Content-Type", "application/zip")
            .header("X-Upload-Content-Length", total.to_string())
            .json(&metadata)
            .send()
            .await
            .map_err(|e| upload_error(format!("Failed to start Google Drive upload: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(upload_error(format!(
                "Google Drive rejected the upload session ({}): {}",
                status, body
            )));
        }

        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| upload_error("Google Drive did not return an upload session URI".into()))
    }

    /// Send one chunk (or an empty status query when `body` is empty).
    async fn put_chunk(
        &self,
        session_uri: &str,
        body: Vec<u8>,
        content_range: String,
    ) -> Result<UploadProgress, BackupError> {
        let response = self
            .http
            .put(session_uri)
            .header(reqwest::header::CONTENT_RANGE, content_range)
            .body(body)
            .send()
            .await
            .map_err(|e| upload_error(format!("Google Drive chunk upload failed: {}", e)))?;

        let status = response.status();
        if status.as_u16() == 308 {
            let next_offset = response
                .headers()
                .get(reqwest::header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_range_end)
                .map(|end| end + 1)
                .unwrap_or(0);
            return Ok(UploadProgress::Incomplete(next_offset));
        }
        if status.is_success() {
            let file: DriveFile = response
                .json()
                .await
                .map_err(|e| upload_error(format!("Invalid Google Drive upload response: {}", e)))?;
            return Ok(UploadProgress::Done(file.id));
        }

        let body = response.text().await.unwrap_or_default();
        Err(upload_error(format!(
            "Google Drive chunk upload rejected ({}): {}",
            status, body
        )))
    }
}

impl Storage for GoogleDriveStorage {
    async fn upload(&self, source: &Path, filename: &str) -> Result<String, BackupError> {
        let mut file = tokio::fs::File::open(source).await.map_err(|e| {
            upload_error(format!(
                "Failed to open '{}' for Google Drive upload: {}",
                source.display(),
                e
            ))
        })?;
        let total = file
            .metadata()
            .await
            .map_err(|e| upload_error(format!("Failed to read '{}' size: {}", source.display(), e)))?
            .len();

        let access_token = self.access_token().await?;
        let session_uri = self.start_session(&access_token, filename, total).await?;

        tracing::info!(filename = filename, size_bytes = total, "Uploading file to Google Drive");

        let mut buffer = vec![0u8; self.chunk_size];
        let mut offset: u64 = 0;
        let mut failures: u32 = 0;

        loop {
            let len = (total - offset).min(self.chunk_size as u64) as usize;
            file.seek(SeekFrom::Start(offset))
                .await
                .and(file.read_exact(&mut buffer[..len]).await.map(|_| ()))
                .map_err(|e| upload_error(format!("Failed to read '{}': {}", source.display(), e)))?;

            let progress = match self
                .put_chunk(&session_uri, buffer[..len].to_vec(), content_range(offset, len, total))
                .await
            {
                Ok(progress) => progress,
                Err(e) => {
                    failures += 1;
                    if failures >= MAX_ATTEMPTS {
                        return Err(e);
                    }
                    tracing::warn!(error = %e, attempt = failures, "Retrying Google Drive chunk");
                    tokio::time::sleep(self.retry_delay * failures).await;
                    // Ask Google how many bytes it really kept before resending.
                    match self
                        .put_chunk(&session_uri, Vec::new(), format!("bytes */{}", total))
                        .await
                    {
                        Ok(UploadProgress::Done(file_id)) => return Ok(drive_file_url(&file_id)),
                        Ok(UploadProgress::Incomplete(next)) => offset = next.min(total),
                        Err(_) => {} // Resend from the current offset.
                    }
                    // Already counted as a failure above; do not count it twice.
                    continue;
                }
            };

            match progress {
                UploadProgress::Done(file_id) => {
                    tracing::info!(file_id = %file_id, "Google Drive upload completed");
                    return Ok(drive_file_url(&file_id));
                }
                UploadProgress::Incomplete(next) if next > offset => {
                    failures = 0;
                    offset = next.min(total);
                }
                UploadProgress::Incomplete(next) => {
                    // Every answer without progress counts as a failure, so a
                    // misbehaving server cannot keep us looping forever.
                    failures += 1;
                    if failures >= MAX_ATTEMPTS {
                        return Err(upload_error(format!(
                            "Google Drive upload stalled at byte {} of {}",
                            next, total
                        )));
                    }
                    tokio::time::sleep(self.retry_delay * failures).await;
                    offset = next.min(total);
                }
            }
        }
    }
}

fn build_http_client(request_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        // Drive answers intermediate chunks with "308 Resume Incomplete",
        // which must NOT be treated as an HTTP redirect.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(30))
        // A hung connection becomes a retryable error instead of blocking the job forever.
        .timeout(request_timeout)
        .build()
        .unwrap_or_default()
}

fn drive_file_url(file_id: &str) -> String {
    format!("https://drive.google.com/file/d/{}/view", file_id)
}

/// Build the `Content-Range` header for a chunk.
fn content_range(offset: u64, len: usize, total: u64) -> String {
    if len == 0 {
        format!("bytes */{}", total)
    } else {
        format!("bytes {}-{}/{}", offset, offset + len as u64 - 1, total)
    }
}

/// Parse the last acknowledged byte from a `Range: bytes=0-N` header.
fn parse_range_end(range: &str) -> Option<u64> {
    range.trim().strip_prefix("bytes=")?.split_once('-')?.1.trim().parse().ok()
}

fn upload_error(message: String) -> BackupError {
    BackupError::StorageUpload(message)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    #[test]
    fn content_range_formats_chunks_and_status_queries() {
        assert_eq!(content_range(0, 4, 10), "bytes 0-3/10");
        assert_eq!(content_range(8, 2, 10), "bytes 8-9/10");
        assert_eq!(content_range(0, 0, 0), "bytes */0");
    }

    #[test]
    fn parse_range_end_reads_last_byte() {
        assert_eq!(parse_range_end("bytes=0-262143"), Some(262_143));
        assert_eq!(parse_range_end("garbage"), None);
    }

    #[test]
    fn default_chunk_size_is_multiple_of_256_kib() {
        assert_eq!(DEFAULT_CHUNK_SIZE % (256 * 1024), 0);
    }

    // -------------------------------------------------------------------------
    // Fake Google endpoint (token + resumable upload) over plain HTTP.
    // -------------------------------------------------------------------------

    #[derive(Default)]
    struct FakeDrive {
        received: Vec<u8>,
        metadata: String,
        chunk_puts: usize,
        /// Reject the chunk starting at this offset once with a 503.
        fail_once_at: Option<u64>,
        /// Answer every chunk with "308" without keeping any bytes.
        stall: bool,
        /// Never answer chunk requests (simulates a hung connection).
        hang: bool,
    }

    async fn spawn_fake_drive(state: Arc<Mutex<FakeDrive>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let session_uri = format!("{}/session", base);

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let state = state.clone();
                let session_uri = session_uri.clone();
                tokio::spawn(async move {
                    let (head, body) = read_request(&mut socket).await;
                    match handle(&head, body, &state, &session_uri) {
                        Some(response) => {
                            let _ = socket.write_all(response.as_bytes()).await;
                        }
                        // Keep the socket open without answering.
                        None => tokio::time::sleep(Duration::from_secs(3600)).await,
                    }
                });
            }
        });
        base
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let head_end = loop {
            let n = socket.read(&mut buf).await.unwrap();
            data.extend_from_slice(&buf[..n]);
            if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let head = String::from_utf8_lossy(&data[..head_end]).to_lowercase();
        let length = header(&head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        while data.len() < head_end + length {
            let n = socket.read(&mut buf).await.unwrap();
            data.extend_from_slice(&buf[..n]);
        }
        (head, data[head_end..head_end + length].to_vec())
    }

    fn header(head: &str, name: &str) -> Option<String> {
        head.lines()
            .find_map(|l| l.strip_prefix(&format!("{}:", name)))
            .map(|v| v.trim().to_string())
    }

    fn reply(status: &str, extra_headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            extra_headers,
            body.len(),
            body
        )
    }

    fn handle(head: &str, body: Vec<u8>, state: &Mutex<FakeDrive>, session_uri: &str) -> Option<String> {
        let mut s = state.lock().unwrap();
        if head.starts_with("post /token") {
            return Some(reply("200 OK", "Content-Type: application/json\r\n", r#"{"access_token":"tok"}"#));
        }
        if head.starts_with("post /upload?uploadtype=resumable") {
            assert_eq!(header(head, "authorization").as_deref(), Some("bearer tok"));
            s.metadata = String::from_utf8_lossy(&body).to_string();
            return Some(reply("200 OK", &format!("Location: {}\r\n", session_uri), ""));
        }
        // PUT /session
        if s.hang {
            return None;
        }
        if s.stall {
            s.chunk_puts += 1;
            return Some(reply("308 Resume Incomplete", "", ""));
        }
        let range = header(head, "content-range").unwrap();
        let (span, total) = range.strip_prefix("bytes ").unwrap().split_once('/').unwrap();
        let total: u64 = total.parse().unwrap();
        if span != "*" {
            s.chunk_puts += 1;
            let start: u64 = span.split_once('-').unwrap().0.parse().unwrap();
            if s.fail_once_at == Some(start) {
                s.fail_once_at = None;
                return Some(reply("503 Service Unavailable", "", "busy"));
            }
            assert_eq!(start, s.received.len() as u64, "chunk must continue at the acked offset");
            s.received.extend_from_slice(&body);
        }
        Some(if s.received.len() as u64 == total {
            reply("200 OK", "Content-Type: application/json\r\n", r#"{"id":"file123"}"#)
        } else if s.received.is_empty() {
            reply("308 Resume Incomplete", "", "")
        } else {
            reply("308 Resume Incomplete", &format!("Range: bytes=0-{}\r\n", s.received.len() - 1), "")
        })
    }

    async fn run_upload(state: Arc<Mutex<FakeDrive>>, content: &[u8]) -> Result<String, BackupError> {
        let base = spawn_fake_drive(state).await;
        let source = std::env::temp_dir().join(format!("gdrive_test_{}.zip", uuid::Uuid::new_v4()));
        tokio::fs::write(&source, content).await.unwrap();

        let mut storage = GoogleDriveStorage::new(&GoogleDriveConfig {
            enabled: true,
            client_id: "cid".into(),
            client_secret: "csecret".into(),
            refresh_token: "rtoken".into(),
            folder_id: Some("folderXYZ".into()),
        });
        storage.token_url = format!("{}/token", base);
        storage.upload_url = format!("{}/upload", base);
        storage.chunk_size = 4;
        storage.retry_delay = Duration::ZERO;
        storage.http = build_http_client(Duration::from_millis(300));

        // Guard so a regression fails the test instead of hanging the suite.
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            storage.upload(&source, "ProdDB_20261025.zip"),
        )
        .await
        .expect("upload must terminate");
        let _ = tokio::fs::remove_file(&source).await;
        result
    }

    #[tokio::test]
    async fn uploads_file_in_chunks_into_configured_folder() {
        let state = Arc::new(Mutex::new(FakeDrive::default()));
        let result = run_upload(state.clone(), b"0123456789").await;

        assert_eq!(result.unwrap(), "https://drive.google.com/file/d/file123/view");
        let s = state.lock().unwrap();
        assert_eq!(s.received, b"0123456789");
        assert_eq!(s.chunk_puts, 3, "10 bytes with 4-byte chunks = 3 PUTs");
        assert!(s.metadata.contains("ProdDB_20261025.zip"), "{}", s.metadata);
        assert!(s.metadata.contains("folderXYZ"), "{}", s.metadata);
    }

    #[tokio::test]
    async fn resumes_from_acknowledged_offset_after_server_error() {
        let state = Arc::new(Mutex::new(FakeDrive {
            fail_once_at: Some(4),
            ..Default::default()
        }));
        let result = run_upload(state.clone(), b"0123456789").await;

        assert!(result.is_ok(), "{result:?}");
        let s = state.lock().unwrap();
        assert_eq!(s.received, b"0123456789");
        assert_eq!(s.chunk_puts, 4, "the rejected chunk is sent again");
    }

    #[tokio::test]
    async fn gives_up_when_server_never_makes_progress() {
        let state = Arc::new(Mutex::new(FakeDrive {
            stall: true,
            ..Default::default()
        }));
        let err = run_upload(state.clone(), b"0123456789").await.unwrap_err().to_string();

        assert!(err.contains("stalled"), "{err}");
        assert_eq!(state.lock().unwrap().chunk_puts as u32, MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn gives_up_when_server_stops_answering() {
        let state = Arc::new(Mutex::new(FakeDrive {
            hang: true,
            ..Default::default()
        }));
        let err = run_upload(state, b"0123456789").await.unwrap_err().to_string();

        assert!(err.contains("chunk upload failed"), "{err}");
    }
}
