// Label placement while the camera moves. MapLibre places the labels again (which show, which
// collide) once the last placement is older than the map's fade (120 ms here): a few milliseconds a
// frame for a few frames, then every label's opacity updated at once, up to 4 ms in dense views,
// every 150 ms or so of a gesture. While the camera moves a placement is kept for MOVING_MS instead
// (MapLibre's default fade), at rest as before; labels still fade in and out in 120 ms.
// Between placements MapLibre also updates the opacities of every symbol layer whenever any of them
// has new tiles (most frames of a gesture: tiles arrive for one source or another); only the layers
// with new tiles need it (fades are timed in the shader), so only they are.
import type { Map as MLMap } from 'maplibre-gl';

const MOVING_MS = 300;

type Placement = {
  commitTime: number;
  stillRecent(now: number, zoom: number): boolean;
  updateLayerOpacities(layer: { id: string }, tiles: unknown[]): void;
  __moving?: boolean;
};
type SymbolIndex = { addLayer(layer: { id: string }, tiles: unknown[], lng: number): boolean; __moving?: boolean };
type Style = {
  placement?: Placement;
  crossTileSymbolIndex?: SymbolIndex;
  _updatePlacement(...args: unknown[]): boolean;
  __moving?: boolean;
};

const own = (o: object, k: string) => Object.prototype.hasOwnProperty.call(o, k);

export function steadierPlacement(map: MLMap, moving: () => boolean) {
  /** Within _updatePlacement: the placement it started with, and the layers with new tiles. */
  let before: Placement | null = null;
  let changed: Set<string> | null = null;
  const patch = () => {
    const style = (map as unknown as { style?: Style }).style;
    const pl = style?.placement, idx = style?.crossTileSymbolIndex;
    if (!style || !pl || !idx) return false;
    const plProto = Object.getPrototypeOf(pl) as Placement;
    if (!own(plProto, '__moving')) {
      plProto.__moving = true;
      const recent = plProto.stillRecent;
      plProto.stillRecent = function (this: Placement, now: number, zoom: number) {
        // (Called first either way: it tracks the zoom.)
        return recent.call(this, now, zoom) || (moving() && this.commitTime + MOVING_MS > now);
      };
      const update = plProto.updateLayerOpacities;
      plProto.updateLayerOpacities = function (this: Placement, layer: { id: string }, tiles: unknown[]) {
        // No new placement committed (the same one) and no new tiles for this layer: as it was.
        if (changed && this === before && !changed.has(layer.id)) return;
        update.call(this, layer, tiles);
      };
    }
    const idxProto = Object.getPrototypeOf(idx) as SymbolIndex;
    if (!own(idxProto, '__moving')) {
      idxProto.__moving = true;
      const add = idxProto.addLayer;
      idxProto.addLayer = function (this: SymbolIndex, layer: { id: string }, tiles: unknown[], lng: number) {
        const r = add.call(this, layer, tiles, lng);
        if (r) changed?.add(layer.id);
        return r;
      };
    }
    const stProto = Object.getPrototypeOf(style) as Style;
    if (!own(stProto, '__moving')) {
      stProto.__moving = true;
      const place = stProto._updatePlacement;
      stProto._updatePlacement = function (this: Style, ...args: unknown[]) {
        before = this.placement ?? null;
        changed = new Set();
        try {
          return place.apply(this, args);
        } finally {
          before = null;
          changed = null;
        }
      };
    }
    return true;
  };
  if (patch()) return;
  const onStyle = () => {
    if (patch()) map.off('styledata', onStyle);
  };
  map.on('styledata', onStyle);
}
