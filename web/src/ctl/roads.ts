// The roads' and rail's controller: their two layers (roads/layer.ts; rail is a RoadLayer drawn
// as railways), the styles the settings give them, rail's service frequencies, the In view
// statistics over both and their colour ranges, which auto-fit to the lines in view and ease
// there in the frame loop (ctl/frame.ts calls `tick`).
import type { Map as MLMap } from 'maplibre-gl';
import { FIT_LEN_DEFAULT, fitByPct, fitByWidths, pairOf, spread, unitOf } from '../autofit';
import { getWay, keepable, ver, version, type Meta } from '../api';
import * as cam3d from '../camera3d';
import { CASING_CLASSES_MASK, DIM_GREY, FADES, FADE_Z, RAIL0, RAIL_GROUPS, interp, setWidths } from '../config';
import { MAP_SCHEMES, lookUniforms, presetLook, type RoadMapLook } from '../mapschemes';
import { lineName } from '../names';
import { hexRgb, paletteRgb } from '../palettes';
import { RAIL_GROUP_COLOURS, railMetricDef, railMetricOf } from '../rail';
import { RoadLayer, type HoverInfo, type RoadStyle, type SchemeUniforms } from '../roads/layer';
import { distFromSamples, viewStatsGen, type Dist, type ViewStats } from '../roads/stats';
import { isScenic, modeDef } from '../scenic';
import { ROAD_WEIGHT, classMask, defaults, groupMask, lineWeight, railMask, roadLenM, surfaceMask, tollMask, unnamedHideClasses, unnamedHideGroups, type AppState, type Store } from '../state';
import { tasks, type Task } from '../tasks';
import { cdfOf, fadeAlpha, passes, scaleU } from '../ui/scale';
import type { InViewExtra } from '../ui/stats';
import type { FramePart } from './frame';

export interface RoadsHost {
  /** The frame loop to run (statistics to compute, a range with a new target). */
  wake: () => void;
  /** The view's width on the ground at its centre (km): the scales' fit by screen widths. */
  widthKm: () => number;
  /** The In view summary's other layers, each while shown: ferries, landmarks, the summit. */
  others: () => Pick<InViewExtra, 'ferry' | 'landmarks'> & { summit: NonNullable<InViewExtra['terrain']>['summit'] };
}

/** Time a statistics pass may take a frame (ms): at rest, and while the camera moves (the frame's
 * own work then fills most of its 8 ms). */
const STATS_MS = 3;
const STATS_MOVING_MS = 1;

const hex = (c: number) => `#${c.toString(16).padStart(6, '0')}`;
const cssOfRgb = (c: number[]) => `rgb(${c.map((v) => Math.round(v * 255)).join(',')})`;

/** The road layer's share of the settings' road look (AppState.roadLook; the width table aside:
 * config.ts setWidths). Rail keeps the defaults'. */
function roadLookStyle(s: AppState): Pick<RoadStyle, 'minZoom' | 'casingFrom' | 'casingWidth' | 'tunnelFade' | 'bridgeCasing' | 'dashUnpaved'> {
  const l = s.roadLook;
  return { minZoom: [...l.minZoom], casingFrom: l.casingFrom, casingWidth: l.casingWidth, tunnelFade: l.tunnel, bridgeCasing: l.bridgeCasing, dashUnpaved: l.dashUnpaved };
}

export class RoadsRail implements FramePart {
  readonly roads: RoadLayer;
  readonly rails: RoadLayer;
  /** The roads' colour range as drawn, easing to its target. */
  cur: [number, number];
  /** The roads' equalisation lookup (from the metric's distribution in view), when on. */
  cdf: Uint8Array | null = null;
  private cdfKey = '';
  /** Rail's colour range as drawn, and its equalisation lookup. */
  railCur: [number, number];
  railCdf: Uint8Array | null = null;
  private railCdfKey = '';
  stats: ViewStats | null = null;
  railStats: ViewStats | null = null;
  /** Distribution of the rail colour metric over rail in view. */
  railDist: Dist | null = null;
  /** Rail in view by trains a day (log10), for the frequency filter's histogram. */
  railFreqDist: Dist | null = null;
  /** Distribution of the current colour metric over roads in view. */
  mdist: Dist | null = null;
  viewExtra: InViewExtra = {};
  /** The busiest lines' names as shown (by way). */
  private railName = new Map<number, string>();
  private statsDirty = true;
  private lastStats = 0;
  /** The statistics pass under way (computeStats), run a few ms a frame, and whether it is a full
   * one (dropped when the camera moves: the view it was for is gone). */
  private statsJob: Generator<void, void> | null = null;
  private statsJobFull = false;
  private lastFullStats = 0;
  /** The Map look's colours (Map display type), rebuilt when it changes. */
  private lookOf: RoadMapLook | null = null;
  private lookU: ReturnType<typeof lookUniforms> | null = null;
  private railScheme: SchemeUniforms;
  /** The rail frequencies' file version loaded or loading (null: none, or a failed request). */
  private railFreqFor: string | null = null;

  constructor(private map: MLMap, private store: Store, meta: Meta, private host: RoadsHost) {
    const s0 = store.s;
    setWidths(s0.roadLook.widths);
    this.cur = [...s0.range] as [number, number];
    this.railCur = [...s0.rail.range] as [number, number];
    const roads = (this.roads = new RoadLayer({
      freqFilter: { on: false, min: 0, max: 0, unknown: true },
      mode: s0.mode,
      palette: s0.palette,
      range: [...s0.range] as [number, number],
      classMask: classMask(s0),
      surfaceMask: surfaceMask(s0),
      tollMask: tollMask(s0),
      unnamedHide: unnamedHideClasses(s0),
      weight: ROAD_WEIGHT * lineWeight(s0, 'roads'),
      ties: 0,
      threshold: { ...s0.threshold },
      visible: s0.layers.roads,
      weights: [...s0.weights],
      equalize: s0.equalize,
      routeGlow: s0.routeGlow,
      terrain3d: s0.terrain.on,
      exaggeration: s0.terrain.exaggeration,
      lowFade: s0.lowFade,
      lowSpan: s0.lowSpan,
      occlude: s0.occlude,
      opacity: s0.roadOpacity,
      lenMin: roadLenM(s0)[0],
      lenMax: roadLenM(s0)[1],
      direct: 0,
      scheme: null,
      single: [1, 1, 1],
      modeId: null,
      railMask: 0,
      railWeights: [],
      casingMask: CASING_CLASSES_MASK,
      land: hexRgb(s0.land),
      ...roadLookStyle(s0),
      persp: { ...s0.roadPersp },
    }));
    // The tiles' version in their URLs (cached for good): the catalogue's for the layer, else none. (Not
    // the build time in meta: it stayed when the tiles' way column changed from indices to OSM ids,
    // and a browser holding the older tiles under the same URLs would have kept them.)
    roads.setSource(version('roads.tiles'), meta.bounds);
    this.applyMapMode(s0);

    // Passenger rail: its service groups in their colours, cased dark.
    this.railScheme = (() => {
      const u = lookUniforms(presetLook(MAP_SCHEMES.find((m) => m.key === 'mono')!));
      RAIL_GROUP_COLOURS.forEach((c, k) => {
        u.classCol.set([...hexRgb(c), 1], (RAIL0 + k) * 4);
        u.classCas.set(hexRgb('#07090c'), (RAIL0 + k) * 3);
      });
      return u;
    })();
    const rails = (this.rails = new RoadLayer(
      // Drawn as railways (a thin line with cross-ties: the layer's rail pattern), cased when zoomed in.
      { ...roads.style, ...roadLookStyle(defaults), minZoom: [], classMask: 0x7c00, casingMask: 0x7c00, surfaceMask: 3, tollMask: 3, unnamedHide: 0, threshold: { on: false, dir: 'above', value: 0 }, routeGlow: false, lenMin: 0, lenMax: Infinity, equalize: false, opacity: 1 },
      {
        id: 'rails',
        rail: true,
        metric: (_st, e, g, ch, ground, style, fq) =>
          railMetricOf(store.s.rail.metric, { elev: e, grade: g, ground, bridge: (style & 96) === 32, tunnel: (style & 64) !== 0, ch, freq: fq }, store.s.rail.weights),
      },
    ));
    rails.setSource(version('rails.tiles'), meta.bounds);
    // The tilted tile cover unprojects onto the terrain (not the camera pivot's level).
    roads.groundSamples = rails.groundSamples = (pts) => cam3d.coverSamples(map, pts);
    this.applyRailStyle(s0);
    if (s0.rail.on) this.loadRailFreq();
    rails.onChange = () => this.markDirty();
  }

  /** Street-map colours (Map display type): the Map look's; its classes cased (the other display
   * types: tertiary and up). */
  private applyMapMode(s: AppState) {
    const st = this.roads.style;
    const on = s.mode === 'map';
    if (on && this.lookOf !== s.roadMap) {
      this.lookOf = s.roadMap;
      this.lookU = lookUniforms(s.roadMap);
    }
    st.direct = on ? 1 : 0;
    st.scheme = on ? this.lookU : null;
    st.casingMask = on && this.lookU ? this.lookU.casedMask : CASING_CLASSES_MASK;
  }

  private applyRailStyle(s: AppState) {
    const r = s.rail, st: RoadStyle = this.rails.style;
    st.visible = r.on;
    st.railMask = railMask(s);
    st.direct = r.colour === 'line' ? 2 : r.colour === 'service' ? 3 : r.colour === 'single' ? 4 : 0;
    st.scheme = this.railScheme;
    st.single = hexRgb(r.single);
    st.modeId = railMetricDef(r.metric).id;
    st.palette = r.palette;
    st.railWeights = [...r.weights];
    st.freqFilter = { on: r.freqOn, min: r.freqMin, max: r.freqMax, unknown: r.freqUnknown };
    st.weight = lineWeight(s, 'rail');
    st.ties = r.ties;
    st.opacity = r.opacity;
    const metric = r.colour === 'metric';
    st.lowFade = metric ? r.lowFade : 0;
    st.lowSpan = r.lowSpan;
    st.equalize = metric && r.equalize;
    st.threshold = metric ? { ...r.threshold } : { on: false, dir: 'above', value: 0 };
    st.terrain3d = s.terrain.on;
    st.exaggeration = s.terrain.exaggeration;
    st.occlude = s.occlude;
    st.range = this.railCur;
  }

  /** Rail service frequency per way (railfreq): sorted OSM way ids (as the rail tiles' way column)
   * and trains a day each way. Loaded for a version of the file (again for a new catalogue's); a
   * failed request isn't kept (asked again the next time rail changes). */
  private loadRailFreq() {
    const v = version('rail-freq.bin');
    if (this.railFreqFor === v) return;
    this.railFreqFor = v;
    const req = fetch(`/api/railfreq${ver('rail-freq.bin')}`).then(keepable).then((r) => (r.ok ? r.arrayBuffer() : null));
    tasks.track('railfreq', 'Rail frequencies', req, 'trains a day per line')
      .then((b) => {
        if (this.railFreqFor !== v || !b || b.byteLength < 8) return;
        const n = b.byteLength / 8;
        const dv = new DataView(b);
        // Negative: a lower bound (MTR lines with only published headways).
        const ways = new Uint32Array(n), vals = new Float32Array(n);
        for (let i = 0; i < n; i++) {
          ways[i] = dv.getUint32(i * 8, true);
          vals[i] = dv.getFloat32(i * 8 + 4, true);
        }
        const find = (w: number) => {
          let lo = 0, hi = n - 1;
          while (lo <= hi) {
            const m = (lo + hi) >> 1;
            if (ways[m] < w) lo = m + 1;
            else if (ways[m] > w) hi = m - 1;
            else return m;
          }
          return -1;
        };
        this.rails.setLineValues((w) => {
          const i = find(w);
          return i < 0 ? -1 : Math.abs(vals[i]);
        }, (w) => {
          const i = find(w);
          return i >= 0 && vals[i] < 0;
        });
      })
      .catch(() => {
        if (this.railFreqFor === v) this.railFreqFor = null;
      });
  }

  /** A new version of the rail frequencies' file: loaded again, if they had been. */
  railFreqChanged() {
    if (this.railFreqFor !== null && this.store.s.rail.on) this.loadRailFreq();
  }

  /** The settings changed: the layers' styles, and what follows from what changed. */
  apply(s: AppState, ch: Set<keyof AppState>) {
    const st = this.roads.style;
    st.mode = s.mode;
    st.palette = s.palette;
    st.classMask = classMask(s);
    st.surfaceMask = surfaceMask(s);
    st.tollMask = tollMask(s);
    st.unnamedHide = unnamedHideClasses(s);
    st.weight = ROAD_WEIGHT * lineWeight(s, 'roads');
    st.persp = this.rails.style.persp = { ...s.roadPersp };
    st.threshold = { ...s.threshold };
    st.visible = s.layers.roads;
    st.weights = [...s.weights];
    st.equalize = s.equalize;
    st.routeGlow = s.routeGlow;
    st.terrain3d = s.terrain.on;
    st.exaggeration = s.terrain.exaggeration;
    st.lowFade = s.lowFade;
    st.lowSpan = s.lowSpan;
    st.occlude = s.occlude;
    st.opacity = s.roadOpacity;
    Object.assign(st, roadLookStyle(s));
    setWidths(s.roadLook.widths);
    [st.lenMin, st.lenMax] = roadLenM(s);
    this.applyMapMode(s);
    this.applyRailStyle(s);
    if (ch.has('rail')) {
      this.markDirty();
      if (s.rail.on) this.loadRailFreq();
      this.map.triggerRepaint();
    }
    if (ch.has('groups') || ch.has('unnamed') || ch.has('roadLen') || ch.has('roadLenOn') || ch.has('surface') || ch.has('toll') || ch.has('layers') || ch.has('mode') || ch.has('weights')) this.markDirty();
    if (ch.has('equalize') || ch.has('mode')) this.cdfKey = '';
  }

  /** A new catalogue: its data's bounds (where tiles are asked for). */
  setBounds(bounds: Meta['bounds']) {
    this.roads.setSource(version('roads.tiles'), bounds);
    this.rails.setSource(version('rails.tiles'), bounds);
  }

  /** The tiles still loading, for the status line (rail while shown). */
  loading(): Task[] {
    const out: Task[] = [];
    for (const [layer, label] of [[this.roads, 'Roads'], [this.rails, 'Rail']] as const) {
      if (layer === this.rails && !this.store.s.rail.on) continue;
      const p = layer.progress();
      if (p.loaded < p.wanted) out.push({ label, done: p.loaded, total: p.wanted, detail: 'tiles (downloaded, decoded and uploaded)' });
    }
    return out;
  }

  /** The best fitLen[0] screen widths of line in view to the best fitLen[1] (the roads' scenic
   * metrics, rail's ranked ones): a fixed amount of line at the view centre's scale, whatever
   * share of the view it is. `dist`: the metric over the lines in view; `km`: their length. */
  private byLen(dist: Dist, km: number, fitLen: [number, number], step: number): [number, number] {
    return fitByWidths(dist, km, this.host.widthKm(), fitLen, step);
  }

  private target(): [number, number] {
    const s = this.store.s;
    const d = modeDef(s.mode);
    const { stats, mdist } = this;
    if (s.mode === 'relief') {
      const e = stats?.elev;
      return e ? spread(e.quantile(0), e.quantile(1), 10) : this.cur;
    }
    if (!s.auto) return s.range;
    if (isScenic(s.mode) && unitOf(s.fitUnits, s.mode) === 'widths') return mdist && mdist.total > 0 && stats && stats.totalKm > 0 ? this.byLen(mdist, stats.totalKm, pairOf(s.fitLens, s.mode, FIT_LEN_DEFAULT), d.step) : this.cur;
    return mdist && mdist.total > 0 ? fitByPct(mdist, s.fit, d.step) : this.cur;
  }

  /**
   * The In view statistics; `full`: everything, else (while the camera moves) only what the colour
   * ranges follow: the current metric's distribution, from fewer samples. The rest (km, the other
   * metrics, landmarks, summit, ferries, busiest line) stays from the last settled view. A tile at a
   * time (a generator): the frame loop runs it a few milliseconds a frame (at 120 Hz a frame has
   * 8 ms, and a full pass over a dense view took 40 ms), and the results take effect once it ends.
   */
  private *computeStats(full = true): Generator<void, void> {
    const s = this.store.s;
    const { roads, rails } = this;
    const nSamples = full ? 90_000 : 15_000;
    const d = modeDef(s.mode);
    const fromSketches = s.mode === 'elev' || s.mode === 'relief' || s.mode === 'grade';
    let st = this.stats, md = this.mdist, extra = this.viewExtra, rst = this.railStats, rdist = this.railDist, rfd = this.railFreqDist;
    if (full || fromSketches || !st) {
      const lr = roadLenM(s);
      st = yield* viewStatsGen(roads, groupMask(s), classMask(s), surfaceMask(s), unnamedHideGroups(s), lr[0] > 0 || lr[1] < Infinity ? lr : null, tollMask(s));
    }
    if (fromSketches) {
      md = s.mode === 'grade' ? st!.grade : st!.elev;
    } else if (!full) {
      const smp = yield* roads.metricSamplesGen([s.mode], s.weights, nSamples);
      md = distFromSamples(smp.v[0], smp.w, ...d.domain);
    }
    let sc: Dist | null = null, vi: Dist | null = null;
    if (full) {
      // Scenic summary for the stats card + the current metric's distribution, in one pass.
      const modes = ['score', 'openness', 'trees', 'vista', 'bldg'] as const;
      const scenicMode = d.id >= 3 && !(modes as readonly string[]).includes(s.mode) ? [s.mode] : [];
      const smp = yield* roads.metricSamplesGen([...modes, ...scenicMode], s.weights, nSamples);
      const dists = modes.map((m, i) => distFromSamples(smp.v[i], smp.w, ...modeDef(m).domain));
      if (!fromSketches) {
        const i = (modes as readonly string[]).indexOf(s.mode);
        md = i >= 0 ? dists[i] : distFromSamples(smp.v[modes.length], smp.w, ...d.domain);
      }
      [sc, , , vi] = dists;
    }
    // (The summary reads the roads' and rail's statistics of the pass before.)
    if (full) extra = this.inViewExtra(sc, vi);
    // Rail: km per service group, and the colour metric's distribution.
    const r = s.rail;
    if (full || !rst) rst = r.on ? yield* viewStatsGen(rails, 31, 0x7c00, 3, 0, null) : null;
    if (r.on && r.colour === 'metric') {
      const rd = railMetricDef(r.metric);
      const smpR = yield* rails.sampleWithGen([(e, g, ch, ground, style, fq) =>
        railMetricOf(r.metric, { elev: e, grade: g, ground, bridge: (style & 96) === 32, tunnel: (style & 64) !== 0, ch, freq: fq }, r.weights)], nSamples);
      // Lines without a timetable have no frequency (NaN): left out of the distribution.
      const keep = smpR.v[0].map((v) => (Number.isNaN(v) ? 0 : 1));
      const vv = smpR.v[0].filter((_, i) => keep[i]), ww = smpR.w.filter((_, i) => keep[i]);
      rdist = distFromSamples(vv, ww, ...rd.domain);
    } else rdist = null;
    if (r.on && (full || !rfd)) {
      // Lines without a timetable (NaN or none) are left out.
      const smpF = yield* rails.sampleWithGen([(_e, _g, _ch, _ground, _style, fq) => (fq > 0 ? Math.log10(fq) : NaN)], nSamples);
      const keep = smpF.v[0].map((v) => (Number.isNaN(v) ? 0 : 1));
      rfd = distFromSamples(smpF.v[0].filter((_, i) => keep[i]), smpF.w.filter((_, i) => keep[i]), Math.log10(0.5), Math.log10(3000), 256);
    } else if (!r.on) rfd = null;
    this.stats = st;
    this.mdist = md;
    this.viewExtra = extra;
    this.railStats = rst;
    this.railDist = rdist;
    this.railFreqDist = rfd;
  }

  /** The In view summary beyond roads: rail and ferries, landmarks and terrain (each while shown).
   * `sc`, `vi`: the scenic score's and vista's distributions over the roads in view. */
  private inViewExtra(sc: Dist | null, vi: Dist | null): InViewExtra {
    const s = this.store.s, st = this.stats;
    const x: InViewExtra = {
      scenic: sc && sc.total > 0 ? [sc.quantile(0.5), sc.quantile(0.9)] : null,
      vista: vi && vi.total > 0 ? [vi.quantile(0.5), vi.quantile(0.9)] : null,
    };
    const railStats = this.railStats;
    if (s.rail.on && railStats) {
      const groups = RAIL_GROUPS.map((g, k) => ({ label: g.label, colour: RAIL_GROUP_COLOURS[k], km: railStats.classKm[RAIL0 + k] }));
      const total = groups.reduce((a, g) => a + g.km, 0);
      const b = this.rails.busiestInView();
      x.rail = { total, groups, busiest: b ? { name: this.railName.get(b.way) ?? 'Rail line', perDay: b.perDay, lngLat: b.lngLat } : null, highest: railStats.highest };
      if (b && !this.railName.has(b.way)) getWay(b.way, b.lngLat).then((info) => {
        // The line's name without a route's direction or service codes ("Highland Sleeper").
        this.railName.set(b.way, (info && lineName(info)) || 'Rail line');
        this.markDirty();
      });
    }
    const o = this.host.others();
    if (o.ferry !== undefined) x.ferry = o.ferry;
    if (o.landmarks !== undefined) x.landmarks = o.landmarks;
    const hi = Math.max(o.summit?.ele ?? -Infinity, st?.highest?.elev ?? -Infinity);
    const lo = st?.lowest?.elev;
    x.terrain = {
      summit: o.summit,
      relief: Number.isFinite(hi) && lo !== undefined ? hi - lo : null,
      above1000: st?.elev ? st.elev.above(1000) : null,
    };
    return x;
  }

  /** The statistics need recomputing (and the loop wakes to do it). */
  markDirty() {
    this.statsDirty = true;
    this.host.wake();
  }

  get due() {
    return this.statsDirty || !!this.statsJob;
  }

  /** A frame: the statistics (while the camera moves, the cheap ones only, and less often;
   * everything once settled: a pass runs STATS_MS a frame, STATS_MOVING_MS while moving, until it
   * ends), and both colour ranges eased by `k` toward their targets. */
  tick(now: number, moving: boolean, k: number): { panels: boolean; easing: boolean } {
    let panels = false, easing = false;
    if (!this.statsJob && this.statsDirty && now - this.lastStats > (moving ? 400 : 150)) {
      this.statsDirty = false;
      // While moving, the cheap pass (what the colour ranges follow), and every 2.5 s a full one:
      // the In view summary follows the view too, a little behind.
      this.statsJobFull = !moving || now - this.lastFullStats > 2500;
      if (this.statsJobFull) this.lastFullStats = now;
      this.statsJob = this.computeStats(this.statsJobFull);
    }
    if (this.statsJob) {
      const t0 = performance.now();
      const budget = moving ? STATS_MOVING_MS : STATS_MS;
      while (performance.now() - t0 < budget) {
        if (this.statsJob.next().done) {
          this.statsJob = null;
          this.lastStats = now;
          panels = true;
          break;
        }
      }
    }
    const t = this.target(), cur = this.cur;
    const next: [number, number] = [cur[0] + (t[0] - cur[0]) * k, cur[1] + (t[1] - cur[1]) * k];
    if (Math.abs(next[0] - cur[0]) + Math.abs(next[1] - cur[1]) > modeDef(this.store.s.mode).step * 0.01) {
      this.cur = next;
      this.roads.style.range = next;
      this.map.triggerRepaint();
      panels = true;
      easing = true;
    }
    const r = this.store.s.rail, rc = this.railCur, rdist = this.railDist, rst = this.railStats;
    const rd = railMetricDef(r.metric);
    const tt: [number, number] = !r.auto ? r.range
      : !rdist || rdist.total <= 0 ? rc
      : rd.byLen && unitOf(r.fitUnits, r.metric) === 'widths' ? (rst && rst.totalKm > 0 ? this.byLen(rdist, rst.totalKm, pairOf(r.fitLens, r.metric, FIT_LEN_DEFAULT), rd.step) : rc)
      : fitByPct(rdist, r.fit, rd.step);
    const nr: [number, number] = [rc[0] + (tt[0] - rc[0]) * k, rc[1] + (tt[1] - rc[1]) * k];
    if (Math.abs(nr[0] - rc[0]) + Math.abs(nr[1] - rc[1]) > rd.step * 0.01) {
      this.railCur = nr;
      this.rails.style.range = nr;
      this.map.triggerRepaint();
      panels = true;
      easing = true;
    }
    return { panels, easing };
  }

  /** Both equalisation lookups again, for the ranges and distributions as they stand. */
  updateCdfs() {
    const s = this.store.s, cur = this.cur, mdist = this.mdist;
    if (!s.equalize || !mdist || mdist.total <= 0) {
      if (this.cdf) {
        this.cdf = null;
        this.roads.setCdf(null);
      }
    } else {
      const key = `${s.mode}|${cur[0].toFixed(3)}|${cur[1].toFixed(3)}|${mdist.total.toFixed(0)}`;
      if (key !== this.cdfKey) {
        this.cdfKey = key;
        const out = new Uint8Array(256);
        const a = 1 - mdist.above(cur[0]), b = 1 - mdist.above(cur[1]);
        const span = Math.max(1e-9, b - a);
        for (let i = 0; i < 256; i++) {
          const v = cur[0] + (i / 255) * (cur[1] - cur[0]);
          out[i] = Math.round(Math.max(0, Math.min(1, (1 - mdist.above(v) - a) / span)) * 255);
        }
        this.cdf = out;
        this.roads.setCdf(out);
      }
    }
    const r = s.rail, rc = this.railCur, rdist = this.railDist;
    if (!r.equalize || r.colour !== 'metric' || !rdist || rdist.total <= 0) {
      if (this.railCdf) {
        this.railCdf = null;
        this.railCdfKey = '';
        this.rails.setCdf(null);
      }
      return;
    }
    const key = `${r.metric}|${rc[0].toFixed(3)}|${rc[1].toFixed(3)}|${rdist.total.toFixed(0)}`;
    if (key === this.railCdfKey) return;
    this.railCdfKey = key;
    this.railCdf = cdfOf(rdist, rc);
    this.rails.setCdf(this.railCdf);
  }

  /** A CSS colour ('#rrggbb' or 'rgb(r,g,b)') mixed toward the land colour, `k` of it kept (as the
   * rail layer's u_bg). */
  private mixBg(css: string, k: number) {
    const c = /^#[0-9a-f]{6}$/i.test(css) ? hexRgb(css) : (css.match(/[\d.]+/g) ?? ['0', '0', '0']).slice(0, 3).map((v) => Number(v) / 255);
    const bg = hexRgb(this.store.s.land);
    return cssOfRgb(c.map((v, i) => bg[i] + (v - bg[i]) * k));
  }

  /** A rail line's colour and opacity at a point as the rail layer draws it (roads/layer.ts
   * endColour), for the stop dot there: its colour (by line, service, single or the metric's
   * palette, a highlight's misses grey) mixed toward the background by the class's colour strength
   * at this zoom (tunnels at 45 %, misses at most 85 %), and with a metric the scale's low-end fade
   * as its opacity. The dots took the bare colour, so on a tunnel or at the low end of the scale a
   * stop shone brighter than its line. */
  railLook(hv: HoverInfo): { c: string; a: number } {
    const r = this.store.s.rail;
    const cls = hv.style & 15;
    let col: string, a = 1;
    let fade = interp(FADE_Z, FADES[cls], this.map.getZoom()) * ((hv.style & 64) !== 0 ? 0.45 : 1);
    const lc = hv.tile.data!.lineColour[hv.hit.line];
    if (r.colour === 'line') col = lc ? hex(lc - 1) : RAIL_GROUP_COLOURS[cls - RAIL0];
    else if (r.colour === 'service') col = RAIL_GROUP_COLOURS[cls - RAIL0];
    else if (r.colour === 'single') col = r.single;
    else {
      const v = railMetricOf(r.metric, { elev: hv.elev, grade: hv.grade, ground: hv.ground, bridge: (hv.style & 96) === 32, tunnel: (hv.style & 64) !== 0, ch: hv.ch, freq: hv.fq }, r.weights);
      if (Number.isNaN(v)) col = '#5c6673';
      else {
        const u = scaleU(v, this.railCur, r.equalize ? this.railCdf : null);
        a = fadeAlpha(u, r.lowFade, r.lowSpan);
        if (passes(v, r.threshold, this.railCur)) col = paletteRgb(r.palette, u);
        else {
          col = cssOfRgb(DIM_GREY);
          fade = Math.min(fade, 0.85);
        }
      }
    }
    return { c: this.mixBg(col, fade), a };
  }
}
