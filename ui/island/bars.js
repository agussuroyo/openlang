// Three bars that follow the loudest source. Each bar reacts a little differently, like a voice.
const WEIGHTS = [0.65, 1, 0.8];
const REST = 0.25;

export function soundBars(root) {
  const bars = [...root.children];
  let levels = {};

  function draw() {
    const loudest = Math.max(0, ...Object.values(levels));
    bars.forEach((bar, i) => {
      const scale = REST + (1 - REST) * Math.min(1, loudest * WEIGHTS[i]);
      bar.style.transform = `scaleY(${scale.toFixed(3)})`;
    });
  }

  return {
    set(origin, level) { levels[origin] = level; draw(); },
    reset() { levels = {}; draw(); },
  };
}
