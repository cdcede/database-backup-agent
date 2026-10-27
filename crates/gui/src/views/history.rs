use eframe::egui;

use crate::theme;
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

/// `history` is `None` until the first fetch completes.
pub fn show(ui: &mut egui::Ui, history: Option<&[BackupResult]>, service_error: Option<&str>) {
    ui.vertical(|ui| {
        ui.heading(
            egui::RichText::new("Backup History")
                .color(theme::TEXT_STRONG)
                .size(24.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("Outcome of past automated and manual backup executions.")
                .color(theme::TEXT_SUBTLE)
                .size(13.0),
        );
        ui.add_space(20.0);

        let history = match history {
            Some(h) if !h.is_empty() => h,
            None if service_error.is_none() => {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.spinner();
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Loading backup history...")
                            .color(theme::TEXT_SUBTLE)
                            .italics(),
                    );
                });
                return;
            }
            _ => {
                // "No records" would be misleading when we simply could not ask.
                let message = if service_error.is_some() {
                    "Backup history is unavailable while the service is unreachable."
                } else {
                    "No historical backup records found."
                };
                egui::Frame::none()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::RAISED))
                    .rounding(6.0)
                    .inner_margin(20.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 24.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new(message)
                                    .color(theme::TEXT_MUTED)
                                    .italics(),
                            );
                        });
                    });
                return;
            }
        };

        // Scrollable table area (both vertical and horizontal to support smaller screens)
        egui::ScrollArea::both()
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
                        ui.label(egui::RichText::new("Start Time").strong().color(theme::TEXT_STRONG));
                        ui.label(egui::RichText::new("Database").strong().color(theme::TEXT_STRONG));
                        ui.label(egui::RichText::new("Status").strong().color(theme::TEXT_STRONG));
                        ui.label(egui::RichText::new("Size (Raw)").strong().color(theme::TEXT_STRONG));
                        ui.label(egui::RichText::new("Size (Zip)").strong().color(theme::TEXT_STRONG));
                        ui.label(egui::RichText::new("Duration").strong().color(theme::TEXT_STRONG));
                        ui.label(egui::RichText::new("Destination").strong().color(theme::TEXT_STRONG));
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
                                        theme::BADGE_SUCCESS_TEXT, // Light green
                                        theme::BADGE_SUCCESS_BG // Dark green
                                    );
                                }
                                BackupStatus::Failed => {
                                    let err_msg = item.error_message.as_deref().unwrap_or("Unknown error");
                                    ui.horizontal(|ui| {
                                        draw_badge(
                                            ui,
                                            "Failed",
                                            theme::BADGE_FAILED_TEXT, // Light red
                                            theme::BADGE_FAILED_BG
                                        );
                                    }).response.on_hover_text(err_msg);
                                }
                                _ => {
                                    draw_badge(
                                        ui,
                                        &format!("{:?}", item.status),
                                        theme::BADGE_PENDING_TEXT, // Light yellow
                                        theme::BADGE_PENDING_BG // Dark amber
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
