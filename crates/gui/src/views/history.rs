use eframe::egui;
use backup_agent_core::domain::backup_result::{BackupResult, BackupStatus};

fn draw_badge(ui: &mut egui::Ui, text: &str, text_color: egui::Color32, bg_color: egui::Color32) {
    egui::Frame::none()
        .fill(bg_color)
        .rounding(10.0) // pill shape
        .inner_margin(egui::Margin::symmetric(10.0, 3.0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .color(text_color)
                    .size(11.0)
                    .strong()
            );
        });
}

pub fn show(ui: &mut egui::Ui, history: &[BackupResult]) {
    ui.vertical(|ui| {
        ui.heading(
            egui::RichText::new("Backup History")
                .color(egui::Color32::from_rgb(255, 255, 255))
                .size(24.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("Outcome of past automated and manual backup executions.")
                .color(egui::Color32::from_rgb(140, 155, 175))
                .size(13.0),
        );
        ui.add_space(20.0);

        if history.is_empty() {
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(20, 26, 38))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 41, 59)))
                .rounding(6.0)
                .inner_margin(20.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("No historical backup records found.")
                                .color(egui::Color32::from_rgb(148, 163, 184))
                                .italics(),
                        );
                    });
                });
            return;
        }

        // Scrollable table area
        egui::ScrollArea::vertical()
            .max_width(ui.available_width() - 24.0)
            .show(ui, |ui| {
                // Table header
                egui::Grid::new("history_table_grid")
                    .num_columns(7)
                    .spacing([16.0, 12.0])
                    .min_col_width(80.0)
                    .striped(true)
                    .show(ui, |ui| {
                        // Header columns
                        ui.label(egui::RichText::new("Start Time").strong().color(egui::Color32::from_rgb(255, 255, 255)));
                        ui.label(egui::RichText::new("Database").strong().color(egui::Color32::from_rgb(255, 255, 255)));
                        ui.label(egui::RichText::new("Status").strong().color(egui::Color32::from_rgb(255, 255, 255)));
                        ui.label(egui::RichText::new("Size (Raw)").strong().color(egui::Color32::from_rgb(255, 255, 255)));
                        ui.label(egui::RichText::new("Size (Zip)").strong().color(egui::Color32::from_rgb(255, 255, 255)));
                        ui.label(egui::RichText::new("Duration").strong().color(egui::Color32::from_rgb(255, 255, 255)));
                        ui.label(egui::RichText::new("Destination").strong().color(egui::Color32::from_rgb(255, 255, 255)));
                        ui.end_row();

                        // Render history items in reverse order (newest first)
                        for item in history.iter().rev() {
                            // Date
                            let local_time = item.started_at.with_timezone(&chrono::Local);
                            ui.label(local_time.format("%Y-%m-%d %H:%M:%S").to_string());
                            
                            // Database
                            ui.label(&item.database_name);
                            
                            // Status
                            match item.status {
                                BackupStatus::Completed => {
                                    draw_badge(
                                        ui,
                                        "Success",
                                        egui::Color32::from_rgb(187, 247, 208), // Light green
                                        egui::Color32::from_rgb(20, 83, 45) // Dark green
                                    );
                                }
                                BackupStatus::Failed => {
                                    let err_msg = item.error_message.as_deref().unwrap_or("Unknown error");
                                    ui.horizontal(|ui| {
                                        draw_badge(
                                            ui,
                                            "Failed",
                                            egui::Color32::from_rgb(254, 202, 202), // Light red
                                            egui::Color32::from_rgb(127, 29, 29) // Dark red
                                        );
                                    }).response.on_hover_text(err_msg);
                                }
                                _ => {
                                    draw_badge(
                                        ui,
                                        &format!("{:?}", item.status),
                                        egui::Color32::from_rgb(254, 243, 199), // Light yellow
                                        egui::Color32::from_rgb(120, 53, 4) // Dark amber
                                    );
                                }
                            }

                            // Raw Size
                            ui.label(item.human_size());

                            // Compressed Size
                            if let Some(size) = item.human_compressed_size() {
                                ui.label(size);
                            } else {
                                ui.label("-");
                            }

                            // Duration
                            ui.label(format!("{}s", item.duration_secs));

                            // Destination
                            ui.label(&item.storage_destination).on_hover_text(&item.storage_destination);
                            
                            ui.end_row();
                        }
                    });
            });
    });
}
