// Three bars that follow the loudest source. Each bar reacts a little differently, like a voice.
const WEIGHTS = [0.65, 1, 0.8];
const REST = 0.2;
// Levels come every 50 ms while there is sound. Without them the bars drop instead of freezing.
const STALE_MS = 300;

export function soundBars(root) {
  const bars = [...root.children];
  let levels = {}, stale = null;

  function draw() {
    const loudest = Math.max(0, ...Object.values(levels));
    bars.forEach((bar, i) => {
      const scale = REST + (1 - REST) * Math.min(1, loudest * WEIGHTS[i]);
      bar.style.transform = `scaleY(${scale.toFixed(3)})`;
    });
  }

  function reset() {
    clearTimeout(stale);
    levels = {};
    draw();
  }

  function set(origin, level) {
    levels[origin] = level;
    draw();
    clearTimeout(stale);
    stale = setTimeout(reset, STALE_MS);
  }

  return { set, reset };
}
