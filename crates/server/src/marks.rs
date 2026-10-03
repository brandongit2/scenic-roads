//! Landmarks (docs/phase5.md): what the In view panel and the Sights list compute, as the app's
//! landmarks worker did — the prominence score, heritage tiers and the stops' filters — so the
//! server can answer them over its per-area columns. Each function is a port of the app's, value
//! for value (web/src/basemap.ts landmarkScoreOf, heritageTierOf; web/src/stopfilters.ts).

use pipeline::marks::{axis_pos, Axis, FilterDef, FilterKind as Kind, FILTERS};
use serde::Deserialize;
use std::collections::BTreeMap;

pub use pipeline::marks::score;

/// One filter's setting (min / max: 0 = no limit on that side).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub struct Setting {
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub min: f64,
    #[serde(default)]
    pub max: f64,
}

/// A feature's values for the filters: a number, or none (`typeof v !== 'number'` in the app).
pub trait Props {
    fn num(&self, prop: &str) -> Option<f64>;
}

impl Props for serde_json::Map<String, serde_json::Value> {
    fn num(&self, prop: &str) -> Option<f64> {
        self.get(prop).and_then(serde_json::Value::as_f64)
    }
}

/// The filters of an overlay that are on and restrict something.
fn active<'a>(ov: &str, set: &'a BTreeMap<String, Setting>) -> Vec<(&'static FilterDef, Setting)> {
    FILTERS
        .iter()
        .filter(|d| d.ov == ov)
        .filter_map(|d| {
            let s = *set.get(d.key)?;
            let on = s.on && (matches!(d.kind, Kind::Flag { .. }) || s.min != 0.0 || s.max != 0.0);
            on.then_some((d, s))
        })
        .collect()
}

/// The overlay's filters as one test (None: no filter on). A missing value passes a range when
/// `keep_unknown`; a flag must be set.
pub fn pass_fn(ov: &str, set: &BTreeMap<String, Setting>, keep_unknown: bool) -> Option<impl Fn(&dyn Props) -> bool> {
    let act = active(ov, set);
    if act.is_empty() {
        return None;
    }
    Some(move |p: &dyn Props| {
        act.iter().all(|(d, s)| {
            let v = p.num(d.prop);
            match d.kind {
                // `Math.floor((Number(v) || 0) / bit) % 2 === 1`, `Number(v) === 1`
                Kind::Flag { bit: Some(b) } => (v.unwrap_or(0.0) / b as f64).floor().rem_euclid(2.0) == 1.0,
                Kind::Flag { bit: None } => v == Some(1.0),
                Kind::Range { .. } => match v {
                    None => keep_unknown,
                    Some(v) => (s.min == 0.0 || v >= s.min) && (s.max == 0.0 || v <= s.max),
                },
            }
        })
    })
}

/// Bins of a range filter's histogram.
pub const FILTER_BINS: usize = 128;

/// The histograms of an overlay's range filters over the features added: each filter's over the
/// features the overlay's other filters keep (so its own limits show what they leave out).
pub struct FilterHists {
    hs: Vec<(&'static FilterDef, Axis, f64, f64, Vec<f64>, u64)>,
    others: Vec<Option<Box<dyn Fn(&dyn Props) -> bool + Send + Sync>>>,
}

impl FilterHists {
    pub fn new(ov: &str, set: &BTreeMap<String, Setting>, keep_unknown: bool) -> FilterHists {
        let defs: Vec<&'static FilterDef> = FILTERS.iter().filter(|d| d.ov == ov && matches!(d.kind, Kind::Range { .. })).collect();
        let others = defs
            .iter()
            .map(|d| {
                let rest: BTreeMap<String, Setting> = set.iter().filter(|(k, _)| k.as_str() != d.key).map(|(k, v)| (k.clone(), *v)).collect();
                pass_fn(ov, &rest, keep_unknown).map(|f| Box::new(f) as Box<dyn Fn(&dyn Props) -> bool + Send + Sync>)
            })
            .collect();
        let hs = defs
            .iter()
            .map(|d| {
                let Kind::Range { domain, axis, .. } = d.kind else { unreachable!() };
                let (a, b) = (axis_pos(axis, domain.0), axis_pos(axis, domain.1));
                (*d, axis, a, FILTER_BINS as f64 / (b - a), vec![0.0; FILTER_BINS], 0u64)
            })
            .collect();
        FilterHists { hs, others }
    }

    pub fn add(&mut self, p: &dyn Props) {
        for (j, (d, axis, a, k, bins, n)) in self.hs.iter_mut().enumerate() {
            let Some(v) = p.num(d.prop) else { continue };
            if self.others[j].as_ref().is_some_and(|f| !f(p)) {
                continue;
            }
            let i = ((axis_pos(*axis, v) - *a) * *k).floor().clamp(0.0, (FILTER_BINS - 1) as f64) as usize;
            bins[i] += 1.0;
            *n += 1;
        }
    }

    /// Per filter key, its bins and how many values they hold.
    pub fn done(self) -> BTreeMap<&'static str, (Vec<f64>, u64)> {
        self.hs.into_iter().map(|(d, _, _, _, bins, n)| (d.key, (bins, n))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipeline::marks::heritage_tier;
    use serde_json::json;

    fn props(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn score_as_the_app() {
        // landmarkScoreOf(fa, ia, b) for a few values, worked out in JS.
        assert!((score(2.5, 10.0, 0.0) - 0.5).abs() < 1e-12);
        assert!((score(7.0, 10.0, 0.0) - 1.0).abs() < 1e-12);
        // log10(10) + 1.3 = 2.3; / 5.6
        assert!((score(0.0, 10.0, 1.0) - 2.3 / 5.6).abs() < 1e-12);
        // ia below 0.05 counts as 0.05: log10(0.05) + 1.3 < 0, clamped to 0.
        assert_eq!(score(0.0, 0.0, 1.0), 0.0);
        assert!((score(2.5, 10.0, 0.5) - (0.25 + 0.5 * 2.3 / 5.6)).abs() < 1e-12);
    }

    #[test]
    fn tiers_as_the_app() {
        assert_eq!(heritage_tier(Some("n.mon"), Some(1.0)), "n.mon");
        assert_eq!(heritage_tier(None, Some(1.0)), "w.c");
        assert_eq!(heritage_tier(None, Some(3.0)), "n.second");
        assert_eq!(heritage_tier(None, None), "m.des");
        assert_eq!(heritage_tier(None, Some(0.0)), "m.des");
        assert_eq!(heritage_tier(None, Some(9.0)), "m.des");
    }

    #[test]
    fn filters_as_the_app() {
        let mut set = BTreeMap::new();
        set.insert("peak.pr".to_string(), Setting { on: true, min: 100.0, max: 0.0 });
        set.insert("peak.ele".to_string(), Setting { on: false, min: 2000.0, max: 0.0 });
        let f = pass_fn("peak", &set, false).unwrap();
        assert!(f(&props(json!({"pr": 150, "ele": 10}))));
        assert!(!f(&props(json!({"pr": 50}))));
        assert!(!f(&props(json!({"ele": 3000}))), "no prominence, unknowns not kept");
        let f = pass_fn("peak", &set, true).unwrap();
        assert!(f(&props(json!({"ele": 3000}))), "unknowns kept");
        // Flags: a bit of `fac`, or the value 1.
        let mut set = BTreeMap::new();
        set.insert("rest.shelter".to_string(), Setting { on: true, min: 0.0, max: 0.0 });
        let f = pass_fn("rest", &set, true).unwrap();
        assert!(f(&props(json!({"fac": 5}))) && !f(&props(json!({"fac": 3}))) && !f(&props(json!({}))));
        let mut set = BTreeMap::new();
        set.insert("viewpoint.pan".to_string(), Setting { on: true, min: 0.0, max: 0.0 });
        let f = pass_fn("viewpoint", &set, true).unwrap();
        assert!(f(&props(json!({"pan": 1}))) && !f(&props(json!({"pan": 0}))) && !f(&props(json!({}))));
        // A range that's on with no limits doesn't filter.
        let mut set = BTreeMap::new();
        set.insert("peak.pr".to_string(), Setting { on: true, min: 0.0, max: 0.0 });
        assert!(pass_fn("peak", &set, false).is_none());
    }

    #[test]
    fn filter_histograms_as_the_app() {
        let mut set = BTreeMap::new();
        set.insert("peak.pr".to_string(), Setting { on: true, min: 100.0, max: 0.0 });
        let mut h = FilterHists::new("peak", &set, false);
        h.add(&props(json!({"pr": 150, "ele": 1000, "is": 1})));
        h.add(&props(json!({"pr": 50, "ele": 1000, "is": 1})));
        let d = h.done();
        // Prominence's own histogram ignores its limit: both; elevation's only the one it keeps.
        assert_eq!(d["peak.pr"].1, 2);
        assert_eq!(d["peak.ele"].1, 1);
        // ele 1000 on 0–6000 lin: bin floor(1000 / 6000 × 128) = 21.
        assert_eq!(d["peak.ele"].0[21], 1.0);
        // An age axis: year 1900 → −log10(130).
        assert!((axis_pos(Axis::Age, 1900.0) + 130f64.log10()).abs() < 1e-12);
    }
}

// ---- the In view query and the points by view (docs/phase5.md), over markdata --------------------

use crate::markview::MarkView;
use crate::pages::Lru;
use crate::S;
use axum::{
    extract::{Path, Query, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use pipeline::marks::{self as pm, flag, Cell, MarkPt, MarkTile, KINDS};
use anyhow::Context;
use rayon::prelude::*;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

/// Bins of the in-view score histogram (the worker's HIST_BINS).
const HIST_BINS: usize = 512;
/// The most sized points beyond the tiles a view gets (`extra`).
const EXTRA_MAX: usize = 5000;

/// One kind to consider (the worker's KindQuery): its layer, filters and switched-off tiers.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct KindQ {
    pub k: String,
    #[serde(default)]
    pub layer: String,
    #[serde(default)]
    pub filters: BTreeMap<String, Setting>,
    #[serde(default, rename = "keepUnknown")]
    pub keep_unknown: bool,
    #[serde(default)]
    pub hists: bool,
    #[serde(default)]
    pub off: Vec<String>,
}

/// `POST /api/marks/view`: the worker's `query`, plus what the tiles at `tz` lack.
#[derive(Deserialize, Debug)]
pub struct ViewQ {
    #[serde(default)]
    pub outline: Vec<[f64; 2]>,
    pub bounds: [f64; 4],
    pub balance: f64,
    pub kinds: Vec<KindQ>,
    pub top: usize,
    pub ranks: [f64; 2],
    /// The zoom of the tiles the client shows (6: z6 blocks, which hold every point).
    #[serde(default = "blocks_zoom")]
    pub tz: u8,
    /// The size range when it's locked (auto off).
    #[serde(default)]
    pub range: Option<[f64; 2]>,
    /// The extras the client holds.
    #[serde(default)]
    pub have: Vec<u64>,
    /// The catalog the client's points came from.
    #[serde(default)]
    pub v: Option<u64>,
}

fn blocks_zoom() -> u8 {
    pm::KZ_NONE
}

/// The view's test (the worker's inOutline): inside the outline (with its bounding box first), or
/// without a usable outline the bounds (west > east: across the antimeridian).
pub struct Region {
    poly: Vec<[f64; 2]>,
    bbox: [f64; 4],
    bounds: [f64; 4],
}

impl Region {
    pub fn new(poly: Vec<[f64; 2]>, bounds: [f64; 4]) -> Region {
        let (mut w, mut s, mut e, mut n) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &[x, y] in &poly {
            w = w.min(x);
            e = e.max(x);
            s = s.min(y);
            n = n.max(y);
        }
        Region { poly, bbox: [w, s, e, n], bounds }
    }

    pub fn test(&self, x: f64, y: f64) -> bool {
        if self.poly.len() < 3 {
            let [w0, s0, e0, n0] = self.bounds;
            return y >= s0 && y <= n0 && if w0 <= e0 { x >= w0 && x <= e0 } else { x >= w0 || x <= e0 };
        }
        let [w, s, e, n] = self.bbox;
        if x < w || x > e || y < s || y > n {
            return false;
        }
        let p = &self.poly;
        let mut inside = false;
        let mut j = p.len() - 1;
        for i in 0..p.len() {
            let ([xi, yi], [xj, yj]) = (p[i], p[j]);
            if (yi > y) != (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    /// The z6 tiles a point passing the test can be in (a tile's margin around the box).
    pub fn tiles(&self) -> Vec<(u32, u32)> {
        let [w, s, e, n] = if self.poly.len() < 3 { self.bounds } else { self.bbox };
        if !(s <= n) || !w.is_finite() || !e.is_finite() {
            return Vec::new();
        }
        let lons: Vec<(f64, f64)> = if self.poly.len() < 3 && w > e { vec![(w, 180.0), (-180.0, e)] } else { vec![(w, e)] };
        let (lat_n, lat_s) = (n.clamp(-85.06, 85.06), s.clamp(-85.06, 85.06));
        let mut out = Vec::new();
        for (a, b) in lons {
            if a > 180.0 || b < -180.0 {
                continue;
            }
            let (x0, y0) = pm::tile_at(a.max(-180.0), lat_n, 6);
            let (x1, y1) = pm::tile_at(b.min(180.0), lat_s, 6);
            for x in x0.saturating_sub(1)..=(x1 + 1).min(63) {
                for y in y0.saturating_sub(1)..=(y1 + 1).min(63) {
                    out.push((x, y));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// A row's values for the filters (its kind's fields, NaN: none).
struct RowProps<'a> {
    fields: &'a [&'static str],
    vals: &'a [f64],
}

impl Props for RowProps<'_> {
    fn num(&self, prop: &str) -> Option<f64> {
        let i = self.fields.iter().position(|f| *f == prop)?;
        let v = self.vals[i];
        (!v.is_nan()).then_some(v)
    }
}

/// A kind's request resolved: its index, filters and switched-off tiers.
struct KindPlan {
    k: usize,
    pass: Option<Box<dyn Fn(&dyn Props) -> bool + Send + Sync>>,
    off: HashSet<u8>,
}

impl KindPlan {
    fn new(q: &KindQ) -> Option<KindPlan> {
        let k = pm::kind_index(&q.k)?;
        let pass = pass_fn(&q.k, &q.filters, q.keep_unknown).map(|f| Box::new(f) as Box<dyn Fn(&dyn Props) -> bool + Send + Sync>);
        let off = q.off.iter().filter_map(|t| pm::tier_index(t)).collect();
        Some(KindPlan { k, pass, off })
    }
}

/// A point in view that the kind's filters keep.
#[derive(Clone, Copy)]
struct Cand {
    score: f64,
    qi: u16,
    rank: u32,
    view: u32,
    row: u32,
    pt: MarkPt,
}

/// What one z6 tile adds to a view.
#[derive(Default)]
struct Part {
    cands: Vec<Cand>,
    /// Per kind asking for histograms: the values of every one of the kind in view (minus
    /// switched-off tiers), whatever its own filters.
    hist_vals: Vec<(u16, Vec<f64>)>,
}

/// The order of the worker's lists: score, then the kind's place in the request, then rank.
fn by_rank(a: &Cand, b: &Cand) -> std::cmp::Ordering {
    b.score.total_cmp(&a.score).then(a.qi.cmp(&b.qi)).then(a.rank.cmp(&b.rank))
}

/// The worker's spreadRange (overlays.ts): at least 0.02 wide.
fn spread_range(lo: f64, hi: f64) -> [f64; 2] {
    if hi - lo >= 0.02 {
        return [lo, hi];
    }
    let m = (lo + hi) / 2.0;
    [(m - 0.01).max(0.0), (m + 0.01).min(1.0)]
}

/// Lean properties with their display names (`main`/`sub`, and `cmain`/`csub` for a World
/// Heritage component's own name), as the layer files get them.
fn named_props(s: &crate::AppState, raw: &[u8], lon: f64, lat: f64) -> Map<String, Value> {
    let mut props: Map<String, Value> = serde_json::from_slice(raw).unwrap_or_default();
    if let Some(key) = crate::cache::NAME_KEYS.iter().find(|k| props.get(**k).and_then(|x| x.as_str()).is_some_and(|x| !x.is_empty())) {
        let own = crate::cache::EN_KEYS.iter().find_map(|k| props.get(*k).and_then(|x| x.as_str()).filter(|x| !x.is_empty())).map(str::to_owned);
        crate::cache::put_names(s, &mut props, key, own.as_deref(), [lon, lat], "");
        crate::cache::put_names(s, &mut props, "cn", None, [lon, lat], "c");
    }
    props
}

/// The markdata tiles a region can need.
fn views_for(s: &S, region: &Region) -> anyhow::Result<Vec<Arc<MarkView>>> {
    let cat = s.data.catalog();
    let want: Vec<String> = region.tiles().into_iter().map(|(x, y)| format!("6/{x}/{y}")).filter(|t| cat.markdata.contains_key(t)).collect();
    want.par_iter().filter_map(|t| s.data.markdata(t).transpose()).collect()
}

/// One tile's part of a view.
fn scan(v: &MarkView, region: &Region, plans: &[KindPlan], hists: &[bool], balance: f64, view: u32) -> anyhow::Result<Part> {
    let (_, pts, fvals) = v.scan()?;
    let (pts, fvals) = (pts.cast::<MarkPt>(), fvals.cast::<f64>());
    let mut part = Part::default();
    for (qi, p) in plans.iter().enumerate() {
        let r = v.range(p.k);
        let fields = &v.fields[p.k];
        let cols: Vec<&[f64]> = (0..fields.len()).map(|f| &fvals[v.fval_range(p.k, f)]).collect();
        let mut vals = vec![0.0; fields.len()];
        for (j, row) in r.clone().enumerate() {
            let pt = pts[row];
            if pt.flags & flag::COMPONENT != 0 || !region.test(pm::deg(pt.lon), pm::deg(pt.lat)) || p.off.contains(&pt.tier) {
                continue;
            }
            for (f, c) in cols.iter().enumerate() {
                vals[f] = c[j];
            }
            let props = RowProps { fields, vals: &vals };
            if hists[qi] {
                part.hist_vals.push((qi as u16, vals.clone()));
            }
            if p.pass.as_ref().is_some_and(|f| !f(&props)) {
                continue;
            }
            part.cands.push(Cand { score: score(pt.fa as f64, pt.ia as f64, balance), qi: qi as u16, rank: pt.rank, view, row: row as u32, pt });
        }
    }
    Ok(part)
}

/// The worker's query over markdata (see the module doc), as JSON.
pub fn view_json(s: &S, q: &ViewQ) -> anyhow::Result<Value> {
    let region = Region::new(q.outline.clone(), q.bounds);
    let views = views_for(s, &region)?;
    let plans: Vec<(usize, KindPlan)> = q.kinds.iter().enumerate().filter_map(|(i, k)| KindPlan::new(k).map(|p| (i, p))).collect();
    let (qidx, plans): (Vec<usize>, Vec<KindPlan>) = plans.into_iter().unzip();
    let hists: Vec<bool> = qidx.iter().map(|&i| q.kinds[i].hists).collect();
    let parts: Vec<Part> = views.par_iter().enumerate().map(|(vi, v)| scan(v, &region, &plans, &hists, q.balance, vi as u32)).collect::<anyhow::Result<_>>()?;
    let mut cands: Vec<Cand> = parts.iter().flat_map(|p| p.cands.iter().copied()).collect();

    // The histogram and the scores at the ranks (from the scores as f32, as the worker sorts them).
    let mut hist = vec![0.0f64; HIST_BINS];
    for c in &cands {
        hist[((c.score * HIST_BINS as f64).floor()).clamp(0.0, (HIST_BINS - 1) as f64) as usize] += 1.0;
    }
    let mut sorted: Vec<f32> = cands.iter().map(|c| c.score as f32).collect();
    sorted.sort_by(f32::total_cmp);
    let at = |rank: f64| -> f64 {
        let n = sorted.len() as f64;
        sorted[(n - rank.min(n)).max(0.0) as usize] as f64
    };
    let at_ranks = (!sorted.is_empty()).then(|| [at(q.ranks[0]), at(q.ranks[1])]);

    let item = |c: &Cand| -> anyhow::Result<Value> {
        let v = &views[c.view as usize];
        let (lon, lat) = (pm::deg(c.pt.lon), pm::deg(c.pt.lat));
        let props = named_props(s, &v.props(c.row as usize)?, lon, lat);
        let kq = &q.kinds[qidx[c.qi as usize]];
        Ok(json!({ "k": kq.k, "layer": kq.layer, "score": c.score, "props": props, "lngLat": [lon, lat] }))
    };
    // Per kind: its count and best-known named point (the highest fame, the first in rank order).
    let mut by_kind = Vec::new();
    for (qi, &i) in qidx.iter().enumerate() {
        let kq = &q.kinds[i];
        let mine = cands.iter().filter(|c| c.qi as usize == qi);
        let n = mine.clone().count();
        let best = mine.filter(|c| c.pt.flags & flag::NAMED != 0).min_by(|a, b| (b.pt.fa as f64).total_cmp(&(a.pt.fa as f64)).then(a.rank.cmp(&b.rank)));
        let best = match best {
            Some(c) => {
                let v = &views[c.view as usize];
                let (lon, lat) = (pm::deg(c.pt.lon), pm::deg(c.pt.lat));
                let props = named_props(s, &v.props(c.row as usize)?, lon, lat);
                json!({ "name": props.get("name").and_then(Value::as_str).unwrap_or(""), "lngLat": [lon, lat], "layer": kq.layer, "props": props })
            }
            None => Value::Null,
        };
        by_kind.push(json!({ "key": kq.k, "n": n, "best": best }));
    }
    cands.sort_by(by_rank);
    let top: Vec<Value> = cands.iter().take(q.top).map(item).collect::<anyhow::Result<_>>()?;
    let mut top_by_kind = Map::new();
    for (qi, &i) in qidx.iter().enumerate() {
        let list: Vec<Value> = cands.iter().filter(|c| c.qi as usize == qi).take(q.top).map(item).collect::<anyhow::Result<_>>()?;
        top_by_kind.insert(q.kinds[i].k.clone(), Value::Array(list));
    }
    // The open filters' histograms.
    let mut fhist = Map::new();
    for (qi, &i) in qidx.iter().enumerate() {
        let kq = &q.kinds[i];
        if !kq.hists {
            continue;
        }
        let fields = pm::fields(&kq.k);
        let mut fh = FilterHists::new(&kq.k, &kq.filters, kq.keep_unknown);
        for p in &parts {
            for (hq, vals) in &p.hist_vals {
                if *hq as usize == qi {
                    fh.add(&RowProps { fields: &fields, vals });
                }
            }
        }
        for (key, (bins, n)) in fh.done() {
            fhist.insert(key.to_string(), json!({ "bins": bins, "n": n }));
        }
    }
    // The highest named peak in view: the first in view in the summits list's order.
    let summit = views
        .iter()
        .flat_map(|v| v.summits.iter().enumerate().map(move |(i, r)| (v, i, r)))
        .filter(|(_, _, r)| region.test(pm::deg(r.lon), pm::deg(r.lat)))
        .min_by_key(|(_, _, r)| r.rank);
    let summit = match summit {
        Some((v, i, r)) => {
            let (lon, lat) = (pm::deg(r.lon), pm::deg(r.lat));
            let name = v.summit_name(i)?;
            let d = s.names.display(names::Kind::Place, name, None, lon, lat);
            let shown = match d.sub {
                Some(sub) if !d.main.is_empty() => format!("{} ({sub})", d.main),
                _ => d.main.to_string(),
            };
            json!({ "name": shown, "ele": r.ele, "lngLat": [lon, lat] })
        }
        None => Value::Null,
    };
    let mut out = json!({
        "hist": hist, "n": cands.len(), "atRanks": at_ranks, "byKind": by_kind, "top": top,
        "topByKind": top_by_kind, "fhist": fhist, "summit": summit,
    });
    // What the tiles at `tz` lack: the points sized at this view's range they don't keep.
    if q.tz < pm::KZ_NONE {
        let lo = match (q.range, at_ranks) {
            (Some(r), _) => Some(r[0]),
            (None, Some([a, b])) => Some(spread_range(a, b)[0]),
            _ => None,
        };
        if let Some(lo) = lo {
            let extra: Vec<&Cand> = cands.iter().filter(|c| c.pt.kz > q.tz && c.score > lo).take(EXTRA_MAX).collect();
            let have: HashSet<u64> = q.have.iter().copied().collect();
            let mut ids = Vec::with_capacity(extra.len());
            let mut by: BTreeMap<String, Vec<&Cand>> = BTreeMap::new();
            for c in &extra {
                let v = &views[c.view as usize];
                let id = v.ids.get(c.row as usize)?;
                ids.push(id);
                if !have.contains(&id) {
                    by.entry(q.kinds[qidx[c.qi as usize]].k.clone()).or_default().push(c);
                }
            }
            let mut kinds = Map::new();
            for (k, list) in by {
                kinds.insert(k, extra_json(s, &views, &list)?);
            }
            out["extra"] = json!({ "ids": ids, "kinds": kinds });
        }
    }
    Ok(out)
}

/// Points as columns (the marks tile format's fields, NaN as null), for `extra`.
fn extra_json(s: &S, views: &[Arc<MarkView>], list: &[&Cand]) -> anyhow::Result<Value> {
    let num = |v: f64| if v.is_nan() { Value::Null } else { json!(v) };
    let (mut ids, mut lon, mut lat, mut fa, mut ia, mut mz, mut rank, mut kz, mut class, mut tier, mut flags, mut props) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    let mut fvals: Vec<Vec<Value>> = Vec::new();
    for c in list {
        let v = &views[c.view as usize];
        let row = c.row as usize;
        let p = c.pt;
        ids.push(v.ids.get(row)?);
        lon.push(p.lon);
        lat.push(p.lat);
        fa.push(num(p.fa as f64));
        ia.push(num(p.ia as f64));
        mz.push(num(p.mz as f64));
        rank.push(p.rank);
        kz.push(p.kz);
        class.push(p.class);
        tier.push(p.tier);
        flags.push(p.flags);
        let k = (0..KINDS.len()).find(|&k| v.range(k).contains(&row)).context("row outside every kind")?;
        let nf = v.fields[k].len();
        fvals.resize(nf, Vec::new());
        let j = row - v.range(k).start;
        for (f, col) in fvals.iter_mut().enumerate() {
            col.push(num(v.fvals.get(v.fval_range(k, f).start + j)?));
        }
        props.push(Value::Object(named_props(s, &v.props(row)?, pm::deg(p.lon), pm::deg(p.lat))));
    }
    Ok(json!({ "ids": ids, "lon": lon, "lat": lat, "fa": fa, "ia": ia, "mz": mz, "rank": rank, "kz": kz, "class": class, "tier": tier, "flags": flags, "fvals": fvals, "props": props }))
}

fn catalog_mismatch(s: &S, v: Option<u64>) -> bool {
    v.is_some_and(|v| v != s.data.catalog().n)
}

pub async fn view(State(s): State<S>, Json(q): Json<ViewQ>) -> Response {
    if catalog_mismatch(&s, q.v) {
        return StatusCode::CONFLICT.into_response();
    }
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || view_json(&s2, &q)).await {
        Ok(Ok(v)) => ([(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))], Json(v)).into_response(),
        Ok(Err(e)) => {
            eprintln!("marks view: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ---- tiles, blocks and specks ----------------------------------------------------------------------

/// Tiles made for serving (names attached, gzip'd), by key, under a byte budget.
static MADE: LazyLock<Mutex<Lru<String, Arc<Vec<u8>>>>> = LazyLock::new(|| Mutex::new(Lru::new(192 << 20)));

fn gzip(b: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(4));
    e.write_all(b).expect("gzip to memory");
    e.finish().expect("gzip to memory")
}

fn gunzip(b: &[u8]) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    if !b.starts_with(&[0x1f, 0x8b]) {
        return Ok(b.to_vec());
    }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(b).read_to_end(&mut out)?;
    Ok(out)
}

fn respond_tile(body: Arc<Vec<u8>>, etag: &str, versioned: bool) -> Response {
    let mut r = body.to_vec().into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    if let Ok(e) = HeaderValue::from_str(etag) {
        h.insert(header::ETAG, e);
    }
    h.insert(header::CACHE_CONTROL, crate::cache::cache_control(versioned, "no-cache"));
    r
}

/// A tile with names attached to its points' properties.
fn with_names(s: &S, mut t: MarkTile) -> MarkTile {
    let props: Vec<Vec<u8>> = t.props.iter().zip(&t.pts).map(|(p, pt)| serde_json::to_vec(&named_props(s, p, pm::deg(pt.lon), pm::deg(pt.lat))).unwrap_or_else(|_| p.clone())).collect();
    t.props = props;
    t
}

/// Serves a made tile: a 304 for its ETag, from memory, or made now.
async fn serve_made(s: S, key: String, etag: String, headers: HeaderMap, q: Option<String>, make: impl FnOnce(&S) -> anyhow::Result<Option<Vec<u8>>> + Send + 'static) -> Response {
    let versioned = crate::cache::versioned(q.as_deref());
    if crate::tiles::etag_match(&headers, &etag) {
        return crate::tiles::not_modified(&etag, versioned);
    }
    if let Some(b) = MADE.lock().unwrap().get(&key) {
        return respond_tile(b, &etag, versioned);
    }
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || make(&s2)).await {
        Ok(Ok(Some(raw))) => {
            let b = Arc::new(gzip(&raw));
            MADE.lock().unwrap().put(key, b.clone(), b.len() as u64);
            respond_tile(b, &etag, versioned)
        }
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("marks tile: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /api/marks/tile/{kind}/{z}/{x}/{y}` (z ≤ 5): a thinned tile.
pub async fn tile(State(s): State<S>, Path((kind, z, x, y)): Path<(String, u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    if pm::kind_index(&kind).is_none() || z > pm::THIN_MAX_Z || x >> z != 0 || y >> z != 0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let layer = format!("marks-{kind}");
    let (s2, l2) = (s.clone(), layer.clone());
    let h = match tokio::task::spawn_blocking(move || s2.data.tile_hash(&l2, z, x, y)).await {
        Ok(Ok(Some(h))) => h,
        Ok(Ok(None)) => return StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("{layer} {z}/{x}/{y}: {e:#}");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let nv = s.names.version_for_tile(z, x, y, 0.0);
    let etag = format!("\"{h:016x}-{nv:x}\"");
    let key = format!("t|{kind}|{z}/{x}/{y}|{h:x}|{nv:x}");
    serve_made(s, key, etag, headers, q, move |s| {
        let Some((b, _)) = s.data.tile(&layer, z, x, y)? else { return Ok(None) };
        let t = MarkTile::decode(&gunzip(b.bytes())?)?;
        Ok(Some(with_names(s, t).encode()))
    })
    .await
}

/// A z6 tile's points of one kind, as a marks tile.
fn block_tile(v: &MarkView, k: usize) -> anyhow::Result<MarkTile> {
    let r = v.range(k);
    let ids = v.ids.range(r.clone())?.into_owned();
    let pts = v.pts.range(r.clone())?.into_owned();
    let fvals = (0..v.fields[k].len()).map(|f| v.fvals.range(v.fval_range(k, f)).map(|c| c.into_owned())).collect::<anyhow::Result<_>>()?;
    let props = r.map(|i| v.props(i)).collect::<anyhow::Result<_>>()?;
    Ok(MarkTile { ids, fvals, pts, cells: Vec::new(), props })
}

/// `GET /api/marks/block/{kind}/6/{x}/{y}`: every point of the kind in the z6 tile.
pub async fn block(State(s): State<S>, Path((kind, z, x, y)): Path<(String, u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let Some(k) = pm::kind_index(&kind) else { return StatusCode::NOT_FOUND.into_response() };
    if z != 6 || x >> 6 != 0 || y >> 6 != 0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let tile = format!("6/{x}/{y}");
    let Some(content) = s.data.catalog().markdata.get(&tile).and_then(|l| s.data.content(l)) else { return StatusCode::NO_CONTENT.into_response() };
    let nv = s.names.version_for_tile(6, x, y, 0.0);
    let h = xxhash_rust::xxh3::xxh3_64(format!("{content}|{kind}").as_bytes());
    let etag = format!("\"{h:016x}-{nv:x}\"");
    let key = format!("b|{content}|{kind}|{nv:x}");
    serve_made(s, key, etag, headers, q, move |s| {
        let Some(v) = s.data.markdata(&tile)? else { return Ok(None) };
        if v.range(k).is_empty() {
            return Ok(None);
        }
        Ok(Some(with_names(s, block_tile(&v, k)?).encode()))
    })
    .await
}

/// A kind's filters, unknowns and switched-off tiers, as the app sends them (`q=` JSON).
#[derive(Deserialize, Default)]
pub struct SpeckQ {
    #[serde(default)]
    pub filters: BTreeMap<String, Setting>,
    #[serde(default, rename = "keepUnknown")]
    pub keep_unknown: bool,
    #[serde(default)]
    pub off: Vec<String>,
}

#[derive(Deserialize)]
pub struct QParam {
    #[serde(default)]
    pub q: Option<String>,
}

/// The z6 tiles under tile z/x/y (z ≤ 6) that have markdata.
fn z6_under(s: &S, z: u8, x: u32, y: u32) -> Vec<String> {
    let cat = s.data.catalog();
    let d = 6 - z;
    let mut out = Vec::new();
    for xx in (x << d)..((x + 1) << d) {
        for yy in (y << d)..((y + 1) << d) {
            let t = format!("6/{xx}/{yy}");
            if cat.markdata.contains_key(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// `GET /api/marks/specks/{kind}/{z}/{x}/{y}?q=…` (z ≤ 5): the speck cells of the points passing
/// the kind's filters that the thinned tile doesn't keep.
pub async fn specks(State(s): State<S>, Path((kind, z, x, y)): Path<(String, u8, u32, u32)>, Query(p): Query<QParam>, RawQuery(raw): RawQuery, headers: HeaderMap) -> Response {
    let Some(k) = pm::kind_index(&kind) else { return StatusCode::NOT_FOUND.into_response() };
    if z > pm::THIN_MAX_Z || x >> z != 0 || y >> z != 0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let qs = p.q.unwrap_or_default();
    let Ok(sq) = (if qs.is_empty() { Ok(SpeckQ::default()) } else { serde_json::from_str::<SpeckQ>(&qs) }) else { return StatusCode::BAD_REQUEST.into_response() };
    let tiles = z6_under(&s, z, x, y);
    let contents: Vec<String> = tiles.iter().filter_map(|t| s.data.catalog().markdata.get(t).and_then(|l| s.data.content(l))).collect();
    let h = xxhash_rust::xxh3::xxh3_64(format!("{}|{kind}|{z}/{x}/{y}|{qs}", contents.join(",")).as_bytes());
    let etag = format!("\"{h:016x}\"");
    let key = format!("s|{h:x}");
    serve_made(s, key, etag, headers, raw, move |s| {
        let pass = pass_fn(&kind, &sq.filters, sq.keep_unknown);
        let off: HashSet<u8> = sq.off.iter().filter_map(|t| pm::tier_index(t)).collect();
        let n = (1u64 << (z + pm::CELL_DZ)) as f64;
        let side = 1u32 << pm::CELL_DZ;
        let mut counts: BTreeMap<(u32, u8), u32> = BTreeMap::new();
        for t in &tiles {
            let Some(v) = s.data.markdata(t)? else { continue };
            let (_, pts, fvals) = v.scan()?;
            let (pts, fvals) = (pts.cast::<MarkPt>(), fvals.cast::<f64>());
            let fields = &v.fields[k];
            let cols: Vec<&[f64]> = (0..fields.len()).map(|f| &fvals[v.fval_range(k, f)]).collect();
            let mut vals = vec![0.0; fields.len()];
            for (j, row) in v.range(k).enumerate() {
                let pt = pts[row];
                if pt.kz <= z || pt.flags & flag::COMPONENT != 0 || off.contains(&pt.tier) {
                    continue;
                }
                for (f, c) in cols.iter().enumerate() {
                    vals[f] = c[j];
                }
                if pass.as_ref().is_some_and(|f| !f(&RowProps { fields, vals: &vals })) {
                    continue;
                }
                let (mx, my) = pm::merc(pm::deg(pt.lon), pm::deg(pt.lat));
                let cx = ((mx * n).floor() as u32).saturating_sub(x * side).min(side - 1);
                let cy = ((my * n).floor() as u32).saturating_sub(y * side).min(side - 1);
                *counts.entry((pm::morton(cx, cy), pt.tier)).or_default() += 1;
            }
        }
        let cells: Vec<Cell> = counts.into_iter().map(|((code, tier), count)| Cell { code, tier, count }).collect();
        Ok(Some(MarkTile { cells, ..Default::default() }.encode()))
    })
    .await
}

/// Worldwide filtered counts, per catalog, kind and query.
static COUNTS: LazyLock<Mutex<HashMap<(u64, String, String), (u64, u64)>>> = LazyLock::new(Default::default);

#[derive(Deserialize)]
pub struct CountQ {
    pub kind: String,
    #[serde(default)]
    pub q: Option<String>,
}

/// `GET /api/marks/count?kind=…&q=…`: how many of the kind pass its filters worldwide, of how many
/// (the worker's `count`).
pub async fn count(State(s): State<S>, Query(c): Query<CountQ>) -> Response {
    let Some(k) = pm::kind_index(&c.kind) else { return StatusCode::NOT_FOUND.into_response() };
    let qs = c.q.unwrap_or_default();
    let Ok(sq) = (if qs.is_empty() { Ok(SpeckQ::default()) } else { serde_json::from_str::<SpeckQ>(&qs) }) else { return StatusCode::BAD_REQUEST.into_response() };
    let n_cat = s.data.catalog().n;
    let key = (n_cat, c.kind.clone(), qs);
    if let Some((n, of)) = COUNTS.lock().unwrap().get(&key).copied() {
        return Json(json!({ "n": n, "of": of })).into_response();
    }
    let s2 = s.clone();
    let kind = c.kind.clone();
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, u64)> {
        let pass = pass_fn(&kind, &sq.filters, sq.keep_unknown);
        let off: HashSet<u8> = sq.off.iter().filter_map(|t| pm::tier_index(t)).collect();
        let tiles: Vec<String> = s2.data.catalog().markdata.keys().cloned().collect();
        let per: Vec<(u64, u64)> = tiles
            .par_iter()
            .map(|t| -> anyhow::Result<(u64, u64)> {
                let Some(v) = s2.data.markdata(t)? else { return Ok((0, 0)) };
                let (_, pts, fvals) = v.scan()?;
                let (pts, fvals) = (pts.cast::<MarkPt>(), fvals.cast::<f64>());
                let fields = &v.fields[k];
                let cols: Vec<&[f64]> = (0..fields.len()).map(|f| &fvals[v.fval_range(k, f)]).collect();
                let mut vals = vec![0.0; fields.len()];
                let (mut n, mut of) = (0u64, 0u64);
                for (j, row) in v.range(k).enumerate() {
                    let pt = pts[row];
                    if pt.flags & flag::COMPONENT != 0 {
                        continue;
                    }
                    of += 1;
                    if off.contains(&pt.tier) {
                        continue;
                    }
                    for (f, c) in cols.iter().enumerate() {
                        vals[f] = c[j];
                    }
                    if pass.as_ref().is_none_or(|f| f(&RowProps { fields, vals: &vals })) {
                        n += 1;
                    }
                }
                Ok((n, of))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(per.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1)))
    })
    .await;
    match got {
        Ok(Ok((n, of))) => {
            COUNTS.lock().unwrap().insert(key, (n, of));
            Json(json!({ "n": n, "of": of })).into_response()
        }
        Ok(Err(e)) => {
            eprintln!("marks count: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub struct DetailQ {
    pub at: String,
    #[serde(default)]
    pub v: Option<u64>,
}

/// `GET /api/marks/detail/{kind}/{id}?at=lon,lat`: the popup record (descriptions laid over), from
/// the z6 tile holding the point (or one next to it, for a point on an edge).
pub async fn detail(State(s): State<S>, Path((kind, id)): Path<(String, u64)>, Query(d): Query<DetailQ>) -> Response {
    let Some(k) = pm::kind_index(&kind) else { return StatusCode::NOT_FOUND.into_response() };
    if catalog_mismatch(&s, d.v) {
        return StatusCode::CONFLICT.into_response();
    }
    let mut it = d.at.split(',').map(|v| v.trim().parse::<f64>());
    let (Some(Ok(lon)), Some(Ok(lat))) = (it.next(), it.next()) else { return StatusCode::BAD_REQUEST.into_response() };
    let s2 = s.clone();
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<u8>>> {
        let (x, y) = pm::tile_at(lon, lat, 6);
        let near = [(0i64, 0i64), (-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];
        for (dx, dy) in near {
            let (tx, ty) = (x as i64 + dx, y as i64 + dy);
            if !(0..64).contains(&tx) || !(0..64).contains(&ty) {
                continue;
            }
            let Some(v) = s2.data.markdata(&format!("6/{tx}/{ty}"))? else { continue };
            if let Some(row) = v.find(k, id)? {
                return Ok(Some(v.info(row)?));
            }
        }
        Ok(None)
    })
    .await;
    match got {
        Ok(Ok(Some(info))) if !info.is_empty() => {
            let body = s.descriptions.apply(std::str::from_utf8(&info).unwrap_or("{}")).into_owned();
            ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=60")], body).into_response()
        }
        Ok(Ok(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("marks detail: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
