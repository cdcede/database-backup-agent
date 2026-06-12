//! Cron-based backup scheduler implementation.
//!
//! Parses horary/cron schedules and manages the wait-loop calculating exact
//! durations to sleep before firing the next backup task.

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule;

/// Manages backup execution timing.
pub struct BackupScheduler {
    schedule: Schedule,
}

impl BackupScheduler {
    /// Create a new `BackupScheduler` from a schedule string.
    ///
    /// Supports:
    /// 1. Simple 24h format: `"HH:MM"` (e.g. `"02:30"` -> daily at 02:30:00).
    /// 2. Advanced 7-field Cron format: `"sec min hour dom month dow year"`
    ///    (e.g. `"0 0/15 * * * * *"` -> every 15 minutes).
    pub fn new(schedule_str: &str) -> Result<Self, String> {
        let cron_expr = if schedule_str.contains(':') && schedule_str.len() == 5 {
            let parts: Vec<&str> = schedule_str.split(':').collect();
            if parts.len() == 2 {
                let hour = parts[0].parse::<u32>().map_err(|_| "Invalid hour number")?;
                let min = parts[1].parse::<u32>().map_err(|_| "Invalid minute number")?;
                
                if hour > 23 || min > 59 {
                    return Err("Hour must be 0-23 and minute 0-59".into());
                }
                
                // Construct standard 7-field cron: sec min hour dom month dow year
                format!("0 {} {} * * * *", min, hour)
            } else {
                return Err("Invalid 24h format. Expected HH:MM".into());
            }
        } else {
            schedule_str.to_string()
        };

        let schedule = Schedule::from_str(&cron_expr)
            .map_err(|e| format!("Cron parsing error: {}", e))?;

        Ok(Self {
            schedule,
        })
    }

    /// Calculate the Duration remaining until the next scheduled fire time.
    ///
    /// Returns `None` if there are no more matches in the cron timeline.
    pub fn duration_until_next(&self) -> Option<Duration> {
        let now = Utc::now();
        let next = self.schedule.upcoming(Utc).next()?;
        let duration = next.signed_duration_since(now);
        let secs = duration.num_seconds();

        if secs > 0 {
            Some(Duration::from_secs(secs as u64))
        } else {
            // Prevent hot-looping if time delta is zero or negative (sub-second edge cases)
            Some(Duration::from_secs(1))
        }
    }

    /// Get the next scheduled execution timestamp.
    pub fn next_execution(&self) -> Option<DateTime<Utc>> {
        self.schedule.upcoming(Utc).next()
    }

}

/// Run a background coordinator that manages multiple scheduled backup tasks.
///
/// Spawns separate task runners for each task, and listens to the reload channel
/// to hot-reload the schedule runners if the configuration changes.
pub async fn start_multi_scheduler(
    state: std::sync::Arc<crate::service_handler::DaemonState>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    tracing::info!("Starting background multi-task scheduler coordinator");

    let mut reload_rx = state.reload_tx.subscribe();
    let mut active_runners: Vec<(String, tokio::task::JoinHandle<()>, tokio::sync::oneshot::Sender<()>)> = Vec::new();

    // Helper closure to spawn all task runners
    let spawn_runners = |state: std::sync::Arc<crate::service_handler::DaemonState>,
                         runners: &mut Vec<(String, tokio::task::JoinHandle<()>, tokio::sync::oneshot::Sender<()>)>| {
        let config_guard = match state.config.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("Configuration lock is temporarily busy. Retrying runner spawn...");
                return;
            }
        };

        for task in &config_guard.tasks {
            let task_name = task.name.clone();
            let schedule_str = task.schedule.clone();

            let scheduler = match BackupScheduler::new(&schedule_str) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to parse schedule '{}' for task '{}': {}", schedule_str, task_name, e);
                    continue;
                }
            };

            let (task_shutdown_tx, mut task_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let state_clone = state.clone();
            let name_clone = task_name.clone();

            tracing::info!("Starting task scheduler runner for task '{}' [schedule: {}]", task_name, schedule_str);

            let handle = tokio::spawn(async move {
                loop {
                    let sleep_duration = match scheduler.duration_until_next() {
                        Some(dur) => dur,
                        None => {
                            tracing::error!("No further execution dates resolved for task '{}'; halting loop.", name_clone);
                            break;
                        }
                    };

                    let next_time = scheduler.next_execution().unwrap();
                    tracing::info!(
                        task = %name_clone,
                        next_execution = %next_time.format("%Y-%m-%d %H:%M:%S UTC"),
                        wait_seconds = sleep_duration.as_secs(),
                        "Task scheduler waiting for next fire event"
                    );

                    tokio::select! {
                        _ = tokio::time::sleep(sleep_duration) => {
                            tracing::info!("Scheduled interval elapsed for task '{}'. Invoking backup execution.", name_clone);
                            crate::service_handler::run_task_backup_job(&name_clone, state_clone.clone()).await;
                        }
                        _ = &mut task_shutdown_rx => {
                            tracing::info!("Scheduler runner for task '{}' received cancel signal. Exiting.", name_clone);
                            break;
                        }
                    }
                }
            });

            runners.push((task_name, handle, task_shutdown_tx));
        }
    };

    // Initial spawn
    spawn_runners(state.clone(), &mut active_runners);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!("Global shutdown signal received in multi-scheduler coordinator. Cancelling all task runners.");
                for (_, _, shutdown_tx) in active_runners {
                    let _ = shutdown_tx.send(());
                }
                break;
            }
            Ok(_) = reload_rx.recv() => {
                tracing::info!("Configuration reload event received. Cancelling existing task runners and respawning...");
                for (_, _, shutdown_tx) in active_runners.drain(..) {
                    let _ = shutdown_tx.send(());
                }
                // Respawn with new configuration
                spawn_runners(state.clone(), &mut active_runners);
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    use backup_agent_core::domain::config::AppConfig;

    #[test]
    fn parses_24h_hour_format() {
        let scheduler = BackupScheduler::new("02:30").unwrap();
        
        let next = scheduler.next_execution().unwrap();
        assert_eq!(next.minute(), 30);
        assert_eq!(next.hour(), 2);
        assert_eq!(next.second(), 0);
    }

    #[test]
    fn parses_advanced_cron() {
        // Every 5 minutes: sec min hour dom month dom year (standard 6 or 7 fields)
        let scheduler = BackupScheduler::new("0 */5 * * * * *").unwrap();
        
        let next = scheduler.next_execution().unwrap();
        assert_eq!(next.second(), 0);
        assert!(next.minute() % 5 == 0, "Minute should be multiple of 5");
    }

    #[test]
    fn returns_err_on_invalid_bounds() {
        assert!(BackupScheduler::new("24:00").is_err(), "Hour 24 is out of bounds");
        assert!(BackupScheduler::new("12:60").is_err(), "Minute 60 is out of bounds");
        assert!(BackupScheduler::new("invalid").is_err(), "Random string is invalid");
    }

    #[test]
    fn calculates_positive_duration() {
        let scheduler = BackupScheduler::new("0 0 12 * * * *").unwrap();
        let dur = scheduler.duration_until_next();
        assert!(dur.is_some());
        assert!(dur.unwrap().as_secs() > 0);
    }

    #[tokio::test]
    async fn scheduler_exits_loop_on_shutdown() {
        let (reload_tx, _) = tokio::sync::broadcast::channel(16);
        let state = std::sync::Arc::new(crate::service_handler::DaemonState {
            exe_dir: std::path::PathBuf::from("/tmp"),
            config: tokio::sync::Mutex::new(AppConfig::default()),
            active_jobs: tokio::sync::Mutex::new(Vec::new()),
            history: tokio::sync::Mutex::new(Vec::new()),
            reload_tx,
        });

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            start_multi_scheduler(state, shutdown_rx).await;
        });

        // Trigger shutdown immediately
        shutdown_tx.send(()).unwrap();

        // The scheduler loop should terminate and join quickly without sleeping for hours
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("Scheduler coordinator failed to exit loop on shutdown signal")
            .unwrap();
    }
}
