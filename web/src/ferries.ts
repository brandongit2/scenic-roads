// Passenger ferries on the map: data (fetched on first use), styling from the "Ferries" state,
// hover summaries, click popups and km in view per service group.
import * as maplibregl from 'maplibre-gl';
import { inPolygon } from './overlays';
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap } from 'maplibre-gl';
import { ver } from './api';
import { hostFor } from './hosts';
import { tasks } from './tasks';
import { fitPopup } from './popupfit';
import { legibleCss } from './linecolour';
import { withEnglish } from './english';
import {
  FERRY_GROUP_COLOURS, FERRY_GROUPS, NFERRY, ferryColourExpr, ferryColourOf, ferryMetricDef, ferryOpacityExpr, fmtDuration, fmtPerDay, freqText,
  lineTitle, operatorColour, type FerryLine,
} from './ferry';
import type { FeatureSummary } from './overlays';
import { distFromSamples, type Dist } from './roads/stats';
import { passes } from './ui/scale';
import { labelShown, lineWeight, type AppState, type FerryState } from './state';
import { cap, fmt, h } from './ui/dom';

const LINE = 'ferry-line';
const LABELS = ['ferry-label', 'ferry-terminal-label'];
const TERMINALS = 'ferry-terminal';
/** A terminal takes the colour of a line within this many pixels of it. */
const TERMINAL_PX = 6;
/** Lines as far as this from a terminal count (TERMINAL_PX at zoom 4, where terminals appear). */
const NEAR_M = 30000;

/** Per feature, its line's bounding box (west, south, east, north; not a line: empty). */
function lineBoxes(fs: Feature[]): Float64Array {
  const out = new Float64Array(fs.length * 4).fill(NaN);
  fs.forEach((f, i) => {
    if (f.geometry.type !== 'LineString') return;
    let w = Infinity, s = Infinity, e = -Infinity, n = -Infinity;
    for (const [x, y] of (f.geometry as GeoJSON.LineString).coordinates) {
      if (x < w) w = x;
      if (x > e) e = x;
      if (y < s) s = y;
      if (y > n) n = y;
    }
    out.set([w, s, e, n], i * 4);
  });
  return out;
}

/** Per terminal (Point feature, by index), the lines (LineString features, by index) within
 * NEAR_M of it, nearest first: [line index, metres]. Segments bucketed on a grid of NEAR_M. A
 * generator (yields every few terminals: idle.ts runs it between frames), returning the map. */
function* nearLines(fs: Feature[]): Generator<void, Map<number, [number, number][]>> {
  const M = 111320; // metres per degree of latitude
  const cell = NEAR_M / M; // degrees
  const grid = new Map<string, [number, number, number, number, number][]>();
  fs.forEach((f, li) => {
    if (f.geometry.type !== 'LineString') return;
    const c = (f.geometry as GeoJSON.LineString).coordinates;
    for (let k = 0; k + 1 < c.length; k++) {
      const [x0, y0] = c[k], [x1, y1] = c[k + 1];
      for (let gy = Math.floor(Math.min(y0, y1) / cell); gy <= Math.floor(Math.max(y0, y1) / cell); gy++)
        for (let gx = Math.floor(Math.min(x0, x1) / cell); gx <= Math.floor(Math.max(x0, x1) / cell); gx++) {
          const key = `${gx}/${gy}`;
          let b = grid.get(key);
          if (!b) grid.set(key, (b = []));
          b.push([li, x0, y0, x1, y1]);
        }
    }
  });
  const out = new Map<number, [number, number][]>();
  yield;
  for (let ti = 0; ti < fs.length; ti++) {
    const f = fs[ti];
    if (ti % 64 === 63) yield;
    if (f.geometry.type !== 'Point') continue;
    const [px, py] = (f.geometry as GeoJSON.Point).coordinates;
    const kx = M * Math.cos((py * Math.PI) / 180);
    const best = new Map<number, number>();
    const gx0 = Math.floor(px / cell), gy0 = Math.floor(py / cell);
    const rx = Math.ceil(NEAR_M / Math.max(1, kx) / cell);
    for (let gy = gy0 - 1; gy <= gy0 + 1; gy++)
      for (let gx = gx0 - rx; gx <= gx0 + rx; gx++)
        for (const [li, x0, y0, x1, y1] of grid.get(`${gx}/${gy}`) ?? []) {
          // Point to segment, in metres on the local plane.
          const ax = (x0 - px) * kx, ay = (y0 - py) * M, bx = (x1 - px) * kx, by = (y1 - py) * M;
          const dx = bx - ax, dy = by - ay, l2 = dx * dx + dy * dy;
          const t = l2 > 0 ? Math.max(0, Math.min(1, -(ax * dx + ay * dy) / l2)) : 0;
          const m = Math.hypot(ax + t * dx, ay + t * dy);
          if (m <= NEAR_M && m < (best.get(li) ?? Infinity)) best.set(li, m);
        }
    out.set(ti, [...best.entries()].sort((a, b) => a[1] - b[1]));
  }
  return out;
}

type Feature = GeoJSON.Feature<GeoJSON.Geometry, Record<string, any>>;

export interface FerryCoverage {
  lines: number;
  known: number;
}

export class Ferries {
  private fc: GeoJSON.FeatureCollection<GeoJSON.Geometry, Record<string, any>> | null = null;
  lines: Record<string, FerryLine> = {};
  private loading: Promise<void> | null = null;
  private popup: maplibregl.Popup | null = null;
  private style: FerryState | null = null;
  /** The metric colouring's range in use (auto-fitted or fixed) and equalisation lookup. */
  private range: [number, number] = [0, 1];
  private cdf: Uint8Array | null = null;
  onLoaded: () => void = () => {};

  constructor(private map: MLMap) {}

  get loaded() {
    return this.fc !== null;
  }

  private ensure() {
    if (this.loading) return;
    tasks.begin('ferries', 'Ferries', 'downloading the lines and timetables');
    this.loading = Promise.all([
      fetch(`${hostFor('layers')}/api/layer/ferries${ver('ferries.json')}`).then((r) => (r.ok ? r.json() : null)),
      fetch(`${hostFor('layers')}/api/layer/ferry-lines${ver('ferry-lines.json')}`).then((r) => (r.ok ? r.json() : null)),
    ])
      .then(([fc, lines]) => {
        tasks.end('ferries');
        if (!fc) return;
        for (const f of fc.features as Feature[]) {
          const p = f.properties;
          if (f.geometry.type !== 'LineString') continue;
          p.oc = legibleCss(p.col) ?? operatorColour(p.op);
          p.gs = String(p.gs ?? '') || digits(p.gb);
          p.km = lengthKm((f.geometry as GeoJSON.LineString).coordinates);
        }
        this.fc = fc;
        this.boxes = null;
        this.lines = lines ?? {};
        this.map.getSource<GeoJSONSource>('ferries')?.setData(fc);
        this.onLoaded();
      })
      .catch(() => tasks.end('ferries'));
  }

  apply(s: AppState) {
    const map = this.map;
    const f = s.ferry;
    this.style = f;
    if (f.on) this.ensure();
    if (!map.getLayer(LINE)) return;
    const vis = (id: string, on: boolean) => map.getLayer(id) && map.setLayoutProperty(id, 'visibility', on ? 'visible' : 'none');
    vis(LINE, f.on);
    vis(TERMINALS, f.on);
    for (const id of LABELS) vis(id, f.on && labelShown(s, 'ferries'));
    const groups: ExpressionSpecification = ['any', ...f.groups.flatMap((on, i) => (on ? [['in', String(i), ['get', 'gs']] as ExpressionSpecification] : [])), false];
    // Sailings-a-day filter (f < 0: no daily count known).
    const known: ExpressionSpecification = ['all', ['>=', ['get', 'f'], 0], ...(f.freqMin > 0 ? [['>=', ['get', 'f'], f.freqMin] as ExpressionSpecification] : []),
      ...(f.freqMax > 0 ? [['<=', ['get', 'f'], f.freqMax] as ExpressionSpecification] : [])];
    const freq: ExpressionSpecification = f.freqOn ? ['any', known, f.freqUnknown ? ['<', ['get', 'f'], 0] : false] : ['literal', true] as unknown as ExpressionSpecification;
    map.setFilter(LINE, ['all', ['==', ['geometry-type'], 'LineString'], groups, freq]);
    map.setFilter('ferry-label', ['all', ['==', ['geometry-type'], 'LineString'], ['!=', ['get', 'n'], ''], groups, freq]);
    this.paint();
    const o = f.opacity;
    // (shown once coloured: recolourTerminals)
    const shown = (v: number): ExpressionSpecification => ['case', ['boolean', ['feature-state', 'k'], false], v, 0];
    map.setPaintProperty(TERMINALS, 'circle-opacity', shown(Math.min(1, o + 0.05)));
    map.setPaintProperty(TERMINALS, 'circle-stroke-opacity', shown(Math.min(1, o + 0.05)));
    const w = lineWeight(s, 'ferries');
    map.setPaintProperty(LINE, 'line-width', ['interpolate', ['linear'], ['zoom'], 3, 0.6 * w, 7, 1.1 * w, 11, 1.8 * w, 15, 3 * w, 18, 4.5 * w]);
    map.setPaintProperty(TERMINALS, 'circle-radius', ['interpolate', ['linear'], ['zoom'], 4, 1 * w, 9, 1.8 * w, 14, 3.5 * w]);
    map.setPaintProperty(LINE, 'line-dasharray', f.dashed ? ['literal', [2.5, 1.6]] : ['literal', [1, 0]]);
  }

  /** Per terminal (its index in the data, its feature id): the ferry lines within NEAR_M of it,
   * nearest first, as [line index, metres]. Found once. */
  private nearLines: Map<number, [number, number][]> | null = null;
  /** Colour set per terminal (null: ferry blue). */
  private terminalColours = new Map<number, string | null>();

  /** Each terminal takes the colour of the nearest ferry line shown within TERMINAL_PX of it, else
   * ferry blue; from the data, not the rendered features (on the 3D globe their queries ray-march
   * the terrain). Only terminals whose colour changed are set (`all`: every one). A generator: it
   * yields every few terminals (idle.ts runs it between frames). */
  *recolourTerminals(all = false): Generator<void, void> {
    const map = this.map, st = this.style, fc = this.fc;
    if (!st || !fc || !map.getLayer(TERMINALS) || map.getLayoutProperty(TERMINALS, 'visibility') === 'none') return;
    this.nearLines ??= yield* nearLines(fc.features as Feature[]);
    if (all) this.terminalColours.clear();
    const shown = (p: Record<string, any>) => {
      const v = Number(p.f);
      return st.groups.some((on, i) => on && String(p.gs).includes(String(i)))
        && (!st.freqOn || (v < 0 ? st.freqUnknown : (!(st.freqMin > 0) || v >= st.freqMin) && (!(st.freqMax > 0) || v <= st.freqMax)));
    };
    const pxM = 40075016.686 / (512 * 2 ** map.getZoom());
    let k = 0;
    for (const [ti, near] of this.nearLines) {
      if (++k % 256 === 0) yield;
      const lat = ((fc.features[ti].geometry as GeoJSON.Point).coordinates[1] * Math.PI) / 180;
      const maxM = TERMINAL_PX * pxM * Math.cos(lat);
      let c: string | null = null;
      for (const [li, m] of near) {
        if (m > maxM) break;
        const p = (fc.features[li] as Feature).properties;
        if (!shown(p)) continue;
        c = ferryColourOf(p, st, this.range, this.cdf);
        break;
      }
      if (this.terminalColours.has(ti) && this.terminalColours.get(ti) === c) continue;
      this.terminalColours.set(ti, c);
      map.setFeatureState({ source: 'ferries', id: ti }, { c, k: true });
    }
  }

  /** Colour and opacity (after a style change, or a new scale range or lookup). */
  private paint() {
    const f = this.style;
    if (!f || !this.map.getLayer(LINE)) return;
    this.map.setPaintProperty(LINE, 'line-color', ferryColourExpr(f, this.range, this.cdf));
    this.map.setPaintProperty(LINE, 'line-opacity', ferryOpacityExpr(f, this.range, this.cdf, f.opacity));
  }

  /** The metric scale in use: range (auto-fitted or fixed) and equalisation lookup. */
  setScale(range: [number, number], cdf: Uint8Array | null) {
    const same = range[0] === this.range[0] && range[1] === this.range[1] && cdf === this.cdf;
    this.range = range;
    this.cdf = cdf;
    if (!same && this.style?.colour === 'freq') this.paint();
  }

  /** The metric over the ferry lines in view (shown by the filters), weighted by length. */
  metricDist(): Dist | null {
    const st = this.style;
    if (!this.fc || !st) return null;
    const d = ferryMetricDef(st.metric);
    const b = this.map.getBounds();
    const box = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()] as const;
    const pass = (v: number) => !st.freqOn || (v < 0 ? st.freqUnknown : (!(st.freqMin > 0) || v >= st.freqMin) && (!(st.freqMax > 0) || v <= st.freqMax));
    const vs: number[] = [], ws: number[] = [];
    const fs = this.fc.features as Feature[];
    this.boxes ??= lineBoxes(fs);
    for (let fi = 0; fi < fs.length; fi++) {
      const f = fs[fi], p = f.properties;
      if (f.geometry.type !== 'LineString' || !pass(Number(p.f)) || !st.groups.some((on, i) => on && String(p.gs).includes(String(i)))) continue;
      const v = d.value(p);
      if (Number.isNaN(v)) continue;
      const km = this.kmInView(fi, box);
      if (km <= 0) continue;
      vs.push(v);
      ws.push(km);
    }
    return distFromSamples(Float32Array.from(vs), Float32Array.from(ws), d.domain[0], d.domain[1]);
  }

  /** Km of a line (feature index) within the bounds (west, south, east, north): its segments
   * clipped to them, so that the auto-fit's screen widths measure what the view shows (a long
   * crossing reaching into the view counted whole before). */
  private kmInView(fi: number, [w, s, e, n]: readonly [number, number, number, number]): number {
    const bx = this.boxes!;
    if (bx[fi * 4] > e || bx[fi * 4 + 2] < w || bx[fi * 4 + 1] > n || bx[fi * 4 + 3] < s) return 0;
    const c = ((this.fc!.features as Feature[])[fi].geometry as GeoJSON.LineString).coordinates;
    let km = 0;
    for (let i = 1; i < c.length; i++) {
      const [x0, y0] = c[i - 1], [x1, y1] = c[i];
      const dx = x1 - x0, dy = y1 - y0;
      // Liang–Barsky: the share of the segment inside the box.
      let t0 = 0, t1 = 1, out = false;
      for (const [pp, q] of [[-dx, x0 - w], [dx, e - x0], [-dy, y0 - s], [dy, n - y0]]) {
        if (pp === 0) {
          if (q < 0) out = true;
          continue;
        }
        const r = q / pp;
        if (pp < 0) t0 = Math.max(t0, r);
        else t1 = Math.min(t1, r);
      }
      if (out || t1 <= t0) continue;
      const my = y0 + (dy * (t0 + t1)) / 2;
      km += (t1 - t0) * Math.hypot(dx * 111.32 * Math.cos((my * Math.PI) / 180), dy * 110.57);
    }
    return km;
  }

  /** The ferry lines in view (the groups shown, whatever the frequency filter) by sailings a day,
   * log10, weighted by length: the frequency filter's histogram. */
  freqDist(): Dist | null {
    const st = this.style;
    if (!this.fc || !st) return null;
    const b = this.map.getBounds();
    const box = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()] as const;
    const vs: number[] = [], ws: number[] = [];
    const fs = this.fc.features as Feature[];
    this.boxes ??= lineBoxes(fs);
    for (let fi = 0; fi < fs.length; fi++) {
      const f = fs[fi], p = f.properties;
      const v = Number(p.f);
      if (f.geometry.type !== 'LineString' || !(v > 0) || !st.groups.some((on, i) => on && String(p.gs).includes(String(i)))) continue;
      const km = this.kmInView(fi, box);
      if (km <= 0) continue;
      vs.push(Math.log10(v));
      ws.push(km);
    }
    return distFromSamples(Float32Array.from(vs), Float32Array.from(ws), Math.log10(0.1), Math.log10(500), 256);
  }

  /** Km of ferry route in view per service group (primary group of each way), and frequency
   * coverage of the lines in view. */
  inView(): { km: number[]; cov: FerryCoverage } {
    const km = new Array(NFERRY).fill(0);
    const cov = { lines: 0, known: 0 };
    if (!this.fc) return { km, cov };
    const b = this.map.getBounds();
    const [w, s, e, n] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
    const seen = new Set<string>();
    const st = this.style;
    const pass = (v: number) => !st?.freqOn || (v < 0 ? st.freqUnknown : (!(st.freqMin > 0) || v >= st.freqMin) && (!(st.freqMax > 0) || v <= st.freqMax));
    for (const f of this.fc.features as Feature[]) {
      if (f.geometry.type !== 'LineString' || !pass(Number(f.properties.f))) continue;
      const c = (f.geometry as GeoJSON.LineString).coordinates;
      if (!c.some(([x, y]) => x >= w && x <= e && y >= s && y <= n)) continue;
      km[f.properties.g] += f.properties.km;
      for (const id of String(f.properties.lines).split(',')) {
        if (seen.has(id)) continue;
        seen.add(id);
        cov.lines++;
        if (this.lines[id]?.freq?.per_day !== undefined) cov.known++;
      }
    }
    return { km, cov };
  }

  /** Outline of the ground in view (lng, lat), as the "in view" lists use. */
  viewOutline: (() => [number, number][]) | null = null;

  /** Per feature, the bounding box of a line (west, south, east, north), for skipping lines out of
   * view; found once. */
  private boxes: Float64Array | null = null;

  /** Ferry routes in view (after the filters): count, seasonal ones, the busiest crossing. */
  viewSummary(): { routes: number; seasonal: number; busiest: { name: string; perDay: number; lngLat: [number, number] } | null } {
    const out = { routes: 0, seasonal: 0, busiest: null as { name: string; perDay: number; lngLat: [number, number] } | null };
    if (!this.fc) return out;
    const poly = this.viewOutline?.() ?? [];
    const b = this.map.getBounds();
    let [w, s, e, n] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
    const test = poly.length >= 3 ? inPolygon(poly) : (x: number, y: number) => x >= w && x <= e && y >= s && y <= n;
    if (poly.length >= 3) for (const [x, y] of poly) [w, s, e, n] = [Math.min(w, x), Math.min(s, y), Math.max(e, x), Math.max(n, y)];
    const fs = this.fc.features as Feature[];
    this.boxes ??= lineBoxes(fs);
    const bx = this.boxes;
    const seen = new Set<string>();
    for (let fi = 0; fi < fs.length; fi++) {
      const f = fs[fi];
      if (f.geometry.type !== 'LineString' || bx[fi * 4] > e || bx[fi * 4 + 2] < w || bx[fi * 4 + 1] > n || bx[fi * 4 + 3] < s) continue;
      const c = (f.geometry as GeoJSON.LineString).coordinates;
      const inside = c.filter(([x, y]) => test(x, y));
      if (!inside.length) continue;
      const p = f.properties;
      const key = String(p.lines ?? p.n ?? '');
      if (!seen.has(key)) {
        seen.add(key);
        out.routes++;
        if (Number(p.m) >= 0 && Number(p.m) < 12) out.seasonal++;
      }
      const perDay = Number(p.f);
      if (perDay > (out.busiest?.perDay ?? 0)) {
        const mid = inside[inside.length >> 1];
        out.busiest = { name: String(p.n || 'Ferry'), perDay, lngLat: [mid[0], mid[1]] };
      }
    }
    return out;
  }

  private at(pt: { x: number; y: number }) {
    const map = this.map;
    if (!map.getLayer(LINE) || map.getLayoutProperty(LINE, 'visibility') === 'none') return null;
    const fs = map.queryRenderedFeatures([[pt.x - 4, pt.y - 4], [pt.x + 4, pt.y + 4]], { layers: [LINE] });
    // With the highlight on, dimmed lines can't be hovered or clicked (as roads).
    const st = this.style;
    if (st?.colour === 'freq' && st.threshold.on) {
      const d = ferryMetricDef(st.metric);
      return fs.find((f) => {
        const v = d.value(f.properties ?? {});
        return Number.isNaN(v) || passes(v, st.threshold, this.range);
      }) ?? null;
    }
    return fs[0] ?? null;
  }

  /** The ferry under the cursor, for the bottom bar. */
  hoverAt(pt: { x: number; y: number }): FeatureSummary | null {
    const f = this.at(pt);
    if (!f) return null;
    const p = f.properties ?? {};
    const ls = String(p.lines ?? '').split(',').map((id) => this.lines[id]).filter(Boolean);
    if (!ls.length) return null;
    const l = ls[0];
    const st = this.style;
    const facts: string[] = [];
    if (l.duration) facts.push(fmtDuration(l.duration));
    if (ls.length > 1) {
      facts.push(`${ls.length} lines`);
      if (Number(p.f) >= 0) facts.push(`${fmtPerDay(Number(p.f))} each way${Number(p.fp) ? '+' : ''}`);
    } else {
      const ft = freqText(l);
      facts.push(ft ?? 'frequency unknown');
    }
    if (l.seasonText) facts.push(l.seasonText);
    if (l.operator || l.network) facts.push(l.operator || l.network);
    const route = l.from && l.to ? `${l.from} → ${l.to}${l.via ? ` via ${l.via.replace(/;/g, ', ')}` : ''}` : '';
    const src = l.freq?.source ? `Sailings: ${l.freq.source}${l.freq.checked ? ` (${l.freq.checked})` : ''}` : 'Sailings: no timetable found yet';
    return {
      title: cap(withEnglish(lineTitle(l), null, l.en)) + (ls.length > 1 ? ` +${ls.length - 1}` : ''),
      kind: FERRY_GROUPS[l.group].one + (l.vehicles ? ' · cars & foot passengers' : ' · foot passengers'),
      colour: st ? ferryColourOf(p, st, this.range, this.cdf) : FERRY_GROUP_COLOURS[p.g],
      facts,
      source: '',
      desc: [route, `${fmt.n(l.km)} km`, src].filter(Boolean).join(' · '),
      area: false,
    };
  }

  click(pt: { x: number; y: number }, at: maplibregl.LngLat): boolean {
    const f = this.at(pt);
    if (!f) return false;
    const ls = String(f.properties?.lines ?? '').split(',').map((id) => [id, this.lines[id]] as const).filter(([, l]) => l);
    if (!ls.length) return false;
    const body = h('div', { class: 'pop' });
    const kv = (k: string, v: unknown) => (v === undefined || v === null || v === '' ? null : h('div', { class: 'kv' }, h('span', {}, k), h('b', {}, String(v))));
    const link = (url: string | undefined, label: string) => (url ? h('a', { href: url, target: '_blank', rel: 'noopener' }, `${label} ↗`) : null);
    ls.forEach(([, l], i) => {
      const dot = h('span', { class: 'dot' });
      dot.style.background = legibleCss(l.colour) ?? FERRY_GROUP_COLOURS[l.group];
      const items = [
        h('div', { class: 'ttl' }, cap(withEnglish(lineTitle(l), null, l.en)), l.ref && l.name && !l.name.includes(l.ref) ? h('span', { class: 'faint' }, ` ${l.ref}`) : ''),
        h('div', { class: 'sub' }, dot, FERRY_GROUPS[l.group].label, h('span', { class: 'faint' }, l.vehicles ? ' · cars & foot passengers' : ' · foot passengers')),
        kv('Route', l.from && l.to ? `${l.from} → ${l.to}` : null),
        kv('Via', l.via ? l.via.replace(/;/g, ', ') : null),
        kv('Operator', l.operator || l.network),
        kv('Crossing', l.duration ? fmtDuration(l.duration) : null),
        kv('Length', `${fmt.n(l.km)} km`),
        kv('Sailings', freqText(l) ?? 'unknown'),
        kv('Season', l.freq?.months ?? (l.seasonText || null)),
        kv('Overnight', l.freq?.overnight ? 'yes' : null),
        kv('Bicycles', l.bicycle === 'yes' ? 'allowed' : l.bicycle === 'no' ? 'not allowed' : null),
        h('div', { class: 'links' }, link(l.website, 'Operator'), link(l.freq?.url, 'Timetable source'), link(`https://www.openstreetmap.org/${l.osm[0]}`, 'OSM')),
        h('div', { class: 'src' }, `Sailings: ${l.freq?.source ?? 'no timetable found yet'}${l.freq?.checked ? ` · checked ${l.freq.checked}` : ''} · Route: OpenStreetMap`),
      ];
      if (i > 0) body.append(h('hr'));
      body.append(...(items.filter((x) => x !== null) as Node[]));
    });
    this.popup?.remove();
    const popup = new maplibregl.Popup({ closeButton: true, maxWidth: '320px', className: 'dark-pop', offset: 8 });
    const fit = fitPopup(this.map, popup, body);
    this.popup = popup.setLngLat(at).setDOMContent(fit.el).addTo(this.map);
    fit.fit();
    return true;
  }

  closePopup() {
    this.popup?.remove();
    this.popup = null;
  }
}

function digits(gb: number): string {
  let s = '';
  for (let i = 0; i < NFERRY; i++) if (gb & (1 << i)) s += String(i);
  return s;
}

function lengthKm(c: GeoJSON.Position[]): number {
  let d = 0;
  for (let i = 1; i < c.length; i++) {
    const [x0, y0] = c[i - 1], [x1, y1] = c[i];
    const k = Math.cos((((y0 + y1) / 2) * Math.PI) / 180);
    d += Math.hypot((x1 - x0) * k, y1 - y0) * 111.195;
  }
  return d;
}
