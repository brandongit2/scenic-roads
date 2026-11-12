// What the polar caps draw past 85.05° (docs/polar.md §10; task #668): the labels, landmarks and
// roads there, from each pole's file (/api/layer/polar-<n|s>, landmarks::polar, names attached).
// MapLibre's symbol and dot layers work in Web Mercator tile coordinates and can't place a point
// past the tiles' edge, so these are drawn by the app, on the globe, over the map:
//
// - each name in its MapLibre label layer's look, evaluated for it (place-city … place-minor,
//   place-state, water-name, park-label, poi-<kind>-label: colour, halo, font, size, letter
//   spacing, case, wrap; the Labels → Style looks and Label size with them), drawn from the style's
//   own SDF glyphs as the layer's shader draws them (polarlabels.ts), shown while that layer is;
// - thinned as the label tiles are: a name shows once its isolation (`r`, the ground metres to the
//   nearest that matters more) spans its kind's label spacing on screen at its own place, an area's
//   once its √area (`w`) is big enough there, the most important first where they collide;
// - the landmarks as dots in their kind's colour under their names, the roads as thin lines.
//
// A custom layer gives the frame's matrix (the globe's, unit sphere to clip space); the points are
// drawn on a 2D canvas over the map's in the same frame, each on the ground as the cap draws it
// (its heights times the terrain's exaggeration: polarglobe.ts capGround), none past the horizon.

import type { CustomLayerInterface, CustomRenderMethodInput, Map as MlMap } from 'maplibre-gl';
import { keepable, onVersions, ver, version } from './api';
import { HALO, LABEL_TILE_LAYERS, POI_ICON_MIN, POI_ICON_R, POI_ICON_STROKE, POI_R } from './basemap';
import { hostFor } from './hosts';
import { EARTH_R } from './merc';
import { displayOf } from './names';
import { capGround } from './polarglobe';
import { iconPath } from './poiicons';
import { charsOf, nameShows, rasterize, type Glyph, type Rgba, type TextLine, type TextLook } from './polarlabels';
import { DEFAULT_DENSITY, kindSpacing, type DensityKind, type LabelDensity } from './state';

type Props = Record<string, unknown> & { n?: string; k?: string; c?: string; s?: number; r?: number; w?: number; ele?: number };
type Feature = { geometry: { type: 'Point' | 'LineString'; coordinates: number[] | number[][] }; properties: Props };

/** A landmark name's gap under its icon (CSS px). */
const NAME_GAP = 1;
/** A size (CSS px) at a zoom from stops (dots.ts's). */
const interpStops = (stops: [number, number][], z: number) => {
  if (z <= stops[0][0]) return stops[0][1];
  for (let i = 1; i < stops.length; i++) {
    const [z1, v1] = stops[i], [z0, v0] = stops[i - 1];
    if (z <= z1) return v0 + ((v1 - v0) * (z - z0)) / (z1 - z0);
  }
  return stops[stops.length - 1][1];
};
const icons = new Map<string, Path2D | null>();
const iconOf = (kind: string) => {
  if (!icons.has(kind)) {
    const d = iconPath(kind);
    icons.set(kind, d ? new Path2D(d) : null);
  }
  return icons.get(kind)!;
};
/** The roads' look: thin, in the basemap's quiet grey, widest for the most important. */
const ROAD_COLOUR = 'rgba(176, 184, 196, 0.85)';
/** Collision padding (CSS px), as MapLibre's text-padding. */
const PADDING = 2;
/** The poles' files (meta's versions). */
const FILES = { n: 'polar-n.json', s: 'polar-s.json' } as const;

/** The MapLibre layer whose look and visibility a point takes, and its density kind. */
export function layerOf(p: Props): { id: string; density: DensityKind } | null {
  const c = String(p.c ?? '');
  switch (p.k) {
    case 'place': {
      const id = Object.entries(LABEL_TILE_LAYERS).find(([, l]) => l.kind === 'place' && l.classes?.includes(c))?.[0];
      return id ? { id, density: 'places' } : null;
    }
    case 'state':
      return { id: 'place-state', density: 'places' };
    case 'water':
      return { id: 'water-name', density: 'water' };
    case 'park':
      return { id: 'park-label', density: 'parks' };
    case 'poi':
      return { id: `poi-${c}-label`, density: 'landmarks' };
    default:
      return null;
  }
}

/** MapLibre's unit globe (polarglobe.ts's). */
function sphere(lon: number, lat: number, r = 1): [number, number, number] {
  const l = (lon * Math.PI) / 180, f = (lat * Math.PI) / 180;
  return [r * Math.sin(l) * Math.cos(f), r * Math.sin(f), r * Math.cos(l) * Math.cos(f)];
}

type StyleLayerish = {
  visibility?: string;
  paint: { get(name: string): unknown };
  layout: { get(name: string): unknown };
};
type Colorish = { r: number; g: number; b: number; a: number };
/** A style property's value for a feature (a data-driven one evaluated). */
const ev = (v: unknown, f: unknown): unknown =>
  v && typeof (v as { evaluate?: unknown }).evaluate === 'function' ? (v as { evaluate: (f: unknown, s: object) => unknown }).evaluate(f, {}) : v;
/** MapLibre's colour (premultiplied) as straight RGBA. */
const straight = (c: unknown): Rgba => {
  const x = c as Colorish | undefined;
  if (!x || !(x.a > 0)) return [0, 0, 0, 0];
  return [x.r / x.a, x.g / x.a, x.b / x.a, x.a];
};
const css = (c: Rgba) => `rgba(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)}, ${c[3]})`;

/** A name's lines as the layer's text-field makes them: main, the sub line smaller under it, and a
 * peak's height. */
function linesOf(p: Props): TextLine[] {
  const main = displayOf({ ...p, sub: undefined }, 'n');
  if (!main) return [];
  const cap = main.charAt(0).toUpperCase() + main.slice(1);
  const out: TextLine[] = [{ text: cap, scale: 1 }];
  if (typeof p.sub === 'string' && p.sub) out.push({ text: p.sub, scale: 0.8 });
  if (p.k === 'poi' && p.c === 'peak' && typeof p.ele === 'number') out.push({ text: `${Math.round(p.ele)} m`, scale: 1 });
  return out;
}

type Drawn = { canvas: HTMLCanvasElement; ox: number; oy: number; box: [number, number, number, number] };

/** The polar caps' points on `map`, while the catalogue has their files. */
export class PolarPoints {
  private feats: Record<'n' | 's', Feature[]> = { n: [], s: [] };
  private loaded: Record<'n' | 's', string> = { n: '', s: '' };
  private glyphs = new Map<string, Map<string, Glyph | null>>();
  private asked = new Set<string>();
  private names = new Map<string, Drawn | null>();
  private overlay: HTMLCanvasElement;
  readonly layer: CustomLayerInterface;

  constructor(
    private map: MlMap,
    private density: () => LabelDensity,
  ) {
    this.overlay = document.createElement('canvas');
    this.overlay.className = 'polar-points';
    Object.assign(this.overlay.style, { position: 'absolute', left: '0', top: '0', pointerEvents: 'none' });
    map.getCanvasContainer().appendChild(this.overlay);
    const self = this;
    this.layer = {
      id: 'polar-points',
      type: 'custom',
      renderingMode: '2d',
      render(_gl, args: CustomRenderMethodInput) {
        self.draw(args);
      },
      onRemove() {
        self.clear();
      },
    };
    onVersions(Object.values(FILES), () => this.load());
    this.load();
  }

  /** Each pole's file at its version now (none while the catalogue has none). */
  private load() {
    for (const pole of ['n', 's'] as const) {
      const v = version(FILES[pole]);
      if (v === this.loaded[pole]) continue;
      this.loaded[pole] = v;
      if (!v) {
        this.feats[pole] = [];
        continue;
      }
      fetch(`${hostFor('layers')}/api/layer/polar-${pole}${ver(FILES[pole])}`)
        .then(keepable)
        .then((r) => (r.ok ? r.json() : null))
        .then((j: { features?: Feature[] } | null) => {
          if (this.loaded[pole] !== v) return;
          this.feats[pole] = j?.features ?? [];
          this.names.clear();
          this.map.triggerRepaint();
        })
        .catch((e) => {
          console.warn(`polar points ${pole}`, e);
          if (this.loaded[pole] === v) this.loaded[pole] = '';
        });
    }
  }

  private clear() {
    const g = this.overlay.getContext('2d');
    g?.clearRect(0, 0, this.overlay.width, this.overlay.height);
  }

  /** The style's glyphs of `stack` for `chars`, or null while some are still being fetched (the
   * map repaints once they're here). */
  private glyphsFor(stack: string, chars: string[]): Map<string, Glyph | null> | null {
    let have = this.glyphs.get(stack);
    if (!have) this.glyphs.set(stack, (have = new Map()));
    const want = chars.filter((c) => !have.has(c));
    if (!want.length) return have;
    const key = `${stack}|${want.join('')}`;
    if (!this.asked.has(key)) {
      this.asked.add(key);
      const gm = (this.map as unknown as { style?: { glyphManager?: { getGlyphs(r: Record<string, Record<string, string[]>>): Promise<Record<string, Record<string, Record<string, Glyph | null>>>> } } }).style?.glyphManager;
      gm?.getGlyphs({ [stack]: { default: want } })
        .then((r) => {
          for (const c of want) have.set(c, r[stack]?.default?.[c] ?? null);
          this.map.triggerRepaint();
        })
        .catch((e) => console.warn('polar points: glyphs', e))
        .finally(() => this.asked.delete(key));
    }
    return null;
  }

  /** A name drawn in its layer's look (cached by text and look), or null when it can't be yet. */
  private name(p: Props, sl: StyleLayerish, anchor: 'center' | 'top', dpr: number): Drawn | null | undefined {
    const lines = linesOf(p);
    if (!lines.length) return null;
    const f = { type: 1, properties: p };
    const font = ev(sl.layout.get('text-font'), f);
    const stack = Array.isArray(font) ? font.join(',') : 'Noto Sans Regular';
    const transform = String(ev(sl.layout.get('text-transform'), f) ?? 'none') as TextLook['transform'];
    const look: TextLook = {
      fill: straight(ev(sl.paint.get('text-color'), f)),
      halo: straight(ev(sl.paint.get('text-halo-color'), f)),
      haloWidth: Number(ev(sl.paint.get('text-halo-width'), f) ?? 0),
      haloBlur: Number(ev(sl.paint.get('text-halo-blur'), f) ?? 0),
      size: Number(ev(sl.layout.get('text-size'), f) ?? 12),
      spacing: Number(ev(sl.layout.get('text-letter-spacing'), f) ?? 0),
      maxWidth: Number(ev(sl.layout.get('text-max-width'), f) ?? 10),
      transform,
    };
    // (The landmarks' names fade with their dots' prominence on the Mercator side; here, whole.)
    if (p.k !== 'poi') look.fill[3] *= Number(ev(sl.paint.get('text-opacity'), f) ?? 1);
    const key = JSON.stringify([lines, look, anchor, dpr, stack]);
    if (this.names.has(key)) return this.names.get(key);
    const glyphs = this.glyphsFor(stack, charsOf(lines, look));
    if (!glyphs) return undefined;
    const img = rasterize(lines, glyphs, look, anchor, dpr);
    let d: Drawn | null = null;
    if (img) {
      const c = document.createElement('canvas');
      c.width = img.width;
      c.height = img.height;
      c.getContext('2d')!.putImageData(new ImageData(img.data, img.width, img.height), 0, 0);
      d = { canvas: c, ox: img.ox, oy: img.oy, box: img.box };
    }
    this.names.set(key, d);
    return d;
  }

  private draw(args: CustomRenderMethodInput) {
    const map = this.map;
    const pd = args.defaultProjectionData;
    const canvas = map.getCanvas();
    // (At the screen's own density, whatever the map's canvas is drawn at: names stay sharp where
    // the map lowers its resolution, as on a phone's 3× screen.)
    const dpr = window.devicePixelRatio || 1;
    const W = canvas.clientWidth, H = canvas.clientHeight;
    if (this.overlay.width !== Math.round(W * dpr) || this.overlay.height !== Math.round(H * dpr)) {
      this.overlay.width = Math.round(W * dpr);
      this.overlay.height = Math.round(H * dpr);
      this.overlay.style.width = `${W}px`;
      this.overlay.style.height = `${H}px`;
    }
    const g = this.overlay.getContext('2d')!;
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.clearRect(0, 0, this.overlay.width, this.overlay.height);
    // (The globe only: the flat Web Mercator map ends at 85.05°.)
    if (pd.projectionTransition < 0.999 || (!this.feats.n.length && !this.feats.s.length)) return;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const m = pd.mainMatrix as unknown as Float32Array;
    const plane = pd.clippingPlane;
    const ground = capGround(map);
    const exag = (map as unknown as { terrain?: { exaggeration: number } | null }).terrain?.exaggeration ?? 0;
    const at = (lon: number, lat: number): [number, number, number] => {
      const h = exag ? (ground?.(lon, lat) ?? 0) * exag : 0;
      return sphere(lon, lat, 1 + h / EARTH_R);
    };
    // A point on screen (CSS px) and whether it's in front: in the frustum and this side of the horizon.
    const screen = (p: [number, number, number]): [number, number] | null => {
      const cw = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
      if (cw <= 0) return null;
      const x = (m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12]) / cw, y = (m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13]) / cw;
      return [((x + 1) / 2) * W, ((1 - y) / 2) * H];
    };
    const front = (p: [number, number, number]) => p[0] * plane[0] + p[1] * plane[1] + p[2] * plane[2] + plane[3] >= -1e-6;
    // Ground metres a CSS pixel at a point: a short step along each of two tangents, the longer on
    // screen (the less foreshortened), as a label tile's zoom is the view's along the ground.
    const STEP = 200 / EARTH_R;
    const mppAt = (p: [number, number, number], s: [number, number]): number => {
      const n = Math.hypot(p[0], p[1], p[2]);
      const u = [p[0] / n, p[1] / n, p[2] / n];
      const ax = Math.abs(u[1]) > 0.9 ? [1, 0, 0] : [0, 1, 0];
      let t1 = [u[1] * ax[2] - u[2] * ax[1], u[2] * ax[0] - u[0] * ax[2], u[0] * ax[1] - u[1] * ax[0]];
      const l1 = Math.hypot(t1[0], t1[1], t1[2]);
      t1 = t1.map((v) => v / l1);
      const t2 = [u[1] * t1[2] - u[2] * t1[1], u[2] * t1[0] - u[0] * t1[2], u[0] * t1[1] - u[1] * t1[0]];
      let best = 0;
      for (const t of [t1, t2]) {
        const q = screen([p[0] + t[0] * STEP * n, p[1] + t[1] * STEP * n, p[2] + t[2] * STEP * n]);
        if (q) best = Math.max(best, Math.hypot(q[0] - s[0], q[1] - s[1]));
      }
      return best > 0 ? 200 / best : Infinity;
    };
    const style = (map as unknown as { style?: { getLayer(id: string): StyleLayerish | undefined } }).style;
    const shown = (id: string) => !!map.getLayer(id) && map.getLayoutProperty(id, 'visibility') !== 'none';
    const d = this.density();
    const margin = 64;
    const onScreen = (s: [number, number]) => s[0] > -margin && s[1] > -margin && s[0] < W + margin && s[1] < H + margin;
    let pending = false;

    // The roads, thin lines under everything else (while the road layer shows).
    if (shown('roads')) {
      g.strokeStyle = ROAD_COLOUR;
      g.lineCap = g.lineJoin = 'round';
      for (const f of [...this.feats.n, ...this.feats.s]) {
        if (f.geometry.type !== 'LineString') continue;
        const cls = String(f.properties.c ?? '');
        g.lineWidth = ['motorway', 'trunk', 'primary'].includes(cls) ? 1.6 : ['secondary', 'tertiary'].includes(cls) ? 1.2 : 0.8;
        g.beginPath();
        let open = false;
        for (const c of f.geometry.coordinates as number[][]) {
          const p = at(c[0], c[1]);
          const s = front(p) ? screen(p) : null;
          if (!s) {
            open = false;
            continue;
          }
          if (open) g.lineTo(s[0], s[1]);
          else g.moveTo(s[0], s[1]);
          open = true;
        }
        g.stroke();
      }
    }

    // The points in view whose layer shows: labels first, then the landmarks, each most important
    // first (the files' order).
    type Cand = { f: Feature; s: [number, number]; sl: StyleLayerish; poi: boolean; show: boolean };
    const cands: Cand[] = [];
    for (const f of [...this.feats.n, ...this.feats.s]) {
      if (f.geometry.type !== 'Point') continue;
      const l = layerOf(f.properties);
      const sl = l && shown(l.id) ? style?.getLayer(l.id) : undefined;
      if (!l || !sl) continue;
      const [lon, lat] = f.geometry.coordinates as number[];
      const p = at(lon, lat);
      if (!front(p)) continue;
      const s = screen(p);
      if (!s || !onScreen(s)) continue;
      const show = !!f.properties.n && nameShows(f.properties, mppAt(p, s), kindSpacing(d, l.density), DEFAULT_DENSITY.px);
      cands.push({ f, s, sl, poi: f.properties.k === 'poi', show });
    }
    cands.sort((a, b) => Number(a.poi) - Number(b.poi));
    // (The landmarks under the names, as the Mercator side's: those whose names show as their
    // kind's icon at the zoom's size, in their kind's colour with the point outline round it; the
    // rest as small dots, as its specks.)
    const zoom = map.getZoom();
    const iconR = Math.max(POI_ICON_MIN, interpStops(POI_ICON_R, zoom));
    const dotR = interpStops(POI_R, zoom);
    const halfOf = new Map<Cand, number>();
    for (const c of cands.filter((x) => x.poi)) {
      g.fillStyle = css(straight(ev(c.sl.paint.get('text-color'), { type: 1, properties: c.f.properties })));
      g.strokeStyle = HALO;
      const icon = c.show ? iconOf(String(c.f.properties.c)) : null;
      if (icon) {
        // (The 15-unit icon centred, its half-width iconR; the outline drawn first, under it.)
        const k = (2 * iconR) / 15;
        g.save();
        g.translate(c.s[0] - iconR, c.s[1] - iconR);
        g.scale(k, k);
        g.lineWidth = (2 * POI_ICON_STROKE) / k;
        g.lineJoin = 'round';
        g.stroke(icon);
        g.fill(icon);
        g.restore();
        halfOf.set(c, iconR);
      } else {
        g.beginPath();
        g.arc(c.s[0], c.s[1], dotR, 0, 2 * Math.PI);
        g.lineWidth = 1;
        g.stroke();
        g.fill();
        halfOf.set(c, dotR);
      }
    }
    const placed: [number, number, number, number][] = [];
    for (const c of cands) {
      if (!c.show) continue;
      const anchor = c.poi ? 'top' : 'center';
      const n = this.name(c.f.properties, c.sl, anchor, dpr);
      if (n === undefined) {
        pending = true;
        continue;
      }
      if (!n) continue;
      const y = c.poi ? c.s[1] + (halfOf.get(c) ?? 0) + POI_ICON_STROKE + NAME_GAP : c.s[1];
      const b: [number, number, number, number] = [c.s[0] + n.box[0] / dpr - PADDING, y + n.box[1] / dpr - PADDING, c.s[0] + n.box[2] / dpr + PADDING, y + n.box[3] / dpr + PADDING];
      if (placed.some((q) => q[0] < b[2] && b[0] < q[2] && q[1] < b[3] && b[1] < q[3])) continue;
      placed.push(b);
      // (On whole device pixels: the image is made at the screen's density, so drawn unscaled, unblurred.)
      const px = (v: number) => Math.round(v * dpr) / dpr;
      g.drawImage(n.canvas, px(c.s[0] - n.ox / dpr), px(y - n.oy / dpr), n.canvas.width / dpr, n.canvas.height / dpr);
    }
    if (pending) map.triggerRepaint();
  }
}
