// A colour scale's spacing and its legend's ticks (ui/scale.ts ScaleControls and scaleU; the depth
// ramp's Linear / Log, depth.ts), apart from the DOM so web/test/scaleaxis.test.ts can run them.

/** How a scale spaces its values: in proportion, or by log(1 + v) (more room at the low end). */
export type Spacing = 'linear' | 'log';

/** A value's place along an axis of this spacing: itself, or log(1 + v) (mirrored below 0). */
export const spaced = (sp: Spacing, v: number) => (sp === 'log' ? Math.sign(v) * Math.log1p(Math.abs(v)) : v);
/** The value at a place along the axis (spaced's inverse). */
export const unspaced = (sp: Spacing, t: number) => (sp === 'log' ? Math.sign(t) * Math.expm1(Math.abs(t)) : t);

/** Round a range to "nice" tick values. */
export function niceStep(span: number, target: number): number {
  const raw = span / target;
  const p = 10 ** Math.floor(Math.log10(raw));
  const r = raw / p;
  return (r < 1.5 ? 1 : r < 3 ? 2 : r < 7 ? 5 : 10) * p;
}

/** A tick: its value, its x (px), and the left edge of its label (none: no room for it). */
export interface Tick { v: number; x: number; lx?: number }

/** The ticks of an axis `width` px wide over [a, b] (in spaced units). Linear: at a nice step, no
 * finer than `step` (the metric's own). Log: 0 and 1, 2 and 5 times the powers of ten from `step`
 * up (mirrored below 0). Labels go to powers of ten first, then fives, then twos, left to right,
 * each kept inside the axis and where it stays `gap` px from the others and from the `taken` spans
 * (the range handles' labels). A tick is labelled only when its text is its own: not one already
 * shown, and not a rounding of a value between two (0.5 m read "1 m" beside 1 m's "1 m").
 * `nice`: the candidate values and their ranks between two values, in place of the spacing's (a
 * metric whose values are logarithms: data available's, available.ts logTicks). */
export function axisTicks(sp: Spacing, a: number, b: number, width: number, step: number,
  label: (v: number) => string, measure: (text: string) => number, taken: [number, number][] = [], gap = 4,
  nice?: (lo: number, hi: number) => [number, number][]): Tick[] {
  // (The ends a hair wider, so a round end comes back from its log as itself.)
  const lo = unspaced(sp, a), hi = unspaced(sp, b), eps = 1e-9 * Math.max(Math.abs(lo), Math.abs(hi), step);
  const vals: [number, number][] = [];
  if (nice) {
    vals.push(...nice(lo, hi));
  } else if (sp === 'linear') {
    const s = Math.max(niceStep(hi - lo, 5), step);
    for (let v = Math.ceil((lo - eps) / s) * s; v <= hi + eps; v += s) vals.push([Number(v.toPrecision(12)), 0]);
  } else {
    if (lo <= eps && hi >= -eps) vals.push([0, 0]);
    for (let k = Math.floor(Math.log10(step)); 10 ** k <= Math.max(-lo, hi) + eps; k++) {
      for (const [m, rank] of [[1, 0], [5, 1], [2, 2]]) {
        const v = Number((m * 10 ** k).toPrecision(12));
        if (v < step) continue;
        for (const sv of [v, -v]) if (sv >= lo - eps && sv <= hi + eps) vals.push([sv, rank]);
      }
    }
  }
  const ticks = vals.map(([v, rank]) => ({ v, rank, x: ((spaced(sp, v) - a) / (b - a)) * width, lx: undefined as number | undefined }));
  ticks.sort((p, q) => p.x - q.x);
  const boxes = [...taken], shown = new Set<string>();
  for (const t of [...ticks].sort((p, q) => p.rank - q.rank || p.x - q.x)) {
    const text = label(t.v), d = 1e-3 * Math.max(Math.abs(t.v), step);
    // (0 is never a rounding of anything, and a hair below it may read "-0".)
    if (shown.has(text) || (t.v !== 0 && (label(t.v - d) !== text || label(t.v + d) !== text))) continue;
    const w = measure(text), l = Math.max(0, Math.min(width - w, t.x - w / 2));
    if (boxes.some(([s, e]) => l < e + gap && l + w > s - gap)) continue;
    boxes.push([l, l + w]);
    shown.add(text);
    t.lx = l;
  }
  return ticks.map(({ v, x, lx }) => (lx === undefined ? { v, x } : { v, x, lx }));
}
