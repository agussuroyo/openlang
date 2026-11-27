// Movie style subtitles: only the newest segment, its original above its translation.
const QUIET_AFTER = 6000;  // ms with nothing new
const KEEP = 40;           // originals kept while their translation is on the way

export function subtitles({ original, translation, onQuiet }) {
  const originals = new Map();  // segment id -> original text
  let current = null, lead = 'translation', timer = null;

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
    if (lead === 'original' || id === current) show(id);
  }

  function said(id, text) {
    if (lead === 'original') return;
    show(id);
    translation.textContent = text;
  }

  function clear() {
    clearTimeout(timer);
    originals.clear();
    current = null;
    original.textContent = translation.textContent = '';
    onQuiet(true);
  }

  return { follow, heard, said, clear };
}
