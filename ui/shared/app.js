// The only place that talks to the Rust side, plus wording shared by both windows.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

export { invoke, listen };

const names = new Intl.DisplayNames(['en'], { type: 'language' });
export function languageName(tag) {
  if (!tag) return 'Detect automatically';
  try { return names.of(tag) ?? tag; } catch { return tag; }
}
// Words that differ per system, so the UI reads right on macOS, Windows and Linux.
export const isMac = /Mac/.test(navigator.userAgent);
export const hotkeyName = isMac ? 'Control+Option+L' : 'Ctrl+Alt+L';
export const trayName = isMac ? 'the menu bar' : 'the system tray';

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
