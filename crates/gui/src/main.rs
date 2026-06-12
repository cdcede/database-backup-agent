#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod views;
mod ipc_client;


use app::BackupAgentApp;
use eframe::egui;

fn main() -> eframe::Result {
    // Initialize standard logging
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([950.0, 620.0])
            .with_min_inner_size([750.0, 500.0])
            .with_title("Backup Agent"),
        ..Default::default()
    };


    eframe::run_native(
        "Backup Agent",
        options,
        Box::new(|cc| Ok(Box::new(BackupAgentApp::new(cc)))),
    )
}
