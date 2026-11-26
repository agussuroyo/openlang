import { invoke, listen, languageName, explain } from '../shared/app.js';
import { bottomBar } from './bottom-bar.js';

const $ = id => document.getElementById(id);
let data;

function h(tag, props = {}, ...children) {
  const el = Object.assign(document.createElement(tag), props);
  el.append(...children.filter(c => c != null));
  return el;
}
let ids = 0;
function field(label, control, hint) {
  control.id ||= `f${ids++}`;
  return h('div', { className: 'field' }, h('label', { htmlFor: control.id, textContent: label }), control,
    hint && h('p', { className: 'hint', textContent: hint }));
}
function select(options, value, onchange) {
  const el = h('select');
  for (const [v, text] of options) el.append(h('option', { value: v, textContent: text, selected: v === value }));
  el.addEventListener('change', () => onchange(el.value));
  return el;
}
function check(label, hint, checked, onchange, type = 'checkbox', name = '') {
  const box = h('input', { type, checked, name });
  box.addEventListener('change', () => onchange(box.checked, box));
  return h('label', { className: 'check' }, box, h('span', {}, label, hint && h('span', { textContent: hint })));
}

function problem(error) {
  const p = $('problem');
  p.hidden = !error;
  if (error) { const e = explain(error); p.textContent = `${e.title}. ${e.detail}`; }
}
// Saves run one after another so a slow one cannot overwrite a newer one.
let saving = Promise.resolve();
function save(change, refresh = false) {
  Object.assign(data.settings, change);
  const copy = structuredClone(data.settings);
  saving = saving.then(() => invoke('save', { settings: copy })).then(() => problem(null), problem)
    .then(() => refresh && load());
  return saving;
}

const chosen = list => list[0];
const provider = (list, id) => list.find(p => p.descriptor.id === id);
const byName = tags => tags.map(t => [t, languageName(t)]).sort((a, b) => a[1].localeCompare(b[1], 'en'));

function language() {
  const s = data.settings;
  const stt = provider(data.stt, chosen(s.stt)), tr = provider(data.translators, chosen(s.translators));
  const spoken = byName(stt?.languages.tags ?? []);
  if (stt?.languages.can_detect) spoken.unshift(['', languageName(null)]);
  const target = byName(tr?.languages.tags ?? []);
  const keep = (list, v) => list.some(([x]) => x === v) ? list : [[v, `${languageName(v)} (not supported by this engine)`], ...list];
  return [
    h('h2', { textContent: 'Language' }),
    field('Spoken language', select(keep(spoken, s.source_language ?? ''), s.source_language ?? '',
      v => save({ source_language: v || null }, true))),
    field('Translate to', select(keep(target, s.target_language), s.target_language, v => save({ target_language: v }, true))),
    state('Speech recognition', stt), state('Translation', tr),
  ];
}

function state(label, p) {
  if (!p) return h('p', { className: 'state bad', textContent: `${label}: no engine installed.` });
  const r = p.readiness, size = r.bytes ? ` (${Math.round(r.bytes / 1e6)} MB)` : '';
  const text = {
    ready: 'ready.',
    needs_download: `model${size} downloads once when you press Start.`,
    needs_action: r.message,
    unsupported: r.message,
  }[r.state];
  const tone = r.state === 'ready' ? ' ok' : r.state === 'needs_download' ? '' : ' bad';
  return h('p', { className: 'state' + tone, textContent: `${label} ${p.descriptor.name}: ${text}` });
}

function audio() {
  const s = data.settings;
  return [
    h('h2', { textContent: 'Audio' }),
    h('div', {}, ...data.sources.map(({ descriptor: d }) => check(d.name, d.summary, s.sources.includes(d.id), (on, box) => {
      const next = s.sources.filter(x => x !== d.id);
      if (on) next.push(d.id);
      if (!next.length) { box.checked = true; return; }
      save({ sources: next });
    }))),
    h('p', { className: 'hint', textContent: 'Keep at least one source on. macOS asks for permission the first time each is used. Audio is not saved.' }),
    h('button', { className: 'btn', textContent: 'Open system audio permission', onclick: () => invoke('open_privacy', { pane: 'system_audio' }).catch(problem) }),
    h('button', { className: 'btn', textContent: 'Open microphone permission', onclick: () => invoke('open_privacy', { pane: 'microphone' }).catch(problem) }),
  ];
}

function engines(title, key, list) {
  const s = data.settings, active = chosen(s[key]);
  return h('div', {}, h('h3', { textContent: title }), ...list.flatMap(p => {
    const d = p.descriptor, on = d.id === active;
    return [
      check(d.name, d.summary, on, () => save({ [key]: [d.id, ...s[key].slice(1).filter(x => x !== d.id)] }, true), 'radio', key),
      on ? h('div', { className: 'engine' },
        ...d.fields.map(f => fieldFor(d, f, p)),
        p.location === 'network' && h('p', { className: 'state bad', textContent: 'Online: data is sent to this address.' }) || null,
        fallback(key, list, d.id)) : null,
    ].filter(Boolean);
  }));
}

function fallback(key, list, active) {
  const others = list.filter(p => p.descriptor.id !== active);
  if (!others.length) return null;
  const options = [['', 'None'], ...others.map(p => [p.descriptor.id, p.descriptor.name])];
  return field('Fallback if it fails', select(options, data.settings[key][1] ?? '', v => save({ [key]: v ? [active, v] : [active] })));
}

function fieldFor(d, f, p) {
  const s = data.settings;
  if (f.kind.type === 'secret') {
    const input = h('input', { type: 'password', autocomplete: 'off', placeholder: p.stored_secrets.includes(f.key) ? 'Saved in Keychain' : 'Empty' });
    input.addEventListener('change', () => invoke('set_secret', { provider: d.id, key: f.key, value: input.value.trim() }).then(load, problem));
    return field(f.label, input);
  }
  const value = s.options[d.id]?.[f.key] ?? f.default;
  const set = v => save({ options: { ...s.options, [d.id]: { ...s.options[d.id], [f.key]: v } } }, true);
  if (f.kind.type === 'choice') return field(f.label, select(f.kind.choices.map(c => [c.value, c.label]), value, set));
  const input = h('input', { type: 'text', value, spellcheck: false });
  input.addEventListener('change', () => set(input.value.trim()));
  return field(f.label, input);
}

function engine() {
  const s = data.settings;
  return [
    h('h2', { textContent: 'Engine' }),
    engines('Speech recognition', 'stt', data.stt),
    engines('Translation', 'translators', data.translators),
    check('Allow online engines', 'When off, speech and text never leave this computer.', s.allow_network, v => save({ allow_network: v })),
  ];
}

const EDGES = ['start', 'middle', 'end'];
const PLACES = [['Top left', 'Top center', 'Top right'], ['Left', 'Center', 'Right'], ['Bottom left', 'Bottom center', 'Bottom right']];
function places(title, name, place) {
  const grid = h('div', { className: 'places', role: 'group', ariaLabel: title });
  PLACES.forEach((row, y) => row.forEach((label, x) => {
    const on = place.kind === 'preset' && place.h === EDGES[x] && place.v === EDGES[y];
    const b = h('button', { className: 'place', title: label, ariaLabel: label, ariaPressed: String(on) });
    b.addEventListener('click', () => invoke('set_preset', { window: name, h: EDGES[x], v: EDGES[y] }).catch(problem));
    grid.append(b);
  }));
  const note = place.kind === 'custom' ? 'Placed by dragging.' : 'Or drag it on screen.';
  return h('div', { className: 'field' }, h('h3', { textContent: title }), grid, h('p', { className: 'hint', textContent: note }));
}

const saveSubtitle = change => save({ subtitle: { ...data.settings.subtitle, ...change } });

function display() {
  const s = data.settings;
  const size = h('input', { type: 'range', min: 11, max: 40, step: 1, value: s.subtitle.text_size });
  const num = h('span', { className: 'num', textContent: `${s.subtitle.text_size} px` });
  const sample = h('p', { className: 'sample', textContent: 'Subtitle size preview' });
  sample.style.fontSize = s.subtitle.text_size + 'px';
  size.addEventListener('input', () => { num.textContent = `${size.value} px`; sample.style.fontSize = size.value + 'px'; });
  size.addEventListener('change', () => saveSubtitle({ text_size: Number(size.value) }));
  size.id = 'size';
  return [
    h('h2', { textContent: 'Display' }),
    h('div', { className: 'pair' },
      places('Subtitle position', 'caption', s.caption_place),
      places('Controls position', 'island', s.island_place)),
    field('Show controls', select([
      ['always', 'Always'],
      ['hide_listening', 'Hide while listening'],
      ['problems', 'Only when action is needed'],
    ], s.island_mode, v => save({ island_mode: v })), 'While hidden, use Control+Option+L or the OpenLang menu.'),
    h('div', { className: 'field' }, h('label', { htmlFor: 'size', textContent: 'Text size' }), h('div', { className: 'row' }, size, num)),
    sample,
    field('Subtitle text', select([
      ['both', 'Original and translation'],
      ['translation', 'Translation only'],
      ['original', 'Original only'],
    ], s.subtitle.show, v => saveSubtitle({ show: v }))),
    check('Click through subtitles', 'Clicks go to the app underneath. Turn it off here.', s.click_through, v => save({ click_through: v })),
    h('p', { className: 'hint', textContent: 'Start or stop from anywhere with Control+Option+L, or from OpenLang in the menu bar.' }),
  ];
}

const views = { language, audio, engine, display };
function draw() {
  const y = $('main').scrollTop;
  for (const [name, view] of Object.entries(views)) $(`tab-${name}`).replaceChildren(...view());
  $('main').scrollTop = y;
}
function redrawPlaces() {
  const root = $('tab-display'), focus = document.activeElement?.ariaLabel, group = document.activeElement?.parentElement?.ariaLabel;
  const { caption_place, island_place } = data.settings;
  root.querySelector('.pair')?.replaceChildren(
    places('Subtitle position', 'caption', caption_place), places('Controls position', 'island', island_place));
  if (focus) [...root.querySelectorAll('.place')].find(b => b.ariaLabel === focus && b.parentElement.ariaLabel === group)?.focus();
}
async function load() {
  try {
    data = await invoke('snapshot');
    $('loading').hidden = true;
    draw();
  } catch (e) { $('loading').hidden = true; problem(e); }
}

bottomBar($('bar'), name => {
  for (const key of Object.keys(views)) $(`tab-${key}`).hidden = key !== name;
  // A sample subtitle shows where the real one will be.
  invoke('preview', { on: name === 'display' });
  $('main').scrollTop = 0;
});
await load();
// Dragging a window changes its place while this screen is open.
await listen('settings', e => { if (data) { data.settings = e.payload; redrawPlaces(); } });
// A model may have been downloaded while this window was in the background.
await listen('status', e => { if (e.payload.state === 'listening') load(); });
