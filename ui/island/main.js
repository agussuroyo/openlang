import { invoke, listen, short, explain, isMac, platform } from '../shared/app.js';
import { draggable } from '../shared/drag.js';
import { createIsland } from './island.js';
import { soundBars } from './bars.js';

const PAD = 10;   // room around the island for its overshoot
const $ = id => document.getElementById(id);
let settings, status = { state: 'idle' }, startedAt = 0, ticking = null, waiting = null, collapse = null, settle = null, fetching = null;
let size = [0, 0], problem = false;

// The window is only as big as the island, so it never blocks clicks around it.
function fit([w, h]) {
  const want = [w + 2 * PAD, h + 2 * PAD];
  clearTimeout(settle);
  const room = [Math.max(size[0], want[0]), Math.max(size[1], want[1])];
  const apply = s => { size = s; invoke('resize_island', { width: s[0], height: s[1] }); };
  apply(room);
  if (room[0] !== want[0] || room[1] !== want[1]) settle = setTimeout(() => apply(want), 700);
}
const island = createIsland($('island'), fit);
const bars = soundBars($('bars'));

function visibility() {
  const busy = problem || ['preparing', 'starting', 'error'].includes(status.state);
  const on = busy || settings.island_mode === 'always'
    || settings.island_mode === 'hide_listening' && status.state !== 'listening';
  invoke('set_visible', { window: 'island', visible: on });
}

function applySettings(next) {
  settings = next;
  const pair = `${short(next.source_language)} to ${short(next.target_language)}`;
  for (const id of ['idlePair', 'pair', 'panelPair']) $(id).textContent = pair;
  island.refit();
  document.body.dataset.bars = next.sound_bars ? 'on' : 'off';
  visibility();
}

function tickClock() {
  const s = Math.floor((Date.now() - startedAt) / 1000);
  $('clock').textContent = `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

function fail(error) {
  const e = explain(error), act = $('errAct');
  $('errTitle').textContent = e.title;
  $('errDetail').textContent = e.detail;
  $('errDetail').title = e.detail;
  // The system's privacy page when it has one for this capture, otherwise Settings.
  const pane = /microphone/i.test(e.detail) ? 'microphone' : 'system_audio';
  const allow = e.code === 'permission' && platform.privacy.includes(pane);
  act.textContent = allow ? 'Allow' : 'Settings';
  act.onclick = () => (allow ? invoke('open_privacy', { pane }) : invoke('show_settings')).catch(() => {});
  problem = true;
  island.go('error');
  visibility();
}

function applyStatus(next) {
  const was = status.state;
  status = next;
  problem = false;
  const live = next.state === 'listening';
  if (live && was !== 'listening') {
    startedAt = Date.now();
    tickClock();
    ticking = setInterval(tickClock, 1000);
    $('note').textContent = 'Listening';
  }
  if (!live) { clearInterval(ticking); bars.reset(); }
  clearTimeout(waiting);
  if (next.state === 'error') return fail(next.error);
  if (next.state === 'preparing') {
    const p = Math.max(0, Math.min(1, next.progress));
    $('model').textContent = next.name;
    $('pct').textContent = Math.round(p * 100) + '%';
    $('ring').style.strokeDashoffset = 81.7 * (1 - p);
  }
  if (next.state === 'starting') {
    $('startText').textContent = 'Starting';
    // Capture blocks while macOS waits for an answer to its permission dialog.
    if (isMac) waiting = setTimeout(() => { $('startText').textContent = 'Check the macOS prompt'; }, 5000);
  }
  island.go(next.state);
  visibility();
}

// Silence on macOS is usually a missing permission; elsewhere nothing asks, so it is the device.
const silentNote = isMac ? 'No sound yet. Check permission.' : 'No sound yet. Check the output device.';
function onEvent({ type, data }) {
  if (type === 'level') {
    if (settings.sound_bars) bars.set(data.origin, data.level);
  } else if (type === 'silent') {
    $('note').textContent = data.silent ? silentNote : 'Listening';
  } else if (type === 'failed' && data.error?.code === 'model_missing' && status.state === 'listening') {
    // A newly heard language fetches its model while captions go on. Each sentence reports it
    // again until the model is there, so a quiet while means it arrived.
    $('note').textContent = 'Getting the model for this language';
    clearTimeout(fetching);
    fetching = setTimeout(() => { if (status.state === 'listening') $('note').textContent = 'Listening'; }, 8000);
  } else if (type === 'failed') {
    fail(data.error);
  }
}

const actions = {
  start: () => invoke('start').catch(() => {}),
  stop: () => invoke('stop'),
  settings: () => invoke('show_settings'),
  panel: () => {
    island.go('panel');
    clearTimeout(collapse);
    collapse = setTimeout(() => { if (island.state === 'panel') island.go('listening'); }, 5000);
  },
  dismiss: () => {
    problem = false;
    if (status.state === 'error') status = { state: 'idle' };
    island.go(status.state);
    visibility();
  },
};
document.addEventListener('click', e => {
  const name = e.target.closest('[data-do]')?.dataset.do;
  if (name) actions[name]();
});

draggable('island');
await listen('settings', e => applySettings(e.payload));
await listen('status', e => applyStatus(e.payload));
await listen('session', e => onEvent(e.payload.body));
await listen('edges', e => { [document.body.dataset.h, document.body.dataset.v] = e.payload; });
const [s, st] = await invoke('current');
status = st;
applySettings(s);
island.go('idle');
applyStatus(st);
