// Read Along, fed live. A pill slides behind the word being read; the edge in front is fast and
// the one behind lags, so it stretches. At a new line it closes and opens again.
import { S, spring, tune, jump, clamp, register, wake } from '../shared/motion.js';

const PAD = .16;        // em the pill reaches past each side of a word; .read leaves room for it
const CALM = 3;         // words of backlog before reading speeds up
const KEEP = 3;         // finished segments kept for context
const QUIET_AFTER = 6;  // seconds with nothing new

// Splits into words; languages without spaces are cut by the browser's word breaker.
const spaceless = /^(ja|zh|th|yue|lo|km|my)/;
function split(text, language) {
  const t = text.trim();
  if (!t) return { words: [], glue: ' ' };
  if (!spaceless.test(language) || !Intl.Segmenter) return { words: t.split(/\s+/), glue: ' ' };
  const parts = [...new Intl.Segmenter(language, { granularity: 'word' }).segment(t)];
  const words = [];
  for (const p of parts) {
    if (!p.isWordLike && words.length) words[words.length - 1] += p.segment;
    else words.push(p.segment);
  }
  return { words, glue: '' };
}

export function readAlong({ read, pill, view, onSegment, onQuiet }) {
  const LEAD = spring('--read-lead'), TRAIL = spring('--read-trail');
  const L = S(0, LEAD), R = S(0, LEAD), Y = S(0, spring('--read-line')), O = S(0, spring('--read-fade'));
  const scroll = S(0, spring('--spring-soft'));
  const segments = [];
  let at = null, wait = 0, idle = 0, lineTop = null, quiet = true;

  const em = () => parseFloat(getComputedStyle(read).fontSize);
  const flat = () => segments.flatMap(s => s.words.map((el, i) => ({ el, s, i })));
  const cursor = list => at ? list.findIndex(w => w.s === at.s && w.i === at.i) : -1;

  function paint(list, n) {
    list.forEach(({ el }, k) => {
      el.classList.toggle('past', k < n);
      el.classList.toggle('now', k === n);
    });
  }

  // Whole lines only, so no line is ever cut in half.
  function fit() {
    const line = em() * 1.45;
    read.parentElement.style.maxHeight = Math.max(1, Math.floor(view.parentElement.clientHeight && room() / line)) * line + 'px';
  }
  function room() {
    const box = view.parentElement, style = getComputedStyle(box);
    const used = [...box.children].filter(c => c !== view && !c.hidden).reduce((n, c) => n + c.offsetHeight, 0);
    const gap = view.previousElementSibling?.hidden ? 0 : parseFloat(style.rowGap);
    return box.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom) - used - gap;
  }

  function place(el) {
    const size = em(), p = size * PAD;
    const b = {
      l: el.offsetLeft - p,
      r: el.offsetLeft + el.offsetWidth + p,
      top: el.offsetTop + (el.offsetHeight - size * 1.3) / 2,
    };
    const forward = b.l >= L.t;
    if (lineTop === null || Math.abs(b.top - lineTop) > 2 || O.x < .05) {
      jump(L, b.l); jump(R, b.l); jump(Y, b.top);
    }
    lineTop = b.top;
    tune(forward ? R : L, LEAD);
    tune(forward ? L : R, TRAIL);
    L.t = b.l; R.t = b.r; Y.t = b.top; O.t = 1;
    // Keep the line being read, and one line of context above it, in view.
    const line = size * 1.45, lines = Math.round(view.clientHeight / line);
    const here = Math.round((b.top + size * .65 - line / 2) / line);
    const total = Math.round(read.scrollHeight / line);
    const top = lines > 1 ? Math.min(here - 1, total - lines) : here;
    scroll.t = -Math.max(0, top) * line;
  }

  function show(list = flat()) {
    const n = cursor(list);
    paint(list, n);
    if (n < 0) { O.t = 0; return; }
    place(list[n].el);
    onSegment(list[n].s);
  }

  function prune() {
    while (segments.length > KEEP && at && segments.indexOf(at.s) >= KEEP) {
      const old = segments.shift(), before = read.scrollHeight;
      old.el.remove();
      const gone = before - read.scrollHeight;
      for (const s of [L, R]) s.v = 0;
      Y.x -= gone; Y.t -= gone; lineTop -= gone;
      scroll.x += gone; scroll.t += gone;
    }
  }

  function advance() {
    const list = flat(), n = cursor(list), next = list[n + 1];
    if (!next || next.i >= next.s.stable) return false;
    at = { s: next.s, i: next.i };
    const backlog = list.slice(n + 1).filter(w => w.i < w.s.stable).length;
    const rush = 1 + Math.max(0, backlog - CALM) * .35;
    wait = (.2 + next.el.textContent.trim().length * .035) / rush;
    prune();
    show();
    return true;
  }

  // Same segment id replaces its words in place, so the pill never loses its spot.
  function update({ id, text, language, stable, origin }) {
    let s = segments.find(x => x.id === id);
    if (!s) {
      s = { id, origin, words: [], stable: 0, el: document.createElement('span') };
      s.el.className = 'seg';
      read.append(s.el);
      segments.push(s);
    }
    const { words, glue } = split(text, language);
    words.forEach((w, i) => {
      let el = s.words[i];
      if (!el) {
        el = document.createElement('span');
        el.className = 'w';
        el.addEventListener('click', () => { if (i < s.stable) { at = { s, i }; wait = .4; show(); wake(); } });
        s.words.push(el);
        // The space stays outside the word, so the pill fits the word itself.
        s.el.append(...(i ? [glue, el] : [el]));
      }
      if (el.textContent !== w) el.textContent = w;
    });
    s.words.splice(words.length).forEach(el => {
      if (el.previousSibling?.nodeType === Node.TEXT_NODE) el.previousSibling.remove();
      el.remove();
    });
    s.stable = stable >= words.length || glue === '' && stable > 0 ? words.length : stable;
    if (at?.s === s && at.i >= words.length) at = words.length ? { s, i: words.length - 1 } : null;
    idle = 0;
    if (quiet) { quiet = false; onQuiet(false); }
    if (at) show();
    wake();
  }

  function clear() {
    segments.splice(0).forEach(s => s.el.remove());
    at = lineTop = null; O.t = 0; jump(scroll, 0);
    quiet = true; onQuiet(true);
    wake();
  }

  // Waiting for the next word or for silence needs a timer, not frames.
  let timer = null;
  function schedule(seconds) {
    clearTimeout(timer);
    timer = setTimeout(wake, seconds * 1000);
  }
  let stamp = performance.now();
  register({
    springs: [L, R, Y, O, scroll],
    tick() {
      const now = performance.now(), passed = (now - stamp) / 1000;
      stamp = now;
      wait -= passed; idle += passed;
      if (wait <= 0 && advance()) idle = 0;
      if (wait > 0) schedule(wait + .005);
      else if (!quiet && idle >= QUIET_AFTER) { quiet = true; O.t = 0; onQuiet(true); }
      else if (!quiet) schedule(QUIET_AFTER - idle + .02);
      return false;
    },
    render() {
      pill.style.transform = `translate(${L.x.toFixed(2)}px,${Y.x.toFixed(2)}px)`;
      pill.style.width = Math.max(0, R.x - L.x).toFixed(2) + 'px';
      pill.style.opacity = clamp(O.x, 0, 1).toFixed(3);
      read.style.transform = `translateY(${scroll.x.toFixed(2)}px)`;
    },
  });
  new ResizeObserver(() => { fit(); lineTop = null; if (at) { show(); wake(); } }).observe(view.parentElement);
  return { update, clear };
}
