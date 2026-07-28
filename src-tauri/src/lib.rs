mod commands;
mod cron;
mod memory;
mod ssh;

use commands::AppState;
use cron::CronService;
use memory::MemoryStore;
use ssh::manager::SshConfig;
use std::sync::{Arc, Mutex};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize app services
    let ssh_config = SshConfig {
        host: "localhost".to_string(),
        port: 22,
        username: "user".to_string(),
        key_path: None,
        password: None,
        reverse_tunnel: None,
    };

    let ssh_manager = Arc::new(ssh::manager::SshManager::new(ssh_config));
    let cron_service = Arc::new(CronService::new());
    let memory_store = Arc::new(Mutex::new(MemoryStore::load()));

    let app_state = AppState {
        ssh: ssh_manager,
        cron: cron_service.clone(),
        memory: memory_store,
    };

    tauri::Builder::default()
        .manage(app_state)
        .setup(move |app| {
            // Start cron scheduler (Tokio runtime is ready here)
            cron_service.start();
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Set up system tray
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            let _tray = TrayIconBuilder::new()
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ssh_connect,
            commands::ssh_disconnect,
            commands::ssh_exec,
            commands::ssh_status,
            commands::cron_list,
            commands::cron_add,
            commands::cron_remove,
            commands::cron_toggle,
            commands::cron_history,
            commands::memory_get,
            commands::memory_set,
            commands::memory_list,
            commands::memory_list_category,
            commands::system_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running retana");
}
