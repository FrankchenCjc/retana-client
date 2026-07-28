// Tauri IPC commands — bridge between frontend and backend

use crate::cron::CronService;
use crate::memory::MemoryStore;
use crate::ssh::manager::{ConnectionStatus, SshConfig, SshManager};
use std::sync::Arc;
use tauri::State;

/// Application state shared across Tauri commands
pub struct AppState {
    pub ssh: Arc<SshManager>,
    pub cron: Arc<CronService>,
    pub memory: Arc<std::sync::Mutex<MemoryStore>>,
}

// ─── SSH Commands ───

#[tauri::command]
pub async fn ssh_connect(state: State<'_, AppState>, _config: SshConfig) -> Result<String, String> {
    // Update the manager config by reconnecting
    match state.ssh.connect() {
        Ok(()) => Ok("Connected successfully".to_string()),
        Err(e) => Err(format!("Connection failed: {}", e)),
    }
}

#[tauri::command]
pub async fn ssh_disconnect(state: State<'_, AppState>) -> Result<String, String> {
    match state.ssh.disconnect() {
        Ok(()) => Ok("Disconnected".to_string()),
        Err(e) => Err(format!("Disconnect failed: {}", e)),
    }
}

#[tauri::command]
pub async fn ssh_exec(state: State<'_, AppState>, command: String) -> Result<String, String> {
    state.ssh.exec(&command).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ssh_status(state: State<'_, AppState>) -> Result<ConnectionStatus, String> {
    Ok(state.ssh.status())
}

// ─── Cron Commands ───

#[tauri::command]
pub async fn cron_list(state: State<'_, AppState>) -> Result<Vec<crate::cron::ScheduledTask>, String> {
    Ok(state.cron.list_tasks())
}

#[tauri::command]
pub async fn cron_add(
    state: State<'_, AppState>,
    task: crate::cron::ScheduledTask,
) -> Result<String, String> {
    state.cron.add_task(task);
    Ok("Task added".to_string())
}

#[tauri::command]
pub async fn cron_remove(state: State<'_, AppState>, id: String) -> Result<String, String> {
    if state.cron.remove_task(&id) {
        Ok("Task removed".to_string())
    } else {
        Err("Task not found".to_string())
    }
}

#[tauri::command]
pub async fn cron_toggle(state: State<'_, AppState>, id: String, enabled: bool) -> Result<String, String> {
    if state.cron.set_enabled(&id, enabled) {
        Ok(format!("Task {} {}", id, if enabled { "enabled" } else { "disabled" }))
    } else {
        Err("Task not found".to_string())
    }
}

#[tauri::command]
pub async fn cron_history(state: State<'_, AppState>) -> Result<Vec<crate::cron::TaskResult>, String> {
    Ok(state.cron.history())
}

// ─── Memory Commands ───

#[tauri::command]
pub async fn memory_get(state: State<'_, AppState>, key: String) -> Result<String, String> {
    let store = state.memory.lock().unwrap();
    store
        .get(&key)
        .map(|v| v.to_string())
        .ok_or_else(|| format!("Key '{}' not found", key))
}

#[tauri::command]
pub async fn memory_set(
    state: State<'_, AppState>,
    key: String,
    value: String,
    category: String,
) -> Result<String, String> {
    let mut store = state.memory.lock().unwrap();
    store.set(&key, &value, &category);
    store.save().map_err(|e| format!("Save failed: {}", e))?;
    Ok("Memory updated".to_string())
}

#[tauri::command]
pub async fn memory_list(state: State<'_, AppState>) -> Result<Vec<crate::memory::MemoryEntry>, String> {
    let store = state.memory.lock().unwrap();
    Ok(store.all_entries().to_vec())
}

#[tauri::command]
pub async fn memory_list_category(
    state: State<'_, AppState>,
    category: String,
) -> Result<Vec<crate::memory::MemoryEntry>, String> {
    let store = state.memory.lock().unwrap();
    Ok(store
        .list_by_category(&category)
        .iter()
        .map(|e| (*e).clone())
        .collect())
}

// ─── System Info ───

#[tauri::command]
pub async fn system_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "hostname": hostname(),
    }))
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| "unknown".to_string())
        .trim()
        .to_string()
}
