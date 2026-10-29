// The ground in view, from its outline (main.ts groundOutline: lng, lat points around the screen),
// as the "in view" lists, landmarks and ferries test it. In longitude and latitude the outline of a
// view around a pole or across the antimeridian isn't a plain polygon (its longitudes wrap, or
// circle the pole), so the test is made on the stereographic plane centred in the outline, where it
// is one; the far hemisphere, behind the planet, is outside (#826, after #822's tint). Its box:
// longitudes in −180…180 with west > east across the antimeridian, every longitude when the outline
// circles a pole. crates/server/src/inview.rs is the same for the server's lists.

/** West, south, east, north (degrees); west > east: across the antimeridian. */
export type Box = [number, number, number, number];

export type ViewArea = {
  /** Whether a point (lng, lat) is in view. */
  test: (lng: number, lat: number) => boolean;
  /** The box holding the area. */
  box: Box;
  /** Which pole the outline goes round (null: neither). */
  pole: 'n' | 's' | null;
};

const D = Math.PI / 180;
const wrap = (x: number) => ((((x + 180) % 360) + 360) % 360) - 180;

/** The area inside `outline`; without a usable outline (under 3 points), `bounds`. */
export function viewArea(outline: [number, number][], bounds: Box): ViewArea {
  if (outline.length < 3) {
    const [w, s, e, n] = bounds;
    const all = e - w >= 360;
    const box: Box = all ? [-180, s, 180, n] : [wrap(w), s, wrap(e), n];
    return { test: (x, y) => inBox(box, x, y), box, pole: null };
  }
  // The centre: the mean of the outline's points as unit vectors.
  let cx = 0, cy = 0, cz = 0;
  for (const [x, y] of outline) {
    const c = Math.cos(y * D);
    cx += c * Math.cos(x * D); cy += c * Math.sin(x * D); cz += Math.sin(y * D);
  }
  const l0 = Math.atan2(cy, cx), s0 = cz / Math.hypot(cx, cy, cz), c0 = Math.sqrt(Math.max(0, 1 - s0 * s0));
  const plane = (lng: number, lat: number): [number, number] | null => {
    const dl = lng * D - l0, s = Math.sin(lat * D), c = Math.cos(lat * D), cd = Math.cos(dl);
    const q = 1 + s0 * s + c0 * c * cd;
    if (q < 1e-9) return null;
    const k = 2 / q;
    return [k * c * Math.sin(dl), k * (c0 * s - s0 * c * cd)];
  };
  const pts = outline.map(([x, y]) => plane(x, y)).filter((p): p is [number, number] => !!p);
  let pw = Infinity, pe = -Infinity, ps = Infinity, pn = -Infinity;
  for (const [x, y] of pts) {
    pw = Math.min(pw, x); pe = Math.max(pe, x); ps = Math.min(ps, y); pn = Math.max(pn, y);
  }
  const test = (lng: number, lat: number) => {
    const p = plane(lng, lat);
    if (!p) return false;
    const [x, y] = p;
    if (x < pw || x > pe || y < ps || y > pn) return false;
    let c = false;
    for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
      const [xi, yi] = pts[i], [xj, yj] = pts[j];
      if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) c = !c;
    }
    return c;
  };
  // The box: the longitudes unwrapped along the outline (each within 180° of the one before).
  let s = Infinity, n = -Infinity, u = outline[0][0], w = u, e = u;
  for (let i = 0; i < outline.length; i++) {
    const [x, y] = outline[i];
    if (i) u += wrap(x - outline[i - 1][0]);
    w = Math.min(w, u); e = Math.max(e, u); s = Math.min(s, y); n = Math.max(n, y);
  }
  u += wrap(outline[0][0] - outline[outline.length - 1][0]);
  const turns = Math.round((u - outline[0][0]) / 360);
  const pole = turns !== 0 ? (test(0, 90) ? 'n' : test(0, -90) ? 's' : null) : null;
  let box: Box;
  if (pole === 'n') box = [-180, s, 180, 90];
  else if (pole === 's') box = [-180, -90, 180, n];
  else if (e - w >= 360 || turns !== 0) box = [-180, s, 180, n];
  else box = [wrap(w), s, wrap(e), n];
  return { test, box, pole };
}

/** Whether a point is in a box (west > east: across the antimeridian). */
export function inBox([w, s, e, n]: Box, x: number, y: number): boolean {
  return y >= s && y <= n && (w <= e ? x >= w && x <= e : x >= w || x <= e);
}

/** Whether two boxes meet (either may cross the antimeridian). */
export function boxesMeet(a: Box, b: Box): boolean {
  if (a[1] > b[3] || a[3] < b[1]) return false;
  const spans = ([w, , e]: Box): [number, number][] => (w <= e ? [[w, e]] : [[w, 180], [-180, e]]);
  return spans(a).some(([aw, ae]) => spans(b).some(([bw, be]) => aw <= be && bw <= ae));
}

/** The box's widest extent (km): east–west at its middle latitude, or south–north; a box round a
 * pole is as wide as the cap it holds. */
export function boxKm([w, s, e, n]: Box): number {
  const ns = (n - s) * 111.32;
  if (n >= 90 || s <= -90) return 2 * ns;
  const dl = w <= e ? e - w : e - w + 360;
  return Math.max(dl * 111.32 * Math.cos(((s + n) / 2) * D), ns);
}
