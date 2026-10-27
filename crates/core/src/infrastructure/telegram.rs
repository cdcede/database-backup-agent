//! Telegram notification adapter implementation.
//!
//! Satisfies the `Notifier` port. Sends HTML-formatted messages to a Telegram
//! chat using the Telegram Bot API.

use std::time::Duration;

use reqwest::Client;
use serde::Serialize;

use crate::domain::backup_result::humanize_bytes;
use crate::domain::error::BackupError;
use crate::ports::notifier::Notifier;

/// Telegram Bot API request payload for `sendMessage`.
#[derive(Debug, Serialize)]
struct SendMessagePayload<'a> {
    chat_id: &'a str,
    text: &'a str,
    parse_mode: &'a str,
}

/// Telegram notification adapter.
pub struct TelegramNotifier {
    client: Client,
    token: String,
    chat_id: String,
    /// Base URL for the Telegram API (customizable for testing/mocking).
    base_url: Option<String>,
}

impl TelegramNotifier {
    /// Create a new `TelegramNotifier` instance with a default 10-second timeout.
    pub fn new(token: String, chat_id: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            token,
            chat_id,
            base_url: None,
        }
    }

    /// Internal constructor allowing a custom base URL for unit testing.
    #[cfg(test)]
    fn new_with_base_url(token: String, chat_id: String, base_url: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            token,
            chat_id,
            base_url: Some(base_url),
        }
    }

    /// Dispatch the message request to Telegram.
    async fn send_message(&self, text: &str) -> Result<(), BackupError> {
        let host = self.base_url.as_deref().unwrap_or("https://api.telegram.org");
        let url = format!("{}/bot{}/sendMessage", host, self.token);

        let payload = SendMessagePayload {
            chat_id: &self.chat_id,
            text,
            parse_mode: "HTML",
        };

        // ---------------------------------------------------------------------
        // Resilience: Fail fast on network issues / invalid responses
        // ---------------------------------------------------------------------
        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                BackupError::Notification(format!("Failed to connect to Telegram: {}", e))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error response body".to_string());
            
            return Err(BackupError::Notification(format!(
                "Telegram API returned status {}: {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Notify a non-fatal problem after a backup that itself succeeded
    /// (e.g. cloud retention cleanup failed).
    pub async fn send_warning(&self, database: &str, reason: &str) -> Result<(), BackupError> {
        let message = format!(
            "🟡 <b>Backup Warning</b>\n\n\
             <b>Database:</b> <code>{}</code>\n\
             <b>Issue:</b>\n<code>{}</code>",
            escape_html(database),
            escape_html(reason)
        );

        self.send_message(&message).await
    }
}

/// Escape text interpolated into an HTML `parse_mode` message.
/// Telegram only requires `&`, `<` and `>` to be escaped.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl Notifier for TelegramNotifier {
    async fn send_success(
        &self,
        database: &str,
        size_bytes: u64,
        destination: &str,
        elapsed_secs: u64,
    ) -> Result<(), BackupError> {
        let size_str = humanize_bytes(size_bytes);

        // We use HTML tags since they are simple to format and only require
        // escaping `&`, `<` and `>` in interpolated values (see `escape_html`).
        let message = format!(
            "🟢 <b>Backup Successful!</b>\n\n\
             <b>Database:</b> <code>{}</code>\n\
             <b>Backup Size:</b> <code>{}</code>\n\
             <b>Destination:</b> <code>{}</code>\n\
             <b>Time Elapsed:</b> <code>{} seconds</code>",
            escape_html(database),
            size_str,
            escape_html(destination),
            elapsed_secs
        );

        self.send_message(&message).await
    }

    async fn send_failure(&self, database: &str, reason: &str) -> Result<(), BackupError> {
        let message = format!(
            "🔴 <b>Backup Failed!</b>\n\n\
             <b>Database:</b> <code>{}</code>\n\
             <b>Reason:</b>\n<code>{}</code>",
            escape_html(database),
            escape_html(reason)
        );

        self.send_message(&message).await
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Launches a temporary local HTTP mock server that reads a single request
    /// and responds with the provided status code and response body.
    async fn run_mock_server(response_status: u16, response_body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let address = format!("http://127.0.0.1:{}", port);

        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let n = stream.read(&mut buffer).await.unwrap();
            let request_raw = String::from_utf8_lossy(&buffer[..n]).to_string();

            // HTTP 1.1 mock response
            let response = format!(
                "HTTP/1.1 {}\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n\
                 {}",
                if response_status == 200 { "200 OK" } else { "400 Bad Request" },
                response_body.len(),
                response_body
            );

            stream.write_all(response.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();

            request_raw
        });

        (address, handle)
    }

    #[tokio::test]
    async fn send_success_notification_transmits_html_payload() {
        let (mock_url, server_handle) = run_mock_server(200, r#"{"ok":true}"#).await;

        let notifier = TelegramNotifier::new_with_base_url(
            "mock-token".to_string(),
            "123456789".to_string(),
            mock_url,
        );

        notifier
            .send_success("AdventureWorks", 1024 * 1024 * 5, "S3 Bucket", 42)
            .await
            .unwrap();

        let request_content = server_handle.await.unwrap();
        
        // Assertions verifying structure and HTTP details
        assert!(request_content.contains("POST /botmock-token/sendMessage HTTP/1.1"));
        assert!(request_content.contains("\"chat_id\":\"123456789\""));
        assert!(request_content.contains("AdventureWorks"));
        assert!(request_content.contains("5.0 MB"));
        assert!(request_content.contains("S3 Bucket"));
        assert!(request_content.contains("parse_mode\":\"HTML\""));
    }

    #[tokio::test]
    async fn send_failure_notification_transmits_html_payload() {
        let (mock_url, server_handle) = run_mock_server(200, r#"{"ok":true}"#).await;

        let notifier = TelegramNotifier::new_with_base_url(
            "mock-token".to_string(),
            "123456789".to_string(),
            mock_url,
        );

        notifier
            .send_failure("AdventureWorks", "Database connection lost during backup")
            .await
            .unwrap();

        let request_content = server_handle.await.unwrap();
        
        assert!(request_content.contains("POST /botmock-token/sendMessage HTTP/1.1"));
        assert!(request_content.contains("\"chat_id\":\"123456789\""));
        assert!(request_content.contains("AdventureWorks"));
        assert!(request_content.contains("Database connection lost during backup"));
    }

    #[test]
    fn escape_html_escapes_telegram_reserved_characters() {
        assert_eq!(escape_html("a<b>&c"), "a&lt;b&gt;&amp;c");
        assert_eq!(escape_html("plain"), "plain");
    }

    #[tokio::test]
    async fn interpolated_text_is_html_escaped_in_every_message() {
        // Google error bodies and paths can contain <, > and &; unescaped they make
        // Telegram reject the whole message (HTML parse_mode) and the alert is lost.
        let raw = "error <html> & 'quota'";
        let escaped = "error &lt;html&gt; &amp; 'quota'";

        let (url, server) = run_mock_server(200, r#"{"ok":true}"#).await;
        let notifier = TelegramNotifier::new_with_base_url("t".into(), "1".into(), url);
        notifier.send_failure("Db<1>", raw).await.unwrap();
        let request = server.await.unwrap();
        assert!(request.contains(escaped), "failure: {request}");
        assert!(request.contains("Db&lt;1&gt;"), "failure database: {request}");
        assert!(request.contains("<b>Backup Failed!</b>"), "own markup kept: {request}");

        let (url, server) = run_mock_server(200, r#"{"ok":true}"#).await;
        let notifier = TelegramNotifier::new_with_base_url("t".into(), "1".into(), url);
        notifier.send_warning("Db", raw).await.unwrap();
        assert!(server.await.unwrap().contains(escaped), "warning not escaped");

        let (url, server) = run_mock_server(200, r#"{"ok":true}"#).await;
        let notifier = TelegramNotifier::new_with_base_url("t".into(), "1".into(), url);
        notifier.send_success("Db", 10, raw, 1).await.unwrap();
        assert!(server.await.unwrap().contains(escaped), "success destination not escaped");
    }

    #[tokio::test]
    async fn send_warning_is_not_labelled_as_a_failed_backup() {
        let (mock_url, server_handle) = run_mock_server(200, r#"{"ok":true}"#).await;

        let notifier = TelegramNotifier::new_with_base_url(
            "mock-token".to_string(),
            "123456789".to_string(),
            mock_url,
        );

        notifier
            .send_warning("AdventureWorks", "Google Drive retention cleanup failed")
            .await
            .unwrap();

        let request_content = server_handle.await.unwrap();
        assert!(request_content.contains("Backup Warning"), "{request_content}");
        assert!(!request_content.contains("Backup Failed"), "{request_content}");
        assert!(request_content.contains("AdventureWorks"));
        assert!(request_content.contains("Google Drive retention cleanup failed"));
    }

    #[tokio::test]
    async fn handles_api_error_responses_gracefully() {
        let (mock_url, _server_handle) = run_mock_server(400, r#"{"ok":false,"description":"Unauthorized"}"#).await;

        let notifier = TelegramNotifier::new_with_base_url(
            "mock-token".to_string(),
            "123456789".to_string(),
            mock_url,
        );

        let result = notifier
            .send_success("TestDB", 100, "Local Disk", 5)
            .await;

        assert!(result.is_err());
        if let Err(BackupError::Notification(msg)) = result {
            assert!(msg.contains("Telegram API returned status 400"));
            assert!(msg.contains("Unauthorized"));
        } else {
            panic!("Expected BackupError::Notification");
        }
    }
}
