use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use eframe::egui;

use crate::ipc_client::IpcClientHandle;
use crate::theme;
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
    Toast(ToastKind, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

/// How often the Dashboard refreshes history for its "Last Backup" card.
/// History can hold hundreds of entries, so it is not fetched on every poll.
const DASHBOARD_HISTORY_INTERVAL: Duration = Duration::from_secs(15);

/// Whether this poll should also fetch history. `since_last` is the time since
/// the last history fetch, `None` when it was never fetched or must be refreshed.
fn history_due(view: ActiveView, since_last: Option<Duration>) -> bool {
    match view {
        ActiveView::History => true,
        ActiveView::Dashboard => since_last.map_or(true, |e| e >= DASHBOARD_HISTORY_INTERVAL),
        ActiveView::Settings | ActiveView::Logs => false,
    }
}

/// How long a toast stays on screen.
const TOAST_DURATION: Duration = Duration::from_secs(5);

struct Toast {
    kind: ToastKind,
    text: String,
    created: Instant,
}

/// Lets views (and the async work they spawn) report the outcome of a user
/// action as a toast, and wake the UI so it shows up immediately.
#[derive(Clone)]
pub struct Notifier {
    tx: Sender<AppUpdateEvent>,
    ctx: egui::Context,
}

impl Notifier {
    pub fn success(&self, text: impl Into<String>) {
        self.notify(ToastKind::Success, text.into());
    }

    pub fn error(&self, text: impl Into<String>) {
        self.notify(ToastKind::Error, text.into());
    }

    /// Wake the UI after async work changed state it renders.
    pub fn repaint(&self) {
        self.ctx.request_repaint();
    }

    fn notify(&self, kind: ToastKind, text: String) {
        let _ = self.tx.send(AppUpdateEvent::Toast(kind, text));
        self.ctx.request_repaint();
    }
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
    /// `None` until history is fetched for the first time.
    pub history: Option<Vec<BackupResult>>,
    pub error_message: Option<String>,
    toasts: Vec<Toast>,
    notifier: Notifier,

    // Timer
    last_update: Instant,
    /// When history was last requested; `None` forces a fetch on the next poll.
    last_history_fetch: Option<Instant>,
    /// Set while a status poll is running, so slow responses never pile up.
    poll_in_flight: Arc<AtomicBool>,
}

impl BackupAgentApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_custom_styles(&cc.egui_ctx);

        let (update_tx, update_rx) = channel();
        let ipc_client = IpcClientHandle::new();
        let notifier = Notifier {
            tx: update_tx.clone(),
            ctx: cc.egui_ctx.clone(),
        };

        Self {
            active_view: ActiveView::Dashboard,
            ipc_client,
            update_tx,
            update_rx,
            config: None,
            active_jobs: Vec::new(),
            history: None,
            error_message: None,
            toasts: Vec::new(),
            notifier,
            // Initialize last_update to far in the past to trigger instant load
            last_update: Instant::now() - Duration::from_secs(10),
            last_history_fetch: None,
            poll_in_flight: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Make the next frame poll the service right away.
    fn refresh_now(&mut self) {
        self.last_update = Instant::now() - Duration::from_secs(10);
    }

    /// Fetch config and active jobs (plus history when it is on screen).
    /// At most one poll runs at a time, so a slow service never piles up requests.
    fn poll_service(&mut self, ctx: &egui::Context) {
        // Without config, poll right away, unless the service just failed: then
        // wait the normal interval, or the poll/repaint cycle spins at frame rate.
        let poll_due = (self.config.is_none() && self.error_message.is_none())
            || self.last_update.elapsed() > Duration::from_secs(2);
        if !poll_due || self.poll_in_flight.swap(true, Ordering::SeqCst) {
            return;
        }

        self.last_update = Instant::now();
        let client = self.ipc_client.clone();
        let tx = self.update_tx.clone();
        let in_flight = self.poll_in_flight.clone();
        let repaint_ctx = ctx.clone();
        // History can hold hundreds of entries; only fetch it when it is on screen.
        let fetch_history = history_due(self.active_view, self.last_history_fetch.map(|t| t.elapsed()));
        if fetch_history {
            self.last_history_fetch = Some(Instant::now());
        }

        self.ipc_client.spawn(async move {
            let reachable = match client.send(IpcRequest::GetConfig).await {
                Ok(IpcResponse::Config(cfg)) => {
                    let _ = tx.send(AppUpdateEvent::ConfigFetched(cfg));
                    true
                }
                Err(e) => {
                    let _ = tx.send(AppUpdateEvent::IpcError(e));
                    false
                }
                _ => true,
            };

            // Skip the remaining requests while the service is down.
            if reachable {
                match client.send(IpcRequest::GetStatus).await {
                    Ok(IpcResponse::Status(jobs)) => {
                        let _ = tx.send(AppUpdateEvent::StatusFetched(jobs));
                    }
                    Err(e) => {
                        let _ = tx.send(AppUpdateEvent::IpcError(e));
                    }
                    _ => {}
                }

                if fetch_history {
                    match client.send(IpcRequest::GetHistory).await {
                        Ok(IpcResponse::History(hist)) => {
                            let _ = tx.send(AppUpdateEvent::HistoryFetched(hist));
                        }
                        Err(e) => {
                            let _ = tx.send(AppUpdateEvent::IpcError(e));
                        }
                        _ => {}
                    }
                }
            }

            in_flight.store(false, Ordering::SeqCst);
            repaint_ctx.request_repaint();
        });
    }

    fn show_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.created.elapsed() < TOAST_DURATION);
        if self.toasts.is_empty() {
            return;
        }

        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for toast in &self.toasts {
                    let (fill, color, icon) = match toast.kind {
                        ToastKind::Success => (theme::SUCCESS_SURFACE, theme::SUCCESS, "✔"),
                        ToastKind::Error => (theme::DANGER_SURFACE, theme::DANGER, "✖"),
                    };
                    egui::Frame::none()
                        .fill(fill)
                        .stroke(egui::Stroke::new(1.0, color))
                        .rounding(6.0)
                        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                        .show(ui, |ui| {
                            ui.set_max_width(360.0);
                            ui.label(egui::RichText::new(format!("{icon} {}", toast.text)).color(color).strong());
                        });
                    ui.add_space(8.0);
                }
            });

        // Repaint again once the oldest toast expires so it disappears on time.
        if let Some(oldest) = self.toasts.iter().map(|t| t.created).min() {
            ctx.request_repaint_after(TOAST_DURATION.saturating_sub(oldest.elapsed()));
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
                    // A job just finished: show its result without waiting for the interval.
                    if !self.active_jobs.is_empty() && jobs.is_empty() {
                        self.last_history_fetch = None;
                    }
                    self.active_jobs = jobs;
                    self.error_message = None;
                }
                AppUpdateEvent::HistoryFetched(hist) => {
                    self.history = Some(hist);
                    self.error_message = None;
                }
                AppUpdateEvent::IpcError(e) => {
                    self.error_message = Some(e);
                    // The history request was skipped or failed; retry it on reconnect.
                    self.last_history_fetch = None;
                }
                AppUpdateEvent::Toast(kind, text) => {
                    self.toasts.push(Toast { kind, text, created: Instant::now() });
                }
            }
        }

        // 2. Query status periodically (every 2 seconds)
        self.poll_service(ctx);

        // Left side navigation bar
        egui::SidePanel::left("sidebar")
            .resizable(false)
            .default_width(220.0)
            .frame(egui::Frame::none().fill(theme::SIDEBAR_BG)) // Sleek deep navy-black
            .show(ctx, |ui| {
                ui.add_space(24.0);

                // App Logo/Header in a premium container
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    egui::Frame::none()
                        .fill(theme::SURFACE)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                        .show(ui, |ui| {
                            ui.set_width(180.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("💾 Backup Agent")
                                        .color(theme::TEXT_STRONG)
                                        .size(17.0)
                                        .strong(),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("Automated SQL Backups")
                                        .color(theme::TEXT_MUTED)
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
                                    theme::ACCENT // Electric Indigo accent
                                );
                                ui.add_space(6.0);
                            } else {
                                ui.add_space(10.0); // Balanced alignment
                            }

                            let text_color = if is_active {
                                theme::TEXT_STRONG
                            } else {
                                theme::TEXT_MUTED // Slate-400
                            };

                            let fill_color = if is_active {
                                theme::RAISED // Slate-800
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
                                self.last_update = Instant::now() - Duration::from_secs(10);
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
                .fill(theme::APP_BG) // Slate-950 equivalent
                .inner_margin(egui::Margin::same(24.0))
            )
            .show(ctx, |ui| {
                let mut retry_clicked = false;
                if let Some(ref err) = self.error_message {
                    egui::Frame::none()
                        .fill(theme::DANGER_SURFACE)
                        .stroke(egui::Stroke::new(1.0, theme::DANGER_BORDER))
                        .rounding(6.0)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let retry_width = 90.0;
                                ui.vertical(|ui| {
                                    ui.set_max_width(ui.available_width() - retry_width);
                                    ui.label(
                                        egui::RichText::new("⚠ Backup Agent service unavailable")
                                            .color(theme::DANGER)
                                            .strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(err)
                                            .color(theme::DANGER_SOFT)
                                            .size(11.5),
                                    );
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if self.poll_in_flight.load(Ordering::SeqCst) {
                                        ui.spinner();
                                    } else if ui.button("🔄 Retry").clicked() {
                                        retry_clicked = true;
                                    }
                                });
                            });
                        });
                    ui.add_space(10.0);
                }

                let service_error = self.error_message.as_deref();
                match self.active_view {
                    ActiveView::Dashboard => views::dashboard::show(ui, &self.ipc_client, &self.notifier, &self.config, &self.active_jobs, self.history.as_deref(), service_error),
                    ActiveView::Settings => views::settings::show(ui, &self.ipc_client, &self.notifier, &self.config, service_error),
                    ActiveView::History => views::history::show(ui, self.history.as_deref(), service_error),
                    ActiveView::Logs => views::logs::show(ui, &self.notifier),
                }

                if retry_clicked {
                    self.refresh_now();
                }
            });

        self.show_toasts(ctx);

        // Keep updating UI while active backup jobs are running
        if !self.active_jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        } else {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
    }
}

fn setup_custom_styles(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.widgets.active.bg_fill = theme::ACCENT; // Electric Indigo active state
    visuals.widgets.hovered.bg_fill = theme::RAISED; // Slate-800 hover
    visuals.widgets.inactive.bg_fill = theme::SUNKEN; // Slate-900 inactive
    visuals.widgets.inactive.fg_stroke.color = theme::TEXT_BODY; // Slate-200 text
    visuals.widgets.noninteractive.bg_fill = theme::SIDEBAR_BG; // Main body background
    visuals.widgets.noninteractive.fg_stroke.color = theme::TEXT_MUTED; // Slate-400 labels
    visuals.window_rounding = 8.0.into();
    ctx.set_visuals(visuals);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_view_always_fetches_history() {
        assert!(history_due(ActiveView::History, Some(Duration::ZERO)));
        assert!(history_due(ActiveView::History, None));
    }

    #[test]
    fn dashboard_fetches_history_first_time_then_at_its_own_interval() {
        assert!(history_due(ActiveView::Dashboard, None));
        assert!(!history_due(ActiveView::Dashboard, Some(Duration::from_secs(3))));
        assert!(history_due(ActiveView::Dashboard, Some(DASHBOARD_HISTORY_INTERVAL)));
    }

    #[test]
    fn other_views_never_fetch_history() {
        assert!(!history_due(ActiveView::Settings, None));
        assert!(!history_due(ActiveView::Logs, Some(Duration::from_secs(600))));
    }
}
