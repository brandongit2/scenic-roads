// The region layer's tiles as the app reads them (docs/steps.md §15; crates/codecs/src/regionlayer):
// their MVT layer of borders, each a line with the regions on its left and right (`l`, `r`: their
// ids, comma-joined), and the styling expressions that pick borders by the regions either side.
// Pure: node's tests load it (web/test/regionlayer.test.ts).
import type { ExpressionSpecification, FilterSpecification } from 'maplibre-gl';

/** The layer in the catalogue, its tiles' MVT layer and its finest zoom. */
export const LAYER = 'regions';
export const SOURCE_LAYER = 'borders';
export const MAXZ = 12;

/** What `pick` and `labelPoints` read of a listed region (regions.ts's Region). */
interface Listed {
  id: string;
  name: string;
  disabled?: boolean;
  bbox?: [number, number, number, number];
  label?: [number, number];
}

/** Of the regions at a point, the one the footer tells: the smallest by its box (a province over
 * the waters round its country, where regions overlap; a region with no box yet last); null for
 * none. */
export function pick(ids: readonly string[], byId: ReadonlyMap<string, Listed>): string | null {
  let best: string | null = null, bestA = Infinity;
  for (const id of ids) {
    const b = byId.get(id)?.bbox;
    const a = b ? (b[2] - b[0]) * (b[3] - b[1]) : Infinity;
    if (best === null || a < bestA) {
      best = id;
      bestA = a;
    }
  }
  return best;
}

/** Whether a border's side (`l` or `r`) has any of `ids` (false for none). */
export function sideHas(side: 'l' | 'r', ids: readonly string[]): ExpressionSpecification | false {
  if (!ids.length) return false;
  // (Padded with commas: "fr" isn't found in "fr-cor".)
  const padded: ExpressionSpecification = ['concat', ',', ['get', side], ','];
  return ['any', ...ids.map((id): ExpressionSpecification => ['in', `,${id},`, padded])];
}

/** The borders of the regions `ids` taken as one (a folder's): those with them on one side only. */
export const borderOf = (ids: readonly string[]): FilterSpecification => ['!=', sideHas('l', ids), sideHas('r', ids)] as FilterSpecification;

export const GREEN = '#5fcf8f';
export const GREY = '#8a919c';
/** The borders' colour: green, grey for a border with no enabled region either side. */
export const colourOf = (enabled: readonly string[]): ExpressionSpecification | string =>
  enabled.length ? ['case', ['any', sideHas('l', enabled), sideHas('r', enabled)], GREEN, GREY] : GREY;

/** The regions' names as points (where the server put each: inside its largest polygon), a
 * disabled region's marked. */
export function labelPoints(rs: readonly Listed[]): GeoJSON.FeatureCollection<GeoJSON.Point, { name: string; disabled: boolean }> {
  return {
    type: 'FeatureCollection',
    features: rs.flatMap((r) => r.label
      ? [{ type: 'Feature' as const, geometry: { type: 'Point' as const, coordinates: r.label }, properties: { name: r.name, disabled: !!r.disabled } }]
      : []),
  };
}
