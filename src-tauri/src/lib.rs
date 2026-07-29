mod commands;
mod config;
mod cron;
mod memory;
mod server;
mod ssh;

use commands::AppState;
use config::RetanaConfig;
use cron::CronService;
use memory::MemoryStore;
use ssh::manager::{ReverseTunnelConfig, SshConfig};
use ssh::reverse_tunnel;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::Manager;

/// Try to auto-connect SSH + reverse tunnel on startup.
/// Runs in background; retries if Hermes server isn't reachable yet.
fn auto_connect(ssh_manager: Arc<ssh::manager::SshManager>, shutdown: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        // Brief delay so WS server is up before tunnel tries to forward
        std::thread::sleep(std::time::Duration::from_secs(2));

        let config = match RetanaConfig::load() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Failed to load retana config: {}", e);
                return;
            }
        };

        if !config.auto_connect {
            log::info!("Auto-connect disabled in config");
            return;
        }

        log::info!(
            "Auto-connecting to {}@{}:{} ...",
            config.hermes_user,
            config.hermes_host,
            config.hermes_port
        );

        let ssh_config = SshConfig {
            host: config.hermes_host.clone(),
            port: config.hermes_port,
            username: config.hermes_user.clone(),
            key_path: config.hermes_key.clone(),
            password: None,
            reverse_tunnel: Some(ReverseTunnelConfig {
                remote_port: config.tunnel_remote_port,
                local_port: config.tunnel_local_port,
            }),
        };

        // Retry loop
        let max_retries = 10;
        for attempt in 1..=max_retries {
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            match ssh_manager.connect_with_config(&ssh_config) {
                Ok(()) => {
                    log::info!("✅ SSH connected on attempt {}", attempt);

                    // Start reverse tunnel
                    if let Some(session) = ssh_manager.session_arc() {
                        match reverse_tunnel::start_reverse_tunnel(
                            session,
                            config.tunnel_remote_port,
                            config.tunnel_local_port,
                            Arc::clone(&shutdown),
                        ) {
                            Ok(()) => {
                                log::info!("✅ Reverse tunnel established");
                            }
                            Err(e) => {
                                log::error!("Reverse tunnel failed: {}", e);
                            }
                        }
                    }
                    return; // Success
                }
                Err(e) => {
                    log::warn!(
                        "SSH connect attempt {}/{} failed: {}",
                        attempt,
                        max_retries,
                        e
                    );
                    if attempt < max_retries {
                        let delay = std::time::Duration::from_secs(5 * attempt);
                        log::info!("Retrying in {}s...", delay.as_secs());
                        std::thread::sleep(delay);
                    }
                }
            }
        }

        log::error!("Failed to auto-connect after {} attempts", max_retries);
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let ssh_manager = Arc::new(ssh::manager::SshManager::new(SshConfig {
        host: "localhost".into(),
        port: 22,
        username: "user".into(),
        key_path: None,
        password: None,
        reverse_tunnel: None,
    }));
    let cron_service = Arc::new(CronService::new());
    let memory_store = Arc::new(Mutex::new(MemoryStore::load()));
    let shutdown = Arc::new(AtomicBool::new(false));

    // Start the local WebSocket server + bridge proxy
    let server_shutdown = Arc::clone(&shutdown);
    let server_port: u16 = 9000;
    let bridge_url = "ws://115.159.116.195:9001/ws".to_string();
    let url_for_log = bridge_url.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new()
            .expect("Failed to create server Tokio runtime");
        rt.block_on(async {
            if let Err(e) = server::run_server(server_port, &bridge_url, server_shutdown).await {
                log::error!("WebSocket server error: {}", e);
            }
        });
    });
    log::info!("Local WS server on port {}, proxying to {}", server_port, url_for_log);

    // Auto-connect SSH + reverse tunnel
    auto_connect(Arc::clone(&ssh_manager), Arc::clone(&shutdown));

    let app_state = AppState {
        ssh: ssh_manager,
        cron: cron_service.clone(),
        memory: memory_store,
        shutdown: Arc::clone(&shutdown),
    };

    let app_shutdown = Arc::clone(&shutdown);

    tauri::Builder::default()
        .manage(app_state)
        .setup(move |app| {
            cron_service.start();
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

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
        .on_window_event(move |_window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                app_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
            }
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
            commands::exec_local,
        ])
        .run(tauri::generate_context!())
        .expect("error while running retana");
}
