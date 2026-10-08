// The map's own loading for the status line (tasks.ts): per kind of data (basemap, its small islands
// and lakes, terrain, slope, trees, each overlay), the tiles in view that have arrived out of those wanted, from MapLibre's
// tile managers; an overlay file still being tiled in MapLibre's worker shows as processing.
// MapLibre keeps these in its internals (style.tileManagers), read defensively: none of it is
// public API, and a change there only empties this list.
import type { Map as MLMap } from 'maplibre-gl';
import { OVERLAY_SOURCE } from './basemap';
import { OVERLAYS } from './state';
import type { Task } from './tasks';

interface TileLike { state?: string }
interface ManagerLike {
  used?: boolean;
  usedForTerrain?: boolean;
  _sourceLoaded?: boolean;
  _source?: { type?: string };
  _inViewTiles?: { getAllTiles?: () => TileLike[] };
}

function labelOf(id: string): string | null {
  if (id === 'base') return 'Basemap';
  if (id === 'water') return 'Water';
  if (id === 'dem' || id === 'dem-hs') return 'Terrain';
  if (id === 'slope') return 'Slope';
  if (id === 'contours') return 'Contours';
  if (id.startsWith('trees-')) return 'Trees';
  if (id === 'stations') return 'Stations';
  if (id === 'ferries') return 'Ferries';
  if (id === 'whs') return 'World Heritage outlines';
  const k = Object.keys(OVERLAY_SOURCE).find((key) => OVERLAY_SOURCE[key] === id);
  return k ? OVERLAYS.find(([o]) => o === k)?.[1] ?? null : null;
}

export function mapTasks(map: MLMap): Task[] {
  const tms = (map as unknown as { style?: { tileManagers?: Record<string, ManagerLike> } }).style?.tileManagers;
  if (!tms) return [];
  const by = new Map<string, { done: number; total: number; processing: boolean }>();
  for (const [id, tm] of Object.entries(tms)) {
    if (!tm.used && !tm.usedForTerrain) continue;
    const label = labelOf(id);
    if (!label) continue;
    const a = by.get(label) ?? { done: 0, total: 0, processing: false };
    if (tm._source?.type === 'geojson' && tm._sourceLoaded === false) a.processing = true;
    for (const t of tm._inViewTiles?.getAllTiles?.() ?? []) {
      a.total++;
      if (t.state === 'loaded' || t.state === 'errored' || t.state === 'expired') a.done++;
    }
    by.set(label, a);
  }
  const out: Task[] = [];
  for (const [label, a] of by) {
    if (a.processing) out.push({ label, detail: 'processing the file' });
    else if (a.done < a.total) out.push({ label, done: a.done, total: a.total, detail: 'tiles' });
  }
  return out;
}
