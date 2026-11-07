// Number formats the app shows: lengths, areas, signed heights, rates a day. One copy of each, so
// the sidebar, the hover readouts, the legends and the lists write a value alike.

const nf0 = new Intl.NumberFormat('en-CA', { maximumFractionDigits: 0 });
const nf1 = new Intl.NumberFormat('en-CA', { minimumFractionDigits: 1, maximumFractionDigits: 1 });

/** A count given a day, as written: a day from one a day ("24", "2.5"), else a week ("5", "0.7"). */
function rate(v: number): { n: string; week: boolean } {
  if (v >= 1) return { n: v >= 10 ? nf0.format(v) : String(+v.toFixed(1)), week: false };
  const wk = v * 7;
  return { n: wk >= 1.5 ? nf0.format(wk) : String(+wk.toFixed(1)), week: true };
}

export const fmt = {
  m: (v: number) => `${nf0.format(v)} m`,
  m1: (v: number) => `${nf1.format(v)} m`,
  km: (v: number) => (v < 10 ? `${nf1.format(v)} km` : `${nf0.format(v)} km`),
  dist: (m: number) => (m < 1000 ? `${nf0.format(m)} m` : m < 10000 ? `${nf1.format(m / 1000)} km` : `${nf0.format(m / 1000)} km`),
  pct: (v: number, d = 1) => `${v.toFixed(d)} %`,
  n: (v: number) => nf0.format(v),
  /** A whole number with its sign, a true minus: "+12", "−40", "0". */
  signed: (v: number) => {
    const n = nf0.format(Math.abs(v));
    return `${n === '0' ? '' : v > 0 ? '+' : '−'}${n}`;
  },
  /** An area in km² (the number): "0.42", "3.1", "1,250". */
  area: (v: number) => (v < 1 ? v.toFixed(2) : v < 10 ? v.toFixed(1) : nf0.format(v)),
  /** A rate given a day, in words, with what is counted: "24 a day", "5 trains a week". */
  perDay: (v: number, what = '') => {
    const r = rate(v);
    return `${r.n}${what ? ` ${what}` : ''} a ${r.week ? 'week' : 'day'}`;
  },
  /** A rate given a day, short (legends): "24/day", "5/wk". */
  perDayShort: (v: number) => {
    const r = rate(v);
    return `${r.n}/${r.week ? 'wk' : 'day'}`;
  },
  /** A rate given a day under a "/day" label: the count, a week's with "/wk" ("24", "5/wk"). */
  perDayNum: (v: number) => {
    const r = rate(v);
    return r.week ? `${r.n}/wk` : r.n;
  },
  big: (v: number) => (v >= 1e6 ? `${(v / 1e6).toFixed(1)} M` : v >= 1e3 ? `${(v / 1e3).toFixed(0)} k` : `${v}`),
  mb: (b: number) => `${(b / 1e6).toFixed(0)} MB`,
  coord: (lat: number, lng: number) => `${Math.abs(lat).toFixed(5)}° ${lat >= 0 ? 'N' : 'S'}, ${Math.abs(lng).toFixed(5)}° ${lng >= 0 ? 'E' : 'W'}`,
};
