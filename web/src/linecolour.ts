// Rail and ferry lines' own colours (OSM route colour tags) made legible on the dark map: a colour
// darker than MIN_L (OKLab lightness, perceptual) is lifted to it, keeping its hue and as much of
// its chroma as stays in gamut, so the Eurostar's navy (#011633, as dark as the map) becomes a
// clear mid blue and a black line a grey. Lighter colours are left as they are. Everything that
// shows a line's colour goes through this: the rail layer and stop dots (roads/worker.ts decodes
// the tiles' line colours with it), the Rides list and the bottom bar's swatch, and ferries in
// Operator colours (their lines, terminals and popups).

const MIN_L = 0.6;

const toLin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const toSrgb = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);

function oklab(r: number, g: number, b: number): [number, number, number] {
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

/** OKLab → linear sRGB (may fall outside 0..1). */
function linear(L: number, a: number, b: number): [number, number, number] {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}

const pack = (c: [number, number, number]) => {
  const [r, g, b] = c.map((v) => Math.round(toSrgb(Math.min(1, Math.max(0, v))) * 255));
  return (r << 16) | (g << 8) | b;
};

/** 0xRRGGBB, lifted to MIN_L if darker. */
export function legibleRgb(rgb: number): number {
  const [L, a, b] = oklab(toLin(((rgb >> 16) & 255) / 255), toLin(((rgb >> 8) & 255) / 255), toLin((rgb & 255) / 255));
  if (L >= MIN_L) return rgb;
  // The same hue at the lighter level, its chroma raised as much as the lightness (a dark colour
  // looks more saturated than its chroma says: navy should stay a blue, not turn slate), then
  // reduced until it fits in sRGB.
  for (let k = Math.min(2, Math.sqrt(MIN_L / Math.max(L, 0.05))); k > 0.05; k *= 0.9) {
    const c = linear(MIN_L, a * k, b * k);
    if (c.every((v) => v >= -1e-4 && v <= 1 + 1e-4)) return pack(c);
  }
  return pack(linear(MIN_L, 0, 0));
}

let ctx: OffscreenCanvasRenderingContext2D | null | undefined;

/** A CSS colour as 0xRRGGBB: '#rgb', '#rrggbb' or a name ('blue', as some OSM colour tags are);
 * null if it isn't an opaque colour (OSM's 'none'). */
function cssRgb(c: string): number | null {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(c.trim());
  if (m) return parseInt(m[1].length === 3 ? [...m[1]].map((d) => d + d).join('') : m[1], 16);
  if (ctx === undefined) ctx = typeof OffscreenCanvas === 'undefined' ? null : new OffscreenCanvas(1, 1).getContext('2d');
  if (!ctx) return null;
  // The browser reads names; an unknown one leaves the sentinel.
  ctx.fillStyle = '#010203';
  ctx.fillStyle = c;
  const v = String(ctx.fillStyle);
  return v !== '#010203' && /^#[0-9a-f]{6}$/i.test(v) ? parseInt(v.slice(1), 16) : null;
}

/** A CSS colour as "#rrggbb", lifted like legibleRgb; null if it isn't one. */
export function legibleCss(c: string | null | undefined): string | null {
  const rgb = c ? cssRgb(c) : null;
  return rgb === null ? null : `#${legibleRgb(rgb).toString(16).padStart(6, '0')}`;
}
