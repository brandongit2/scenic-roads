// Rail stops on the map (pipeline::stations): a dot per stop of a passenger line, shown once its
// lines' stop spacing spans STOP_PX on screen and sized by it, so intercity stations show from far
// out and large, metro and tram stops only close in and small (raildraw.ts stopFactor; the rail
// card's stop size and size contrast scale it); shown with the rail layer and its group toggles,
// at its opacity. Each dot takes the colour and opacity of the rail line as drawn at it (recolour:
// the lines are coloured on the GPU, per vertex; its tunnels' and low end's fades included) and
// shows only once it has them (feature state k), so it never appears in another colour first;
// where no line is drawn at it, its service group's. Its outline's colour and opacity are the
// rail card's. Stops are coloured once the view settles, from the source's tiles and
// the rail line at their place: querying the rendered dots on the 3D globe ray-marched the
// terrain for the view's corners in every tile, up to a second and more after a gesture.
import type { ExpressionSpecification, Map as MLMap } from 'maplibre-gl';
import { onVersions } from './api';
import { STATION_LAYER } from './basemap';
import { RAIL_GROUP_COLOURS } from './rail';
import { STOP_R, STOP_REF_M, stopBounds, stopSlope } from './raildraw';
import { kindSpacing, labelShown, lineWeight, type AppState } from './state';

/** A rail line's colour and opacity as drawn at a point. */
export interface Look {
  c: string;
  a: number;
}

const DOTS = 'rail-stop';
const LABELS = 'rail-stop-label';
/** A stop shows once its lines' average stop spacing spans this many pixels; its name at LABEL_PX
 * (at the default label density; Layers → Map labels → Density scales it). */
const STOP_PX = 12;
const LABEL_PX = 70;

/** From the zoom where the spacing spans `px` pixels (mz: where it spans one). */
const spaced = (px: number): ExpressionSpecification => ['>=', ['zoom'], ['+', ['get', 'mz'], Math.log2(px)]];

export class Stations {
  private railMask = 0;
  /** Stops coloured for the colouring of the moment (feature id → colour; null: their group's). */
  private coloured = new Map<number, Look | null>();
  /** At start-up, the stops' tiles wait for the roads in view (as the overlays: release()). */
  private held = true;
  private state: AppState | null = null;

  release() {
    if (!this.held) return;
    this.held = false;
    if (this.state) this.apply(this.state);
  }

  constructor(private map: MLMap) {
    // New stops (a new catalog): the new tiles come with the others' (main.ts), and the colours go
    // with them.
    onVersions(['stations.tiles'], () => {
      this.coloured.clear();
      map.removeFeatureState({ source: 'stations', sourceLayer: STATION_LAYER });
    });
  }

  apply(s: AppState) {
    const map = this.map;
    const r = s.rail;
    this.state = s;
    if (!map.getLayer(DOTS)) return;
    this.railMask = r.groups.reduce((m, on, i) => (on ? m | (1 << i) : m), 0);
    const on = r.on && !this.held;
    map.setLayoutProperty(DOTS, 'visibility', on ? 'visible' : 'none');
    map.setLayoutProperty(LABELS, 'visibility', on && labelShown(s, 'stations') ? 'visible' : 'none');
    // Any of the groups calling there shown (m: a bit per group).
    const groups: ExpressionSpecification = ['any', ...r.groups.flatMap((on, i) => (on ? [['==', ['%', ['floor', ['/', ['get', 'm'], 2 ** i]], 2], 1] as ExpressionSpecification] : [])), false];
    map.setFilter(DOTS, ['all', groups, spaced(STOP_PX)]);
    map.setFilter(LABELS, ['all', groups, spaced(kindSpacing(s.labelDensity, 'stations', LABEL_PX)), ['!=', ['get', 'n'], '']]);
    // Size by spacing (raildraw.ts stopFactor), × the stop size and the rail line weight.
    const [lo, hi] = stopBounds(r.stopContrast);
    const k: ExpressionSpecification = ['*', lineWeight(s, 'rail') * r.stopSize,
      ['min', hi, ['max', lo, ['+', 1, ['*', stopSlope(r.stopContrast), ['log2', ['/', ['get', 'sp'], STOP_REF_M]]]]]]];
    map.setPaintProperty(DOTS, 'circle-radius', ['interpolate', ['linear'], ['zoom'], ...STOP_R.flatMap(([z, f]) => [z, ['*', f, k]])] as ExpressionSpecification);
    map.setPaintProperty(DOTS, 'circle-color', ['to-color', ['coalesce', ['feature-state', 'c'], ['match', ['get', 'g'], ...RAIL_GROUP_COLOURS.flatMap((c, i) => [i, c]), '#cfd6e0']]] as unknown as ExpressionSpecification);
    // Shown once coloured, at the line's opacity there (a) × the layer's.
    const shown = (o: number): ExpressionSpecification => ['case', ['boolean', ['feature-state', 'k'], false], ['*', o, ['number', ['feature-state', 'a'], 1]], 0];
    map.setPaintProperty(DOTS, 'circle-opacity', shown(r.opacity));
    map.setPaintProperty(DOTS, 'circle-stroke-opacity', shown(r.opacity * r.stopOutlineOpacity));
    map.setPaintProperty(DOTS, 'circle-stroke-color', r.stopOutline);
  }

  /** The stops shown and not yet coloured (with `all`, the colouring changed: every stop again)
   * take the look of the rail line at them (`colourAt`: lng, lat → colour and opacity; null where
   * a rail tile is drawn but no line near; undefined where no rail tile is drawn) and show; where
   * no line is, their service group's once the rail tiles in view are all drawn (`settled`), until
   * then they wait. A generator: it yields every few stops (idle.ts runs it between frames). */
  *recolour(colourAt: (lng: number, lat: number) => Look | null | undefined, all: boolean, settled: boolean): Generator<void, void> {
    const map = this.map;
    if (!map.getLayer(DOTS) || map.getLayoutProperty(DOTS, 'visibility') === 'none') return;
    if (all) this.coloured.clear();
    // Shown at this zoom (the dots' filter, spaced(STOP_PX)).
    const minZ = map.getZoom() - Math.log2(STOP_PX);
    const layer = { sourceLayer: STATION_LAYER };
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
      map.setFeatureState({ source: 'stations', id, ...layer }, { c: c?.c ?? null, a: c?.a ?? 1, k: true });
      this.coloured.set(id, c);
    }
  }
}
