use eframe::egui;
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::ipc_client::IpcClientHandle;
use backup_agent_core::domain::config::{AppConfig, AuthMethod, SqlServerConfig, BackupConfig, StorageConfig, StorageProviderType, S3Config, TelegramConfig, BackupTaskConfig};
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
    pub success: Option<String>,
    pub is_saving: bool,
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
            telegram_enabled: config.telegram.enabled,
            telegram_bot_token: config.telegram.bot_token.clone(),
            telegram_chat_id: config.telegram.chat_id.clone(),
            
            tasks,
            selected_task_index: 0,
            discovered_databases: Vec::new(),
            connection_test_status: None,
            is_testing_connection: false,

            errors: Vec::new(),
            success: None,
            is_saving: false,
        }
    }

    fn to_config(&self) -> Result<AppConfig, String> {
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
            },
            service: Default::default(),
        };

        Ok(config)
    }
}

static SETTINGS_STATE: OnceLock<Mutex<Option<SettingsState>>> = OnceLock::new();

fn text_input(ui: &mut egui::Ui, text: &mut String, is_password: bool) -> egui::Response {
    let width = (ui.available_width() - 24.0).min(400.0).max(150.0);
    let mut edit = egui::TextEdit::singleline(text).desired_width(width);
    if is_password {
        edit = edit.password(true);
    }
    ui.add(edit)
}

pub fn show(ui: &mut egui::Ui, client: &IpcClientHandle, config_opt: &Option<AppConfig>) {
    let Some(config) = config_opt else {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.spinner();
            ui.add_space(10.0);
            ui.label("Loading configuration settings...");
        });
        return;
    };

    let state_cell = SETTINGS_STATE.get_or_init(|| Mutex::new(None));
    let mut state_lock = state_cell.lock().unwrap();

    // If local state is uninitialized, fill from current loaded config
    let state = state_lock.get_or_insert_with(|| SettingsState::from_config(config));

    // Scrollable region for forms
    egui::ScrollArea::vertical()
        .max_width(ui.available_width() - 24.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.heading(
                    egui::RichText::new("Settings")
                        .color(egui::Color32::from_rgb(255, 255, 255))
                        .size(24.0)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new("Configure parameters for SQL Server connections, scheduling, S3 storage, and notifications.")
                        .color(egui::Color32::from_rgb(140, 155, 175))
                        .size(13.0),
                );
                ui.add_space(16.0);

                // Feedback warnings
                if !state.errors.is_empty() {
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        ui.style_mut().visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(45, 15, 20);
                        ui.vertical(|ui| {
                            for err in &state.errors {
                                ui.label(
                                    egui::RichText::new(format!("⚠️ {}", err))
                                        .color(egui::Color32::from_rgb(239, 68, 68))
                                        .strong()
                                );
                            }
                        });
                    });
                    ui.add_space(10.0);
                }

                if let Some(ref succ) = state.success {
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        ui.style_mut().visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(15, 45, 25);
                        ui.label(
                            egui::RichText::new(succ)
                                .color(egui::Color32::from_rgb(34, 197, 94))
                                .strong()
                        );
                    });
                    ui.add_space(10.0);
                }

                // -------------------------------------------------------------
                // SQL Server Settings
                // -------------------------------------------------------------
                ui.label(egui::RichText::new("SQL Server Connection").size(16.0).strong().color(egui::Color32::from_rgb(255, 255, 255)));
                ui.add_space(8.0);

                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(20, 26, 38))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 41, 59)))
                    .rounding(8.0)
                    .inner_margin(16.0)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            egui::Grid::new("sql_server_grid")
                                .num_columns(2)
                                .spacing([24.0, 14.0])
                                .min_col_width(140.0)
                                .show(ui, |ui| {
                                    ui.label("Host address:");
                                    text_input(ui, &mut state.host, false);
                                    ui.end_row();

                                    ui.label("Port number:");
                                    text_input(ui, &mut state.port, false);
                                    ui.end_row();

                                    ui.label("Authentication:");
                                    ui.horizontal_wrapped(|ui| {
                                        ui.selectable_value(&mut state.auth_method, "sql".to_string(), "SQL Server Auth");
                                        ui.selectable_value(&mut state.auth_method, "windows".to_string(), "Windows Integrated");
                                    });
                                    ui.end_row();

                                    if state.auth_method == "sql" {
                                        ui.label("Username:");
                                        text_input(ui, &mut state.username, false);
                                        ui.end_row();

                                        ui.label("Password:");
                                        text_input(ui, &mut state.password, true);
                                        ui.end_row();
                                    }
                                });
                            
                            ui.add_space(16.0);

                            // Connection Test Panel
                            ui.horizontal(|ui| {
                                let test_btn = egui::Button::new(
                                    egui::RichText::new("🔌 Test Connection & Fetch Databases")
                                        .color(egui::Color32::from_rgb(255, 255, 255))
                                        .strong()
                                )
                                .fill(egui::Color32::from_rgb(99, 102, 241)) // Electric Indigo
                                .min_size(egui::vec2(250.0, 32.0));

                                if state.is_testing_connection {
                                    ui.add_enabled(false, test_btn);
                                    ui.spinner();
                                } else {
                                    if ui.add(test_btn).clicked() {
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

                                        std::thread::spawn(move || {
                                            let rt = tokio::runtime::Runtime::new().unwrap();
                                            rt.block_on(async {
                                                match client_clone.send(IpcRequest::TestConnection(db_config)).await {
                                                    Ok(IpcResponse::Databases(dbs)) => {
                                                        let lock_cell = SETTINGS_STATE.get().unwrap();
                                                        let mut l = lock_cell.lock().unwrap();
                                                        if let Some(s) = l.as_mut() {
                                                            s.discovered_databases = dbs;
                                                            s.connection_test_status = Some(Ok("Connection successful! Databases discovered.".to_string()));
                                                            s.is_testing_connection = false;
                                                        }
                                                    }
                                                    Ok(IpcResponse::Error(e)) => {
                                                        let lock_cell = SETTINGS_STATE.get().unwrap();
                                                        let mut l = lock_cell.lock().unwrap();
                                                        if let Some(s) = l.as_mut() {
                                                            s.connection_test_status = Some(Err(format!("Error: {}", e)));
                                                            s.is_testing_connection = false;
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let lock_cell = SETTINGS_STATE.get().unwrap();
                                                        let mut l = lock_cell.lock().unwrap();
                                                        if let Some(s) = l.as_mut() {
                                                            s.connection_test_status = Some(Err(format!("IPC Error: {}", e)));
                                                            s.is_testing_connection = false;
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            });
                                        });
                                    }
                                }

                                if let Some(ref status) = state.connection_test_status {
                                    match status {
                                        Ok(msg) => {
                                            ui.label(egui::RichText::new(format!("🟢 {}", msg)).color(egui::Color32::from_rgb(34, 197, 94)).strong());
                                        }
                                        Err(err) => {
                                            ui.label(egui::RichText::new(format!("🔴 {}", err)).color(egui::Color32::from_rgb(239, 68, 68)).strong());
                                        }
                                    }
                                }
                            });
                        });
                    });

                ui.add_space(24.0);
                ui.add_space(10.0);

                // -------------------------------------------------------------
                // Backup Tasks Manager
                // -------------------------------------------------------------
                ui.label(egui::RichText::new("📅 Backup Tasks & Schedules").size(16.0).strong().color(egui::Color32::from_rgb(255, 255, 255)));
                ui.add_space(4.0);
                ui.label("Configure one or more backup tasks. Each task specifies a subset of databases and its own schedule/retention rules.");
                ui.add_space(12.0);

                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(20, 26, 38))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 41, 59)))
                    .rounding(8.0)
                    .inner_margin(16.0)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            // Task selectors tab row
                            ui.horizontal(|ui| {
                                for i in 0..state.tasks.len() {
                                    let is_selected = state.selected_task_index == i;
                                    if ui.selectable_label(is_selected, format!("📁 {}", state.tasks[i].name)).clicked() {
                                        state.selected_task_index = i;
                                    }
                                }
                                
                                ui.label("|");
                                
                                let add_btn = egui::Button::new(
                                    egui::RichText::new("➕ Add Task")
                                        .color(egui::Color32::from_rgb(255, 255, 255))
                                        .strong()
                                )
                                .fill(egui::Color32::from_rgb(30, 41, 59))
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
                                            .color(egui::Color32::from_rgb(255, 255, 255))
                                            .strong()
                                    )
                                    .fill(egui::Color32::from_rgb(185, 28, 28))
                                    .min_size(egui::vec2(90.0, 24.0));

                                    if ui.add(del_btn).clicked() {
                                        state.tasks.remove(state.selected_task_index);
                                        state.selected_task_index = 0;
                                    }
                                }
                            });
                            ui.add_space(14.0);

                            // Form editor for the selected task
                            if let Some(active_task) = state.tasks.get_mut(state.selected_task_index) {
                                egui::Grid::new("task_editor_grid")
                                    .num_columns(2)
                                    .spacing([24.0, 14.0])
                                    .min_col_width(140.0)
                                    .show(ui, |ui| {
                                        ui.label("Task name:");
                                        text_input(ui, &mut active_task.name, false);
                                        ui.end_row();

                                        ui.label("Schedule Type:");
                                        ui.horizontal(|ui| {
                                            ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Daily, "Daily");
                                            ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Weekly, "Weekly");
                                            ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Monthly, "Monthly");
                                            ui.selectable_value(&mut active_task.schedule_mode, ScheduleMode::Custom, "Custom (Cron)");
                                        });
                                        ui.end_row();

                                        ui.label("Schedule Details:");
                                        ui.vertical(|ui| {
                                            match active_task.schedule_mode {
                                                ScheduleMode::Daily => {
                                                    ui.horizontal(|ui| {
                                                        ui.label("Times (HH:MM, comma separated):");
                                                        text_input(ui, &mut active_task.daily_times, false);
                                                    });
                                                    ui.label(
                                                        egui::RichText::new("Example: 09:00, 22:00. Note: multiple times must share the same minute (e.g. :00).")
                                                            .size(10.5)
                                                            .color(egui::Color32::from_rgb(148, 163, 184))
                                                    );
                                                }
                                                ScheduleMode::Weekly => {
                                                    ui.vertical(|ui| {
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
                                                            text_input(ui, &mut active_task.weekly_time, false);
                                                        });
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
                                                        text_input(ui, &mut active_task.monthly_time, false);
                                                    });
                                                }
                                                ScheduleMode::Custom => {
                                                    ui.horizontal(|ui| {
                                                        ui.label("Cron Expr (7 fields):");
                                                        text_input(ui, &mut active_task.custom_cron, false);
                                                    });
                                                    ui.label(
                                                        egui::RichText::new("Format: sec min hour dom month dow year (e.g. 0 0/15 * * * * *)")
                                                            .size(10.5)
                                                            .color(egui::Color32::from_rgb(148, 163, 184))
                                                    );
                                                }
                                            }
                                        });
                                        ui.end_row();

                                        ui.label("Retention policy (days):");
                                        text_input(ui, &mut active_task.retention_days, false);
                                        ui.end_row();

                                        // Databases selection
                                        if state.discovered_databases.is_empty() {
                                            ui.label("Databases (comma separated):");
                                            ui.vertical(|ui| {
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
                                                    egui::RichText::new("💡 Tip: Click 'Test Connection' above to populate databases automatically.")
                                                        .size(11.0)
                                                        .color(egui::Color32::from_rgb(148, 163, 184))
                                                );
                                            });
                                            ui.end_row();
                                        } else {
                                            ui.label("Selected databases:");
                                            ui.vertical(|ui| {
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
                                            ui.end_row();
                                        }
                                    });
                            }
                        });
                    });

                ui.add_space(24.0);
                ui.add_space(10.0);

                // -------------------------------------------------------------
                // Backup global storage settings
                // -------------------------------------------------------------
                ui.label(egui::RichText::new("Storage Configuration").size(16.0).strong().color(egui::Color32::from_rgb(255, 255, 255)));
                ui.add_space(8.0);

                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(20, 26, 38))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 41, 59)))
                    .rounding(8.0)
                    .inner_margin(16.0)
                    .show(ui, |ui| {
                        egui::Grid::new("storage_grid")
                            .num_columns(2)
                            .spacing([24.0, 14.0])
                            .min_col_width(140.0)
                            .show(ui, |ui| {
                                ui.label("Local destination path:");
                                text_input(ui, &mut state.local_path, false);
                                ui.end_row();

                                ui.label("Storage provider:");
                                ui.horizontal_wrapped(|ui| {
                                    ui.selectable_value(&mut state.storage_provider, "local".to_string(), "Local Destination");
                                    ui.selectable_value(&mut state.storage_provider, "s3".to_string(), "AWS S3 Compatible Bucket");
                                });
                                ui.end_row();

                                if state.storage_provider == "s3" {
                                    ui.label("S3 Custom Endpoint:");
                                    text_input(ui, &mut state.s3_endpoint, false);
                                    ui.end_row();

                                    ui.label("S3 Bucket Name:");
                                    text_input(ui, &mut state.s3_bucket, false);
                                    ui.end_row();

                                    ui.label("S3 Region Name:");
                                    text_input(ui, &mut state.s3_region, false);
                                    ui.end_row();

                                    ui.label("S3 Access Key:");
                                    text_input(ui, &mut state.s3_access_key, false);
                                    ui.end_row();

                                    ui.label("S3 Secret Key:");
                                    text_input(ui, &mut state.s3_secret_key, true);
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(24.0);

                // -------------------------------------------------------------
                // Telegram alerts
                // -------------------------------------------------------------
                ui.label(egui::RichText::new("Telegram Notifications").size(16.0).strong().color(egui::Color32::from_rgb(255, 255, 255)));
                ui.add_space(8.0);

                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(20, 26, 38))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 41, 59)))
                    .rounding(8.0)
                    .inner_margin(16.0)
                    .show(ui, |ui| {
                        egui::Grid::new("telegram_grid")
                            .num_columns(2)
                            .spacing([24.0, 14.0])
                            .min_col_width(140.0)
                            .show(ui, |ui| {
                                ui.label("Enable notification alerts:");
                                ui.checkbox(&mut state.telegram_enabled, "");
                                ui.end_row();

                                if state.telegram_enabled {
                                    ui.label("Telegram Bot Token:");
                                    text_input(ui, &mut state.telegram_bot_token, true);
                                    ui.end_row();

                                    ui.label("Telegram Chat ID:");
                                    text_input(ui, &mut state.telegram_chat_id, false);
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(28.0);

                // Save buttons
                ui.horizontal(|ui| {
                    let save_btn = egui::Button::new(
                        egui::RichText::new("💾 Save Configurations & Reload Scheduler")
                            .color(egui::Color32::from_rgb(255, 255, 255))
                            .strong()
                    )
                    .fill(egui::Color32::from_rgb(99, 102, 241)) // Electric Indigo
                    .min_size(egui::vec2(280.0, 36.0));

                    if state.is_saving {
                        ui.add_enabled(false, save_btn);
                        ui.spinner();
                    } else {
                        if ui.add(save_btn).clicked() {
                            state.errors.clear();
                            state.success = None;
                            
                            match state.to_config() {
                                Ok(new_config) => {
                                    let mut core_errors = new_config.validate();
                                    if !core_errors.is_empty() {
                                        state.errors.append(&mut core_errors);
                                    } else {
                                        state.is_saving = true;
                                        let client_clone = client.clone();
                                        
                                        std::thread::spawn(move || {
                                            let rt = tokio::runtime::Runtime::new().unwrap();
                                            rt.block_on(async {
                                                match client_clone.send(IpcRequest::UpdateConfig(new_config)).await {
                                                    Ok(IpcResponse::Ok) => {
                                                        let lock_cell = SETTINGS_STATE.get().unwrap();
                                                        let mut l = lock_cell.lock().unwrap();
                                                        if let Some(s) = l.as_mut() {
                                                            s.success = Some("Configurations saved and scheduler hot-reloaded successfully!".to_string());
                                                            s.is_saving = false;
                                                        }
                                                    }
                                                    Ok(IpcResponse::Error(e)) => {
                                                        let lock_cell = SETTINGS_STATE.get().unwrap();
                                                        let mut l = lock_cell.lock().unwrap();
                                                        if let Some(s) = l.as_mut() {
                                                            s.errors.push(format!("Daemon persistence error: {}", e));
                                                            s.is_saving = false;
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let lock_cell = SETTINGS_STATE.get().unwrap();
                                                        let mut l = lock_cell.lock().unwrap();
                                                        if let Some(s) = l.as_mut() {
                                                            s.errors.push(format!("Daemon connection error: {}", e));
                                                            s.is_saving = false;
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            });
                                        });
                                    }
                                }
                                Err(e) => {
                                    state.errors.push(e);
                                }
                            }
                        }
                    }

                    ui.add_space(8.0);
                    let reset_btn = egui::Button::new(
                        egui::RichText::new("🔄 Reset Form")
                            .color(egui::Color32::from_rgb(255, 255, 255))
                            .strong()
                    )
                    .fill(egui::Color32::from_rgb(30, 41, 59))
                    .min_size(egui::vec2(100.0, 36.0));

                    if ui.add(reset_btn).clicked() {
                        *state = SettingsState::from_config(config);
                    }
                });
            });
        });
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
}
