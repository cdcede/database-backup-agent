use eframe::egui;

use crate::app::Notifier;
use crate::ipc_client::IpcClientHandle;
use crate::theme;
use backup_agent_core::domain::config::AppConfig;
use backup_agent_core::domain::backup_job::BackupJob;
use backup_agent_core::domain::backup_result::{BackupResult, BackupStatus};
use backup_agent_core::ipc::messages::{IpcRequest, IpcResponse};

/// Most recent backup in `history`, by start time.
pub(crate) fn last_backup(history: &[BackupResult]) -> Option<&BackupResult> {
    history.iter().max_by_key(|r| r.started_at)
}

/// One-line outcome of a backup for the "Last Backup" card, and its color.
pub(crate) fn last_backup_summary(result: &BackupResult) -> (String, egui::Color32) {
    let time = result.started_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M");
    match result.status {
        BackupStatus::Completed => {
            let size = result.human_compressed_size().unwrap_or_else(|| result.human_size());
            (format!("✔ Success · {time} · {size}"), theme::SUCCESS)
        }
        BackupStatus::Failed => (format!("✖ Failed · {time}"), theme::DANGER),
        ref other => (format!("{other:?} · {time}"), theme::ACCENT),
    }
}

/// Narrowest a summary card may get before it moves to the next row.
const MIN_CARD_WIDTH: f32 = 200.0;
const CARD_GAP: f32 = 16.0;
const CARD_PADDING: f32 = 16.0;

/// How many summary cards fit per row in `available` width, and how wide each
/// one is so the row fills the width exactly.
pub(crate) fn card_layout(available: f32, cards: usize) -> (usize, f32) {
    let fit = ((available + CARD_GAP) / (MIN_CARD_WIDTH + CARD_GAP)).floor() as usize;
    let columns = fit.clamp(1, cards.max(1));
    let width = (available - CARD_GAP * (columns - 1) as f32) / columns as f32;
    (columns, width)
}

/// One summary card: title, main value and a smaller line under it.
struct Card {
    title: &'static str,
    value: String,
    sub: String,
    sub_color: egui::Color32,
    /// Extra detail shown on hover (e.g. why the last backup failed).
    detail: Option<String>,
}

impl Card {
    fn new(title: &'static str, value: impl Into<String>, sub: impl Into<String>) -> Self {
        Self { title, value: value.into(), sub: sub.into(), sub_color: theme::ACCENT, detail: None }
    }
}

/// Lay the cards out in rows that always fit the available width.
fn show_cards(ui: &mut egui::Ui, cards: &[Card]) {
    let (columns, width) = card_layout(ui.available_width(), cards.len());
    for (row_index, row) in cards.chunks(columns).enumerate() {
        if row_index > 0 {
            ui.add_space(CARD_GAP);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for card in row {
                draw_card(ui, card, width);
            }
        });
    }
}

/// `history` is `None` until it is fetched for the first time.
pub fn show(
    ui: &mut egui::Ui,
    client: &IpcClientHandle,
    notifier: &Notifier,
    config_opt: &Option<AppConfig>,
    active_jobs: &[BackupJob],
    history: Option<&[BackupResult]>,
    service_error: Option<&str>,
) {
    egui::ScrollArea::vertical()
        .max_width(ui.available_width() - 24.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
        ui.heading(
            egui::RichText::new("Dashboard")
                .color(theme::TEXT_STRONG)
                .size(24.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("Monitor the status of your SQL Server backups in real-time.")
                .color(theme::TEXT_SUBTLE)
                .size(13.0),
        );
        ui.add_space(20.0);

        let Some(config) = config_opt else {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                // While the service is unreachable the banner above explains why;
                // a spinner here would suggest progress that is not happening.
                let message = if service_error.is_some() {
                    "The dashboard will load once the Backup Agent service is reachable."
                } else {
                    ui.spinner();
                    ui.add_space(10.0);
                    "Connecting to Backup Agent Service..."
                };
                ui.label(
                    egui::RichText::new(message)
                        .color(theme::TEXT_SUBTLE)
                        .italics()
                );
            });
            return;
        };

        // 1. Service status summary cards, laid out to always fit the width.
        let tasks_count = config.tasks.len();
        let schedule = match tasks_count {
            0 => "No tasks configured".to_string(),
            1 => format!("Schedule: {}", config.tasks[0].schedule),
            _ => "Multiple Schedules".to_string(),
        };
        let provider = if config.storage.google_drive.enabled {
            format!("{:?} + Google Drive", config.storage.provider)
        } else {
            format!("{:?}", config.storage.provider)
        };
        let destination = if config.storage.provider == backup_agent_core::domain::config::StorageProviderType::S3 {
            config.storage.s3.as_ref().map(|s3| s3.bucket.clone()).unwrap_or_else(|| "No Bucket".to_string())
        } else {
            config.backup.local_path.to_string_lossy().to_string()
        };
        let (notify_status, notify_sub) = if config.telegram.enabled {
            ("Telegram Active", config.telegram.chat_id.as_str())
        } else {
            ("Disabled", "")
        };
        let last_backup_card = match history.map(last_backup) {
            Some(Some(last)) => {
                let (summary, color) = last_backup_summary(last);
                Card {
                    sub_color: color,
                    detail: last.error_message.clone(),
                    ..Card::new("🕑 Last Backup", last.database_name.clone(), summary)
                }
            }
            Some(None) => Card::new("🕑 Last Backup", "No backups yet", "Results appear here after the first run"),
            None => Card::new("🕑 Last Backup", "Loading...", ""),
        };

        show_cards(ui, &[
            Card::new("🖥️ Database Server", config.sql_server.host.clone(), format!("Port: {}", config.sql_server.port)),
            Card::new("⏳ Backup Schedule", format!("{tasks_count} Active Tasks"), schedule),
            Card::new("📦 Storage Destination", provider, destination),
            Card::new("🔔 Notifications", notify_status, notify_sub),
            last_backup_card,
        ]);

        ui.add_space(32.0);

        // 2. Active Jobs
        ui.label(
            egui::RichText::new("Active Operations")
                .color(theme::TEXT_STRONG)
                .size(18.0)
                .strong(),
        );
        ui.add_space(8.0);

        if active_jobs.is_empty() {
            egui::Frame::none()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::RAISED))
                .rounding(6.0)
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("No backup jobs are currently running.")
                                .color(theme::TEXT_MUTED)
                                .italics()
                        );
                    });
                });
        } else {
            for job in active_jobs {
                egui::Frame::none()
                    .fill(theme::RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .rounding(6.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 24.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("Backup of database '{}'", job.database_name))
                                        .color(theme::TEXT_STRONG)
                                        .strong()
                                );
                                ui.label(
                                    egui::RichText::new(format!("Status: {:?}", job.status))
                                        .color(theme::ACCENT) // Electric Indigo
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
                .color(theme::TEXT_STRONG)
                .size(18.0)
                .strong(),
        );
        ui.add_space(8.0);

        egui::Frame::none()
            .fill(theme::SURFACE)
            .stroke(egui::Stroke::new(1.0, theme::RAISED))
            .rounding(8.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 24.0);
                // Wraps to a new line when the window is too narrow for one row.
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Select Database: ")
                            .color(theme::TEXT_BODY)
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
                                .color(theme::TEXT_MUTED)
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
                            .color(theme::TEXT_STRONG)
                            .strong()
                    )
                    .fill(theme::ACCENT) // Electric Indigo
                    .min_size(egui::vec2(150.0, 32.0));

                    if is_running {
                        ui.add_enabled(false, trigger_btn);
                        ui.label(
                            egui::RichText::new("Please wait for active jobs to complete.")
                                .color(theme::DANGER)
                                .italics()
                        );
                    } else {
                        if ui.add(trigger_btn).clicked() {
                            let selected_db = db_list[selected].clone();
                            let client_clone = client.clone();
                            let notifier = notifier.clone();
                            client.spawn(async move {
                                match client_clone.send(IpcRequest::TriggerBackup {
                                    database_name: selected_db.clone(),
                                }).await {
                                    Ok(IpcResponse::Ok) => notifier.success(format!(
                                        "Backup of '{selected_db}' started. Check History for the result."
                                    )),
                                    Ok(IpcResponse::Error(e)) => notifier.error(format!(
                                        "Could not start backup of '{selected_db}': {e}"
                                    )),
                                    Ok(_) => notifier.error("Unexpected response from the service."),
                                    Err(e) => notifier.error(format!(
                                        "Could not start backup of '{selected_db}': {e}"
                                    )),
                                }
                            });
                        }
                    }
                });
            });
        });
    });
}

fn draw_card(ui: &mut egui::Ui, card: &Card, width: f32) {
    let response = egui::Frame::none()
        .fill(theme::RAISED) // Slate-800
        .stroke(egui::Stroke::new(1.0, theme::BORDER)) // Slate-700
        .rounding(8.0)
        .inner_margin(CARD_PADDING)
        .show(ui, |ui| {
            ui.set_width(width - 2.0 * CARD_PADDING);
            ui.set_height(105.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(card.title)
                        .color(theme::TEXT_MUTED) // Slate-400
                        .size(11.0)
                        .strong(),
                );
                ui.add_space(8.0);
                // Long values are cut with "…" to keep the card in its column;
                // the full text is in the hover tooltip.
                let value = egui::RichText::new(&card.value)
                    .color(theme::TEXT_STRONG)
                    .size(16.0)
                    .strong();
                ui.add(egui::Label::new(value).wrap_mode(egui::TextWrapMode::Truncate));
                ui.add_space(8.0);
                let sub = egui::RichText::new(&card.sub)
                    .color(card.sub_color)
                    .size(10.5)
                    .italics();
                ui.add(egui::Label::new(sub).wrap_mode(egui::TextWrapMode::Truncate));
            });
        })
        .response;

    let mut tooltip = card.value.clone();
    if !card.sub.is_empty() {
        tooltip.push('\n');
        tooltip.push_str(&card.sub);
    }
    if let Some(detail) = &card.detail {
        tooltip.push_str("\n\n");
        tooltip.push_str(detail);
    }
    response.on_hover_text(tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;
    use backup_agent_core::domain::backup_result::BackupStatus;

    fn result(db: &str, status: BackupStatus, started_offset_mins: i64) -> BackupResult {
        let mut job = BackupJob::new(db);
        job.set_status(status);
        let mut r = BackupResult::from_completed_job(&job, 2048, Some(1024), "local:D:/backups");
        r.started_at = r.started_at + chrono::Duration::minutes(started_offset_mins);
        r
    }

    #[test]
    fn last_backup_is_the_most_recent_by_start_time() {
        // History is appended in completion order, but do not rely on it.
        let history = vec![
            result("OrdersDB", BackupStatus::Completed, 10),
            result("Payroll", BackupStatus::Failed, 30),
            result("Archive", BackupStatus::Completed, 20),
        ];
        let last = last_backup(&history).unwrap();
        assert_eq!(last.database_name, "Payroll");
        assert_eq!(last.status, BackupStatus::Failed);
    }

    #[test]
    fn card_layout_fits_as_many_columns_as_the_width_allows() {
        // Wide window: every card in one row, sharing the width.
        let (columns, width) = card_layout(1200.0, 5);
        assert_eq!(columns, 5);
        assert!((width * 5.0 + CARD_GAP * 4.0 - 1200.0).abs() < 0.01);

        // Typical window: fewer columns, never narrower than the minimum.
        let (columns, width) = card_layout(500.0, 5);
        assert_eq!(columns, 2);
        assert!(width >= MIN_CARD_WIDTH);
        assert!(width * 2.0 + CARD_GAP <= 500.0 + 0.01);
    }

    #[test]
    fn card_layout_never_returns_zero_columns_or_more_than_the_cards() {
        let (columns, width) = card_layout(100.0, 5);
        assert_eq!(columns, 1);
        assert_eq!(width, 100.0);

        assert_eq!(card_layout(2000.0, 3).0, 3);
    }

    #[test]
    fn last_backup_of_empty_history_is_none() {
        assert!(last_backup(&[]).is_none());
    }

    #[test]
    fn last_backup_summary_shows_outcome_time_and_size() {
        let ok = result("OrdersDB", BackupStatus::Completed, 0);
        let (text, color) = last_backup_summary(&ok);
        assert!(text.starts_with("✔ Success · "), "{text}");
        assert!(text.ends_with(" · 1.0 KB"), "{text}");
        assert_eq!(color, theme::SUCCESS);

        let failed = result("OrdersDB", BackupStatus::Failed, 0);
        let (text, color) = last_backup_summary(&failed);
        assert!(text.starts_with("✖ Failed · "), "{text}");
        assert_eq!(color, theme::DANGER);
    }
}
