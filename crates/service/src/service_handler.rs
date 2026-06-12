//! Windows Service lifecycle and control handler implementation.
//!
//! Exposes SCM (Service Control Manager) integration for Windows, and fallback
//! interactive console runners for development and testing on Unix/macOS.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
#[cfg(windows)]
use tokio::sync::oneshot;

use backup_agent_core::domain::config::{AppConfig, StorageProviderType};
use backup_agent_core::domain::error::BackupError;
use backup_agent_core::domain::backup_job::BackupJob;
use backup_agent_core::domain::backup_result::BackupResult;
use backup_agent_core::infrastructure::telegram::TelegramNotifier;
use backup_agent_core::infrastructure::mssql::MssqlBackup;
use backup_agent_core::infrastructure::compression::ZipCompressor;
use backup_agent_core::infrastructure::storage::local::LocalStorage;
#[cfg(feature = "s3")]
use backup_agent_core::infrastructure::storage::s3::S3Storage;
use backup_agent_core::application::backup_service::BackupOrchestrator;
use backup_agent_core::application::retention::clean_old_backups;
use backup_agent_core::ports::notifier::Notifier;




/// Shared application context or state.
pub struct ServiceContext {
    /// Directory where the service executable resides.
    pub exe_dir: PathBuf,
}

impl ServiceContext {
    /// Resolve context based on the current running executable.
    pub fn resolve() -> std::io::Result<Self> {
        let exe_path = std::env::current_exe()?;
        let exe_dir = exe_path
            .parent()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "Failed to resolve executable directory")
            })?
            .to_path_buf();

        Ok(Self { exe_dir })
    }
}

/// Global shared daemon state.
pub struct DaemonState {
    pub exe_dir: PathBuf,
    pub config: Mutex<AppConfig>,
    pub active_jobs: Mutex<Vec<BackupJob>>,
    pub history: Mutex<Vec<BackupResult>>,
    pub reload_tx: tokio::sync::broadcast::Sender<()>,
}

/// Fallback runner to execute the service daemon interactively in the terminal.
///
/// Used during development on macOS/Linux and manual console execution on Windows.
pub fn run_in_console() {
    tracing::info!("Starting Backup Agent Daemon in interactive console mode");

    let context = match ServiceContext::resolve() {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::error!("Failed to resolve service context: {}", e);
            return;
        }
    };

    // Configure tracing to print to stdout
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        // Load configuration
        let config = match load_config(&context) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::error!("Failed to load config: {}", e);
                return;
            }
        };

        if let Err(e) = config.ensure_valid() {
            tracing::error!("Configuration validation failed: {}", e);
            return;
        }

        // Load historical records
        let history = load_history(&context.exe_dir);

        // Initialize state
        let (reload_tx, _) = tokio::sync::broadcast::channel(16);
        let state = Arc::new(DaemonState {
            exe_dir: context.exe_dir.clone(),
            config: Mutex::new(config.clone()),
            active_jobs: Mutex::new(Vec::new()),
            history: Mutex::new(history),
            reload_tx,
        });

        tracing::info!("Daemon running. Press Ctrl+C to shutdown gracefully.");

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (ipc_shutdown_tx, ipc_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn IPC Server
        let state_for_ipc = state.clone();
        let ipc_handle = tokio::spawn(async move {
            crate::ipc_server::start_ipc_server(state_for_ipc, ipc_shutdown_rx).await;
        });

        // Spawn scheduler loop
        let state_for_scheduler = state.clone();
        let scheduler_handle = tokio::spawn(async move {
            crate::scheduler::start_multi_scheduler(state_for_scheduler, shutdown_rx).await;
        });

        // Wait for standard console terminate signal (Ctrl+C)
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("Failed to register Ctrl+C listener: {}", e);
        }

        tracing::info!("Shutdown signal received. Cleaning up resources...");
        let _ = shutdown_tx.send(());
        let _ = ipc_shutdown_tx.send(());
        let _ = scheduler_handle.await;
        let _ = ipc_handle.await;
    });

    tracing::info!("Backup Agent Daemon terminated");
}

// =============================================================================
// Helper Functions for Configuration, History and Backup Execution
// =============================================================================

fn load_config(context: &ServiceContext) -> Result<AppConfig, BackupError> {
    let config_path = context.exe_dir.join("config.toml");
    backup_agent_core::infrastructure::config_loader::load_or_create_default(&config_path)
}

fn load_history(exe_dir: &std::path::Path) -> Vec<BackupResult> {
    let history_path = exe_dir.join("history.json");
    if history_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&history_path) {
            if let Ok(history) = serde_json::from_str(&content) {
                return history;
            }
        }
    }
    Vec::new()
}

fn save_history(exe_dir: &std::path::Path, history: &[BackupResult]) {
    let history_path = exe_dir.join("history.json");
    if let Ok(content) = serde_json::to_string_pretty(history) {
        let _ = std::fs::write(&history_path, content);
    }
}

struct NoopNotifier;

impl backup_agent_core::ports::notifier::Notifier for NoopNotifier {
    async fn send_success(
        &self,
        _database: &str,
        _size_bytes: u64,
        _destination: &str,
        _elapsed_secs: u64,
    ) -> Result<(), BackupError> {
        Ok(())
    }

    async fn send_failure(&self, _database: &str, _reason: &str) -> Result<(), BackupError> {
        Ok(())
    }
}

enum AppNotifier {
    Telegram(backup_agent_core::infrastructure::telegram::TelegramNotifier),
    Noop(NoopNotifier),
}


impl backup_agent_core::ports::notifier::Notifier for AppNotifier {
    async fn send_success(
        &self,
        database: &str,
        size_bytes: u64,
        destination: &str,
        elapsed_secs: u64,
    ) -> Result<(), BackupError> {
        match self {
            Self::Telegram(n) => n.send_success(database, size_bytes, destination, elapsed_secs).await,
            Self::Noop(n) => n.send_success(database, size_bytes, destination, elapsed_secs).await,
        }
    }

    async fn send_failure(&self, database: &str, reason: &str) -> Result<(), BackupError> {
        match self {
            Self::Telegram(n) => n.send_failure(database, reason).await,
            Self::Noop(n) => n.send_failure(database, reason).await,
        }
    }
}

enum AppStorage {
    Local(backup_agent_core::infrastructure::storage::local::LocalStorage),
    #[cfg(feature = "s3")]
    S3(backup_agent_core::infrastructure::storage::s3::S3Storage),
}

impl backup_agent_core::ports::storage::Storage for AppStorage {
    async fn upload(&self, source: &std::path::Path, filename: &str) -> Result<String, BackupError> {
        match self {
            Self::Local(s) => s.upload(source, filename).await,
            #[cfg(feature = "s3")]
            Self::S3(s) => s.upload(source, filename).await,
        }
    }
}

async fn execute_single_backup(db_name: &str, state: Arc<DaemonState>, retention_days: u32) -> BackupResult {
    use chrono::Utc;

    let config = state.config.lock().await.clone();

    // 1. Create a BackupJob and add to active_jobs
    let mut job = BackupJob::new(db_name);
    job.start();
    {
        let mut active = state.active_jobs.lock().await;
        active.push(job.clone());
    }

    // 2. Prepare temp dir and backend ports
    let temp_dir = config.backup.local_path.join("temp");
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let storage_filename = format!("{}_{}.zip", db_name, timestamp);

    let notifier = if config.telegram.enabled {
        AppNotifier::Telegram(TelegramNotifier::new(
            config.telegram.bot_token.clone(),
            config.telegram.chat_id.clone(),
        ))
    } else {
        AppNotifier::Noop(NoopNotifier)
    };

    let storage = match config.storage.provider {
        StorageProviderType::Local => {
            AppStorage::Local(LocalStorage::new(config.backup.local_path.clone()))
        }
        StorageProviderType::S3 => {
            #[cfg(feature = "s3")]
            {
                if let Some(s3_cfg) = &config.storage.s3 {
                    AppStorage::S3(S3Storage::new(s3_cfg.bucket.clone(), None))
                } else {
                    tracing::error!("S3 storage configured but [storage.s3] section is missing.");
                    use backup_agent_core::ports::notifier::Notifier;
                    let _ = notifier.send_failure(db_name, "S3 storage settings are missing").await;
                    
                    let mut completed_job = job;
                    completed_job.fail("S3 storage settings are missing");
                    
                    // Remove from active_jobs
                    {
                        let mut active = state.active_jobs.lock().await;
                        active.retain(|j| j.id != completed_job.id);
                    }
                    
                    return BackupResult::from_completed_job(&completed_job, 0, None, "");
                }
            }
            #[cfg(not(feature = "s3"))]
            {
                tracing::error!("S3 storage configured but service was built without S3 support.");
                use backup_agent_core::ports::notifier::Notifier;
                let _ = notifier.send_failure(db_name, "S3 storage not enabled in this build").await;
                
                let mut completed_job = job;
                completed_job.fail("S3 storage not enabled in this build");
                
                // Remove from active_jobs
                {
                    let mut active = state.active_jobs.lock().await;
                    active.retain(|j| j.id != completed_job.id);
                }
                
                return BackupResult::from_completed_job(&completed_job, 0, None, "");
            }
        }
    };

    let db = MssqlBackup::new(config.sql_server.clone());
    let compressor = ZipCompressor::new();

    let orchestrator = BackupOrchestrator::new(db, compressor, storage, notifier);
    let result = orchestrator.run_backup(db_name, &temp_dir, &storage_filename).await;

    // 3. Remove from active_jobs
    {
        let mut active = state.active_jobs.lock().await;
        active.retain(|j| j.id != job.id);
    }

    // 4. Save to history
    {
        let mut history = state.history.lock().await;
        history.push(result.clone());
        save_history(&state.exe_dir, &history);
    }

    // 5. Run retention cleanup
    if config.storage.provider == StorageProviderType::Local {
        if let Err(e) = clean_old_backups(&config.backup.local_path, db_name, retention_days).await {
            tracing::error!("Failed to run retention cleanup for database '{}': {}", db_name, e);
        }
    }

    result
}

pub async fn run_task_backup_job(task_name: &str, state: Arc<DaemonState>) {
    let config = state.config.lock().await.clone();
    let temp_dir = config.backup.local_path.join("temp");

    // Find the task in the configuration
    let Some(task) = config.tasks.iter().find(|t| t.name == task_name) else {
        tracing::error!("Task '{}' not found in configuration", task_name);
        return;
    };

    if let Err(e) = tokio::fs::create_dir_all(&temp_dir).await {
        tracing::error!("Failed to create temporary directory '{}': {}", temp_dir.display(), e);
        let notifier = if config.telegram.enabled {
            AppNotifier::Telegram(TelegramNotifier::new(
                config.telegram.bot_token.clone(),
                config.telegram.chat_id.clone(),
            ))
        } else {
            AppNotifier::Noop(NoopNotifier)
        };
        let _ = notifier.send_failure(&format!("Task: {task_name}"), &format!("Failed to create temp directory: {e}")).await;
        return;
    }

    tracing::info!("Starting scheduled backup execution for Task '{}'", task_name);
    for db_name in &task.databases {
        execute_single_backup(db_name, state.clone(), task.retention_days).await;
    }

    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}

pub async fn run_single_manual_backup(db_name: &str, state: Arc<DaemonState>) {
    let config = state.config.lock().await.clone();
    let temp_dir = config.backup.local_path.join("temp");
    let _ = tokio::fs::create_dir_all(&temp_dir).await;

    // Use task-specific retention days if configured, otherwise default to 7
    let retention_days = config.tasks.iter()
        .find(|t| t.databases.contains(&db_name.to_string()))
        .map(|t| t.retention_days)
        .unwrap_or(7);

    execute_single_backup(db_name, state.clone(), retention_days).await;

    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}

// =============================================================================
// Windows Service Control Manager Integration
// =============================================================================

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
#[cfg(windows)]
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};

#[cfg(windows)]
windows_service::define_windows_service!(ffi_service_main, my_service_main);

/// Low-level service entry point called by the Windows SCM.
#[cfg(windows)]
fn my_service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service_loop() {
        tracing::error!("Fatal Windows Service runtime error: {:?}", e);
    }
}

/// Main execution loop under Windows SCM control.
#[cfg(windows)]
fn run_service_loop() -> Result<(), windows_service::Error> {
    tracing::info!("Initializing Windows Service control handler");

    // Channels to communicate SCM stop events to our async Tokio tasks
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let mut shutdown_tx = Some(shutdown_tx);

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop => {
                tracing::info!("SCM Stop signal received; triggering graceful shutdown");
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => {
                // SCM requesting status check
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    // Register our event callback with the Windows Service Controller
    let status_handle = service_control_handler::register("BackupAgent", event_handler)?;

    // Transition service state to running in the SCM database
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::from_secs(5),
    })?;

    // Create the Tokio runtime to execute async I/O and tasks
    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        windows_service::Error::Win32(e.raw_os_error().unwrap_or(1) as u32)
    })?;

    let context = ServiceContext::resolve().map_err(|e| {
        windows_service::Error::Win32(e.raw_os_error().unwrap_or(1) as u32)
    })?;

    rt.block_on(async {
        tracing::info!("Async service scheduler and loop active");
        
        // Load configuration
        let config = match load_config(&context) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::error!("Failed to load config: {}", e);
                return;
            }
        };

        if let Err(e) = config.ensure_valid() {
            tracing::error!("Configuration validation failed: {}", e);
            return;
        }

        // Load historical records
        let history = load_history(&context.exe_dir);

        // Initialize state
        let (reload_tx, _) = tokio::sync::broadcast::channel(16);
        let state = Arc::new(DaemonState {
            exe_dir: context.exe_dir.clone(),
            config: Mutex::new(config.clone()),
            active_jobs: Mutex::new(Vec::new()),
            history: Mutex::new(history),
            reload_tx,
        });

        let (scheduler_shutdown_tx, scheduler_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (ipc_shutdown_tx, ipc_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn IPC Server
        let state_for_ipc = state.clone();
        let ipc_handle = tokio::spawn(async move {
            crate::ipc_server::start_ipc_server(state_for_ipc, ipc_shutdown_rx).await;
        });

        // Spawn scheduler loop
        let state_for_scheduler = state.clone();
        let scheduler_handle = tokio::spawn(async move {
            crate::scheduler::start_multi_scheduler(state_for_scheduler, scheduler_shutdown_rx).await;
        });

        // Wait for SCM stop event trigger
        let _ = shutdown_rx.await;
        
        tracing::info!("SCM Stop callback completed. Cleaning up scheduler tasks");
        let _ = scheduler_shutdown_tx.send(());
        let _ = ipc_shutdown_tx.send(());
        let _ = scheduler_handle.await;
        let _ = ipc_handle.await;
    });

    // Notify SCM that we have successfully terminated
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::default(),
    })?;

    tracing::info!("Windows Service stopped cleanly");
    Ok(())
}

/// SCM dispatcher entry point to start the dispatcher loop.
///
/// Returns `Ok(true)` if dispatcher started, or `Ok(false)` if running in console
/// mode (dispatcher connection failed).
#[cfg(windows)]
pub fn start_dispatcher() -> Result<bool, windows_service::Error> {
    use windows_service::service_dispatcher;
    
    tracing::debug!("Attempting SCM dispatcher handshake");
    match service_dispatcher::start("BackupAgent", ffi_service_main) {
        Ok(_) => Ok(true),
        Err(windows_service::Error::Win32(1063)) => {
            // ERROR_FAILED_SERVICE_CONTROLLER_CONNECT -> Interactive CLI mode
            Ok(false)
        }
        Err(e) => Err(e),
    }
}
