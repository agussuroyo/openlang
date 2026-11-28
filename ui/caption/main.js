import { invoke, listen } from '../shared/app.js';
import { draggable } from '../shared/drag.js';
import { applyStyle } from './style.js';
import { subtitles } from './subtitles.js';

const $ = id => document.getElementById(id);
let live = false, previewing = false, hiding = null, shown = false;

// The window itself goes away when there is nothing to read, so it never covers or blocks anything.
function show(on) {
  clearTimeout(hiding);
  $('subtitle').classList.toggle('quiet', !on);
  if (on && !shown) { shown = true; invoke('set_visible', { window: 'caption', visible: true }); }
  if (!on && shown) hiding = setTimeout(() => { shown = false; invoke('set_visible', { window: 'caption', visible: false }); }, 220);
}

const subtitle = subtitles({
  original: $('original'), translation: $('translation'),
  onQuiet: quiet => show(!quiet || previewing && !live),
});

// The window is exactly as tall as its two-line slots, so the text always fits and no empty space covers the screen.
function fitHeight() {
  const lines = [...document.querySelectorAll('.original,.translation')].filter(l => getComputedStyle(l).display !== 'none');
  const page = getComputedStyle(document.body);
  const height = parseFloat(page.paddingTop) + parseFloat(page.paddingBottom)
    + parseFloat(getComputedStyle($('subtitle')).rowGap) * (lines.length - 1)
    + lines.reduce((sum, l) => sum + 2 * parseFloat(getComputedStyle(l.firstElementChild).lineHeight), 0);
  invoke('fit_caption', { height: Math.ceil(height) });
}

function applySettings(settings) {
  applyStyle(settings.subtitle);
  subtitle.follow(settings.subtitle.show);
  fitHeight();
}

function applyStatus(status) {
  const now = status.state === 'listening';
  if (now === live) return;
  live = now;
  subtitle.clear();
  if (!live && previewing) sample();
}

function sample() {
  subtitle.heard('sample', 'Original text');
  subtitle.said('sample', 'Drag me anywhere on the screen.');
}

function preview(on) {
  previewing = on;
  if (live) return;
  subtitle.clear();
  if (on) sample();
}

function onEvent({ type, data }) {
  if (type === 'transcript') {
    const mic = data.segment.origin === 'microphone';
    subtitle.heard(data.segment.id, mic ? `You: ${data.text}` : data.text);
  } else if (type === 'caption') {
    subtitle.said(data.translation.segment.id, data.translation.text);
  }
}

draggable('caption');
await listen('settings', e => applySettings(e.payload));
await listen('status', e => applyStatus(e.payload));
await listen('session', e => { if (live) onEvent(e.payload.body); });
await listen('preview', e => preview(e.payload));
const [settings, status] = await invoke('current');
applySettings(settings);
applyStatus(status);
