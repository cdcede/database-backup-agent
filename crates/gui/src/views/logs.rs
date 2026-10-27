use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use eframe::egui;

use crate::theme;

use crate::app::Notifier;

/// Number of log lines shown in the view.
const MAX_LINES: usize = 150;

/// Only the end of the log is read: enough for `MAX_LINES` of typical length,
/// without loading a log that can grow to 10 MiB before rotation.
const TAIL_BYTES: u64 = 256 * 1024;

/// Last lines read from disk, reloaded only when the file changes.
#[derive(Clone, Default)]
struct LogCache {
    /// (length, modified time) of the file when `lines` was read.
    stamp: Option<(u64, Option<SystemTime>)>,
    lines: Arc<Vec<String>>,
    error: Option<String>,
}

/// Read the last `max_lines` lines of `path`, reading at most `TAIL_BYTES`.
pub(crate) fn read_log_tail(path: &Path, max_lines: usize) -> std::io::Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);

    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        // The read most likely began mid-line; drop that fragment.
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(max_lines);
    Ok(lines[skip..].iter().map(|l| l.to_string()).collect())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// Level of a log line, `None` for lines without one (e.g. continuations).
///
/// The level is the first whitespace-separated word that names a level
/// (`2026-10-27T02:00:00Z  INFO message`), so words inside the message, such
/// as `status=ERROR`, do not change it.
pub(crate) fn line_level(line: &str) -> Option<LogLevel> {
    line.split_whitespace().find_map(|word| match word {
        "ERROR" | "FATAL" => Some(LogLevel::Error),
        "WARN" | "WARNING" => Some(LogLevel::Warn),
        "INFO" => Some(LogLevel::Info),
        "DEBUG" | "TRACE" => Some(LogLevel::Debug),
        _ => None,
    })
}

/// What the user narrowed the log view down to.
#[derive(Clone, Default, Debug)]
pub(crate) struct LogFilter {
    /// Show only lines at this level or above; `None` shows every line.
    pub min_level: Option<LogLevel>,
    /// Case-insensitive text the line must contain.
    pub query: String,
}

impl LogFilter {
    pub(crate) fn matches(&self, line: &str) -> bool {
        let level_ok = match self.min_level {
            None => true,
            // Lines without a level (continuations) are hidden by a level filter.
            Some(min) => line_level(line).map_or(false, |level| level >= min),
        };
        let query = self.query.trim().to_lowercase();
        level_ok && (query.is_empty() || line.to_lowercase().contains(&query))
    }

    fn is_active(&self) -> bool {
        self.min_level.is_some() || !self.query.trim().is_empty()
    }
}

fn show_filter_bar(ui: &mut egui::Ui, filter: &mut LogFilter) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Level:");
        ui.selectable_value(&mut filter.min_level, None, "All");
        ui.selectable_value(&mut filter.min_level, Some(LogLevel::Info), "Info+");
        ui.selectable_value(&mut filter.min_level, Some(LogLevel::Warn), "Warnings+");
        ui.selectable_value(&mut filter.min_level, Some(LogLevel::Error), "Errors");
        ui.add_space(12.0);

        ui.add(
            egui::TextEdit::singleline(&mut filter.query)
                .hint_text("🔍 Search log lines")
                .desired_width(240.0),
        );

        if filter.is_active() && ui.button("✖ Clear filters").clicked() {
            *filter = LogFilter::default();
        }
    });
}

fn file_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

pub fn show(ui: &mut egui::Ui, notifier: &Notifier) {
    let cache_id = egui::Id::new("logs.cache");
    let confirm_id = egui::Id::new("logs.confirm_clear");

    ui.vertical(|ui| {
        ui.heading(
            egui::RichText::new("Service Logs")
                .color(theme::TEXT_STRONG)
                .size(24.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("Inspect recent output logs from the background daemon.")
                .color(theme::TEXT_SUBTLE)
                .size(13.0),
        );
        ui.add_space(20.0);

        // Resolve log path
        let exe_dir = match std::env::current_exe() {
            Ok(p) => p.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            Err(_) => std::path::PathBuf::default(),
        };
        let log_path = exe_dir.join("backup-agent.log");

        if log_path.exists() {
            let mut force_reload = false;

            ui.horizontal(|ui| {
                let clear_btn = egui::Button::new(
                    egui::RichText::new("🗑 Clear Log File")
                        .color(theme::TEXT_STRONG)
                        .strong()
                )
                .fill(theme::DANGER_BUTTON)
                .min_size(egui::vec2(120.0, 28.0));

                let refresh_btn = egui::Button::new(
                    egui::RichText::new("🔄 Refresh Logs")
                        .color(theme::TEXT_STRONG)
                        .strong()
                )
                .fill(theme::RAISED)
                .min_size(egui::vec2(120.0, 28.0));

                if ui.add(clear_btn).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(confirm_id, true));
                }
                ui.add_space(8.0);
                if ui.add(refresh_btn).clicked() {
                    force_reload = true;
                }
            });
            ui.add_space(10.0);

            // Reload only when the file changed (or on Refresh), not every frame.
            let mut cache: LogCache = ui.ctx().data(|d| d.get_temp(cache_id)).unwrap_or_default();
            let stamp = file_stamp(&log_path);
            if force_reload || cache.stamp.is_none() || cache.stamp != stamp {
                cache = match read_log_tail(&log_path, MAX_LINES) {
                    Ok(lines) => LogCache { stamp, lines: Arc::new(lines), error: None },
                    Err(e) => LogCache { stamp, lines: Arc::default(), error: Some(e.to_string()) },
                };
                ui.ctx().data_mut(|d| d.insert_temp(cache_id, cache.clone()));
            }

            if let Some(err) = &cache.error {
                ui.label(
                    egui::RichText::new(format!("Error reading log file: {err}"))
                        .color(theme::DANGER)
                );
            } else {
                let filter_id = egui::Id::new("logs.filter");
                let mut filter: LogFilter = ui.ctx().data(|d| d.get_temp(filter_id)).unwrap_or_default();
                show_filter_bar(ui, &mut filter);
                ui.ctx().data_mut(|d| d.insert_temp(filter_id, filter.clone()));

                if filter.is_active() {
                    let shown = cache.lines.iter().filter(|l| filter.matches(l)).count();
                    ui.label(
                        egui::RichText::new(format!("Showing {shown} of the last {} lines", cache.lines.len()))
                            .color(theme::TEXT_MUTED)
                            .size(11.5),
                    );
                }
                ui.add_space(8.0);

                // Use ScrollArea::both() to scroll horizontally and vertically
                egui::ScrollArea::both()
                    .max_width(ui.available_width() - 24.0)
                    .max_height(ui.available_height() - 80.0)
                    .stick_to_bottom(true) // Auto-scroll to latest logs
                    .show(ui, |ui| {
                        egui::Frame::none()
                            .fill(theme::SUNKEN) // dark slate/black terminal background
                            .stroke(egui::Stroke::new(1.0, theme::BORDER)) // slate border
                            .rounding(6.0)
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                // Set minimum width so horizontal scroll kicks in
                                ui.set_min_width(ui.available_width());
                                ui.vertical(|ui| {
                                    for line in cache.lines.iter().filter(|l| filter.matches(l)) {
                                        let color = match line_level(line) {
                                            Some(LogLevel::Error) => theme::LOG_ERROR,
                                            Some(LogLevel::Warn) => theme::LOG_WARN,
                                            Some(LogLevel::Info) => theme::LOG_INFO,
                                            Some(LogLevel::Debug) => theme::LOG_DEBUG,
                                            None => theme::LOG_TEXT,
                                        };

                                        // We use Label with wrap(false) to ensure horizontal scrolling works without wrapping
                                        let text = egui::RichText::new(line.as_str())
                                            .monospace()
                                            .color(color)
                                            .size(11.0);

                                        ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
                                    }
                                });
                            });
                    });
            }

            show_clear_confirmation(ui.ctx(), confirm_id, cache_id, &log_path, notifier);
        } else {
            ui.group(|ui| {
                ui.set_width(ui.available_width() - 24.0);
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new("No service log file found yet.")
                            .color(theme::TEXT_FAINT)
                            .italics()
                    );
                    ui.label(
                        egui::RichText::new(format!("Ensure the service is running and creating logs at:\n{}", log_path.display()))
                            .color(theme::TEXT_FAINT)
                            .size(11.0)
                    );
                    ui.add_space(20.0);
                });
            });
        }
    });
}

/// Ask before truncating the log: the content cannot be recovered.
fn show_clear_confirmation(
    ctx: &egui::Context,
    confirm_id: egui::Id,
    cache_id: egui::Id,
    log_path: &Path,
    notifier: &Notifier,
) {
    let open: bool = ctx.data(|d| d.get_temp(confirm_id)).unwrap_or(false);
    if !open {
        return;
    }

    let mut close = false;
    egui::Window::new("Clear log file?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label("This permanently deletes the contents of the service log.");
            ui.label("It cannot be undone.");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let confirm_btn = egui::Button::new(
                    egui::RichText::new("Clear Log")
                        .color(theme::TEXT_STRONG)
                        .strong()
                )
                .fill(theme::DANGER_BUTTON);

                if ui.add(confirm_btn).clicked() {
                    match std::fs::write(log_path, "") {
                        Ok(()) => notifier.success("Log file cleared."),
                        Err(e) => notifier.error(format!("Could not clear the log file: {e}")),
                    }
                    ctx.data_mut(|d| d.remove::<LogCache>(cache_id));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    if close {
        ctx.data_mut(|d| d.insert_temp(confirm_id, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("backup-agent-gui-test-{}-{name}.log", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn tail_returns_last_lines_in_order() {
        let content: String = (1..=300).map(|i| format!("line {i}\n")).collect();
        let path = temp_log("last-lines", &content);

        let lines = read_log_tail(&path, 150).unwrap();

        assert_eq!(lines.len(), 150);
        assert_eq!(lines.first().unwrap(), "line 151");
        assert_eq!(lines.last().unwrap(), "line 300");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn tail_reads_only_the_end_of_a_large_file() {
        // Far larger than the tail window: the first line must never be read.
        let filler = "x".repeat(1_000);
        let mut content = String::from("FIRST LINE\n");
        for i in 0..1_000 {
            content.push_str(&format!("{i} {filler}\n"));
        }
        content.push_str("LAST LINE\n");
        let path = temp_log("large", &content);

        let lines = read_log_tail(&path, 150).unwrap();

        assert_eq!(lines.last().unwrap(), "LAST LINE");
        assert!(lines.iter().all(|l| l != "FIRST LINE"));
        assert!(
            lines.iter().all(|l| l.starts_with(char::is_numeric) || l == "LAST LINE"),
            "no partial first line"
        );
        std::fs::remove_file(path).ok();
    }

    const LINES: [&str; 5] = [
        "2026-10-27T02:00:00 INFO backup started db=OrdersDB",
        "2026-10-27T02:00:05 WARN Drive retention failed",
        "2026-10-27T02:00:06 ERROR Login failed for user 'sa'",
        "    at mssql::connect",
        "2026-10-27T02:00:07 DEBUG pipe connected",
    ];

    fn visible(filter: &LogFilter) -> Vec<&'static str> {
        LINES.iter().copied().filter(|l| filter.matches(l)).collect()
    }

    #[test]
    fn line_level_detects_each_level() {
        assert_eq!(line_level(LINES[0]), Some(LogLevel::Info));
        assert_eq!(line_level(LINES[1]), Some(LogLevel::Warn));
        assert_eq!(line_level(LINES[2]), Some(LogLevel::Error));
        assert_eq!(line_level(LINES[3]), None);
        assert_eq!(line_level(LINES[4]), Some(LogLevel::Debug));
        assert_eq!(line_level("x FATAL y"), Some(LogLevel::Error));
    }

    #[test]
    fn line_level_uses_the_level_field_not_words_in_the_message() {
        assert_eq!(line_level("2026-10-27T02:00:00 INFO job status=ERROR"), Some(LogLevel::Info));
        assert_eq!(line_level("2026-10-27T02:00:00  WARN retry after ERROR"), Some(LogLevel::Warn));
        assert_eq!(line_level("2026-10-27T02:00:00 INFO INFORMATIONAL"), Some(LogLevel::Info));
        assert_eq!(line_level("no level here: status=ERROR"), None);
    }

    #[test]
    fn default_log_filter_shows_every_line() {
        assert_eq!(visible(&LogFilter::default()), LINES.to_vec());
        assert!(!LogFilter::default().is_active());
    }

    #[test]
    fn level_filter_shows_that_level_and_above() {
        let warn = LogFilter { min_level: Some(LogLevel::Warn), ..Default::default() };
        assert_eq!(visible(&warn), vec![LINES[1], LINES[2]]);

        let errors = LogFilter { min_level: Some(LogLevel::Error), ..Default::default() };
        assert_eq!(visible(&errors), vec![LINES[2]]);
    }

    #[test]
    fn log_text_search_is_case_insensitive_and_combines_with_level() {
        let search = LogFilter { query: "ordersdb".into(), ..Default::default() };
        assert_eq!(visible(&search), vec![LINES[0]]);

        let combined = LogFilter { min_level: Some(LogLevel::Warn), query: "LOGIN".into() };
        assert_eq!(visible(&combined), vec![LINES[2]]);
        assert!(combined.is_active());
    }

    #[test]
    fn tail_of_short_file_returns_all_lines() {
        let path = temp_log("short", "a\nb\n");
        assert_eq!(read_log_tail(&path, 150).unwrap(), vec!["a", "b"]);
        std::fs::remove_file(path).ok();
    }
}
