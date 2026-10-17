// The map's own loading for the status line (tasks.ts): per kind of data (basemap, the water,
// terrain, slope, trees, 3D buildings, coast, depth, labels, each overlay; any other source by its
// id), the tiles in view that have arrived out of those wanted, from MapLibre's tile managers (a
// tile counts as arrived once parsed in MapLibre's worker, its glyphs and images in: loaded, or
// errored); a GeoJSON source still being tiled in MapLibre's worker shows as processing; glyphs
// still to be drawn on the page (glyphs.ts) and shader programs still linking in the background
// (glprog.ts, the fork's Context.pendingPrograms, as of the last frame drawn). The regions' own
// sources aren't counted: downloads have their own panel.
// MapLibre keeps these in its internals (style.tileManagers), read defensively: none of it is
// public API, and a change there only empties this list.
import type { Map as MLMap } from 'maplibre-gl';
import { OVERLAY_SOURCE } from './basemap';
import { OVERLAYS } from './state';
import type { Task } from './tasks';
import { glyphsWaiting } from './glyphs';

interface TileLike { state?: string }
interface ManagerLike {
  used?: boolean;
  usedForTerrain?: boolean;
  _sourceLoaded?: boolean;
  _source?: { type?: string };
  _inViewTiles?: { getAllTiles?: () => TileLike[] };
}

/** Sources not counted: the regions panel's own (downloads); the viewshed's image (its search is a
 * task of its own); the app's own GeoJSON it fills itself (the selection, the climb, the drives and
 * their highlight, the marks); the shoreline check's (evalmode.ts, eval-…). Any other id shows by
 * itself: a loader new since. */
const SKIP = new Set(['coverage', 'region-hover', 'regions', 'viewshed', 'selection', 'climb', 'drives', 'drive-hl', 'marks']);

function labelOf(id: string): string | null {
  if (SKIP.has(id) || id.startsWith('eval-')) return null;
  if (id === 'base') return 'Basemap';
  if (id === 'bld') return '3D buildings';
  if (id === 'coast') return 'Coast';
  if (id.startsWith('depth-')) return 'Depth';
  if (id === 'lbl') return 'Labels';
  if (id === 'water') return 'Water';
  if (id === 'dem' || id === 'dem-hs') return 'Terrain';
  if (id === 'slope') return 'Slope';
  if (id === 'contours') return 'Contours';
  if (id.startsWith('trees-')) return 'Trees';
  if (id === 'stations') return 'Stations';
  if (id === 'ferries') return 'Ferries';
  if (id === 'whs') return 'World Heritage outlines';
  const k = Object.keys(OVERLAY_SOURCE).find((key) => OVERLAY_SOURCE[key] === id);
  return (k ? OVERLAYS.find(([o]) => o === k)?.[1] : null) ?? id;
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
    // ('reloading': a loaded tile parsed again, its old data drawn meanwhile, counted as arrived: a
    // live layer's refresh or an expired tile's refetch doesn't show; 'unloaded': dropped)
    for (const t of tm._inViewTiles?.getAllTiles?.() ?? []) {
      if (t.state === 'unloaded') continue;
      a.total++;
      if (t.state === 'loaded' || t.state === 'reloading' || t.state === 'errored' || t.state === 'expired') a.done++;
    }
    by.set(label, a);
  }
  const out: Task[] = [];
  for (const [label, a] of by) {
    if (a.processing) out.push({ label, detail: 'processing the file' });
    else if (a.done < a.total) out.push({ label, done: a.done, total: a.total, detail: 'tiles' });
  }
  const g = glyphsWaiting();
  if (g) out.push({ label: 'Glyphs', detail: `${g} to draw` });
  const progs = (map as unknown as { painter?: { context?: { pendingPrograms?: number } } }).painter?.context?.pendingPrograms ?? 0;
  if (progs) out.push({ label: 'Shaders', detail: `${progs} draws waiting for a program to link` });
  return out;
}
