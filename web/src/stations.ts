// Rail stops on the map (dem/stations.py): a dot per stop of a passenger line, shown once its
// lines' stop spacing spans STOP_PX on screen and sized by it, so intercity stations show from far
// out and large, metro and tram stops only close in and small; shown with the rail layer and its
// group toggles, at its opacity. Each dot takes the colour of the rail line drawn at it
// (recolour: the lines are coloured on the GPU, per vertex) and shows only once it has it
// (feature state k), so it never appears in another colour first; where no line is drawn at it,
// its service group's. Stops are coloured once the view settles, from the source's tiles and
// the rail line at their place: querying the rendered dots on the 3D globe ray-marched the
// terrain for the view's corners in every tile, up to a second and more after a gesture.
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap } from 'maplibre-gl';
import { onVersions, ver } from './api';
import { STATION_LAYER, stationTilesOn } from './basemap';
import { hostFor } from './hosts';
import { RAIL_GROUP_COLOURS } from './rail';
import { kindSpacing, labelShown, lineWeight, type AppState } from './state';

const DOTS = 'rail-stop';
const LABELS = 'rail-stop-label';
/** A stop shows once its lines' average stop spacing spans this many pixels; its name at LABEL_PX
 * (at the default label density; Layers → Map labels → Density scales it). */
const STOP_PX = 12;
const LABEL_PX = 70;

/** The stops' layer file. */
const stopsUrl = () => `${hostFor('layers')}/api/layer/stations${ver('stations.json')}`;

/** From the zoom where the spacing spans `px` pixels (mz: where it spans one). */
const spaced = (px: number): ExpressionSpecification => ['>=', ['zoom'], ['+', ['get', 'mz'], Math.log2(px)]];

export class Stations {
  private requested = false;
  private railMask = 0;
  /** Stops coloured for the colouring of the moment (feature id → colour; null: their group's). */
  private coloured = new Map<number, string | null>();

  constructor(private map: MLMap) {
    // New stops (a new catalog): fetched again if they were, and coloured anew (their feature ids
    // are their places in the file). By view, the new tiles come with the others' (main.ts), and
    // the colours go with them.
    onVersions(['stations.json', 'stations.tiles'], (files) => {
      if (stationTilesOn()) {
        if (!files.includes('stations.tiles')) return;
        this.coloured.clear();
        map.removeFeatureState({ source: 'stations', sourceLayer: STATION_LAYER });
        return;
      }
      if (!this.requested || !files.includes('stations.json')) return;
      this.coloured.clear();
      map.removeFeatureState({ source: 'stations' });
      map.getSource<GeoJSONSource>('stations')?.setData(stopsUrl());
    });
  }

  apply(s: AppState) {
    const map = this.map;
    const r = s.rail;
    if (!map.getLayer(DOTS)) return;
    if (r.on && !this.requested) {
      this.requested = true;
      if (!stationTilesOn()) map.getSource<GeoJSONSource>('stations')?.setData(stopsUrl());
    }
    this.railMask = r.groups.reduce((m, on, i) => (on ? m | (1 << i) : m), 0);
    map.setLayoutProperty(DOTS, 'visibility', r.on ? 'visible' : 'none');
    map.setLayoutProperty(LABELS, 'visibility', r.on && labelShown(s, 'stations') ? 'visible' : 'none');
    // Any of the groups calling there shown (m: a bit per group).
    const groups: ExpressionSpecification = ['any', ...r.groups.flatMap((on, i) => (on ? [['==', ['%', ['floor', ['/', ['get', 'm'], 2 ** i]], 2], 1] as ExpressionSpecification] : [])), false];
    map.setFilter(DOTS, ['all', groups, spaced(STOP_PX)]);
    map.setFilter(LABELS, ['all', groups, spaced(kindSpacing(s.labelDensity, 'stations', LABEL_PX)), ['!=', ['get', 'n'], '']]);
    // Size by spacing: ×1 at 4 km (a commuter line; about a ferry terminal's dot), smaller for
    // closer stops, larger for wider ones; the rail line weight scales it too.
    const k: ExpressionSpecification = ['*', lineWeight(s, 'rail'), ['min', 1.4, ['max', 0.5, ['+', 1, ['*', 0.15, ['log2', ['/', ['get', 'sp'], 4000]]]]]]];
    map.setPaintProperty(DOTS, 'circle-radius', ['interpolate', ['linear'], ['zoom'], 4, ['*', 0.5, k], 9, ['*', 0.9, k], 14, ['*', 1.75, k], 18, ['*', 2.5, k]]);
    map.setPaintProperty(DOTS, 'circle-color', ['to-color', ['coalesce', ['feature-state', 'c'], ['match', ['get', 'g'], ...RAIL_GROUP_COLOURS.flatMap((c, i) => [i, c]), '#cfd6e0']]] as unknown as ExpressionSpecification);
    const shown: ExpressionSpecification = ['case', ['boolean', ['feature-state', 'k'], false], r.opacity, 0];
    map.setPaintProperty(DOTS, 'circle-opacity', shown);
    map.setPaintProperty(DOTS, 'circle-stroke-opacity', shown);
  }

  /** The stops shown and not yet coloured (with `all`, the colouring changed: every stop again)
   * take the colour of the rail line at them (`colourAt`: lng, lat → colour; null where a rail tile
   * is drawn but no line near; undefined where no rail tile is drawn) and show; where no line is,
   * their service group's once the rail tiles in view are all drawn (`settled`), until then they
   * wait. A generator: it yields every few stops (idle.ts runs it between frames). */
  *recolour(colourAt: (lng: number, lat: number) => string | null | undefined, all: boolean, settled: boolean): Generator<void, void> {
    const map = this.map;
    if (!map.getLayer(DOTS) || map.getLayoutProperty(DOTS, 'visibility') === 'none') return;
    if (all) this.coloured.clear();
    // Shown at this zoom (the dots' filter, spaced(STOP_PX)).
    const minZ = map.getZoom() - Math.log2(STOP_PX);
    const layer = stationTilesOn() ? { sourceLayer: STATION_LAYER } : {};
    const fs = map.querySourceFeatures('stations', layer);
    yield;
    for (let i = 0; i < fs.length; i++) {
      if (i % 32 === 31) yield;
      const f = fs[i];
      const id = f.id as number | undefined;
      if (id === undefined || this.coloured.has(id)) continue;
      const p = f.properties ?? {};
      if (!(Number(p.mz) <= minZ) || !(Number(p.m) & this.railMask)) continue;
      const [lng, lat] = (f.geometry as GeoJSON.Point).coordinates;
      const c = colourAt(lng, lat);
      if (c === undefined || (c === null && !settled)) continue;
      map.setFeatureState({ source: 'stations', id, ...layer }, { c, k: true });
      this.coloured.set(id, c);
    }
  }
}
