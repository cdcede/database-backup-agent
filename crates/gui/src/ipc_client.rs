//! IPC client implementation for the Backup Agent GUI.
//!
//! Spawns a background worker thread that handles connection lifecycle,
//! reconnecting, and serializing requests to/from the service daemon.

use tokio::sync::mpsc;


use backup_agent_core::ipc::messages::{IpcRequest, IpcResponse};
use backup_agent_core::ipc::transport::{receive_message, send_message};

#[derive(Clone)]
pub struct IpcClientHandle {
    tx: mpsc::Sender<(IpcRequest, tokio::sync::oneshot::Sender<Result<IpcResponse, String>>)>,
}

impl IpcClientHandle {
    /// Create and spawn a new `IpcClientHandle` worker.
    pub fn new() -> Self {
        let (tx, mut rx) = mpsc::channel::<(IpcRequest, tokio::sync::oneshot::Sender<Result<IpcResponse, String>>)>(32);

        // Spawn a background thread running tokio runtime for IPC communications
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime for IPC client");
            rt.block_on(async {
                while let Some((request, reply_tx)) = rx.recv().await {
                    let response = execute_ipc_roundtrip(request).await;
                    let _ = reply_tx.send(response);
                }
            });
        });

        Self { tx }
    }

    /// Send a request to the background worker and await the response asynchronously.
    pub async fn send(&self, request: IpcRequest) -> Result<IpcResponse, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if self.tx.send((request, reply_tx)).await.is_err() {
            return Err("IPC background worker thread is dead".to_string());
        }
        match reply_rx.await {
            Ok(res) => res,
            Err(_) => Err("IPC background worker dropped response channel".to_string()),
        }
    }
}

async fn execute_ipc_roundtrip(request: IpcRequest) -> Result<IpcResponse, String> {
    #[cfg(windows)]
    {
        execute_windows_ipc(request).await
    }

    #[cfg(not(windows))]
    {
        execute_unix_ipc(request).await
    }
}

#[cfg(not(windows))]
async fn execute_unix_ipc(request: IpcRequest) -> Result<IpcResponse, String> {
    use tokio::net::UnixStream;
    let socket_path = "/tmp/backup_agent.sock";

    let mut stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| format!("Service is not running (UDS connection failed: {})", e))?;

    send_message(&mut stream, &request)
        .await
        .map_err(|e| format!("IPC write error: {}", e))?;

    let response: IpcResponse = receive_message(&mut stream)
        .await
        .map_err(|e| format!("IPC read error: {}", e))?;

    Ok(response)
}

#[cfg(windows)]
async fn execute_windows_ipc(request: IpcRequest) -> Result<IpcResponse, String> {
    use tokio::net::windows::named_pipe::ClientOptions;
    let pipe_name = r"\\.\pipe\BackupAgentPipe";

    // Open Named Pipe
    let mut stream = ClientOptions::new()
        .open(pipe_name)
        .map_err(|e| format!("Service is not running (Named Pipe connection failed: {})", e))?;

    send_message(&mut stream, &request)
        .await
        .map_err(|e| format!("IPC write error: {}", e))?;

    let response: IpcResponse = receive_message(&mut stream)
        .await
        .map_err(|e| format!("IPC read error: {}", e))?;

    Ok(response)
}
