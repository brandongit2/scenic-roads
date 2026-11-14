// The data available layers' controller (docs/steps.md §14): the themes the catalogue lists, each
// one's description read from the server at its version (/api/available/<theme>), the layer shown
// on the map and over the polar caps at that version, coloured through the layer's colour map as
// the terrain tint is (ctl/tint.ts): its values in view measured from the tiles drawn
// (terrainstats.ts), the range auto-fitted, locked or whole, equalized, faded and highlighted, the
// panel's section, and the click's panel.
import type { LngLat, Map as MLMap } from 'maplibre-gl';
import { spread } from '../autofit';
import { version } from '../api';
import { atUrl, axisOf, layerName, parsePieces, parseTheme, pieceRows, polarTileUrl, rampStops, scaleOf, shown, themesOf, tileUrl, type AvTheme, type ThemeLayers } from '../available';
import { AVAILABLE_LAYER, AvailableMap } from '../availablemap';
import { hostFor } from '../hosts';
import { paletteFn } from '../palettes';
import type { Pole } from '../polar';
import type { PolarCaps } from '../polarcaps';
import { MapPopup } from '../popup';
import type { Dist } from '../roads/stats';
import type { Store } from '../state';
import { terrainDist } from '../terrainstats';
import { piecesView, type AvailableSection } from '../ui/available';
import { cdfOf, fadeAlpha, scaleU } from '../ui/scale';

/** How often the values in view are measured again while the view or its tiles change (ms). */
const MEASURE_MS = 200;

export class Available {
  private themes: ThemeLayers[] = [];
  /** Each theme's description, and the version it was read at (a new version reads it again). */
  private desc = new Map<string, { v: string; d: AvTheme | 'loading' | 'failed' }>();
  private layer: AvailableMap;
  private popup: MapPopup;
  private polar: PolarCaps | null = null;
  private clickAbort: AbortController | null = null;
  /** The values in view of the layer measured (`key`), and the range drawn. */
  private dist: Dist | null = null;
  private range: [number, number] | null = null;
  private key = '';
  private timer = 0;
  /** A palette previewed (the ramp list hovered), else null. */
  private preview: string | null = null;
  /** Whether the section is shown (a catalogue listing a theme): set by the controller. */
  onShown: (on: boolean) => void = () => {};

  /** `outline`: the ground in view (main.ts groundOutline), as the other layers' in-view take it. */
  constructor(private map: MLMap, private store: Store, private section: AvailableSection, before: string, private outline: () => [number, number][]) {
    this.layer = new AvailableMap(map, before);
    this.popup = new MapPopup(map);
    section.onPreview = (k) => {
      this.preview = k;
      this.refresh();
    };
    map.on('move', () => this.measureSoon());
    map.on('sourcedata', (e) => {
      if (e.sourceId === 'available' && (e as { tile?: unknown }).tile) this.measureSoon();
    });
  }

  /** The polar caps, once made: the layer drawn over them too. */
  setPolar(p: PolarCaps) {
    this.polar = p;
    this.apply();
  }

  /** The catalogue's layers (at start and for each answer): the themes, each read again at a new version. */
  setCatalogue(layers: Record<string, unknown> | undefined) {
    this.themes = themesOf(layers);
    this.onShown(this.themes.length > 0);
    for (const t of this.themes) {
      const v = version(`available-${t.theme}`);
      const have = this.desc.get(t.theme);
      if (have && have.v === v && have.d !== 'failed') continue;
      this.desc.set(t.theme, { v, d: have && have.d !== 'failed' ? have.d : 'loading' });
      fetch(`/api/available/${t.theme}${v ? `?v=${v}` : ''}`)
        .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`))))
        .then((j) => parseTheme(j) ?? Promise.reject(new Error('not a theme')))
        .then(
          (d) => this.desc.get(t.theme)?.v === v && this.desc.set(t.theme, { v, d }),
          () => this.desc.get(t.theme)?.v === v && this.desc.set(t.theme, { v, d: 'failed' }),
        )
        .then(() => this.apply());
    }
    this.apply();
  }

  /** The theme shown and its description, while the group is on and it's read. */
  private current(): { t: ThemeLayers; d: AvTheme } | null {
    const s = this.store.s.available;
    const t = this.themes.find((x) => x.theme === s.theme) ?? this.themes[0];
    const d = t && this.desc.get(t.theme)?.d;
    return t && d && typeof d === 'object' ? { t, d } : null;
  }

  /** The values in view measured again soon (at most every MEASURE_MS). */
  private measureSoon() {
    if (this.timer || !this.store.s.available.on) return;
    this.timer = window.setTimeout(() => {
      this.timer = 0;
      const c = this.current();
      if (!c || !this.store.s.available.on) return;
      const l = shown(this.store.s.available, c.d).layer;
      const d = terrainDist(this.map, 'available', axisOf(l).domain, this.outline(), null, null, true);
      if (!d) return;
      this.dist = d;
      this.refresh();
    }, MEASURE_MS);
  }

  /** The map and the panel as the settings say (at each change of them and of the catalogue). */
  apply() {
    this.section.setThemes(this.themes.map((t) => ({ theme: t.theme, desc: this.desc.get(t.theme)?.d ?? 'loading' })));
    this.refresh();
  }

  /** The map and the colour map's legend (also as the values in view change). */
  private refresh() {
    const s = this.store.s.available;
    const c = s.on ? this.current() : null;
    if (!c) {
      this.layer.show(null);
      this.polar?.setOverlay(() => null);
      if (!s.on) this.closePopup();
      return;
    }
    const { layer, set } = shown(s, c.d);
    const theme = c.t.theme;
    // Another layer or switch: its values measured anew (another axis, other pieces).
    const key = `${theme}|${set}|${layer.id}|${s.avail}`;
    if (key !== this.key) {
      this.key = key;
      this.dist = null;
      this.range = null;
      this.measureSoon();
    }
    const sc = scaleOf(s, layer), axis = axisOf(layer);
    // The range: a layer of words', theirs; else auto-fitted to the values in view (until they're
    // measured, the last fit or the whole), or the one chosen.
    let range: [number, number];
    if (axis.fixed) range = axis.domain;
    else if (!sc.auto) range = sc.range;
    else if (this.dist && this.dist.total > 0) range = spread(this.dist.quantile(sc.fit[0] / 100), this.dist.quantile(sc.fit[1] / 100), axis.step * 4);
    else range = this.range ?? axis.domain;
    this.range = range;
    const cdf = sc.equalize && !axis.fixed ? cdfOf(this.dist, range) : null;
    const colour = paletteFn(this.preview ?? sc.palette);
    const colourAt = (v: number) => {
      const u = scaleU(v, range, cdf);
      const rgb = colour(u).map((x) => Math.round(Math.max(0, Math.min(1, x)) * 255));
      return `rgba(${rgb.join(',')},${fadeAlpha(u, sc.lowFade, sc.lowSpan).toFixed(3)})`;
    };
    const stops = rampStops(range, !!cdf, colourAt, sc.threshold);
    this.layer.show({ url: tileUrl(hostFor('layers'), theme, set, layer.id, s.avail, version(layerName(theme, set))), maxzoom: c.t.maxzoom, scale: `${theme}/${layer.id}`, opacity: s.opacity, stops });
    this.polar?.setOverlay((pole: Pole) => {
      const maxZoom = c.t.poles[pole];
      if (maxZoom === undefined) return null;
      const v = version(layerName(theme, set, pole));
      return {
        key: `${theme}/${set}/${layer.id}/${s.avail}/${v}`,
        maxZoom,
        url: (z, x, y) => polarTileUrl(hostFor('layers'), pole, theme, set, layer.id, s.avail, v, z, x, y),
        ramp: AVAILABLE_LAYER,
      };
    });
    this.section.update(this.dist, range, cdf);
  }

  /** A click on the map while the group is on: the pieces there in a popup (true: handled). */
  click(at: LngLat): boolean {
    const s = this.store.s.available;
    const c = s.on ? this.current() : null;
    if (!c) return false;
    const { layer, set } = shown(s, c.d);
    const all = s.avail === 'all';
    this.popup.show(at, piecesView('loading', layer, all, set));
    this.clickAbort?.abort();
    const abort = (this.clickAbort = new AbortController());
    fetch(atUrl(c.t.theme, set, at.lng, at.lat, s.avail), { signal: abort.signal })
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((j) => parsePieces(j) ?? Promise.reject(new Error('not an answer')))
      .then(
        (ps) => !abort.signal.aborted && this.popup.show(at, piecesView(pieceRows(ps, layer.id, c.d), layer, all, set)),
        (e) => !abort.signal.aborted && (e as Error).name !== 'AbortError' && this.popup.show(at, piecesView('failed', layer, all, set)),
      );
    return true;
  }

  closePopup() {
    this.clickAbort?.abort();
    this.popup.close();
  }
}
