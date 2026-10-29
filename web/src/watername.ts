// The water's name and kind under the cursor, for the footer's water info (ui/strip.ts showWater),
// from what the map draws there: a river's or canal's line under the cursor (the basemap's
// `waterway` and its names), else the nearest water label in the view within a short reach (a sea's,
// bay's or lake's name, basemap.ts `water-name`), else the depth source's kind of water alone.
import type { Map as MLMap, PointLike } from 'maplibre-gl';
import { displayOf } from './names';

/** How far (px) a water label may stand from the cursor and still name the water there. */
const LABEL_REACH = 160;
/** Words for the basemap's water classes (OpenMapTiles' water_name and waterway classes). */
const KINDS: Record<string, string> = {
  ocean: 'ocean', sea: 'sea', bay: 'bay', strait: 'strait', lagoon: 'lagoon', lake: 'lake', reservoir: 'reservoir',
  pond: 'pond', river: 'river', canal: 'canal', stream: 'stream', ditch: 'ditch', drain: 'drain', dock: 'dock',
};
const kindOf = (p: Record<string, unknown>) => KINDS[String(p.class ?? p.c ?? '')] ?? '';
const nameOf = (p: Record<string, unknown>) => displayOf(p) || displayOf(p, 'n');

export function waterNameAt(map: MLMap, pt: { x: number; y: number }, water: 'sea' | 'lake' | null): { name: string; kind: string } {
  const has = (ids: string[]) => ids.filter((id) => map.getLayer(id));
  const box = (r: number): [PointLike, PointLike] => [[pt.x - r, pt.y - r], [pt.x + r, pt.y + r]];
  // A river or canal under the cursor.
  for (const f of map.queryRenderedFeatures(box(5), { layers: has(['waterway', 'water-name-line']) })) {
    const p = f.properties ?? {};
    const kind = kindOf(p);
    if (kind && f.geometry.type.includes('Line')) return { name: nameOf(p), kind };
  }
  // The nearest water label (a point label's anchor, a line label's nearest vertex).
  let best = { name: '', kind: '', d: LABEL_REACH };
  for (const f of map.queryRenderedFeatures(box(LABEL_REACH), { layers: has(['water-name']) })) {
    const p = f.properties ?? {};
    const name = nameOf(p);
    if (!name) continue;
    const g = f.geometry;
    const pts = g.type === 'Point' ? [g.coordinates] : g.type === 'LineString' || g.type === 'MultiPoint' ? g.coordinates : g.type === 'MultiLineString' ? g.coordinates.flat() : [];
    for (const c of pts) {
      const q = map.project(c as [number, number]);
      const d = Math.hypot(q.x - pt.x, q.y - pt.y);
      if (d < best.d) best = { name, kind: kindOf(p), d };
    }
  }
  return { name: best.name, kind: best.kind || (water ?? '') };
}
