// Place search, top right beside the view controls: the map's own place names (its labels: towns
// and cities, lakes and bays, parks, states), found by any word of the name the map shows (its
// main and sub, plan §7) or of the place's own name and English, as typed (the map's server,
// /api/places: crates/server/src/places.rs), the best first and, among like ones, the nearest to
// the view; two letters at least (one ideograph or kana). ↑ ↓ pick, Return goes (to what's typed:
// searched first if the list isn't of it), Esc closes then clears; / (outside a field) opens it.
// Opened, it has the server make its index (the first time after the map's labels or translations
// change), so it's ready by the time the words are typed; until it is, the box asks again, less
// and less often, while it's open.
import { fmt, h } from './dom';
import { sameName } from '../names';

export type Place = { name: string; en: string | null; main: string; sub: string | null; kind: string; class: string; lon: number; lat: number; zoom: number };

/** The server's answer: `failed` (why) and `again` (in how many seconds it tries again) when its
 * last try to make the places failed. */
type Answer = { ready: boolean; hits: Place[]; failed?: string; again?: number };

/** Asked again while the places are made: after a second, doubling to ten. */
const AGAIN0 = 1000, AGAIN_MAX = 10000;

/** Great-circle distance (km). */
function km(a: [number, number], b: [number, number]): number {
  const r = Math.PI / 180;
  const dp = (b[1] - a[1]) * r, dl = (b[0] - a[0]) * r;
  const s = Math.sin(dp / 2) ** 2 + Math.cos(a[1] * r) * Math.cos(b[1] * r) * Math.sin(dl / 2) ** 2;
  return 6371 * 2 * Math.asin(Math.min(1, Math.sqrt(s)));
}

/** What a place is, in words: its class as OSM names it ("national_park" → "national park"). */
const what = (p: Place) => (p.kind === 'state' && p.class === 'state' ? 'state' : p.class.replace(/_/g, ' '));

/** A place's own name and English where the map shows others (a translation's): said after them,
 * muted. */
const others = (p: Place) => [p.name, p.en].filter((x, k, all): x is string => !!x && ![p.main, p.sub, ...all.slice(0, k)].some((y) => y && sameName(x, y)));

/** Whether a query is too short to search, as the server has it (places.rs `too_short`): one letter
 * of a script written with spaces would find every place with a word so starting; one ideograph
 * or kana is a word. */
export function tooShort(q: string): boolean {
  const s = [...q.normalize('NFKD').replace(/[^\p{L}\p{N}]/gu, '')];
  return s.length < 2 && !/[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}]/u.test(s.join(''));
}

export class PlaceSearch {
  /** A place picked: the map goes there. */
  onGo: (p: Place) => void = () => {};
  /** The search cleared (or its words gone): what it marked goes. */
  onClear: () => void = () => {};
  private input: HTMLInputElement;
  /** What opens under the box: the places found (a listbox), or what the search says. */
  private list: HTMLDivElement;
  private opts: HTMLDivElement;
  private status: HTMLDivElement;
  private hits: Place[] = [];
  private pick = -1;
  /** The pause after typing, and the next ask while the server makes its places (waiting `wait`). */
  private typing = 0;
  private again = 0;
  private wait = AGAIN0;
  private abort: AbortController | null = null;
  /** The query the hits are of. */
  private shown = '';

  constructor(private root: HTMLElement, private near: () => [number, number]) {
    this.input = h('input', { type: 'search', placeholder: 'Search places', 'aria-label': 'Search places', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', enterkeyhint: 'go', role: 'combobox', 'aria-expanded': 'false', 'aria-controls': 'search-hits', 'aria-autocomplete': 'list' });
    this.opts = h('div', { id: 'search-hits', role: 'listbox', 'aria-label': 'Places found' });
    this.status = h('div', { class: 'state', role: 'status', 'aria-live': 'polite' });
    this.list = h('div', { class: 'hits', hidden: true }, this.opts, this.status);
    const clear = h('button', { type: 'button', class: 'x', title: 'Clear (Esc)', 'aria-label': 'Clear', onclick: () => this.clear(true) }, '✕');
    root.append(h('div', { class: 'row' }, h('span', { class: 'glass', 'aria-hidden': 'true' }), this.input, clear), this.list);
    root.addEventListener('submit', (e) => {
      e.preventDefault();
      void this.enter();
    });
    this.input.addEventListener('input', () => this.typed());
    this.input.addEventListener('focus', () => this.opened());
    this.input.addEventListener('keydown', (e) => this.key(e));
    // (Left: closed, and no more asks; but after a press on a hit has landed.)
    root.addEventListener('focusout', (e) => {
      if (root.contains(e.relatedTarget as Node | null)) return;
      setTimeout(() => {
        if (root.contains(document.activeElement)) return;
        root.classList.remove('open');
        clearTimeout(this.again);
        this.show(false);
      }, 120);
    });
  }

  /** Opens it (the `/` key). */
  focus() {
    this.input.focus();
    this.input.select();
  }

  /** Whether it's on screen (the HUD off hides it). */
  get visible(): boolean {
    return this.root.getClientRects().length > 0;
  }

  private get focused(): boolean {
    return document.activeElement === this.input;
  }

  /** The words to ask for now: none while they're too few. */
  private get words(): string {
    const v = this.input.value;
    return tooShort(v) ? '' : v;
  }

  private key(e: KeyboardEvent) {
    // (Keys an input method composes with are its own: Safari says so by keyCode 229 alone.)
    if (e.isComposing || e.keyCode === 229) {
      if (e.key === 'Enter') e.preventDefault();
      return;
    }
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

  /** The box opened: the list as it was when it's of its words, else they're asked for (none: the
   * places made, the first time, so they're ready by the time the words are). */
  private opened() {
    this.root.classList.add('open');
    if (this.shown && this.shown === this.input.value) this.show(true);
    else void this.ask(this.words);
  }

  /** The words changed: searched after a pause; too few, nothing shown; none, cleared. */
  private typed() {
    const v = this.input.value;
    this.root.classList.toggle('typed', v !== '');
    if (!v.trim()) return this.emptied();
    this.stop();
    if (tooShort(v)) return this.state('');
    this.typing = window.setTimeout(() => void this.ask(v), 110);
  }

  /** Stops the asking: the pause after typing, the asks again, the ask under way. */
  private stop() {
    clearTimeout(this.typing);
    clearTimeout(this.again);
    this.wait = AGAIN0;
    this.abort?.abort();
    this.abort = null;
  }

  /** Asks the server for `q`'s places (an empty `q`: only that it makes them); while it makes them,
   * again (`poll`: not the map in use), less and less often, while the box is open. Whether a list
   * of `q`'s came. */
  private async ask(q: string, poll = false): Promise<boolean> {
    clearTimeout(this.again);
    this.abort?.abort();
    const ac = (this.abort = new AbortController());
    const [lon, lat] = this.near();
    const params = new URLSearchParams({ q, near: `${lon.toFixed(4)},${lat.toFixed(4)}`, n: '8' });
    if (poll) params.set('poll', '1');
    let body: Answer;
    try {
      const r = await fetch(`/api/places?${params}`, { cache: 'no-store', signal: ac.signal });
      if (!r.ok) throw new Error(`the map's server answered ${r.status}`);
      body = await r.json();
    } catch (e) {
      if ((e as Error).name === 'AbortError' || ac !== this.abort) return false;
      this.abort = null;
      if (q.trim()) this.state(`Couldn't search: ${(e as Error).message}`);
      return false;
    }
    if (ac !== this.abort) return false;
    this.abort = null;
    if (!body.ready) {
      if (q.trim()) {
        const again = body.again ? ` Tried again in ${body.again} s.` : '';
        this.state(body.failed ? `Couldn't find the map's places: ${body.failed}.${again}` : 'Finding the map’s places… (once after each map update)');
      }
      if (this.focused) {
        this.again = window.setTimeout(() => this.focused && void this.ask(this.words, true), this.wait);
        this.wait = Math.min(this.wait * 2, AGAIN_MAX);
      }
      return false;
    }
    this.wait = AGAIN0;
    if (!q.trim()) return false;
    this.shown = q;
    this.hits = body.hits;
    this.render();
    return true;
  }

  /** Return: the place picked, if the list is of the words typed; else they're searched now and the
   * first found goes. */
  private async enter() {
    const v = this.input.value;
    if (!v.trim()) return;
    if (tooShort(v)) return this.state('Two letters at least (or an ideograph or kana)');
    if (this.shown === v && this.hits.length) return this.go(Math.max(0, this.pick));
    this.stop();
    if ((await this.ask(v)) && this.input.value === v) this.go(0);
  }

  /** What the search says, no places listed (`text` empty: nothing to say, nothing shown). */
  private state(text: string) {
    this.hits = [];
    this.shown = '';
    this.render(text);
  }

  /** The list: the places found, or `none` when there are none; shown only while the box is open
   * (else kept for when it is). */
  private render(none = 'No place of that name on the map') {
    this.pick = this.hits.length ? 0 : -1;
    const here = this.near();
    this.opts.replaceChildren(
      ...this.hits.map((p, i) => {
        // (What the map shows, then the place's own names where they're other.)
        const more = others(p);
        const row = h('div', { class: 'hit', role: 'option', id: `search-hit-${i}`, 'aria-selected': i === this.pick ? 'true' : 'false' },
          h('b', { title: p.main }, p.main),
          h('span', { class: 'what' }, what(p)),
          h('span', { class: 'sub', title: [p.sub, ...more].filter(Boolean).join(' · ') || undefined }, p.sub ?? '', more.length ? h('span', { class: 'other' }, (p.sub ? ' · ' : '') + more.join(' · ')) : null),
          h('span', { class: 'far', title: 'From the middle of the view' }, fmt.km(km(here, [p.lon, p.lat]))),
        );
        row.classList.toggle('on', i === this.pick);
        // (Picked on the click, not the press: a finger's drag scrolls the list, and a tap never
        // reaches the map under it. The press leaves the box focused.)
        row.addEventListener('mousedown', (e) => e.preventDefault());
        row.addEventListener('click', (e) => {
          if (e.button === 0) this.go(i);
        });
        row.addEventListener('pointermove', () => this.select(i));
        return row;
      }),
    );
    this.status.textContent = this.hits.length ? '' : none;
    this.show(this.focused && (this.hits.length > 0 || none !== ''));
  }

  private select(i: number) {
    this.pick = i;
    this.opts.querySelectorAll('.hit').forEach((el, k) => {
      el.classList.toggle('on', k === i);
      el.setAttribute('aria-selected', k === i ? 'true' : 'false');
    });
    this.input.setAttribute('aria-activedescendant', `search-hit-${i}`);
    (this.opts.children[i] as HTMLElement | undefined)?.scrollIntoView({ block: 'nearest' });
  }

  private show(on: boolean) {
    this.list.hidden = !on;
    this.input.setAttribute('aria-expanded', on ? 'true' : 'false');
    if (on && this.pick >= 0) this.input.setAttribute('aria-activedescendant', `search-hit-${this.pick}`);
    else this.input.removeAttribute('aria-activedescendant');
  }

  private go(i: number) {
    const p = this.hits[i];
    if (!p) return;
    this.stop();
    this.input.value = p.main;
    this.root.classList.add('typed');
    this.show(false);
    this.input.blur();
    this.onGo(p);
  }

  /** No words: no list, no places kept, and what was marked goes (the mark goes with the words). */
  private emptied() {
    this.stop();
    this.state('');
    this.onClear();
  }

  private clear(keepFocus: boolean) {
    this.input.value = '';
    this.root.classList.remove('typed');
    this.emptied();
    if (keepFocus) this.input.focus();
  }
}
