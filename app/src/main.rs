#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod engine;
mod modules;
mod placement;
mod settings;
mod windows;

use std::sync::Arc;

use tauri::{
    Manager, WindowEvent,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_global_shortcut::{Builder as Shortcuts, ShortcutState};

const HOTKEY: &str = "Ctrl+Alt+L";

fn open_settings(app: &tauri::AppHandle) {
    if let Err(e) = windows::show_settings(app) {
        eprintln!("settings: {e}");
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let file = settings::SettingsFile(app.path().app_config_dir()?.join("settings.json"));
    let models = app.path().app_data_dir()?.join("models");
    let secrets = Box::new(settings::Keychain("dev.openlang.app"));
    let engine = Arc::new(engine::Engine::new(
        app.handle().clone(),
        modules::registry(&models),
        file,
        secrets,
    ));
    let settings = engine.settings();
    app.manage(windows::Desk::default());
    app.manage::<commands::Shared>(engine);
    windows::float(app.handle());
    windows::arrange(app.handle(), &settings);
    windows::watch_displays(app.handle());

    let toggle = MenuItem::with_id(app, "toggle", "Start / Stop  ⌃⌥L", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit OpenLang", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&toggle, &open, &line, &quit])?;
    TrayIconBuilder::with_id("tray")
        .title("OpenLang")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => commands::toggle(app),
            "settings" => open_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn main() {
    let shortcuts = Shortcuts::new().with_handler(|app, _, event| {
        if event.state() == ShortcutState::Pressed {
            commands::toggle(app);
        }
    });
    // Another app may already own the hotkey; everything else still works.
    let shortcuts = match shortcuts.with_shortcut(HOTKEY) {
        Ok(s) => s.build(),
        Err(e) => {
            eprintln!("hotkey {HOTKEY}: {e}");
            Shortcuts::new().build()
        }
    };
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            open_settings(app)
        }))
        .plugin(shortcuts)
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::current,
            commands::save,
            commands::set_secret,
            commands::start,
            commands::stop,
            commands::show_settings,
            commands::set_visible,
            commands::resize_island,
            commands::drag_start,
            commands::drag_move,
            commands::drag_end,
            commands::set_preset,
            commands::preview,
            commands::open_privacy,
        ])
        .on_window_event(|window, event| match (window.label(), event) {
            ("settings", WindowEvent::CloseRequested { .. }) => {
                windows::preview(window.app_handle(), false);
            }
            ("caption", WindowEvent::Resized(_)) => windows::remember_size(window.app_handle()),
            ("caption" | "island", WindowEvent::CloseRequested { api, .. }) => api.prevent_close(),
            _ => {}
        })
        .setup(setup)
        .build(tauri::generate_context!());
    match result {
        Ok(app) => run(app),
        Err(e) => {
            eprintln!("OpenLang could not start: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
fn run(mut app: tauri::App) {
    // Must come before any window exists: a window born in a Dock app cannot float over another
    // app's full-screen space.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    app.run(|_, _| {});
}
