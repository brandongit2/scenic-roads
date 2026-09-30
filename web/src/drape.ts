// Draped textures, redrawn at a steady pace. With 3D terrain MapLibre draws the flat layers (fills,
// lines, hill-shading, tint, trees…) into a texture per terrain tile and drapes those. It drops a
// tile's textures, to redraw them all in the next frame, whenever a tile of any source under it
// loads (even of a source with nothing draped: the stops and sights, the rail stops), and when a
// source first has tiles under it or a layer enters its zoom range; only textures whose source tiles
// merely changed wait, one a frame. Turning a tilted view brings in tiles of eight draped sources at
// their own pace, a hundred a second: frames redrew up to twenty textures (0.3–0.5 ms of GPU each)
// and missed the display's refresh. Here:
//  - A tile of a source with no draped layer shown leaves the textures alone.
//  - The other drops wait for the next frame as before, but while the camera moves at most
//    MOVING_BUDGET existing textures are redrawn a frame, whatever the reason (the others keep
//    theirs a few frames longer, then follow); a terrain tile without textures is drawn at once. At
//    rest, as MapLibre.
import type { Map as MLMap } from 'maplibre-gl';

const MOVING_BUDGET = 2;
/** Layer types MapLibre draws into the draped textures (render_to_texture.ts LAYERS_TO_TEXTURES). */
const DRAPED = new Set(['background', 'fill', 'line', 'raster', 'hillshade', 'color-relief']);

type TileID = { key: string; equals(o: TileID): boolean; isChildOf(o: TileID): boolean };
type Tile = { tileID: TileID; rttObjects: unknown[]; releaseRTT(painter: unknown): void; __drape?: boolean };
type TerrainTiles = { _tiles: Record<string, Tile>; releaseRTT(id: TileID): void; __drape?: boolean };
type RTT = {
  painter: { options?: { moving?: boolean } };
  _lastPrepareZoom: number;
  needsFollowUpFrame: boolean;
  prepareForRender(style: unknown, zoom: number): void;
  __drape?: boolean;
};
type Style = { _order: string[]; _layers: Record<string, { type: string; source?: string; isHidden(z: number): boolean }> };

export function pacedDrapes(map: MLMap) {
  const m = map as unknown as {
    terrain?: { tileManager?: TerrainTiles } | null;
    painter: { renderToTexture?: RTT | null };
    style: Style;
    _handleTerrainDataEvent?: (e: { dataType?: string; sourceId?: string; tile?: unknown }, id: string) => void;
  };
  /** Terrain tiles whose textures are to be redrawn (by key). */
  const dirty = new Set<string>();
  /** Within prepareForRender: textures that may still be redrawn this frame (Infinity at rest). */
  let budget = Infinity;
  let pacing = false;
  let deferred = false;
  let quiet = false;

  const draped = (sourceId: string | undefined) => {
    if (!sourceId) return true;
    const st = m.style, z = map.getZoom();
    for (const id of st._order) {
      const l = st._layers[id];
      if (l.source === sourceId && DRAPED.has(l.type) && !l.isHidden(z)) return true;
    }
    return false;
  };

  const install = () => {
    const tm = m.terrain?.tileManager;
    const rtt = m.painter.renderToTexture;
    if (!tm || !rtt) return;
    // A terrain tile's release: counted against the frame's budget while pacing.
    const anyTile = Object.values(tm._tiles)[0];
    const tileProto = anyTile && (Object.getPrototypeOf(anyTile) as Tile);
    if (tileProto && !Object.prototype.hasOwnProperty.call(tileProto, '__drape')) {
      tileProto.__drape = true;
      const release = tileProto.releaseRTT;
      tileProto.releaseRTT = function (this: Tile, painter: unknown) {
        if (pacing && this.rttObjects.length) {
          if (budget <= 0) {
            deferred = true;
            return;
          }
          budget--;
        }
        release.call(this, painter);
      };
    }
    // Drops for a source tile (MapLibre's data event): noted, done at the next frame's start.
    const tmProto = Object.getPrototypeOf(tm) as TerrainTiles;
    if (!Object.prototype.hasOwnProperty.call(tmProto, '__drape')) {
      tmProto.__drape = true;
      tmProto.releaseRTT = function (this: TerrainTiles, id: TileID) {
        if (quiet) return;
        for (const key in this._tiles) {
          const t = this._tiles[key].tileID;
          if (t.equals(id) || t.isChildOf(id) || id.isChildOf(t)) dirty.add(key);
        }
        map.triggerRepaint();
      };
    }
    const rttProto = Object.getPrototypeOf(rtt) as RTT;
    if (!Object.prototype.hasOwnProperty.call(rttProto, '__drape')) {
      rttProto.__drape = true;
      const prepare = rttProto.prepareForRender;
      rttProto.prepareForRender = function (this: RTT, style: unknown, zoom: number) {
        const moving = zoom !== this._lastPrepareZoom || !!this.painter.options?.moving;
        budget = moving ? MOVING_BUDGET : Infinity;
        deferred = false;
        pacing = true;
        try {
          const tiles = m.terrain?.tileManager?._tiles ?? {};
          for (const key of [...dirty]) {
            const t = tiles[key];
            if (!t || !t.rttObjects.length) {
              dirty.delete(key);
              continue;
            }
            if (budget <= 0) {
              deferred = true;
              break;
            }
            t.releaseRTT(this.painter);
            dirty.delete(key);
          }
          prepare.call(this, style, zoom);
        } finally {
          pacing = false;
        }
        if (deferred || dirty.size) this.needsFollowUpFrame = true;
      };
    }
    // Tiles of sources with nothing draped: no drops.
    const handle = m._handleTerrainDataEvent;
    if (handle && !(handle as { __drape?: boolean }).__drape) {
      const f = function (this: unknown, e: { dataType?: string; sourceId?: string; tile?: unknown }, id: string) {
        quiet = e.dataType !== 'style' && !!e.tile && !draped(e.sourceId);
        try {
          handle.call(this, e, id);
        } finally {
          quiet = false;
        }
      };
      (f as { __drape?: boolean }).__drape = true;
      m._handleTerrainDataEvent = f;
    }
  };
  // The terrain (and its texture renderer) comes and goes with the 3D setting.
  map.on('terrain', install);
  map.on('render', install);
  install();
}
