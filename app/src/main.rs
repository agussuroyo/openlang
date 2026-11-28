#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod engine;
mod hotkey;
mod modules;
mod placement;
mod settings;
mod windows;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use tauri::{
    Manager, WindowEvent,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_global_shortcut::{Builder as Shortcuts, ShortcutState};

/// The identifier before `dev.openlang.desktop`; its folders are moved over once.
const OLD_IDENTIFIER: &str = "dev.openlang.app";
/// Where the system keychain keeps provider keys. It stays the same, so stored keys survive.
const KEYCHAIN: &str = "dev.openlang.app";

fn open_settings(app: &tauri::AppHandle) {
    if let Err(e) = windows::show_settings(app) {
        eprintln!("settings: {e}");
    }
}

/// Settings file and models folder. Models are large, so on Windows they stay out of the
/// roaming profile.
fn folders(app: &tauri::App) -> tauri::Result<(PathBuf, PathBuf)> {
    let path = app.path();
    let settings = path.app_config_dir()?.join("settings.json");
    let models = path.app_local_data_dir()?.join("models");
    let old = |dir: PathBuf| dir.with_file_name(OLD_IDENTIFIER);
    move_once(&old(path.app_data_dir()?).join("models"), &models);
    move_once(
        &old(path.app_config_dir()?).join("settings.json"),
        &settings,
    );
    for dir in [path.app_data_dir()?, path.app_config_dir()?] {
        let _ = fs::remove_dir(old(dir)); // Only when nothing else is left in it.
    }
    Ok((settings, models))
}

fn move_once(from: &Path, to: &Path) {
    if to.exists() || !from.exists() {
        return;
    }
    let moved = to
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::rename(from, to));
    if let Err(e) = moved {
        eprintln!("could not move {} to {}: {e}", from.display(), to.display());
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let (file, models) = folders(app)?;
    let file = settings::SettingsFile(file);
    let secrets = Box::new(settings::Keychain(KEYCHAIN));
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

    let toggle = MenuItem::with_id(app, "toggle", "Start / Stop", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit OpenLang", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&toggle, &open, &line, &quit])?;
    app.manage(hotkey::TrayEntry(toggle));
    // A taken shortcut leaves everything else working; Settings offers another.
    if let Err(e) = hotkey::replace(app.handle(), None, &settings.hotkey) {
        eprintln!("hotkey: {e}");
    }
    let tray = TrayIconBuilder::with_id("tray")
        .title("OpenLang")
        .tooltip("OpenLang");
    // The menu bar shows the title; Windows and Linux trays show only an icon.
    #[cfg(not(target_os = "macos"))]
    let tray = match app.default_window_icon() {
        Some(icon) => tray.icon(icon.clone()),
        None => tray,
    };
    tray.menu(&menu)
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
    // On Wayland, windows cannot place themselves or stay on top, and global shortcuts need
    // X11, so GTK runs through XWayland when it is there and the user has not chosen.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_some()
        && std::env::var_os("GDK_BACKEND").is_none()
    {
        // SAFETY: no other thread exists yet.
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
    }
    let shortcuts = Shortcuts::new()
        .with_handler(|app, _, event| {
            if event.state() == ShortcutState::Pressed {
                commands::toggle(app);
            }
        })
        .build();
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
            commands::platform,
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
