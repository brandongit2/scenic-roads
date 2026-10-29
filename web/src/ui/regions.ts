// The Regions panel (Layers → Regions; docs/surface.md): the regions the map is built for, read
// only. Regions are files in their inputs folder, inputs/regions/ (docs/inputs.md §6.1), changed there
// alone and checked before they're used: the list shows their folders (by their _group.toml names)
// sorted among them, collapsed until opened, each entry with a flag, its land and water, and its
// check's state (and the findings holding it); a search and a "downloaded only" filter atop the
// section (regionlist.ts). The coverage they add up to is drawn on the map: the one the map's
// catalogue was built for.
// Downloads (docs/macs.md, "Mirror, per Mac"): nothing is copied to this Mac unless it's
// downloaded here: the World, zoomed out, each region (a folder's button: all of its regions), and a
// downloaded region's terrain detail, each with its size, its state and a Download, Cancel or
// Remove button; this Mac's room (downloads.ts).
import { behindText, dlDetail, dlRegion, dlStatus, dlUpdate, dlWorld, pct, setReserve, size, sizeOf, stateClass, stateText, type Dl, type DlStatus, type DetailDl, type RegionDl } from '../downloads';
import { FLAGS } from '../flags';
import * as prefs from '../prefs';
import { allIds, areaText, entries, filter, flagCode, folderFlag, type Shown } from '../regionlist';
import {
  RegionLayers, RegionsError, areaOutline, getCoverage, getRegionsLayer, listRegions,
  type Area, type Coverage, type Folder, type Region, type RegionsLayer,
} from '../regions';
import { fmt, h } from './dom';

/** Opened again within this long of the last load: the list as it was. */
const FRESH_MS = 10_000;

type Bbox = [number, number, number, number];
type Outline = GeoJSON.Feature<GeoJSON.MultiPolygon>;

/** An error in plain words. */
function says(e: unknown): string {
  if (!(e instanceof RegionsError)) return 'Something went wrong';
  if (e.status === 404 && /^no outlines yet/i.test(e.message)) return 'No area outlines yet: the OpenStreetMap pass makes them';
  return e.message;
}

/** The box around some outlines. */
function bboxOf(fs: Outline[]): Bbox | null {
  let w = Infinity, s = Infinity, e = -Infinity, n = -Infinity;
  for (const f of fs) {
    for (const poly of f.geometry.coordinates) {
      for (const [x, y] of poly[0] ?? []) {
        w = Math.min(w, x);
        s = Math.min(s, y);
        e = Math.max(e, x);
        n = Math.max(n, y);
      }
    }
  }
  return w <= e ? [w, s, e, n] : null;
}

export class RegionsPanel {
  /** The section's content. */
  readonly nodes: Node[];
  /** Set by the app: the view framed on a box [w, s, e, n]. */
  onFit: (bbox: Bbox) => void = () => {};

  private covBox = h('input', { type: 'checkbox' });
  private covCount = h('span', { class: 'km' });
  private regBox = h('input', { type: 'checkbox' });
  private regNote = h('span', { class: 'km' });
  private layer: RegionsLayer | null = null;
  private list = h('div', { class: 'rg-list' });
  private note = h('div', { class: 'rg-note' });
  // Downloads: this Mac's room, the World.
  private dlSum = h('div', { class: 'kp-sum' });
  private worldRow = h('div', { class: 'rg dl-world' });
  private worldBtn: HTMLButtonElement | null = null;
  private worldOn: boolean | null = null;
  /** What's going on: the copy under way, waiting for room, away. */
  private dlNow = h('div', { class: 'kp-sum' });
  /** The reserve: the free space downloads leave on this Mac's disk (the server's setting). */
  private resIn = h('input', { class: 'rg-in kp-res-in', type: 'number', min: '5', step: '1', inputmode: 'numeric', 'aria-label': 'Free space to keep, GB' }) as HTMLInputElement;
  private resBtn = h('button', { class: 'pill', type: 'button', title: 'Keep this much free' }, 'Set') as HTMLButtonElement;
  private resSay = h('span', { class: 'kp-res-say', role: 'status', 'aria-live': 'polite' });
  private resRow = h('div', { class: 'kp-line kp-res', title: 'The free space map downloads leave on this Mac’s disk: a download that would go past it waits. Lowered, waiting downloads may start; raised, nothing is deleted.' },
    h('span', {}, 'Keep free'), this.resIn, h('span', {}, 'GB'), this.resBtn, this.resSay);
  private resBusy = false;
  /** The server refused the last save: its message stays until the field is edited again. */
  private resRefused = false;
  private dlMsg = h('div', { class: 'rg-msg' });
  /** The downloads' state, as last asked (null: not yet), when (ms), or why it couldn't be. */
  private dl: DlStatus | null = null;
  private dlAt = 0;
  private dlErr = '';
  private dlTok = 0;
  private dlTimer = 0;
  /** What each region row shows of its download, updated in place as the state comes (a row isn't
   * drawn again: a hover or a rename in it stays). */
  private dlEls = new Map<string, { row: HTMLElement; slot: HTMLElement; btn: HTMLButtonElement | null; mode: Mode | null; size: HTMLElement; prog: HTMLElement; detail: HTMLElement; detailSize: HTMLElement; detailBtn: HTMLButtonElement | null; detailOn: boolean; detailProg: HTMLElement }>();
  /** The same for each folder shown (by path): its regions' download as one. */
  private dirEls = new Map<string, { f: Folder; row: HTMLElement; slot: HTMLElement; btn: HTMLButtonElement | null; mode: Mode | null; size: HTMLElement; prog: HTMLElement }>();
  /** Downloads asked for or cancelled here and not yet in the state the server last told (a region's
   * id: downloaded or not): its button and loading state change at the click, not the answer. */
  private want = new Map<string, boolean>();
  /** The search atop the section, and the downloaded-only filter. */
  private q = h('input', { class: 'rg-in rg-q', type: 'search', placeholder: 'Search regions', 'aria-label': 'Search regions', autocomplete: 'off', spellcheck: 'false' }) as HTMLInputElement;
  private onlyBox = h('input', { type: 'checkbox' }) as HTMLInputElement;
  private bar = h('div', { class: 'rg-top' });
  /** The downloaded regions the list was last filtered by (downloaded-only). */
  private onKey = '';
  /** Under the World's row: its progress, and its Update while it's behind. */
  private worldProg = h('div', { class: 'rg-prog', hidden: true });
  /** The downloads whose Update was asked and not answered yet ("world", a region's id, "<id>/detail"). */
  private updating = new Set<string>();

  private regions: Region[] = [];
  private byId = new Map<string, Region>();
  /** The folders the list is grouped by (null: a server from before folders). */
  private tree: Folder | null = null;
  private bad: [string, string][] = [];
  private coverage: Coverage | null = null;
  /** The outlines of the regions the catalogue doesn't have yet, from their osm: entries' areas. */
  private pendingOutlines = new Map<string, Outline[]>();
  /** The catalogue the map shows, as last told (setCatalogue). */
  private catalogueN: number | undefined;
  private loadedAt = -Infinity;
  private loadTok = 0;
  private open = false;
  /** The region whose row is hovered (its outline shown). */
  private hoverRegion: string | null = null;

  constructor(private layers: RegionLayers) {
    this.covBox.checked = prefs.load('regions.coverage', false);
    this.covBox.addEventListener('change', () => {
      prefs.save('regions.coverage', this.covBox.checked);
      this.layers.setCoverage(this.covBox.checked, this.coverage ?? undefined);
      if (this.covBox.checked && !this.coverage) void this.refresh();
    });
    this.regBox.checked = prefs.load('regions.layer', false);
    this.regBox.addEventListener('change', () => {
      prefs.save('regions.layer', this.regBox.checked);
      this.layers.setRegions(this.regBox.checked, this.layer ?? undefined);
      if (this.regBox.checked && !this.layer) void this.refresh();
    });
    this.resBtn.hidden = true;
    this.resRow.hidden = true;
    this.resIn.addEventListener('input', () => {
      this.resRefused = false;
      this.resEdited();
    });
    this.resIn.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') void this.saveReserve();
      if (e.key === 'Escape') {
        this.resIn.value = String(Math.round((this.dl?.reserve_set ?? 0) / 1e9));
        this.resRefused = false;
        this.resEdited();
      }
    });
    this.resBtn.addEventListener('click', () => void this.saveReserve());
    this.onlyBox.checked = prefs.load('regions.downloadedOnly', false);
    this.onlyBox.addEventListener('change', () => {
      prefs.save('regions.downloadedOnly', this.onlyBox.checked);
      this.renderList();
    });
    let typed = 0;
    this.q.addEventListener('input', () => {
      clearTimeout(typed);
      typed = window.setTimeout(() => this.renderList(), 120);
    });
    this.q.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && this.q.value) {
        this.q.value = '';
        this.renderList();
      }
    });
    this.bar.append(this.q, h('label', { class: 'tog', title: 'Only the regions downloaded to this Mac (and those being downloaded)' }, this.onlyBox, h('span', {}, 'Show only downloaded regions')));
    this.nodes = [
      this.bar,
      h('label', { class: 'tog', title: 'Every region’s outline on the map: the map is built within them' }, this.covBox, h('span', {}, 'Coverage on the map'), this.covCount),
      h('label', { class: 'tog', title: 'Each region’s accepted outline in its own colour, with its name: overlaps and gaps show; dashed while a finding holds it' }, this.regBox, h('span', {}, 'Regions on the map'), this.regNote),
      h('div', { class: 'subhd', title: 'What’s downloaded to this Mac, for when it’s away from the NAS: nothing else is copied here' }, 'Downloads on this Mac'),
      this.dlSum,
      this.resRow,
      this.worldRow,
      this.dlNow,
      this.dlMsg,
      h('div', { class: 'subhd', title: 'Each region’s land and water and its download’s size, and Download, Cancel or Remove; a folder’s button does all of its regions' }, 'Regions'),
      this.list,
      this.note,
    ];
    this.renderList();
  }

  /** The map is ready: the coverage drawn if it's on. */
  start() {
    if (this.covBox.checked || this.regBox.checked) void this.refresh();
  }

  /** The map's catalogue now (catalogue.ts). A new one was built for its own coverage: drawn again
   * while it's shown or the panel is open. */
  setCatalogue(n: number | undefined) {
    if (n === undefined || n === this.catalogueN) return;
    this.catalogueN = n;
    const had = this.coverage?.catalog;
    if (had !== undefined && had !== n && (this.covBox.checked || this.open)) void this.refresh();
  }

  /** The section opened (the list again, unless just loaded) or closed (its previews leave the
   * map). */
  setOpen(open: boolean) {
    this.open = open;
    if (open) {
      this.measure();
      if (performance.now() - this.loadedAt > FRESH_MS) void this.refresh();
      void this.pollDl();
    } else {
      this.layers.setHover(null);
      clearTimeout(this.dlTimer);
      this.dlTok++;
    }
  }

  // ---- the regions ------------------------------------------------------------------

  /** The regions and the coverage again. */
  private async refresh() {
    const tok = ++this.loadTok;
    if (!this.regions.length) this.note.replaceChildren(h('span', { class: 'spin' }), ' Loading the regions…');
    // (The regions layer only while it's shown: it's every region's full outline.)
    const [list, cov, layer] = await Promise.allSettled([listRegions(), getCoverage(), this.regBox.checked ? getRegionsLayer() : Promise.resolve(null)]);
    if (tok !== this.loadTok) return;
    this.loadedAt = performance.now();
    if (layer.status === 'fulfilled' && layer.value) {
      this.layer = layer.value;
      this.layers.setRegions(this.regBox.checked, layer.value);
      this.regNote.textContent = layer.value.source === 'catalog' ? 'as built' : '';
      this.regNote.title = layer.value.source === 'catalog' ? 'This Mac doesn’t have the accepted outlines yet: these are the ones the map was built for' : '';
    } else if (layer.status === 'rejected') {
      this.regNote.textContent = 'unavailable';
      this.regNote.title = says(layer.reason);
    }
    if (cov.status === 'fulfilled') {
      this.coverage = cov.value;
      this.layers.setCoverage(this.covBox.checked, cov.value);
    }
    if (list.status === 'fulfilled') {
      this.regions = list.value.regions;
      this.tree = list.value.tree ?? null;
      this.bad = list.value.bad;
      this.note.replaceChildren();
      this.note.classList.remove('warn');
      // Away from home: the list as last read.
      if (list.value.offline) this.note.textContent = 'Away from the NAS: the list as last read';
    } else {
      this.note.textContent = says(list.reason);
      this.note.classList.add('warn');
    }
    this.renderList();
    this.loadPending(tok);
  }

  /** Not on the map yet: the catalogue has no region by its id with its outline (added or redrawn
   * since it was made). Never when the catalogue doesn't say which regions it was built for. */
  private isPending(r: Region): boolean {
    const built = this.coverage?.regions;
    if (!built) return false;
    const c = built.find((b) => b.id === r.id);
    return !c || c.outline.length !== r.outline.length || c.outline.some((e, i) => e !== r.outline[i]);
  }

  /** A region's outlines: the catalogue's (each entry's; none until the coverage is loaded), or for a
   * pending one its osm: entries' areas, once loaded. */
  private outlinesOf(r: Region): Outline[] {
    if (this.isPending(r)) return this.pendingOutlines.get(r.id) ?? [];
    return (this.coverage?.features ?? []).filter((f) => f.properties?.region === r.id);
  }

  /** The pending regions' outlines, from their osm: entries' areas (other entries have none to
   * show until built), for their flags and showing them on the map; then the list again. */
  private loadPending(tok: number) {
    const pending = this.regions.filter((r) => this.isPending(r));
    void Promise.all(pending.map(async (r) => {
      const fs = await Promise.all(r.outline.filter((e) => e.startsWith('osm:')).map((e) =>
        areaOutline(Number(e.slice(4))).then((f): Outline => ({ ...f, properties: { ...f.properties, region: r.id, entry: e } }), () => null)));
      return [r.id, fs.filter((f): f is Outline => f !== null)] as const;
    })).then((got) => {
      if (tok !== this.loadTok) return;
      this.pendingOutlines = new Map(got);
      if (got.length) this.renderList();
    });
  }

  /** The heights the sticky search bar and folder headings sit under: the section's header (under
   * the panel's top, --top-h) and the bar itself. */
  private measured = false;
  private measure() {
    if (this.measured) return;
    const grp = this.bar.parentElement, sec = this.bar.closest('.section')?.querySelector<HTMLElement>(':scope > .sec');
    if (!grp || !sec) return;
    this.measured = true;
    new ResizeObserver(() => {
      grp.style.setProperty('--rg-sec-h', `${sec.offsetHeight}px`);
      grp.style.setProperty('--rg-bar-h', `${this.bar.offsetHeight}px`);
    }).observe(this.bar);
  }

  /** The region downloaded here, or being: as asked here last, else as the server says. */
  private isOn(id: string): boolean {
    return this.want.get(id) ?? !!this.dl?.regions[id]?.on;
  }

  private renderList() {
    // (A hovered row goes without a mouseleave.)
    if (this.hoverRegion) {
      this.hoverRegion = null;
      this.layers.setHover(null);
    }
    this.dlEls.clear();
    this.dirEls.clear();
    const n = this.regions.length;
    this.covCount.textContent = n ? `${n} region${n === 1 ? '' : 's'}` : '';
    const byId = (this.byId = new Map(this.regions.map((r) => [r.id, r])));
    // The tree; regions it doesn't hold (not in the accepted version yet) at the top level.
    const tree = this.tree ?? { path: '', name: '', regions: [], folders: [] };
    const placed = new Set(allIds(tree));
    const root: Folder = { ...tree, regions: [...tree.regions, ...this.regions.filter((r) => !placed.has(r.id)).map((r) => r.id)] };
    const only = this.onlyBox.checked;
    if (only) this.onKey = this.downloadedKey();
    const shown = filter(root, byId, this.q.value, (id) => !only || this.isOn(id));
    const searching = !!this.q.value.trim();
    const list = (f: Folder, depth: number): HTMLElement[] => entries(f, byId).flatMap((e) => {
      if (e.kind === 'region') return shown.regions.has(e.r.id) ? [this.regionRow(e.r)] : [];
      return shown.folders.has(e.f.path) ? [this.folderBox(e.f, depth, shown, searching, list)] : [];
    });
    this.list.replaceChildren(
      ...list(root, 0),
      ...this.bad.map(([file, e]) => h('div', { class: 'rg-bad', title: e }, h('b', {}, file), ` doesn’t parse: ${e}`)),
    );
    if (!n && !this.bad.length && this.loadedAt > -Infinity) this.list.append(h('div', { class: 'faint rg-empty' }, 'No regions yet'));
    else if (n && !shown.regions.size) this.list.append(h('div', { class: 'faint rg-empty' }, only && !searching ? 'No regions downloaded on this Mac' : 'No region matches'));
    this.renderDirs();
  }

  /** The downloaded regions, as a key (the downloaded-only list is drawn again when it changes). */
  private downloadedKey(): string {
    return this.regions.filter((r) => this.isOn(r.id)).map((r) => r.id).join(',');
  }

  /** A folder: its heading (sticky under the bar and the folders it's in, `depth` of them) like a
   * region's row, with its flag, its regions' land and water and size, and its button; then its
   * entries, indented along a line under its triangle. Collapsed until opened (remembered), but
   * open while a search finds something inside it. */
  private folderBox(f: Folder, depth: number, shown: Shown, searching: boolean, list: (f: Folder, depth: number) => HTMLElement[]): HTMLElement {
    const key = `regions.folder.${f.path}`;
    const forced = searching && shown.open.has(f.path);
    const box = h('details', { class: 'rg-dir', open: forced || prefs.load(key, false) }) as HTMLDetailsElement;
    box.addEventListener('toggle', () => {
      if (!forced) prefs.save(key, box.open);
    });
    const regs = allIds(f).flatMap((id) => this.byId.get(id) ?? []);
    const sum = (k: 'land_km2' | 'water_km2') => (regs.some((r) => r[k] !== undefined) ? regs.reduce((t, r) => t + (r[k] ?? 0), 0) : undefined);
    const name = h('span', { class: 'rg-name', title: `${f.name}: show it` }, f.name);
    name.addEventListener('click', (e) => {
      e.preventDefault();
      const b = bboxOf(regs.flatMap((r) => this.outlinesOf(r)));
      if (b) this.onFit(b);
    });
    const slot = h('span', { class: 'rg-dlslot' });
    const sz = h('span', { class: 'rg-size' });
    const prog = h('div', { class: 'rg-prog', hidden: true });
    const code = folderFlag(f, (id) => this.flagOf(this.byId.get(id)), (id) => this.byId.get(id)?.land_km2 ?? 0);
    const head = h('summary', { class: 'rg', title: `inputs/regions/${f.path}/` },
      h('div', { class: 'rg1' }, h('i', { class: 'rg-tri' }), flag(code), name, slot),
      // (Some of its regions not sized yet: so far, "…" after it.)
      h('div', { class: 'rg2 rg-in2' }, h('span', { class: 'rg2s', title: 'Its regions’ land and water' }, `${areaText(sum('land_km2'), sum('water_km2'))}${regs.some((r) => r.land_km2 === undefined) && regs.some((r) => r.land_km2 !== undefined) ? ' …' : ''}`), sz),
      prog);
    head.style.setProperty('--rg-depth', String(depth));
    this.dirEls.set(f.path, { f, row: head, slot, btn: null, mode: null, size: sz, prog });
    box.append(head, h('div', { class: 'rg-kids' }, ...list(f, depth + 1)));
    return box;
  }

  /** A region's state in the input check: being checked, held by findings, or disabled. */
  private stateOf(r: Region): { text: string; cls: string; title?: string } | null {
    if (r.state === 'checking') return { text: 'checking…', cls: 'wait', title: 'Its file changed: being checked before it’s used' };
    if (r.state === 'held') {
      const fs = r.held ?? [];
      return { text: `held: ${fs[0]?.message ?? 'a finding holds it'}${fs.length > 1 ? ` (+${fs.length - 1})` : ''}`, cls: 'held', title: `Its last accepted outline stays in use until these are fixed:\n${fs.map((f) => `${f.id}: ${f.message}`).join('\n')}` };
    }
    if (r.disabled) return { text: 'disabled', cls: 'off', title: 'Disabled in its file (disabled = true): checked like every region, and other regions may name it, but nothing is built for it, so it isn’t on the map and can’t be downloaded. Its file enables it.' };
    return null;
  }

  /** A region's flag (a flag file's code), from its first osm: part's area, else its geometry's
   * country (regionlist.flagCode). */
  private flagOf(r: Region | undefined): string | null {
    if (!r) return null;
    const first = r.outline.find((e) => e.startsWith('osm:'));
    const a = first ? (this.outlinesOf(r).find((f) => f.properties?.entry === first)?.properties as Partial<Area> | undefined) : undefined;
    return flagCode({ iso: a?.iso, country: a?.country, in: a?.in, geo: r.country }, (c) => FLAGS.has(c));
  }

  private regionRow(r: Region): HTMLElement {
    const fs = this.outlinesOf(r);
    const row = h('div', { class: r.disabled ? 'rg off' : 'rg' });
    const state = this.stateOf(r);
    const name = h('span', { class: 'rg-name', title: `${r.name}: show it` }, r.name);
    name.addEventListener('click', () => {
      const b = bboxOf(fs);
      if (b) this.onFit(b);
    });
    const slot = h('span', { class: 'rg-dlslot' });
    const sz = h('span', { class: 'rg-size' });
    const detailSize = h('span', { class: 'rg-size rg-fsize' });
    const detail = h('div', { class: 'rg2 rg-in2', hidden: true }, h('span', { class: 'rg2s rg-fine', title: 'The terrain past z12 (z13–z16), where its data has the detail: an add-on to the region, which gives the rest of it and every zoom to z12' }, 'Terrain detail (z13+)'), detailSize);
    const prog = h('div', { class: 'rg-prog', hidden: true });
    const detailProg = h('div', { class: 'rg-prog', hidden: true });
    this.dlEls.set(r.id, { row, slot, btn: null, mode: null, size: sz, prog, detail, detailSize, detailBtn: null, detailOn: false, detailProg });
    row.replaceChildren(
      h('div', { class: 'rg1' }, h('i', { class: 'rg-tri none' }), flag(this.flagOf(r)), name, h('span', { class: 'rg-id faint', title: `Its file: inputs/regions/${r.path ?? `${r.id}.toml`}` }, r.id),
        ...(state ? [h('span', { class: `rg-state ${state.cls}`, title: state.title }, state.text)] : []),
        slot),
      h('div', { class: 'rg2 rg-in2' }, h('span', { class: 'rg2s', title: 'Its land and its water (the sea and the lakes over 100 km² it holds)' }, areaText(r.land_km2, r.water_km2)), sz),
      prog,
      detail,
      detailProg,
    );
    this.showDl(r.id);
    row.addEventListener('mouseenter', () => {
      this.hoverRegion = r.id;
      this.layers.setHover(fs);
    });
    row.addEventListener('mouseleave', () => {
      this.hoverRegion = null;
      this.layers.setHover(null);
    });
    return row;
  }

  // ---- downloads (on this Mac) ------------------------------------------------------

  /** The downloads' state again, then every 2 s while something is being copied or is coming,
   * else every 10 s, while the section is open. */
  private async pollDl() {
    clearTimeout(this.dlTimer);
    const tok = ++this.dlTok;
    try {
      this.dl = await dlStatus();
      this.dlAt = Date.now();
      this.dlErr = '';
    } catch (e) {
      if (tok !== this.dlTok) return;
      this.dlErr = says(e);
    }
    if (tok !== this.dlTok) return;
    this.renderDl();
    const k = this.dl, w = k?.wanted;
    const coming = !!k?.copying || (!!w && k!.online && w.more === 0 && (w.here < w.bytes || w.unknown > 0));
    if (this.open) this.dlTimer = window.setTimeout(() => void this.pollDl(), coming ? 2000 : 10_000);
  }

  /** A button that downloads, or removes after a second click (it says how much goes); what
   * happened said by `say` (the block's message line unless given: a region's row has its own). */
  private dlButton(on: boolean, what: string, bytes: () => number, act: (on: boolean) => Promise<unknown>, said: (on: boolean) => string, say: (text: string, warn?: boolean) => void = (t, w) => this.sayDl(t, w)): HTMLButtonElement {
    const b = h('button', { class: `rg-dl${on ? ' rm' : ''}`, type: 'button', title: on ? `Remove ${what} from this Mac` : `Download ${what} to this Mac, for when it’s away from the NAS` }, on ? 'Remove' : 'Download') as HTMLButtonElement;
    let armed = 0;
    b.addEventListener('click', () => {
      if (on && !armed) {
        b.textContent = `Remove ${size(bytes())}?`;
        b.classList.add('arm');
        armed = window.setTimeout(() => {
          armed = 0;
          b.textContent = 'Remove';
          b.classList.remove('arm');
        }, 4000);
        return;
      }
      clearTimeout(armed);
      b.disabled = true;
      b.textContent = on ? 'Removing…' : 'Asking…';
      act(!on).then(
        () => say(said(!on)),
        (e) => {
          say(says(e), true);
          b.disabled = false;
          b.textContent = on ? 'Remove' : 'Download';
          b.classList.remove('arm');
        },
      ).finally(() => void this.pollDl());
    });
    return b;
  }

  /** A region's size, and how much of it is here or its state when downloaded; its button:
   * Download, Cancel while it's coming (from the click on), Remove once it's here. */
  private showDl(id: string) {
    const els = this.dlEls.get(id);
    if (!els) return;
    const k = this.dl;
    const rk: RegionDl | undefined = k?.regions[id];
    const r = this.regions.find((x) => x.id === id);
    // (Asked here and now so on the server: the server's state from here on.)
    if (rk && this.want.get(id) === rk.on) this.want.delete(id);
    // (A disabled region is never built: no size, no Download.)
    if (r?.disabled) {
      els.size.textContent = '';
      els.btn?.remove();
      els.btn = null;
      return;
    }
    const on = this.isOn(id);
    this.showDetail(id, k?.mirror && on ? rk?.detail : undefined, r?.name ?? rk?.name ?? id, on);
    let text = '', cls = 'rg-size';
    if (k?.mirror && rk) {
      if (on && rk.on) {
        text = rk.state === 'missing' ? stateText(rk) : `${sizeOf(rk)} · ${stateText(rk)}`;
        cls += ` ${stateClass(rk.state)}`;
      } else text = on ? `${sizeOf(rk)} · starting` : sizeOf(rk);
    }
    els.size.className = cls;
    els.size.textContent = text;
    els.size.title = rk ? `Its packs and the basemap’s zooms 11–14 over it: ${size(rk.bytes)}, of which ${size(rk.here)} on this Mac` : '';
    const name = r?.name ?? rk?.name ?? id;
    const say = (t: string, w?: boolean) => this.sayRow(els.row, t, w);
    this.showProg(els.prog, k?.mirror && on ? rk : undefined, id, `“${name}”`, () => dlUpdate(id), say, on && !rk?.on);
    const mode: Mode | null = !k?.mirror || !rk ? null : modeOf(on, rk);
    if (els.mode !== mode || !els.btn) {
      els.btn?.remove();
      els.btn = mode ? this.dlControl(mode, `“${name}”`, () => (this.dl?.regions[id]?.bytes ?? 0) + (this.dl?.regions[id]?.detail?.on ? this.dl.regions[id].detail!.bytes : 0), [id], say) : null;
      if (els.btn) els.slot.append(els.btn);
      els.mode = mode;
    }
  }

  /** Each folder's heading: its regions' size, a bar while some are coming, and its button for all
   * of them. */
  private renderDirs() {
    const k = this.dl;
    for (const els of this.dirEls.values()) {
      const ids = allIds(els.f).filter((id) => k?.regions[id] && !this.regions.find((r) => r.id === id)?.disabled);
      const rks = ids.map((id) => k!.regions[id]);
      const ons = ids.filter((id) => this.isOn(id));
      els.size.textContent = k?.mirror && ids.length ? (ons.length ? `${ons.length} of ${ids.length} · ` : '') + size(rks.reduce((t, r) => t + r.bytes, 0)) : '';
      els.size.title = ons.length ? `${ons.length} of its ${ids.length} regions downloaded or coming` : '';
      const coming = ons.filter((id) => modeOf(true, k!.regions[id]) === 'cancel');
      const mode: Mode | null = !k?.mirror || !ids.length ? null : coming.length ? 'cancel' : ons.length === ids.length ? 'remove' : 'download';
      // (A bar while some are coming: of those the server has started.)
      const t = coming.map((id) => k!.regions[id]).filter((r) => r.on).reduce((a, r) => ({ bytes: a.bytes + r.bytes, here: a.here + r.here }), { bytes: 0, here: 0 });
      const bar = coming.length ? progBar(t.bytes ? t.here / t.bytes : null, t.bytes ? `${size(t.here)} of ${size(t.bytes)} here` : 'Starting') : null;
      els.prog.replaceChildren(...(bar ? [bar] : []));
      els.prog.hidden = !bar;
      if (els.mode !== mode || !els.btn) {
        els.btn?.remove();
        const what = `the ${ids.length} regions of “${els.f.name}”`;
        // (Download: those not yet; Cancel: those coming; Remove: all.)
        const which = () => {
          const m = els.mode;
          return allIds(els.f).filter((id) => this.dl?.regions[id] && (m === 'download' ? !this.isOn(id) : m === 'cancel' ? this.isOn(id) && modeOf(true, this.dl.regions[id]) === 'cancel' : this.isOn(id)));
        };
        els.btn = mode ? this.dlControl(mode, what, () => which().reduce((t, id) => t + (this.dl?.regions[id]?.bytes ?? 0), 0), which, (t, w) => this.sayRow(els.row, t, w)) : null;
        if (els.btn) {
          els.btn.addEventListener('click', (e) => e.preventDefault());
          els.slot.append(els.btn);
        }
        els.mode = mode;
      }
    }
  }

  /** A download's button: Download and Cancel act at the click (the rows show it before the server
   * answers); Remove asks again, saying how much goes. `ids`: the regions it acts on (a folder's: all
   * those its mode reaches, worked out at the click), asked one after another. */
  private dlControl(mode: Mode, what: string, bytes: () => number, ids: string[] | (() => string[]), say: (text: string, warn?: boolean) => void): HTMLButtonElement {
    const label = { download: 'Download', cancel: 'Cancel', remove: 'Remove' }[mode];
    const title = { download: `Download ${what} to this Mac, for when it’s away from the NAS`, cancel: `Stop downloading ${what}: what was copied of it goes`, remove: `Remove ${what} from this Mac` }[mode];
    const b = h('button', { class: `rg-dl ${mode === 'download' ? '' : mode === 'cancel' ? 'cx' : 'rm'}`, type: 'button', title }, label) as HTMLButtonElement;
    let armed = 0;
    b.addEventListener('click', () => {
      if (mode === 'remove' && !armed) {
        b.textContent = `Remove ${size(bytes())}?`;
        b.classList.add('arm');
        armed = window.setTimeout(() => {
          armed = 0;
          b.textContent = 'Remove';
          b.classList.remove('arm');
        }, 4000);
        return;
      }
      clearTimeout(armed);
      const on = mode === 'download';
      const list = typeof ids === 'function' ? ids() : ids;
      for (const id of list) this.want.set(id, on);
      // At once: every row it touches in its new state, the list again if it filters by it.
      this.redrawDl();
      say(mode === 'cancel' ? `Stopped downloading ${what}.` : '');
      void (async () => {
        for (const id of list) {
          try {
            await dlRegion(id, on);
          } catch (e) {
            for (const x of list.slice(list.indexOf(id))) this.want.delete(x);
            this.redrawDl();
            say(says(e), true);
            break;
          }
        }
        if (mode === 'remove') say(`${what.charAt(0).toUpperCase()}${what.slice(1)} ${list.length > 1 ? 'are' : 'is'} removed from this Mac.`);
        void this.pollDl();
      })();
    });
    return b;
  }

  /** The rows' downloads drawn again from what's known (and what was just asked here). */
  private redrawDl() {
    if (this.onlyBox.checked && this.downloadedKey() !== this.onKey) return this.renderList();
    for (const id of this.dlEls.keys()) this.showDl(id);
    this.renderDirs();
  }

  /** A region's terrain detail, where it has any: the share of it covered, its size and state, its
   * button: Download only once the region is downloaded (it needs it; removing the region removes
   * it). */
  private showDetail(id: string, f: DetailDl | undefined, name: string, regionOn: boolean) {
    const els = this.dlEls.get(id)!;
    els.detail.hidden = !f;
    this.showProg(els.detailProg, f, `${id}/detail`, `the terrain detail of “${name}”`, () => dlUpdate(id, true), (t, w) => this.sayRow(els.row, t, w));
    if (!f) return;
    const cover = `${f.cover < 1 && f.cover > 0 ? '<1' : fmt.n(f.cover)} % of it`;
    const needs = !f.on && !regionOn;
    els.detailSize.className = `rg-size rg-fsize${f.on ? ` ${stateClass(f.state)}` : ''}`;
    els.detailSize.textContent = f.on ? `${cover} · ${sizeOf(f)} · ${stateText(f)}` : needs ? `${sizeOf(f)} · needs the region` : `${cover} · ${sizeOf(f)}`;
    els.detailSize.title = `The deep packs over it: ${size(f.bytes)}, of which ${size(f.here)} on this Mac; they cover ${fmt.n(f.cover)} % of the region${needs ? '. Download the region first: they need it' : ''}`;
    if (els.detailOn !== f.on || !els.detailBtn) {
      const btn = this.dlButton(f.on, `the terrain detail of “${name}”`, () => this.dl?.regions[id]?.detail?.bytes ?? 0, (o) => dlDetail(id, o), (o) =>
        o ? `Downloading the terrain detail of “${name}”.` : `The terrain detail of “${name}” is removed from this Mac.`, (t, w) => this.sayRow(els.row, t, w));
      if (els.detailBtn) els.detailBtn.replaceWith(btn);
      else els.detail.append(btn);
      els.detailBtn = btn;
      els.detailOn = f.on;
    }
    // (A click in flight keeps its own state until the answer redraws the row.)
    if (!els.detailBtn.textContent?.endsWith('…')) {
      els.detailBtn.disabled = needs;
      els.detailBtn.title = needs ? `Download “${name}” first: its terrain detail is an add-on to it` : f.on ? `Remove the terrain detail of “${name}” from this Mac` : `Download the terrain detail of “${name}” to this Mac, for when it’s away from the NAS`;
    }
  }

  /** The block: this Mac's room; the World row; what's being copied and why it's slow or waits. */
  private renderDl() {
    const k = this.dl;
    this.redrawDl();
    if (!k) {
      this.dlSum.replaceChildren(this.dlErr ? h('div', { class: 'kp-line warn' }, this.dlErr) : h('div', { class: 'kp-line faint' }, h('span', { class: 'spin' }), ' Asking the map server…'));
      this.worldRow.hidden = true;
      this.resRow.hidden = true;
      return;
    }
    this.worldRow.hidden = !k.mirror;
    this.resRow.hidden = !k.mirror;
    if (!k.mirror) {
      this.dlSum.replaceChildren(h('div', { class: 'kp-line faint' }, 'This server keeps no copy of the map (it runs without a mirror)'));
      return;
    }
    const lines: HTMLElement[] = [];
    if (this.dlErr) {
      const at = new Date(this.dlAt).toLocaleTimeString('en-CA', { hour: '2-digit', minute: '2-digit', hour12: false });
      lines.push(h('div', { class: 'kp-line faint', title: this.dlErr }, `As of ${at}: the map server isn’t answering`));
    }
    const free = k.free ?? 0, reserve = k.reserve ?? 0;
    lines.push(h('div', { class: 'kp-line', title: 'What’s downloaded to this Mac, and the disk’s free space' },
      'This Mac: ', h('b', {}, size(k.here ?? 0)), ' downloaded · ', h('b', {}, size(free)), ' free'));
    this.dlSum.replaceChildren(...lines);
    this.renderReserve(k);
    this.renderWorld(k);
    // What's going on: the copy under way (why it's slow), waiting for room, away.
    const act: HTMLElement[] = [];
    const w = k.wanted;
    if (k.copying) {
      const c = k.copying;
      act.push(h('div', { class: 'kp-line', title: c.what }, `Copying ${c.what.replace(/^layers\//, '')} · ${size(c.have)} of ${size(c.bytes)}`,
        ...(c.slow ? [h('span', { class: 'faint' }, ` · the build is running: ${fmt.n((k.rate ?? 20e6) / 1e6)} MB/s`)] : [])));
    }
    if (w && (w.here < w.bytes || w.unknown > 0)) {
      const bar = h('div', { class: 'kp-bar', title: `${size(w.here)} of ${size(w.bytes)} downloaded` }, h('i', {}));
      (bar.firstChild as HTMLElement).style.width = `${(100 * w.here) / Math.max(1, w.bytes)}%`;
      act.push(bar);
      if (w.more > 0) act.push(h('div', { class: 'kp-line warn' }, `The downloads need ${size(w.more)} more room than there is above the reserve: they wait. Free some space on this Mac, or remove a download.`));
      else if (!k.online) act.push(h('div', { class: 'kp-line faint' }, `Away from the NAS: ${size(w.bytes - w.here)} still to copy once it’s back.`));
      else if (k.slow && !k.copying) act.push(h('div', { class: 'kp-line faint' }, `The build is running: downloads keep to ${fmt.n((k.rate ?? 20e6) / 1e6)} MB/s.`));
    } else if (free < reserve) {
      act.push(h('div', { class: 'kp-line warn' }, `${size(free)} free, less than the ${size(reserve)} this Mac keeps free: new downloads wait until there’s room. Downloads are never removed automatically, so to make room, remove one here or free space on this Mac.`));
    }
    // Downloaded regions there's no recipe for any more (removed since): removed from here.
    if (this.loadedAt > -Infinity) {
      for (const [id, rk] of Object.entries(k.regions)) {
        if (!rk.on || this.regions.some((r) => r.id === id)) continue;
        act.push(h('div', { class: 'kp-line warn' }, `“${rk.name}” is downloaded, but isn’t a region any more `,
          h('button', { class: 'pill', type: 'button', title: 'Remove it from this Mac', onclick: () => void dlRegion(id, false).then(() => this.pollDl(), (e) => this.sayDl(says(e), true)) }, 'Remove')));
      }
    }
    this.dlNow.replaceChildren(...act);
  }

  /** The World, zoomed out: its size, its state or what the map needs without it, its button; a
   * warning while regions are downloaded without it (away from the NAS, zoomed out, the map shows
   * nothing). */
  private renderWorld(k: DlStatus) {
    const wd = k.world;
    if (!wd) return;
    const regions = Object.values(k.regions).filter((r) => r.on).length;
    const state = wd.on ? h('span', { class: `rg-size ${stateClass(wd.state)}` }, `${sizeOf(wd)} · ${stateText(wd)}`) : h('span', { class: 'rg-size' }, sizeOf(wd));
    this.showProg(this.worldProg, wd, 'world', 'the World, zoomed out', () => dlUpdate(), (t, w) => this.sayDl(t, w));
    if (this.worldOn !== wd.on || !this.worldBtn) {
      this.worldBtn = this.dlButton(wd.on, 'the World, zoomed out', () => this.dl?.world?.bytes ?? 0, (o) => dlWorld(o), (o) =>
        o ? 'Downloading the World, zoomed out: the map then shows the whole world without the NAS, to zoom 10.' : 'The World is removed from this Mac: the map needs the NAS to show anything.');
      this.worldOn = wd.on;
    }
    const btn = this.worldBtn;
    btn.title = wd.on ? `Remove the World, zoomed out, from this Mac${regions ? ': away from the NAS, the downloaded regions then show only zoomed in' : ''}` : 'Download the World, zoomed out, to this Mac';
    const lacking = !wd.on && regions > 0;
    this.worldRow.classList.toggle('dl-lack', lacking);
    const off = lacking
      ? h('span', { class: 'rg2s warn', title: 'Away from the NAS, the downloaded regions show only zoomed in (zoom 11 and closer); zoomed out, and everywhere else, the map is blank. Download the World to see the whole map.' },
        `⚠ Not downloaded, but ${regions === 1 ? 'a region is' : `${regions} regions are`}: away from the NAS, zoomed out, the map shows nothing`)
      : h('span', { class: 'rg2s warn', title: 'Nothing is copied to this Mac unless it’s downloaded.' }, 'not downloaded: the map needs the NAS to show anything');
    this.worldRow.replaceChildren(
      h('div', { class: 'rg1' }, h('span', { class: 'rg-name rg-static', title: 'The worldwide files, every layer’s zooms 0–8, the landmark points and area details, and the basemap to zoom 10' }, 'World, zoomed out'),
        h('span', { class: 'rg-id faint' }, 'zooms 0–10'), btn),
      h('div', { class: 'rg2' },
        wd.on ? h('span', { class: 'rg2s faint' }, `downloaded ${new Date((wd.at ?? 0) * 1000).toLocaleDateString('en-CA', { month: 'short', day: 'numeric' })}`) : off,
        state),
      this.worldProg,
    );
  }

  /** Under a download's row: a progress bar while it's copied (downloading, or updating: then of
   * the update's files), and while it's behind the catalogue, what changed, the size to copy and an
   * Update button. Hidden when there's none of that. */
  private showProg(box: HTMLElement, d: Dl | undefined, key: string, what: string, act: () => Promise<unknown>, say: (text: string, warn?: boolean) => void, starting = false) {
    const kids: HTMLElement[] = [];
    if (starting) kids.push(progBar(null, 'Starting'));
    else if (d?.on && d.state && d.state !== 'done' && d.state !== 'missing') {
      const t = d.update ?? d;
      kids.push(progBar(t.here / Math.max(1, t.bytes), `${d.update ? 'Updating' : 'Downloading'}: ${size(t.here)} of ${size(t.bytes)} here (${pct(t)} %)`));
    }
    const b = d?.on ? d.behind : undefined;
    if (b && !d!.update) {
      const busy = this.updating.has(key);
      const btn = h('button', { class: 'rg-dl', type: 'button', title: `Copy catalogue ${b.cat}’s files for ${what} to this Mac; its own stay until they’re all here` }, busy ? 'Asking…' : 'Update') as HTMLButtonElement;
      btn.disabled = busy;
      btn.addEventListener('click', () => {
        this.updating.add(key);
        btn.disabled = true;
        btn.textContent = 'Asking…';
        act().then(() => say(`Updating ${what}.`), (e) => say(says(e), true)).finally(() => {
          this.updating.delete(key);
          void this.pollDl();
        });
      });
      const owe = b.unknown > 0 ? `${size(b.bytes)}…` : size(b.bytes);
      kids.push(h('div', { class: 'rg2' },
        h('span', { class: 'rg2s rg-behind', title: `Catalogue ${b.cat} has newer files for it: ${behindText(b)}${b.gone ? `, ${fmt.n(b.gone)} it no longer needs` : ''}. It stays at catalogue ${d!.cat} until it’s updated: the map reads its files here meanwhile, the NAS for the rest` }, `↻ out of date · ${behindText(b)}`),
        h('span', { class: 'rg-size', title: 'Still to copy' }, owe),
        btn));
    }
    box.replaceChildren(...kids);
    box.hidden = kids.length === 0;
  }

  /** A line under a region's row: what its Download, Cancel or Remove did, or why it didn't (gone a
   * little later unless it's a refusal; '' takes it off). A folder's goes under its heading. */
  private sayRow(row: HTMLElement, text: string, warn = false) {
    const dir = row.tagName === 'SUMMARY';
    (dir ? row.parentElement?.querySelector(':scope > .rg-rowmsg') : row.querySelector('.rg-rowmsg'))?.remove();
    if (!text) return;
    const m = h('div', { class: `rg-rowmsg${warn ? ' warn' : ''}` }, text);
    if (dir) row.after(m);
    else row.append(m);
    if (!warn) window.setTimeout(() => m.remove(), 8000);
  }

  /** The reserve's control from the status: its value unless it's being edited, its bounds. */
  private renderReserve(k: DlStatus) {
    const gb = Math.round((k.reserve_set ?? 0) / 1e9);
    const max = k.disk ? Math.floor(k.disk / 1e9) - 1 : null;
    this.resIn.min = String(Math.round((k.reserve_min ?? 5e9) / 1e9));
    if (max != null) this.resIn.max = String(max);
    const kept = k.reserve ?? 0;
    this.resIn.title = kept > (k.reserve_set ?? 0) ? `This Mac’s build keeps ${size(kept)} free (its room target), so downloads leave that` : '';
    if (document.activeElement !== this.resIn && !this.resBusy && this.resIn.value !== String(gb) && this.resBtn.hidden) this.resIn.value = String(gb);
    this.resEdited();
  }

  /** What the typed reserve would be: GB, or why it can't be. */
  private resCheck(): number | string {
    const v = this.resIn.value.trim();
    const n = Number(v);
    const min = Number(this.resIn.min || 5), max = this.resIn.max ? Number(this.resIn.max) : Infinity;
    if (!v || !Number.isInteger(n)) return 'whole GB';
    if (n < min) return `at least ${min} GB`;
    if (n > max) return `at most ${max} GB (the disk is ${max + 1} GB)`;
    return n;
  }

  private resEdited() {
    if (this.resBusy) return;
    const cur = Math.round((this.dl?.reserve_set ?? 0) / 1e9);
    const c = this.resCheck();
    const changed = this.resIn.value.trim() !== String(cur);
    this.resBtn.hidden = !changed;
    this.resBtn.disabled = typeof c === 'string';
    this.resIn.classList.toggle('bad', changed && typeof c === 'string');
    if (changed && typeof c === 'string') this.sayRes(c, true);
    else if (this.resRefused) return;
    else if (changed) this.sayRes('');
    else if (this.resSay.classList.contains('warn')) this.sayRes('');
  }

  private sayRes(text: string, warn = false) {
    this.resSay.textContent = text;
    this.resSay.className = `kp-res-say${warn ? ' warn' : text ? ' ok' : ''}`;
  }

  private async saveReserve() {
    const c = this.resCheck();
    if (typeof c === 'string') return this.sayRes(c, true);
    if (this.resBusy || c === Math.round((this.dl?.reserve_set ?? 0) / 1e9)) return;
    this.resBusy = true;
    this.resBtn.disabled = true;
    this.sayRes('saving…');
    try {
      const r = await setReserve(c);
      if (this.dl) { this.dl.reserve = r.reserve; this.dl.reserve_set = r.reserve_set; }
      this.resBusy = false;
      this.resRefused = false;
      this.resBtn.hidden = true;
      this.sayRes('✓ saved');
      window.setTimeout(() => { if (this.resSay.textContent === '✓ saved') this.sayRes(''); }, 4000);
    } catch (e) {
      this.resBusy = false;
      this.resRefused = true;
      this.sayRes(says(e), true);
      this.resBtn.disabled = false;
    }
    void this.pollDl();
  }

  private sayDl(text: string, warn = false) {
    this.dlMsg.textContent = text;
    this.dlMsg.classList.toggle('warn', warn);
  }
}

/** A download's button: Download; Cancel while it's coming (asked, copying, waiting); Remove once
 * it's here (or being updated: it's whole meanwhile). */
type Mode = 'download' | 'cancel' | 'remove';
function modeOf(on: boolean, rk: RegionDl): Mode {
  if (!on) return 'download';
  return rk.on && (rk.state === 'done' || rk.state === 'updating') ? 'remove' : 'cancel';
}

/** A thin bar: `share` done (null: under way, not sized yet). */
function progBar(share: number | null, title: string): HTMLElement {
  const bar = h('div', { class: `kp-bar rg-bar${share === null ? ' busy' : ''}`, title }, h('i', {}));
  if (share !== null) (bar.firstChild as HTMLElement).style.width = `${100 * Math.min(1, share)}%`;
  return bar;
}

/** A flag of fixed width (flag-icons' 4:3, flags.ts), or a bordered "?" where there's none. */
function flag(code: string | null): HTMLElement {
  const url = code ? FLAGS.get(code) : undefined;
  if (!url) return h('span', { class: 'rg-flag none', title: 'No flag' }, '?');
  return h('span', { class: 'rg-flag' }, h('img', { src: url, alt: '', loading: 'lazy', decoding: 'async', draggable: 'false' }));
}

