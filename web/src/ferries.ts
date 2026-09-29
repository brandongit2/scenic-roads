// Passenger ferries on the map: data (fetched on first use), styling from the "Ferries" state,
// hover summaries, click popups and km in view per service group.
import * as maplibregl from 'maplibre-gl';
import { inPolygon } from './overlays';
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap } from 'maplibre-gl';
import { ver } from './api';
import {
  FERRY_GROUP_COLOURS, FERRY_GROUPS, NFERRY, ferryColourExpr, ferryColourOf, ferryMetricDef, ferryOpacityExpr, fmtDuration, fmtPerDay, freqText,
  lineTitle, operatorColour, type FerryLine,
} from './ferry';
import type { FeatureSummary } from './overlays';
import { distFromSamples, type Dist } from './roads/stats';
import { passes } from './ui/scale';
import { defaults, labelShown, type AppState, type FerryState } from './state';
import { cap, fmt, h } from './ui/dom';

const LINE = 'ferry-line';
const LABELS = ['ferry-label', 'ferry-terminal-label'];
const TERMINALS = 'ferry-terminal';

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
  onBusy: (label: string | null) => void = () => {};

  constructor(private map: MLMap) {}

  get loaded() {
    return this.fc !== null;
  }

  private ensure() {
    if (this.loading) return;
    this.onBusy('Loading ferries…');
    this.loading = Promise.all([
      fetch(`/api/layer/ferries${ver('ferries.json')}`).then((r) => (r.ok ? r.json() : null)),
      fetch(`/api/layer/ferry-lines${ver('ferry-lines.json')}`).then((r) => (r.ok ? r.json() : null)),
    ])
      .then(([fc, lines]) => {
        this.onBusy(null);
        if (!fc) return;
        for (const f of fc.features as Feature[]) {
          const p = f.properties;
          if (f.geometry.type !== 'LineString') continue;
          p.oc = p.col || operatorColour(p.op);
          p.gs = String(p.gs ?? '') || digits(p.gb);
          p.km = lengthKm((f.geometry as GeoJSON.LineString).coordinates);
        }
        this.fc = fc;
        this.lines = lines ?? {};
        this.map.getSource<GeoJSONSource>('ferries')?.setData(fc);
        this.onLoaded();
      })
      .catch(() => this.onBusy(null));
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
    map.setPaintProperty(TERMINALS, 'circle-opacity', Math.min(1, o + 0.05));
    map.setPaintProperty(TERMINALS, 'circle-stroke-opacity', Math.min(1, o + 0.05));
    // Map → Line weight scales ferries too (relative to its default), on top of the card's own.
    const w = f.weight * (s.weight / defaults.weight);
    map.setPaintProperty(LINE, 'line-width', ['interpolate', ['linear'], ['zoom'], 3, 0.6 * w, 7, 1.1 * w, 11, 1.8 * w, 15, 3 * w, 18, 4.5 * w]);
    map.setPaintProperty(LINE, 'line-dasharray', f.dashed ? ['literal', [2.5, 1.6]] : ['literal', [1, 0]]);
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
    const [w, s, e, n] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
    const pass = (v: number) => !st.freqOn || (v < 0 ? st.freqUnknown : (!(st.freqMin > 0) || v >= st.freqMin) && (!(st.freqMax > 0) || v <= st.freqMax));
    const vs: number[] = [], ws: number[] = [];
    for (const f of this.fc.features as Feature[]) {
      const p = f.properties;
      if (f.geometry.type !== 'LineString' || !pass(Number(p.f)) || !st.groups.some((on, i) => on && String(p.gs).includes(String(i)))) continue;
      const c = (f.geometry as GeoJSON.LineString).coordinates;
      if (!c.some(([x, y]) => x >= w && x <= e && y >= s && y <= n)) continue;
      const v = d.value(p);
      if (Number.isNaN(v)) continue;
      vs.push(v);
      ws.push(Math.max(0.01, p.km));
    }
    return distFromSamples(Float32Array.from(vs), Float32Array.from(ws), d.domain[0], d.domain[1]);
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

  /** Ferry routes in view (after the filters): count, seasonal ones, the busiest crossing. */
  viewSummary(): { routes: number; seasonal: number; busiest: { name: string; perDay: number; lngLat: [number, number] } | null } {
    const out = { routes: 0, seasonal: 0, busiest: null as { name: string; perDay: number; lngLat: [number, number] } | null };
    if (!this.fc) return out;
    const poly = this.viewOutline?.() ?? [];
    const b = this.map.getBounds();
    const [w, s, e, n] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
    const test = poly.length >= 3 ? inPolygon(poly) : (x: number, y: number) => x >= w && x <= e && y >= s && y <= n;
    const seen = new Set<string>();
    for (const f of this.fc.features as Feature[]) {
      if (f.geometry.type !== 'LineString') continue;
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
      title: cap(lineTitle(l)) + (ls.length > 1 ? ` +${ls.length - 1}` : ''),
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
      dot.style.background = l.colour || FERRY_GROUP_COLOURS[l.group];
      const items = [
        h('div', { class: 'ttl' }, cap(lineTitle(l)), l.ref && l.name && !l.name.includes(l.ref) ? h('span', { class: 'faint' }, ` ${l.ref}`) : ''),
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
    this.popup = new maplibregl.Popup({ closeButton: true, maxWidth: '320px', className: 'dark-pop', offset: 8 }).setLngLat(at).setDOMContent(body).addTo(this.map);
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
