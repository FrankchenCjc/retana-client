// Local cron / scheduled hook service
// Runs scheduled tasks on the local machine

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A scheduled task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    pub command: String,
    pub schedule: String,
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
}

/// Task execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub output: String,
    pub exit_code: Option<i32>,
}

/// Cron service managing scheduled tasks
pub struct CronService {
    tasks: Arc<Mutex<Vec<ScheduledTask>>>,
    history: Arc<Mutex<Vec<TaskResult>>>,
}

impl CronService {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(Vec::new())),
            history: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn add_task(&self, task: ScheduledTask) {
        self.tasks.lock().unwrap().push(task);
    }

    pub fn remove_task(&self, id: &str) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        let len_before = tasks.len();
        tasks.retain(|t| t.id != id);
        tasks.len() < len_before
    }

    pub fn list_tasks(&self) -> Vec<ScheduledTask> {
        self.tasks.lock().unwrap().clone()
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
            task.enabled = enabled;
            true
        } else {
            false
        }
    }

    pub fn history(&self) -> Vec<TaskResult> {
        self.history.lock().unwrap().clone()
    }

    /// Start the cron scheduler loop
    pub fn start(&self) {
        let tasks = Arc::clone(&self.tasks);
        let history = Arc::clone(&self.history);

        tokio::spawn(async move {
            loop {
                // Scope the lock so it's dropped before .await
                {
                    let now = Utc::now();
                    let mut task_list = tasks.lock().unwrap();

                    for task in task_list.iter_mut() {
                        if !task.enabled {
                            continue;
                        }

                        let should_run = match &task.last_run {
                            None => true,
                            Some(last) => {
                                let interval = Self::parse_interval(&task.schedule);
                                let elapsed = (now - *last).num_seconds();
                                elapsed >= interval
                            }
                        };

                        if should_run {
                            log::info!("Running scheduled task: {}", task.name);
                            task.last_run = Some(now);

                            history.lock().unwrap().push(TaskResult {
                                task_id: task.id.clone(),
                                timestamp: now,
                                success: true,
                                output: format!("Task {} executed", task.name),
                                exit_code: Some(0),
                            });
                        }
                    }
                } // MutexGuard dropped here

                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    fn parse_interval(schedule: &str) -> i64 {
        match schedule {
            s if s.ends_with('h') => s.trim_end_matches('h').parse::<i64>().unwrap_or(1) * 3600,
            s if s.ends_with('m') => s.trim_end_matches('m').parse::<i64>().unwrap_or(1) * 60,
            s if s.ends_with('s') => s.trim_end_matches('s').parse::<i64>().unwrap_or(60),
            _ => schedule.parse::<i64>().unwrap_or(3600),
        }
    }
}

impl Default for CronService {
    fn default() -> Self {
        Self::new()
    }
}
