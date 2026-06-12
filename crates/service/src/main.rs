//! Backup Agent Service background daemon entry point.
//!
//! Exposes command line installation arguments and SCM dispatcher startup paths.
//! Falls back to console execution mode when run interactively.

use clap::{Parser, Subcommand};

mod service_handler;
mod scheduler;
mod ipc_server;


#[derive(Parser, Debug)]
#[command(
    name = "backup-agent-service",
    version,
    about = "Backup Agent Background Daemon"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Register this binary as an automatic Windows Service
    Install,
    /// Remove the Windows Service registration
    Uninstall,
    /// Run the daemon interactively in the terminal (default fallback)
    Run,
}

fn main() {
    // Write logs to a file in the same directory as the executable
    let exe_dir = std::env::current_exe()

        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default();
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(exe_dir.join("backup-agent.log"))
    {
        tracing_subscriber::fmt()
            .with_writer(file)
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt::init();
    }


    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Install) => {
            if let Err(e) = install_service() {
                eprintln!("Installation failed: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Uninstall) => {
            if let Err(e) = uninstall_service() {
                eprintln!("Uninstallation failed: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Run) => {
            service_handler::run_in_console();
        }
        None => {
            // No command: try running inside Windows SCM
            #[cfg(windows)]
            {
                match service_handler::start_dispatcher() {
                    Ok(true) => {
                        // SCM running
                    }
                    Ok(false) => {
                        // Handshake connection failed -> interactive console fallback
                        service_handler::run_in_console();
                    }
                    Err(e) => {
                        eprintln!("Windows Service dispatcher error: {:?}", e);
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(not(windows))]
            {
                // Non-Windows targets run interactively in terminal directly
                service_handler::run_in_console();
            }
        }
    }
}

// -----------------------------------------------------------------------------
// SCM Registration Command Helpers (Windows Only)
// -----------------------------------------------------------------------------

#[cfg(windows)]
fn install_service() -> std::io::Result<()> {
    let current_exe = std::env::current_exe()?;

    // Invoke sc.exe utility (standard Windows service administrator command)
    // Create displays display names and automatic startup configuration
    let output = std::process::Command::new("sc.exe")
        .args(&[
            "create",
            "BackupAgent",
            &format!("binPath= \"{}\"", current_exe.to_string_lossy()),
            "start=",
            "auto",
            "DisplayName=",
            "Backup Agent Service",
        ])
        .output()?;

    if output.status.success() {
        println!("Windows Service 'BackupAgent' registered successfully.");
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("sc.exe error: {}\n{}", err, out),
        ))
    }
}

#[cfg(windows)]
fn uninstall_service() -> std::io::Result<()> {
    let output = std::process::Command::new("sc.exe")
        .args(&["delete", "BackupAgent"])
        .output()?;

    if output.status.success() {
        println!("Windows Service 'BackupAgent' removed successfully.");
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("sc.exe error: {}\n{}", err, out),
        ))
    }
}

// -----------------------------------------------------------------------------
// Cross-Platform Stubs (macOS / Linux)
// -----------------------------------------------------------------------------

#[cfg(not(windows))]
fn install_service() -> std::io::Result<()> {
    println!("Windows Service installation is only supported on Windows targets.");
    Ok(())
}

#[cfg(not(windows))]
fn uninstall_service() -> std::io::Result<()> {
    println!("Windows Service uninstallation is only supported on Windows targets.");
    Ok(())
}
