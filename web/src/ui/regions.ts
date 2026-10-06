// The Regions panel (Layers → Regions; docs/plan.md §1): the regions the map is built for, the
// coverage they add up to on the map, and new regions made of administrative areas, found by name
// or by a click on the map. Edits go to the recipes on the NAS; the build Mac builds what they
// change when it can (the status bar shows it). Renaming rebuilds nothing.
// The coverage drawn is the one the map's catalog was built for; a recipe it doesn't have (added
// or redrawn since, or waiting on this Mac to go to the NAS) is listed as pending.
// On this Mac (docs/plan.md §4, "Mirror, per Mac"): each region's size and how much of it is here,
// kept for offline use or not, the views kept, and the mirror's state (keep.ts).
import { dropView, keepRegion, keepStatus, keepView, renameView, size, stateText, pct, type KeepStatus, type KeptView, type RegionKeep } from '../keep';
import * as prefs from '../prefs';
import {
  QUEUED, RegionLayers, RegionsError, addRegion, areaName, areaOutline, areasAt, editRegion, entryLabel, getCoverage, km2, levelName, listRegions, removeRegion, searchAreas, slug, validId,
  type Area, type Coverage, type Region,
} from '../regions';
import { fmt, h } from './dom';

/** Typing pauses this long before a search. */
const SEARCH_MS = 250;
/** Opened again within this long of the last load: the list as it was. */
const FRESH_MS = 10_000;
const PICK_HINT = 'Click a place for the areas around it · Esc to stop';

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
const union = (bs: Bbox[]): Bbox => [Math.min(...bs.map((b) => b[0])), Math.min(...bs.map((b) => b[1])), Math.max(...bs.map((b) => b[2])), Math.max(...bs.map((b) => b[3]))];

export class RegionsPanel {
  /** The section's content. */
  readonly nodes: Node[];
  /** Waiting for a click on the map (main.ts hands it to pickAt). */
  picking = false;
  /** Set by the app: the view framed on a box [w, s, e, n]; picking on or off (the cursor); a
   * change was saved (the status bar asks the server again). */
  onFit: (bbox: Bbox) => void = () => {};
  onPicking: (on: boolean) => void = () => {};
  onChanged: () => void = () => {};
  /** Set by the app: the ground in view, [lon, lat] points (Keep this view). */
  viewOutline: () => [number, number][] = () => [];

  private covBox = h('input', { type: 'checkbox' });
  private covCount = h('span', { class: 'km' });
  private list = h('div', { class: 'rg-list' });
  private note = h('div', { class: 'rg-note' });
  private search = h('input', { type: 'search', class: 'rg-q', placeholder: 'Find a place by name', spellcheck: false, autocomplete: 'off' });
  private pickBtn = h('button', { class: 'pill', type: 'button', title: 'Click a place on the map for the areas around it' }, 'Pick on map');
  private hint = h('div', { class: 'rg-hint' });
  private cands = h('div', { class: 'rg-cands' });
  private draftBox = h('div', { class: 'rg-draft' });
  private draftHd = h('span', { class: 'faint' });
  private chips = h('div', { class: 'rg-chips' });
  private idIn = h('input', { type: 'text', spellcheck: false, autocomplete: 'off', title: 'The recipe’s id (its file name): lower-case letters, digits and dashes' });
  private nameIn = h('input', { type: 'text', spellcheck: false, autocomplete: 'off' });
  private saveBtn = h('button', { class: 'pill on', type: 'button' }, 'Add region');
  private msg = h('div', { class: 'rg-msg' });
  // On this Mac: the mirror's state, Keep this view, the kept views.
  private keepSum = h('div', { class: 'kp-sum' });
  private keepBtn = h('button', { class: 'pill', type: 'button', title: 'Keep the area in view on this Mac, for when it’s away from the NAS: its files are copied first and never let go of' }, 'Keep this view');
  private keepViews = h('div', { class: 'rg-list kp-views' });
  private keepMsg = h('div', { class: 'rg-msg' });
  /** The mirror's state, as last asked (null: not yet), or why it couldn't be. */
  private keep: KeepStatus | null = null;
  private keepErr = '';
  private keepTok = 0;
  private keepTimer = 0;
  /** What each region row shows of it, updated in place as the state comes (a row isn't drawn
   * again: a hover or a rename in it stays). */
  private keepEls = new Map<string, { sw: HTMLInputElement; size: HTMLElement }>();
  /** The kept views listed (their ids and names), and their size lines. */
  private viewsKey = '';
  private viewEls = new Map<string, HTMLElement>();

  private regions: Region[] = [];
  private bad: [string, string][] = [];
  private coverage: Coverage | null = null;
  /** The outlines of the regions the catalog doesn't have yet, from their osm: entries' areas. */
  private pendingOutlines = new Map<string, Outline[]>();
  /** The catalog the map shows, as last told (setCatalog). */
  private catalogN: number | undefined;
  private loadedAt = -Infinity;
  private loadTok = 0;
  private open = false;
  /** The areas listed (a search's or a pick's), and the one picked with the arrow keys. */
  private found: Area[] = [];
  private active = -1;
  /** The areas chosen for a new region (its outline is their union). */
  private draft: Area[] = [];
  /** The id and name typed (else they follow the first area chosen). */
  private idTyped = false;
  private nameTyped = false;
  private searchTimer = 0;
  private searchAbort: AbortController | null = null;
  private hoverTok = 0;
  private draftTok = 0;
  /** The region whose row is hovered (its outline shown). */
  private hoverRegion: string | null = null;

  constructor(private layers: RegionLayers) {
    this.covBox.checked = prefs.load('regions.coverage', false);
    this.covBox.addEventListener('change', () => {
      prefs.save('regions.coverage', this.covBox.checked);
      this.layers.setCoverage(this.covBox.checked, this.coverage ?? undefined);
      if (this.covBox.checked && !this.coverage) void this.refresh();
    });
    this.search.addEventListener('input', () => {
      clearTimeout(this.searchTimer);
      this.searchTimer = window.setTimeout(() => void this.runSearch(), SEARCH_MS);
    });
    this.search.addEventListener('keydown', (e) => this.keys(e));
    this.pickBtn.addEventListener('click', () => this.setPicking(!this.picking));
    this.cands.addEventListener('mouseleave', () => this.preview(this.found[this.active] ?? null));
    this.idIn.addEventListener('input', () => (this.idTyped = true));
    this.nameIn.addEventListener('input', () => (this.nameTyped = true));
    for (const el of [this.idIn, this.nameIn]) {
      el.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') void this.save();
      });
    }
    this.saveBtn.addEventListener('click', () => void this.save());
    this.draftBox.append(
      h('div', { class: 'rg-dhd' }, h('span', {}, 'New region'), this.draftHd),
      this.chips,
      h('label', { class: 'rg-field' }, h('span', { class: 'muted' }, 'Name'), this.nameIn),
      h('label', { class: 'rg-field' }, h('span', { class: 'muted' }, 'Id'), this.idIn),
      h('div', { class: 'pills' }, this.saveBtn, h('button', { class: 'pill', type: 'button', onclick: () => this.clearDraft() }, 'Clear')),
    );
    this.draftBox.hidden = true;
    this.keepBtn.addEventListener('click', () => void this.keepThisView());
    this.nodes = [
      h('label', { class: 'tog', title: 'Every region’s outline on the map: the map is built within them' }, this.covBox, h('span', {}, 'Coverage on the map'), this.covCount),
      this.list,
      this.note,
      h('div', { class: 'subhd', title: 'This Mac’s copy of the map, for when it’s away from the NAS' }, 'On this Mac'),
      this.keepSum,
      h('div', { class: 'kp-act' }, this.keepBtn),
      this.keepViews,
      this.keepMsg,
      h('div', { class: 'subhd' }, 'Add a region'),
      h('div', { class: 'rg-search' }, this.search, this.pickBtn),
      this.hint,
      this.cands,
      this.draftBox,
      this.msg,
    ];
    this.renderList();
  }

  /** The map is ready: the coverage drawn if it's on. */
  start() {
    if (this.covBox.checked) void this.refresh();
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
      this.showDraft();
      void this.pollKeep();
    } else {
      this.setPicking(false);
      this.layers.setHover(null);
      this.layers.setDraft([]);
      clearTimeout(this.keepTimer);
      this.keepTok++;
    }
  }

  // ---- the regions ------------------------------------------------------------------

  /** The regions and the coverage again. */
  private async refresh() {
    const tok = ++this.loadTok;
    if (!this.regions.length) this.note.replaceChildren(h('span', { class: 'spin' }), ' Loading the regions…');
    const [list, cov] = await Promise.allSettled([listRegions(), getCoverage()]);
    if (tok !== this.loadTok) return;
    this.loadedAt = performance.now();
    if (cov.status === 'fulfilled') {
      this.coverage = cov.value;
      this.layers.setCoverage(this.covBox.checked, cov.value);
    }
    if (list.status === 'fulfilled') {
      this.regions = list.value.regions;
      this.bad = list.value.bad;
      this.note.replaceChildren();
      this.note.classList.remove('warn');
      // Away from home: the list as last read, with what's waiting to go to the NAS.
      const pending = list.value.pending ?? 0;
      if (list.value.offline || pending > 0) {
        this.note.textContent = [list.value.offline ? 'Away from the NAS: the list as last read' : '', pending ? `${pending} change${pending === 1 ? '' : 's'} waiting to go to the NAS` : ''].filter(Boolean).join(' · ');
      }
    } else {
      this.note.textContent = says(list.reason);
      this.note.classList.add('warn');
    }
    this.renderList();
    this.renderCands();
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
    this.keepEls.clear();
    const n = this.regions.length;
    this.covCount.textContent = n ? `${n} region${n === 1 ? '' : 's'}` : '';
    this.list.replaceChildren(
      ...this.regions.map((r) => this.regionRow(r)),
      ...this.bad.map(([file, e]) => h('div', { class: 'rg-bad', title: e }, h('b', {}, file), ` doesn’t parse: ${e}`)),
    );
    if (!n && !this.bad.length && this.loadedAt > -Infinity) this.list.append(h('div', { class: 'faint rg-empty' }, 'No regions yet'));
  }

  /** Per region, how many of its areas are built (the build Mac's heartbeat). */
  private progress: Record<string, { built: number; total: number }> = {};

  setProgress(p: Record<string, { built: number; total: number }> | undefined) {
    const next = p ?? {};
    if (JSON.stringify(next) === JSON.stringify(this.progress)) return;
    this.progress = next;
    this.renderList();
  }

  /** A region's state in words: waiting or building (how many of its areas), from the build Mac's
   * heartbeat; else pending while the map's catalog doesn't have it, built when the heartbeat or
   * the catalog says so. */
  private stateOf(r: Region): { text: string; cls: string; title?: string } | null {
    const p = this.progress[r.id];
    if (p?.total && p.built < p.total) return p.built === 0 ? { text: 'waiting to build', cls: 'wait' } : { text: `building · ${p.built} of ${p.total} areas`, cls: 'on' };
    if (this.isPending(r)) return { text: 'pending', cls: 'wait', title: 'Not on the map yet: it’s added when the build Mac next publishes the map data' };
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
      const sw = h('input', { type: 'checkbox', class: 'switch rg-keep', title: 'Keep on this Mac, for when it’s away from the NAS: its files are copied first and never let go of' });
      sw.addEventListener('change', () => void this.setKept(r, sw));
      const sz = h('span', { class: 'rg-size' });
      this.keepEls.set(r.id, { sw, size: sz });
      row.replaceChildren(
        h('div', { class: 'rg1' }, name, h('span', { class: 'rg-id faint' }, r.id),
          ...(state ? [h('span', { class: `rg-state ${state.cls}`, title: state.title }, state.text)] : []),
          h('button', { class: 'rg-act', type: 'button', title: 'Rename', onclick: () => this.rename(r, row, show) }, '✎'),
          h('button', { class: 'rg-act', type: 'button', title: 'Remove…', onclick: () => this.askRemove(r, row, show) }, '×'),
          sw),
        h('div', { class: 'rg2' }, h('span', { class: 'rg2s', title: summary }, summary), sz),
      );
      this.showKept(r.id);
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

  private rename(r: Region, row: HTMLElement, back: () => void) {
    const input = h('input', { type: 'text', class: 'rg-in', value: r.name, spellcheck: false });
    const save = async () => {
      const name = input.value.trim();
      if (!name || name === r.name) return back();
      try {
        const res = await editRegion(r.id, { name });
        this.say(res.queued ? QUEUED : `Renamed “${r.name}” to “${name}” (nothing to rebuild)`);
        this.changed();
      } catch (e) {
        this.say(says(e), true);
        back();
      }
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

  private askRemove(r: Region, row: HTMLElement, back: () => void) {
    const go = async () => {
      try {
        const res = await removeRegion(r.id);
        this.say(res.queued ? QUEUED : `Removed “${r.name}”. The build Mac takes what only it covered off the map with its next build; the status bar shows progress.`);
        this.changed();
      } catch (e) {
        this.say(says(e), true);
        back();
      }
    };
    row.replaceChildren(h('div', { class: 'rg-ask' },
      h('div', {}, `Remove “${r.name}”? Its roads leave the map once the build Mac has rebuilt without it.`),
      h('div', { class: 'pills' },
        h('button', { class: 'pill on', type: 'button', onclick: () => void go() }, 'Remove'),
        h('button', { class: 'pill', type: 'button', onclick: back }, 'Cancel'))));
  }

  /** A change saved: the list and coverage again, and the status bar. */
  private changed() {
    this.onChanged();
    void this.refresh();
  }

  private say(text: string, warn = false) {
    this.msg.textContent = text;
    this.msg.classList.toggle('warn', warn);
  }

  // ---- on this Mac ------------------------------------------------------------------

  /** The mirror's state again, then every 2 s while something kept is being copied, else every
   * 10 s, while the section is open. */
  private async pollKeep() {
    clearTimeout(this.keepTimer);
    const tok = ++this.keepTok;
    try {
      this.keep = await keepStatus();
      this.keepErr = '';
    } catch (e) {
      if (tok !== this.keepTok) return;
      this.keepErr = says(e);
    }
    if (tok !== this.keepTok) return;
    this.renderKeep();
    // Often while something kept is coming (a copy under way, or kept files missing with the NAS
    // there, the build Mac idle and the room for them).
    const k = this.keep, kept = k?.kept;
    const coming = !!k?.copying || (!!kept && kept.areas > 0 && kept.here < kept.bytes && k!.online && !k!.busy && kept.more === 0);
    if (this.open) this.keepTimer = window.setTimeout(() => void this.pollKeep(), coming ? 2000 : 10_000);
  }

  /** A region's size, and how much of it is here or its state when kept; its switch. */
  private showKept(id: string) {
    const els = this.keepEls.get(id);
    if (!els) return;
    const k = this.keep;
    const rk: RegionKeep | undefined = k?.regions[id];
    els.sw.hidden = !k?.mirror;
    if (!els.sw.disabled) els.sw.checked = !!rk?.kept;
    let text = '', cls = 'rg-size';
    if (!k || !k.mirror) text = '';
    else if (!rk) text = 'not built yet';
    else if (rk.kept && rk.state === 'missing') text = 'kept once it’s built';
    else if (rk.kept) {
      text = `${size(rk.bytes)}; ${stateText(rk, rk.state)}`;
      cls += rk.state === 'kept' ? ' ok' : rk.state === 'room' ? ' warn' : ' on';
    } else text = rk.here >= rk.bytes && rk.bytes > 0 ? `${size(rk.bytes)}; all here` : `${size(rk.bytes)}; ${size(rk.here)} here`;
    els.size.className = cls;
    els.size.textContent = text;
    els.size.title = rk ? `Its files: ${size(rk.bytes)}, of which ${size(rk.here)} on this Mac (besides the basemap and the files every Mac keeps)` : '';
  }

  /** The block's lines: this Mac's copy of the map, what's kept, what's going on; the views. */
  private renderKeep() {
    const k = this.keep;
    for (const id of this.keepEls.keys()) this.showKept(id);
    if (!k) {
      this.keepSum.replaceChildren(this.keepErr ? h('div', { class: 'kp-line warn' }, this.keepErr) : h('div', { class: 'kp-line faint' }, h('span', { class: 'spin' }), ' Asking the map server…'));
      this.keepBtn.hidden = true;
      return;
    }
    this.keepBtn.hidden = !k.mirror;
    if (!k.mirror) {
      this.keepSum.replaceChildren(h('div', { class: 'kp-line faint' }, 'This server keeps no copy of the map (it runs without a mirror)'));
      this.renderViews([]);
      return;
    }
    const lines: HTMLElement[] = [];
    const cat = k.catalog ?? { bytes: 0, here: 0 };
    lines.push(h('div', { class: 'kp-line', title: 'The map’s files on this Mac, the disk’s free space, and the free space the mirror leaves (it lets files go to keep it)' },
      'This Mac: ', h('b', {}, size(cat.here)), ` of the map here (of ${size(cat.bytes)}) · `, h('b', {}, size(k.free ?? 0)), ` free · reserve ${size(k.reserve ?? 0)}`));
    const kept = k.kept;
    if (kept && kept.areas > 0) {
      const all = kept.here >= kept.bytes;
      const doing = all ? 'all here' : kept.more > 0 ? `waiting for room · ${pct(kept)} %` : !k.online ? `${pct(kept)} % here · away from the NAS` : k.busy ? `${pct(kept)} % here · paused while the build Mac works` : `copying ${pct(kept)} %`;
      lines.push(h('div', { class: 'kp-line', title: 'Kept areas, with the basemap and the files every Mac keeps: copied first, never let go of' },
        `Kept · ${kept.areas} area${kept.areas === 1 ? '' : 's'} · ${size(kept.bytes)} · `, h('span', { class: all ? 'ok' : kept.more > 0 ? 'warn' : 'on' }, doing)));
      if (!all) {
        const bar = h('div', { class: 'kp-bar' }, h('i', {}));
        (bar.firstChild as HTMLElement).style.width = `${(100 * kept.here) / Math.max(1, kept.bytes)}%`;
        lines.push(bar);
      }
    }
    // Room: what the kept areas still lack, else how far the disk is under the reserve with
    // everything that may go gone (what's kept stays).
    const lacking = !!kept && kept.areas > 0 && kept.here < kept.bytes && kept.more > 0;
    const short = k.last?.short ?? 0;
    if (lacking) lines.push(h('div', { class: 'kp-line warn' }, `Kept areas need ${size(kept!.more)} more room: free some space on this Mac, or keep less.`));
    else if (short > 0) lines.push(h('div', { class: 'kp-line warn' }, `The disk is ${size(short)} short of the reserve with every file that may go gone${kept && kept.areas > 0 ? ' (what’s kept stays)' : ''}: nothing more is copied.`));
    // What's going on, unless the Kept line says it.
    const keptSays = !!kept && kept.areas > 0 && kept.here < kept.bytes;
    if (k.copying) {
      const c = k.copying;
      lines.push(h('div', { class: 'kp-line faint', title: c.file }, `Copying ${c.file.replace(/^layers\//, '')} · ${size(c.have)} of ${size(c.bytes)}`));
    } else if (!keptSays && !k.online) lines.push(h('div', { class: 'kp-line faint' }, 'Away from the NAS: the map shows what this Mac has.'));
    else if (!keptSays && k.busy) lines.push(h('div', { class: 'kp-line faint' }, 'Copying waits while the build Mac runs a job.'));
    const every = k.essentials, bm = k.basemap;
    if (every && bm) lines.push(h('div', { class: 'kp-line faint', title: 'The worldwide files, the zoomed-out packs (zooms 0–8), the landmark points and the area details: kept on every Mac, whatever its room' },
      `Every Mac keeps ${size(every.bytes)} the map needs anywhere; a kept area adds the basemap (${size(bm.bytes)}).`));
    // Kept regions there's no recipe for any more (removed since): let go of from here.
    if (this.loadedAt > -Infinity) {
      for (const [id, rk] of Object.entries(k.regions)) {
        if (!rk.kept || this.regions.some((r) => r.id === id)) continue;
        lines.push(h('div', { class: 'kp-line warn' }, `“${rk.name}” is kept, but isn’t a region any more `,
          h('button', { class: 'pill', type: 'button', title: 'Keep it no more', onclick: () => void keepRegion(id, false).then(() => this.pollKeep(), (e) => this.sayKeep(says(e), true)) }, 'Let go')));
      }
    }
    this.keepSum.replaceChildren(...lines);
    this.renderViews(k.views);
  }

  /** The kept views: listed again when they change, else their lines updated in place. */
  private renderViews(views: KeptView[]) {
    const key = JSON.stringify(views.map((v) => [v.id, v.name]));
    if (key !== this.viewsKey) {
      this.viewsKey = key;
      this.viewEls.clear();
      this.keepViews.replaceChildren(...views.map((v) => this.viewRow(v)));
    }
    for (const v of views) {
      const el = this.viewEls.get(v.id);
      if (!el) continue;
      el.textContent = `${size(v.bytes)}; ${stateText(v, v.state)}`;
      el.className = `rg-size ${v.state === 'kept' ? 'ok' : v.state === 'room' ? 'warn' : 'on'}`;
    }
  }

  private viewRow(v: KeptView): HTMLElement {
    const row = h('div', { class: 'rg' });
    const ring = v.outline;
    const box = (): Bbox => [Math.min(...ring.map((p) => p[0])), Math.min(...ring.map((p) => p[1])), Math.max(...ring.map((p) => p[0])), Math.max(...ring.map((p) => p[1]))];
    const feature: GeoJSON.Feature = { type: 'Feature', properties: {}, geometry: { type: 'Polygon', coordinates: [[...ring, ring[0]]] } };
    const show = () => {
      const name = h('span', { class: 'rg-name', title: `${v.name}: show it` }, v.name);
      name.addEventListener('click', () => this.onFit(box()));
      const sz = h('span', { class: 'rg-size' });
      this.viewEls.set(v.id, sz);
      row.replaceChildren(
        h('div', { class: 'rg1' }, name, h('span', { class: 'rg-id faint' }, 'view'),
          h('button', { class: 'rg-act', type: 'button', title: 'Rename', onclick: () => this.renameKept(v, row, show) }, '✎'),
          h('button', { class: 'rg-act', type: 'button', title: 'Keep it no more', onclick: () => void this.dropKept(v) }, '×')),
        h('div', { class: 'rg2' }, h('span', { class: 'rg2s faint' }, `kept ${new Date(v.at * 1000).toLocaleDateString('en-CA', { month: 'short', day: 'numeric' })}`), sz),
      );
      sz.textContent = `${size(v.bytes)}; ${stateText(v, v.state)}`;
    };
    show();
    row.addEventListener('mouseenter', () => this.layers.setHover([feature]));
    row.addEventListener('mouseleave', () => this.layers.setHover(null));
    return row;
  }

  private sayKeep(text: string, warn = false) {
    this.keepMsg.textContent = text;
    this.keepMsg.classList.toggle('warn', warn);
  }

  private async setKept(r: Region, sw: HTMLInputElement) {
    const on = sw.checked;
    sw.disabled = true;
    try {
      await keepRegion(r.id, on);
      this.sayKeep(on ? `Keeping “${r.name}” on this Mac: its files come first, and stay.` : `“${r.name}” isn’t kept any more: its files may go when the mirror needs room.`);
    } catch (e) {
      sw.checked = !on;
      this.sayKeep(says(e), true);
    } finally {
      sw.disabled = false;
    }
    void this.pollKeep();
  }

  private async keepThisView() {
    const outline = this.viewOutline();
    if (outline.length < 3) return this.sayKeep('No ground in view to keep', true);
    this.keepBtn.disabled = true;
    this.keepBtn.textContent = 'Keeping…';
    try {
      const v = await keepView(outline);
      this.sayKeep(`Keeping “${v.name}” on this Mac (✎ renames it).`);
    } catch (e) {
      this.sayKeep(says(e), true);
    } finally {
      this.keepBtn.disabled = false;
      this.keepBtn.textContent = 'Keep this view';
    }
    void this.pollKeep();
  }

  private renameKept(v: KeptView, row: HTMLElement, back: () => void) {
    const input = h('input', { type: 'text', class: 'rg-in', value: v.name, spellcheck: false });
    const save = async () => {
      const name = input.value.trim();
      if (!name || name === v.name) return back();
      try {
        await renameView(v.id, name);
        this.sayKeep(`Renamed “${v.name}” to “${name}”`);
      } catch (e) {
        this.sayKeep(says(e), true);
        back();
      }
      void this.pollKeep();
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

  private async dropKept(v: KeptView) {
    try {
      await dropView(v.id);
      this.layers.setHover(null);
      this.sayKeep(`“${v.name}” isn’t kept any more: its files may go when the mirror needs room.`);
    } catch (e) {
      this.sayKeep(says(e), true);
    }
    void this.pollKeep();
  }

  // ---- finding areas ----------------------------------------------------------------

  private async runSearch() {
    const q = this.search.value.trim();
    this.searchAbort?.abort();
    if (!q) return this.showFound([], '');
    const ctrl = (this.searchAbort = new AbortController());
    this.hint.replaceChildren(h('span', { class: 'spin' }), ' Searching…');
    try {
      const areas = await searchAreas(q, ctrl.signal);
      this.showFound(areas, areas.length ? '' : `No area named “${q}…”`);
    } catch (e) {
      if ((e as Error).name !== 'AbortError') this.showFound([], says(e));
    }
  }

  private setPicking(on: boolean) {
    if (on === this.picking) return;
    this.picking = on;
    this.pickBtn.classList.toggle('on', on);
    this.pickBtn.textContent = on ? 'Click the map…' : 'Pick on map';
    if (on) this.hint.textContent = PICK_HINT;
    else if (this.hint.textContent === PICK_HINT) this.hint.textContent = '';
    this.onPicking(on);
  }

  /** Stops waiting for a click on the map (Esc). */
  stopPicking() {
    this.setPicking(false);
  }

  /** A click on the map while picking: the areas around the point, smallest first. */
  async pickAt(ll: [number, number]) {
    this.setPicking(false);
    this.search.value = '';
    this.searchAbort?.abort();
    const at = fmt.coord(ll[1], ll[0]).replace(/(\d+\.\d{2})\d+/g, '$1');
    this.hint.replaceChildren(h('span', { class: 'spin' }), ` Finding the areas at ${at}…`);
    try {
      const areas = await areasAt(ll[0], ll[1]);
      this.showFound(areas, areas.length ? `Around ${at}, smallest first` : `No area outlines at ${at}`);
    } catch (e) {
      this.showFound([], says(e));
    }
  }

  private showFound(areas: Area[], note: string) {
    this.found = areas;
    this.active = -1;
    this.hint.textContent = note;
    this.preview(null);
    this.renderCands();
  }

  private renderCands() {
    // Which region each area is in already, if any.
    const inRegion = new Map<number, string>();
    for (const r of this.regions) for (const e of r.outline) if (e.startsWith('osm:')) inRegion.set(Number(e.slice(4)), r.name);
    this.cands.replaceChildren(...this.found.map((a, i) => {
      const chosen = this.draft.some((d) => d.id === a.id);
      const also = inRegion.get(a.id);
      const row = h('div', { class: `cand${chosen ? ' in' : ''}${i === this.active ? ' active' : ''}`, title: chosen ? 'Click to take it out of the new region' : 'Click to add it to the new region' },
        h('span', { class: 'cn' }, areaName(a)),
        h('span', { class: 'ck' }, km2(a.km2)),
        h('span', { class: 'cs' }, [a.en && a.en !== a.name ? a.name : '', levelName(a), a.in_name || a.iso, also ? `in “${also}”` : ''].filter(Boolean).join(' · ')));
      row.addEventListener('mouseenter', () => this.preview(a));
      row.addEventListener('click', () => this.toggle(a));
      return row;
    }));
  }

  /** An area's outline on the map (none: null). */
  private preview(a: Area | null) {
    const tok = ++this.hoverTok;
    if (!a) return this.layers.setHover(null);
    areaOutline(a.id).then((f) => tok === this.hoverTok && this.layers.setHover([f]), () => {});
  }

  /** The arrow keys go through the areas listed (each shown and framed), Enter adds or takes out
   * the one picked, Esc clears the search. */
  private keys(e: KeyboardEvent) {
    const n = this.found.length;
    if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && n) {
      e.preventDefault();
      this.active = this.active < 0 ? (e.key === 'ArrowDown' ? 0 : n - 1) : (this.active + (e.key === 'ArrowDown' ? 1 : -1) + n) % n;
      const a = this.found[this.active];
      this.renderCands();
      this.cands.children[this.active]?.scrollIntoView({ block: 'nearest' });
      this.preview(a);
      this.onFit(a.bbox);
    } else if (e.key === 'Enter') {
      const a = this.found[this.active] ?? (n === 1 ? this.found[0] : undefined);
      if (a) this.toggle(a);
    } else if (e.key === 'Escape') {
      e.stopPropagation();
      if (this.search.value) {
        this.search.value = '';
        this.showFound([], '');
      } else this.search.blur();
    }
  }

  // ---- a new region -----------------------------------------------------------------

  /** An area into the new region (the view framed on it all), or out of it. */
  private toggle(a: Area) {
    const i = this.draft.findIndex((d) => d.id === a.id);
    if (i >= 0) this.draft.splice(i, 1);
    else {
      this.draft.push(a);
      this.onFit(union(this.draft.map((d) => d.bbox)));
      this.msg.textContent = '';
    }
    this.showDraft();
    this.renderCands();
  }

  private clearDraft() {
    this.draft = [];
    this.idTyped = this.nameTyped = false;
    this.showDraft();
    this.renderCands();
  }

  /** The new region's block (its id and name following its first area unless typed), and its
   * outline on the map. */
  private showDraft() {
    const d = this.draft;
    this.draftBox.hidden = !d.length;
    const tok = ++this.draftTok;
    if (!d.length) {
      this.idTyped = this.nameTyped = false;
      return this.layers.setDraft([]);
    }
    const first = d[0];
    if (!this.nameTyped) this.nameIn.value = areaName(first);
    if (!this.idTyped) this.idIn.value = this.freeId(slug(first.en || first.name) || `area-${first.id}`);
    this.draftHd.textContent = d.length === 1 ? '1 area' : `${d.length} areas, their union`;
    this.chips.replaceChildren(...d.map((a) =>
      h('span', { class: 'rg-chip', title: `${a.name} · ${levelName(a)} · ${km2(a.km2)}` }, areaName(a),
        h('button', { type: 'button', title: 'Take it out', onclick: () => this.toggle(a) }, '×'))));
    if (!this.open) return;
    void Promise.all(d.map((a) => areaOutline(a.id).catch(() => null))).then((fs) => {
      if (tok === this.draftTok) this.layers.setDraft(fs.filter((f): f is Outline => !!f));
    });
  }

  /** `id`, or with a number after it when a region has it already. */
  private freeId(id: string): string {
    const taken = new Set(this.regions.map((r) => r.id));
    if (!taken.has(id)) return id;
    let k = 2;
    while (taken.has(`${id}-${k}`)) k++;
    return `${id}-${k}`;
  }

  private async save() {
    const id = this.idIn.value.trim(), name = this.nameIn.value.trim();
    if (!this.draft.length) return;
    if (!validId(id)) return this.say('The id is lower-case letters, digits and dashes (northumberland, kanto-2)', true);
    if (!name) return this.say('The region needs a name', true);
    if (this.regions.some((r) => r.id === id)) return this.say(`There is a region ${id} already: choose another id`, true);
    this.saveBtn.disabled = true;
    try {
      const res = await addRegion({ id, name, outline: this.draft.map((a) => `osm:${a.id}`) });
      this.say(res.queued ? QUEUED : `Added “${name}”. The build Mac builds it when it can; the status bar shows progress.`);
      this.clearDraft();
      this.changed();
    } catch (e) {
      this.say(says(e), true);
    } finally {
      this.saveBtn.disabled = false;
    }
  }
}
