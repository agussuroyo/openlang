// Morph island. Width is stiffer than height, so a change lands sideways first, then drops.
import { S, spring, num, tune, clamp, register, wake } from '../shared/motion.js';

const SIZE = {
  idle: [160, 44], starting: [196, 28], stopping: [110, 28], listening: [128, 28],
  panel: [310, 44], preparing: [250, 44], error: [344, 64],
};
const INSET = 24;   // room between the content and the island edge

export function createIsland(island, onSize) {
  const w = spring('--island-w'), h = spring('--island-h'), radius = num('--island-radius');
  const W = S(SIZE.idle[0], w), H = S(SIZE.idle[1], h), press = S(1, spring('--press'));
  const contents = [...island.querySelectorAll('.ic')];
  let state = null;

  // SIZE is the smallest shape. Longer text, such as "AUTO to ID", makes it wider.
  function measure(name) {
    const [w, h] = SIZE[name], content = contents.find(c => c.dataset.s === name);
    return [Math.max(w, Math.ceil(content.offsetWidth) + INSET), h];
  }

  function go(next) {
    if (next === state) return;
    const [tw, th] = measure(next), grow = tw * th > W.t * H.t;
    tune(H, [h[0] * (grow ? 1 : 1.3), h[1]]);
    W.t = tw; H.t = th;
    if (state) press.v -= grow ? 1.6 : 1;
    state = next;
    onSize([tw, th]);
    contents.forEach(c => c.classList.toggle('on', c.dataset.s === next));
    wake();
  }

  function refit() {
    if (!state) return;
    const [tw, th] = measure(state);
    if (tw === W.t) return;
    W.t = tw;
    onSize([tw, th]);
    wake();
  }
  island.addEventListener('pointerdown', () => { press.t = .96; wake(); });
  addEventListener('pointerup', () => { press.t = 1; wake(); });

  register({
    springs: [W, H, press],
    render() {
      const height = Math.max(H.x, 20), give = clamp(-H.v / 7000, -.05, .05);
      island.style.width = W.x * (1 + give) + 'px';
      island.style.height = height + 'px';
      island.style.borderRadius = Math.min(height / 2, radius) + 'px';
      island.style.transform = `scale(${press.x})`;
    },
  });
  return { go, refit, get state() { return state; } };
}
