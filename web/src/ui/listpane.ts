// The "in view" list tabs' shared machinery (Scenic drives: drives.ts; Scenic rides and Rail lines:
// rides.ts): loads after a pause, again only when the query changes, the last one aborted; a count
// line and a spinner; ranked rows that hover and select.
import { h } from './dom';

/** Lengths offered for a stretch (km). */
export const LENGTHS = [2, 5, 10, 25];

export abstract class ListPane<T, Q extends { weights: number[] }> {
  protected list = h('div', { class: 'picks' });
  protected count = h('span', { class: 'faint' });
  protected spin = h('span', { class: 'spin', hidden: true });
  private abort: AbortController | null = null;
  private timer = 0;
  protected lastKey = '';
  onHover: (x: T | null) => void = () => {};
  onSelect: (x: T) => void = () => {};
  /** The row under the pointer (G, M, O open it). */
  hovered: T | null = null;

  /** `unavailable`: what the list says when its query fails. */
  constructor(readonly root: HTMLElement, private unavailable: string, public query: () => Q) {}

  refresh(now = false) {
    if (this.root.hidden) return;
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.load(), now ? 0 : 300);
  }

  /** What the list depends on besides the query's area and weights (`w`, formatted). */
  protected abstract key(q: Q, w: string): string;
  protected abstract fetch(q: Q, w: string, signal: AbortSignal): Promise<void>;
  /** The query failed (after the list says so). */
  protected failed() {}

  private async load() {
    const q = this.query();
    const w = q.weights.map((v) => v.toFixed(2)).join(',');
    const key = this.key(q, w);
    if (key === this.lastKey) return;
    this.lastKey = key;
    this.abort?.abort();
    this.abort = new AbortController();
    this.spin.hidden = false;
    try {
      await this.fetch(q, w, this.abort.signal);
    } catch (e) {
      if ((e as Error).name !== 'AbortError') {
        this.list.replaceChildren(h('div', { class: 'muted' }, this.unavailable));
        this.failed();
      }
    } finally {
      this.spin.hidden = true;
    }
  }

  /** A segmented choice of stretch lengths, `on` marking the one chosen. */
  protected lengthSeg(noun: string, pick: (l: number) => void): { el: HTMLDivElement; on: (l: number) => void } {
    const btns: HTMLButtonElement[] = [];
    const el = h('div', { class: 'seg small' });
    for (const l of LENGTHS) {
      const b = h('button', { title: `Best ${l} km stretch of each ${noun}`, onclick: () => pick(l) }, `${l} km`);
      btns.push(b);
      el.append(b);
    }
    return { el, on: (l) => btns.forEach((b, i) => b.classList.toggle('on', LENGTHS[i] === l)) };
  }

  /** A ranked row: its number, title and score, a bar, and the line under it. */
  protected row(i: number, title: Node, score: number, sub: string, x: T) {
    const bar = h('i', { class: 'sbar' });
    bar.style.width = `${Math.max(4, score)}%`;
    const r = h('a', { class: 'pick', onclick: () => this.onSelect(x) },
      h('span', { class: 'rank' }, String(i + 1)),
      h('div', { class: 'cbody' },
        h('div', { class: 'cl1' }, title, h('b', {}, score.toFixed(0))),
        h('div', { class: 'sbarw' }, bar),
        h('div', { class: 'cl2' }, sub),
      ),
    );
    r.addEventListener('mouseenter', () => this.onHover((this.hovered = x)));
    r.addEventListener('mouseleave', () => this.onHover((this.hovered = null)));
    return r;
  }
}
