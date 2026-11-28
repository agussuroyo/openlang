use std::sync::Arc;

use openlang_core::{
    Error, Result,
    auto_language::to_prepare,
    ports::{Descriptor, FieldKind, Job, Languages, Location, Provider, Readiness},
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::{
    engine::{Engine, Status},
    hotkey,
    placement::{Edge, Place},
    settings::{Settings, secret_key},
    windows::{self, Floating},
};

pub type Shared = Arc<Engine>;

#[derive(Serialize)]
pub struct SourceInfo {
    descriptor: Descriptor,
}

#[derive(Serialize)]
pub struct ProviderInfo {
    descriptor: Descriptor,
    location: Location,
    languages: Languages,
    readiness: Readiness,
    /// Keys of secrets already stored. Their values never reach the window.
    stored_secrets: Vec<&'static str>,
}

#[derive(Serialize)]
pub struct Snapshot {
    settings: Settings,
    /// False when another app owns the shortcut.
    hotkey_works: bool,
    sources: Vec<SourceInfo>,
    stt: Vec<ProviderInfo>,
    translators: Vec<ProviderInfo>,
}

/// Runs blocking work off the main thread.
async fn background<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(Error::Provider(e.to_string())))
}

/// `detector` finds the language for a recognizer that cannot.
fn info(
    e: &Engine,
    s: &Settings,
    p: &dyn Provider,
    detector: Option<&dyn Provider>,
) -> ProviderInfo {
    let d = p.descriptor().clone();
    // Without a system keychain (Linux with no Secret Service) Settings still opens.
    let options = s
        .options_for(&d, e.secrets.as_ref())
        .unwrap_or_else(|_| s.options.get(d.id).cloned().unwrap_or_default());
    let stored_secrets = d
        .fields
        .iter()
        .filter(|f| f.kind == FieldKind::Secret && options.contains_key(f.key))
        .map(|f| f.key)
        .collect();
    let location = p.location(&options);
    let job = Job {
        source: s.source_language.clone(),
        target: s.target_language.clone(),
        options,
    };
    let mut languages = p.languages();
    languages.can_detect |= detector.is_some();
    ProviderInfo {
        location,
        languages,
        readiness: to_prepare(p, detector, &job).readiness(&job),
        stored_secrets,
        descriptor: d,
    }
}

#[tauri::command]
pub async fn snapshot(app: AppHandle, engine: State<'_, Shared>) -> Result<Snapshot> {
    let e = engine.inner().clone();
    background(move || {
        let s = e.settings();
        Ok(Snapshot {
            hotkey_works: hotkey::works(&app, &s.hotkey),
            sources: e
                .registry
                .sources
                .iter()
                .map(|x| SourceInfo {
                    descriptor: x.descriptor().clone(),
                })
                .collect(),
            stt: e
                .registry
                .stt
                .iter()
                .map(|p| info(&e, &s, p.as_ref(), e.detector()))
                .collect(),
            translators: e
                .registry
                .translators
                .iter()
                .map(|p| info(&e, &s, p.as_ref(), None))
                .collect(),
            settings: s,
        })
    })
    .await
}

/// Cheap state for a window that just loaded.
#[tauri::command]
pub fn current(engine: State<'_, Shared>) -> (Settings, Status) {
    (engine.settings(), engine.status())
}

#[tauri::command]
pub async fn save(engine: State<'_, Shared>, settings: Settings) -> Result<()> {
    let e = engine.inner().clone();
    background(move || e.save(settings)).await
}

#[tauri::command]
pub async fn set_secret(
    engine: State<'_, Shared>,
    provider: String,
    key: String,
    value: String,
) -> Result<()> {
    let e = engine.inner().clone();
    background(move || e.secrets.set(&secret_key(&provider, &key), &value)).await
}

#[tauri::command]
pub async fn start(engine: State<'_, Shared>) -> Result<()> {
    let e = engine.inner().clone();
    background(move || e.start()).await
}

#[tauri::command]
pub async fn stop(engine: State<'_, Shared>) -> Result<()> {
    let e = engine.inner().clone();
    background(move || {
        e.stop();
        Ok(())
    })
    .await
}

/// Async so the window is not built inside a synchronous webview callback.
#[tauri::command]
pub async fn show_settings(app: AppHandle) -> Result<()> {
    windows::show_settings(&app)
}

#[tauri::command]
pub fn set_visible(app: AppHandle, window: Floating, visible: bool) {
    windows::set_visible(&app, window, visible);
}

#[tauri::command]
pub fn resize_island(app: AppHandle, engine: State<'_, Shared>, width: f64, height: f64) {
    windows::resize_island(&app, &engine.settings(), (width, height));
}

#[tauri::command]
pub fn drag_start(app: AppHandle, window: Floating) {
    windows::drag_start(&app, window);
}

#[tauri::command]
pub fn drag_move(app: AppHandle, dx: f64, dy: f64) {
    windows::drag_move(&app, dx, dy);
}

#[tauri::command]
pub async fn drag_end(app: AppHandle, engine: State<'_, Shared>) -> Result<()> {
    match windows::drag_end(&app) {
        Some((which, place)) => keep_place(engine.inner().clone(), which, place).await,
        None => Ok(()),
    }
}

#[tauri::command]
pub async fn set_preset(
    app: AppHandle,
    engine: State<'_, Shared>,
    window: Floating,
    h: Edge,
    v: Edge,
) -> Result<()> {
    let place = windows::preset(&app, window, &engine.settings(), h, v);
    keep_place(engine.inner().clone(), window, place).await
}

async fn keep_place(engine: Shared, which: Floating, place: Place) -> Result<()> {
    background(move || {
        let mut s = engine.settings();
        match which {
            Floating::Caption => s.caption_place = place,
            Floating::Island => s.island_place = place,
        }
        engine.save(s)
    })
    .await
}

#[tauri::command]
pub fn preview(app: AppHandle, on: bool) {
    windows::preview(&app, on);
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyPane {
    SystemAudio,
    Microphone,
}

/// The system's privacy settings page for each kind of capture it asks about.
#[cfg(target_os = "macos")]
const PRIVACY: &[(PrivacyPane, &str)] = &[
    (
        PrivacyPane::SystemAudio,
        "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture",
    ),
    (
        PrivacyPane::Microphone,
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
    ),
];
/// Windows asks only about the microphone; recording the output needs no permission.
#[cfg(windows)]
const PRIVACY: &[(PrivacyPane, &str)] =
    &[(PrivacyPane::Microphone, "ms-settings:privacy-microphone")];
/// Linux does not ask.
#[cfg(not(any(target_os = "macos", windows)))]
const PRIVACY: &[(PrivacyPane, &str)] = &[];

/// What the windows word and offer differently per system.
#[derive(Serialize)]
pub struct Platform {
    os: &'static str,
    privacy: Vec<PrivacyPane>,
}

#[tauri::command]
pub fn platform() -> Platform {
    Platform {
        os: std::env::consts::OS,
        privacy: PRIVACY.iter().map(|(pane, _)| *pane).collect(),
    }
}

/// Opens the system's privacy settings for one kind of capture.
#[tauri::command]
pub fn open_privacy(pane: PrivacyPane) -> Result<()> {
    let (_, page) = PRIVACY
        .iter()
        .find(|(p, _)| *p == pane)
        .ok_or_else(|| Error::Config("this system has no privacy page for it".into()))?;
    let program = if cfg!(windows) { "explorer" } else { "open" };
    std::process::Command::new(program)
        .arg(page)
        .spawn()
        .map(drop)
        .map_err(|e| Error::Config(e.to_string()))
}

pub fn toggle(app: &AppHandle) {
    let e = app.state::<Shared>().inner().clone();
    std::thread::spawn(move || e.toggle());
}
