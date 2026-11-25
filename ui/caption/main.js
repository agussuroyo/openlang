import { invoke, listen } from '../shared/app.js';
import { draggable } from '../shared/drag.js';
import { readAlong } from './read-along.js';

const $ = id => document.getElementById(id);
const heard = new Map();   // segment id -> latest original text
let settings, live = false, previewing = false, reading = null, hiding = null, shown = false;

// The window itself goes away when there is nothing to read, so it never covers or blocks anything.
function show(on) {
  clearTimeout(hiding);
  $('panel').classList.toggle('quiet', !on);
  if (on && !shown) { shown = true; invoke('set_visible', { window: 'caption', visible: true }); }
  if (!on && shown) hiding = setTimeout(() => { shown = false; invoke('set_visible', { window: 'caption', visible: false }); }, 220);
}

const reader = readAlong({
  read: $('read'), pill: $('pill'), view: $('view'),
  onSegment: s => { reading = s.id; showOriginal(); },
  onQuiet: quiet => show(!quiet || previewing && !live),
});

function showOriginal() {
  const item = heard.get(reading);
  const b = document.createElement('bdi');
  b.textContent = item ? (item.mic ? `You: ${item.text}` : item.text) : '';
  $('original').replaceChildren(b);
}

function applySettings(next) {
  settings = next;
  document.documentElement.style.setProperty('--size', next.text_size + 'px');
  $('original').hidden = !next.show_original;
  showOriginal();
}

function reset() {
  reader.clear();
  heard.clear();
  reading = null;
  showOriginal();
}

function applyStatus(status) {
  const now = status.state === 'listening';
  if (now === live) return;
  live = now;
  reset();
  if (!live && previewing) sample();
}

function sample() {
  heard.set('sample', { text: 'Original text', mic: false });
  reading = 'sample';
  reader.update({ id: 'sample', text: 'Drag me anywhere on the screen.', language: 'en', stable: 6, origin: 'system' });
  showOriginal();
  show(true);
}

function preview(on) {
  previewing = on;
  if (live) return;
  reset();
  if (on) sample();
}

function onEvent({ type, data }) {
  if (type === 'transcript') {
    heard.set(data.segment.id, { text: data.text, mic: data.segment.origin === 'microphone' });
    if (heard.size > 40) heard.delete(heard.keys().next().value);
    if (data.segment.id === reading || reading === null) showOriginal();
  } else if (type === 'caption') {
    const t = data.translation;
    reader.update({
      id: t.segment.id, text: t.text, language: t.language,
      stable: data.stable_words, origin: t.segment.origin,
    });
  }
}

draggable('caption');
await listen('settings', e => applySettings(e.payload));
await listen('status', e => applyStatus(e.payload));
await listen('session', e => { if (live) onEvent(e.payload.body); });
await listen('preview', e => preview(e.payload));
const [s, st] = await invoke('current');
applySettings(s);
applyStatus(st);
