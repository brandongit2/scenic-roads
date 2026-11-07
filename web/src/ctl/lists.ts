// The lists' controller: the panel's lists of what's in view (Drives, Rides, Rail lines, Sights,
// and the In view summary's places), what they ask the server for (the ground in view, the
// settings' filters), their items on the map as they're hovered, and the road or line selected
// (from a list or a click: its profile, and a drive's or ride's stretch of it).
import * as maplibregl from 'maplibre-gl';
import type { GeoJSONSource } from 'maplibre-gl';
import { getProfile, getWay, peekWay, type Drive, type Ride } from '../api';
import * as cam3d from '../camera3d';
import { RAIL0 } from '../config';
import { fmt } from '../fmt';
import { boxKm, viewArea } from '../inview';
import { displayName, lineName } from '../names';
import type { Overlays } from '../overlays';
import { classMask, railMask, roadLenKm, surfaceMask, tollMask, unnamedHideClasses, type Selection, type Store, type Stretch } from '../state';
import type { DrivesPane } from '../ui/drives';
import type { ProfilePanel } from '../ui/profile';
import type { LinesPane, RidesPane } from '../ui/rides';
import type { Sight, SightsPane } from '../ui/sights';
import type { StatsCard, ViewPlace } from '../ui/stats';
import type { ListItem } from './hover';
import { line, point, type Marks } from './marks';
import type { RoadsRail } from './roads';

export interface ListsHost {
  /** The ground in view's outline (main.ts groundOutline). */
  outline: () => maplibregl.LngLat[];
  /** A list item hovered (null: none): the bottom bar shows it (ctl/hover.ts list). */
  hover: (x: ListItem | null) => void;
  /** A sight hovered: its OSM object asked for, ready for the O key. */
  sightHovered: (x: Sight) => void;
  /** Work for when the view settles, and while it moves at most every `ms`. */
  onSettled: (f: () => void) => void;
  duringMoves: (f: () => void, ms: number) => void;
}

export interface ListPanes {
  drives: DrivesPane;
  rides: RidesPane;
  lines: LinesPane;
  sights: SightsPane;
}

/** Length of a line (degrees, scaled by latitude: only for comparing lines). */
const lineLen = (g: [number, number][]) => {
  let l = 0;
  for (let i = 1; i < g.length; i++) l += Math.hypot((g[i][0] - g[i - 1][0]) * Math.cos((g[i][1] * Math.PI) / 180), g[i][1] - g[i - 1][1]);
  return l;
};

/** The point halfway along a line. */
const midpoint = (g: [number, number][]): [number, number] => {
  const half = lineLen(g) / 2;
  let l = 0;
  for (let i = 1; i < g.length; i++) {
    const d = Math.hypot((g[i][0] - g[i - 1][0]) * Math.cos((g[i][1] * Math.PI) / 180), g[i][1] - g[i - 1][1]);
    if (l + d >= half && d > 0) {
      const t = (half - l) / d;
      return [g[i - 1][0] + (g[i][0] - g[i - 1][0]) * t, g[i - 1][1] + (g[i][1] - g[i - 1][1]) * t];
    }
    l += d;
  }
  return g[0];
};

/** The part of a line between the points nearest two others. */
const cut = (coords: [number, number][], a: [number, number], b: [number, number]) => {
  const near = (q: [number, number]) => {
    const k = Math.cos((q[1] * Math.PI) / 180);
    let best = 0, bd = Infinity;
    coords.forEach((c, i) => {
      const d = ((c[0] - q[0]) * k) ** 2 + (c[1] - q[1]) ** 2;
      if (d < bd) [bd, best] = [d, i];
    });
    return best;
  };
  const [i, j] = [near(a), near(b)].sort((x, y) => x - y);
  return coords.slice(i, j + 1);
};

const driveStretch = (d: Drive): Stretch => ({
  a: d.geom[0], b: d.geom[d.geom.length - 1], label: `Scenic ${d.score.toFixed(0)} · ${fmt.dist(d.length_m)}`,
});
const rideStretch = (r: Ride): Stretch => ({
  a: r.geom[0], b: r.geom[r.geom.length - 1], label: `Scenic ride ${r.score.toFixed(0)} · ${fmt.dist(r.length_m)}`,
});

export class Lists {
  /** A drive's or ride's stretch on the map: the list's geometry, or, when it came from a link,
   * the stretch cut out of the road's profile once loaded. */
  private pinned: [number, number][] | null = null;
  /** The selected road's profile's line, once loaded. */
  private profileCoords: [number, number][] | null = null;
  private profileAbort: AbortController | null = null;
  /** Zoomed-out lists: from the server's summaries (null: not decided yet). */
  private approxLists: boolean | null = null;

  constructor(
    private map: maplibregl.Map,
    private store: Store,
    statsCard: StatsCard,
    private profile: ProfilePanel,
    private panes: ListPanes,
    overlays: Overlays,
    rd: RoadsRail,
    private marks: Marks,
    private host: ListsHost,
  ) {
    const { drives, rides, lines, sights } = panes;
    const { roads, rails } = rd;
    const hover = host.hover;

    // Sights: the landmarks shown, the most prominent in view of each kind.
    sights.kinds = () => overlays.viewLandmarks.filter((l) => store.s.overlays[l.key]);
    sights.query = (k) => overlays.topInView(k);
    sights.onSelect = (x) => overlays.select(x);
    sights.onHover = (x) => {
      marks.set('high', x ? point(x.lngLat, 'high') : null);
      if (x) host.sightHovered(x);
      hover(x ? { feature: { layer: x.layer, props: x.props, lngLat: x.lngLat } } : null);
    };

    // The In view summary's places, highest and lowest roads.
    statsCard.onPlace = (pl: ViewPlace) => {
      if (pl.layer) overlays.select({ lngLat: pl.lngLat, layer: pl.layer, props: pl.props });
      else map.flyTo({ center: pl.lngLat, zoom: Math.max(map.getZoom(), 11), duration: 900 });
    };
    statsCard.onPlaceHover = (pl: ViewPlace | null) => {
      marks.set('high', pl ? point(pl.lngLat, 'high') : null);
      if (!pl) return hover(null);
      if (pl.layer) return hover({ feature: { layer: pl.layer, props: pl.props ?? {}, lngLat: pl.lngLat } });
      if (pl === rd.viewExtra.rail?.busiest) return hover({ layer: rails, at: pl.lngLat });
      if (pl === rd.viewExtra.ferry?.busiest) return hover({ ferryAt: pl.lngLat });
      hover(null);
    };
    statsCard.onMark = (x, kind) => {
      marks.set(kind, x ? point(x.lngLat, kind, fmt.m(x.elev)) : null);
      hover(x ? { layer: ((x.tile.data?.lineStyle[x.line] ?? 0) & 15) >= RAIL0 ? rails : roads, at: x.lngLat, way: x.tile.data?.lineWay[x.line] } : null);
    };
    statsCard.onFly = (x) => {
      const ll = maplibregl.LngLat.convert(x.lngLat);
      map.flyTo({ ...cam3d.frame(map, ll, map.queryTerrainElevation(ll) ?? 0, Math.max(map.getZoom(), 14)), duration: 1200 });
      const kind = x === rd.stats?.highest ? 'high' : 'low';
      marks.set(kind, point(x.lngLat, kind, fmt.m(x.elev)));
      setTimeout(() => marks.set(kind, null), 6000);
    };
    statsCard.wayName = async (x) => {
      const info = await getWay(x.tile.data!.lineWay[x.line], x.lngLat);
      // Rail: the line's name without a route's direction or service codes.
      if (info && ['tram', 'metro', 'commuter', 'intercity', 'heritage'].includes(info.class)) return lineName(info);
      return info ? [info.ref, displayName(info.main, info.name, info.sub)].filter(Boolean).join(' ') : '';
    };

    // Scenic drives.
    drives.query = () => ({ bbox: this.bboxQuery(), poly: this.polyQuery(), classes: classMask(store.s), surface: surfaceMask(store.s), toll: tollMask(store.s), unnamed: unnamedHideClasses(store.s), len: roadLenKm(store.s), weights: store.s.weights, approx: this.approxQuery() });
    drives.onResults = (ds) =>
      map.getSource<GeoJSONSource>('drives')?.setData({
        type: 'FeatureCollection',
        features: ds.map((d) => ({ type: 'Feature', properties: { score: d.score }, geometry: { type: 'LineString', coordinates: d.geom } })),
      });
    drives.onHover = (d) => {
      hover(d ? { layer: roads, at: midpoint(d.geom), way: d.way, geom: [d.geom] } : null);
      return d ? this.setDriveHl(d.geom) : this.drawStretch();
    };
    drives.onSelect = (d) => this.pickStretch({ way: d.way, at: d.at }, driveStretch(d), d.approx ? null : d.geom);

    // Scenic rides and rail lines: the rail weights and service groups shown.
    const railQuery = () => ({ bbox: this.bboxQuery(), poly: this.polyQuery(), weights: store.s.rail.weights, groups: railMask(store.s), approx: this.approxQuery() });
    rides.query = railQuery;
    lines.query = railQuery;
    rides.onResults = (rs) => drives.onResults(rs.map((r) => ({ score: r.score, geom: r.geom }) as unknown as Drive));
    rides.onHover = (r) => {
      hover(r ? { layer: rails, at: midpoint(r.geom), geom: [r.geom] } : null);
      return r ? this.setDriveHl(r.geom) : this.drawStretch();
    };
    rides.onSelect = (r) => this.pickStretch({ way: r.way, at: r.at }, rideStretch(r), r.approx ? null : r.geom);
    lines.onHover = (l) => {
      hover(l ? { layer: rails, at: midpoint(l.geom.reduce((a, b) => (lineLen(b) > lineLen(a) ? b : a))), geom: l.geom } : null);
      map.getSource<GeoJSONSource>('drive-hl')?.setData(l ? { type: 'Feature', properties: {}, geometry: { type: 'MultiLineString', coordinates: l.geom } } : line(null));
      if (!l) this.drawStretch();
    };
    // A line picked from the list: selected, as a click on it on the map does.
    lines.onSelect = (l) => store.set({ selected: { way: l.way, at: l.at }, stretch: null });

    statsCard.onTab = (k) => {
      if (k === 'drives') drives.refresh(true);
      else if (k === 'rides') rides.refresh(true);
      else drives.onResults([]);
      if (k === 'lines') lines.refresh(true);
      if (k === 'sights') sights.refresh();
      this.drawStretch(); // drop hover highlights of the list left behind
    };
    const refresh = () => {
      drives.refresh();
      rides.refresh();
      lines.refresh();
    };
    host.onSettled(refresh);
    // The lists follow the view while it moves too, every 1.5 s (a request at a time: a newer one
    // replaces the one under way).
    host.duringMoves(refresh, 1500);
    drives.onShowChange = () => this.applyDrivesShown();

    statsCard.show(statsCard.tab); // restored tab: run its loaders now that callbacks exist
  }

  /** The drives' layer shown or hidden, as the Drives list's switch says. */
  applyDrivesShown() {
    if (this.map.getLayer('drives-line')) this.map.setLayoutProperty('drives-line', 'visibility', this.panes.drives.showOnMap ? 'visible' : 'none');
  }

  private bboxQuery() {
    const b = this.map.getBounds();
    return [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()].map((x) => x.toFixed(5)).join(',');
  }

  private polyQuery() {
    return this.host.outline().map((ll) => `${ll.lng.toFixed(4)},${ll.lat.toFixed(4)}`).join(',');
  }

  /** Zoomed-out lists (docs/phase5.md "Zoomed-out queries"): from the server's summaries when the
   * outline's box is wider than 1,200 km, exact again below 900 km (between, as it was: panning or a
   * resize doesn't flip it), one mode for drives, rides and rail lines. */
  private approxQuery(): boolean {
    const o = this.host.outline();
    if (o.length < 3) return this.approxLists ?? false;
    // (Its box across the antimeridian or round a pole: inview.ts.)
    const km = boxKm(viewArea(o.map((ll) => [ll.lng, ll.lat]), [-180, -90, 180, 90]).box);
    this.approxLists = this.approxLists === null ? km > 1050 : this.approxLists ? km > 900 : km > 1200;
    return this.approxLists;
  }

  private setDriveHl(g: [number, number][] | null) {
    this.map.getSource<GeoJSONSource>('drive-hl')?.setData(line(g));
  }

  /** The selected stretch on the map (a stretch picked from a list is of the selected road:
   * store.s.stretch, so links carry it). */
  private drawStretch() {
    const st = this.store.s.selected !== null ? this.store.s.stretch : null;
    if (st && !this.pinned && this.profileCoords) this.pinned = cut(this.profileCoords, st.a, st.b);
    this.setDriveHl(st ? this.pinned : null);
  }

  /** The stretch selected changed: on the profile and the map. */
  applyStretch() {
    const st = this.store.s.selected !== null ? this.store.s.stretch : null;
    if (!st) this.pinned = null;
    this.profile.highlight(st ? { start: st.a, end: st.b, label: st.label } : null);
    this.drawStretch();
  }

  /** A drive or ride picked from a list: its road or line selected (by its first way and the point
   * the list gives on it) and the stretch marked, the map staying where it is. A zoomed-out list's
   * geometry (its summaries' samples, 500 m apart) isn't kept: the stretch is cut from the road's
   * profile once loaded, as a link's is. */
  private pickStretch(sel: Selection, st: Stretch, geom: [number, number][] | null) {
    this.store.set({ selected: sel, stretch: st });
    this.pinned = geom;
    this.drawStretch();
  }

  /** The road or line selected (null: none): its profile loaded and shown, its line on the map. */
  async select(sel: Selection | null) {
    const { map, profile } = this;
    this.profileAbort?.abort();
    this.profileCoords = null;
    if (sel === null) {
      profile.hide();
      map.getSource<GeoJSONSource>('selection')?.setData(line(null));
      this.pinned = null;
      this.drawStretch();
      return;
    }
    const abort = (this.profileAbort = new AbortController());
    const info = peekWay(sel.way) ?? (await getWay(sel.way, sel.at));
    profile.loading((info && (info.ref || displayName(info.main, info.name, info.sub))) || 'this road');
    try {
      const p = await getProfile(sel.way, sel.at, abort.signal);
      profile.show(p);
      map.getSource<GeoJSONSource>('selection')?.setData(line(p.coords));
      this.profileCoords = p.coords;
      this.drawStretch();
    } catch (e) {
      if ((e as Error).name !== 'AbortError') profile.error((e as Error).message);
    }
  }
}
