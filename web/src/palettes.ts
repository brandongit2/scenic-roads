// Colour ramps, baked into a 256 × N RGBA lookup texture for the road shader.
// Polynomial fits of matplotlib's viridis / magma / plasma (Matt Zucker) and
// Google's turbo. Each ramp is trimmed at the dark end so it stays legible on
// the dark background.

type RGB = [number, number, number];

function poly(c: number[][], t: number): RGB {
  const out: RGB = [0, 0, 0];
  for (let k = 0; k < 3; k++) {
    let v = 0;
    for (let i = c.length - 1; i >= 0; i--) v = v * t + c[i][k];
    out[k] = v;
  }
  return out;
}

const VIRIDIS = [
  [0.2777273272234177, 0.005407344544966578, 0.3340998053353061],
  [0.1050930431085774, 1.404613529898575, 1.384590162594685],
  [-0.3308618287255563, 0.214847559468213, 0.09509516302823659],
  [-4.634230498983486, -5.799100973351585, -19.33244095627987],
  [6.228269936347081, 14.17993336680509, 56.69055260068105],
  [4.776384997670288, -13.74514537774601, -65.35303263337234],
  [-5.435455855934631, 4.645852612178535, 26.3124352495832],
];
const MAGMA = [
  [-0.002136485053939582, -0.000749655052795221, -0.005386127855323933],
  [0.2516605407371642, 0.6775232436837668, 2.494026599312351],
  [8.353717279216625, -3.577719514958484, 0.3144679030132573],
  [-27.66873308576866, 14.26473078096533, -13.64921318813922],
  [52.17613981234068, -27.94360607168351, 12.94416944238394],
  [-50.76852536473588, 29.04658282127291, 4.23415299384598],
  [18.65570506591883, -11.48977351997711, -5.601961508734096],
];
const PLASMA = [
  [0.05873234392399702, 0.02333670892565664, 0.5433401826748754],
  [2.176514634195958, 0.2383834171260182, 0.7539604599784036],
  [-2.689460476458034, -7.455851135738909, 3.110799939717086],
  [6.130348345893603, 42.3461881477227, -28.51885465332158],
  [-11.10743619062271, -82.66631109428045, 60.13984767418263],
  [10.02306557647065, 71.41361770095349, -54.07218655560067],
  [-3.658713842777788, -22.93153465461149, 18.19190778539828],
];

function turbo(x: number): RGB {
  const v4 = [1, x, x * x, x * x * x];
  const v2 = [v4[2] * v4[2], v4[3] * v4[2]];
  const d = (a: number[], b: number[]) => a.reduce((s, v, i) => s + v * b[i], 0);
  return [
    d(v4, [0.13572138, 4.6153926, -42.66032258, 132.13108234]) + d(v2, [-152.94239396, 59.28637943]),
    d(v4, [0.09140261, 2.19418839, 4.84296658, -14.18503333]) + d(v2, [4.27729857, 2.82956604]),
    d(v4, [0.1066733, 12.64194608, -60.58204836, 110.36276771]) + d(v2, [-89.90310912, 27.34824973]),
  ];
}

function hexStops(stops: [number, string][]): (t: number) => RGB {
  const cs = stops.map(([t, h]) => [t, parseInt(h.slice(1, 3), 16) / 255, parseInt(h.slice(3, 5), 16) / 255, parseInt(h.slice(5, 7), 16) / 255]);
  return (t) => {
    for (let i = 1; i < cs.length; i++) {
      if (t <= cs[i][0]) {
        const u = (t - cs[i - 1][0]) / (cs[i][0] - cs[i - 1][0]);
        return [1, 2, 3].map((k) => cs[i - 1][k] + (cs[i][k] - cs[i - 1][k]) * u) as RGB;
      }
    }
    return [cs[cs.length - 1][1], cs[cs.length - 1][2], cs[cs.length - 1][3]];
  };
}

export interface Palette {
  key: string;
  label: string;
  fn: (t: number) => RGB;
}

const trim = (f: (t: number) => RGB, a: number, b = 1) => (t: number) => f(a + (b - a) * t);

export const PALETTES: Palette[] = [
  { key: 'viridis', label: 'Viridis', fn: trim((t) => poly(VIRIDIS, t), 0.14) },
  { key: 'magma', label: 'Magma', fn: trim((t) => poly(MAGMA, t), 0.22, 0.98) },
  { key: 'plasma', label: 'Plasma', fn: trim((t) => poly(PLASMA, t), 0.06, 0.96) },
  { key: 'turbo', label: 'Turbo', fn: trim(turbo, 0.06, 0.96) },
  {
    key: 'hypso',
    label: 'Hypsometric',
    fn: hexStops([
      [0, '#2f7d57'], [0.18, '#6fa65a'], [0.36, '#c8c46e'], [0.54, '#c9965a'],
      [0.72, '#a86b53'], [0.88, '#b89a92'], [1, '#f4f1ec'],
    ]),
  },
  {
    // For signed metrics (ridge ↔ valley): cool below, neutral grey at the middle, warm above.
    key: 'diverge',
    label: 'Diverging',
    fn: hexStops([[0, '#3d7fd9'], [0.25, '#79a9df'], [0.5, '#8b919c'], [0.75, '#e6a25c'], [1, '#ff6a3d']]),
  },
];

export const LUT_W = 256;

export function buildLut(): Uint8Array {
  const data = new Uint8Array(LUT_W * PALETTES.length * 4);
  PALETTES.forEach((p, row) => {
    for (let i = 0; i < LUT_W; i++) {
      const c = p.fn(i / (LUT_W - 1));
      const o = (row * LUT_W + i) * 4;
      for (let k = 0; k < 3; k++) data[o + k] = Math.max(0, Math.min(255, Math.round(c[k] * 255)));
      data[o + 3] = 255;
    }
  });
  return data;
}

export function paletteCss(key: string, steps = 12): string {
  const p = PALETTES.find((x) => x.key === key) ?? PALETTES[0];
  const parts: string[] = [];
  for (let i = 0; i <= steps; i++) {
    const c = p.fn(i / steps).map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255));
    parts.push(`rgb(${c[0]},${c[1]},${c[2]}) ${((i / steps) * 100).toFixed(1)}%`);
  }
  return `linear-gradient(90deg, ${parts.join(', ')})`;
}

export function paletteRgb(key: string, t: number): string {
  const p = PALETTES.find((x) => x.key === key) ?? PALETTES[0];
  const c = p.fn(Math.max(0, Math.min(1, t))).map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255));
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}
