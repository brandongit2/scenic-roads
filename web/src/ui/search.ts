// Place search, top right beside the view controls: the map's own place names (its labels: towns
// and cities, lakes and bays, parks, states), found by any word of their name or their English as
// typed (the map's server, /api/places: crates/server/src/places.rs), the best first and, among
// like ones, the nearest to the view. ↑ ↓ pick, Return goes, Esc closes then clears; / (outside a
// field) opens it. Opened, it has the server make its index (the first time after a new catalog),
// so it's ready by the time the words are typed.
import { fmt, h } from './dom';

export type Place = { name: string; en: string | null; kind: string; class: string; lon: number; lat: number; zoom: number };

/** Great-circle distance (km). */
function km(a: [number, number], b: [number, number]): number {
  const r = Math.PI / 180;
  const dp = (b[1] - a[1]) * r, dl = (b[0] - a[0]) * r;
  const s = Math.sin(dp / 2) ** 2 + Math.cos(a[1] * r) * Math.cos(b[1] * r) * Math.sin(dl / 2) ** 2;
  return 6371 * 2 * Math.asin(Math.min(1, Math.sqrt(s)));
}

/** What a place is, in words: its class as OSM names it ("national_park" → "national park"). */
const what = (p: Place) => (p.kind === 'state' && p.class === 'state' ? 'state' : p.class.replace(/_/g, ' '));

export class PlaceSearch {
  /** A place picked: the map goes there. */
  onGo: (p: Place) => void = () => {};
  /** The search cleared: what it marked goes. */
  onClear: () => void = () => {};
  private input: HTMLInputElement;
  private list: HTMLDivElement;
  private hits: Place[] = [];
  private pick = -1;
  private timer = 0;
  private abort: AbortController | null = null;
  /** The query the list shows (or is being asked). */
  private shown = '';

  constructor(private root: HTMLElement, private near: () => [number, number]) {
    this.input = h('input', { type: 'search', placeholder: 'Search places', 'aria-label': 'Search places', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', enterkeyhint: 'go', role: 'combobox', 'aria-expanded': 'false', 'aria-controls': 'search-hits' });
    this.list = h('div', { class: 'hits', id: 'search-hits', role: 'listbox', hidden: true });
    const clear = h('button', { type: 'button', class: 'x', title: 'Clear (Esc)', 'aria-label': 'Clear', onclick: () => this.clear(true) }, '✕');
    root.append(h('div', { class: 'row' }, h('span', { class: 'glass', 'aria-hidden': 'true' }), this.input, clear), this.list);
    root.addEventListener('submit', (e) => {
      e.preventDefault();
      this.go(Math.max(0, this.pick));
    });
    this.input.addEventListener('input', () => {
      root.classList.toggle('typed', this.input.value !== '');
      this.soon();
    });
    this.input.addEventListener('focus', () => {
      root.classList.add('open');
      // (Has the server make the places, the first time: ready by the time the words are.)
      if (!this.input.value.trim()) void this.ask('');
      else this.show(this.hits.length > 0 || this.shown !== '');
    });
    this.input.addEventListener('keydown', (e) => this.key(e));
    // (Left: closed, but after a pointer's press on a hit has landed.)
    root.addEventListener('focusout', (e) => {
      if (root.contains(e.relatedTarget as Node | null)) return;
      setTimeout(() => {
        if (root.contains(document.activeElement)) return;
        root.classList.remove('open');
        this.show(false);
      }, 120);
    });
  }

  /** Opens it (the `/` key). */
  focus() {
    this.input.focus();
    this.input.select();
  }

  private key(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (!this.hits.length) return;
      if (this.list.hidden) this.show(true);
      const n = this.hits.length;
      this.select(this.pick < 0 ? (e.key === 'ArrowDown' ? 0 : n - 1) : (this.pick + (e.key === 'ArrowDown' ? 1 : n - 1)) % n);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (!this.list.hidden) this.show(false);
      else if (this.input.value) this.clear(false);
      else this.input.blur();
    }
  }

  private soon() {
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => void this.ask(this.input.value), 110);
  }

  /** Asks the server for `q`'s places (an empty `q`: only that it makes them); not ready, again. */
  private async ask(q: string) {
    this.abort?.abort();
    const ac = (this.abort = new AbortController());
    const [lon, lat] = this.near();
    const url = `/api/places?${new URLSearchParams({ q, near: `${lon.toFixed(4)},${lat.toFixed(4)}`, n: '8' })}`;
    let body: { ready: boolean; hits: Place[] } | null = null;
    try {
      const r = await fetch(url, { cache: 'no-store', signal: ac.signal });
      body = r.ok ? await r.json() : null;
      if (!r.ok) throw new Error(`the map's server answered ${r.status}`);
    } catch (e) {
      if ((e as Error).name === 'AbortError') return;
      if (q.trim()) this.state(`Couldn't search: ${(e as Error).message}`);
      return;
    }
    if (ac !== this.abort || !body) return;
    if (!body.ready) {
      // (Made now, once after a new catalog: asked again in a moment, while it's open.)
      if (q.trim()) this.state('Finding the map’s places… (once after each map update)');
      this.timer = window.setTimeout(() => {
        if (document.activeElement === this.input || this.input.value.trim()) void this.ask(this.input.value);
      }, 1200);
      return;
    }
    if (!q.trim()) return;
    this.shown = q;
    this.hits = body.hits;
    this.render();
  }

  private state(text: string) {
    this.hits = [];
    this.pick = -1;
    this.list.replaceChildren(h('div', { class: 'state' }, text));
    this.show(true);
  }

  private render() {
    this.pick = this.hits.length ? 0 : -1;
    if (!this.hits.length) return this.state('No place of that name on the map');
    const here = this.near();
    this.list.replaceChildren(
      ...this.hits.map((p, i) => {
        const en = p.en && p.en !== p.name ? p.en : null;
        const row = h('div', { class: 'hit', role: 'option', id: `search-hit-${i}`, 'aria-selected': i === this.pick ? 'true' : 'false' },
          h('b', { title: p.name }, p.name),
          h('span', { class: 'what' }, what(p)),
          h('span', { class: 'en' }, en ?? ''),
          h('span', { class: 'far', title: 'From the middle of the view' }, fmt.km(km(here, [p.lon, p.lat]))),
        );
        row.classList.toggle('on', i === this.pick);
        // (On the press: before the field's blur closes the list.)
        row.addEventListener('pointerdown', (e) => {
          e.preventDefault();
          this.go(i);
        });
        row.addEventListener('pointermove', () => this.select(i));
        return row;
      }),
    );
    this.show(true);
  }

  private select(i: number) {
    this.pick = i;
    this.list.querySelectorAll('.hit').forEach((el, k) => {
      el.classList.toggle('on', k === i);
      el.setAttribute('aria-selected', k === i ? 'true' : 'false');
    });
    this.input.setAttribute('aria-activedescendant', `search-hit-${i}`);
    (this.list.children[i] as HTMLElement | undefined)?.scrollIntoView({ block: 'nearest' });
  }

  private show(on: boolean) {
    this.list.hidden = !on;
    this.input.setAttribute('aria-expanded', on ? 'true' : 'false');
  }

  private go(i: number) {
    const p = this.hits[i];
    if (!p) return;
    this.input.value = p.name;
    this.root.classList.add('typed');
    this.show(false);
    this.input.blur();
    this.onGo(p);
  }

  private clear(keepFocus: boolean) {
    clearTimeout(this.timer);
    this.abort?.abort();
    this.input.value = '';
    this.root.classList.remove('typed');
    this.hits = [];
    this.shown = '';
    this.pick = -1;
    this.show(false);
    this.onClear();
    if (keepFocus) this.input.focus();
  }
}
