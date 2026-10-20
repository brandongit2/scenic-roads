// The thin load bar along the top of the map while the first view's road tiles come in, and the
// banner for a backend that can't be reached.
const bar = () => document.getElementById('loadbar')!;

export const loadBar = {
  /** Progress, 0–1 (never backwards). */
  set(f: number) {
    const el = bar();
    const w = Math.max(Number(el.dataset.f ?? 0), Math.max(0, Math.min(1, f)));
    el.dataset.f = String(w);
    el.style.transform = `scaleX(${0.04 + 0.96 * w})`;
  },
  done() {
    const el = bar();
    el.style.transform = 'scaleX(1)';
    el.classList.add('done');
  },
  /** Ends the bar and shows `msg` in a banner that stays. */
  fail(msg: string) {
    bar().classList.add('done');
    const b = document.getElementById('banner')!;
    b.textContent = msg;
    b.hidden = false;
  },
};
