// The data available layers on the map (docs/steps.md §14): the one part of them MapLibre's, a
// raster-dem source over the server's tiles of values and a colour-relief layer colouring them on
// the GPU by the ramp's stops (available.ts rampStops), as the terrain tint is coloured; the polar
// caps' overlay (polarcaps.ts) takes this layer's ramp. What is shown, from where and in which
// colours comes from the controller (ctl/available.ts); nothing here reads the layers' meaning. In
// the native port: a ground layer painted into the drapes from the same tiles and stops.
import type { ExpressionSpecification, Map as MLMap, RasterDEMTileSource } from 'maplibre-gl';

const SOURCE = 'available';
/** The colour-relief layer (the polar caps' overlay reads its ramp: polarglobe.ts). */
export const AVAILABLE_LAYER = 'available';

/** What the Mercator layer draws: its tiles' URL template, their finest zoom (overzoomed past it),
 * which values they carry (`scale`: the layer's id, whose ramp `stops` is), its opacity and its
 * ramp's stops; null: nothing. */
export type AvailableDraw = { url: string; maxzoom: number; scale: string; opacity: number; stops: [number, string][] } | null;

export class AvailableMap {
  private url = '';
  private maxzoom = -1;
  private scale = '';
  private stops = '';

  /** `before`: the layer it goes under (the map's lines and labels above it). */
  constructor(private map: MLMap, private before: string) {}

  show(d: AvailableDraw) {
    const m = this.map;
    if (!d) {
      if (m.getLayer(AVAILABLE_LAYER)) m.removeLayer(AVAILABLE_LAYER);
      if (m.getSource(SOURCE)) m.removeSource(SOURCE);
      this.url = this.stops = '';
      return;
    }
    const src = m.getSource(SOURCE) as RasterDEMTileSource | undefined;
    // (Another layer's values: its tiles dropped, not drawn by the new ramp while the new ones
    // load (a grid spacing's metres by a year's ramp: one colour everywhere); the switch's or a new
    // version's tiles stay until replaced, their values on the same axis.)
    if (src && (this.maxzoom !== d.maxzoom || this.scale !== d.scale)) {
      m.removeLayer(AVAILABLE_LAYER);
      m.removeSource(SOURCE);
    } else if (src && this.url !== d.url) {
      src.setTiles([d.url]);
      this.url = d.url;
    }
    if (!m.getSource(SOURCE)) {
      // (Tiles of 256 px, z0 to its finest, Terrarium-coded values; 204 where there's none: no
      // tile.)
      m.addSource(SOURCE, { type: 'raster-dem', encoding: 'terrarium', tiles: [d.url], tileSize: 256, minzoom: 0, maxzoom: d.maxzoom, attribution: '' });
      m.addLayer(
        // (Nearest: a pixel is one piece's value, never a blend of two or of one and none.)
        { id: AVAILABLE_LAYER, type: 'color-relief', source: SOURCE, paint: { 'color-relief-opacity': d.opacity, resampling: 'nearest' } as never },
        m.getLayer(this.before) ? this.before : undefined,
      );
      this.url = d.url;
      this.maxzoom = d.maxzoom;
      this.scale = d.scale;
      this.stops = '';
    }
    const key = JSON.stringify(d.stops);
    if (key !== this.stops) {
      this.stops = key;
      m.setPaintProperty(AVAILABLE_LAYER, 'color-relief-color', ['interpolate', ['linear'], ['elevation'], ...d.stops.flat()] as ExpressionSpecification, { validate: false });
    }
    if (m.getPaintProperty(AVAILABLE_LAYER, 'color-relief-opacity') !== d.opacity) m.setPaintProperty(AVAILABLE_LAYER, 'color-relief-opacity', d.opacity);
  }
}
