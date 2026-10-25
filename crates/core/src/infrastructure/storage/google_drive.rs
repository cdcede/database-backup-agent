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

use crate::application::retention::parse_backup_timestamp;
use crate::domain::config::GoogleDriveConfig;
use crate::domain::error::BackupError;
use crate::ports::storage::Storage;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";
const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";

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
    files_url: String,
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

/// A failed request before the upload starts, tagged with whether retrying can help.
struct Failure {
    error: BackupError,
    retryable: bool,
}

impl Failure {
    /// Network errors and unreadable responses: worth another attempt.
    fn transient(message: String) -> Self {
        Self { error: upload_error(message), retryable: true }
    }

    /// Only 5xx and 429 are retried; other 4xx (e.g. revoked credentials) are permanent.
    fn from_status(status: reqwest::StatusCode, message: String) -> Self {
        Self {
            error: upload_error(message),
            retryable: status.is_server_error() || status.as_u16() == 429,
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
}

#[derive(Deserialize)]
struct NamedDriveFile {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<NamedDriveFile>,
    next_page_token: Option<String>,
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
            files_url: FILES_URL.to_string(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            retry_delay: Duration::from_secs(2),
            http,
        }
    }

    /// Exchange the long-lived refresh token for a short-lived access token.
    async fn access_token(&self) -> Result<String, Failure> {
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
            .map_err(|e| {
                Failure::transient(format!("Google OAuth token request failed: {}", e.without_url()))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Failure::from_status(
                status,
                format!("Google OAuth token request rejected ({}): {}", status, body),
            ));
        }

        let token: TokenResponse = response.json().await.map_err(|e| {
            Failure::transient(format!("Invalid Google OAuth token response: {}", e.without_url()))
        })?;
        Ok(token.access_token)
    }

    /// Run a pre-upload request, retrying only transient failures with backoff.
    async fn with_retries<T, F, Fut>(&self, what: &str, mut request: F) -> Result<T, BackupError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, Failure>>,
    {
        let mut attempt: u32 = 1;
        loop {
            match request().await {
                Ok(value) => return Ok(value),
                Err(failure) if failure.retryable && attempt < MAX_ATTEMPTS => {
                    tracing::warn!(error = %failure.error, attempt, "Retrying Google Drive {}", what);
                    tokio::time::sleep(self.retry_delay * attempt).await;
                    attempt += 1;
                }
                Err(failure) => return Err(failure.error),
            }
        }
    }

    /// Open a resumable upload session and return its session URI.
    async fn start_session(
        &self,
        access_token: &str,
        filename: &str,
        total: u64,
    ) -> Result<String, Failure> {
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
            .map_err(|e| {
                Failure::transient(format!("Failed to start Google Drive upload: {}", e.without_url()))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Failure::from_status(
                status,
                format!("Google Drive rejected the upload session ({}): {}", status, body),
            ));
        }

        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| Failure::transient("Google Drive did not return an upload session URI".into()))
    }

    /// Permanently delete this database's backups older than `retention_days`
    /// from the configured folder (Drive root when no folder is set).
    ///
    /// Uses the same `<database>_YYYYMMDD_HHMMSS.zip` matching as the local
    /// retention, so manually uploaded or other databases' files are never touched.
    /// Files are deleted (not trashed) because trashed files still use quota.
    /// Returns the number of deleted files. `retention_days = 0` disables cleanup.
    pub async fn clean_old_backups(
        &self,
        database_name: &str,
        retention_days: u32,
    ) -> Result<u32, BackupError> {
        if retention_days == 0 {
            return Ok(0);
        }
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
        let access_token = self.with_retries("token request", || self.access_token()).await?;

        let parent = self.folder_id.as_deref().unwrap_or("root");
        let query = format!(
            "'{}' in parents and name contains '{}' and trashed = false",
            drive_query_literal(parent),
            drive_query_literal(&format!("{}_", database_name)),
        );

        // Collect first, delete after: deleting while paging can shift pages.
        let mut expired = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page = self
                .with_retries("file listing", || self.list_page(&access_token, &query, page_token.as_deref()))
                .await?;
            for file in page.files {
                if let Some(timestamp) = parse_backup_timestamp(&file.name, database_name) {
                    if timestamp < cutoff && file.name.ends_with(".zip") {
                        expired.push(file);
                    }
                }
            }
            match page.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }

        let mut deleted = 0;
        for file in &expired {
            tracing::info!(file = %file.name, "Deleting expired Google Drive backup");
            self.with_retries("file deletion", || self.delete_file(&access_token, &file.id))
                .await?;
            deleted += 1;
        }
        Ok(deleted)
    }

    async fn list_page(
        &self,
        access_token: &str,
        query: &str,
        page_token: Option<&str>,
    ) -> Result<FileList, Failure> {
        let mut params = vec![
            ("q", query),
            ("fields", "nextPageToken,files(id,name)"),
            ("pageSize", "1000"),
            ("supportsAllDrives", "true"),
            ("includeItemsFromAllDrives", "true"),
        ];
        if let Some(token) = page_token {
            params.push(("pageToken", token));
        }

        let response = self
            .http
            .get(&self.files_url)
            .bearer_auth(access_token)
            .query(&params)
            .send()
            .await
            .map_err(|e| Failure::transient(format!("Failed to list Google Drive files: {}", e.without_url())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Failure::from_status(
                status,
                format!("Google Drive file listing rejected ({}): {}", status, body),
            ));
        }
        response.json().await.map_err(|e| {
            Failure::transient(format!("Invalid Google Drive file listing: {}", e.without_url()))
        })
    }

    async fn delete_file(&self, access_token: &str, file_id: &str) -> Result<(), Failure> {
        let response = self
            .http
            .delete(format!("{}/{}?supportsAllDrives=true", self.files_url, file_id))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| Failure::transient(format!("Failed to delete Google Drive file: {}", e.without_url())))?;

        let status = response.status();
        // 404: already gone (e.g. deleted by hand) — nothing left to do.
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(Failure::from_status(
            status,
            format!("Google Drive file deletion rejected ({}): {}", status, body),
        ))
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
            // `without_url`: the session URI itself grants upload access, keep it out of logs.
            .map_err(|e| upload_error(format!("Google Drive chunk upload failed: {}", e.without_url())))?;

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
                .map_err(|e| {
                    upload_error(format!("Invalid Google Drive upload response: {}", e.without_url()))
                })?;
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

        let access_token = self.with_retries("token request", || self.access_token()).await?;
        let session_uri = self
            .with_retries("upload session", || self.start_session(&access_token, filename, total))
            .await?;

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

/// Escape a value for a single-quoted string literal in a Drive `q` query.
fn drive_query_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
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
        /// Answer the first token request with a 503.
        fail_token_once: bool,
        /// Answer the first session request with a 503.
        fail_session_once: bool,
        /// Answer every token request with 400 invalid_grant (revoked credentials).
        reject_token: bool,
        token_requests: usize,
        /// Files returned by the list endpoint as (id, name), split in two pages.
        listed_files: Vec<(String, String)>,
        /// `q` parameter of every list request (lowercased).
        list_queries: Vec<String>,
        deleted_ids: Vec<String>,
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
            s.token_requests += 1;
            if s.reject_token {
                return Some(reply("400 Bad Request", "", r#"{"error":"invalid_grant"}"#));
            }
            if s.fail_token_once {
                s.fail_token_once = false;
                return Some(reply("503 Service Unavailable", "", "busy"));
            }
            return Some(reply("200 OK", "Content-Type: application/json\r\n", r#"{"access_token":"tok"}"#));
        }
        if head.starts_with("post /upload?uploadtype=resumable") && s.fail_session_once {
            s.fail_session_once = false;
            return Some(reply("503 Service Unavailable", "", "busy"));
        }
        if head.starts_with("post /upload?uploadtype=resumable") {
            assert_eq!(header(head, "authorization").as_deref(), Some("bearer tok"));
            s.metadata = String::from_utf8_lossy(&body).to_string();
            return Some(reply("200 OK", &format!("Location: {}\r\n", session_uri), ""));
        }
        if head.starts_with("get /files?") {
            assert_eq!(header(head, "authorization").as_deref(), Some("bearer tok"));
            let request_line = head.lines().next().unwrap();
            s.list_queries.push(request_line.to_string());
            // First page: first file + a page token; second page: the rest.
            let (page, next): (Vec<_>, &str) = if request_line.contains("pagetoken=page2") {
                (s.listed_files.iter().skip(1).cloned().collect(), "")
            } else {
                (s.listed_files.iter().take(1).cloned().collect(), r#""nextPageToken":"page2","#)
            };
            let files: Vec<String> = page
                .iter()
                .map(|(id, name)| format!(r#"{{"id":"{}","name":"{}"}}"#, id, name))
                .collect();
            let body = format!(r#"{{{}"files":[{}]}}"#, next, files.join(","));
            return Some(reply("200 OK", "Content-Type: application/json\r\n", &body));
        }
        if let Some(rest) = head.strip_prefix("delete /files/") {
            let id = rest.split(|c| c == '?' || c == ' ').next().unwrap().to_string();
            s.deleted_ids.push(id);
            return Some(reply("204 No Content", "", ""));
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

    async fn fake_storage(state: Arc<Mutex<FakeDrive>>) -> GoogleDriveStorage {
        let base = spawn_fake_drive(state).await;
        let mut storage = GoogleDriveStorage::new(&GoogleDriveConfig {
            enabled: true,
            client_id: "cid".into(),
            client_secret: "csecret".into(),
            refresh_token: "rtoken".into(),
            folder_id: Some("folderXYZ".into()),
        });
        storage.token_url = format!("{}/token", base);
        storage.upload_url = format!("{}/upload", base);
        storage.files_url = format!("{}/files", base);
        storage.chunk_size = 4;
        storage.retry_delay = Duration::ZERO;
        storage.http = build_http_client(Duration::from_millis(300));
        storage
    }

    async fn run_upload(state: Arc<Mutex<FakeDrive>>, content: &[u8]) -> Result<String, BackupError> {
        let storage = fake_storage(state).await;
        let source = std::env::temp_dir().join(format!("gdrive_test_{}.zip", uuid::Uuid::new_v4()));
        tokio::fs::write(&source, content).await.unwrap();

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
        // The resumable session URI grants upload access: it must not reach logs or Telegram.
        assert!(!err.contains("/session"), "error leaks the session URI: {err}");
    }

    #[tokio::test]
    async fn retries_transient_failures_before_the_upload_starts() {
        let state = Arc::new(Mutex::new(FakeDrive {
            fail_token_once: true,
            fail_session_once: true,
            ..Default::default()
        }));
        let result = run_upload(state.clone(), b"0123456789").await;

        assert!(result.is_ok(), "{result:?}");
        let s = state.lock().unwrap();
        assert_eq!(s.received, b"0123456789");
        assert_eq!(s.token_requests, 2, "token request retried once");
    }

    #[tokio::test]
    async fn does_not_retry_rejected_credentials() {
        let state = Arc::new(Mutex::new(FakeDrive {
            reject_token: true,
            ..Default::default()
        }));
        let err = run_upload(state.clone(), b"0123456789").await.unwrap_err().to_string();

        assert!(err.contains("invalid_grant"), "{err}");
        assert_eq!(state.lock().unwrap().token_requests, 1, "4xx is permanent: no retry");
    }

    #[test]
    fn drive_query_literal_escapes_quotes_and_backslashes() {
        assert_eq!(drive_query_literal(r"O'Brien\DB"), r"O\'Brien\\DB");
    }

    #[tokio::test]
    async fn retention_deletes_only_expired_backups_of_this_database() {
        let recent = format!("ProdDB_{}.zip", chrono::Utc::now().format("%Y%m%d_%H%M%S"));
        let state = Arc::new(Mutex::new(FakeDrive {
            listed_files: vec![
                ("old1".into(), "ProdDB_20200101_000000.zip".into()),
                ("new1".into(), recent),
                ("other".into(), "ProdDBArchive_20200101_000000.zip".into()),
                ("manual".into(), "ProdDB_notes.zip".into()),
                ("old2".into(), "ProdDB_20200615_120000.zip".into()),
            ],
            ..Default::default()
        }));
        let storage = fake_storage(state.clone()).await;

        let deleted = storage.clean_old_backups("ProdDB", 7).await.unwrap();

        assert_eq!(deleted, 2);
        let s = state.lock().unwrap();
        assert_eq!(s.deleted_ids, vec!["old1", "old2"], "both pages scanned, only expired ProdDB files");
        assert_eq!(s.list_queries.len(), 2, "follows nextPageToken");
        assert!(s.list_queries[0].contains("folderxyz"), "scoped to the configured folder: {}", s.list_queries[0]);
        assert!(s.list_queries[0].contains("trashed"), "excludes trashed files: {}", s.list_queries[0]);
    }

    #[tokio::test]
    async fn retention_zero_days_makes_no_requests() {
        let state = Arc::new(Mutex::new(FakeDrive::default()));
        let storage = fake_storage(state.clone()).await;

        assert_eq!(storage.clean_old_backups("ProdDB", 0).await.unwrap(), 0);
        let s = state.lock().unwrap();
        assert_eq!(s.token_requests, 0);
        assert!(s.list_queries.is_empty());
    }
}
