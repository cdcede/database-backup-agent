use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;
use eframe::egui;

use crate::ipc_client::IpcClientHandle;
use crate::views;
use backup_agent_core::domain::config::AppConfig;
use backup_agent_core::domain::backup_job::BackupJob;
use backup_agent_core::domain::backup_result::BackupResult;
use backup_agent_core::ipc::messages::IpcRequest;
use backup_agent_core::ipc::messages::IpcResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Settings,
    History,
    Logs,
}

pub enum AppUpdateEvent {
    ConfigFetched(AppConfig),
    StatusFetched(Vec<BackupJob>),
    HistoryFetched(Vec<BackupResult>),
    IpcError(String),
}

pub struct BackupAgentApp {
    active_view: ActiveView,
    ipc_client: IpcClientHandle,
    
    // Channels for async updates
    update_tx: Sender<AppUpdateEvent>,
    update_rx: Receiver<AppUpdateEvent>,
    
    // Cached states
    pub config: Option<AppConfig>,
    pub active_jobs: Vec<BackupJob>,
    pub history: Vec<BackupResult>,
    pub error_message: Option<String>,
    
    // Timer
    last_update: Instant,
}

impl BackupAgentApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_custom_styles(&cc.egui_ctx);

        let (update_tx, update_rx) = channel();
        let ipc_client = IpcClientHandle::new();

        Self {
            active_view: ActiveView::Dashboard,
            ipc_client,
            update_tx,
            update_rx,
            config: None,
            active_jobs: Vec::new(),
            history: Vec::new(),
            error_message: None,
            // Initialize last_update to far in the past to trigger instant load
            last_update: Instant::now() - std::time::Duration::from_secs(10),
        }
    }
}

impl eframe::App for BackupAgentApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Process pending background events
        while let Ok(event) = self.update_rx.try_recv() {
            match event {
                AppUpdateEvent::ConfigFetched(cfg) => {
                    self.config = Some(cfg);
                    self.error_message = None;
                }
                AppUpdateEvent::StatusFetched(jobs) => {
                    self.active_jobs = jobs;
                }
                AppUpdateEvent::HistoryFetched(hist) => {
                    self.history = hist;
                }
                AppUpdateEvent::IpcError(e) => {
                    self.error_message = Some(e);
                }
            }
        }

        // 2. Query status periodically (every 2 seconds)
        if self.config.is_none() || self.last_update.elapsed() > std::time::Duration::from_secs(2) {
            self.last_update = Instant::now();
            let client = self.ipc_client.clone();
            let tx = self.update_tx.clone();
            
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    // Fetch config
                    match client.send(IpcRequest::GetConfig).await {
                        Ok(IpcResponse::Config(cfg)) => {
                            let _ = tx.send(AppUpdateEvent::ConfigFetched(cfg));
                        }
                        Err(e) => {
                            let _ = tx.send(AppUpdateEvent::IpcError(e));
                        }
                        _ => {}
                    }
                    
                    // Fetch active status
                    match client.send(IpcRequest::GetStatus).await {
                        Ok(IpcResponse::Status(jobs)) => {
                            let _ = tx.send(AppUpdateEvent::StatusFetched(jobs));
                        }
                        _ => {}
                    }

                    // Fetch history
                    match client.send(IpcRequest::GetHistory).await {
                        Ok(IpcResponse::History(hist)) => {
                            let _ = tx.send(AppUpdateEvent::HistoryFetched(hist));
                        }
                        _ => {}
                    }
                });
            });
        }

        // Left side navigation bar
        egui::SidePanel::left("sidebar")
            .resizable(false)
            .default_width(220.0)
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(9, 13, 22))) // Sleek deep navy-black
            .show(ctx, |ui| {
                ui.add_space(24.0);
                
                // App Logo/Header in a premium container
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(20, 26, 38))
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                        .show(ui, |ui| {
                            ui.set_width(180.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("💾 Backup Agent")
                                        .color(egui::Color32::from_rgb(255, 255, 255))
                                        .size(17.0)
                                        .strong(),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("Automated SQL Backups")
                                        .color(egui::Color32::from_rgb(148, 163, 184))
                                        .size(10.0),
                                );
                            });
                        });
                });
                
                ui.add_space(32.0);

                // Navigation options
                ui.vertical(|ui| {
                    ui.style_mut().spacing.item_spacing.y = 12.0;

                    let mut render_nav_button = |ui: &mut egui::Ui, label: &str, view: ActiveView| {
                        let is_active = self.active_view == view;
                        
                        ui.horizontal(|ui| {
                            // Active tab indicator line on the very left
                            if is_active {
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(4.0, 28.0), egui::Sense::hover());
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    egui::Color32::from_rgb(99, 102, 241) // Electric Indigo accent
                                );
                                ui.add_space(6.0);
                            } else {
                                ui.add_space(10.0); // Balanced alignment
                            }

                            let text_color = if is_active {
                                egui::Color32::from_rgb(255, 255, 255)
                            } else {
                                egui::Color32::from_rgb(148, 163, 184) // Slate-400
                            };

                            let fill_color = if is_active {
                                egui::Color32::from_rgb(30, 41, 59) // Slate-800
                            } else {
                                egui::Color32::TRANSPARENT
                            };

                            let btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .color(text_color)
                                    .size(13.5)
                                    .strong(),
                            )
                            .fill(fill_color)
                            .min_size(egui::vec2(180.0, 36.0))
                            .rounding(6.0);

                            if ui.add(btn).clicked() {
                                self.active_view = view;
                                // Request immediate update on click
                                self.last_update = Instant::now() - std::time::Duration::from_secs(10);
                            }
                        });
                    };

                    render_nav_button(ui, "  📊   Dashboard", ActiveView::Dashboard);
                    render_nav_button(ui, "  ⚙   Settings", ActiveView::Settings);
                    render_nav_button(ui, "  ⏳   History", ActiveView::History);
                    render_nav_button(ui, "  📝   Logs", ActiveView::Logs);
                });
            });

        // Main central panel showing selected view contents
        egui::CentralPanel::default()
            .frame(egui::Frame::none()
                .fill(egui::Color32::from_rgb(11, 15, 25)) // Slate-950 equivalent
                .inner_margin(egui::Margin::same(24.0))
            )
            .show(ctx, |ui| {
                if let Some(ref err) = self.error_message {
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        ui.style_mut().visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(45, 15, 20);
                        ui.label(
                            egui::RichText::new(format!("⚠️ IPC Error: {}", err))
                                .color(egui::Color32::from_rgb(239, 68, 68))
                                .strong()
                        );
                    });
                    ui.add_space(10.0);
                }

                match self.active_view {
                    ActiveView::Dashboard => views::dashboard::show(ui, &self.ipc_client, &self.config, &self.active_jobs),
                    ActiveView::Settings => views::settings::show(ui, &self.ipc_client, &self.config),
                    ActiveView::History => views::history::show(ui, &self.history),
                    ActiveView::Logs => views::logs::show(ui),
                }
            });

        // Keep updating UI while active backup jobs are running
        if !self.active_jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }
}

fn setup_custom_styles(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(99, 102, 241); // Electric Indigo active state
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(30, 41, 59); // Slate-800 hover
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(15, 23, 42); // Slate-900 inactive
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(226, 232, 240); // Slate-200 text
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(9, 13, 22); // Main body background
    visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_rgb(148, 163, 184); // Slate-400 labels
    visuals.window_rounding = 8.0.into();
    ctx.set_visuals(visuals);
}
