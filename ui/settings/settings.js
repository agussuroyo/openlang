import { invoke, listen, languageName, explain, isMac, platform, trayName, keyCaps } from '../shared/app.js';

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
  return h('header', { className: 'head' }, h('h1', { textContent: title }), h('p', { textContent: text }));
}
function group(title, rows, note) {
  return h('div', { className: 'group' },
    title && h('h2', { textContent: title }),
    h('div', { className: 'panel' }, ...rows),
    note && h('p', { className: 'note', textContent: note }));
}
function words(title, note) {
  return h('span', { className: 'words' }, h('span', { className: 'title', textContent: title }),
    note && h('span', { className: 'sub', textContent: note }));
}
// A label row makes the whole line clickable for its control.
const row = (title, note, control) => h('label', { className: 'row' }, words(title, note), control);
const info = (title, note, end) => h('div', { className: 'row' }, words(title, note), end);
function link(title, note, onclick, end) {
  return h('button', { className: 'row link', onclick }, words(title, note), end, svg(CHEVRON));
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
// Shows a shortcut; press it, then type the new keys. Escape or leaving keeps the old one.
function recorder(value, onchange) {
  const el = h('button', { type: 'button', className: 'keys record', ariaLabel: `${keyCaps(value).join(' ')}, press to change` });
  let recording = false;
  const show = shortcut => el.replaceChildren(...shortcut
    ? keyCaps(shortcut).map(k => h('kbd', { textContent: k }))
    : [h('span', { className: 'sub', textContent: 'Type the new keys' })]);
  const end = () => { recording = false; show(value); };
  el.addEventListener('click', () => { recording = true; show(null); });
  el.addEventListener('blur', end);
  el.addEventListener('keydown', e => {
    if (!recording) return;
    e.preventDefault();
    if (e.key === 'Escape') return end();
    if (/^(Control|Shift|Alt|Meta|OS)/.test(e.code)) return;
    const mods = [e.ctrlKey && 'Ctrl', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Super'];
    recording = false;
    onchange([...mods.filter(Boolean), e.code].join('+'));
  });
  show(value);
  return el;
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

// The first engine in a list is the one in use; the second, if any, is its fallback.
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

// Translate: what you hear, what you read, and which engines do it.
function engineLink(label, p) {
  const open = () => { location.hash = 'engines'; };
  if (!p) return link(label, 'No engine installed.', open, badge('bad', 'Missing'));
  const r = readiness(p);
  return link(label, [p.descriptor.name, r.detail].filter(Boolean).join('. '), open, badge(r.tone, r.label));
}

function translate() {
  const s = data.settings;
  const stt = provider(data.stt, s.stt[0]), tr = provider(data.translators, s.translators[0]);
  const spoken = byName(stt?.languages.tags ?? []);
  if (stt?.languages.can_detect) spoken.unshift(['', languageName(null)]);
  const target = byName(tr?.languages.tags ?? []);
  const keep = (list, v) => list.some(([x]) => x === v) ? list : [[v, `${languageName(v)} (not supported by this engine)`], ...list];
  const autoNote = s.source_language ? null : 'Listens for a few seconds first, then picks the language it hears.';
  return [
    group(null, [
      row('Spoken language', autoNote, select(keep(spoken, s.source_language ?? ''), s.source_language ?? '',
        v => save({ source_language: v || null }, true))),
      row('Translate to', null, select(keep(target, s.target_language), s.target_language,
        v => save({ target_language: v }, true))),
    ]),
    group('Engines in use', [engineLink('Speech recognition', stt), engineLink('Translation', tr)]),
  ];
}

const PRIVACY = {
  system_audio: ['System audio', 'Needed to hear films and calls.'],
  microphone: ['Microphone', 'Needed to hear your voice.'],
};
function audio() {
  const s = data.settings;
  const source = ({ descriptor: d }) => row(d.name, d.summary, toggle(s.sources.includes(d.id), (on, box) => {
    const next = s.sources.filter(x => x !== d.id);
    if (on) next.push(d.id);
    if (!next.length) { box.checked = true; return; }
    save({ sources: next });
  }));
  const privacy = pane => () => invoke('open_privacy', { pane }).catch(problem);
  const pages = platform.privacy.map(pane => link(...PRIVACY[pane], privacy(pane)));
  return [
    group('Sources', data.sources.map(source), 'Keep at least one source on.'),
    pages.length && group('Permissions', pages, isMac
      ? 'macOS asks the first time each source is used.'
      : 'Windows blocks the microphone while its privacy setting is off.'),
  ];
}

// Where a floating window sits: nine spots on a small screen, or wherever it was dragged.
const WINDOWS = {
  caption: { key: 'caption_place', label: 'Subtitle position' },
  island: { key: 'island_place', label: 'Controls position' },
};
const EDGES = ['start', 'middle', 'end'];
const SPOTS = [['Top left', 'Top center', 'Top right'], ['Left', 'Center', 'Right'], ['Bottom left', 'Bottom center', 'Bottom right']];
function placeRow(name) {
  const { key, label } = WINDOWS[name], place = data.settings[key];
  const grid = h('div', { className: 'places', role: 'group', ariaLabel: label });
  SPOTS.forEach((line, y) => line.forEach((spot, x) => {
    const on = place.kind === 'preset' && place.h === EDGES[x] && place.v === EDGES[y];
    const b = h('button', { className: 'place', title: spot, ariaLabel: spot, ariaPressed: String(on) });
    b.addEventListener('click', () => invoke('set_preset', { window: name, h: EDGES[x], v: EDGES[y] }).catch(problem));
    grid.append(b);
  }));
  const el = info('On screen', place.kind === 'custom' ? 'Placed by dragging.' : 'Pick a spot, or drag it on screen.', grid);
  el.dataset.window = name;
  return el;
}
// Dragging a window changes its place while this screen is open.
function redrawPlaces() {
  const focus = document.activeElement;
  const owner = focus?.closest('[data-window]')?.dataset.window, spot = focus?.ariaLabel;
  for (const old of document.querySelectorAll('[data-window]')) old.replaceWith(placeRow(old.dataset.window));
  if (owner) document.querySelector(`[data-window=${owner}] .place[aria-label="${spot}"]`)?.focus();
}

function preview(style) {
  const screen = h('div', { className: 'screen', ariaHidden: 'true' },
    h('p', { className: 'sample' }, h('span', { className: 'original', textContent: 'Where is the nearest station?' }),
      h('span', { className: 'translation', textContent: 'Di mana stasiun terdekat?' })));
  screen.style.setProperty('--size', `${style.text_size}px`);
  screen.dataset.show = style.show;
  return screen;
}

const saveSubtitle = change => save({ subtitle: { ...data.settings.subtitle, ...change } });

function subtitles() {
  const s = data.settings, screen = preview(s.subtitle);
  const size = h('input', { type: 'range', min: 11, max: 40, step: 1, value: s.subtitle.text_size, id: uid() });
  const value = h('output', { className: 'value', textContent: `${s.subtitle.text_size} px` });
  size.addEventListener('input', () => {
    value.textContent = `${size.value} px`;
    screen.style.setProperty('--size', `${size.value}px`);
  });
  size.addEventListener('change', () => saveSubtitle({ text_size: Number(size.value) }));
  const show = v => { screen.dataset.show = v; saveSubtitle({ show: v }); };
  return [
    screen,
    group('Look', [
      h('div', { className: 'stack' },
        h('div', { className: 'line' }, h('label', { className: 'title', htmlFor: size.id, textContent: 'Text size' }), value), size),
      h('div', { className: 'stack' }, h('span', { className: 'title', textContent: 'Show' }), segmented('Show', [
        ['both', 'Both'], ['translation', 'Translation'], ['original', 'Original'],
      ], s.subtitle.show, show)),
    ]),
    group('Behaviour', [row('Click through subtitles', 'Clicks go to the app underneath.',
      toggle(s.click_through, v => save({ click_through: v })))]),
    group('Position', [placeRow('caption')]),
  ];
}

function controls() {
  const s = data.settings;
  return [
    group(null, [
      row('Show controls', null, select([
        ['always', 'Always'],
        ['hide_listening', 'Hide while listening'],
        ['problems', 'Only when action is needed'],
      ], s.island_mode, v => save({ island_mode: v }))),
      row('Sound bars', 'Three bars on the controls move with what OpenLang hears.',
        toggle(s.sound_bars, v => save({ sound_bars: v }))),
    ]),
    group('Position', [placeRow('island')]),
    group('Shortcut', [info('Start or stop', data.hotkey_works
      ? `Works in any app, or use OpenLang in ${trayName}.`
      : 'Another app uses this shortcut. Press it to choose another.',
    recorder(s.hotkey, v => save({ hotkey: v }, true)))]),
  ];
}

// Engines: one list per kind; the engine in use shows its own settings below the list.
function engineList(title, key, list) {
  const s = data.settings, active = provider(list, s[key][0]);
  const pick = id => save({ [key]: [id, ...s[key].slice(1).filter(x => x !== id)] }, true);
  const choices = list.map(p => {
    const d = p.descriptor, r = readiness(p);
    const input = h('input', { type: 'radio', name: key, checked: p === active });
    input.addEventListener('change', () => pick(d.id));
    input.addEventListener('keydown', e => e.key.startsWith('Arrow') && e.preventDefault());
    return h('label', { className: 'row choice' }, input, words(d.name, d.summary),
      h('span', { className: 'badges' }, p.location === 'network' && badge('', 'Online'), badge(r.tone, r.label)),
      svg(CHECK));
  });
  return [group(title, choices), active && engineOptions(key, list, active)];
}

function engineOptions(key, list, p) {
  const d = p.descriptor;
  const online = p.location === 'network' && h('p', { className: 'sub bad', textContent: 'Online: speech or text is sent to this address.' });
  const blocked = online && !data.settings.allow_network && h('p', { className: 'sub bad', textContent: 'Allow online engines is off, so this engine will not start.' });
  const parts = [...d.fields.map(f => fieldFor(d, f, p)), online, blocked, fallback(key, list, d.id)].filter(Boolean);
  if (!parts.length) return null;
  return group(`${d.name} settings`, [h('div', { className: 'options' }, ...parts)]);
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

function engines() {
  return [
    group('Privacy', [row('Allow online engines', 'When off, speech and text never leave this computer.',
      toggle(data.settings.allow_network, v => save({ allow_network: v }, true)))]),
    ...engineList('Speech recognition', 'stt', data.stt),
    ...engineList('Translation', 'translators', data.translators),
  ];
}

// Every area of the settings, in sidebar order. Sidebar, headings and routing all come from here.
const icon = path => `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${path}"/></svg>`;
const VIEWS = [
  { id: 'translate', title: 'Translate', text: 'What you hear and what you read.', build: translate,
    icon: icon('M3 6h9M7.500 3.500V6M10.500 6c-.8 4-3.300 7-7 9M5 9c1.200 2.700 3.200 4.600 6 5.800M12.500 20.500l4-10 4 10M14 17h5') },
  { id: 'listen', title: 'Listen', text: 'What OpenLang listens to. Audio is never saved.', build: audio,
    icon: icon('M4 10v4M8 7v10M12 4v16M16 8v8M20 10.500v3') },
  // A sample subtitle shows where the real one will be while this view is open.
  { id: 'subtitles', title: 'Subtitles', text: 'How subtitles look and where they sit.', build: subtitles, preview: true,
    icon: icon('M4 19 10 5l6 14M6.200 14h7.600M16 19l3-7 3 7M17 17h4') },
  { id: 'controls', title: 'Controls', text: 'The floating controls and the shortcut.', build: controls,
    icon: icon('M7 8h10a4 4 0 0 1 0 8H7a4 4 0 0 1 0-8zM8 12h.01') },
  { id: 'engines', title: 'Engines', text: 'The engines that hear and translate.', build: engines,
    icon: icon('M7 7h10v10H7zM10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4') },
];

const links = VIEWS.map(v => h('a', { className: 'nav', href: `#${v.id}`, innerHTML: v.icon }, v.title));
const sections = VIEWS.map(v => h('section', { role: 'region', ariaLabel: v.title, hidden: true }));
$('nav').append(...links);
$('main').append(...sections);

function draw() {
  const y = $('main').scrollTop;
  VIEWS.forEach((v, i) => sections[i].replaceChildren(heading(v.title, v.text), ...v.build().filter(Boolean)));
  $('main').scrollTop = y;
}
let current;
function show() {
  const i = Math.max(0, VIEWS.findIndex(v => v.id === location.hash.slice(1)));
  if (current === i) return;
  current = i;
  VIEWS.forEach((v, k) => {
    sections[k].hidden = k !== i;
    if (k === i) links[k].ariaCurrent = 'page'; else links[k].removeAttribute('aria-current');
  });
  invoke('preview', { on: !!VIEWS[i].preview });
  $('main').scrollTop = 0;
}
// Up and down move through the sidebar like a list.
$('nav').addEventListener('keydown', e => {
  const move = { ArrowDown: 1, ArrowUp: -1 }[e.key];
  if (!move) return;
  e.preventDefault();
  const next = links[(current + move + links.length) % links.length];
  next.focus();
  next.click();
});

async function load() {
  try {
    data = await invoke('snapshot');
    $('loading').hidden = true;
    draw();
  } catch (e) { $('loading').hidden = true; problem(e); }
}

// macOS draws the window buttons over the page, so the sidebar and the strip above the view make room.
if (isMac) document.documentElement.dataset.chrome = 'blend';
addEventListener('hashchange', show);
show();
await load();
// Updated in place: controls already drawn keep reading the same object.
await listen('settings', e => { if (data) { Object.assign(data.settings, e.payload); redrawPlaces(); } });
// A model may have been downloaded while this window was in the background.
await listen('status', e => { if (e.payload.state === 'listening') load(); });
