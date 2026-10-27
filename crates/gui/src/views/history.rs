use eframe::egui;

use crate::theme;
use backup_agent_core::domain::backup_result::{BackupResult, BackupStatus};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum StatusFilter {
    #[default]
    All,
    Success,
    Failed,
}

/// What the user narrowed the history table down to.
#[derive(Clone, Default, Debug)]
pub(crate) struct HistoryFilter {
    pub status: StatusFilter,
    /// `None` shows every database.
    pub database: Option<String>,
    /// Case-insensitive text matched against database, destination and error.
    pub query: String,
}

impl HistoryFilter {
    pub(crate) fn matches(&self, item: &BackupResult) -> bool {
        let status_ok = match self.status {
            StatusFilter::All => true,
            StatusFilter::Success => item.status == BackupStatus::Completed,
            StatusFilter::Failed => item.status == BackupStatus::Failed,
        };
        let database_ok = self.database.as_deref().map_or(true, |db| item.database_name == db);

        let query = self.query.trim().to_lowercase();
        let query_ok = query.is_empty()
            || [
                Some(item.database_name.as_str()),
                Some(item.storage_destination.as_str()),
                item.error_message.as_deref(),
            ]
            .iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(&query));

        status_ok && database_ok && query_ok
    }

    fn is_active(&self) -> bool {
        self.status != StatusFilter::All || self.database.is_some() || !self.query.trim().is_empty()
    }
}

fn show_filter_bar(ui: &mut egui::Ui, filter: &mut HistoryFilter, history: &[BackupResult]) {
    let mut databases: Vec<&str> = history.iter().map(|r| r.database_name.as_str()).collect();
    databases.sort_unstable();
    databases.dedup();

    ui.horizontal_wrapped(|ui| {
        ui.label("Status:");
        ui.selectable_value(&mut filter.status, StatusFilter::All, "All");
        ui.selectable_value(&mut filter.status, StatusFilter::Success, "Success");
        ui.selectable_value(&mut filter.status, StatusFilter::Failed, "Failed");
        ui.add_space(12.0);

        ui.label("Database:");
        egui::ComboBox::from_id_salt("history.filter.database")
            .selected_text(filter.database.as_deref().unwrap_or("All databases"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.database, None, "All databases");
                for db in &databases {
                    ui.selectable_value(&mut filter.database, Some(db.to_string()), *db);
                }
            });
        ui.add_space(12.0);

        ui.add(
            egui::TextEdit::singleline(&mut filter.query)
                .hint_text("🔍 Search database, destination or error")
                .desired_width(240.0),
        );

        if filter.is_active() && ui.button("✖ Clear filters").clicked() {
            *filter = HistoryFilter::default();
        }
    });
}

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

        let filter_id = egui::Id::new("history.filter");
        let mut filter: HistoryFilter = ui.ctx().data(|d| d.get_temp(filter_id)).unwrap_or_default();
        show_filter_bar(ui, &mut filter, history);
        ui.ctx().data_mut(|d| d.insert_temp(filter_id, filter.clone()));

        let shown = history.iter().filter(|i| filter.matches(i)).count();
        if filter.is_active() {
            ui.label(
                egui::RichText::new(format!("Showing {shown} of {} backups", history.len()))
                    .color(theme::TEXT_MUTED)
                    .size(11.5),
            );
        }
        ui.add_space(8.0);

        if shown == 0 {
            egui::Frame::none()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::RAISED))
                .rounding(6.0)
                .inner_margin(20.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("No backups match the current filters.")
                                .color(theme::TEXT_MUTED)
                                .italics(),
                        );
                    });
                });
            return;
        }

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
                        for item in history.iter().rev().filter(|i| filter.matches(i)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use backup_agent_core::domain::backup_job::BackupJob;

    fn result(db: &str, status: BackupStatus, destination: &str, error: Option<&str>) -> BackupResult {
        let mut job = BackupJob::new(db);
        job.set_status(status);
        job.error_message = error.map(str::to_string);
        BackupResult::from_completed_job(&job, 1024, None, destination)
    }

    fn sample() -> Vec<BackupResult> {
        vec![
            result("OrdersDB", BackupStatus::Completed, "local:D:/backups/OrdersDB_1.zip", None),
            result("OrdersDB", BackupStatus::Failed, "", Some("Login failed for user 'sa'")),
            result("Payroll", BackupStatus::Completed, "local:D:/backups/Payroll_1.zip", None),
        ]
    }

    fn names(filter: &HistoryFilter) -> Vec<(String, BackupStatus)> {
        sample()
            .into_iter()
            .filter(|r| filter.matches(r))
            .map(|r| (r.database_name, r.status))
            .collect()
    }

    #[test]
    fn default_filter_shows_everything() {
        assert_eq!(names(&HistoryFilter::default()).len(), 3);
        assert!(!HistoryFilter::default().is_active());
    }

    #[test]
    fn status_filter_keeps_only_that_outcome() {
        let failed = HistoryFilter { status: StatusFilter::Failed, ..Default::default() };
        assert_eq!(names(&failed), vec![("OrdersDB".to_string(), BackupStatus::Failed)]);

        let success = HistoryFilter { status: StatusFilter::Success, ..Default::default() };
        assert_eq!(names(&success).len(), 2);
        assert!(names(&success).iter().all(|(_, s)| *s == BackupStatus::Completed));
    }

    #[test]
    fn database_filter_keeps_only_that_database() {
        let filter = HistoryFilter { database: Some("Payroll".into()), ..Default::default() };
        assert_eq!(names(&filter), vec![("Payroll".to_string(), BackupStatus::Completed)]);
    }

    #[test]
    fn text_search_is_case_insensitive_and_covers_error_and_destination() {
        let by_error = HistoryFilter { query: "LOGIN FAILED".into(), ..Default::default() };
        assert_eq!(names(&by_error), vec![("OrdersDB".to_string(), BackupStatus::Failed)]);

        let by_destination = HistoryFilter { query: "payroll_1".into(), ..Default::default() };
        assert_eq!(names(&by_destination), vec![("Payroll".to_string(), BackupStatus::Completed)]);

        let blank = HistoryFilter { query: "   ".into(), ..Default::default() };
        assert_eq!(names(&blank).len(), 3);
    }

    #[test]
    fn filters_combine() {
        let filter = HistoryFilter {
            status: StatusFilter::Success,
            database: Some("OrdersDB".into()),
            query: String::new(),
        };
        assert_eq!(names(&filter), vec![("OrdersDB".to_string(), BackupStatus::Completed)]);
        assert!(filter.is_active());
    }
}
