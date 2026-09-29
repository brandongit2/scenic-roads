// The app's colour-map picker: a dropdown whose options show their gradients under section
// headings; hovering (or arrowing to) an option previews it live, leaving the list or Escape
// reverts, a click commits. A ⇄ toggle beside it reverses the ramp (key + "_r"); the list then
// shows every ramp reversed.
import { baseKey, isRev, withRev } from '../palettes';
import { h } from './dom';

export interface RampItem {
  key: string;
  label: string;
  /** Section heading (consecutive items with the same group share one). */
  group?: string;
}

export class RampSelect {
  readonly el: HTMLSpanElement;
  private btn: HTMLButtonElement;
  private rev: HTMLButtonElement;
  private swatch: HTMLSpanElement;
  private label: HTMLSpanElement;
  private menu: HTMLDivElement | null = null;
  private rows: HTMLDivElement[] = [];
  private active = -1;
  private value = '';
  private off: (() => void)[] = [];

  constructor(
    private items: RampItem[],
    /** CSS gradient for a ramp key (reflecting current bands, emphasis, fade). */
    private css: (key: string) => string,
    private onPick: (key: string) => void,
    private onPreview: (key: string | null) => void,
  ) {
    this.swatch = h('span', { class: 'ramp-sw' });
    this.label = h('span', { class: 'ramp-lb' });
    this.btn = h('button', { class: 'ramp-btn', type: 'button', title: 'Colour ramp (hover an option to preview it on the map)' }, this.swatch, this.label, h('i', {}, '▾'));
    this.rev = h('button', { class: 'ramp-rev', type: 'button', title: 'Reverse the colour ramp' }, '⇄');
    this.rev.addEventListener('click', () => {
      const key = withRev(this.value, !isRev(this.value));
      this.set(key);
      this.onPick(key);
    });
    this.el = h('span', { class: 'ramp-pick' }, this.btn, this.rev);
    this.btn.addEventListener('click', () => (this.menu ? this.close(true) : this.open()));
    this.btn.addEventListener('keydown', (e) => {
      if (!this.menu && (e.key === 'ArrowDown' || e.key === 'Enter' || e.key === ' ')) {
        e.preventDefault();
        this.open();
      } else if (this.menu) this.keys(e);
    });
  }

  set(key: string) {
    this.value = key;
    this.refresh();
  }

  /** Re-render the button's swatch (the ramps depend on other settings). */
  refresh() {
    const base = baseKey(this.value);
    const it = this.items.find((i) => i.key === base) ?? this.items[0];
    this.swatch.style.background = this.css(this.keyOf(it));
    this.label.textContent = it.label;
    this.rev.classList.toggle('on', isRev(this.value));
  }

  /** An item's key in the current direction. */
  private keyOf(it: RampItem) {
    return withRev(it.key, isRev(this.value));
  }

  private open() {
    const r = this.btn.getBoundingClientRect();
    const base = baseKey(this.value);
    const nodes: HTMLElement[] = [];
    this.rows = this.items.map((it, i) => {
      if (it.group && it.group !== this.items[i - 1]?.group) nodes.push(h('div', { class: 'ramp-group' }, it.group));
      const sw = h('span', { class: 'ramp-sw' });
      sw.style.background = this.css(this.keyOf(it));
      const row = h('div', { class: 'ramp-item' + (it.key === base ? ' sel' : ''), role: 'option' }, sw, h('span', {}, it.label));
      row.addEventListener('mouseenter', () => this.highlight(i));
      row.addEventListener('click', () => this.pick(i));
      nodes.push(row);
      return row;
    });
    const menu = h('div', { class: 'ramp-menu', role: 'listbox' }, ...nodes);
    menu.addEventListener('mouseleave', () => {
      this.active = -1;
      this.rows.forEach((x) => x.classList.remove('active'));
      this.onPreview(null);
    });
    document.body.append(menu);
    // Below the button, or above it if there's no room; right-aligned to the button.
    const mh = menu.offsetHeight, mw = Math.max(r.width, menu.offsetWidth);
    menu.style.width = `${mw}px`;
    menu.style.left = `${Math.max(8, Math.min(window.innerWidth - mw - 8, r.right - mw))}px`;
    menu.style.top = `${r.bottom + 4 + mh < window.innerHeight ? r.bottom + 4 : Math.max(8, r.top - 4 - mh)}px`;
    this.menu = menu;
    this.active = this.items.findIndex((i) => i.key === base);
    this.rows[this.active]?.scrollIntoView({ block: 'nearest' });
    const outside = (e: PointerEvent) => {
      if (!menu.contains(e.target as Node) && !this.btn.contains(e.target as Node)) this.close(true);
    };
    const esc = (e: KeyboardEvent) => this.keys(e);
    const away = () => this.close(true);
    document.addEventListener('pointerdown', outside, true);
    document.addEventListener('keydown', esc, true);
    window.addEventListener('resize', away);
    window.addEventListener('blur', away);
    this.off = [
      () => document.removeEventListener('pointerdown', outside, true),
      () => document.removeEventListener('keydown', esc, true),
      () => window.removeEventListener('resize', away),
      () => window.removeEventListener('blur', away),
    ];
  }

  private keys(e: KeyboardEvent) {
    if (!this.menu) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      this.close(true);
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const n = this.items.length;
      this.highlight(((this.active < 0 ? 0 : this.active + (e.key === 'ArrowDown' ? 1 : -1)) + n) % n);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (this.active >= 0) this.pick(this.active);
    }
  }

  private highlight(i: number) {
    this.active = i;
    this.rows.forEach((x, k) => x.classList.toggle('active', k === i));
    this.rows[i]?.scrollIntoView({ block: 'nearest' });
    this.onPreview(this.keyOf(this.items[i]));
  }

  private pick(i: number) {
    const key = this.keyOf(this.items[i]);
    this.close(false);
    this.onPreview(null);
    this.set(key);
    this.onPick(key);
  }

  private close(revert: boolean) {
    this.menu?.remove();
    this.menu = null;
    this.off.forEach((f) => f());
    this.off = [];
    if (revert) this.onPreview(null);
    this.btn.focus();
  }
}
