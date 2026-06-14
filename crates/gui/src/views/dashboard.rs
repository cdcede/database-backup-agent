use eframe::egui;

use crate::ipc_client::IpcClientHandle;
use backup_agent_core::domain::config::AppConfig;
use backup_agent_core::domain::backup_job::BackupJob;
use backup_agent_core::ipc::messages::IpcRequest;

pub fn show(
    ui: &mut egui::Ui,
    client: &IpcClientHandle,
    config_opt: &Option<AppConfig>,
    active_jobs: &[BackupJob],
) {
    egui::ScrollArea::vertical()
        .max_width(ui.available_width() - 24.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
        ui.heading(
            egui::RichText::new("Dashboard")
                .color(egui::Color32::from_rgb(255, 255, 255))
                .size(24.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("Monitor the status of your SQL Server backups in real-time.")
                .color(egui::Color32::from_rgb(140, 155, 175))
                .size(13.0),
        );
        ui.add_space(20.0);

        let Some(config) = config_opt else {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.spinner();
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("Connecting to Backup Agent Service...")
                        .color(egui::Color32::from_rgb(140, 155, 175))
                        .italics()
                );
            });
            return;
        };

        // 1. Service Status summary cards (wrapped horizontally for responsiveness)
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().spacing.item_spacing.x = 16.0;
            ui.style_mut().spacing.item_spacing.y = 16.0;

            // Database host card
            draw_card(ui, "🖥️ Database Server", &config.sql_server.host, &format!("Port: {}", config.sql_server.port));

            // Schedule card
            let tasks_count = config.tasks.len();
            let main_text = format!("{} Active Tasks", tasks_count);
            let sub_text = if tasks_count == 1 {
                format!("Schedule: {}", config.tasks[0].schedule)
            } else if tasks_count > 1 {
                format!("Multiple Schedules")
            } else {
                "No tasks configured".to_string()
            };
            draw_card(ui, "⏳ Backup Schedule", &main_text, &sub_text);

            // Storage card
            let provider_type = format!("{:?}", config.storage.provider);
            let sub_text = if config.storage.provider == backup_agent_core::domain::config::StorageProviderType::S3 {
                config.storage.s3.as_ref().map(|s3| s3.bucket.clone()).unwrap_or_else(|| "No Bucket".to_string())
            } else {
                config.backup.local_path.to_string_lossy().to_string()
            };
            draw_card(ui, "📦 Storage Destination", &provider_type, &sub_text);

            // Notifications card
            let status = if config.telegram.enabled { "Telegram Active" } else { "Disabled" };
            let sub = if config.telegram.enabled { &config.telegram.chat_id } else { "" };
            draw_card(ui, "🔔 Notifications", status, sub);
        });

        ui.add_space(32.0);

        // 2. Active Jobs
        ui.label(
            egui::RichText::new("Active Operations")
                .color(egui::Color32::from_rgb(255, 255, 255))
                .size(18.0)
                .strong(),
        );
        ui.add_space(8.0);

        if active_jobs.is_empty() {
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(20, 26, 38))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 41, 59)))
                .rounding(6.0)
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("No backup jobs are currently running.")
                                .color(egui::Color32::from_rgb(148, 163, 184))
                                .italics()
                        );
                    });
                });
        } else {
            for job in active_jobs {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(30, 41, 59))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 65, 85)))
                    .rounding(6.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 24.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("Backup of database '{}'", job.database_name))
                                        .color(egui::Color32::from_rgb(255, 255, 255))
                                        .strong()
                                );
                                ui.label(
                                    egui::RichText::new(format!("Status: {:?}", job.status))
                                        .color(egui::Color32::from_rgb(99, 102, 241)) // Electric Indigo
                                );
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spinner();
                            });
                        });
                    });
                ui.add_space(8.0);
            }
        }

        ui.add_space(32.0);

        // 3. Trigger manual backup
        ui.label(
            egui::RichText::new("Trigger Manual Backup")
                .color(egui::Color32::from_rgb(255, 255, 255))
                .size(18.0)
                .strong(),
        );
        ui.add_space(8.0);

        egui::Frame::none()
            .fill(egui::Color32::from_rgb(20, 26, 38))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 41, 59)))
            .rounding(8.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 24.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Select Database: ")
                            .color(egui::Color32::from_rgb(226, 232, 240))
                            .strong()
                    );
                    
                    // Let's create a simple combo box to choose database
                    let mut db_list: Vec<String> = config
                        .tasks
                        .iter()
                        .flat_map(|t| t.databases.clone())
                        .collect();
                    db_list.sort();
                    db_list.dedup();

                    if db_list.is_empty() {
                        ui.label(
                            egui::RichText::new("No databases configured.")
                                .color(egui::Color32::from_rgb(148, 163, 184))
                                .italics()
                        );
                        return;
                    }

                    let id = egui::Id::new("dashboard.manual_backup.selected_index");
                    let mut selected: usize = ui.ctx().data(|d| {
                        d.get_temp::<usize>(id).unwrap_or(0)
                    });
                    if selected >= db_list.len() {
                        selected = 0;
                    }

                    egui::ComboBox::from_id_salt("manual_backup_db_select")
                        .selected_text(&db_list[selected])
                        .show_ui(ui, |ui| {
                            for (idx, db) in db_list.iter().enumerate() {
                                ui.selectable_value(&mut selected, idx, db);
                            }
                        });

                    ui.ctx().data_mut(|d| d.insert_temp(id, selected));

                    ui.add_space(12.0);

                    let is_running = !active_jobs.is_empty();
                    let trigger_btn = egui::Button::new(
                        egui::RichText::new("🚀 Start Backup Now")
                            .color(egui::Color32::from_rgb(255, 255, 255))
                            .strong()
                    )
                    .fill(egui::Color32::from_rgb(99, 102, 241)) // Electric Indigo
                    .min_size(egui::vec2(150.0, 32.0));

                    if is_running {
                        ui.add_enabled(false, trigger_btn);
                        ui.label(
                            egui::RichText::new("Please wait for active jobs to complete.")
                                .color(egui::Color32::from_rgb(239, 68, 68))
                                .italics()
                        );
                    } else {
                        if ui.add(trigger_btn).clicked() {
                            let selected_db = db_list[selected].clone();
                            let client_clone = client.clone();
                            std::thread::spawn(move || {
                                let rt = tokio::runtime::Runtime::new().unwrap();
                                let _ = rt.block_on(async {
                                    let _ = client_clone.send(IpcRequest::TriggerBackup {
                                        database_name: selected_db,
                                    }).await;
                                });
                            });
                        }
                    }
                });
            });
        });
    });
}

fn draw_card(ui: &mut egui::Ui, title: &str, value: &str, sub: &str) {
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(30, 41, 59)) // Slate-800
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 65, 85))) // Slate-700
        .rounding(8.0)
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_width(170.0);
            ui.set_height(105.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .color(egui::Color32::from_rgb(148, 163, 184)) // Slate-400
                        .size(11.0)
                        .strong(),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(value)
                        .color(egui::Color32::from_rgb(255, 255, 255))
                        .size(16.0)
                        .strong(),
                );
                ui.add_space(8.0);
                
                // Truncate the card subtext if it's too long, preventing it from overflowing the card boundary
                let text = egui::RichText::new(sub)
                    .color(egui::Color32::from_rgb(99, 102, 241)) // Electric Indigo
                    .size(10.5)
                    .italics();
                ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Truncate));
            });
        });
}
