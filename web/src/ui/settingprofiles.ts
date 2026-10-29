// Settings profiles (task #819): named sets of every sidebar setting, shared by every user through
// the server (crates/server/src/profiles.rs). A dropdown at the top of the sidebar picks the profile
// in use; there is always one, and a change to any setting shows "modified" by it (the settings the
// link then carries: state.ts overrides). In the open menu, hovering a profile's name shows it on the
// map until nothing is hovered; its buttons, off the menu's right edge: rename, replace with the
// current settings, make the default, delete (not the one in use or the default). Replace, default
// and delete offer Undo in a snackbar at the map's bottom left. The last item saves the current
// settings as a new profile.
import * as prefs from '../prefs';
import { fromProfile, overrides, profileSettings, type AppState, type LinkBase, type Store } from '../state';
import { h, toast } from './dom';

export interface ProfileItem {
  id: string;
  name: string;
  settings: Record<string, unknown>;
}

/** The server's answer (GET and every change). */
export interface Listing {
  default: string;
  profiles: ProfileItem[];
  /** False while the NAS is away: the list is this Mac's copy and can't be changed. */
  writable: boolean;
  /** The profile a change made or changed. */
  id?: string;
}

/** The list before the server has answered, or when it can't: the app's defaults alone. */
export const FALLBACK: Listing = { default: 'base', profiles: [{ id: 'base', name: 'Default', settings: {} }], writable: false };

export async function fetchProfiles(): Promise<Listing> {
  try {
    const r = await fetch('/api/profiles', { cache: 'no-store' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const l = (await r.json()) as Listing;
    return l.profiles?.length ? l : FALLBACK;
  } catch (e) {
    console.warn('profiles', e);
    return FALLBACK;
  }
}

async function call(method: string, path: string, body?: unknown): Promise<Listing> {
  const r = await fetch(`/api/profiles${path}`, {
    method,
    headers: body === undefined ? undefined : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
  return (await r.json()) as Listing;
}

const svg = (d: string) => {
  const el = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  el.setAttribute('viewBox', '0 0 16 16');
  el.innerHTML = d;
  return el;
};
const ICON = {
  rename: '<path d="M3 13h2.2l7-7L10 3.8l-7 7V13zM11 2.8l2.2 2.2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>',
  replace: '<path d="M8 2.5v7M5 6.8 8 9.8l3-3M3 11v2.5h10V11" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>',
  star: '<path d="M8 2.2l1.8 3.7 4 .6-2.9 2.8.7 4L8 11.4l-3.6 1.9.7-4L2.2 6.5l4-.6z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>',
  starOn: '<path d="M8 2.2l1.8 3.7 4 .6-2.9 2.8.7 4L8 11.4l-3.6 1.9.7-4L2.2 6.5l4-.6z" fill="currentColor"/>',
  trash: '<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>',
};

type Edit = { kind: 'add' } | { kind: 'rename'; id: string };

export class Profiles {
  /** The dropdown (the sidebar's top, below its heading). */
  readonly el: HTMLDivElement;
  /** True while a hovered profile is shown on the map (links and the saved state take userState). */
  private previewing = false;
  /** Asked for the link once the profile in use or its settings changed. */
  onChange: () => void = () => {};

  private list: ProfileItem[];
  private def: string;
  private writable: boolean;
  private active: string;
  private states = new Map<string, AppState>();
  private btn: HTMLButtonElement;
  private btnName: HTMLSpanElement;
  private btnMod: HTMLSpanElement;
  private menu: HTMLDivElement;
  private edit: Edit | null = null;
  /** The state before the preview, put back when nothing is hovered. */
  private before: AppState | null = null;
  private revertTimer = 0;
  private snack: HTMLDivElement;
  private snackTimer = 0;

  constructor(private store: Store, l: Listing, active: string, private buildings: () => boolean) {
    this.list = l.profiles;
    this.def = l.default;
    this.writable = l.writable;
    this.active = this.list.some((p) => p.id === active) ? active : this.def;
    this.btnName = h('span', { class: 'pf-name' });
    this.btnMod = h('span', { class: 'pf-mod' }, 'modified');
    this.btn = h('button', { class: 'pf-btn', type: 'button', title: 'Settings profile: every setting of this panel, shared by everyone using the map', 'aria-haspopup': 'listbox' },
      this.btnName, this.btnMod, h('i', { class: 'pf-caret' }));
    this.btn.setAttribute('aria-expanded', 'false');
    // (Opened from the keyboard, detail 0: the focus goes to the profile in use.)
    this.btn.addEventListener('click', (e) => (this.menu.hidden ? this.open(e.detail === 0) : this.close()));
    this.menu = h('div', { class: 'pf-menu', role: 'listbox' });
    this.menu.hidden = true;
    this.menu.addEventListener('mouseleave', () => this.scheduleRevert());
    document.body.append(this.menu);
    this.snack = h('div', { class: 'pf-snack', role: 'status' });
    this.snack.hidden = true;
    document.body.append(this.snack);
    this.el = h('div', { class: 'pf' }, this.btn);
    document.addEventListener('pointerdown', (e) => {
      if (!this.menu.hidden && !this.menu.contains(e.target as Node) && !this.btn.contains(e.target as Node)) this.close();
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !this.menu.hidden) {
        if (this.edit) {
          this.edit = null;
          this.render();
        } else {
          this.close();
          this.btn.focus();
        }
      }
    });
    // Up and down move through the menu's profiles (and its last item).
    this.menu.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
      const items = [...this.menu.querySelectorAll<HTMLButtonElement>('.pf-main:not(:disabled)')];
      if (!items.length) return;
      e.preventDefault();
      const i = items.indexOf(document.activeElement as HTMLButtonElement);
      items[(i + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length].focus();
    });
    window.addEventListener('resize', () => this.place());
    let queued = false;
    store.on(() => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        this.renderButton();
      });
    });
    this.renderButton();
  }

  /** The profile in use, as links name it. */
  base(): LinkBase {
    const p = this.item(this.active)!;
    return { id: p.id, name: p.name, s: this.state(p.id) };
  }

  /** The profile `id`'s state, if there is one. */
  has(id: string | null): id is string {
    return !!id && this.list.some((p) => p.id === id);
  }

  /** The settings of profile `id` (fromProfile; cached). */
  state(id: string): AppState {
    let s = this.states.get(id);
    if (!s) {
      s = fromProfile(this.item(id)?.settings ?? {});
      this.states.set(id, s);
    }
    return s;
  }

  /** Makes `id` the profile in use, without changing the settings (a link's or the saved ones). */
  use(id: string) {
    this.active = this.has(id) ? id : this.def;
    prefs.save('profile', this.active);
    this.renderButton();
  }

  /** The user's state: while a profile is previewed, the one before it (with the map's camera and
   * selection), so a link, the saved state or a new profile never take the preview's. */
  userState(): AppState {
    const b = this.before, s = this.store.s;
    return b ? { ...b, view: s.view, selected: s.selected, stretch: s.stretch } : s;
  }

  /** Whether the settings differ from the profile's (what a link would carry). */
  modified(): boolean {
    return overrides(this.store.s, this.state(this.active), this.buildings()).size > 0;
  }

  private item(id: string) {
    return this.list.find((p) => p.id === id);
  }

  private take(l: Listing) {
    this.list = l.profiles;
    this.def = l.default;
    this.writable = l.writable;
    this.states.clear();
    if (!this.has(this.active)) this.active = this.def;
    this.renderButton();
    if (!this.menu.hidden) this.render();
    this.onChange();
  }

  /** The settings of `s` laid over the map's (the camera and the selection kept). */
  private patch(s: AppState): Partial<AppState> {
    const { view: _v, selected: _s, stretch: _t, ...rest } = s;
    return this.buildings() ? rest : { ...rest, buildings: this.store.s.buildings };
  }

  /** Picks profile `id`: its settings, all of them. */
  pick(id: string) {
    this.endPreview(false);
    this.active = id;
    prefs.save('profile', id);
    this.store.set(this.patch(this.state(id)));
    this.close();
    this.onChange();
  }

  /** Back to the app's defaults (Reset), the default profile in use. */
  reset() {
    this.active = this.def;
    prefs.save('profile', this.def);
  }

  private preview(id: string) {
    clearTimeout(this.revertTimer);
    if (!this.before) this.before = this.store.s;
    this.previewing = true;
    this.store.set(this.patch(this.state(id)));
    this.menu.querySelectorAll('.pf-item').forEach((el) => el.classList.toggle('pv', (el as HTMLElement).dataset.id === id));
  }

  /** Puts the state before the preview back soon (unless another profile is hovered first). */
  private scheduleRevert() {
    clearTimeout(this.revertTimer);
    this.revertTimer = window.setTimeout(() => this.endPreview(true), 60);
  }

  private endPreview(restore: boolean) {
    clearTimeout(this.revertTimer);
    const b = this.before;
    this.before = null;
    this.menu.querySelectorAll('.pf-item.pv').forEach((el) => el.classList.remove('pv'));
    if (b && restore) this.store.set(this.patch(b));
    this.previewing = false;
  }

  private open(focus = false) {
    this.edit = null;
    this.menu.hidden = false;
    this.btn.classList.add('open');
    this.btn.setAttribute('aria-expanded', 'true');
    this.render();
    if (focus) this.menu.querySelector<HTMLElement>('.pf-item.on .pf-main')?.focus();
    // Others' changes since the page loaded.
    fetchProfiles().then((l) => {
      if (l !== FALLBACK && !this.menu.hidden && !this.edit) this.take(l);
    });
  }

  private close() {
    this.endPreview(true);
    this.edit = null;
    this.menu.hidden = true;
    this.btn.classList.remove('open');
    this.btn.setAttribute('aria-expanded', 'false');
  }

  /** The menu under the button (fixed: its buttons reach past the panel's edge). */
  private place() {
    if (this.menu.hidden) return;
    const r = this.btn.getBoundingClientRect();
    Object.assign(this.menu.style, { left: `${r.left}px`, top: `${r.bottom + 3}px`, width: `${r.width}px` });
  }

  /** The button's name and "modified" (and the menu's, for the profile in use). */
  private renderButton() {
    const p = this.item(this.active);
    this.btnName.textContent = p?.name ?? 'Default';
    // (While previewing, the settings shown aren't the user's: "modified" stays as it was.)
    if (this.previewing) return;
    const m = this.modified();
    this.btnMod.hidden = !m;
    this.menu.querySelectorAll<HTMLElement>('.pf-item.on .pf-mod').forEach((el) => (el.hidden = !m));
  }

  private async run(what: Promise<Listing>, done?: (l: Listing) => void) {
    try {
      const l = await what;
      this.take(l);
      done?.(l);
    } catch (e) {
      toast(`Profiles: ${(e as Error).message}`);
    }
  }

  /** A snackbar at the map's bottom left, with Undo, gone by itself. */
  private notice(msg: string, undo: () => void) {
    clearTimeout(this.snackTimer);
    const u = h('button', { type: 'button', class: 'pf-undo' }, 'Undo');
    u.addEventListener('click', () => {
      this.snack.hidden = true;
      undo();
    });
    this.snack.replaceChildren(h('span', {}, msg), u);
    this.snack.hidden = false;
    this.snackTimer = window.setTimeout(() => (this.snack.hidden = true), 6000);
  }

  private replace(p: ProfileItem) {
    const old = p.settings;
    this.run(call('PUT', `/${p.id}`, { settings: profileSettings(this.userState()) }), () =>
      this.notice(`“${p.name}” now has the current settings`, () => this.run(call('PUT', `/${p.id}`, { settings: old }))));
  }

  private makeDefault(p: ProfileItem) {
    const old = this.def;
    this.run(call('POST', `/${p.id}/default`), () =>
      this.notice(`“${p.name}” is the default`, () => this.run(call('POST', `/${old}/default`))));
  }

  private remove(p: ProfileItem) {
    this.run(call('DELETE', `/${p.id}`), () =>
      this.notice(`“${p.name}” deleted`, () => this.run(call('POST', '', { id: p.id, name: p.name, settings: p.settings }))));
  }

  /** A name box (add, rename): Return or the button saves, Esc cancels. */
  private nameBox(value: string, label: string, save: (name: string) => void) {
    const input = h('input', { type: 'text', class: 'pf-input', value, maxlength: 60, placeholder: 'Profile name', spellcheck: 'false' });
    const go = () => {
      const v = input.value.trim();
      if (v) save(v);
    };
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') go();
    });
    const b = h('button', { type: 'button', class: 'pf-save' }, label);
    b.addEventListener('click', go);
    queueMicrotask(() => {
      input.focus();
      input.select();
    });
    return h('div', { class: 'pf-edit' }, input, b);
  }

  private render() {
    const off = !this.writable;
    const why = off ? 'The NAS is away: profiles can be changed once it is back' : '';
    const rows = this.list.map((p) => {
      const isActive = p.id === this.active, isDef = p.id === this.def;
      if (this.edit?.kind === 'rename' && this.edit.id === p.id) {
        return h('div', { class: 'pf-item editing' }, this.nameBox(p.name, 'Rename', (name) => {
          this.edit = null;
          this.run(call('PUT', `/${p.id}`, { name }));
        }));
      }
      const name = h('button', { type: 'button', class: 'pf-main', role: 'option', 'aria-selected': isActive ? 'true' : 'false' },
        h('span', { class: 'pf-check' }, isActive ? '✓' : ''),
        h('span', { class: 'pf-label' }, p.name),
        isActive ? h('span', { class: 'pf-mod', hidden: this.btnMod.hidden }, 'modified') : null,
        isDef ? h('span', { class: 'pf-def', title: 'The default profile' }, 'default') : null);
      name.addEventListener('mouseenter', () => this.preview(p.id));
      name.addEventListener('mouseleave', () => this.scheduleRevert());
      name.addEventListener('click', () => this.pick(p.id));
      const act = (icon: string, title: string, f: () => void, disabled = false) => {
        const b = h('button', { type: 'button', class: 'pf-act', title: disabled ? title : off ? why : title, 'aria-label': title }, svg(icon));
        b.disabled = disabled || off;
        b.addEventListener('click', (e) => {
          e.stopPropagation();
          f();
        });
        return b;
      };
      const acts = h('div', { class: 'pf-acts' },
        act(ICON.rename, 'Rename', () => {
          this.endPreview(true);
          this.edit = { kind: 'rename', id: p.id };
          this.render();
        }),
        act(ICON.replace, 'Replace with the current settings', () => this.replace(p)),
        act(isDef ? ICON.starOn : ICON.star, isDef ? 'The default profile' : 'Make the default', () => this.makeDefault(p), isDef),
        act(ICON.trash, isActive ? 'The profile in use can’t be deleted' : isDef ? 'The default profile can’t be deleted' : 'Delete', () => this.remove(p), isActive || isDef));
      const row = h('div', { class: `pf-item${isActive ? ' on' : ''}` }, name, acts);
      row.dataset.id = p.id;
      return row;
    });
    const add = this.edit?.kind === 'add'
      ? h('div', { class: 'pf-item pf-new editing' }, this.nameBox('', 'Save', (name) => {
          this.edit = null;
          this.run(call('POST', '', { name, settings: profileSettings(this.userState()) }), (l) => {
            if (l.id) this.use(l.id);
            this.onChange();
          });
        }))
      : (() => {
          const b = h('button', { type: 'button', class: 'pf-main pf-addbtn', title: off ? why : 'A new profile with every setting as it is now' }, h('span', { class: 'pf-check' }, '+'), h('span', { class: 'pf-label' }, 'Save current settings as a new profile…'));
          b.disabled = off;
          b.addEventListener('click', () => {
            this.endPreview(true);
            this.edit = { kind: 'add' };
            this.render();
          });
          return h('div', { class: 'pf-item pf-new' }, b);
        })();
    this.menu.replaceChildren(...rows, add);
    this.place();
  }
}
