// The data available layers' controller (docs/steps.md §14): the themes the catalogue lists, each
// one's description read from the server at its version (/api/available/<theme>), the layer shown
// on the map and over the polar caps at that version, the panel's section, and the click's panel.
import type { LngLat, Map as MLMap } from 'maplibre-gl';
import { version } from '../api';
import { atUrl, parsePieces, parseTheme, pieceRows, polarTileUrl, shown, themesOf, tileUrl, type AvTheme, type ThemeLayers } from '../available';
import { AvailableMap } from '../availablemap';
import { hostFor } from '../hosts';
import type { Pole } from '../polar';
import type { PolarCaps } from '../polarcaps';
import { MapPopup } from '../popup';
import type { Store } from '../state';
import { piecesView, type AvailableSection } from '../ui/available';

export class Available {
  private themes: ThemeLayers[] = [];
  /** Each theme's description, and the version it was read at (a new version reads it again). */
  private desc = new Map<string, { v: string; d: AvTheme | 'loading' | 'failed' }>();
  private layer: AvailableMap;
  private popup: MapPopup;
  private polar: PolarCaps | null = null;
  private clickAbort: AbortController | null = null;
  /** Whether the section is shown (a catalogue listing a theme): set by the controller. */
  onShown: (on: boolean) => void = () => {};

  constructor(map: MLMap, private store: Store, private section: AvailableSection, before: string) {
    this.layer = new AvailableMap(map, before);
    this.popup = new MapPopup(map);
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

  /** The map and the panel as the settings say (at each change of them, and of the catalogue). */
  apply() {
    const s = this.store.s.available;
    this.section.setThemes(this.themes.map((t) => ({ theme: t.theme, desc: this.desc.get(t.theme)?.d ?? 'loading' })));
    const c = s.on ? this.current() : null;
    if (!c) {
      this.layer.show(null);
      this.polar?.setOverlay(() => null);
      if (!s.on) this.closePopup();
      return;
    }
    const { layer, set } = shown(s, c.d);
    const theme = c.t.theme;
    this.layer.show({ url: tileUrl(hostFor('layers'), theme, set, layer.id, s.avail, version(`available-${theme}`)), maxzoom: c.t.maxzoom, opacity: s.opacity });
    this.polar?.setOverlay((pole: Pole) => {
      const maxZoom = c.t.poles[pole];
      if (maxZoom === undefined) return null;
      const v = version(`available-${theme}-${pole}`);
      return {
        key: `${theme}/${set}/${layer.id}/${s.avail}/${v}`,
        maxZoom,
        url: (z, x, y) => polarTileUrl(hostFor('layers'), pole, theme, set, layer.id, s.avail, v, z, x, y),
        opacity: s.opacity,
      };
    });
  }

  /** A click on the map while the group is on: the pieces there in a popup (true: handled). */
  click(at: LngLat): boolean {
    const s = this.store.s.available;
    const c = s.on ? this.current() : null;
    if (!c) return false;
    const { layer, set } = shown(s, c.d);
    const all = s.avail === 'all';
    this.popup.show(at, piecesView('loading', layer, all));
    this.clickAbort?.abort();
    const abort = (this.clickAbort = new AbortController());
    fetch(atUrl(c.t.theme, set, at.lng, at.lat, s.avail), { signal: abort.signal })
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((j) => parsePieces(j) ?? Promise.reject(new Error('not an answer')))
      .then(
        (ps) => !abort.signal.aborted && this.popup.show(at, piecesView(pieceRows(ps, layer.id, c.d), layer, all)),
        (e) => !abort.signal.aborted && (e as Error).name !== 'AbortError' && this.popup.show(at, piecesView('failed', layer, all)),
      );
    return true;
  }

  closePopup() {
    this.clickAbort?.abort();
    this.popup.close();
  }
}
