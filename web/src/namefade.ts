// Landmark names easing with their dots. The names are MapLibre symbol layers, and their opacity
// was a paint expression of the prominence scale: MapLibre snaps such a change (it never eases a
// data-driven paint property), so the names jumped while their dots eased over 350 ms, and it lays
// the whole source out again for it (every point tile of the eight sources, whenever the view
// settled). Now a name's opacity is its feature state `o` (basemap.ts nameOpacityPaint), else as
// its tile was made: the landmarks worker writes each name's opacity on the scale of the moment
// into the tile, and sends the tile's named points with it. While the dots ease to a new scale,
// the names that can show at this zoom (and one zoom in) get theirs every other frame, from the
// same eased scale as the dots (dots.ts nameScale); a hundred or so in a dense view. A tile made
// before the first scale has no opacity in it (its names hidden): they fade in once there is one.
import type { Map as MLMap, OverscaledTileID } from 'maplibre-gl';
import { LABEL_SPACING_PX, POINT_TILE_LAYER, nameOpacity, type NameScale } from './basemap';
import type { TileNames } from './landmarks.worker';

/** Names are eased for points whose name shows within this many zooms in. */
const MARGIN_Z = 1;
/** Tiles remembered per source (the most recent), beyond those MapLibre holds in view. */
const MAX_TILES = 600;
/** The fade-in of names whose tile was made without their opacity, as long as the dots' ease. */
const FADE_IN_MS = 350;
const easeInOut = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);

interface Scales {
  nameScale(now?: number): NameScale | null;
  easing(now?: number): boolean;
}
type TileManagerLike = { getVisibleCoordinates(symbolLayer?: boolean): OverscaledTileID[]; getSource(): { maxzoom: number } };

export class NameFader {
  /** Per source: tile ("z/x/y") → its named points. */
  private tiles = new Map<string, Map<string, TileNames>>();
  /** Per source: point id → the opacity last set. */
  private last = new Map<string, Map<number, number>>();
  private raf = 0;
  private skip = false;
  /** Tiles whose names are fading in (made before the first scale): since when. */
  private fading = new Map<TileNames, number>();

  constructor(private map: MLMap, private dots: Scales) {}

  /** A point tile's named points, as the worker made it. */
  tile(src: string, z: number, x: number, y: number, names: TileNames) {
    if (!names.ids.length) return;
    let m = this.tiles.get(src);
    if (!m) this.tiles.set(src, (m = new Map()));
    const key = `${z}/${x}/${y}`;
    m.delete(key);
    m.set(key, names);
    if (m.size > MAX_TILES) m.delete(m.keys().next().value!);
    // The scale may have moved on since the tile was asked for (it was made on the one of then).
    if (this.dots.nameScale()) this.kick();
  }

  /** A new scale (the dots ease to it): the names follow. */
  kick() {
    if (this.raf === 0) this.raf = requestAnimationFrame(this.frame);
  }

  private frame = () => {
    this.raf = 0;
    const now = performance.now();
    const easing = this.dots.easing(now) || this.fading.size > 0;
    // Every other frame while easing (opacity in 16 ms steps is as smooth); always the last.
    this.skip = easing && !this.skip;
    if (!this.skip) {
      const sc = this.dots.nameScale(now);
      if (sc) this.update(sc, now);
    }
    if (easing || this.fading.size > 0) this.raf = requestAnimationFrame(this.frame);
  };

  /** Every name that can show in view (and within MARGIN_Z zooms in) set to its opacity on `sc`. */
  private update(sc: NameScale, now: number) {
    const managers = (this.map as unknown as { style?: { tileManagers?: Record<string, TileManagerLike> } }).style?.tileManagers;
    if (!managers) return;
    const zmax = this.map.getZoom() + MARGIN_Z - Math.log2(LABEL_SPACING_PX);
    const shown = new Set<TileNames>();
    for (const [src, tiles] of this.tiles) {
      const tm = managers[src];
      if (!tm) continue;
      let last = this.last.get(src);
      if (!last) this.last.set(src, (last = new Map()));
      const seen = new Set<string>();
      // Past the source's deepest zoom MapLibre slices that tile's data into deeper tiles: the
      // names are the ancestor's.
      const zs = tm.getSource().maxzoom;
      for (const id of tm.getVisibleCoordinates(true)) {
        const c = id.canonical, d = Math.max(0, c.z - zs);
        const key = `${c.z - d}/${c.x >> d}/${c.y >> d}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const n = tiles.get(key);
        if (!n) continue;
        shown.add(n);
        let f = 1;
        if (!n.scaled) {
          let t0 = this.fading.get(n);
          if (t0 === undefined) this.fading.set(n, (t0 = now));
          const t = (now - t0) / FADE_IN_MS;
          if (t < 1) f = easeInOut(t);
          else (n.scaled = true), this.fading.delete(n);
        }
        for (let i = 0; i < n.ids.length; i++) {
          if (n.mz[i] > zmax) continue;
          const o = Math.round(nameOpacity(n.fa[i], n.ia[i], sc) * f * 250) / 250;
          if (last.get(n.ids[i]) === o) continue;
          last.set(n.ids[i], o);
          this.map.setFeatureState({ source: src, sourceLayer: POINT_TILE_LAYER, id: n.ids[i] }, { o });
        }
      }
    }
    // A tile out of view is done fading in (back in view, the view's next scale sets its names).
    for (const n of this.fading.keys()) if (!shown.has(n)) (n.scaled = true), this.fading.delete(n);
  }
}
