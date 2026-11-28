// The only place that talks to the Rust side, plus wording shared by both windows.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

export { invoke, listen };

const names = new Intl.DisplayNames(['en'], { type: 'language' });
export function languageName(tag) {
  if (!tag) return 'Detect automatically';
  try { return names.of(tag) ?? tag; } catch { return tag; }
}
// What differs per system comes from the Rust side, so the UI reads right on macOS, Windows
// and Linux. `privacy` lists the capture kinds this system has a privacy page for.
export const platform = await invoke('platform');
export const isMac = platform.os === 'macos';
export const trayName = isMac ? 'the menu bar' : 'the system tray';

// The keys of a shortcut such as "Ctrl+Alt+Shift+Space", named as this system names them.
const MAC_KEYS = { Ctrl: '⌃', Alt: '⌥', Shift: '⇧', Super: '⌘' };
const superKey = platform.os === 'windows' ? 'Win' : 'Super';
export const keyCaps = shortcut => shortcut.split('+').map(k => k.replace(/^(Key|Digit)(?=.)/, ''))
  .map(k => isMac ? MAC_KEYS[k] ?? k : k === 'Super' ? superKey : k);

export const short =tag => tag ? tag.split('-')[0].toUpperCase() : 'AUTO';

const titles = {
  permission: 'Permission needed',
  language: 'Language not supported',
  model_missing: 'Setup needed',
  config: 'Settings incomplete',
  device: 'Audio device problem',
  provider: 'Engine failed',
  graph: 'Internal error',
};
export function explain(error) {
  if (!error?.code) return { title: 'Something went wrong', detail: String(error) };
  return { title: titles[error.code] ?? error.code, detail: error.detail, code: error.code };
}

// A desktop app, not a web page: no browser menu, reload or print (WebView2 and WebKitGTK offer them).
// Text fields keep theirs, for paste.
addEventListener('contextmenu', e => { if (!e.target.closest?.('input, textarea')) e.preventDefault(); });
addEventListener('keydown', e => {
  const mod = e.ctrlKey || e.metaKey, key = e.key.toLowerCase();
  if (e.key === 'F5' || mod && (key === 'r' || key === 'p')) e.preventDefault();
});
