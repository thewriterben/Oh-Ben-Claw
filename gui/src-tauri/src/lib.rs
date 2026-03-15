mod commands;
mod state;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, WindowEvent,
};
use tracing_subscriber::{fmt, EnvFilter};

pub fn run() {
    // Initialize logging
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("oh_ben_claw=info,warn")),
        )
        .init();

    tauri::Builder::default()
        // ── Plugins ──────────────────────────────────────────────────────────
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        // ── App State ─────────────────────────────────────────────────────────
        .manage(state::AppState::new())
        // ── Commands ──────────────────────────────────────────────────────────
        .invoke_handler(tauri::generate_handler![
            // Agent
            commands::send_message,
            commands::get_agent_status,
            commands::start_agent,
            commands::stop_agent,
            // Sessions
            commands::list_sessions,
            commands::create_session,
            commands::load_session_history,
            commands::clear_session,
            commands::delete_session,
            // Nodes
            commands::list_nodes,
            commands::add_node,
            commands::remove_node,
            commands::scan_usb_devices,
            // Tool log
            commands::get_tool_log,
            commands::clear_tool_log,
            // Vault
            commands::get_vault_status,
            commands::unlock_vault,
            commands::lock_vault,
            commands::list_vault_secrets,
            commands::set_vault_secret,
            commands::delete_vault_secret,
            // Settings
            commands::get_settings,
            commands::save_settings,
        ])
        // ── Setup ─────────────────────────────────────────────────────────────
        .setup(|app| {
            // Build system tray
            let show = MenuItem::with_id(app, "show", "Show Oh-Ben-Claw", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Oh-Ben-Claw")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
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
        // ── Window close → minimize to tray ───────────────────────────────────
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Check if minimize-to-tray is enabled in settings
                // For now, always minimize to tray (configurable in settings)
                window.hide().unwrap_or_default();
                api.prevent_close();
            }
        })
        // ── Run ───────────────────────────────────────────────────────────────
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
