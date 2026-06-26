//! Poe2Obsidian desktop GUI (Tauri v2).
//!
//! Wraps the headless `ravenvault` core: runs the WebSocket server in the
//! background and provides a system-tray icon (Settings / Open Vault / Quit) plus
//! a settings window that edits `~/.config/ravenvault/config.json`.
//!
//! Note: this is an *alternative* to the systemd service — both bind
//! `127.0.0.1:53122`, so only one should run at a time.

// Don't spawn a console window on Windows (harmless on Linux).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use ravenvault::context::{self, AppContext, FileConfig};
use ravenvault::{server, WS_BIND_ADDR};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, WindowEvent,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// Return the current on-disk configuration to the settings window.
#[tauri::command]
fn get_config() -> FileConfig {
    context::current_file_config()
}

/// Persist configuration edited in the settings window.
#[tauri::command]
fn save_config(cfg: FileConfig) -> Result<(), String> {
    context::save_file_config(&cfg).map_err(|e| e.to_string())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_config, save_config])
        .setup(|app| {
            // Run the WebSocket server the extension connects to.
            let ctx = Arc::new(AppContext::load());
            match &ctx.vault_path {
                Some(p) => info!(vault = %p.display(), "vault configured"),
                None => info!("no vault configured yet (open Settings)"),
            }
            tauri::async_runtime::spawn(async move {
                match server::bind(WS_BIND_ADDR).await {
                    Ok(listener) => {
                        info!(addr = WS_BIND_ADDR, "WebSocket server listening");
                        if let Err(e) = server::serve(listener, ctx).await {
                            error!(error = %e, "server stopped");
                        }
                    }
                    Err(e) => error!(
                        error = %e,
                        "could not bind {WS_BIND_ADDR} — is the ravenvault service already running?"
                    ),
                }
            });

            // System tray with a menu.
            let settings_i = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let open_vault_i =
                MenuItem::with_id(app, "open_vault", "Open Vault", true, None::<&str>)?;
            let ingest_i = MenuItem::with_id(
                app,
                "ingest",
                "Ingest vault → MemPalace",
                true,
                None::<&str>,
            )?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_i, &open_vault_i, &ingest_i, &quit_i])?;

            let tray = TrayIconBuilder::with_id("main")
                .tooltip("Poe2Obsidian")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => {
                        if let Some(w) = app.get_webview_window("settings") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "open_vault" => {
                        if let Some(path) = context::current_file_config().vault_path {
                            let _ = std::process::Command::new("xdg-open").arg(path).spawn();
                        }
                    }
                    "ingest" => {
                        // Run the (potentially long) mine in the background.
                        let cfg = context::current_file_config();
                        if let Some(vault) = cfg.vault_path.filter(|s| !s.is_empty()) {
                            let mp = ravenvault::mempalace::MemPalaceConfig {
                                enabled: true,
                                binary: cfg
                                    .mempalace_bin
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or_else(|| "mempalace".to_string()),
                                ..Default::default()
                            };
                            tauri::async_runtime::spawn(async move {
                                info!("ingesting vault into MemPalace…");
                                ravenvault::mempalace::ingest_best_effort(
                                    &mp,
                                    std::path::Path::new(&vault),
                                )
                                .await;
                            });
                        } else {
                            warn!("no vault configured; open Settings first");
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });

            // Use the bundled app icon for the tray when available. On vanilla
            // GNOME (no AppIndicator) the tray may not render — the settings
            // window is still reachable, so this is a soft failure.
            let tray = match app.default_window_icon() {
                Some(icon) => tray.icon(icon.clone()),
                None => tray,
            };
            tray.build(app)?;

            Ok(())
        })
        // Closing the settings window hides it instead of quitting the app.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Poe2Obsidian");
}
