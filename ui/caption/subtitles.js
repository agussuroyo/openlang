// Movie style subtitles: only the newest segment, its original above its translation.
const QUIET_AFTER = 6000;       // ms with nothing new
const KEEP = 40;                // originals kept while their translation is on the way
const WAIT_TRANSLATION = 1500;  // ms a newer original waits for its translation before it is shown alone

// Keeps the newest whole lines of a line's text in view. The text slides up when a line is added and jumps
// back without sliding when it gets shorter, so a new sentence never comes down from above.
function newestLines(span) {
  const slot = span.parentElement;
  let shift = 0;
  const fit = () => {
    const line = parseFloat(getComputedStyle(span).lineHeight);
    const lines = Math.floor(slot.getBoundingClientRect().height / line + 0.05);
    slot.classList.toggle('cramped', lines < 1);
    const next = Math.max(0, span.offsetHeight - Math.max(1, lines) * line);
    if (next === shift) return;
    span.style.transition = next > shift ? '' : 'none';
    span.style.transform = next ? `translateY(${-next}px)` : '';
    shift = next;
  };
  const watch = new ResizeObserver(fit);
  watch.observe(span);
  watch.observe(slot);
}

export function subtitles({ original, translation, onQuiet }) {
  newestLines(original);
  newestLines(translation);
  const originals = new Map();  // segment id -> original text
  // Segment ids are random, so their order is the order they were first seen. Two sources (system audio
  // and microphone) interleave, and a late result of an older segment must not pull the caption back.
  const order = new Map();      // segment id -> sequence number
  let current = null, lead = 'translation', timer = null, waiting = null, latest = null, seq = 0;

  function rank(id) {
    if (!order.has(id)) {
      order.set(id, ++seq);
      if (order.size > KEEP) order.delete(order.keys().next().value);
    }
    return order.get(id);
  }

  const older = id => rank(id) < (order.get(current) ?? 0);

  function wake() {
    clearTimeout(timer);
    onQuiet(false);
    timer = setTimeout(() => onQuiet(true), QUIET_AFTER);
  }

  function show(id) {
    if (id !== current) {
      current = id;
      translation.textContent = '';
    }
    original.textContent = originals.get(id) ?? '';
    wake();
  }

  // Translation leads unless only the original is shown, so both lines stay on the same sentence.
  function follow(mode) {
    lead = mode === 'original' ? 'original' : 'translation';
  }

  function heard(id, text) {
    originals.set(id, text);
    if (originals.size > KEEP) originals.delete(originals.keys().next().value);
    if (older(id)) return;
    if (lead === 'original' || id === current) return show(id);
    // A slow or failed translation must not leave the caption on the previous sentence.
    latest = id;
    waiting ??= setTimeout(() => {
      waiting = null;
      if (latest !== current && !older(latest)) show(latest);
    }, WAIT_TRANSLATION);
  }

  function said(id, text) {
    if (lead === 'original' || older(id)) return;
    if (rank(id) >= order.get(latest)) {
      clearTimeout(waiting);
      waiting = null;
    }
    show(id);
    translation.textContent = text;
  }

  function clear() {
    clearTimeout(timer);
    clearTimeout(waiting);
    waiting = latest = null;
    originals.clear();
    order.clear();
    current = null;
    original.textContent = translation.textContent = '';
    onQuiet(true);
  }

  return { follow, heard, said, clear };
}
