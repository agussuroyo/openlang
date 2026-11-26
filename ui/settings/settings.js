import { invoke, listen, languageName, explain, isMac, hotkeyName, trayName } from '../shared/app.js';
import { bottomBar } from './bottom-bar.js';

const $ = id => document.getElementById(id);
let data;

function h(tag, props = {}, ...children) {
  const el = Object.assign(document.createElement(tag), props);
  el.append(...children.filter(c => c != null && c !== false));
  return el;
}
const svg = markup => h('span', { className: 'glyph', ariaHidden: 'true', innerHTML: markup });
const CHEVRON = '<svg viewBox="0 0 24 24"><path d="m9 6 6 6-6 6"/></svg>';
const CHECK = '<svg viewBox="0 0 24 24"><path d="m5 12.500 4.500 4.500L19 7"/></svg>';
let ids = 0;
const uid = () => `f${ids++}`;

// Building blocks. Each one makes one kind of thing and knows nothing about settings.
function heading(title, text) {
  return h('header', { className: 'head' }, h('h2', { textContent: title }), h('p', { textContent: text }));
}
function group(title, rows, note) {
  return h('div', { className: 'group' },
    title && h('h3', { textContent: title }),
    h('div', { className: 'panel' }, ...rows),
    note && h('p', { className: 'note', textContent: note }));
}
function words(title, note, tone = '') {
  return h('span', { className: 'words' }, h('span', { className: 'title', textContent: title }),
    note && h('span', { className: `sub ${tone}`, textContent: note }));
}
// A label row makes the whole line clickable for its control.
const row = (title, note, control) => h('label', { className: 'row' }, words(title, note), control);
const info = (title, note, end) => h('div', { className: 'row' }, words(title, note), end);
function link(title, note, onclick) {
  return h('button', { className: 'row link', onclick }, words(title, note), svg(CHEVRON));
}
function field(label, control, note) {
  control.id ||= uid();
  return h('div', { className: 'field' }, h('label', { htmlFor: control.id, textContent: label }), control,
    note && h('p', { className: 'sub', textContent: note }));
}
function select(options, value, onchange) {
  const el = h('select');
  for (const [v, text] of options) el.append(h('option', { value: v, textContent: text, selected: v === value }));
  el.addEventListener('change', () => onchange(el.value));
  return el;
}
function toggle(checked, onchange) {
  const box = h('input', { type: 'checkbox', role: 'switch', checked });
  box.addEventListener('change', () => onchange(box.checked, box));
  return h('span', { className: 'switch' }, box, h('i'));
}
function segmented(label, options, value, onchange) {
  const name = uid();
  return h('div', { className: 'segmented', role: 'radiogroup', ariaLabel: label }, ...options.map(([v, text]) => {
    const input = h('input', { type: 'radio', name, value: v, checked: v === value });
    input.addEventListener('change', () => onchange(v));
    return h('label', {}, input, h('span', { textContent: text }));
  }));
}
const badge = (tone, text) => h('span', { className: `badge ${tone}` }, h('i'), text);
function keys(...names) {
  return h('span', { className: 'keys', ariaLabel: hotkeyName }, ...names.map(k => h('kbd', { textContent: k })));
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

function readiness({ readiness: r }) {
  if (r.state === 'ready') return { tone: 'ok', label: 'Ready', detail: '' };
  if (r.state === 'needs_download') {
    const size = r.bytes ? `${Math.round(r.bytes / 1e6)} MB` : 'Download';
    return { tone: 'wait', label: size, detail: 'Downloads once when you press Start.' };
  }
  return { tone: 'bad', label: r.state === 'unsupported' ? 'Not supported' : 'Needs setup', detail: r.message };
}
function status(label, p) {
  if (!p) return info(label, 'No engine installed.', badge('bad', 'Missing'));
  const r = readiness(p);
  return info(label, [p.descriptor.name, r.detail].filter(Boolean).join('. '), badge(r.tone, r.label));
}

function language() {
  const s = data.settings;
  const stt = provider(data.stt, chosen(s.stt)), tr = provider(data.translators, chosen(s.translators));
  const spoken = byName(stt?.languages.tags ?? []);
  if (stt?.languages.can_detect) spoken.unshift(['', languageName(null)]);
  const target = byName(tr?.languages.tags ?? []);
  const keep = (list, v) => list.some(([x]) => x === v) ? list : [[v, `${languageName(v)} (not supported by this engine)`], ...list];
  return [
    heading('Language', 'Choose what you hear and what you read.'),
    group(null, [
      row('Spoken language', null, select(keep(spoken, s.source_language ?? ''), s.source_language ?? '',
        v => save({ source_language: v || null }, true))),
      row('Translate to', null, select(keep(target, s.target_language), s.target_language,
        v => save({ target_language: v }, true))),
    ]),
    group('Engines', [status('Speech recognition', stt), status('Translation', tr)]),
  ];
}

function audio() {
  const s = data.settings;
  const source = ({ descriptor: d }) => row(d.name, d.summary, toggle(s.sources.includes(d.id), (on, box) => {
    const next = s.sources.filter(x => x !== d.id);
    if (on) next.push(d.id);
    if (!next.length) { box.checked = true; return; }
    save({ sources: next });
  }));
  const privacy = pane => () => invoke('open_privacy', { pane }).catch(problem);
  return [
    heading('Audio', 'Pick what OpenLang listens to. Audio is never saved.'),
    group('Sources', data.sources.map(source), 'Keep at least one source on.'),
    isMac && group('Permissions', [
      link('System audio', 'Needed to hear films and calls.', privacy('system_audio')),
      link('Microphone', 'Needed to hear your voice.', privacy('microphone')),
    ], 'macOS asks the first time each source is used.'),
  ];
}

function engines(title, key, list) {
  const s = data.settings, active = chosen(s[key]);
  const pick = id => save({ [key]: [id, ...s[key].slice(1).filter(x => x !== id)] }, true);
  return group(title, list.flatMap(p => {
    const d = p.descriptor, on = d.id === active, r = readiness(p);
    const input = h('input', { type: 'radio', name: key, checked: on });
    input.addEventListener('change', () => pick(d.id));
    const choice = h('label', { className: 'row choice' }, input, words(d.name, d.summary),
      badge(r.tone, r.label), svg(CHECK));
    return on ? [choice, options(key, list, p)] : [choice];
  }).filter(Boolean));
}

function options(key, list, p) {
  const d = p.descriptor;
  const online = p.location === 'network' && h('p', { className: 'sub bad', textContent: 'Online: speech or text is sent to this address.' });
  const parts = [...d.fields.map(f => fieldFor(d, f, p)), online, fallback(key, list, d.id)].filter(Boolean);
  if (!parts.length) return null;
  return h('div', { className: 'options' }, ...parts);
}

function fallback(key, list, active) {
  const others = list.filter(p => p.descriptor.id !== active);
  if (!others.length) return null;
  const choices = [['', 'None'], ...others.map(p => [p.descriptor.id, p.descriptor.name])];
  return field('Fallback if it fails', select(choices, data.settings[key][1] ?? '', v => save({ [key]: v ? [active, v] : [active] })));
}

function fieldFor(d, f, p) {
  const s = data.settings;
  if (f.kind.type === 'secret') {
    const input = h('input', { type: 'password', autocomplete: 'off', placeholder: p.stored_secrets.includes(f.key) ? 'Saved securely' : 'Empty' });
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
    heading('Engine', 'The engines that hear and translate.'),
    engines('Speech recognition', 'stt', data.stt),
    engines('Translation', 'translators', data.translators),
    group('Privacy', [row('Allow online engines', 'When off, speech and text never leave this computer.',
      toggle(s.allow_network, v => save({ allow_network: v })))]),
  ];
}

const EDGES = ['start', 'middle', 'end'];
const PLACES = [['Top left', 'Top center', 'Top right'], ['Left', 'Center', 'Right'], ['Bottom left', 'Bottom center', 'Bottom right']];
function places(title, name, place) {
  const grid = h('div', { className: 'places', role: 'group', ariaLabel: title });
  PLACES.forEach((line, y) => line.forEach((label, x) => {
    const on = place.kind === 'preset' && place.h === EDGES[x] && place.v === EDGES[y];
    const b = h('button', { className: 'place', title: label, ariaLabel: label, ariaPressed: String(on) });
    b.addEventListener('click', () => invoke('set_preset', { window: name, h: EDGES[x], v: EDGES[y] }).catch(problem));
    grid.append(b);
  }));
  const note = place.kind === 'custom' ? 'Placed by dragging.' : 'Or drag it on screen.';
  return h('div', { className: 'spot' }, h('span', { className: 'title', textContent: title }), grid, h('span', { className: 'sub', textContent: note }));
}
const placePair = s => h('div', { className: 'pair' },
  places('Subtitles', 'caption', s.caption_place), places('Controls', 'island', s.island_place));

function preview(style) {
  const screen = h('div', { className: 'screen', ariaHidden: 'true' },
    h('p', { className: 'sample' }, h('span', { className: 'original', textContent: 'Where is the nearest station?' }),
      h('span', { className: 'translation', textContent: 'Di mana stasiun terdekat?' })));
  screen.style.setProperty('--size', `${style.text_size}px`);
  screen.dataset.show = style.show;
  return screen;
}

const saveSubtitle = change => save({ subtitle: { ...data.settings.subtitle, ...change } });

function display() {
  const s = data.settings, screen = preview(s.subtitle);
  const size = h('input', { type: 'range', min: 11, max: 40, step: 1, value: s.subtitle.text_size });
  const value = h('output', { className: 'value', textContent: `${s.subtitle.text_size} px` });
  size.addEventListener('input', () => {
    value.textContent = `${size.value} px`;
    screen.style.setProperty('--size', `${size.value}px`);
  });
  size.addEventListener('change', () => saveSubtitle({ text_size: Number(size.value) }));
  size.id = uid();
  const show = v => { screen.dataset.show = v; saveSubtitle({ show: v }); };
  return [
    heading('Display', 'How subtitles look and where things sit.'),
    screen,
    group('Subtitles', [
      h('div', { className: 'stack' },
        h('div', { className: 'line' }, h('label', { className: 'title', htmlFor: size.id, textContent: 'Text size' }), value), size),
      h('div', { className: 'stack' }, h('span', { className: 'title', textContent: 'Show' }), segmented('Show', [
        ['both', 'Both'], ['translation', 'Translation'], ['original', 'Original'],
      ], s.subtitle.show, show)),
      row('Click through subtitles', 'Clicks go to the app underneath.',
        toggle(s.click_through, v => save({ click_through: v }))),
    ]),
    group('Position', [placePair(s)]),
    group('Controls', [
      row('Show controls', null, select([
        ['always', 'Always'],
        ['hide_listening', 'Hide while listening'],
        ['problems', 'Only when action is needed'],
      ], s.island_mode, v => save({ island_mode: v }))),
      row('Pulse while listening', 'The live dot on the controls beats softly.',
        toggle(s.pulse, v => save({ pulse: v }))),
      info('Start or stop', `Works in any app, or use OpenLang in ${trayName}.`,
        isMac ? keys('⌃', '⌥', 'L') : keys('Ctrl', 'Alt', 'L')),
    ]),
  ];
}

const views = { language, audio, engine, display };
function draw() {
  const y = $('main').scrollTop;
  for (const [name, view] of Object.entries(views)) $(`tab-${name}`).replaceChildren(...view().filter(Boolean));
  $('main').scrollTop = y;
}
function redrawPlaces() {
  const root = $('tab-display'), focus = document.activeElement?.ariaLabel, spot = document.activeElement?.parentElement?.ariaLabel;
  root.querySelector('.pair')?.replaceWith(placePair(data.settings));
  if (focus) [...root.querySelectorAll('.place')].find(b => b.ariaLabel === focus && b.parentElement.ariaLabel === spot)?.focus();
}
async function load() {
  try {
    data = await invoke('snapshot');
    $('loading').hidden = true;
    draw();
  } catch (e) { $('loading').hidden = true; problem(e); }
}

// macOS draws the window buttons over the page, so the top strip makes room and moves the window.
if (isMac) document.documentElement.dataset.chrome = 'blend';
$('main').addEventListener('scroll', () => $('top').classList.toggle('edge', $('main').scrollTop > 0), { passive: true });

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
