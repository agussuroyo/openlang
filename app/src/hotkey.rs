//! The global Start/Stop shortcut, chosen in Settings.

use std::str::FromStr;

use openlang_core::{Error, Result};
use tauri::{AppHandle, Manager, menu::MenuItem};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut};

/// Taken by nothing on macOS, Windows, GNOME, KDE, XFCE or Cinnamon. Ctrl+Alt+L locks the
/// screen on several Linux desktops and is AltGr+L (a letter) on Windows, Alt+Shift switches
/// the keyboard layout, Ctrl+Shift+letter belongs to apps, and Option+Shift+letter types on Mac.
pub const DEFAULT: &str = "Ctrl+Alt+Shift+Space";

/// The tray menu entry that shows the shortcut beside its name.
pub struct TrayEntry(pub MenuItem<tauri::Wry>);

pub fn parse(text: &str) -> Result<Shortcut> {
    let s = Shortcut::from_str(text).map_err(|e| Error::Config(format!("shortcut {text}: {e}")))?;
    if !s
        .mods
        .intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER)
    {
        return Err(Error::Config(
            "a shortcut needs Ctrl, Alt or the system key, so typing never triggers it".into(),
        ));
    }
    Ok(s)
}

/// Listens to `new` instead of `old`. When `new` cannot be had, `old` keeps working.
pub fn replace(app: &AppHandle, old: Option<&str>, new: &str) -> Result<()> {
    let wanted = parse(new)?;
    let keys = app.global_shortcut();
    let old = old
        .and_then(|o| parse(o).ok())
        .filter(|o| keys.is_registered(*o));
    if let Some(o) = old {
        let _ = keys.unregister(o);
    }
    if let Err(e) = keys.register(wanted) {
        if let Some(o) = old {
            let _ = keys.register(o);
        }
        return Err(Error::Config(format!(
            "{new} could not be used, another app may own it: {e}"
        )));
    }
    if let Some(entry) = app.try_state::<TrayEntry>() {
        let _ = entry.0.set_accelerator(Some(new));
    }
    Ok(())
}

/// False when another app owns the shortcut, so Settings can say so.
pub fn works(app: &AppHandle, text: &str) -> bool {
    parse(text).is_ok_and(|s| app.global_shortcut().is_registered(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_parses() {
        assert!(parse(DEFAULT).is_ok());
    }

    #[test]
    fn a_shortcut_without_ctrl_alt_or_super_is_refused() {
        assert!(parse("Shift+KeyL").is_err());
        assert!(parse("KeyL").is_err());
        assert!(parse("Super+KeyL").is_ok());
        assert!(parse("nonsense").is_err());
    }
}
