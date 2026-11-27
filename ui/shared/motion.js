// Springs and one frame loop that sleeps when nothing moves.
const reduce = matchMedia('(prefers-reduced-motion: reduce)');
const style = getComputedStyle(document.documentElement);

export const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
export const num = name => parseFloat(style.getPropertyValue(name));
export const spring = name => style.getPropertyValue(name).trim().split(/\s+/).map(Number);

export function S(x, [k, c]) {
  const s = { x, v: 0, t: x };
  tune(s, [k, c]);
  return s;
}
export function tune(s, [k, c]) {
  s.k = k;
  s.c = reduce.matches ? 2 * Math.sqrt(k) : c;
}
function jump(s, x) { s.x = s.t = x; s.v = 0; }

function step(s, dt) {
  s.v += (-s.k * (s.x - s.t) - s.c * s.v) * dt;
  s.x += s.v * dt;
}
function resting(s) {
  if (Math.abs(s.x - s.t) > .004 || Math.abs(s.v) > .02) return false;
  jump(s, s.t);
  return true;
}

// A component is { springs, render, tick? }. tick(dt) returns true while it needs more frames.
const parts = [];
let running = false, last = 0;

export function register(part) {
  parts.push(part);
  wake();
}
export function wake() {
  if (running) return;
  running = true;
  last = performance.now();
  requestAnimationFrame(loop);
}
function loop(now) {
  const dt = Math.min((now - last) / 1000, 1 / 30);
  last = now;
  let busy = false;
  for (const p of parts) if (p.tick?.(dt)) busy = true;
  for (const p of parts) {
    for (let i = 0; i < 4; i++) for (const s of p.springs) step(s, dt / 4);
    for (const s of p.springs) if (!resting(s)) busy = true;
    p.render();
  }
  running = busy;
  if (busy) requestAnimationFrame(loop);
}
