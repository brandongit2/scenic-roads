// The terrain tint's controller: a colour scale like the roads' (ui/scale.ts) over the terrain in
// view, its histogram from the DEM tiles drawn (terrainstats.ts). Its range eases to the target as
// the road colours' does (the frame loop, ctl/frame.ts, calls `tick`), and the terrain is measured
// again every few hundred milliseconds while the view changes, mid-gesture too.
import type { Map as MLMap } from 'maplibre-gl';
import { spread } from '../autofit';
import { SLOPE_MAX } from '../basemap';
import { capGround } from '../polarglobe';
import type { Dist } from '../roads/stats';
import type { AppState, Store } from '../state';
import { applyTint, TINT_VARS, tintColourAt, tintCss } from '../terrain';
import { terrainDist } from '../terrainstats';
import type { LayersCard } from '../ui/layers';
import { cdfOf } from '../ui/scale';

export interface TintHost {
  /** The roads' colour range as drawn (the tint can follow it: Match). */
  roadRange: () => [number, number];
  /** The ground in view's outline (main.ts groundOutline). */
  outline: () => [number, number][];
  /** Whether the basemap style is in (the tint's layers with it). */
  styleReady: () => boolean;
}

export class TerrainTint {
  /** The terrain in view (null: not measured yet). */
  dist: Dist | null = null;
  /** The colour range as drawn, easing to its target. */
  cur: [number, number];
  /** The equalised scale's lookup (null: not equalised). */
  cdf: Uint8Array | null = null;
  private dirty = true;
  private lastMeasured = 0;
  private snap = true;
  private cdfKey = '';
  /** A palette previewed (the ramp list hovered), else null. */
  private preview: string | null = null;
  /** What the tint shows (on, variable): another one measures again and snaps. */
  private key: string;
  private colourAt: ((v: number) => [[number, number, number], number]) | null = null;

  constructor(private map: MLMap, private store: Store, private host: TintHost) {
    const t = store.s.terrain;
    this.cur = [...t.tintScales[t.tintVar].range] as [number, number];
    this.key = `${t.tint}|${t.tintVar}`;
  }

  /** Roads coloured so that the tint can follow their range: by elevation (or grade, for slope). */
  private matchable = () => {
    const s = this.store.s;
    return s.terrain.tintVar === 'slope' ? s.mode === 'grade' : s.mode === 'elev' || s.mode === 'relief';
  };

  private scale() {
    const t = this.store.s.terrain;
    const sc = t.tintScales[t.tintVar];
    return this.preview ? { ...sc, palette: this.preview } : sc;
  }

  private target(): [number, number] {
    const t = this.store.s.terrain;
    const sc = t.tintScales[t.tintVar];
    if (t.tintMatch && this.matchable()) return this.host.roadRange();
    if (!sc.auto) return sc.range;
    if (!this.dist || this.dist.total <= 0) return this.cur;
    return spread(this.dist.quantile(sc.fit[0] / 100), this.dist.quantile(sc.fit[1] / 100), TINT_VARS[t.tintVar].step * 4);
  }

  private measure() {
    const t = this.store.s.terrain;
    this.lastMeasured = performance.now();
    this.dirty = false;
    if (!t.tint || !this.host.styleReady()) return;
    const slope = t.tintVar === 'slope';
    const d = terrainDist(this.map, slope ? 'slope' : 'dem-tint', TINT_VARS[t.tintVar].domain, this.host.outline(), slope ? { max: SLOPE_MAX } : null, capGround(this.map));
    if (d) this.dist = d;
  }

  private updateCdf() {
    const sc = this.store.s.terrain.tintScales[this.store.s.terrain.tintVar];
    if (!sc.equalize || !this.dist || this.dist.total <= 0) {
      this.cdf = null;
      this.cdfKey = '';
      return;
    }
    const key = `${this.cur[0].toPrecision(4)}|${this.cur[1].toPrecision(4)}|${this.dist.total.toPrecision(6)}`;
    if (key === this.cdfKey) return;
    this.cdfKey = key;
    this.cdf = cdfOf(this.dist, this.cur);
  }

  /** The tint drawn again: its colours, range and equalisation. */
  refresh() {
    if (!this.host.styleReady()) return;
    this.updateCdf();
    applyTint(this.map, this.store.s.terrain, this.scale(), this.cur, this.cdf, this.store.s.palette, this.cdfKey);
    this.colourAt = null;
  }

  /** The Layers card's tint section: its previews, colours under the legend and Match. */
  wire(layers: LayersCard) {
    const s = () => this.store.s;
    layers.tintCssFor = (key) => tintCss(s().terrain, { ...this.scale(), palette: key }, this.cur, s().palette);
    layers.tintColourAt = (v) => {
      this.colourAt ??= tintColourAt(s().terrain, this.scale(), this.cur, this.cdf, s().palette);
      const [c, a] = this.colourAt(v);
      return [`rgb(${c.map((x) => Math.round(Math.max(0, Math.min(1, x)) * 255)).join(',')})`, a];
    };
    layers.tintMatchAvailable = this.matchable;
    layers.onTintPreview = (key) => {
      this.preview = key;
      this.refresh();
      layers.updateTint(this.dist, this.cur, this.cdf);
    };
  }

  /** The terrain in view changed (the camera moved, its tiles arrived): measured again. */
  invalidate() {
    this.dirty = true;
  }

  /** Whether the frame loop has measuring to do. */
  get due() {
    return this.store.s.terrain.tint && this.dirty;
  }

  /** A frame: measured again every 300 ms while the view changes (150 ms at rest), the range eased
   * by `k` toward its target. `panels`: the legend has changed; `easing`: the range is moving. */
  tick(now: number, moving: boolean, k: number): { panels: boolean; easing: boolean } {
    const t = this.store.s.terrain;
    let panels = false, easing = false;
    if (t.tint && this.dirty && now - this.lastMeasured > (moving ? 300 : 150)) {
      this.measure();
      panels = true;
    }
    if (t.tint) {
      const tt = this.target();
      const kt = this.snap ? 1 : k;
      this.snap = false;
      const nt: [number, number] = [this.cur[0] + (tt[0] - this.cur[0]) * kt, this.cur[1] + (tt[1] - this.cur[1]) * kt];
      if (Math.abs(nt[0] - this.cur[0]) + Math.abs(nt[1] - this.cur[1]) > TINT_VARS[t.tintVar].step * 0.01) {
        this.cur = nt;
        this.refresh();
        panels = true;
        easing = true;
      }
    }
    return { panels, easing };
  }

  /** The settings changed (terrain, palette or the roads' mode): another variable (metres ↔ percent)
   * or the tint just shown is measured and snapped to. */
  apply(s: AppState) {
    const key = `${s.terrain.tint}|${s.terrain.tintVar}`;
    if (key !== this.key) {
      this.key = key;
      this.snap = true;
      this.dist = null;
      this.dirty = true;
    }
    this.refresh();
  }
}
