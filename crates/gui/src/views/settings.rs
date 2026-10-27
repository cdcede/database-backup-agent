use eframe::egui;

use crate::theme;
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::app::Notifier;
use crate::ipc_client::IpcClientHandle;
use backup_agent_core::domain::config::{AppConfig, AuthMethod, SqlServerConfig, BackupConfig, StorageConfig, StorageProviderType, S3Config, TelegramConfig, BackupTaskConfig, GoogleDriveConfig};
use backup_agent_core::ipc::messages::{IpcRequest, IpcResponse};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScheduleMode {
    Daily,
    Weekly,
    Monthly,
    Custom,
}

#[derive(Clone, PartialEq)]
pub struct TaskSettingsState {
    pub name: String,
    pub databases: Vec<String>,
    pub schedule: String,
    pub retention_days: String,
    
    // UI state for the Schedule Builder
    pub schedule_mode: ScheduleMode,
    pub daily_times: String,
    pub weekly_time: String,
    pub weekly_days: [bool; 7],
    pub monthly_day: u32,
    pub monthly_time: String,
    pub custom_cron: String,
}

impl TaskSettingsState {
    pub fn from_config_task(
        name: String,
        databases: Vec<String>,
        schedule: String,
        retention_days: String,
    ) -> Self {
        let (schedule_mode, daily_times, weekly_time, weekly_days, monthly_day, monthly_time, custom_cron) =
            parse_schedule_string(&schedule);

        Self {
            name,
            databases,
            schedule,
            retention_days,
            schedule_mode,
            daily_times,
            weekly_time,
            weekly_days,
            monthly_day,
            monthly_time,
            custom_cron,
        }
    }
}

#[derive(Clone)]
pub struct SettingsState {
    pub host: String,
    pub port: String,
    pub auth_method: String, // "sql" | "windows"
    pub username: String,
    pub password: String,
    pub local_path: String,
    pub storage_provider: String, // "local" | "s3"
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub gdrive_enabled: bool,
    pub gdrive_client_id: String,
    pub gdrive_client_secret: String,
    pub gdrive_refresh_token: String,
    pub gdrive_folder_id: String,
    pub telegram_enabled: bool,
    pub telegram_bot_token: String,
    pub telegram_chat_id: String,
    
    // Multi-task list
    pub tasks: Vec<TaskSettingsState>,
    pub selected_task_index: usize,

    // Connection testing
    pub discovered_databases: Vec<String>,
    pub connection_test_status: Option<Result<String, String>>,
    pub is_testing_connection: bool,
    
    // Feedback
    pub errors: Vec<String>,
    pub is_saving: bool,
    /// Destructive action waiting for the user to confirm it.
    pub pending_confirm: Option<ConfirmAction>,
    pub active_tab: SettingsTab,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ConfirmAction {
    DeleteTask(usize),
    ResetForm,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    SqlServer,
    Tasks,
    Storage,
    Notifications,
}

impl SettingsTab {
    const ALL: [SettingsTab; 4] = [
        SettingsTab::SqlServer,
        SettingsTab::Tasks,
        SettingsTab::Storage,
        SettingsTab::Notifications,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::SqlServer => "🖥 SQL Server",
            SettingsTab::Tasks => "📅 Tasks",
            SettingsTab::Storage => "📦 Storage",
            SettingsTab::Notifications => "🔔 Notifications",
        }
    }
}

/// A form field that can carry a validation error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Port,
    Username,
    Password,
    LocalPath,
    S3Endpoint,
    S3Bucket,
    S3Region,
    S3AccessKey,
    S3SecretKey,
    GdriveClientId,
    GdriveClientSecret,
    GdriveRefreshToken,
    TelegramBotToken,
    TelegramChatId,
    /// The task list as a whole (e.g. no task configured).
    Tasks,
    TaskName(usize),
    TaskSchedule(usize),
    TaskRetention(usize),
    TaskDatabases(usize),
}

impl Field {
    fn tab(self) -> SettingsTab {
        match self {
            Field::Port | Field::Username | Field::Password => SettingsTab::SqlServer,
            Field::Tasks
            | Field::TaskName(_)
            | Field::TaskSchedule(_)
            | Field::TaskRetention(_)
            | Field::TaskDatabases(_) => SettingsTab::Tasks,
            Field::LocalPath
            | Field::S3Endpoint
            | Field::S3Bucket
            | Field::S3Region
            | Field::S3AccessKey
            | Field::S3SecretKey
            | Field::GdriveClientId
            | Field::GdriveClientSecret
            | Field::GdriveRefreshToken => SettingsTab::Storage,
            Field::TelegramBotToken | Field::TelegramChatId => SettingsTab::Notifications,
        }
    }

    fn task_index(self) -> Option<usize> {
        match self {
            Field::TaskName(i) | Field::TaskSchedule(i) | Field::TaskRetention(i) | Field::TaskDatabases(i) => Some(i),
            _ => None,
        }
    }
}

/// Validation errors of the form, keyed by the field they belong to,
/// in form order.
#[derive(Clone, Default, Debug)]
pub struct FieldErrors(Vec<(Field, String)>);

impl FieldErrors {
    fn push(&mut self, field: Field, message: impl Into<String>) {
        self.0.push((field, message.into()));
    }

    pub fn get(&self, field: Field) -> Option<&str> {
        self.0.iter().find(|(f, _)| *f == field).map(|(_, m)| m.as_str())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn count_in(&self, tab: SettingsTab) -> usize {
        self.0.iter().filter(|(f, _)| f.tab() == tab).count()
    }

    pub fn task_has_errors(&self, index: usize) -> bool {
        self.0.iter().any(|(f, _)| f.task_index() == Some(index))
    }

    fn first(&self) -> Option<Field> {
        self.0.first().map(|(f, _)| *f)
    }
}

impl SettingsState {
    fn from_config(config: &AppConfig) -> Self {
        let tasks = config.tasks.iter().map(|t| {
            TaskSettingsState::from_config_task(
                t.name.clone(),
                t.databases.clone(),
                t.schedule.clone(),
                t.retention_days.to_string(),
            )
        }).collect::<Vec<_>>();

        Self {
            host: config.sql_server.host.clone(),
            port: config.sql_server.port.to_string(),
            auth_method: match config.sql_server.auth_method {
                AuthMethod::Sql => "sql".to_string(),
                AuthMethod::Windows => "windows".to_string(),
            },
            username: config.sql_server.username.clone().unwrap_or_default(),
            password: config.sql_server.password.clone().unwrap_or_default(),
            local_path: config.backup.local_path.to_string_lossy().to_string(),
            storage_provider: match config.storage.provider {
                StorageProviderType::Local => "local".to_string(),
                StorageProviderType::S3 => "s3".to_string(),
            },
            s3_endpoint: config.storage.s3.as_ref().map(|s3| s3.endpoint.clone()).unwrap_or_default(),
            s3_bucket: config.storage.s3.as_ref().map(|s3| s3.bucket.clone()).unwrap_or_default(),
            s3_region: config.storage.s3.as_ref().map(|s3| s3.region.clone()).unwrap_or_default(),
            s3_access_key: config.storage.s3.as_ref().map(|s3| s3.access_key.clone()).unwrap_or_default(),
            s3_secret_key: config.storage.s3.as_ref().map(|s3| s3.secret_key.clone()).unwrap_or_default(),
            gdrive_enabled: config.storage.google_drive.enabled,
            gdrive_client_id: config.storage.google_drive.client_id.clone(),
            gdrive_client_secret: config.storage.google_drive.client_secret.clone(),
            gdrive_refresh_token: config.storage.google_drive.refresh_token.clone(),
            gdrive_folder_id: config.storage.google_drive.folder_id.clone().unwrap_or_default(),
            telegram_enabled: config.telegram.enabled,
            telegram_bot_token: config.telegram.bot_token.clone(),
            telegram_chat_id: config.telegram.chat_id.clone(),
            
            tasks,
            selected_task_index: 0,
            discovered_databases: Vec::new(),
            connection_test_status: None,
            is_testing_connection: false,

            errors: Vec::new(),
            is_saving: false,
            pending_confirm: None,
            active_tab: SettingsTab::SqlServer,
        }
    }

    /// Check every field the way saving would, so errors can be shown next
    /// to the field while the user edits.
    pub fn field_errors(&self) -> FieldErrors {
        let mut errors = FieldErrors::default();
        let blank = |s: &str| s.trim().is_empty();

        if self.port.parse::<u16>().is_err() {
            errors.push(Field::Port, "Port must be a number between 0 and 65535");
        }
        if self.auth_method == "sql" {
            if blank(&self.username) {
                errors.push(Field::Username, "Required for SQL Server authentication");
            }
            if blank(&self.password) {
                errors.push(Field::Password, "Required for SQL Server authentication");
            }
        }

        if self.tasks.is_empty() {
            errors.push(Field::Tasks, "Add at least one backup task");
        }
        for (idx, t) in self.tasks.iter().enumerate() {
            if blank(&t.name) {
                errors.push(Field::TaskName(idx), "Task name is required");
            }
            if let Err(e) = compile_schedule_string(
                t.schedule_mode,
                &t.daily_times,
                &t.weekly_time,
                &t.weekly_days,
                t.monthly_day,
                &t.monthly_time,
                &t.custom_cron,
            ) {
                errors.push(Field::TaskSchedule(idx), e);
            }
            match t.retention_days.trim().parse::<u32>() {
                Err(_) => errors.push(Field::TaskRetention(idx), "Retention must be a whole number of days"),
                Ok(0) => errors.push(Field::TaskRetention(idx), "Retention must be at least 1 day"),
                Ok(_) => {}
            }
            if t.databases.is_empty() {
                errors.push(Field::TaskDatabases(idx), "Select at least one database");
            }
        }

        if blank(&self.local_path) {
            errors.push(Field::LocalPath, "Destination path is required");
        }
        if self.storage_provider == "s3" {
            for (field, value) in [
                (Field::S3Endpoint, &self.s3_endpoint),
                (Field::S3Bucket, &self.s3_bucket),
                (Field::S3Region, &self.s3_region),
                (Field::S3AccessKey, &self.s3_access_key),
                (Field::S3SecretKey, &self.s3_secret_key),
            ] {
                if blank(value) {
                    errors.push(field, "Required for S3 storage");
                }
            }
        }
        if self.gdrive_enabled {
            for (field, value) in [
                (Field::GdriveClientId, &self.gdrive_client_id),
                (Field::GdriveClientSecret, &self.gdrive_client_secret),
                (Field::GdriveRefreshToken, &self.gdrive_refresh_token),
            ] {
                if blank(value) {
                    errors.push(field, "Required when Google Drive copy is enabled");
                }
            }
        }

        if self.telegram_enabled {
            if blank(&self.telegram_bot_token) {
                errors.push(Field::TelegramBotToken, "Required when Telegram alerts are enabled");
            }
            if blank(&self.telegram_chat_id) {
                errors.push(Field::TelegramChatId, "Required when Telegram alerts are enabled");
            }
        }

        errors
    }

    /// Validate and build the config to save. On failure, records the
    /// problems for display, jumps to the first invalid field, and returns
    /// the message for the error toast.
    fn prepare_save(&mut self, base: &AppConfig) -> Result<AppConfig, String> {
        self.errors.clear();

        let field_errors = self.field_errors();
        if let Some(field) = field_errors.first() {
            self.active_tab = field.tab();
            if let Some(idx) = field.task_index() {
                self.selected_task_index = idx;
            }
            return Err(format!(
                "Settings not saved: fix the {} field(s) marked in red.",
                field_errors.len()
            ));
        }

        // The field checks mirror these; they stay as the final authority.
        let config = self.to_config(base).and_then(|config| {
            let core_errors = config.validate();
            if core_errors.is_empty() { Ok(config) } else { Err(core_errors.join("\n")) }
        });
        config.map_err(|e| {
            self.errors.extend(e.lines().map(str::to_string));
            format!(
                "Settings not saved: {} problem(s) listed at the top of the page.",
                self.errors.len()
            )
        })
    }

    /// Build the config to save. Sections the form does not edit (`service`)
    /// are carried over from `base` so saving never resets them.
    fn to_config(&self, base: &AppConfig) -> Result<AppConfig, String> {
        let port = self.port.parse::<u16>().map_err(|_| "SQL Port must be a valid number (0-65535)".to_string())?;
        
        let auth_method = match self.auth_method.as_str() {
            "sql" => AuthMethod::Sql,
            _ => AuthMethod::Windows,
        };

        let storage_provider = match self.storage_provider.as_str() {
            "s3" => StorageProviderType::S3,
            _ => StorageProviderType::Local,
        };

        let s3 = if storage_provider == StorageProviderType::S3 {
            Some(S3Config {
                endpoint: self.s3_endpoint.trim().to_string(),
                bucket: self.s3_bucket.trim().to_string(),
                region: self.s3_region.trim().to_string(),
                access_key: self.s3_access_key.trim().to_string(),
                secret_key: self.s3_secret_key.trim().to_string(),
            })
        } else {
            None
        };

        let mut tasks = Vec::new();
        for (idx, t) in self.tasks.iter().enumerate() {
            let retention_days = t.retention_days.parse::<u32>().map_err(|_| {
                format!("Task '{}': retention days must be a positive number", t.name)
            })?;
            
            if t.name.trim().is_empty() {
                return Err(format!("Task {} has an empty name", idx + 1));
            }
            if t.databases.is_empty() {
                return Err(format!("Task '{}' must have at least one database selected", t.name));
            }
            
            // Compile schedule string from builder state
            let compiled_schedule = compile_schedule_string(
                t.schedule_mode,
                &t.daily_times,
                &t.weekly_time,
                &t.weekly_days,
                t.monthly_day,
                &t.monthly_time,
                &t.custom_cron,
            ).map_err(|e| format!("Task '{}': {}", t.name, e))?;
            
            tasks.push(BackupTaskConfig {
                name: t.name.trim().to_string(),
                databases: t.databases.clone(),
                schedule: compiled_schedule,
                retention_days,
            });
        }

        let config = AppConfig {
            sql_server: SqlServerConfig {
                host: self.host.trim().to_string(),
                port,
                auth_method,
                username: if self.username.trim().is_empty() { None } else { Some(self.username.trim().to_string()) },
                password: if self.password.trim().is_empty() { None } else { Some(self.password.trim().to_string()) },
            },
            backup: BackupConfig {
                local_path: std::path::PathBuf::from(self.local_path.trim()),
            },
            tasks,
            telegram: TelegramConfig {
                enabled: self.telegram_enabled,
                bot_token: self.telegram_bot_token.trim().to_string(),
                chat_id: self.telegram_chat_id.trim().to_string(),
            },
            storage: StorageConfig {
                provider: storage_provider,
                s3,
                google_drive: GoogleDriveConfig {
                    enabled: self.gdrive_enabled,
                    client_id: self.gdrive_client_id.trim().to_string(),
                    client_secret: self.gdrive_client_secret.trim().to_string(),
                    refresh_token: self.gdrive_refresh_token.trim().to_string(),
                    folder_id: extract_drive_folder_id(&self.gdrive_folder_id),
                },
            },
            service: base.service.clone(),
        };

        Ok(config)
    }
}

/// Accept either a raw folder ID or a pasted folder URL
/// (`https://drive.google.com/drive/folders/<id>?usp=sharing`). Empty = Drive root.
fn extract_drive_folder_id(input: &str) -> Option<String> {
    let input = input.trim();
    let id = match input.split_once("/folders/") {
        Some((_, rest)) => rest.split(|c| c == '?' || c == '/' || c == '#').next().unwrap_or(""),
        None => input,
    };
    if id.is_empty() { None } else { Some(id.to_string()) }
}

static SETTINGS_STATE: OnceLock<Mutex<Option<SettingsState>>> = OnceLock::new();

fn text_input(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    let width = (ui.available_width() - 24.0).min(400.0).max(150.0);
    ui.add(egui::TextEdit::singleline(text).desired_width(width))
}

/// Input for a password, secret or token: hidden by default, with a
/// Show/Hide toggle that lasts until the app is closed.
fn secret_input(ui: &mut egui::Ui, text: &mut String, id_salt: &str) -> egui::Response {
    let id = egui::Id::new(("settings.reveal", id_salt));
    let mut revealed: bool = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(false);

    let response = ui
        .horizontal(|ui| {
            let width = (ui.available_width() - 80.0).min(400.0).max(150.0);
            let response = ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(width)
                    .password(!revealed),
            );
            let (label, hint) = if revealed {
                ("Hide", "Hide the value")
            } else {
                ("Show", "Show the value")
            };
            if ui.small_button(label).on_hover_text(hint).clicked() {
                revealed = !revealed;
            }
            response
        })
        .inner;

    ui.ctx().data_mut(|d| d.insert_temp(id, revealed));
    response
}

fn field_error(ui: &mut egui::Ui, errors: &FieldErrors, field: Field) {
    if let Some(message) = errors.get(field) {
        ui.label(
            egui::RichText::new(format!("⚠ {message}"))
                .color(theme::DANGER)
                .size(11.0),
        );
    }
}

/// One row of a form grid: label, input, and the field's error under the input.
fn form_row(
    ui: &mut egui::Ui,
    label: &str,
    errors: &FieldErrors,
    field: Option<Field>,
    add_input: impl FnOnce(&mut egui::Ui),
) {
    ui.label(label);
    ui.vertical(|ui| {
        add_input(ui);
        if let Some(field) = field {
            field_error(ui, errors, field);
        }
    });
    ui.end_row();
}

fn form_grid(ui: &mut egui::Ui, id: &str, add_rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([24.0, 14.0])
        .min_col_width(140.0)
        .show(ui, add_rows);
}

fn section_frame(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::RAISED))
        .rounding(8.0)
        .inner_margin(16.0)
        .show(ui, |ui| {
            // Same width on every tab, so switching tabs does not resize the panel.
            ui.set_min_width(ui.available_width());
            add_contents(ui);
        });
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).size(16.0).strong().color(theme::TEXT_STRONG));
    ui.add_space(8.0);
}

pub fn show(
    ui: &mut egui::Ui,
    client: &IpcClientHandle,
    notifier: &Notifier,
    config_opt: &Option<AppConfig>,
    service_error: Option<&str>,
) {
    let Some(config) = config_opt else {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            if service_error.is_some() {
                ui.label("Settings will load once the Backup Agent service is reachable.");
            } else {
                ui.spinner();
                ui.add_space(10.0);
                ui.label("Loading configuration settings...");
            }
        });
        return;
    };

    let state_cell = SETTINGS_STATE.get_or_init(|| Mutex::new(None));
    let mut state_lock = state_cell.lock().unwrap();

    // If local state is uninitialized, fill from current loaded config
    let state = state_lock.get_or_insert_with(|| SettingsState::from_config(config));
    // Recomputed every frame so errors appear and clear while the user types.
    let field_errors = state.field_errors();

    // Scrollable region for forms
    egui::ScrollArea::vertical()
        .max_width(ui.available_width() - 24.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.heading(
                    egui::RichText::new("Settings")
                        .color(theme::TEXT_STRONG)
                        .size(24.0)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new("Configure the SQL Server connection, backup tasks, storage, and notifications.")
                        .color(theme::TEXT_SUBTLE)
                        .size(13.0),
                );
                ui.add_space(16.0);

                // Problems found by the service or the final check on save.
                if !state.errors.is_empty() {
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        ui.style_mut().visuals.widgets.noninteractive.bg_fill = theme::DANGER_SURFACE;
                        ui.vertical(|ui| {
                            for err in &state.errors {
                                ui.label(
                                    egui::RichText::new(format!("⚠️ {}", err))
                                        .color(theme::DANGER)
                                        .strong()
                                );
                            }
                        });
                    });
                    ui.add_space(10.0);
                }

                show_tab_bar(ui, state, &field_errors);
                ui.add_space(16.0);

                match state.active_tab {
                    SettingsTab::SqlServer => show_sql_server_tab(ui, state, &field_errors, client, notifier),
                    SettingsTab::Tasks => show_tasks_tab(ui, state, &field_errors),
                    SettingsTab::Storage => show_storage_tab(ui, state, &field_errors),
                    SettingsTab::Notifications => show_notifications_tab(ui, state, &field_errors),
                }

                ui.add_space(28.0);
                show_save_bar(ui, state, config, client, notifier);
            });
        });

    show_confirmation(ui.ctx(), state, config);
}

/// Tabs across the top of the form; a tab with invalid fields shows how many.
fn show_tab_bar(ui: &mut egui::Ui, state: &mut SettingsState, errors: &FieldErrors) {
    ui.horizontal_wrapped(|ui| {
        for tab in SettingsTab::ALL {
            let is_active = state.active_tab == tab;
            let error_count = errors.count_in(tab);

            let text = if error_count > 0 {
                format!("{}  ⚠ {error_count}", tab.label())
            } else {
                tab.label().to_string()
            };
            let color = if error_count > 0 {
                theme::DANGER
            } else if is_active {
                theme::TEXT_STRONG
            } else {
                theme::TEXT_MUTED
            };

            let btn = egui::Button::new(egui::RichText::new(text).color(color).size(13.5).strong())
                .fill(if is_active { theme::RAISED } else { egui::Color32::TRANSPARENT })
                .stroke(egui::Stroke::new(1.0, if is_active { theme::ACCENT } else { theme::RAISED }))
                .min_size(egui::vec2(130.0, 32.0))
                .rounding(6.0);

            let response = ui.add(btn);
            let response = if error_count > 0 {
                response.on_hover_text(format!("{error_count} field(s) need attention"))
            } else {
                response
            };
            if response.clicked() {
                state.active_tab = tab;
            }
        }
    });
}

fn show_sql_server_tab(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    errors: &FieldErrors,
    client: &IpcClientHandle,
    notifier: &Notifier,
) {
    section_title(ui, "SQL Server Connection");

    section_frame(ui, |ui| {
        form_grid(ui, "sql_server_grid", |ui| {
            form_row(ui, "Host address:", errors, None, |ui| {
                text_input(ui, &mut state.host);
            });
            form_row(ui, "Port number:", errors, Some(Field::Port), |ui| {
                text_input(ui, &mut state.port);
            });
            form_row(ui, "Authentication:", errors, None, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut state.auth_method, "sql".to_string(), "SQL Server Auth");
                    ui.selectable_value(&mut state.auth_method, "windows".to_string(), "Windows Integrated");
                });
            });

            if state.auth_method == "sql" {
                form_row(ui, "Username:", errors, Some(Field::Username), |ui| {
                    text_input(ui, &mut state.username);
                });
                form_row(ui, "Password:", errors, Some(Field::Password), |ui| {
                    secret_input(ui, &mut state.password, "sql_password");
                });
            }
        });

        ui.add_space(16.0);

        // Connection Test Panel
        ui.horizontal(|ui| {
            let test_btn = egui::Button::new(
                egui::RichText::new("🔌 Test Connection & Fetch Databases")
                    .color(theme::TEXT_STRONG)
                    .strong()
            )
            .fill(theme::ACCENT)
            .min_size(egui::vec2(250.0, 32.0));

            if state.is_testing_connection {
                ui.add_enabled(false, test_btn);
                ui.spinner();
            } else if ui.add(test_btn).clicked() {
                let client_clone = client.clone();
                let db_config = SqlServerConfig {
                    host: state.host.trim().to_string(),
                    port: state.port.parse::<u16>().unwrap_or(1433),
                    auth_method: match state.auth_method.as_str() {
                        "sql" => AuthMethod::Sql,
                        _ => AuthMethod::Windows,
                    },
                    username: if state.username.trim().is_empty() { None } else { Some(state.username.trim().to_string()) },
                    password: if state.password.trim().is_empty() { None } else { Some(state.password.trim().to_string()) },
                };

                state.is_testing_connection = true;
                state.connection_test_status = None;

                let notifier = notifier.clone();
                client.spawn(async move {
                    let status = match client_clone.send(IpcRequest::TestConnection(db_config)).await {
                        Ok(IpcResponse::Databases(dbs)) => Ok(dbs),
                        Ok(IpcResponse::Error(e)) => Err(format!("Error: {}", e)),
                        Ok(_) => Err("Unexpected response from the service.".to_string()),
                        Err(e) => Err(format!("Service error: {}", e)),
                    };
                    let lock_cell = SETTINGS_STATE.get().unwrap();
                    let mut l = lock_cell.lock().unwrap();
                    if let Some(s) = l.as_mut() {
                        s.is_testing_connection = false;
                        s.connection_test_status = Some(match status {
                            Ok(dbs) => {
                                s.discovered_databases = dbs;
                                Ok("Connection successful! Databases discovered.".to_string())
                            }
                            Err(e) => Err(e),
                        });
                    }
                    notifier.repaint();
                });
            }

            if let Some(ref status) = state.connection_test_status {
                match status {
                    Ok(msg) => {
                        ui.label(egui::RichText::new(format!("🟢 {}", msg)).color(theme::SUCCESS).strong());
                    }
                    Err(err) => {
                        ui.label(egui::RichText::new(format!("🔴 {}", err)).color(theme::DANGER).strong());
                    }
                }
            }
        });
    });
}

fn show_tasks_tab(ui: &mut egui::Ui, state: &mut SettingsState, errors: &FieldErrors) {
    section_title(ui, "📅 Backup Tasks & Schedules");
    ui.label("Configure one or more backup tasks. Each task specifies a subset of databases and its own schedule/retention rules.");
    ui.add_space(12.0);

    section_frame(ui, |ui| {
        // Task selectors tab row
        ui.horizontal_wrapped(|ui| {
            for i in 0..state.tasks.len() {
                let is_selected = state.selected_task_index == i;
                let label = if errors.task_has_errors(i) {
                    egui::RichText::new(format!("📁 {} ⚠", state.tasks[i].name)).color(theme::DANGER)
                } else {
                    egui::RichText::new(format!("📁 {}", state.tasks[i].name))
                };
                if ui.selectable_label(is_selected, label).clicked() {
                    state.selected_task_index = i;
                }
            }

            ui.label("|");

            let add_btn = egui::Button::new(
                egui::RichText::new("➕ Add Task")
                    .color(theme::TEXT_STRONG)
                    .strong()
            )
            .fill(theme::RAISED)
            .min_size(egui::vec2(90.0, 24.0));

            if ui.add(add_btn).clicked() {
                state.tasks.push(TaskSettingsState::from_config_task(
                    format!("Task {}", state.tasks.len() + 1),
                    Vec::new(),
                    "02:00".to_string(),
                    "7".to_string(),
                ));
                state.selected_task_index = state.tasks.len() - 1;
            }

            if state.tasks.len() > 1 {
                let del_btn = egui::Button::new(
                    egui::RichText::new("🗑 Delete Task")
                        .color(theme::TEXT_STRONG)
                        .strong()
                )
                .fill(theme::DANGER_BUTTON)
                .min_size(egui::vec2(90.0, 24.0));

                if ui.add(del_btn).clicked() {
                    state.pending_confirm = Some(ConfirmAction::DeleteTask(state.selected_task_index));
                }
            }
        });
        field_error(ui, errors, Field::Tasks);
        ui.add_space(14.0);

        // Form editor for the selected task
        let idx = state.selected_task_index;
        if let Some(active_task) = state.tasks.get_mut(idx) {
            form_grid(ui, "task_editor_grid", |ui| {
                form_row(ui, "Task name:", errors, Some(Field::TaskName(idx)), |ui| {
                    text_input(ui, &mut active_task.name);
                });

                form_row(ui, "Schedule Type:", errors, None, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Daily, "Daily");
                        ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Weekly, "Weekly");
                        ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Monthly, "Monthly");
                        ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Custom, "Custom (Cron)");
                    });
                });

                form_row(ui, "Schedule Details:", errors, Some(Field::TaskSchedule(idx)), |ui| {
                    show_schedule_editor(ui, active_task);
                });

                form_row(ui, "Retention policy (days):", errors, Some(Field::TaskRetention(idx)), |ui| {
                    text_input(ui, &mut active_task.retention_days);
                });

                // Databases selection
                if state.discovered_databases.is_empty() {
                    form_row(ui, "Databases (comma separated):", errors, Some(Field::TaskDatabases(idx)), |ui| {
                        let mut dbs_str = active_task.databases.join(", ");
                        let width = (ui.available_width() - 24.0).min(400.0).max(150.0);
                        let edit = egui::TextEdit::singleline(&mut dbs_str).desired_width(width);
                        if ui.add(edit).changed() {
                            active_task.databases = dbs_str.split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                        ui.label(
                            egui::RichText::new("💡 Tip: Click 'Test Connection' on the SQL Server tab to pick databases from a list.")
                                .size(11.0)
                                .color(theme::TEXT_MUTED)
                        );
                    });
                } else {
                    form_row(ui, "Selected databases:", errors, Some(Field::TaskDatabases(idx)), |ui| {
                        for db in &state.discovered_databases {
                            let mut is_checked = active_task.databases.contains(db);
                            if ui.checkbox(&mut is_checked, db).changed() {
                                if is_checked {
                                    active_task.databases.push(db.clone());
                                } else {
                                    active_task.databases.retain(|d| d != db);
                                }
                            }
                        }
                    });
                }
            });
        }
    });
}

fn show_schedule_editor(ui: &mut egui::Ui, active_task: &mut TaskSettingsState) {
    match active_task.schedule_mode {
        ScheduleMode::Daily => {
            ui.horizontal(|ui| {
                ui.label("Times (HH:MM, comma separated):");
                text_input(ui, &mut active_task.daily_times);
            });
            ui.label(
                egui::RichText::new("Example: 09:00, 22:00. Note: multiple times must share the same minute (e.g. :00).")
                    .size(10.5)
                    .color(theme::TEXT_MUTED)
            );
        }
        ScheduleMode::Weekly => {
            ui.horizontal(|ui| {
                ui.label("Days of week:");
                let day_labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
                for i in 0..7 {
                    ui.toggle_value(&mut active_task.weekly_days[i], day_labels[i]);
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Time (HH:MM):");
                text_input(ui, &mut active_task.weekly_time);
            });
        }
        ScheduleMode::Monthly => {
            ui.horizontal(|ui| {
                ui.label("Day of month (1-31):");
                let mut day_str = active_task.monthly_day.to_string();
                let edit = egui::TextEdit::singleline(&mut day_str).desired_width(40.0);
                if ui.add(edit).changed() {
                    if let Ok(val) = day_str.parse::<u32>() {
                        if val >= 1 && val <= 31 {
                            active_task.monthly_day = val;
                        }
                    }
                }
                ui.add_space(12.0);
                ui.label("Time (HH:MM):");
                text_input(ui, &mut active_task.monthly_time);
            });
        }
        ScheduleMode::Custom => {
            ui.horizontal(|ui| {
                ui.label("Cron Expr (7 fields):");
                text_input(ui, &mut active_task.custom_cron);
            });
            ui.label(
                egui::RichText::new("Format: sec min hour dom month dow year (e.g. 0 0/15 * * * * *)")
                    .size(10.5)
                    .color(theme::TEXT_MUTED)
            );
        }
    }
}

fn show_storage_tab(ui: &mut egui::Ui, state: &mut SettingsState, errors: &FieldErrors) {
    section_title(ui, "Storage Configuration");

    section_frame(ui, |ui| {
        form_grid(ui, "storage_grid", |ui| {
            form_row(ui, "Local destination path:", errors, Some(Field::LocalPath), |ui| {
                text_input(ui, &mut state.local_path);
            });

            form_row(ui, "Storage provider:", errors, None, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut state.storage_provider, "local".to_string(), "Local Destination");
                    ui.selectable_value(&mut state.storage_provider, "s3".to_string(), "AWS S3 Compatible Bucket");
                });
            });

            if state.storage_provider == "s3" {
                form_row(ui, "S3 Custom Endpoint:", errors, Some(Field::S3Endpoint), |ui| {
                    text_input(ui, &mut state.s3_endpoint);
                });
                form_row(ui, "S3 Bucket Name:", errors, Some(Field::S3Bucket), |ui| {
                    text_input(ui, &mut state.s3_bucket);
                });
                form_row(ui, "S3 Region Name:", errors, Some(Field::S3Region), |ui| {
                    text_input(ui, &mut state.s3_region);
                });
                form_row(ui, "S3 Access Key:", errors, Some(Field::S3AccessKey), |ui| {
                    text_input(ui, &mut state.s3_access_key);
                });
                form_row(ui, "S3 Secret Key:", errors, Some(Field::S3SecretKey), |ui| {
                    secret_input(ui, &mut state.s3_secret_key, "s3_secret_key");
                });
            }

            form_row(ui, "Also copy to Google Drive:", errors, None, |ui| {
                ui.checkbox(&mut state.gdrive_enabled, "");
            });

            if state.gdrive_enabled {
                form_row(ui, "OAuth Client ID:", errors, Some(Field::GdriveClientId), |ui| {
                    text_input(ui, &mut state.gdrive_client_id);
                });
                form_row(ui, "OAuth Client Secret:", errors, Some(Field::GdriveClientSecret), |ui| {
                    secret_input(ui, &mut state.gdrive_client_secret, "gdrive_client_secret");
                });
                form_row(ui, "OAuth Refresh Token:", errors, Some(Field::GdriveRefreshToken), |ui| {
                    secret_input(ui, &mut state.gdrive_refresh_token, "gdrive_refresh_token");
                });
                form_row(ui, "Drive folder (ID or URL):", errors, None, |ui| {
                    text_input(ui, &mut state.gdrive_folder_id)
                        .on_hover_text("Leave empty to upload to the Drive root folder");
                });
            }
        });
    });
}

fn show_notifications_tab(ui: &mut egui::Ui, state: &mut SettingsState, errors: &FieldErrors) {
    section_title(ui, "Telegram Notifications");

    section_frame(ui, |ui| {
        form_grid(ui, "telegram_grid", |ui| {
            form_row(ui, "Enable notification alerts:", errors, None, |ui| {
                ui.checkbox(&mut state.telegram_enabled, "");
            });

            if state.telegram_enabled {
                form_row(ui, "Telegram Bot Token:", errors, Some(Field::TelegramBotToken), |ui| {
                    secret_input(ui, &mut state.telegram_bot_token, "telegram_bot_token");
                });
                form_row(ui, "Telegram Chat ID:", errors, Some(Field::TelegramChatId), |ui| {
                    text_input(ui, &mut state.telegram_chat_id);
                });
            }
        });
    });
}

/// Save and Reset, shown under every tab: saving covers the whole form.
fn show_save_bar(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    config: &AppConfig,
    client: &IpcClientHandle,
    notifier: &Notifier,
) {
    ui.horizontal(|ui| {
        let save_btn = egui::Button::new(
            egui::RichText::new("💾 Save Configurations & Reload Scheduler")
                .color(theme::TEXT_STRONG)
                .strong()
        )
        .fill(theme::ACCENT)
        .min_size(egui::vec2(280.0, 36.0));

        if state.is_saving {
            ui.add_enabled(false, save_btn);
            ui.spinner();
        } else if ui.add(save_btn).clicked() {
            match state.prepare_save(config) {
                Ok(new_config) => {
                    state.is_saving = true;
                    let client_clone = client.clone();
                    let notifier = notifier.clone();

                    client.spawn(async move {
                        let error = match client_clone.send(IpcRequest::UpdateConfig(new_config)).await {
                            Ok(IpcResponse::Ok) => None,
                            Ok(IpcResponse::Error(e)) => Some(format!("Daemon persistence error: {}", e)),
                            Ok(_) => Some("Unexpected response from the service.".to_string()),
                            Err(e) => Some(format!("Daemon connection error: {}", e)),
                        };
                        {
                            let lock_cell = SETTINGS_STATE.get().unwrap();
                            let mut l = lock_cell.lock().unwrap();
                            if let Some(s) = l.as_mut() {
                                s.is_saving = false;
                                if let Some(ref e) = error {
                                    s.errors.push(e.clone());
                                }
                            }
                        }
                        match error {
                            None => notifier.success("Settings saved and scheduler reloaded."),
                            Some(e) => notifier.error(format!("Settings not saved. {e}")),
                        }
                    });
                }
                Err(message) => notifier.error(message),
            }
        }

        ui.add_space(8.0);
        let reset_btn = egui::Button::new(
            egui::RichText::new("🔄 Reset Form")
                .color(theme::TEXT_STRONG)
                .strong()
        )
        .fill(theme::RAISED)
        .min_size(egui::vec2(100.0, 36.0));

        if ui.add(reset_btn).clicked() {
            state.pending_confirm = Some(ConfirmAction::ResetForm);
        }
    });
}

/// Ask before discarding work: deleting a task or resetting the whole form.
fn show_confirmation(ctx: &egui::Context, state: &mut SettingsState, config: &AppConfig) {
    let Some(action) = state.pending_confirm else {
        return;
    };

    let (title, message, confirm_label) = match action {
        ConfirmAction::DeleteTask(idx) => (
            "Delete task?",
            format!(
                "Task '{}' will be removed from the form. The change takes effect when you save.",
                state.tasks.get(idx).map(|t| t.name.as_str()).unwrap_or("")
            ),
            "Delete Task",
        ),
        ConfirmAction::ResetForm => (
            "Discard changes?",
            "All unsaved edits will be lost and the form will show the saved settings again.".to_string(),
            "Discard Changes",
        ),
    };

    let mut confirmed = false;
    let mut cancelled = false;
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(message);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let confirm_btn = egui::Button::new(
                    egui::RichText::new(confirm_label)
                        .color(theme::TEXT_STRONG)
                        .strong()
                )
                .fill(theme::DANGER_BUTTON);
                confirmed = ui.add(confirm_btn).clicked();
                cancelled = ui.button("Cancel").clicked();
            });
        });

    if confirmed {
        match action {
            ConfirmAction::DeleteTask(idx) => {
                if idx < state.tasks.len() && state.tasks.len() > 1 {
                    state.tasks.remove(idx);
                    state.selected_task_index = idx.min(state.tasks.len() - 1);
                }
                state.pending_confirm = None;
            }
            ConfirmAction::ResetForm => {
                // Rebuilding the state also clears `pending_confirm`.
                *state = SettingsState::from_config(config);
            }
        }
    } else if cancelled {
        state.pending_confirm = None;
    }
}

// ============================================================================
// Schedule Parsing & Compilation Helpers
// ============================================================================

fn parse_schedule_string(schedule_str: &str) -> (ScheduleMode, String, String, [bool; 7], u32, String, String) {
    let mut mode = ScheduleMode::Daily;
    let mut daily_times = "02:00".to_string();
    let mut weekly_time = "02:00".to_string();
    let mut weekly_days = [false; 7];
    let mut monthly_day = 1;
    let mut monthly_time = "02:00".to_string();
    let mut custom_cron = "".to_string();

    let clean = schedule_str.trim();
    if clean.is_empty() {
        return (mode, daily_times, weekly_time, weekly_days, monthly_day, monthly_time, custom_cron);
    }

    if clean.contains(':') && clean.len() == 5 && !clean.contains(' ') {
        mode = ScheduleMode::Daily;
        daily_times = clean.to_string();
    } else {
        let parts: Vec<&str> = clean.split_whitespace().collect();
        if parts.len() >= 6 {
            custom_cron = clean.to_string();
            // Let's see if we can decode common simple cron patterns
            if parts[0] == "0" && parts[1] == "0" && parts[3] == "*" && parts[4] == "*" && parts[5] == "*" {
                mode = ScheduleMode::Daily;
                let hours: Vec<&str> = parts[2].split(',').collect();
                let mut times = Vec::new();
                for h in hours {
                    if let Ok(hour_val) = h.parse::<u32>() {
                        if hour_val <= 23 {
                            times.push(format!("{:02}:00", hour_val));
                        }
                    }
                }
                if !times.is_empty() {
                    daily_times = times.join(", ");
                }
            } else if parts[0] == "0" && parts[1] == "0" && parts[3] == "*" && parts[4] == "*" && parts[5] != "*" {
                if let (Ok(h), Ok(m)) = (parts[2].parse::<u32>(), parts[1].parse::<u32>()) {
                    if h <= 23 && m <= 59 {
                        mode = ScheduleMode::Weekly;
                        weekly_time = format!("{:02}:{:02}", h, m);
                        let dow_str = parts[5].to_lowercase();
                        weekly_days[0] = dow_str.contains("mon") || dow_str.contains("1");
                        weekly_days[1] = dow_str.contains("tue") || dow_str.contains("2");
                        weekly_days[2] = dow_str.contains("wed") || dow_str.contains("3");
                        weekly_days[3] = dow_str.contains("thu") || dow_str.contains("4");
                        weekly_days[4] = dow_str.contains("fri") || dow_str.contains("5");
                        weekly_days[5] = dow_str.contains("sat") || dow_str.contains("6");
                        weekly_days[6] = dow_str.contains("sun") || dow_str.contains("7") || dow_str.contains("0");
                    }
                }
            } else if parts[0] == "0" && parts[1] == "0" && parts[4] == "*" && parts[5] == "*" {
                if let (Ok(h), Ok(dom)) = (parts[2].parse::<u32>(), parts[3].parse::<u32>()) {
                    if h <= 23 && dom >= 1 && dom <= 31 {
                        mode = ScheduleMode::Monthly;
                        monthly_day = dom;
                        monthly_time = format!("{:02}:00", h);
                    }
                }
            } else {
                mode = ScheduleMode::Custom;
            }
        } else {
            mode = ScheduleMode::Custom;
            custom_cron = clean.to_string();
        }
    }

    (mode, daily_times, weekly_time, weekly_days, monthly_day, monthly_time, custom_cron)
}

fn compile_schedule_string(
    mode: ScheduleMode,
    daily_times: &str,
    weekly_time: &str,
    weekly_days: &[bool; 7],
    monthly_day: u32,
    monthly_time: &str,
    custom_cron: &str,
) -> Result<String, String> {
    match mode {
        ScheduleMode::Daily => {
            let mut hours = Vec::new();
            let mut minutes = Vec::new();
            
            let times: Vec<&str> = daily_times.split(',').collect();
            for t in times {
                let clean_t = t.trim();
                if clean_t.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = clean_t.split(':').collect();
                if parts.len() != 2 {
                    return Err(format!("Invalid time format '{}'. Expected HH:MM", clean_t));
                }
                let h = parts[0].parse::<u32>().map_err(|_| format!("Invalid hour in '{}'", clean_t))?;
                let m = parts[1].parse::<u32>().map_err(|_| format!("Invalid minute in '{}'", clean_t))?;
                if h > 23 || m > 59 {
                    return Err(format!("Time '{}' out of bounds (Hour 0-23, Minute 0-59)", clean_t));
                }
                hours.push(h);
                minutes.push(m);
            }
            
            if hours.is_empty() {
                return Err("At least one time must be specified for Daily schedule".to_string());
            }
            
            if hours.len() == 1 {
                Ok(format!("{:02}:{:02}", hours[0], minutes[0]))
            } else {
                let first_m = minutes[0];
                let all_same_min = minutes.iter().all(|&m| m == first_m);
                
                if all_same_min {
                    let mut unique_hours = hours.clone();
                    unique_hours.sort();
                    unique_hours.dedup();
                    let hours_str: Vec<String> = unique_hours.iter().map(|h| h.to_string()).collect();
                    Ok(format!("0 {} {} * * * *", first_m, hours_str.join(",")))
                } else {
                    Err("For multiple daily times, all times must have the same minute (e.g. all at :00) to be represented in a single Cron schedule.".to_string())
                }
            }
        }
        ScheduleMode::Weekly => {
            let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            let mut selected_days = Vec::new();
            for i in 0..7 {
                if weekly_days[i] {
                    selected_days.push(day_names[i]);
                }
            }
            if selected_days.is_empty() {
                return Err("At least one weekday must be selected for Weekly schedule".to_string());
            }
            
            let parts: Vec<&str> = weekly_time.trim().split(':').collect();
            if parts.len() != 2 {
                return Err("Invalid time for Weekly schedule. Expected HH:MM".to_string());
            }
            let h = parts[0].parse::<u32>().map_err(|_| "Invalid hour in Weekly time".to_string())?;
            let m = parts[1].parse::<u32>().map_err(|_| "Invalid minute in Weekly time".to_string())?;
            if h > 23 || m > 59 {
                return Err("Weekly time out of bounds (Hour 0-23, Minute 0-59)".to_string());
            }
            
            Ok(format!("0 {} {} * * {} *", m, h, selected_days.join(",")))
        }
        ScheduleMode::Monthly => {
            if monthly_day < 1 || monthly_day > 31 {
                return Err("Monthly day must be between 1 and 31".to_string());
            }
            let parts: Vec<&str> = monthly_time.trim().split(':').collect();
            if parts.len() != 2 {
                return Err("Invalid time for Monthly schedule. Expected HH:MM".to_string());
            }
            let h = parts[0].parse::<u32>().map_err(|_| "Invalid hour in Monthly time".to_string())?;
            let m = parts[1].parse::<u32>().map_err(|_| "Invalid minute in Monthly time".to_string())?;
            if h > 23 || m > 59 {
                return Err("Monthly time out of bounds (Hour 0-23, Minute 0-59)".to_string());
            }
            
            Ok(format!("0 {} {} {} * * *", m, h, monthly_day))
        }
        ScheduleMode::Custom => {
            let clean = custom_cron.trim();
            if clean.is_empty() {
                return Err("Custom Cron expression cannot be empty".to_string());
            }
            
            let fields: Vec<&str> = clean.split_whitespace().collect();
            if fields.len() != 7 {
                return Err("Custom Cron must contain exactly 7 fields: sec min hour dom month dow year".to_string());
            }
            
            Ok(clean.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_compile_daily() {
        let parsed = parse_schedule_string("02:00");
        assert_eq!(parsed.0, ScheduleMode::Daily);
        assert_eq!(parsed.1, "02:00");

        let compiled = compile_schedule_string(parsed.0, &parsed.1, &parsed.2, &parsed.3, parsed.4, &parsed.5, &parsed.6);
        assert_eq!(compiled.unwrap(), "02:00");
    }

    #[test]
    fn test_parse_and_compile_multiple_daily() {
        let parsed = parse_schedule_string("0 0 9,22 * * * *");
        assert_eq!(parsed.0, ScheduleMode::Daily);
        assert_eq!(parsed.1, "09:00, 22:00");

        let compiled = compile_schedule_string(parsed.0, &parsed.1, &parsed.2, &parsed.3, parsed.4, &parsed.5, &parsed.6);
        assert_eq!(compiled.unwrap(), "0 0 9,22 * * * *");
    }

    #[test]
    fn test_parse_and_compile_weekly() {
        let parsed = parse_schedule_string("0 0 3 * * Mon,Fri *");
        assert_eq!(parsed.0, ScheduleMode::Weekly);
        assert_eq!(parsed.2, "03:00");
        assert_eq!(parsed.3[0], true); // Mon
        assert_eq!(parsed.3[4], true); // Fri

        let compiled = compile_schedule_string(parsed.0, &parsed.1, &parsed.2, &parsed.3, parsed.4, &parsed.5, &parsed.6);
        assert_eq!(compiled.unwrap(), "0 0 3 * * Mon,Fri *");
    }

    #[test]
    fn test_parse_and_compile_monthly() {
        let parsed = parse_schedule_string("0 0 12 5 * * *");
        assert_eq!(parsed.0, ScheduleMode::Monthly);
        assert_eq!(parsed.4, 5); // 5th of month
        assert_eq!(parsed.5, "12:00");

        let compiled = compile_schedule_string(parsed.0, &parsed.1, &parsed.2, &parsed.3, parsed.4, &parsed.5, &parsed.6);
        assert_eq!(compiled.unwrap(), "0 0 12 5 * * *");
    }

    fn config_with_one_task() -> AppConfig {
        let mut config = AppConfig::default();
        // The default ships an empty SQL password on purpose; make it valid.
        config.sql_server.password = Some("secret".into());
        config.tasks = vec![BackupTaskConfig {
            name: "Nightly".into(),
            databases: vec!["OrdersDB".into()],
            schedule: "02:00".into(),
            retention_days: 7,
        }];
        config
    }

    #[test]
    fn saving_preserves_service_section() {
        let mut config = config_with_one_task();
        config.service.history_max_entries = 42;
        config.service.log_rotate_threshold_bytes = 1_048_576;

        let saved = SettingsState::from_config(&config).to_config(&config).unwrap();

        assert_eq!(saved.service, config.service);
    }

    #[test]
    fn saving_keeps_edited_form_values() {
        let config = config_with_one_task();
        let mut state = SettingsState::from_config(&config);
        state.host = "  sql-prod  ".into();

        let saved = state.to_config(&config).unwrap();

        assert_eq!(saved.sql_server.host, "sql-prod");
        assert_eq!(saved.tasks, config.tasks);
    }

    #[test]
    fn drive_folder_id_accepts_raw_id_url_or_empty() {
        assert_eq!(extract_drive_folder_id("  1AbC_xyz  ").as_deref(), Some("1AbC_xyz"));
        assert_eq!(
            extract_drive_folder_id("https://drive.google.com/drive/folders/1AbC_xyz?usp=sharing").as_deref(),
            Some("1AbC_xyz")
        );
        assert_eq!(
            extract_drive_folder_id("https://drive.google.com/drive/u/0/folders/1AbC_xyz").as_deref(),
            Some("1AbC_xyz")
        );
        assert_eq!(extract_drive_folder_id("   "), None);
    }

    fn valid_state() -> (AppConfig, SettingsState) {
        let config = config_with_one_task();
        let state = SettingsState::from_config(&config);
        (config, state)
    }

    #[test]
    fn valid_form_has_no_field_errors() {
        let (_, state) = valid_state();
        assert_eq!(state.field_errors().len(), 0);
    }

    #[test]
    fn invalid_port_is_reported_on_the_sql_server_tab() {
        let (_, mut state) = valid_state();
        state.port = "99999".into();

        let errors = state.field_errors();

        assert_eq!(errors.get(Field::Port), Some("Port must be a number between 0 and 65535"));
        assert_eq!(errors.count_in(SettingsTab::SqlServer), 1);
        assert_eq!(errors.count_in(SettingsTab::Tasks), 0);
    }

    #[test]
    fn sql_auth_requires_username_and_password_but_windows_auth_does_not() {
        let (_, mut state) = valid_state();
        state.auth_method = "sql".into();
        state.username = "  ".into();
        state.password = String::new();

        let errors = state.field_errors();
        assert_eq!(errors.get(Field::Username), Some("Required for SQL Server authentication"));
        assert_eq!(errors.get(Field::Password), Some("Required for SQL Server authentication"));

        state.auth_method = "windows".into();
        assert_eq!(state.field_errors().len(), 0);
    }

    #[test]
    fn task_fields_are_reported_per_task() {
        let (_, mut state) = valid_state();
        state.tasks.push(TaskSettingsState::from_config_task(
            "  ".into(),
            Vec::new(),
            "02:00".into(),
            "abc".into(),
        ));
        state.tasks[1].schedule_mode = ScheduleMode::Custom;
        state.tasks[1].custom_cron = "0 0 *".into();

        let errors = state.field_errors();

        assert!(!errors.task_has_errors(0));
        assert!(errors.task_has_errors(1));
        assert_eq!(errors.get(Field::TaskName(1)), Some("Task name is required"));
        assert_eq!(errors.get(Field::TaskDatabases(1)), Some("Select at least one database"));
        assert_eq!(errors.get(Field::TaskRetention(1)), Some("Retention must be a whole number of days"));
        assert_eq!(
            errors.get(Field::TaskSchedule(1)),
            Some("Custom Cron must contain exactly 7 fields: sec min hour dom month dow year")
        );
        assert_eq!(errors.count_in(SettingsTab::Tasks), 4);
    }

    #[test]
    fn zero_retention_and_empty_task_list_are_reported() {
        let (_, mut state) = valid_state();
        state.tasks[0].retention_days = "0".into();
        assert_eq!(
            state.field_errors().get(Field::TaskRetention(0)),
            Some("Retention must be at least 1 day")
        );

        state.tasks.clear();
        assert_eq!(state.field_errors().get(Field::Tasks), Some("Add at least one backup task"));
    }

    #[test]
    fn enabled_integrations_require_their_credentials() {
        let (_, mut state) = valid_state();
        state.local_path = " ".into();
        state.gdrive_enabled = true;
        state.telegram_enabled = true;

        let errors = state.field_errors();

        assert_eq!(errors.get(Field::LocalPath), Some("Destination path is required"));
        for field in [Field::GdriveClientId, Field::GdriveClientSecret, Field::GdriveRefreshToken] {
            assert_eq!(errors.get(field), Some("Required when Google Drive copy is enabled"), "{field:?}");
        }
        for field in [Field::TelegramBotToken, Field::TelegramChatId] {
            assert_eq!(errors.get(field), Some("Required when Telegram alerts are enabled"), "{field:?}");
        }
        assert_eq!(errors.count_in(SettingsTab::Storage), 4);
        assert_eq!(errors.count_in(SettingsTab::Notifications), 2);
    }

    #[test]
    fn s3_provider_requires_its_fields() {
        let (_, mut state) = valid_state();
        state.storage_provider = "s3".into();

        let errors = state.field_errors();

        for field in [Field::S3Endpoint, Field::S3Bucket, Field::S3Region, Field::S3AccessKey, Field::S3SecretKey] {
            assert_eq!(errors.get(field), Some("Required for S3 storage"), "{field:?}");
        }
    }

    #[test]
    fn save_with_field_errors_is_blocked_and_jumps_to_the_first_invalid_field() {
        let (config, mut state) = valid_state();
        state.tasks.push(TaskSettingsState::from_config_task(
            "Weekly".into(),
            Vec::new(),
            "02:00".into(),
            "7".into(),
        ));
        state.telegram_enabled = true;
        state.active_tab = SettingsTab::Notifications;
        state.selected_task_index = 0;

        let result = state.prepare_save(&config);

        assert_eq!(
            result.unwrap_err(),
            "Settings not saved: fix the 3 field(s) marked in red."
        );
        assert_eq!(state.active_tab, SettingsTab::Tasks);
        assert_eq!(state.selected_task_index, 1);
    }

    #[test]
    fn save_of_a_valid_form_returns_the_config() {
        let (config, mut state) = valid_state();
        state.errors.push("stale error".into());

        let saved = state.prepare_save(&config).unwrap();

        assert_eq!(saved.tasks, config.tasks);
        assert!(state.errors.is_empty());
    }
}
