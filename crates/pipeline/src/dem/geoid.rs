//! Heights brought to one reference, EGM2008, before the datasets are blended (docs/inputs.md §6.8,
//! step 3). Each roads' dataset declares its height reference (`heights`); the conversions are
//! declared sources too (inputs/sources/, role `geoid`, §6.9), each linking two references:
//!
//! - a **separation grid** (`geoid-grid`): a GeoTIFF in geographic degrees whose value at a point,
//!   read bilinearly, is what is added to a height in `from` to make it a height in `to` (a geoid
//!   model's undulation N: ellipsoidal h = H + N);
//! - a **Helmert transformation** (`helmert`) between two ellipsoidal frames: the change of a
//!   point's ellipsoidal height when its geocentric position goes from `from`'s frame to `to`'s.
//!
//! A dataset's conversion is the shortest chain of such links from its reference to EGM2008 (a
//! link walked from `to` to `from` subtracts), the references ordered by name where chains tie,
//! so the same declarations give the same chain. A link may be declared by several sources (a
//! grid per region: GEOID18 over the conterminous US, GEOID12B over Alaska): at a point, the first
//! by id with a value there serves. A point no source of a link covers keeps its height as sampled,
//! counted (`Stats::unconverted`). No reference, grid or parameter is named in code but the target.

use super::generic::{open_url, stored, Opts};
use crate::geotiff::Tiff;
use crate::inputs::sources::{Data, Source};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// The reference every roads' height is brought to.
pub const TARGET: &str = "EGM2008";

/// The fields of a geoid source's `params`.
pub const PARAMS: [&str; 10] =
    ["from", "to", "store", "translation_m", "rotation_mas", "scale_ppb", "convention", "rates", "epoch", "at"];

/// What's wrong with a `geoid` source's `[data]` (docs/inputs.md §6.9): `params.from` and `to`,
/// two references; a grid's `url` (one GeoTIFF; `params.store`: read from `sources/fetched/<id>/`
/// by the URL's last part, never fetched by a build); a Helmert's `url` (where its parameters are
/// published) and `translation_m` (3), `rotation_mas` (3), `scale_ppb`, `convention`
/// (`position-vector` or `coordinate-frame`), and optionally `rates` (the seven per year, in the
/// same units), `epoch` (their reference epoch) and `at` (the epoch applied).
pub fn params_problems(d: &Data) -> Vec<String> {
    let mut why = Vec::new();
    let p = &d.params;
    let text = |k: &str| p.get(k).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty());
    match (text("from"), text("to")) {
        (Some(a), Some(b)) if a == b => why.push("`params.from` and `params.to` are the same reference".into()),
        (Some(_), Some(_)) => {}
        _ => why.push("`params.from` and `params.to` are required: the two height references it links".into()),
    }
    if d.url.is_none() {
        why.push("a geoid source names one `url` (its grid, or where its parameters are published)".into());
    }
    let nums = |k: &str, n: usize| {
        p.get(k)
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == n && a.iter().all(|x| x.as_f64().is_some_and(f64::is_finite)))
            .is_some()
    };
    let num = |k: &str| p.get(k).and_then(|v| v.as_f64()).is_some_and(f64::is_finite);
    let allowed: &[&str] = if d.format == "helmert" {
        if !nums("translation_m", 3) || !nums("rotation_mas", 3) || !num("scale_ppb") {
            why.push("a Helmert names `params.translation_m` (3 numbers), `rotation_mas` (3) and `scale_ppb`".into());
        }
        if !matches!(text("convention"), Some("position-vector" | "coordinate-frame")) {
            why.push("`params.convention` isn't \"position-vector\" or \"coordinate-frame\"".into());
        }
        let timed = [p.contains_key("rates"), num("epoch"), num("at")];
        if timed.iter().any(|b| *b) && !(nums("rates", 7) && timed[1] && timed[2]) {
            why.push("a time-dependent Helmert names `params.rates` (7 numbers), `epoch` and `at` together".into());
        }
        &["from", "to", "translation_m", "rotation_mas", "scale_ppb", "convention", "rates", "epoch", "at"]
    } else {
        if p.get("store").is_some_and(|v| !v.is_boolean()) {
            why.push("`params.store` isn't true or false".into());
        }
        &["from", "to", "store"]
    };
    for k in p.keys().filter(|k| !allowed.contains(&k.as_str())) {
        why.push(format!("`params.{k}` isn't a field of a {} source", d.format));
    }
    why
}

/// One declared link as the elevation step applies it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Link {
    /// A separation grid: its URL, read from the store (`sources/fetched/<id>/`) when `store`.
    Grid {
        id: String,
        url: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        store: bool,
    },
    /// A Helmert transformation at its epoch applied: translation (m), rotation (mas), scale (ppb).
    Helmert { id: String, t: [f64; 3], r: [f64; 3], s: f64, frame: bool },
}

/// One step of a chain: from one reference to the next, by the first of `by` with a value at a
/// point, each added (`sign` 1) or subtracted (−1: the link walked backwards).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hop {
    pub from: String,
    pub to: String,
    pub by: Vec<(Link, i8)>,
}

/// A dataset's conversion to `TARGET`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conversion {
    pub hops: Vec<Hop>,
    /// The digest of the sources it reads (their ids and `keyed`), as the keys name it.
    pub digest: String,
}

/// A source made a link, with its two ends.
fn link_of(s: &Source) -> Result<(String, String, Link)> {
    let from = s.param_str("from")?.to_string();
    let to = s.param_str("to")?.to_string();
    let arr = |k: &str| -> Result<Vec<f64>> {
        s.param(k)?
            .as_array()
            .and_then(|a| a.iter().map(|x| x.as_f64()).collect::<Option<Vec<f64>>>())
            .with_context(|| format!("the geoid source {}: `params.{k}` isn't numbers", s.id))
    };
    let link = match s.data.format.as_str() {
        "geoid-grid" => Link::Grid {
            id: s.id.clone(),
            url: s.url()?.to_string(),
            store: s.data.params.get("store").and_then(|v| v.as_bool()).unwrap_or(false),
        },
        "helmert" => {
            let (mut t, mut r, mut sc) = (arr("translation_m")?, arr("rotation_mas")?, s.param("scale_ppb")?.as_f64());
            if s.data.params.contains_key("rates") {
                let rates = arr("rates")?;
                let dt = s.param("at")?.as_f64().unwrap_or(0.0) - s.param("epoch")?.as_f64().unwrap_or(0.0);
                for i in 0..3 {
                    t[i] += rates[i] * dt;
                    r[i] += rates[3 + i] * dt;
                }
                sc = sc.map(|v| v + rates[6] * dt);
            }
            Link::Helmert {
                id: s.id.clone(),
                t: [t[0], t[1], t[2]],
                r: [r[0], r[1], r[2]],
                s: sc.context("scale_ppb")?,
                frame: s.param_str("convention")? == "coordinate-frame",
            }
        }
        f => bail!("the geoid source {}: format {f}", s.id),
    };
    Ok((from, to, link))
}

/// The conversion of heights in `from` to `TARGET` by the declared `geoid` sources: None when
/// `from` is the target; an error when no chain of them reaches it.
pub fn conversion(from: &str, sources: &[Source]) -> Result<Option<Conversion>> {
    if from == TARGET {
        return Ok(None);
    }
    let mut ids: Vec<&Source> = sources.iter().collect();
    ids.sort_by(|a, b| a.id.cmp(&b.id));
    // (The links between each pair of references, both ways, in id order.)
    let mut edges: BTreeMap<String, BTreeMap<String, Vec<(Link, i8, String)>>> = BTreeMap::new();
    for s in ids {
        let (a, b, l) = link_of(s)?;
        let k = format!("{} {}", s.id, s.keyed);
        edges.entry(a.clone()).or_default().entry(b.clone()).or_default().push((l.clone(), 1, k.clone()));
        edges.entry(b).or_default().entry(a).or_default().push((l, -1, k));
    }
    // (Breadth first, neighbours by name: the shortest chain, the same each time.)
    let mut prev: BTreeMap<String, String> = BTreeMap::new();
    let mut seen: BTreeSet<String> = [from.to_string()].into();
    let mut q = VecDeque::from([from.to_string()]);
    while let Some(n) = q.pop_front() {
        if n == TARGET {
            break;
        }
        for m in edges.get(&n).into_iter().flat_map(|e| e.keys()) {
            if seen.insert(m.clone()) {
                prev.insert(m.clone(), n.clone());
                q.push_back(m.clone());
            }
        }
    }
    if !seen.contains(TARGET) {
        bail!(
            "no declared geoid source (inputs/sources/, role geoid) links {from} to {TARGET}: declare the grids \
             that do (docs/inputs.md §6.8)"
        );
    }
    let mut path = vec![TARGET.to_string()];
    while let Some(p) = prev.get(path.last().unwrap()) {
        path.push(p.clone());
    }
    path.reverse();
    let mut hops = Vec::new();
    let mut named = Vec::new();
    for w in path.windows(2) {
        let by = &edges[&w[0]][&w[1]];
        named.extend(by.iter().map(|(_, _, k)| k.clone()));
        hops.push(Hop {
            from: w[0].clone(),
            to: w[1].clone(),
            by: by.iter().map(|(l, s, _)| (l.clone(), *s)).collect(),
        });
    }
    named.sort();
    named.dedup();
    let digest = store::naming::hash16(format!("{from}\n{}", named.join("\n")).as_bytes());
    Ok(Some(Conversion { hops, digest }))
}

// ---- Applying a conversion ----------------------------------------------------------------------

/// GRS 80's semi-major axis and flattening (the frames' ellipsoid: WGS 84's differs by 0.1 mm).
const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_222_101;

/// The change in ellipsoidal height at `lon`, `lat` (degrees, on the ellipsoid) of a Helmert:
/// translation `t` (m), rotation `r` (mas), scale `s` (ppb), coordinate-frame or position-vector.
pub fn helmert_dh(lon: f64, lat: f64, t: [f64; 3], r: [f64; 3], s: f64, frame: bool) -> f64 {
    let e2 = F * (2.0 - F);
    let (sl, cl) = lat.to_radians().sin_cos();
    let (so, co) = lon.to_radians().sin_cos();
    let n = A / (1.0 - e2 * sl * sl).sqrt();
    let x = [n * cl * co, n * cl * so, n * (1.0 - e2) * sl];
    let mas = std::f64::consts::PI / (180.0 * 3600.0 * 1000.0);
    let (rx, ry, rz) =
        if frame { (r[0] * mas, r[1] * mas, r[2] * mas) } else { (-r[0] * mas, -r[1] * mas, -r[2] * mas) };
    let k = 1.0 + s * 1e-9;
    let y = [
        t[0] + k * (x[0] + rz * x[1] - ry * x[2]),
        t[1] + k * (-rz * x[0] + x[1] + rx * x[2]),
        t[2] + k * (ry * x[0] - rx * x[1] + x[2]),
    ];
    // (The new point's ellipsoidal height: iterated to convergence, a fixed number of times.)
    let p = y[0].hypot(y[1]);
    let mut phi = y[2].atan2(p * (1.0 - e2));
    let mut h = 0.0;
    for _ in 0..6 {
        let (sp, cp) = phi.sin_cos();
        let n = A / (1.0 - e2 * sp * sp).sqrt();
        h = p * cp + y[2] * sp - A * (1.0 - e2 * sp * sp).sqrt();
        phi = y[2].atan2(p * (1.0 - e2 * n / (n + h)));
    }
    h
}

/// The grids of a conversion as they're read, opened once a run.
pub struct Grids<'a> {
    o: &'a Opts<'a>,
    open: std::sync::Mutex<BTreeMap<String, Option<Arc<Tiff>>>>,
}

impl<'a> Grids<'a> {
    pub fn new(o: &'a Opts<'a>) -> Grids<'a> {
        Grids { o, open: std::sync::Mutex::new(BTreeMap::new()) }
    }

    fn tiff(&self, id: &str, url: &str, store: bool) -> Result<Option<Arc<Tiff>>> {
        if let Some(t) = self.open.lock().unwrap().get(url) {
            return Ok(t.clone());
        }
        let src = if store {
            let dir = self.o.store.as_ref().with_context(|| {
                format!("{id}: its grid is read from the store, and no store is given (SCENIC_STORE)")
            })?;
            stored(id, &dir.join(id), url)?
        } else {
            open_url(self.o.fetch, url)?
        };
        let t = match src {
            Some(s) => Some(Arc::new(Tiff::open(s).with_context(|| format!("{id}: {url}"))?)),
            None => bail!("{id}: its grid {url} isn't on the server"),
        };
        self.open.lock().unwrap().insert(url.to_string(), t.clone());
        Ok(t)
    }

    /// A grid's values at the points (NaN outside it or where a neighbour has none): bilinear
    /// between pixel centres in f64, the edges clamping within half a pixel; an integer grid scaled
    /// by its metadata (PROJ's packaging); longitudes tried a turn either way for a grid not in
    /// −180°…180°. Read a window of `WINDOW` pixels at a time, the points grouped by it.
    fn grid(&self, id: &str, url: &str, store: bool, lon: &[f64], lat: &[f64]) -> Result<Vec<f64>> {
        const WINDOW: i64 = 128;
        let Some(t) = self.tiff(id, url, store)? else { return Ok(vec![f64::NAN; lon.len()]) };
        let gt = t.transform(0)?;
        let img = t.level(0)?;
        let (w, h) = (img.width as i64, img.height as i64);
        let (scale, offset) = t.scale_offset();
        let nodata = t.nodata();
        let x1 = gt[0] + gt[1] * w as f64;
        // (Each point's pixel-centre coordinates, or None outside the grid.)
        let at: Vec<Option<(f64, f64)>> = lon
            .iter()
            .zip(lat)
            .map(|(&x, &y)| {
                let x = [x, x + 360.0, x - 360.0].into_iter().find(|v| *v >= gt[0].min(x1) && *v <= gt[0].max(x1))?;
                let (c, r) = ((x - gt[0]) / gt[1] - 0.5, (y - gt[3]) / gt[5] - 0.5);
                (c >= -0.5 && r >= -0.5 && c <= w as f64 - 0.5 && r <= h as f64 - 0.5).then_some((c, r))
            })
            .collect();
        let mut groups: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, a) in at.iter().enumerate() {
            if let Some((c, r)) = a {
                let key = ((c.floor() as i64).max(0) / WINDOW, (r.floor() as i64).max(0) / WINDOW);
                groups.entry(key).or_default().push(i);
            }
        }
        let mut out = vec![f64::NAN; lon.len()];
        for ((gx, gy), pts) in groups {
            let (c0, r0) = (gx * WINDOW, gy * WINDOW);
            let (ww, wh) = ((WINDOW + 1).min(w - c0), (WINDOW + 1).min(h - r0));
            let a =
                t.read_window(0, c0 as u32, r0 as u32, ww as u32, wh as u32).with_context(|| format!("{id}: {url}"))?;
            let px = |c: i64, r: i64| -> f64 {
                let (c, r) = (c.clamp(0, w - 1) - c0, r.clamp(0, h - 1) - r0);
                let v = a[(r.clamp(0, wh - 1) * ww + c.clamp(0, ww - 1)) as usize] as f64;
                if !v.is_finite() || nodata.is_some_and(|nd| v == nd) {
                    f64::NAN
                } else {
                    v * scale + offset
                }
            };
            for i in pts {
                let (c, r) = at[i].expect("a point in the grid");
                let (cf, rf) = (c.floor(), r.floor());
                let (fx, fy) = (c - cf, r - rf);
                let (ci, ri) = (cf as i64, rf as i64);
                out[i] = (px(ci, ri) * (1.0 - fx) + px(ci + 1, ri) * fx) * (1.0 - fy)
                    + (px(ci, ri + 1) * (1.0 - fx) + px(ci + 1, ri + 1) * fx) * fy;
            }
        }
        Ok(out)
    }

    /// The offsets (metres) taking heights at the points to `TARGET` by `c`: NaN where a hop has
    /// no source with a value.
    pub fn offsets(&self, c: &Conversion, lon: &[f64], lat: &[f64]) -> Result<Vec<f64>> {
        let n = lon.len();
        let mut off = vec![0.0f64; n];
        for hop in &c.hops {
            let mut got = vec![f64::NAN; n];
            for (link, sign) in &hop.by {
                let todo: Vec<usize> = (0..n).filter(|&i| got[i].is_nan() && off[i].is_finite()).collect();
                if todo.is_empty() {
                    break;
                }
                let (lo, la): (Vec<f64>, Vec<f64>) = todo.iter().map(|&i| (lon[i], lat[i])).unzip();
                let v: Vec<f64> = match link {
                    Link::Grid { id, url, store } => self.grid(id, url, *store, &lo, &la)?,
                    Link::Helmert { t, r, s, frame, .. } => {
                        lo.iter().zip(&la).map(|(&x, &y)| helmert_dh(x, y, *t, *r, *s, *frame)).collect()
                    }
                };
                for (&i, v) in todo.iter().zip(v) {
                    if v.is_finite() {
                        got[i] = *sign as f64 * v;
                    }
                }
            }
            for i in 0..n {
                off[i] += got[i];
            }
        }
        Ok(off)
    }
}

#[cfg(test)]
#[path = "geoid_tests.rs"]
mod tests;
