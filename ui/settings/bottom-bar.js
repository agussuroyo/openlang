// Bottom bar. Each label has its own width spring; the pill follows the picked tab.
import { S, spring, clamp, register, wake } from '../shared/motion.js';

export function bottomBar(bar, onPick) {
  const hl = bar.querySelector('.hl');
  const tabs = [...bar.querySelectorAll('.it')].map(el => {
    const nm = el.querySelector('.nm');
    nm.style.width = 'auto';
    const full = nm.offsetWidth + 8;
    nm.style.width = '0px';
    return { el, nm, svg: el.querySelector('svg'), full, w: S(0, spring('--bar-label')), p: S(1, spring('--bar-pop')) };
  });
  const X = S(0, spring('--bar-pill')), W = S(0, spring('--bar-pill'));
  let cur = -1;

  function pick(i, { animate = true, focus = false } = {}) {
    cur = i;
    tabs.forEach((t, k) => {
      t.el.setAttribute('aria-selected', k === i);
      t.el.tabIndex = k === i ? 0 : -1;
      t.w.t = k === i ? t.full : 0;
      if (!animate) { t.w.x = t.w.t; t.w.v = 0; }
    });
    if (animate) { tabs[i].p.x = .72; tabs[i].p.v = 6; }
    if (focus) tabs[i].el.focus();
    onPick(tabs[i].el.dataset.tab);
    wake();
  }
  tabs.forEach((t, i) => t.el.addEventListener('click', () => pick(i)));
  bar.addEventListener('keydown', e => {
    const move = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
    if (!move) return;
    e.preventDefault();
    // Keyboard moves happen often, so they land at once.
    pick((cur + move + tabs.length) % tabs.length, { animate: false, focus: true });
  });

  let first = true;
  register({
    springs: [...tabs.flatMap(t => [t.w, t.p]), X, W],
    render() {
      tabs.forEach(t => {
        const w = Math.max(0, t.w.x);
        t.nm.style.width = w.toFixed(2) + 'px';
        t.nm.style.paddingLeft = clamp(w / t.full, 0, 1) * 8 + 'px';
        t.nm.style.opacity = clamp(w / t.full * 1.4 - .4, 0, 1).toFixed(3);
        t.svg.style.transform = `scale(${t.p.x.toFixed(4)})`;
      });
      if (cur < 0) return;
      X.t = tabs[cur].el.offsetLeft;
      W.t = tabs[cur].el.offsetWidth;
      if (first && W.t) { X.x = X.t; W.x = W.t; first = false; }
      hl.style.transform = `translateX(${X.x.toFixed(2)}px)`;
      hl.style.width = W.x.toFixed(2) + 'px';
    },
  });
  pick(0, { animate: false });
}
