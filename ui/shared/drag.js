// Drag a floating window from anywhere on it. A press that does not travel stays a click.
import { invoke } from './app.js';

const SLOP = 4;

export function draggable(name) {
  let from = null, dragging = false, queued = null, swallow = false;

  function flush() {
    if (!queued) return;
    const [dx, dy] = queued;
    queued = null;
    invoke('drag_move', { dx, dy });
  }
  addEventListener('pointerdown', e => {
    if (e.button === 0) from = { x: e.screenX, y: e.screenY, id: e.pointerId };
    swallow = false;
  });
  addEventListener('pointermove', e => {
    if (!from || e.pointerId !== from.id) return;
    const dx = e.screenX - from.x, dy = e.screenY - from.y;
    if (!dragging) {
      if (Math.hypot(dx, dy) < SLOP) return;
      dragging = swallow = true;
      document.documentElement.setPointerCapture(from.id);
      document.body.classList.add('dragging');
      invoke('drag_start', { window: name });
    }
    if (!queued) requestAnimationFrame(flush);
    queued = [dx, dy];
  });
  function end() {
    from = null;
    if (!dragging) return;
    dragging = false;
    document.body.classList.remove('dragging');
    flush();
    invoke('drag_end');
  }
  addEventListener('pointerup', end);
  addEventListener('pointercancel', end);
  addEventListener('click', e => {
    if (!swallow) return;
    swallow = false;
    e.stopPropagation();
    e.preventDefault();
  }, true);
}
