// The map's markers (the style's `marks` source): the profile's cursor, the highest and lowest
// road in view, the viewshed's eye, a place searched for, and the ring round a hovered list item
// too small on screen to see at a glance.
import type { GeoJSONSource, Map as MLMap } from 'maplibre-gl';
import { HER_R } from '../basemap';

export type MarkKind = 'cursor' | 'high' | 'low' | 'viewshed' | 'ring' | 'place';

/** A point feature of a kind, labelled. */
export const point = (ll: [number, number], kind: string, label?: string): GeoJSON.Feature => ({
  type: 'Feature',
  properties: label ? { kind, label } : { kind },
  geometry: { type: 'Point', coordinates: ll },
});

/** A line feature (null: nothing). */
export const line = (coords: [number, number][] | null): GeoJSON.GeoJSON =>
  coords ? { type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates: coords } } : { type: 'FeatureCollection', features: [] };

export class Marks {
  private marks: Record<MarkKind, GeoJSON.Feature | null> = { cursor: null, high: null, low: null, viewshed: null, ring: null, place: null };

  constructor(private map: MLMap) {}

  /** A marker set (null: gone), and the source again. */
  set(kind: MarkKind, f: GeoJSON.Feature | null) {
    this.marks[kind] = f;
    this.map.getSource<GeoJSONSource>('marks')?.setData({ type: 'FeatureCollection', features: Object.values(this.marks).filter(Boolean) as GeoJSON.Feature[] });
  }

  /** The ring around lines (lng, lat) too small on screen to see at a glance: whose smallest
   * enclosing circle (with the line's width) is under two of the largest landmark dots across. The
   * ring clears them by a comfortable margin. */
  ringAround(parts: [number, number][][] | null) {
    const map = this.map;
    let ring: GeoJSON.Feature | null = null;
    const all = parts?.flat() ?? [];
    if (all.length) {
      const step = Math.max(1, Math.floor(all.length / 400));
      const pts = all.filter((_, i) => i % step === 0 || i === all.length - 1).map((q) => map.project(q));
      // Ritter's bounding circle: from the point farthest from the first to the one farthest from
      // that, grown to take in any point outside.
      const far = (a: { x: number; y: number }) => pts.reduce((b, q) => (Math.hypot(q.x - a.x, q.y - a.y) > Math.hypot(b.x - a.x, b.y - a.y) ? q : b), a);
      const p1 = far(pts[0]), p2 = far(p1);
      let cx = (p1.x + p2.x) / 2, cy = (p1.y + p2.y) / 2, r = Math.hypot(p2.x - p1.x, p2.y - p1.y) / 2;
      for (const q of pts) {
        const d = Math.hypot(q.x - cx, q.y - cy);
        if (d > r) {
          const nr = (r + d) / 2, k = (nr - r) / d;
          cx += (q.x - cx) * k;
          cy += (q.y - cy) * k;
          r = nr;
        }
      }
      const z = map.getZoom();
      // The largest landmark dot: a World Heritage site at the top of the scale (dots.ts).
      const stops = HER_R[0], i = stops.findIndex(([sz]) => sz >= z);
      const base = i < 0 ? stops[stops.length - 1][1] : i === 0 ? stops[0][1] : stops[i - 1][1] + ((stops[i][1] - stops[i - 1][1]) * (z - stops[i - 1][0])) / (stops[i][0] - stops[i - 1][0]);
      const maxDot = 2 * base * 1.25;
      const lineW = 3;
      if (2 * (r + lineW / 2) < 2 * maxDot) {
        const c = map.unproject([cx, cy]);
        ring = { type: 'Feature', properties: { kind: 'ring', r: Math.max(10, r + lineW / 2 + Math.max(6, r * 0.6)) }, geometry: { type: 'Point', coordinates: [c.lng, c.lat] } };
      }
    }
    this.set('ring', ring);
  }
}
