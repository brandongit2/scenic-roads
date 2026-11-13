// The 3D buildings' controller: the meshes' layers (buildings.ts), the far buildings' field
// (bldfield.ts, task #102) and colour by height, the shared colour scale over the buildings in view
// (their tiles' heights, read when the map is idle: buildings.onHeights), auto-fit to its
// percentiles as the roads' is.
import type { Map as MLMap } from 'maplibre-gl';
import type { Meta } from '../api';
import { spread } from '../autofit';
import type { FieldLayer } from '../bldfield';
import { addBuildings, applyBuildings, heightColourFn, heritageChanged, onHeights, type HeightLook } from '../buildings';
import { hexRgb } from '../palettes';
import type { RoadLayer } from '../roads/layer';
import type { Dist } from '../roads/stats';
import type { AppState, Store } from '../state';
import type { LayersCard } from '../ui/layers';
import { cdfOf } from '../ui/scale';

export interface BuildingsHost {
  /** Whether the basemap style is in (the buildings' layers with it). */
  styleReady: () => boolean;
  /** The road and rail layers, whose bridges are drawn after the buildings while they stand in 3D
   * (roads/layer.ts bridgeLayer), with their bridge layers' ids. */
  bridges: () => (readonly [RoadLayer, string])[];
}

export class Buildings {
  /** The far buildings' field, once loaded (null: none, or not yet). */
  field: FieldLayer | null = null;
  private fieldReady: Promise<FieldLayer> | null = null;
  /** `?bldfield=off` leaves the field out, `?bldfield=<url>` takes its tiles from <url>, a bare
   * `?bldfield` the server's (either whatever the catalogue says). */
  private fieldParam = new URLSearchParams(location.search).get('bldfield');
  private dist: Dist | null = null;
  private look: HeightLook;
  /** A palette previewed (the ramp list hovered), else null. */
  private preview: string | null = null;
  private layers: LayersCard | null = null;

  /** The field starts loading where the catalogue has its layer (`buildings-field`); without it
   * the meshes alone (z14), as before the field. */
  constructor(private map: MLMap, private store: Store, meta: Meta, private host: BuildingsHost) {
    this.look = { range: [...store.s.buildings.height.range], cdf: null };
    if (this.wantField(meta)) void this.loadField();
    onHeights(map, (d) => {
      this.dist = d;
      if (host.styleReady() && store.s.buildings.colour === 'height') this.apply(store.s);
    });
  }

  private wantField(m: Meta) {
    return this.fieldParam !== 'off' && (this.fieldParam !== null || !!m.layers?.['buildings-field']);
  }

  private loadField() {
    return (this.fieldReady ??= import('../bldfield').then((m) => (this.field = new m.FieldLayer(this.fieldParam || null))));
  }

  /** The field's layer, once loaded, right after the extruded buildings (when they're there). */
  private addField() {
    void this.fieldReady?.then((f) => {
      const ids = this.map.getStyle().layers.map((l) => l.id);
      const next = ids[ids.indexOf('buildings') + 1];
      if (!this.map.getLayer(f.id) && ids.includes('buildings')) this.map.addLayer(f, next);
      this.apply(this.store.s);
    });
  }

  /** The Layers card's buildings section: its palette previews and height scale. */
  wire(layers: LayersCard) {
    this.layers = layers;
    layers.buildings.onPalettePreview = (k) => {
      this.preview = k;
      if (this.host.styleReady()) this.apply(this.store.s);
    };
  }

  /** The buildings' layers added, the style in and the catalogue having them (where they go:
   * main.ts attach). */
  add() {
    addBuildings(this.map, 'water-name-line', 'boundary-county');
    heritageChanged(this.map, this.store.s.overlays.heritage);
    this.addField();
  }

  /** A new catalogue: the buildings once it has them, the field once it has its layer. */
  catalogue(m: Meta) {
    const ready = this.host.styleReady();
    if (m.layers?.buildings && ready && !this.map.getSource('bld')) {
      addBuildings(this.map, 'water-name-line', 'boundary-county');
      this.apply(this.store.s);
      heritageChanged(this.map, this.store.s.overlays.heritage);
      this.addField();
    }
    if (!this.fieldReady && this.wantField(m) && ready && this.map.getLayer('buildings')) {
      this.loadField();
      this.addField();
    }
  }

  private lookNow(): HeightLook {
    const sc = this.store.s.buildings.height;
    const d = this.dist;
    const range: [number, number] = !sc.auto ? [...sc.range] : d && d.total > 0 ? spread(d.quantile(sc.fit[0] / 100), d.quantile(sc.fit[1] / 100), 4) : this.look.range;
    return { range, cdf: sc.equalize ? cdfOf(d, range) : null };
  }

  /** The buildings' settings applied (the terrain's exaggeration and light are theirs too). */
  apply(s: AppState) {
    const map = this.map;
    // Bridges and elevated rail drawn after the buildings while they stand in 3D: placed after
    // them, before the first symbol layer.
    const apart = !!map.getLayer('buildings') && s.buildings.on && !s.buildings.flat && s.terrain.on;
    for (const [l, id] of this.host.bridges()) {
      if (!map.getLayer(id) && map.getLayer('buildings')) map.addLayer(l.bridgeLayer(id), 'water-name-line');
      if (l.bridgesApart !== apart) {
        l.bridgesApart = apart;
        map.triggerRepaint();
      }
    }
    this.look = this.lookNow();
    const b = this.preview ? { ...s.buildings, height: { ...s.buildings.height, palette: this.preview } } : s.buildings;
    applyBuildings(map, b, s.terrain.on ? s.terrain.exaggeration : 0, s.terrain.light, this.look);
    const field = this.field;
    if (field) {
      field.opacity = b.opacity;
      field.hk = b.scale > 0 ? b.scale : Math.max(1, s.terrain.on ? s.terrain.exaggeration : 0);
      field.on = b.on && !b.flat && s.terrain.on;
      // The plain colour; by height, the meshes' colours (the field plain when coloured by source:
      // the owner's call).
      field.colour = hexRgb(b.plain);
      const hc = heightColourFn(b, this.look);
      field.setHeightColour(hc?.key ?? '', hc?.colour ?? null);
    }
    this.layers?.buildings.updateHeights(this.dist, this.look);
  }
}
