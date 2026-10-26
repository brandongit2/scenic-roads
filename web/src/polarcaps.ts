// The polar caps on the app's map (docs/polar.md; task #601): a pole's cap is drawn (polarglobe.ts)
// while the catalogue lists its layers, `polar-terrain-<pole>` and `polar-slope-<pole>`. Its tiles
// come from /tiles/polar-terrain|polar-slope/<pole>/z/x/y at the layers' versions (their packs' where
// the catalogue gives packs: api.ts packUrl); a new version asks for every tile again. The cap
// follows the map's hillshade layer, the terrain's 3D and exaggeration, the land and water colours
// and the tints by itself (polarGlobeLayer reads them from the map each frame).

import type { Map as MlMap } from 'maplibre-gl';
import { packUrl, packVersion, version } from './api';
import { hostFor } from './hosts';
import { polarLayerName, type Pole, type TileKind } from './polar';
import { polarGlobeLayer, premultiplied, type Rgba } from './polarglobe';

type CatalogueLayer = { maxzoom?: number; version?: string | number };

export type PolarLook = {
  /** The land's colour (CSS). */
  land: () => string;
  /** The water's colour (CSS), or null where the water isn't shown (the land's then). */
  water: () => string | null;
};

/** The caps drawn on `map`, by the catalogue's layers. */
export class PolarCaps {
  private caps = new Map<Pole, { layer: ReturnType<typeof polarGlobeLayer>; v: string; maxZoom: number }>();
  private layers: Record<string, unknown> = {};
  private colour = new Map<string, Rgba>();

  constructor(
    private map: MlMap,
    private look: PolarLook,
    /** The layer the caps go before. */
    private before?: string,
  ) {}

  private premultiplied(c: string): Rgba {
    let v = this.colour.get(c);
    if (!v) this.colour.set(c, (v = premultiplied(c)));
    return v;
  }

  /** A layer's version: the catalogue's, else its file's (meta's versions). */
  private version(name: string): string {
    const l = this.layers[name] as CatalogueLayer | undefined;
    return String(l?.version ?? version(`${name}.tiles`));
  }

  private url(pole: Pole, kind: TileKind, z: number, x: number, y: number): string {
    const v = this.version(polarLayerName(kind, pole));
    return packUrl(`${hostFor('terrain')}/tiles/polar-${kind}/${pole}/${z}/${x}/${y}${v ? `?v=${v}` : ''}`);
  }

  /** The catalogue's layers (at start and for each new catalogue): a cap for each pole listing both. */
  setCatalogue(layers: Record<string, unknown> | undefined) {
    this.layers = layers ?? {};
    for (const pole of ['n', 's'] as Pole[]) {
      const t = this.layers[polarLayerName('terrain', pole)] as CatalogueLayer | undefined;
      const s = this.layers[polarLayerName('slope', pole)] as CatalogueLayer | undefined;
      const have = this.caps.get(pole);
      if (!t || !s) {
        if (have && this.map.getLayer(have.layer.id)) this.map.removeLayer(have.layer.id);
        this.caps.delete(pole);
        continue;
      }
      const v = `${this.version(polarLayerName('terrain', pole))}|${this.version(polarLayerName('slope', pole))}`;
      const maxZoom = Math.min(t.maxzoom ?? 9, s.maxzoom ?? 9);
      if (have && have.maxZoom === maxZoom) {
        if (have.v !== v) have.layer.refresh();
        have.v = v;
        continue;
      }
      if (have && this.map.getLayer(have.layer.id)) this.map.removeLayer(have.layer.id);
      const layer = polarGlobeLayer({
        pole,
        maxZoom,
        url: (kind, z, x, y) => this.url(pole, kind, z, x, y),
        // (A tile no pack holds, a block not built, isn't asked for: '' from meta's packs.)
        stored: (z, x, y) => (['terrain', 'slope'] as TileKind[]).every((k) => packVersion(polarLayerName(k, pole), z, x, y) !== ''),
        land: () => this.premultiplied(this.look.land()),
        water: () => this.premultiplied(this.look.water() ?? this.look.land()),
        hillshade: 'hillshade',
        tints: { elevation: 'tint', slope: 'tint-slope' },
      });
      this.map.addLayer(layer, this.before && this.map.getLayer(this.before) ? this.before : undefined);
      this.caps.set(pole, { layer, v, maxZoom });
    }
  }
}
