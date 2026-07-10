//! IPC server implementation for the Backup Agent Service daemon.
//!
//! Exposes UDS on Unix (macOS) and Named Pipes on Windows to handle incoming GUI requests.

use std::sync::Arc;

use backup_agent_core::ipc::messages::{IpcRequest, IpcResponse};
use backup_agent_core::ipc::transport::{receive_message, send_message};

use crate::service_handler::DaemonState;

/// Starts the IPC server listener.
pub async fn start_ipc_server(
    state: Arc<DaemonState>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {

    #[cfg(windows)]
    {
        start_windows_ipc(state, shutdown_rx).await;
    }

    #[cfg(not(windows))]
    {
        start_unix_ipc(state, shutdown_rx).await;
    }
}

#[cfg(not(windows))]
async fn start_unix_ipc(state: Arc<DaemonState>, mut shutdown_rx: tokio::sync::oneshot::Receiver<()>) {
    use tokio::net::UnixListener;
    
    let socket_path = "/tmp/backup_agent.sock";
    
    // Remove old socket file if it exists
    let _ = tokio::fs::remove_file(socket_path).await;
    
    let listener = match UnixListener::bind(socket_path) {
        Ok(l) => {
            tracing::info!("IPC Server listening on UDS: {}", socket_path);
            l
        }
        Err(e) => {
            tracing::error!("Failed to bind to UDS socket '{}': {}", socket_path, e);
            return;
        }
    };
    
    loop {
        tokio::select! {
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((stream, _)) => {
                        let state_clone = state.clone();
                        tokio::spawn(async move {
                            handle_client_connection(stream, state_clone).await;
                        });
                    }
                    Err(e) => {
                        tracing::error!("UDS accept error: {}", e);
                    }
                }
            }
            _ = &mut shutdown_rx => {
                tracing::info!("Shutdown signal received in Unix IPC server. Exiting loop.");
                break;
            }
        }
    }
    
    let _ = tokio::fs::remove_file(socket_path).await;
}

#[cfg(windows)]
async fn start_windows_ipc(state: Arc<DaemonState>, mut shutdown_rx: tokio::sync::oneshot::Receiver<()>) {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = r"\\.\pipe\BackupAgentPipe";
    tracing::info!("IPC Server listening on Named Pipe: {}", pipe_name);

    // `first_pipe_instance(true)` is only valid on the very first instance of the
    // pipe — it tells Windows to fail if another instance already exists. Every
    // subsequent instance created by this loop (one per accepted client, since
    // clients open a fresh connection per request) must NOT set it, otherwise
    // CreateNamedPipe returns ERROR_ACCESS_DENIED (5) forever after the first
    // client connects, and the server can never accept another connection.
    let mut is_first_instance = true;

    loop {
        let mut server_options = ServerOptions::new();
        server_options.first_pipe_instance(is_first_instance);

        let server = match server_options.create(pipe_name) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to create named pipe server: {}", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        is_first_instance = false;

        tokio::select! {
            connect_res = server.connect() => {
                if connect_res.is_ok() {
                    let state_clone = state.clone();
                    tokio::spawn(async move {
                        handle_client_connection(server, state_clone).await;
                    });
                }
            }
            _ = &mut shutdown_rx => {
                tracing::info!("Shutdown signal received in Windows IPC server. Exiting loop.");
                break;
            }
        }
    }
}

async fn handle_client_connection<S>(mut stream: S, state: Arc<DaemonState>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let request: IpcRequest = match receive_message(&mut stream).await {
            Ok(req) => req,
            Err(backup_agent_core::domain::error::BackupError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => {
                tracing::error!("IPC read error: {:?}", e);
                break;
            }
        };

        let response = match request {
            IpcRequest::GetConfig => {
                let config = state.config.lock().await.clone();
                IpcResponse::Config(config)
            }
            IpcRequest::UpdateConfig(new_config) => {
                if let Err(e) = new_config.ensure_valid() {
                    IpcResponse::Error(format!("Invalid config: {}", e))
                } else {
                    let mut config = state.config.lock().await;
                    *config = new_config.clone();
                    
                    let exe_dir = state.exe_dir.clone();
                    let config_path = exe_dir.join("config.toml");
                    if let Err(e) = backup_agent_core::infrastructure::config_loader::save(&new_config, &config_path) {
                        tracing::error!("Failed to save config to disk: {}", e);
                        IpcResponse::Error(format!("Persistence error: {}", e))
                    } else {
                        // Notify scheduler to hot-reload config tasks!
                        let _ = state.reload_tx.send(());
                        IpcResponse::Ok
                    }
                }
            }
            IpcRequest::TriggerBackup { database_name } => {
                let state_clone = state.clone();
                tokio::spawn(async move {
                    crate::service_handler::run_single_manual_backup(&database_name, state_clone).await;
                });
                IpcResponse::Ok
            }
            IpcRequest::GetStatus => {
                let active = state.active_jobs.lock().await.clone();
                IpcResponse::Status(active)
            }
            IpcRequest::GetHistory => {
                let history = state.history.lock().await.clone();
                IpcResponse::History(history)
            }
            IpcRequest::TestConnection(db_config) => {
                let backup_adapter = backup_agent_core::infrastructure::mssql::MssqlBackup::new(db_config);
                match backup_adapter.list_databases().await {
                    Ok(dbs) => IpcResponse::Databases(dbs),
                    Err(e) => IpcResponse::Error(format!("Connection test failed: {}", e)),
                }
            }
        };

        if let Err(e) = send_message(&mut stream, &response).await {
            tracing::error!("IPC write error: {:?}", e);
            break;
        }
    }
}
