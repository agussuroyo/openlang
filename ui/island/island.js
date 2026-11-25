// Morph island. Width is stiffer than height, so a change lands sideways first, then drops.
import { S, spring, num, tune, clamp, register, wake } from '../shared/motion.js';

const SIZE = {
  idle: [176, 30], starting: [196, 28], stopping: [110, 28], listening: [128, 28],
  panel: [310, 44], preparing: [250, 44], error: [344, 64],
};

export function createIsland(island, onSize) {
  const w = spring('--island-w'), h = spring('--island-h'), radius = num('--island-radius');
  const W = S(SIZE.idle[0], w), H = S(SIZE.idle[1], h), press = S(1, spring('--press'));
  const contents = [...island.querySelectorAll('.ic')];
  let state = null;

  function go(next) {
    if (next === state) return;
    const [tw, th] = SIZE[next], grow = tw * th > W.t * H.t;
    tune(H, [h[0] * (grow ? 1 : 1.3), h[1]]);
    W.t = tw; H.t = th;
    if (state) press.v -= grow ? 1.6 : 1;
    state = next;
    onSize(SIZE[next]);
    contents.forEach(c => c.classList.toggle('on', c.dataset.s === next));
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
  return { go, get state() { return state; } };
}
