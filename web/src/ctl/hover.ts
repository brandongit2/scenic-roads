// Hover and clicks on the map: what's under the cursor (a landmark, a ferry, the nearest road or
// rail line, a building, a region, the areas there, else the water) in the bottom bar, with the
// road or line highlighted whole; a list item hovered shown the same way; and a click opening a
// landmark's popup or selecting a road.
import type * as maplibregl from 'maplibre-gl';
import { getRoadWays, getWay, peekWay, roadWays } from '../api';
import { buildingAt, hiddenByBuilding, setHovered as setBuildingHover, summarise as buildingSummary, type Ray } from '../buildings';
import * as cam3d from '../camera3d';
import type { Depth } from '../depth';
import type { Available } from './available';
import { loadDetail, peekDetail, refKey } from '../details';
import type { Ferries } from '../ferries';
import { AREA_LAYERS, POINT_LAYERS, summariseFeature, withDetails, type FeatureSummary, type Overlays } from '../overlays';
import { regionAt, type RegionLayers } from '../regions';
import type { HoverInfo, RoadLayer } from '../roads/layer';
import type { Store } from '../state';
import type { RegionsPanel } from '../ui/regions';
import type { Strip } from '../ui/strip';
import type { ViewshedTool } from '../ui/viewshed';
import { waterNameAt } from '../watername';
import type { Marks } from './marks';
import type { RoadsRail } from './roads';

/** A list item hovered (Drives, Rides, Rail lines, Sights, In view): a road or rail line near a
 * point (of a way, its geometry ringed when small on screen), a landmark or area, or a ferry. */
export type ListItem =
  | { layer: RoadLayer; at: [number, number]; way?: number; geom?: [number, number][][] }
  | { feature: { layer: string; props: Record<string, any>; lngLat: [number, number] } }
  | { ferryAt: [number, number] };

export interface HoverParts {
  strip: Strip;
  overlays: Overlays;
  ferries: Ferries;
  rd: RoadsRail;
  /** The depth (made later in main.ts). */
  depth: () => Depth;
  viewshed: ViewshedTool;
  /** The data available layers: while on, a click not on a point or a ferry shows the data there. */
  available: Available;
  regionLayers: RegionLayers;
  regions: RegionsPanel;
  marks: Marks;
  /** Whether the camera is flying along a road (Drive) or moving: nothing is picked then. */
  driving: () => boolean;
  moving: () => boolean;
}

/** With the HUD off (I) the map is only for looking: nothing is hovered or selected. */
const hudOff = () => document.body.classList.contains('hud-off');

export class Hover {
  /** The road or rail line hovered. */
  hovered: HoverInfo | null = null;
  /** Last cursor position on the map (terrain-aware), for the Street View shortcut. */
  cursorLL: maplibregl.LngLat | null = null;
  private pickAt: { x: number; y: number } | null = null;
  /** The feature in the bottom bar, whose details are awaited. */
  private featKey = '';
  private hoverAreas: FeatureSummary[] = [];
  /** The pick a water answer (depth.water) is for: one newer, or the cursor gone, drops it. */
  private waterTok = 0;
  /** The newest water pick still to ask about, and whether a probe is out: one at a time, the
   * newest pick next, so fast moves don't queue probes in the depth workers. */
  private waterAt: { tok: number; pt: { x: number; y: number }; ll: maplibregl.LngLat } | null = null;
  private waterBusy = false;
  /** The hovered building highlighted (null: none), when it changed. */
  private hoveredBuilding: unknown = null;
  private listTok = 0;

  constructor(private map: maplibregl.Map, private store: Store, private p: HoverParts) {
    map.on('mousemove', (e) => {
      if (!this.pickAt) requestAnimationFrame(this.pick);
      this.pickAt = { x: e.point.x, y: e.point.y };
      this.cursorLL = e.lngLat;
    });
    map.getCanvas().addEventListener('mouseleave', () => (this.cursorLL = null));
  }

  /** A hovered stop, site or area: shown at once, then again with its details when they arrive. */
  private showFeat(f: FeatureSummary, areas: FeatureSummary[]) {
    const key = f.ref ? refKey(f.ref) : '';
    this.featKey = key;
    const d = f.ref ? peekDetail(f.ref) : undefined;
    this.p.strip.showFeature(d !== undefined ? withDetails(f, d) : f, areas);
    if (f.ref && d === undefined) {
      loadDetail(f.ref).then((dd) => {
        if (this.featKey === key) this.p.strip.showFeature(withDetails(f, dd), areas);
      });
    }
  }

  private pick = () => {
    const { map, store } = this;
    const { strip, overlays, ferries, viewshed } = this.p;
    const { roads, rails } = this.p.rd;
    const pt = this.pickAt;
    this.pickAt = null;
    // Not while the camera moves: the map's queries project through the 3D terrain, and a
    // hover readout mid-gesture is of no use.
    if (!pt || this.p.driving() || this.p.moving() || hudOff()) return;
    this.featKey = '';
    this.waterTok++; // (a water answer still to come is for an older pick)
    // Markers first (small targets), then the nearest road or rail line, then the highlighted
    // areas under the cursor.
    const feats = overlays.hoverAt(pt);
    // Ferries next (car ferries are in the road layer too; their ferry details say more).
    const ferry = feats.point ? null : ferries.hoverAt(pt);
    if (ferry) {
      this.hoverBuilding(null);
      this.hovered = null;
      roads.setHover(null);
      rails.setHover(null);
      map.getCanvas().style.cursor = viewshed.active ? 'crosshair' : 'pointer';
      this.hoverAreas = feats.areas;
      return strip.showFeature(ferry, feats.areas);
    }
    const hr = feats.point ? null : roads.pick(pt.x, pt.y);
    const hl = feats.point || !store.s.rail.on ? null : rails.pick(pt.x, pt.y);
    let hovered = hr && hl ? (hl.px <= hr.px ? hl : hr) : hr ?? hl;
    // A road or rail line behind a building gives way to it (the lines are picked within a few
    // pixels of the cursor, hidden or not).
    const exag = store.s.terrain.on ? store.s.terrain.exaggeration : 0;
    const ray: Ray = { at: (x, y, e) => cam3d.rayAt(map, x, y, e), camera: cam3d.cameraAltitude(map) };
    if (hovered && store.s.buildings.on && hiddenByBuilding(map, hovered.lngLat, store.s.buildings, exag, ray)) hovered = null;
    this.hovered = hovered;
    const layer = hovered && hovered === hl ? rails : roads;
    (layer === rails ? roads : rails).setHover(null);
    // Highlight the whole road (or line), not just the way under the cursor (fetched once per road).
    const road = hovered ? roadWays(hovered.way) : undefined;
    layer.setHover(hovered, road ?? null);
    if (hovered && !road) {
      getRoadWays(hovered.way, hovered.lngLat).then((set) => {
        const now = this.hovered;
        if (set && now && set.has(now.way)) layer.setHover(now, set);
      });
    }
    map.getCanvas().style.cursor = viewshed.active ? 'crosshair' : hovered || feats.point ? 'pointer' : '';
    this.hoverAreas = feats.areas;
    if (!hovered) {
      // A building, when no marker, road or rail line answers: the areas it's in as chips (a whole
      // old town is a heritage area, which would otherwise hide every building in it).
      const bf = feats.point ? null : buildingAt(map, pt, store.s.buildings, exag, ray)?.f ?? null;
      this.hoverBuilding(bf);
      if (bf) {
        roads.setHover(null);
        rails.setHover(null);
        return strip.showFeature(buildingSummary(bf), feats.areas);
      }
      // A region (Show regions on map), before the areas inside it.
      const ll = this.cursorLL ?? map.unproject([pt.x, pt.y]);
      const rg = feats.point ? null : regionAt(this.p.regionLayers.shown, [ll.lng, ll.lat]);
      if (rg) {
        roads.setHover(null);
        rails.setHover(null);
        return strip.showRegion(this.p.regions.regionInfo(rg.properties.region, rg.properties.name));
      }
      const f = feats.point ?? feats.areas[0];
      if (f) return this.showFeat(f, feats.areas);
      // Nothing else here: water, if the depth has it (else the strip goes idle).
      this.waterAt = { tok: this.waterTok, pt, ll: this.cursorLL ?? map.unproject([pt.x, pt.y]) };
      if (!this.waterBusy) void this.askWater();
      return;
    }
    this.hoverBuilding(null);
    const hv = hovered;
    const known = peekWay(hv.way);
    strip.show(hv, known === undefined ? 'loading' : known, this.hoverAreas);
    if (known === undefined) {
      getWay(hv.way, hv.lngLat).then((info) => {
        if (this.hovered?.way === hv.way) strip.show(this.hovered, info, this.hoverAreas);
      });
    }
  };

  private async askWater() {
    this.waterBusy = true;
    try {
      while (this.waterAt) {
        const q = this.waterAt;
        this.waterAt = null;
        const w = await this.p.depth().water(q.ll);
        if (q.tok !== this.waterTok) continue;
        if (!w) this.p.strip.show(null, null);
        else {
          const n = waterNameAt(this.map, q.pt, w.water);
          this.p.strip.showWater(w, n.name, n.kind);
        }
      }
    } finally {
      this.waterBusy = false;
    }
  }

  /** The hovered building highlighted (null: none). */
  hoverBuilding(f: maplibregl.MapGeoJSONFeature | null) {
    const key = f ? JSON.stringify(f.geometry) : null;
    if (key === this.hoveredBuilding) return;
    this.hoveredBuilding = key;
    setBuildingHover(this.map, f);
  }

  /** No hover: the cursor left the map, or the HUD went off. */
  unhover() {
    this.pickAt = null;
    this.waterTok++;
    this.hovered = null;
    this.p.rd.roads.setHover(null);
    this.p.rd.rails.setHover(null);
    this.hoverBuilding(null);
    this.p.strip.show(null, null);
  }

  /** A list item hovered: the bottom bar shows it as a hover on the map would, and a road or line
   * too small on screen to see at a glance gets a ring. */
  list(x: ListItem | null) {
    const { strip, marks } = this.p;
    const tok = ++this.listTok;
    if (!x) {
      marks.ringAround(null);
      strip.show(null, null);
      return;
    }
    if ('feature' in x) {
      marks.ringAround(null);
      const f = summariseFeature(x.feature.layer, x.feature.props, x.feature.lngLat);
      if (f) this.showFeat(f, []);
      return;
    }
    if ('ferryAt' in x) {
      marks.ringAround(null);
      const p = this.map.project(x.ferryAt);
      const f = this.p.ferries.hoverAt({ x: p.x, y: p.y });
      return f ? strip.showFeature(f, []) : strip.show(null, null);
    }
    marks.ringAround(x.geom ?? null);
    // The road's ways where known (a drive spans several), so a road crossing there isn't taken.
    const set = x.way !== undefined ? roadWays(x.way) : undefined;
    const ok = set ? (w: number) => set.has(w) : x.way !== undefined ? (w: number) => w === x.way : undefined;
    const hv = x.layer.pickNear(x.at[0], x.at[1], 4, ok) ?? x.layer.pickNear(x.at[0], x.at[1], 4);
    if (!hv) return strip.show(null, null);
    const known = peekWay(hv.way);
    strip.show(hv, known === undefined ? 'loading' : known, []);
    if (known === undefined) {
      getWay(hv.way, hv.lngLat).then((info) => {
        if (this.listTok === tok) strip.show(hv, info, []);
      });
    }
  }

  /** A click on the map: the viewshed's eye placed, a landmark's or ferry's popup; while the data
   * available layers are on, the data there (a layer of every place: roads and areas wait for it to
   * be off); else a World Heritage line, the nearest road or rail line selected, or an area's popup. */
  click(e: maplibregl.MapMouseEvent) {
    const { overlays, ferries, viewshed, rd, available } = this.p;
    if (hudOff()) return;
    if (viewshed.active) {
      viewshed.run([e.lngLat.lng, e.lngLat.lat]);
      return;
    }
    if (overlays.click(e.point, POINT_LAYERS)) {
      available.closePopup();
      return;
    }
    if (ferries.click(e.point, e.lngLat)) {
      overlays.closePopup();
      available.closePopup();
      return;
    }
    ferries.closePopup();
    if (available.click(e.lngLat)) {
      overlays.closePopup();
      return;
    }
    const hr = rd.roads.pick(e.point.x, e.point.y);
    const hl = this.store.s.rail.on ? rd.rails.pick(e.point.x, e.point.y) : null;
    const hv = hr && hl ? (hl.px <= hr.px ? hl : hr) : hr ?? hl;
    // A World Heritage line (a canal, a wall) against the roads and rail: the nearer wins.
    const wl = overlays.whsLineAt(e.point);
    if (wl && (!hv || wl.px <= hv.px) && overlays.click(e.point, ['whs-line'])) return;
    if (hv) {
      overlays.closePopup();
      this.store.set({ selected: { way: hv.way, at: hv.lngLat }, stretch: null });
      return;
    }
    overlays.click(e.point, AREA_LAYERS);
  }
}
