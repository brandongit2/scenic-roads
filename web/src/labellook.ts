// The look of each type of map label (Layers → Labels → Style): fill and halo colour and opacity,
// halo width and blur, font, size and its growth with zoom, letter spacing, case, shown. The style
// sets each type's defaults (basemap.ts); AppState.labelLooks holds only what was changed, per
// type, applied over them with setPaintProperty / setLayoutProperty.
import type { Map as MLMap } from 'maplibre-gl';

/** Text case: as written, or MapLibre's text-transform. */
export type TextCase = 'none' | 'uppercase' | 'lowercase';

export interface LabelLook {
  /** Fill colour (#rrggbb) and its opacity (0..1). */
  colour: string;
  opacity: number;
  /** Halo colour, its opacity, width and blur (px). */
  halo: string;
  haloOpacity: number;
  haloWidth: number;
  haloBlur: number;
  font: string;
  /** Size × the type's own (and × Label size), and its growth with zoom: × 2^(growth · (z − 10) / 6). */
  size: number;
  growth: number;
  /** Letter spacing (em). */
  spacing: number;
  case: TextCase;
  shown: boolean;
}
export type LabelLooks = Record<string, Partial<LabelLook>>;

/** The fonts the app serves (scripts/fonts.sh): the weight and style are the font's. */
export const LABEL_FONTS = ['Noto Sans Regular', 'Noto Sans Medium', 'Noto Sans Italic'] as const;

/** The stops & sights kinds' names (basemap.ts POI_STYLE's keys, in its order). */
const POI_NAMES: Record<string, string> = {
  viewpoint: 'Viewpoints', peak: 'Peaks', waterfall: 'Waterfalls', lighthouse: 'Lighthouses',
  covered_bridge: 'Covered bridges', rest: 'Rest areas & picnic sites', trailhead: 'Trailheads',
};

/** Every type of label the map draws, its layers (contour and depth labels: their own colour and
 * size under Terrain and Depth; the map draws no country or road names). */
export const LABEL_TYPES: { key: string; label: string; layers: string[] }[] = [
  { key: 'city', label: 'Cities (the biggest larger)', layers: ['place-city'] },
  { key: 'town', label: 'Towns', layers: ['place-town'] },
  { key: 'village', label: 'Villages', layers: ['place-village'] },
  { key: 'minor', label: 'Hamlets & neighbourhoods', layers: ['place-minor'] },
  { key: 'state', label: 'Provinces & states', layers: ['place-state'] },
  { key: 'water', label: 'Seas, bays & lakes', layers: ['water-name'] },
  { key: 'river', label: 'Rivers', layers: ['water-name-line'] },
  { key: 'parks', label: 'Parks & protected areas', layers: ['park-label'] },
  { key: 'heritage', label: 'Heritage sites', layers: ['heritage-label'] },
  { key: 'special', label: 'Biospheres, geoparks & dark-sky places', layers: ['special-label'] },
  { key: 'indigenous', label: 'Indigenous lands', layers: ['indigenous-label'] },
  ...Object.entries(POI_NAMES).map(([k, label]) => ({ key: `poi-${k}`, label, layers: [`poi-${k}-label`] })),
  { key: 'ferry', label: 'Ferry lines', layers: ['ferry-label'] },
  { key: 'ferry-terminal', label: 'Ferry terminals', layers: ['ferry-terminal-label'] },
  { key: 'rail', label: 'Rail stations & stops', layers: ['rail-stop-label'] },
];
const TYPE_OF = new Map(LABEL_TYPES.flatMap((t) => t.layers.map((id) => [id, t.key] as const)));
/** The label type of a layer id. */
export const labelTypeOf = (id: string): string | undefined => TYPE_OF.get(id);

/** Whether a label layer is shown by its type's look (with its kind's switch: state.ts labelShown). */
export const labelLayerShown = (looks: LabelLooks, id: string): boolean => looks[TYPE_OF.get(id) ?? '']?.shown !== false;

const HEX = /^#[0-9a-f]{6}$/i;
/** #rrggbb and alpha of a CSS colour MapLibre accepts (hex, rgb[a]); null for anything else. */
export function parseColour(c: unknown): [string, number] | null {
  if (typeof c !== 'string') return null;
  const s = c.trim().toLowerCase();
  if (HEX.test(s)) return [s, 1];
  if (/^#[0-9a-f]{3}$/.test(s)) return [`#${[...s.slice(1)].map((x) => x + x).join('')}`, 1];
  const m = s.match(/^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/);
  if (!m) return null;
  const hx = (v: string) => Math.round(Math.min(255, Number(v))).toString(16).padStart(2, '0');
  return [`#${hx(m[1])}${hx(m[2])}${hx(m[3])}`, m[4] === undefined ? 1 : Number(m[4])];
}
/** A colour with an alpha, as MapLibre reads it. */
export function rgba(hex: string, a: number): string {
  const n = parseInt(hex.slice(1), 16);
  return a >= 1 ? hex : `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${+a.toFixed(3)})`;
}
/** Every colour in a style value with alpha × a (a colour expression: each colour it names). */
function withAlpha(v: unknown, a: number): unknown {
  const c = parseColour(v);
  if (c) return rgba(c[0], c[1] * a);
  return Array.isArray(v) ? v.map((x) => withAlpha(x, a)) : v;
}
/** The first colour a style value names (a colour expression's: its fallback's, else its first). */
function firstColour(v: unknown): [string, number] | null {
  const c = parseColour(v);
  if (c) return c;
  if (!Array.isArray(v)) return null;
  return firstColour(v[v.length - 1]) ?? v.map(firstColour).find((x) => x) ?? null;
}

/** A layer's paint and layout as the style made them, before any look: read once per layer. */
type Base = { color: unknown; halo: unknown; haloWidth: unknown; haloBlur: unknown; font: unknown; spacing: unknown; transform: unknown };
const bases = new Map<string, Base>();
function baseOf(map: MLMap, id: string): Base {
  let b = bases.get(id);
  if (!b) {
    b = {
      color: map.getPaintProperty(id, 'text-color'), halo: map.getPaintProperty(id, 'text-halo-color'),
      haloWidth: map.getPaintProperty(id, 'text-halo-width'), haloBlur: map.getPaintProperty(id, 'text-halo-blur'),
      font: map.getLayoutProperty(id, 'text-font'), spacing: map.getLayoutProperty(id, 'text-letter-spacing'),
      transform: map.getLayoutProperty(id, 'text-transform'),
    };
    bases.set(id, b);
  }
  return b;
}

/** Each type's look as the style sets it (from its first layer in the map), for the panel. */
export function defaultLooks(map: MLMap): Record<string, LabelLook> {
  const out: Record<string, LabelLook> = {};
  for (const t of LABEL_TYPES) {
    const id = t.layers.find((l) => map.getLayer(l));
    if (!id) continue;
    const b = baseOf(map, id);
    const [colour, opacity] = firstColour(b.color) ?? ['#000000', 1];
    const [halo, haloOpacity] = firstColour(b.halo) ?? ['#000000', 1];
    const font = Array.isArray(b.font) && typeof b.font[0] === 'string' ? b.font[0] : LABEL_FONTS[0];
    out[t.key] = {
      colour, opacity, halo, haloOpacity,
      haloWidth: typeof b.haloWidth === 'number' ? b.haloWidth : 0,
      haloBlur: typeof b.haloBlur === 'number' ? b.haloBlur : 0,
      font, size: 1, growth: 0,
      spacing: typeof b.spacing === 'number' ? b.spacing : 0,
      case: b.transform === 'uppercase' || b.transform === 'lowercase' ? b.transform : 'none',
      shown: true,
    };
  }
  return out;
}

const painted = new Map<string, string>();
function put(map: MLMap, id: string, kind: 'paint' | 'layout', prop: string, v: unknown) {
  const key = `${id}|${prop}`, j = JSON.stringify(v ?? null);
  if (painted.get(key) === j) return;
  painted.set(key, j);
  if (kind === 'paint') map.setPaintProperty(id, prop as never, v as never);
  else map.setLayoutProperty(id, prop as never, v as never);
}

/** Each label layer's look: the type's changes over the style's own (unchanged: the style's). Size
 * and shown are applied with Label size (terrain.ts applyLabelSize) and the layers' visibility. */
export function applyLabelLooks(map: MLMap, looks: LabelLooks) {
  for (const t of LABEL_TYPES) {
    const o = looks[t.key] ?? {};
    for (const id of t.layers) {
      if (!map.getLayer(id)) continue;
      const b = baseOf(map, id);
      const fill = o.colour !== undefined ? rgba(o.colour, o.opacity ?? firstColour(b.color)?.[1] ?? 1)
        : o.opacity !== undefined ? withAlpha(b.color, o.opacity / (firstColour(b.color)?.[1] || 1)) : b.color;
      const hb = firstColour(b.halo) ?? ['#000000', 1];
      const halo = o.halo !== undefined || o.haloOpacity !== undefined ? rgba(o.halo ?? hb[0], o.haloOpacity ?? hb[1]) : b.halo;
      put(map, id, 'paint', 'text-color', fill);
      put(map, id, 'paint', 'text-halo-color', halo);
      put(map, id, 'paint', 'text-halo-width', o.haloWidth ?? b.haloWidth);
      put(map, id, 'paint', 'text-halo-blur', o.haloBlur ?? b.haloBlur);
      put(map, id, 'layout', 'text-font', o.font ? [o.font] : b.font);
      put(map, id, 'layout', 'text-letter-spacing', o.spacing ?? b.spacing);
      put(map, id, 'layout', 'text-transform', o.case ?? b.transform);
    }
  }
}

/** A label layer's size factor and its growth with zoom (applyLabelSize). */
export function labelSizeOf(looks: LabelLooks, id: string): [number, number] {
  const o = looks[TYPE_OF.get(id) ?? ''];
  return [o?.size ?? 1, o?.growth ?? 0];
}

/** Saved looks, kept where each value has its type (older or hand-edited saves). */
export function cleanLooks(v: unknown): LabelLooks {
  const out: LabelLooks = {};
  if (!v || typeof v !== 'object' || Array.isArray(v)) return out;
  const keys = new Set(LABEL_TYPES.map((t) => t.key));
  const num = (x: unknown, lo: number, hi: number) => (typeof x === 'number' && Number.isFinite(x) ? Math.min(hi, Math.max(lo, x)) : undefined);
  for (const [k, raw] of Object.entries(v as Record<string, unknown>)) {
    if (!keys.has(k) || !raw || typeof raw !== 'object') continue;
    const r = raw as Record<string, unknown>;
    const o: Partial<LabelLook> = {
      colour: typeof r.colour === 'string' && HEX.test(r.colour) ? r.colour.toLowerCase() : undefined,
      opacity: num(r.opacity, 0, 1),
      halo: typeof r.halo === 'string' && HEX.test(r.halo) ? r.halo.toLowerCase() : undefined,
      haloOpacity: num(r.haloOpacity, 0, 1),
      haloWidth: num(r.haloWidth, 0, 6),
      haloBlur: num(r.haloBlur, 0, 6),
      font: (LABEL_FONTS as readonly unknown[]).includes(r.font) ? (r.font as string) : undefined,
      size: num(r.size, 0.3, 3),
      growth: num(r.growth, -1, 1),
      spacing: num(r.spacing, -0.2, 1),
      case: r.case === 'none' || r.case === 'uppercase' || r.case === 'lowercase' ? r.case : undefined,
      shown: typeof r.shown === 'boolean' ? r.shown : undefined,
    };
    for (const key of Object.keys(o) as (keyof LabelLook)[]) if (o[key] === undefined) delete o[key];
    if (Object.keys(o).length) out[k] = o;
  }
  return out;
}
