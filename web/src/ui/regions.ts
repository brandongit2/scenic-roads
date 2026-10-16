// The Regions panel (Layers → Regions; docs/plan.md §1): the regions the map is built for, read
// only. Regions are files in the drop box's inputs/regions/ (docs/inputs.md §6.1), changed there
// alone and checked before they're used: the list shows them grouped by their folders, each with
// its check's state (and the findings holding it), and the coverage they add up to on the map.
// The coverage drawn is the one the map's catalog was built for; a region it doesn't have (added
// or redrawn since) is listed as pending.
// Downloads (docs/plan.md §4, "Mirror, per Mac"): nothing is copied to this Mac unless it's
// downloaded here: the World, zoomed out, and each region or view, each with its size, its state
// and a Download or Remove button; this Mac's room (downloads.ts).
import { dlRegion, dlStatus, dlView, dlWorld, dropView, renameView, setReserve, size, sizeOf, stateClass, stateText, viewSize, type DlStatus, type RegionDl, type ViewDl } from '../downloads';
import * as prefs from '../prefs';
import {
  RegionLayers, RegionsError, areaOutline, entryLabel, getCoverage, getRegionsLayer, listRegions,
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
  /** Set by the app: the ground in view, [lon, lat] points (Download this view). */
  viewOutline: () => [number, number][] = () => [];

  private covBox = h('input', { type: 'checkbox' });
  private covCount = h('span', { class: 'km' });
  private regBox = h('input', { type: 'checkbox' });
  private regNote = h('span', { class: 'km' });
  private layer: RegionsLayer | null = null;
  private list = h('div', { class: 'rg-list' });
  private note = h('div', { class: 'rg-note' });
  // Downloads: this Mac's room, the World, Download this view, the views downloaded.
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
  private dlBtn = h('button', { class: 'pill', type: 'button', title: 'Download the area in view to this Mac, for when it’s away from the NAS' }, 'Download this view');
  private dlViews = h('div', { class: 'rg-list kp-views' });
  private dlMsg = h('div', { class: 'rg-msg' });
  /** Download this view's question: the view's size, and download it or not. */
  private dlAsk = h('div', { class: 'rg-ask kp-ask' });
  /** The downloads' state, as last asked (null: not yet), when (ms), or why it couldn't be. */
  private dl: DlStatus | null = null;
  private dlAt = 0;
  private dlErr = '';
  private dlTok = 0;
  private dlTimer = 0;
  /** What each region row shows of its download, updated in place as the state comes (a row isn't
   * drawn again: a hover or a rename in it stays). */
  private dlEls = new Map<string, { row: HTMLElement; slot: HTMLElement; btn: HTMLButtonElement | null; on: boolean; size: HTMLElement }>();
  /** The downloaded views listed (their ids and names), and their size lines. */
  private viewsKey = '';
  private viewEls = new Map<string, HTMLElement>();

  private regions: Region[] = [];
  /** The folders the list is grouped by (null: a server from before folders). */
  private tree: Folder | null = null;
  private bad: [string, string][] = [];
  private coverage: Coverage | null = null;
  /** The outlines of the regions the catalog doesn't have yet, from their osm: entries' areas. */
  private pendingOutlines = new Map<string, Outline[]>();
  /** The catalog the map shows, as last told (setCatalog). */
  private catalogN: number | undefined;
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
    this.dlAsk.hidden = true;
    this.dlBtn.addEventListener('click', () => void this.downloadThisView());
    this.resBtn.hidden = true;
    this.resRow.hidden = true;
    this.resIn.addEventListener('input', () => {
      this.resRefused = false;
      this.resEdited();
    });
    this.resIn.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') void this.saveReserve();
      if (e.key === 'Escape') {
        this.resIn.value = String(Math.round((this.dl?.reserve ?? 0) / 1e9));
        this.resRefused = false;
        this.resEdited();
      }
    });
    this.resBtn.addEventListener('click', () => void this.saveReserve());
    this.nodes = [
      h('label', { class: 'tog', title: 'Every region’s outline on the map: the map is built within them' }, this.covBox, h('span', {}, 'Coverage on the map'), this.covCount),
      h('label', { class: 'tog', title: 'Each region’s accepted outline in its own colour, with its name: overlaps and gaps show; dashed while a finding holds it' }, this.regBox, h('span', {}, 'Regions on the map'), this.regNote),
      h('div', { class: 'subhd', title: 'What’s downloaded to this Mac, for when it’s away from the NAS: nothing else is copied here' }, 'Downloads on this Mac'),
      this.dlSum,
      this.resRow,
      this.worldRow,
      this.dlNow,
      this.dlViews,
      h('div', { class: 'kp-act' }, this.dlBtn),
      this.dlAsk,
      this.dlMsg,
      h('div', { class: 'subhd', title: 'Each region’s size, and Download or Remove' }, 'Regions'),
      this.list,
      this.note,
    ];
    this.renderList();
  }

  /** The map is ready: the coverage drawn if it's on. */
  start() {
    if (this.covBox.checked || this.regBox.checked) void this.refresh();
  }

  /** The map's catalog now (catalog.ts). A new one was built for its own coverage: drawn again
   * while it's shown or the panel is open. */
  setCatalog(n: number | undefined) {
    if (n === undefined || n === this.catalogN) return;
    this.catalogN = n;
    const had = this.coverage?.catalog;
    if (had !== undefined && had !== n && (this.covBox.checked || this.open)) void this.refresh();
  }

  /** The section opened (the list again, unless just loaded) or closed (its previews leave the
   * map). */
  setOpen(open: boolean) {
    this.open = open;
    if (open) {
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

  /** Not on the map yet: the catalog has no region by its id with its outline (added or redrawn
   * since it was made). Never when the catalog doesn't say which regions it was built for. */
  private isPending(r: Region): boolean {
    const built = this.coverage?.regions;
    if (!built) return false;
    const c = built.find((b) => b.id === r.id);
    return !c || c.outline.length !== r.outline.length || c.outline.some((e, i) => e !== r.outline[i]);
  }

  /** A region's outlines: the catalog's (each entry's; none until the coverage is loaded), or for a
   * pending one its osm: entries' areas, once loaded. */
  private outlinesOf(r: Region): Outline[] {
    if (this.isPending(r)) return this.pendingOutlines.get(r.id) ?? [];
    return (this.coverage?.features ?? []).filter((f) => f.properties?.region === r.id);
  }

  /** The pending regions' outlines, from their osm: entries' areas (other entries have none to
   * show until built), for naming their entries and showing them on the map; then the list again. */
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

  private renderList() {
    // (A hovered row goes without a mouseleave.)
    if (this.hoverRegion) {
      this.hoverRegion = null;
      this.layers.setHover(null);
    }
    this.dlEls.clear();
    const n = this.regions.length;
    this.covCount.textContent = n ? `${n} region${n === 1 ? '' : 's'}` : '';
    // Grouped by the folder their files are in under inputs/regions/: the top level first, then the
    // folders in order.
    const byId = new Map(this.regions.map((r) => [r.id, r]));
    const placed = new Set<string>();
    const rows = (ids: string[]) => ids.flatMap((id) => {
      const r = byId.get(id);
      if (!r) return [];
      placed.add(id);
      return [this.regionRow(r)];
    });
    const count = (f: Folder): number => f.regions.filter((id) => byId.has(id)).length + f.folders.reduce((n, g) => n + count(g), 0);
    const group = (f: Folder): HTMLElement => {
      const key = `regions.folder.${f.path}`;
      const box = h('details', { class: 'rg-dir', open: prefs.load(key, true) }) as HTMLDetailsElement;
      box.addEventListener('toggle', () => prefs.save(key, box.open));
      box.append(h('summary', { title: `inputs/regions/${f.path}/` }, f.name, h('span', { class: 'faint' }, ` ${count(f)}`)), ...rows(f.regions), ...f.folders.map(group));
      return box;
    };
    const tree = this.tree;
    const nested = tree ? [...rows(tree.regions), ...tree.folders.map(group)] : [];
    // Regions the tree doesn't hold (not in the accepted version yet) at the top level.
    this.list.replaceChildren(
      ...this.regions.filter((r) => !placed.has(r.id)).map((r) => this.regionRow(r)),
      ...nested,
      ...this.bad.map(([file, e]) => h('div', { class: 'rg-bad', title: e }, h('b', {}, file), ` doesn’t parse: ${e}`)),
    );
    if (!n && !this.bad.length && this.loadedAt > -Infinity) this.list.append(h('div', { class: 'faint rg-empty' }, 'No regions yet'));
  }

  /** Per region, how many of its areas are built (the lead's heartbeat). */
  private progress: Record<string, { built: number; total: number }> = {};

  setProgress(p: Record<string, { built: number; total: number }> | undefined) {
    const next = p ?? {};
    if (JSON.stringify(next) === JSON.stringify(this.progress)) return;
    this.progress = next;
    this.renderList();
  }

  /** A region's state in words: waiting or building (how many of its areas), from the lead's
   * heartbeat; else pending while the map's catalog doesn't have it, built when the heartbeat or
   * the catalog says so. */
  private stateOf(r: Region): { text: string; cls: string; title?: string } | null {
    // The check comes first: a region not accepted yet isn't what's built.
    if (r.state === 'checking') return { text: 'checking…', cls: 'wait', title: 'Its file changed: being checked before it’s used' };
    if (r.state === 'held') {
      const fs = r.held ?? [];
      return { text: `held: ${fs[0]?.message ?? 'a finding holds it'}${fs.length > 1 ? ` (+${fs.length - 1})` : ''}`, cls: 'held', title: `Its last accepted outline stays in use until these are fixed:\n${fs.map((f) => `${f.id}: ${f.message}`).join('\n')}` };
    }
    const p = this.progress[r.id];
    if (p?.total && p.built < p.total) return p.built === 0 ? { text: 'waiting to build', cls: 'wait' } : { text: `building · ${p.built} of ${p.total} areas`, cls: 'on' };
    if (this.isPending(r)) return { text: 'pending', cls: 'wait', title: 'Not on the map yet: it’s added when the lead next publishes the map data' };
    if (p?.total || this.coverage?.regions) return { text: 'built', cls: 'ok' };
    return null;
  }

  private regionRow(r: Region): HTMLElement {
    const fs = this.outlinesOf(r);
    const summary = r.outline.map((e) => entryLabel(e, fs.find((f) => f.properties?.entry === e)?.properties as Area | undefined)).join(' + ');
    const row = h('div', { class: 'rg' });
    const show = () => {
      const state = this.stateOf(r);
      const name = h('span', { class: 'rg-name', title: `${r.name}: show it` }, r.name);
      name.addEventListener('click', () => {
        const b = bboxOf(fs);
        if (b) this.onFit(b);
      });
      const slot = h('span', { class: 'rg-dlslot' });
      const sz = h('span', { class: 'rg-size' });
      this.dlEls.set(r.id, { row, slot, btn: null, on: false, size: sz });
      row.replaceChildren(
        h('div', { class: 'rg1' }, name, h('span', { class: 'rg-id faint', title: `Its file: inputs/regions/${r.path ?? `${r.id}.toml`}` }, r.id),
          ...(state ? [h('span', { class: `rg-state ${state.cls}`, title: state.title }, state.text)] : []),
          slot),
        h('div', { class: 'rg2' }, h('span', { class: 'rg2s', title: summary }, summary), sz),
      );
      this.showDl(r.id);
    };
    show();
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

  /** A region's size, and how much of it is here or its state when downloaded; its button. */
  private showDl(id: string) {
    const els = this.dlEls.get(id);
    if (!els) return;
    const k = this.dl;
    const rk: RegionDl | undefined = k?.regions[id];
    const r = this.regions.find((x) => x.id === id);
    let text = '', cls = 'rg-size', title = '';
    if (!k || !k.mirror) text = '';
    else if (!rk) text = 'not built yet';
    else if (rk.on) {
      text = rk.state === 'missing' ? stateText(rk, rk.state) : `${sizeOf(rk)} · ${stateText(rk, rk.state)}`;
      cls += ` ${stateClass(rk.state)}`;
    } else {
      text = sizeOf(rk);
      const world = k.world && !k.world.on ? k.world : null;
      title = world ? `Its packs and the basemap’s zooms 11–14 over it: ${size(rk.bytes)}. Downloading it downloads the World, zoomed out (${sizeOf(world)}), too: it needs it away from the NAS` : '';
    }
    els.size.className = cls;
    els.size.textContent = text;
    els.size.title = title || (rk ? `Its packs and the basemap’s zooms 11–14 over it: ${size(rk.bytes)}, of which ${size(rk.here)} on this Mac` : '');
    const on = !!rk?.on;
    if (els.on !== on || !els.btn) {
      const name = r?.name ?? rk?.name ?? id;
      const btn = this.dlButton(on, `“${name}”`, () => this.dl?.regions[id]?.bytes ?? 0, (o) => dlRegion(id, o), (o) =>
        o ? `Downloading “${name}” to this Mac${this.dl?.world?.on ? '' : ', with the World, zoomed out'}.` : `“${name}” is removed from this Mac.`, (t, w) => this.sayRow(els.row, t, w));
      btn.hidden = !k?.mirror;
      if (els.btn) els.btn.replaceWith(btn);
      else els.slot.append(btn);
      els.btn = btn;
      els.on = on;
    }
    els.btn.hidden = !k?.mirror;
  }

  /** The block: this Mac's room; the World row; what's being copied and why it's slow or waits;
   * the views. */
  private renderDl() {
    const k = this.dl;
    for (const id of this.dlEls.keys()) this.showDl(id);
    if (!k) {
      this.dlSum.replaceChildren(this.dlErr ? h('div', { class: 'kp-line warn' }, this.dlErr) : h('div', { class: 'kp-line faint' }, h('span', { class: 'spin' }), ' Asking the map server…'));
      this.dlBtn.hidden = true;
      this.worldRow.hidden = true;
      this.resRow.hidden = true;
      return;
    }
    this.dlBtn.hidden = !k.mirror;
    this.worldRow.hidden = !k.mirror;
    this.resRow.hidden = !k.mirror;
    if (!k.mirror) {
      this.dlSum.replaceChildren(h('div', { class: 'kp-line faint' }, 'This server keeps no copy of the map (it runs without a mirror)'));
      this.renderViews([]);
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
    this.renderViews(k.views);
  }

  /** The World, zoomed out: its size, its state or what the map needs without it, its button. */
  private renderWorld(k: DlStatus) {
    const wd = k.world;
    if (!wd) return;
    const areas = Object.values(k.regions).filter((r) => r.on).length + k.views.length;
    const state = wd.on ? h('span', { class: `rg-size ${stateClass(wd.state)}` }, `${sizeOf(wd)} · ${stateText(wd, wd.state)}`) : h('span', { class: 'rg-size' }, sizeOf(wd));
    if (this.worldOn !== wd.on || !this.worldBtn) {
      this.worldBtn = this.dlButton(wd.on, 'the World, zoomed out', () => this.dl?.world?.bytes ?? 0, (o) => dlWorld(o), (o) =>
        o ? 'Downloading the World, zoomed out: the map then shows the whole world without the NAS, to zoom 10.' : 'The World is removed from this Mac: the map needs the NAS to show anything.');
      this.worldOn = wd.on;
    }
    const btn = this.worldBtn;
    btn.disabled = wd.on && areas > 0;
    btn.title = btn.disabled ? 'The downloaded regions and views need it away from the NAS: remove them first' : wd.on ? 'Remove the World, zoomed out, from this Mac' : 'Download the World, zoomed out, to this Mac';
    this.worldRow.replaceChildren(
      h('div', { class: 'rg1' }, h('span', { class: 'rg-name rg-static', title: 'The worldwide files, every layer’s zooms 0–8, the landmark points and area details, and the basemap to zoom 10' }, 'World, zoomed out'),
        h('span', { class: 'rg-id faint' }, 'zooms 0–10'), btn),
      h('div', { class: 'rg2' },
        wd.on ? h('span', { class: 'rg2s faint' }, `downloaded ${new Date((wd.at ?? 0) * 1000).toLocaleDateString('en-CA', { month: 'short', day: 'numeric' })}`) : h('span', { class: 'rg2s warn', title: 'Nothing is copied to this Mac unless it’s downloaded. A region’s or a view’s download brings it too.' }, 'not downloaded: the map needs the NAS to show anything'),
        state),
    );
  }

  /** The downloaded views: listed again when they change, else their lines updated in place. */
  private renderViews(views: ViewDl[]) {
    const key = JSON.stringify(views.map((v) => [v.id, v.name]));
    if (key !== this.viewsKey) {
      this.viewsKey = key;
      this.viewEls.clear();
      this.dlViews.replaceChildren(...views.map((v) => this.viewRow(v)));
    }
    for (const v of views) {
      const el = this.viewEls.get(v.id);
      if (!el) continue;
      el.textContent = `${sizeOf(v)} · ${stateText(v, v.state)}`;
      el.className = `rg-size ${stateClass(v.state)}`;
    }
  }

  private viewRow(v: ViewDl): HTMLElement {
    const row = h('div', { class: 'rg' });
    const ring = v.outline;
    const box = (): Bbox => [Math.min(...ring.map((p) => p[0])), Math.min(...ring.map((p) => p[1])), Math.max(...ring.map((p) => p[0])), Math.max(...ring.map((p) => p[1]))];
    const feature: GeoJSON.Feature = { type: 'Feature', properties: {}, geometry: { type: 'Polygon', coordinates: [[...ring, ring[0]]] } };
    const show = () => {
      const name = h('span', { class: 'rg-name', title: `${v.name}: show it` }, v.name);
      name.addEventListener('click', () => this.onFit(box()));
      const sz = h('span', { class: 'rg-size' });
      this.viewEls.set(v.id, sz);
      const rm = this.dlButton(true, `“${v.name}”`, () => this.dl?.views.find((x) => x.id === v.id)?.bytes ?? 0, () => dropView(v.id).then(() => this.layers.setHover(null)), () => `“${v.name}” is removed from this Mac.`);
      row.replaceChildren(
        h('div', { class: 'rg1' }, name, h('span', { class: 'rg-id faint' }, 'view'),
          h('button', { class: 'rg-act', type: 'button', title: 'Rename', onclick: () => this.renameView(v, row, show) }, '✎'), rm),
        h('div', { class: 'rg2' }, h('span', { class: 'rg2s faint' }, `downloaded ${new Date(v.at * 1000).toLocaleDateString('en-CA', { month: 'short', day: 'numeric' })}`), sz),
      );
      sz.textContent = `${sizeOf(v)} · ${stateText(v, v.state)}`;
      sz.className = `rg-size ${stateClass(v.state)}`;
    };
    show();
    row.addEventListener('mouseenter', () => this.layers.setHover([feature]));
    row.addEventListener('mouseleave', () => this.layers.setHover(null));
    return row;
  }

  /** A line under a region's row: what its Download or Remove did, or why it didn't (gone a little
   * later unless it's a refusal). */
  private sayRow(row: HTMLElement, text: string, warn = false) {
    row.querySelector('.rg-rowmsg')?.remove();
    const m = h('div', { class: `rg-rowmsg${warn ? ' warn' : ''}` }, text);
    row.append(m);
    if (!warn) window.setTimeout(() => m.remove(), 8000);
  }

  /** The reserve's control from the status: its value unless it's being edited, its bounds. */
  private renderReserve(k: DlStatus) {
    const gb = Math.round((k.reserve ?? 0) / 1e9);
    const max = k.disk ? Math.floor(k.disk / 1e9) - 1 : null;
    this.resIn.min = String(Math.round((k.reserve_min ?? 5e9) / 1e9));
    if (max != null) this.resIn.max = String(max);
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
    const cur = Math.round((this.dl?.reserve ?? 0) / 1e9);
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
    if (this.resBusy || c === Math.round((this.dl?.reserve ?? 0) / 1e9)) return;
    this.resBusy = true;
    this.resBtn.disabled = true;
    this.sayRes('saving…');
    try {
      const r = await setReserve(c);
      if (this.dl) this.dl.reserve = r.reserve;
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

  /** Download this view: what it would take first (the ground on screen now), then download it or
   * not. */
  private async downloadThisView() {
    const outline = this.viewOutline();
    if (outline.length < 3) return this.sayDl('No ground in view to download', true);
    this.dlBtn.disabled = true;
    this.sayDl('');
    const done = () => {
      this.dlAsk.hidden = true;
      this.dlBtn.disabled = false;
    };
    try {
      const z = await viewSize(outline);
      const go = h('button', { class: 'pill on', type: 'button' }, 'Download');
      go.addEventListener('click', () => {
        go.disabled = true;
        go.textContent = 'Downloading…';
        void dlView(outline)
          .then((v) => this.sayDl(`Downloading “${v.name}” to this Mac (✎ renames it).`), (e) => this.sayDl(says(e), true))
          .finally(() => {
            done();
            void this.pollDl();
          });
      });
      const cancel = h('button', { class: 'pill', type: 'button', onclick: done }, z.fits ? 'Cancel' : 'OK');
      const world = z.with_world > 0 ? `, and the World, zoomed out (${size(z.with_world)}), with it` : '';
      this.dlAsk.replaceChildren(
        z.fits
          ? h('div', {}, `This view: ${size(z.bytes)}${z.here ? ` (${size(z.here)} here)` : ''}${world}. ${size(z.need)} to copy in all, of the ${size(z.room)} free above the reserve.`)
          : h('div', { class: 'warn' }, `This view${world} takes ${size(z.need)} more on this Mac with what’s downloaded, and it has ${size(z.room)} free above the reserve: free some space, or download a smaller view.`),
        h('div', { class: 'pills' }, ...(z.fits ? [go] : []), cancel),
      );
      this.dlAsk.hidden = false;
    } catch (e) {
      this.sayDl(says(e), true);
      done();
    }
  }

  private renameView(v: ViewDl, row: HTMLElement, back: () => void) {
    const input = h('input', { type: 'text', class: 'rg-in', value: v.name, spellcheck: false });
    const save = async () => {
      const name = input.value.trim();
      if (!name || name === v.name) return back();
      try {
        await renameView(v.id, name);
        this.sayDl(`Renamed “${v.name}” to “${name}”`);
      } catch (e) {
        this.sayDl(says(e), true);
        back();
      }
      void this.pollDl();
    };
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') void save();
      else if (e.key === 'Escape') {
        e.stopPropagation();
        back();
      }
    });
    row.replaceChildren(h('div', { class: 'rg1' }, input,
      h('button', { class: 'rg-act on', type: 'button', title: 'Save', onclick: () => void save() }, '✓'),
      h('button', { class: 'rg-act on', type: 'button', title: 'Cancel', onclick: back }, '×')));
    input.focus();
    input.select();
  }

}
