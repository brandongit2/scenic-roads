// The data available layers on the map (docs/steps.md §14): the one part of them MapLibre's, a
// raster source and layer over the server's painted tiles, and the polar caps' overlay
// (polarcaps.ts). What is shown, from where, comes from the controller (ctl/available.ts) as a URL
// template and an opacity; nothing here reads the layers' meaning (available.ts).
import type { Map as MLMap, RasterTileSource } from 'maplibre-gl';

const SOURCE = 'available';
const LAYER = 'available';

/** What the Mercator layer draws: its tiles' URL template, their finest zoom (overzoomed past it),
 * and its opacity; null: nothing. */
export type AvailableDraw = { url: string; maxzoom: number; opacity: number } | null;

export class AvailableMap {
  private url = '';
  private maxzoom = -1;

  /** `before`: the layer it goes under (the map's lines and labels above it; the hill-shading
   * under, so the classes keep their colours). */
  constructor(private map: MLMap, private before: string) {}

  show(d: AvailableDraw) {
    const m = this.map;
    if (!d) {
      if (m.getLayer(LAYER)) m.removeLayer(LAYER);
      if (m.getSource(SOURCE)) m.removeSource(SOURCE);
      this.url = '';
      return;
    }
    const src = m.getSource(SOURCE) as RasterTileSource | undefined;
    if (src && this.maxzoom !== d.maxzoom) {
      m.removeLayer(LAYER);
      m.removeSource(SOURCE);
    } else if (src && this.url !== d.url) {
      src.setTiles([d.url]);
      this.url = d.url;
    }
    if (!m.getSource(SOURCE)) {
      // (Tiles of 256 px, z0 to its finest; 204 where nothing is coloured: an empty tile.)
      m.addSource(SOURCE, { type: 'raster', tiles: [d.url], tileSize: 256, minzoom: 0, maxzoom: d.maxzoom, attribution: '' });
      m.addLayer(
        // (Nearest: a pixel is one set of pieces, its colour exact; no fade, as a draped raster
        // drawn mid-fade keeps it.)
        { id: LAYER, type: 'raster', source: SOURCE, paint: { 'raster-opacity': d.opacity, 'raster-resampling': 'nearest', 'raster-fade-duration': 0 } },
        m.getLayer(this.before) ? this.before : undefined,
      );
      this.url = d.url;
      this.maxzoom = d.maxzoom;
    }
    if (m.getPaintProperty(LAYER, 'raster-opacity') !== d.opacity) m.setPaintProperty(LAYER, 'raster-opacity', d.opacity);
  }
}
