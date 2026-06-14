//! IPC transport framing and serialization helpers.
//!
//! Implements a length-prefixed framing protocol over any asynchronous stream
//! (TCP, Unix Sockets, or Windows Named Pipes) using `bincode`.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::domain::error::BackupError;

/// Send a serializable message over an async writer using length-prefixed framing.
///
/// Protocol frame layout:
/// `[ 4 bytes (u32 big-endian payload length) ] [ payload bytes ]`
pub async fn send_message<W, M>(writer: &mut W, message: &M) -> Result<(), BackupError>
where
    W: AsyncWrite + Unpin,
    M: serde::Serialize,
{
    // 1. Serialize the message to binary format
    let payload = bincode::serialize(message).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Serialization failed: {}", e),
        )
    })?;

    let len = payload.len() as u32;

    // 2. Write the 4-byte length prefix (big-endian/network byte order)
    writer.write_u32(len).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to write IPC message length prefix");
        e
    })?;

    // 3. Write the actual message payload
    writer.write_all(&payload).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to write IPC message body");
        e
    })?;

    // 4. Flush the stream to ensure buffer is dispatched
    writer.flush().await?;

    Ok(())
}

/// Read a deserializable message from an async reader using length-prefixed framing.
pub async fn receive_message<R, M>(reader: &mut R) -> Result<M, BackupError>
where
    R: AsyncRead + Unpin,
    M: serde::de::DeserializeOwned,
{
    // 1. Read the 4-byte length prefix
    let len = reader.read_u32().await.map_err(|e| {
        // We don't log EOF as an error since connection drops are expected on client exit
        if e.kind() != std::io::ErrorKind::UnexpectedEof {
            tracing::error!(error = %e, "Failed to read IPC message length prefix");
        }
        e
    })?;

    // Safety limit: Don't allocate huge buffers if stream is corrupted (max 16 MB)
    const MAX_PAYLOAD_SIZE: u32 = 16 * 1024 * 1024;
    if len > MAX_PAYLOAD_SIZE {
        return Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("IPC message length prefix too large: {} bytes", len),
        )));
    }

    // 2. Read exactly `len` payload bytes
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await.map_err(|e| {
        tracing::error!(error = %e, expected_bytes = len, "Failed to read full IPC payload");
        e
    })?;

    // 3. Deserialize binary back to typed message
    let message = bincode::deserialize(&payload).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Deserialization failed: {}", e),
        )
    })?;

    Ok(message)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::messages::{IpcRequest, IpcResponse};
    use crate::domain::config::{AppConfig, SqlServerConfig, AuthMethod, BackupConfig, BackupTaskConfig};
    use tokio::io::duplex;

    #[tokio::test]
    async fn roundtrip_request_and_response() {
        // Create an in-memory duplex stream pair (client ↔ server)
        let (mut client, mut server) = duplex(1024);

        // 1. Client sends a request
        let request = IpcRequest::TriggerBackup {
            database_name: "SalesDB".to_string(),
        };

        let client_sender_task = tokio::spawn(async move {
            send_message(&mut client, &request).await.unwrap();
            client
        });

        // 2. Server receives the request and sends a response
        let request_received: IpcRequest = receive_message(&mut server).await.unwrap();
        assert_eq!(
            request_received,
            IpcRequest::TriggerBackup {
                database_name: "SalesDB".to_string()
            }
        );

        let response = IpcResponse::Ok;
        send_message(&mut server, &response).await.unwrap();

        // 3. Client receives the response
        let mut client = client_sender_task.await.unwrap();
        let response_received: IpcResponse = receive_message(&mut client).await.unwrap();
        assert_eq!(response_received, IpcResponse::Ok);
    }

    #[tokio::test]
    async fn roundtrip_complex_config_payload() {
        let (mut client, mut server) = duplex(4096);

        let config = AppConfig {
            sql_server: SqlServerConfig {
                host: "localhost".to_string(),
                port: 1433,
                auth_method: AuthMethod::Sql,
                username: Some("sa".to_string()),
                password: Some("secret".to_string()),
            },
            backup: BackupConfig {
                local_path: std::path::PathBuf::from("/backups"),
            },
            tasks: vec![BackupTaskConfig {
                name: "Daily Task".to_string(),
                databases: vec!["DB1".to_string(), "DB2".to_string()],
                schedule: "03:00".to_string(),
                retention_days: 7,
            }],
            telegram: Default::default(),
            storage: Default::default(),
            service: Default::default(),
        };

        let request = IpcRequest::UpdateConfig(config.clone());
        
        tokio::spawn(async move {
            send_message(&mut client, &request).await.unwrap();
        });

        let request_received: IpcRequest = receive_message(&mut server).await.unwrap();
        assert_eq!(request_received, IpcRequest::UpdateConfig(config));
    }
}
