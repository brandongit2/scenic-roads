// The polar caps' names drawn as MapLibre draws a symbol layer's text (polarpoints.ts; task #668):
// the same glyphs (the style's SDF glyphs, from the map's glyph manager), laid out as its shaping
// lays them (24 px to the em, its -17 baseline offset, lines 1.2 em apart, centred, wrapped at the
// layer's max width), and turned to pixels by its text shader's edges (symbol_sdf.fragment.glsl:
// the fill's edge at 0.75, the halo's at (6 − width / scale) / 8, both softened by its gamma and
// the halo's blur). Done once per name and look on the CPU into an image, not per frame: a cap
// holds some hundreds of names at most. Nothing here touches the page or the map.

/** One em of a glyph (MapLibre's ONE_EM) and the SDF glyphs' border (GLYPH_PBF_BORDER). */
export const ONE_EM = 24;
const BORDER = 3;
/** MapLibre's shaping baseline offset (SHAPING_DEFAULT_OFFSET) and default line height (em). */
const BASELINE = -17;
const LINE_HEIGHT = 1.2;
/** The shader's SDF scale (SDF_PX) and the fill's edge. */
const SDF_PX = 8;
const INNER_EDGE = 192 / 256;

/** An area's name shows once √area spans this many pixels at the default spacing (landmarks::labels SIZE_PX). */
const SIZE_PX = 20;

/** Whether a point's name shows at `mpp` ground metres a CSS pixel at its place, `px` its kind's
 * spacing (`base` the default's): its reach `r` (m) spans the spacing, and an area's √area `w` (m)
 * is big enough on screen (basemap.ts labelTileFilter's rules, on the ground: landmarks::polar). */
export function nameShows(p: { r?: unknown; w?: unknown }, mpp: number, px: number, base: number): boolean {
  if (!(Number(p.r ?? 0) / mpp >= px)) return false;
  return p.w === undefined || Number(p.w) / mpp >= SIZE_PX * Math.sqrt(px / base);
}

/** The text sizes names are made at (CSS px). */
const SIZE_STEP = 0.25;
/** A layer's text size as a name is made at it: in SIZE_STEP steps, so a size that follows the
 * zoom (place-town's, water-name's …) makes a name again a few times over a zoom, not each frame
 * (a two-line name took ~10 ms at 2×, 16 at 3×, M1). */
export const textSize = (px: number): number => Math.round(px / SIZE_STEP) * SIZE_STEP;

/** A glyph as the glyph manager gives it (StyleGlyph): its SDF bitmap and metrics, em px. */
export type Glyph = {
  bitmap: { width: number; height: number; data: Uint8Array | Uint8ClampedArray };
  metrics: { width: number; height: number; left: number; top: number; advance: number };
};

/** Straight (not premultiplied) RGBA, 0..1. */
export type Rgba = [number, number, number, number];

/** A layer's text paint and layout as it evaluates for a name. */
export type TextLook = {
  fill: Rgba;
  halo: Rgba;
  /** Halo width and blur (CSS px). */
  haloWidth: number;
  haloBlur: number;
  /** Text size (CSS px). */
  size: number;
  /** Letter spacing (em) and the most an unbroken line may be wide (em). */
  spacing: number;
  maxWidth: number;
  transform: 'none' | 'uppercase' | 'lowercase';
};

/** A line of a name and its size against the look's (MapLibre's format `font-scale`). */
export type TextLine = { text: string; scale: number };

/** A glyph placed in the name's em space (its bitmap's top left). */
type Placed = { g: Glyph; x: number; y: number; scale: number };

/** The characters of a name's lines (as the glyph manager is asked for them). */
export const charsOf = (lines: TextLine[], look: Pick<TextLook, 'transform'>): string[] => [
  ...new Set(lines.flatMap((l) => [...cased(l.text, look.transform)])),
];

const cased = (t: string, c: TextLook['transform']) => (c === 'uppercase' ? t.toUpperCase() : c === 'lowercase' ? t.toLowerCase() : t);

/** A line's width (em px) at scale 1. */
function width(text: string, glyphs: Map<string, Glyph | null>, spacing: number): number {
  let w = 0;
  for (const ch of text) w += (glyphs.get(ch)?.metrics.advance ?? 0) + spacing * ONE_EM;
  return w;
}

/** A line broken at its spaces into the fewest lines no wider than `max` (em px) where it can be,
 * as even as can be (MapLibre balances its breaks likewise). */
export function wrap(text: string, glyphs: Map<string, Glyph | null>, look: Pick<TextLook, 'spacing' | 'maxWidth'>): string[] {
  const max = look.maxWidth * ONE_EM;
  const words = text.split(' ');
  const total = width(text, glyphs, look.spacing);
  if (total <= max || words.length < 2) return [text];
  const n = Math.min(words.length, Math.ceil(total / max));
  const target = total / n;
  const out: string[] = [];
  let line = '';
  for (const w of words) {
    const next = line ? `${line} ${w}` : w;
    // (A break where this word would carry the line past the even share, more lines left to fill.)
    if (line && out.length < n - 1 && width(next, glyphs, look.spacing) > target + width(w, glyphs, look.spacing) / 2) {
      out.push(line);
      line = w;
    } else line = next;
  }
  out.push(line);
  return out;
}

/** The name's glyphs placed (em px, its anchor at (0, 0)): centred lines, the block centred on the
 * anchor (`anchor` center) or hanging from it (top); with the block's box. */
export function layout(lines: TextLine[], glyphs: Map<string, Glyph | null>, look: TextLook, anchor: 'center' | 'top') {
  const rows = lines.flatMap((l) => wrap(cased(l.text, look.transform), glyphs, look).map((t) => ({ text: t, scale: l.scale })));
  const heights = rows.map((r) => LINE_HEIGHT * ONE_EM * r.scale);
  const block = heights.reduce((a, b) => a + b, 0);
  let y = anchor === 'center' ? -block / 2 : 0;
  const placed: Placed[] = [];
  let x0 = Infinity, x1 = -Infinity;
  rows.forEach((r, i) => {
    const w = width(r.text, glyphs, look.spacing) * r.scale;
    let x = -w / 2;
    x0 = Math.min(x0, x);
    x1 = Math.max(x1, x + w);
    // (The glyphs' origin as MapLibre's shaping puts it: half a line down, then its offset, scaled
    // with the line; one line centred has it at -17.)
    const base = y + heights[i] / 2 + BASELINE * r.scale;
    for (const ch of r.text) {
      const g = glyphs.get(ch);
      if (g) {
        placed.push({ g, x: x + (g.metrics.left - BORDER) * r.scale, y: base + (-g.metrics.top - BORDER) * r.scale, scale: r.scale });
      }
      x += ((g?.metrics.advance ?? 0) + look.spacing * ONE_EM) * r.scale;
    }
    y += heights[i];
  });
  const top = anchor === 'center' ? -block / 2 : 0;
  return { placed, box: [x0, top, x1, top + block] as [number, number, number, number] };
}

const smoothstep = (a: number, b: number, x: number) => {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
};

/** A name as an image (straight RGBA, `dpr` device pixels a CSS pixel), and where its anchor is in
 * it (device px); null for a name with no glyph to draw. */
export function rasterize(lines: TextLine[], glyphs: Map<string, Glyph | null>, look: TextLook, anchor: 'center' | 'top', dpr: number) {
  const { placed, box } = layout(lines, glyphs, look, anchor);
  if (!placed.length) return null;
  // (em px to device px; room round the box for the halo.)
  const k = (look.size / ONE_EM) * dpr;
  const pad = Math.ceil((look.haloWidth + look.haloBlur + 2) * dpr);
  const ox = Math.ceil(-box[0] * k) + pad, oy = Math.ceil(-box[1] * k) + pad;
  const w = Math.ceil((box[2] - box[0]) * k) + 2 * pad, h = Math.ceil((box[3] - box[1]) * k) + 2 * pad;
  const data = new Uint8ClampedArray(w * h * 4);
  const fontScale = look.size / ONE_EM;
  const edge = 0.105 / dpr;
  for (let py = 0; py < h; py++) {
    for (let px = 0; px < w; px++) {
      // This pixel's centre in em space, and the SDF there: each glyph's, sampled bilinear, the
      // nearest edge winning (glyphs barely overlap).
      const ex = (px + 0.5 - ox) / k, ey = (py + 0.5 - oy) / k;
      let d = 0, sc = 1;
      for (const p of placed) {
        const bw = p.g.bitmap.width, bh = p.g.bitmap.height;
        const u = (ex - p.x) / p.scale - 0.5, v = (ey - p.y) / p.scale - 0.5;
        if (u < -1 || v < -1 || u > bw || v > bh) continue;
        const at = (i: number, j: number) => (i < 0 || j < 0 || i >= bw || j >= bh ? 0 : p.g.bitmap.data[j * bw + i] / 255);
        const i0 = Math.floor(u), j0 = Math.floor(v), fu = u - i0, fv = v - j0;
        const s = (at(i0, j0) * (1 - fu) + at(i0 + 1, j0) * fu) * (1 - fv) + (at(i0, j0 + 1) * (1 - fu) + at(i0 + 1, j0 + 1) * fu) * fv;
        if (s > d) {
          d = s;
          sc = p.scale;
        }
      }
      if (d <= 0) continue;
      const fs = fontScale * sc;
      const gamma = edge / fs;
      const fillA = smoothstep(INNER_EDGE - gamma, INNER_EDGE + gamma, d) * look.fill[3];
      const gh = ((look.haloBlur * 1.19) / SDF_PX + edge) / fs;
      const innerHalo = INNER_EDGE + gh;
      const haloEdge = (6 - look.haloWidth / fs) / SDF_PX;
      const haloA =
        look.haloWidth > 0 ? Math.min(smoothstep(haloEdge - gh, haloEdge + gh, d), 1 - smoothstep(innerHalo - gh, innerHalo + gh, d)) * look.halo[3] : 0;
      // (Fill over halo, as the shader puts them together.)
      const a = fillA + (1 - fillA) * haloA;
      if (a <= 0) continue;
      const o = (py * w + px) * 4;
      for (let c = 0; c < 3; c++) data[o + c] = Math.round((255 * (look.fill[c] * fillA + look.halo[c] * haloA * (1 - fillA))) / a);
      data[o + 3] = Math.round(255 * a);
    }
  }
  return { width: w, height: h, data, ox, oy, box: [box[0] * k - pad, box[1] * k - pad, box[2] * k + pad, box[3] * k + pad] as [number, number, number, number] };
}
