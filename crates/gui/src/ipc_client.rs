//! IPC client implementation for the Backup Agent GUI.
//!
//! Owns one tokio runtime for the whole GUI. A background worker serializes
//! requests to/from the service daemon, and views use [`IpcClientHandle::spawn`]
//! to run async work without blocking the UI thread or creating extra runtimes.

use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc;

use backup_agent_core::ipc::messages::{IpcRequest, IpcResponse};
use backup_agent_core::ipc::transport::{receive_message, send_message};

/// Upper bound for one request/response roundtrip. `TestConnection` connects
/// to SQL Server, so this must leave room for a slow TDS handshake.
const IPC_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct IpcClientHandle {
    tx: mpsc::Sender<(IpcRequest, tokio::sync::oneshot::Sender<Result<IpcResponse, String>>)>,
    /// Lives for the whole process. Handles are moved into tasks running on this
    /// runtime, and dropping a runtime from inside one of its own tasks panics.
    runtime: &'static tokio::runtime::Runtime,
}

impl IpcClientHandle {
    /// Create the shared runtime and spawn the IPC worker on it.
    pub fn new() -> Self {
        let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for IPC client"),
        ));
        let (tx, mut rx) = mpsc::channel::<(IpcRequest, tokio::sync::oneshot::Sender<Result<IpcResponse, String>>)>(32);

        runtime.spawn(async move {
            while let Some((request, reply_tx)) = rx.recv().await {
                let response = match tokio::time::timeout(IPC_TIMEOUT, execute_ipc_roundtrip(request)).await {
                    Ok(response) => response,
                    Err(_) => Err(format!(
                        "The Backup Agent service did not respond within {} seconds.",
                        IPC_TIMEOUT.as_secs()
                    )),
                };
                let _ = reply_tx.send(response);
            }
        });

        Self { tx, runtime }
    }

    /// Run async work on the shared runtime (fire and forget).
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future);
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
        .map_err(|e| service_unreachable(&e))?;

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
        .map_err(|e| service_unreachable(&e))?;

    send_message(&mut stream, &request)
        .await
        .map_err(|e| format!("IPC write error: {}", e))?;

    let response: IpcResponse = receive_message(&mut stream)
        .await
        .map_err(|e| format!("IPC read error: {}", e))?;

    Ok(response)
}

fn service_unreachable(e: &std::io::Error) -> String {
    format!("Cannot reach the Backup Agent service. Make sure the 'BackupAgent' Windows service is running. ({e})")
}
