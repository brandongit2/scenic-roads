type Attrs = Record<string, string | number | boolean | ((e: any) => void) | undefined>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Attrs = {}, ...kids: (Node | string | null | undefined)[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === false) continue;
    if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
    else if (k === 'class') el.className = String(v);
    else if (k === 'html') el.innerHTML = String(v);
    else if (v === true) el.setAttribute(k, '');
    else el.setAttribute(k, String(v));
  }
  for (const c of kids) if (c !== null && c !== undefined) el.append(c);
  return el;
}

const nf0 = new Intl.NumberFormat('en-CA', { maximumFractionDigits: 0 });
const nf1 = new Intl.NumberFormat('en-CA', { minimumFractionDigits: 1, maximumFractionDigits: 1 });

/** First letter upper-case, the rest as is ("canal de Coteau-du-Lac" → "Canal de Coteau-du-Lac"). */
export const cap = (s: string | null | undefined): string => (s ? s.charAt(0).toLocaleUpperCase() + s.slice(1) : s ?? '');

export const fmt = {
  m: (v: number) => `${nf0.format(v)} m`,
  m1: (v: number) => `${nf1.format(v)} m`,
  km: (v: number) => (v < 10 ? `${nf1.format(v)} km` : `${nf0.format(v)} km`),
  dist: (m: number) => (m < 1000 ? `${nf0.format(m)} m` : m < 10000 ? `${nf1.format(m / 1000)} km` : `${nf0.format(m / 1000)} km`),
  pct: (v: number, d = 1) => `${v.toFixed(d)} %`,
  n: (v: number) => nf0.format(v),
  big: (v: number) => (v >= 1e6 ? `${(v / 1e6).toFixed(1)} M` : v >= 1e3 ? `${(v / 1e3).toFixed(0)} k` : `${v}`),
  mb: (b: number) => `${(b / 1e6).toFixed(0)} MB`,
  coord: (lat: number, lng: number) => `${Math.abs(lat).toFixed(5)}° ${lat >= 0 ? 'N' : 'S'}, ${Math.abs(lng).toFixed(5)}° ${lng >= 0 ? 'E' : 'W'}`,
};

/** Round a range to "nice" tick values. */
export function niceStep(span: number, target: number): number {
  const raw = span / target;
  const p = 10 ** Math.floor(Math.log10(raw));
  const r = raw / p;
  return (r < 1.5 ? 1 : r < 3 ? 2 : r < 7 ? 5 : 10) * p;
}

export function toast(msg: string) {
  const t = document.getElementById('toast')!;
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout((t as any)._h);
  (t as any)._h = setTimeout(() => t.classList.remove('show'), 1600);
}

export function setupCanvas(c: HTMLCanvasElement): CanvasRenderingContext2D {
  const dpr = window.devicePixelRatio || 1;
  const r = c.getBoundingClientRect();
  const w = Math.max(1, Math.round(r.width * dpr)), hh = Math.max(1, Math.round(r.height * dpr));
  if (c.width !== w || c.height !== hh) {
    c.width = w;
    c.height = hh;
  }
  const ctx = c.getContext('2d')!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}
