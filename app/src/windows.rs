use std::sync::{Mutex, MutexGuard, PoisonError};

use openlang_core::{Error, Result};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::{
    commands::Shared,
    placement::{self, Edge, Monitor, Place, Rect},
    settings::Settings,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Floating {
    Caption,
    Island,
}

impl Floating {
    #[cfg(target_os = "macos")]
    const ALL: [Self; 2] = [Self::Caption, Self::Island];

    pub fn label(self) -> &'static str {
        match self {
            Self::Caption => "caption",
            Self::Island => "island",
        }
    }

    fn place(self, s: &Settings) -> &Place {
        match self {
            Self::Caption => &s.caption_place,
            Self::Island => &s.island_place,
        }
    }

    fn window(self, app: &AppHandle) -> Option<WebviewWindow> {
        app.get_webview_window(self.label())
    }
}

/// Size the island asked for, and where a running drag began.
#[derive(Default)]
pub struct Desk {
    island: Mutex<(f64, f64)>,
    drag: Mutex<Option<(Floating, Rect)>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn monitors(app: &AppHandle) -> Vec<Monitor> {
    let main = app
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().cloned());
    let mut all: Vec<_> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let (area, scale) = (m.work_area(), m.scale_factor());
            let at = area.position.to_logical::<f64>(scale);
            let size = area.size.to_logical::<f64>(scale);
            Monitor {
                name: m.name().cloned(),
                area: Rect {
                    x: at.x,
                    y: at.y,
                    w: size.width,
                    h: size.height,
                },
            }
        })
        .collect();
    all.sort_by_key(|m| m.name != main);
    all
}

fn rect(w: &WebviewWindow) -> Option<Rect> {
    let scale = w.scale_factor().ok()?;
    let at = w.outer_position().ok()?.to_logical::<f64>(scale);
    let size = w.inner_size().ok()?.to_logical::<f64>(scale);
    Some(Rect {
        x: at.x,
        y: at.y,
        w: size.width,
        h: size.height,
    })
}

fn put(w: &WebviewWindow, r: Rect) {
    let _ = w.set_size(LogicalSize::new(r.w, r.h));
    let _ = w.set_position(LogicalPosition::new(r.x, r.y));
}

/// Moves both floating windows to where the settings say.
pub fn arrange(app: &AppHandle, s: &Settings) {
    if lock(&app.state::<Desk>().drag).is_some() {
        return;
    }
    let screens = monitors(app);
    let caption = s.caption_place.resolve(s.caption_size, &screens);
    if let (Some(w), Some(r)) = (Floating::Caption.window(app), caption) {
        let _ = w.set_ignore_cursor_events(s.click_through);
        put(&w, r);
    }
    let size = *lock(&app.state::<Desk>().island);
    if let (Some(w), Some(mut r)) = (
        Floating::Island.window(app),
        s.island_place.resolve(size, &screens),
    ) {
        if let Some(c) = caption {
            r = placement::beside(r, c, &screens);
        }
        put(&w, r);
        let (h, v) = s.island_place.edges();
        let _ = app.emit_to(Floating::Island.label(), "edges", (h, v));
    }
}

/// Puts the windows back when a display is plugged, unplugged, resized or wakes up. While every
/// display sleeps the list is empty and nothing can be placed.
pub fn watch_displays(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut known = Vec::new();
        loop {
            let now = monitors(&app);
            if !now.is_empty() && now != known {
                known = now;
                arrange(&app, &app.state::<Shared>().settings());
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

pub fn resize_island(app: &AppHandle, s: &Settings, size: (f64, f64)) {
    *lock(&app.state::<Desk>().island) = size;
    arrange(app, s);
}

pub fn set_visible(app: &AppHandle, which: Floating, visible: bool) {
    if let Some(w) = which.window(app) {
        let _ = if visible { w.show() } else { w.hide() };
    }
}

pub fn drag_start(app: &AppHandle, which: Floating) {
    if let Some(r) = which.window(app).as_ref().and_then(rect) {
        *lock(&app.state::<Desk>().drag) = Some((which, r));
    }
}

pub fn drag_move(app: &AppHandle, dx: f64, dy: f64) {
    let Some((which, from)) = *lock(&app.state::<Desk>().drag) else {
        return;
    };
    let moved = Rect {
        x: from.x + dx,
        y: from.y + dy,
        ..from
    };
    if let Some(w) = which.window(app) {
        let r = placement::snap(moved, &monitors(app));
        let _ = w.set_position(LogicalPosition::new(r.x, r.y));
    }
}

/// Ends a drag and returns the new place to remember.
pub fn drag_end(app: &AppHandle) -> Option<(Floating, Place)> {
    let (which, _) = lock(&app.state::<Desk>().drag).take()?;
    let r = which.window(app).as_ref().and_then(rect)?;
    Some((which, Place::dropped(r, &monitors(app))?))
}

/// The caption window's size after the user resized it.
pub fn caption_size(app: &AppHandle) -> Option<(f64, f64)> {
    Floating::Caption
        .window(app)
        .as_ref()
        .and_then(rect)
        .map(|r| (r.w, r.h))
}

/// A preset applies to the display the window is on now.
pub fn preset(app: &AppHandle, which: Floating, s: &Settings, h: Edge, v: Edge) -> Place {
    let screens = monitors(app);
    let now = which
        .place(s)
        .resolve(s.caption_size, &screens)
        .and_then(|r| placement::nearest(r, &screens));
    Place::Preset {
        h,
        v,
        monitor: now.and_then(|m| m.name.clone()),
    }
}

/// Built when needed and destroyed on close, so it costs no memory meanwhile.
pub fn show_settings(app: &AppHandle) -> Result<()> {
    let fail = |e: tauri::Error| Error::Config(e.to_string());
    let window = match app.get_webview_window("settings") {
        Some(w) => w,
        None => blend_title_bar(WebviewWindowBuilder::new(
            app,
            "settings",
            WebviewUrl::App("settings/index.html".into()),
        ))
        .title("OpenLang Settings")
        .inner_size(760.0, 600.0)
        .min_inner_size(640.0, 480.0)
        .build()
        .map_err(fail)?,
    };
    window.show().map_err(fail)?;
    window.set_focus().map_err(fail)
}

type Builder<'a> = WebviewWindowBuilder<'a, tauri::Wry, AppHandle>;

/// macOS draws the window buttons over the page. Other systems keep their own title bar.
#[cfg(target_os = "macos")]
fn blend_title_bar(builder: Builder) -> Builder {
    builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
}

#[cfg(not(target_os = "macos"))]
fn blend_title_bar(builder: Builder) -> Builder {
    builder
}

/// Shows or removes the sample caption that lets people see where subtitles will be.
pub fn preview(app: &AppHandle, on: bool) {
    let _ = app.emit_to(Floating::Caption.label(), "preview", on);
}

/// Keeps the floating windows above other apps, including their full-screen spaces.
#[cfg(target_os = "macos")]
pub fn float(app: &AppHandle) {
    use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior as B};
    for w in Floating::ALL.iter().filter_map(|f| f.window(app)) {
        let Ok(raw) = w.ns_window() else { continue };
        // SAFETY: runs on the main thread during setup; both windows live as long as the app.
        let window = unsafe { &*raw.cast::<NSWindow>() };
        window.setCollectionBehavior(
            B::CanJoinAllSpaces | B::FullScreenAuxiliary | B::Stationary | B::IgnoresCycle,
        );
        window.setLevel(NSStatusWindowLevel);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn float(_: &AppHandle) {}

/// Remembers the caption size shortly after the user stops resizing.
pub fn remember_size(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let engine = app.state::<Shared>().inner().clone();
        let s = engine.settings();
        if let Some(size) = caption_size(&app)
            && ((size.0 - s.caption_size.0).abs() > 1.0 || (size.1 - s.caption_size.1).abs() > 1.0)
        {
            let place = rect_place(&app).unwrap_or(s.caption_place.clone());
            let _ = engine.save(Settings {
                caption_size: size,
                caption_place: place,
                ..s
            });
        }
    });
}

fn rect_place(app: &AppHandle) -> Option<Place> {
    let r = Floating::Caption.window(app).as_ref().and_then(rect)?;
    Place::dropped(r, &monitors(app))
}
