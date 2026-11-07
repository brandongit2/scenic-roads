// The ferries' controller: the In view figures of the ferry lines (km by kind, the coverage in
// the Ferry section) and their colour scale, whose range auto-fits to the lines in view as the
// roads' does and eases there in the frame loop (ctl/frame.ts calls `tick`).
import { FIT_LEN_DEFAULT, fitByPct, fitByWidths, pairOf, unitOf } from '../autofit';
import { ferryMetricDef } from '../ferry';
import type { Ferries } from '../ferries';
import type { Dist } from '../roads/stats';
import type { Store } from '../state';
import type { FerryCard } from '../ui/ferry';
import type { LayersCard } from '../ui/layers';
import { cdfOf } from '../ui/scale';
import type { FramePart } from './frame';

export interface FerryHost {
  /** The view's width on the ground at its centre (km): the scale's fit by screen widths. */
  widthKm: () => number;
  /** The frame loop to run (the range has a new target). */
  wake: () => void;
}

export class FerryScale implements FramePart {
  /** The colour metric over the ferry lines in view (weights: their km in view). */
  private dist: Dist | null = null;
  private target: [number, number] | null = null;
  /** The range as drawn, easing to its target (null before the first lines in view). */
  private cur: [number, number] | null = null;
  /** The metric `cur` is for: another one snaps to its target. */
  private key = '';
  private painted = 0;
  readonly due = false;

  constructor(private ferries: Ferries, private store: Store, private layers: LayersCard, private card: FerryCard, private host: FerryHost) {}

  /** The lines in view again (the view settled or moved, the lines loaded, the settings changed). */
  update() {
    const f = this.store.s.ferry;
    if (!this.ferries.loaded || !f.on) {
      this.layers.updateFerry(null);
      this.card.update(null);
      return;
    }
    const v = this.ferries.inView();
    this.layers.updateFerry(v.km);
    this.card.update(v.cov);
    const d = ferryMetricDef(f.metric);
    const dist = (this.dist = this.ferries.metricDist());
    this.target = !f.auto || !dist || dist.total <= 0 ? f.range
      : d.byLen && unitOf(f.fitUnits, f.metric) === 'widths' ? fitByWidths(dist, dist.total, this.host.widthKm(), pairOf(f.fitLens, f.metric, FIT_LEN_DEFAULT), d.step)
      : fitByPct(dist, f.fit, d.step);
    if (!this.cur || this.key !== f.metric) {
      this.key = f.metric;
      this.cur = this.target;
      this.ease(true);
    }
    this.host.wake();
  }

  /** The eased range applied: the lines' colours are a data-driven expression (each change lays
   * the ferry source out again), so at most every 100 ms while it eases. */
  private ease(force = false) {
    if (!this.cur) return;
    const now = performance.now();
    if (!force && now - this.painted < 100) return;
    this.painted = now;
    const cdf = this.store.s.ferry.equalize ? cdfOf(this.dist, this.cur) : null;
    this.ferries.setScale(this.cur, cdf);
    this.card.updateScale(this.dist, this.cur, cdf);
  }

  /** A frame: the range eased by `k` toward its target, the last step straight onto it. */
  tick(_now: number, _moving: boolean, k: number): { panels: boolean; easing: boolean } {
    const cur = this.cur, ft = this.target;
    if (!cur || !ft || !this.store.s.ferry.on) return { panels: false, easing: false };
    const span = Math.max(1e-9, ft[1] - ft[0]);
    const nf: [number, number] = [cur[0] + (ft[0] - cur[0]) * k, cur[1] + (ft[1] - cur[1]) * k];
    if (Math.abs(nf[0] - cur[0]) + Math.abs(nf[1] - cur[1]) > span * 0.002) {
      this.cur = nf;
      this.ease();
      return { panels: false, easing: true };
    }
    if (cur !== ft && Math.abs(ft[0] - cur[0]) + Math.abs(ft[1] - cur[1]) > 0) {
      this.cur = ft;
      this.ease(true);
    }
    return { panels: false, easing: false };
  }
}
