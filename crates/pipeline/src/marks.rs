//! Landmarks by view (docs/phase5.md): the point kinds, their records per z6 tile (`markdata`), the
//! marks tile format the app reads (RDMT), ids, the score and the filters' fields, and the keep
//! rule of the zoomed-out (thinned) tiles. Shared by the build (the `marks` job) and the server, which answers the In view statistics from markdata.

use det::Det;
use anyhow::{bail, ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// The point kinds, in the order markdata's `kinds` section lists them.
pub const KINDS: [&str; 8] = ["viewpoint", "peak", "waterfall", "lighthouse", "covered_bridge", "rest", "trailhead", "heritage"];

pub fn kind_index(k: &str) -> Option<usize> {
    KINDS.iter().position(|x| *x == k)
}

/// Heritage tiers (basemap.ts HERITAGE_TIERS): [`MarkPt::tier`] indexes these.
pub const TIERS: [&str; 16] = ["w.c", "w.n", "n.top", "n.second", "n.lower", "n.mon", "n.land", "n.hist", "n.fed", "p.des", "p.reg", "p.area", "m.des", "m.reg", "m.area", "m.agr"];

pub fn tier_index(t: &str) -> Option<u8> {
    TIERS.iter().position(|x| *x == t).map(|i| i as u8)
}

/// A point as markdata stores it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MarkPt {
    /// E7 degrees.
    pub lon: i32,
    pub lat: i32,
    /// Fame and isolation (`ia` 20000 when unknown, as the app reads it); `mz` NaN when none.
    pub fa: f32,
    pub ia: f32,
    pub mz: f32,
    /// The point's place in its kind's order: the tie-break everywhere.
    pub rank: u32,
    /// The lowest zoom whose thinned tile keeps the point ([`KZ_NONE`]: none).
    pub kz: u8,
    /// Heritage: level class (World Heritage, national top grade, the rest) + 3 × group; else 0.
    pub class: u8,
    /// Heritage: index into the tiers; else 0.
    pub tier: u8,
    pub flags: u8,
}

/// [`MarkPt::flags`].
pub mod flag {
    /// Has a name (the app's `props.name` is a non-empty string).
    pub const NAMED: u8 = 1;
    /// A part of a World Heritage Site shown as one dot (not counted, not a dot).
    pub const COMPONENT: u8 = 2;
    /// A picnic site (kind `rest`).
    pub const PICNIC: u8 = 4;
}

/// [`MarkPt::kz`] of a point only z6 blocks hold.
pub const KZ_NONE: u8 = 6;
/// Thinned tiles' deepest zoom.
pub const THIN_MAX_Z: u8 = 5;
/// Speck cells are at the tile's zoom plus this: 1,024 a side.
pub const CELL_DZ: u8 = 10;
/// Points a thinned tile keeps per balance, and per size filter.
pub const KEEP_TOP: usize = 256;
pub const KEEP_TOP_FILTER: usize = 64;
const KEEP_BALANCES: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// The app's `ia` for a point without one.
pub const IA_UNKNOWN: f32 = 20000.0;

/// E7 from degrees (`(v × 1e7).round()`), and back by a division, which gives back the JSON
/// double for up to 7 decimals (a multiply by 1e-7 can be an ulp off).
pub fn e7(v: f64) -> i32 {
    (v * 1e7).round() as i32
}

pub fn deg(v: i32) -> f64 {
    v as f64 / 1e7
}

/// The landmark score at a balance between fame (0) and isolation (1) (basemap.ts landmarkScoreOf),
/// with the browser's own log10 so the scores are the app's to the last bit.
pub fn score(fa: f64, ia: f64, balance: f64) -> f64 {
    (1.0 - balance) * (fa / 5.0).min(1.0) + balance * (((log10_js(ia.max(0.05)) + 1.3) / 5.6).clamp(0.0, 1.0))
}

fn words(x: f64) -> (i32, u32) {
    let b = x.to_bits();
    ((b >> 32) as u32 as i32, b as u32)
}

fn with_high(x: f64, hi: i32) -> f64 {
    f64::from_bits(((hi as u32 as u64) << 32) | (x.to_bits() & 0xffff_ffff))
}

/// `Math.log` as Node computes it on this Mac: V8's port of fdlibm's e_log.c (src/base/ieee754.cc)
/// as clang compiles it for arm64, where `a * b + c` within one expression is one fused
/// multiply-add (`f64::mul_add` here, at exactly those places). The platform's libm can differ in
/// the last bit, and the server's scores must be the app's exactly.
pub fn log_js(mut x: f64) -> f64 {
    const LN2_HI: f64 = 6.93147180369123816490e-01;
    const LN2_LO: f64 = 1.90821492927058770002e-10;
    const TWO54: f64 = 1.80143985094819840000e+16;
    const LG1: f64 = 6.666666666666735130e-01;
    const LG2: f64 = 3.999999999940941908e-01;
    const LG3: f64 = 2.857142874366239149e-01;
    const LG4: f64 = 2.222219843214978396e-01;
    const LG5: f64 = 1.818357216161805012e-01;
    const LG6: f64 = 1.531383769920937332e-01;
    const LG7: f64 = 1.479819860511658591e-01;
    let (mut hx, lx) = words(x);
    let mut k: i32 = 0;
    if hx < 0x0010_0000 {
        if ((hx & 0x7fff_ffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY;
        }
        if hx < 0 {
            return f64::NAN;
        }
        k -= 54;
        x *= TWO54;
        hx = words(x).0;
    }
    if hx >= 0x7ff0_0000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000f_ffff;
    let i = (hx + 0x95f64) & 0x10_0000;
    x = with_high(x, hx | (i ^ 0x3ff0_0000));
    k += i >> 20;
    let f = x - 1.0;
    if (0x000f_ffff & (2 + hx)) < 3 {
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            let dk = k as f64;
            // dk * ln2_hi + dk * ln2_lo
            return dk.mul_add(LN2_HI, dk * LN2_LO);
        }
        // f * f * (0.5 - 0.33333333333333333 * f)
        let r = f * f * (-0.33333333333333333f64).mul_add(f, 0.5);
        if k == 0 {
            return f - r;
        }
        let dk = k as f64;
        // dk * ln2_hi - ((R - dk * ln2_lo) - f)
        return dk.mul_add(LN2_HI, -((-dk).mul_add(LN2_LO, r) - f));
    }
    let s = f / (2.0 + f);
    let dk = k as f64;
    let z = s * s;
    let mut i = hx - 0x6147a;
    let w = z * z;
    let j = 0x6b851 - hx;
    // w * (Lg2 + w * (Lg4 + w * Lg6)); z * (Lg1 + w * (Lg3 + w * (Lg5 + w * Lg7)))
    let t1 = w * w.mul_add(w.mul_add(LG6, LG4), LG2);
    let t2 = z * w.mul_add(w.mul_add(w.mul_add(LG7, LG5), LG3), LG1);
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            // f - (hfsq - s * (hfsq + R))
            f - (-s).mul_add(hfsq + r, hfsq)
        } else {
            // dk * ln2_hi - ((hfsq - (s * (hfsq + R) + dk * ln2_lo)) - f)
            dk.mul_add(LN2_HI, -((hfsq - s.mul_add(hfsq + r, dk * LN2_LO)) - f))
        }
    } else if k == 0 {
        // f - s * (f - R)
        (-s).mul_add(f - r, f)
    } else {
        // dk * ln2_hi - ((s * (f - R) - dk * ln2_lo) - f)
        dk.mul_add(LN2_HI, -(s.mul_add(f - r, -(dk * LN2_LO)) - f))
    }
}

/// `Math.log10` as Node computes it on this Mac (fdlibm's e_log10.c in V8; see [`log_js`]).
pub fn log10_js(mut x: f64) -> f64 {
    const TWO54: f64 = 1.80143985094819840000e+16;
    const IVLN10: f64 = 4.34294481903251816668e-01;
    const LOG10_2HI: f64 = 3.01029995663611771306e-01;
    const LOG10_2LO: f64 = 3.69423907715893078616e-13;
    let (mut hx, mut lx) = words(x);
    let mut k: i32 = 0;
    if hx < 0x0010_0000 {
        if ((hx & 0x7fff_ffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY;
        }
        if hx < 0 {
            return f64::NAN;
        }
        k -= 54;
        x *= TWO54;
        (hx, lx) = words(x);
    }
    if hx >= 0x7ff0_0000 {
        return x + x;
    }
    if hx == 0x3ff0_0000 && lx == 0 {
        return 0.0;
    }
    k += (hx >> 20) - 1023;
    let i = ((k as u32 & 0x8000_0000) >> 31) as i32;
    hx = (hx & 0x000f_ffff) | ((0x3ff - i) << 20);
    let y = (k + i) as f64;
    x = f64::from_bits(((hx as u32 as u64) << 32) | lx as u64);
    // y * log10_2lo + ivln10 * log(x); z + y * log10_2hi
    let z = y.mul_add(LOG10_2LO, IVLN10 * log_js(x));
    y.mul_add(LOG10_2HI, z)
}

/// A heritage site's tier: its `t`, else from its designation level (data from before tiers;
/// basemap.ts heritageTierOf).
pub fn heritage_tier(t: Option<&str>, level: Option<f64>) -> &str {
    if let Some(t) = t {
        return t;
    }
    const BY_LEVEL: [&str; 5] = ["w.c", "n.top", "n.second", "p.des", "m.des"];
    // (`Number(level) || 5`: missing, zero and NaN all mean 5.)
    let l = level.filter(|v| *v != 0.0 && !v.is_nan()).unwrap_or(5.0);
    let i = l - 1.0;
    if i >= 0.0 && i.fract() == 0.0 && (i as usize) < BY_LEVEL.len() {
        BY_LEVEL[i as usize]
    } else {
        "m.des"
    }
}

// ---- the filters' definitions (stopfilters.ts STOP_FILTERS) ---------------------------------------

/// A filter's scale: even in the value, its logarithm, or the logarithm of its age.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axis {
    Lin,
    Log,
    Age,
}

/// The year ages count from.
const AGE_REF: f64 = 2030.0;

pub fn axis_pos(axis: Axis, v: f64) -> f64 {
    match axis {
        Axis::Log => log10_js(v.max(1e-9)),
        Axis::Age => -log10_js((AGE_REF - v).max(1.0)),
        Axis::Lin => v,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FilterKind {
    /// A number in a range; `year`: a year (not a size, for the thinned tiles' keep rule).
    Range { domain: (f64, f64), axis: Axis, year: bool },
    Flag { bit: Option<u32> },
}

/// One of the stops' filters.
#[derive(Clone, Copy, Debug)]
pub struct FilterDef {
    pub key: &'static str,
    /// The overlay (OverlayKey) it belongs to.
    pub ov: &'static str,
    pub prop: &'static str,
    pub kind: FilterKind,
}

const fn range(key: &'static str, ov: &'static str, prop: &'static str, lo: f64, hi: f64, axis: Axis) -> FilterDef {
    FilterDef { key, ov, prop, kind: FilterKind::Range { domain: (lo, hi), axis, year: false } }
}

const fn years(key: &'static str, ov: &'static str, prop: &'static str, lo: f64, hi: f64, axis: Axis) -> FilterDef {
    FilterDef { key, ov, prop, kind: FilterKind::Range { domain: (lo, hi), axis, year: true } }
}

const fn flag(key: &'static str, ov: &'static str, prop: &'static str, bit: Option<u32>) -> FilterDef {
    FilterDef { key, ov, prop, kind: FilterKind::Flag { bit } }
}

pub const FILTERS: &[FilterDef] = &[
    range("peak.ele", "peak", "ele", 0.0, 6000.0, Axis::Lin),
    range("peak.pr", "peak", "pr", 1.0, 6000.0, Axis::Log),
    range("peak.is", "peak", "is", 0.01, 3000.0, Axis::Log),
    range("waterfall.h", "waterfall", "h", 0.5, 1000.0, Axis::Log),
    range("lighthouse.h", "lighthouse", "h", 2.0, 400.0, Axis::Log),
    range("lighthouse.fh", "lighthouse", "fh", 1.0, 400.0, Axis::Log),
    range("lighthouse.rg", "lighthouse", "rg", 0.0, 50.0, Axis::Lin),
    years("lighthouse.y", "lighthouse", "y", 1600.0, 2025.0, Axis::Lin),
    range("viewpoint.ele", "viewpoint", "ele", 0.0, 4000.0, Axis::Lin),
    flag("viewpoint.pan", "viewpoint", "pan", None),
    flag("viewpoint.tw", "viewpoint", "tw", None),
    range("covered_bridge.len", "covered_bridge", "len", 2.0, 500.0, Axis::Log),
    years("covered_bridge.y", "covered_bridge", "y", 1800.0, 2025.0, Axis::Lin),
    flag("rest.toilets", "rest", "fac", Some(1)),
    flag("rest.water", "rest", "fac", Some(2)),
    flag("rest.shelter", "rest", "fac", Some(4)),
    flag("rest.tables", "rest", "fac", Some(8)),
    flag("rest.bbq", "rest", "fac", Some(16)),
    flag("trailhead.toilets", "trailhead", "fac", Some(1)),
    flag("trailhead.water", "trailhead", "fac", Some(2)),
    years("heritage.by", "heritage", "by", -3000.0, 2020.0, Axis::Age),
    years("heritage.dy", "heritage", "dy", 1900.0, 2026.0, Axis::Lin),
    flag("heritage.wp", "heritage", "wp", None),
    range("heritageAreas.a", "heritageAreas", "a", 0.001, 1000.0, Axis::Log),
    range("special.a", "special", "a", 1.0, 30000.0, Axis::Log),
    range("indigenous.a", "indigenous", "a", 0.001, 100000.0, Axis::Log),
];

/// The properties a kind's filters read, in the filters' order: markdata's `fvals` columns.
pub fn fields(kind: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for d in FILTERS.iter().filter(|d| d.ov == kind) {
        if !v.contains(&d.prop) {
            v.push(d.prop);
        }
    }
    v
}

/// A JSON property as the filters read it: a number, else none (NaN).
pub fn num(props: &serde_json::Map<String, serde_json::Value>, prop: &str) -> f64 {
    props.get(prop).and_then(serde_json::Value::as_f64).unwrap_or(f64::NAN)
}

// ---- ids ------------------------------------------------------------------------------------------

/// Hashed ids start here; an OSM object's id (`id × 4 + type`) is below.
pub const HASHED: u64 = 1 << 51;

/// An OSM reference like "n123" (node), "w5", "r9" as an id.
pub fn osm_id(r: &str) -> Option<u64> {
    let (t, n) = r.split_at_checked(1)?;
    let ty = match t {
        "n" => 0,
        "w" => 1,
        "r" => 2,
        _ => return None,
    };
    let n: u64 = n.parse().ok()?;
    let id = n.checked_mul(4)? + ty;
    (id < HASHED).then_some(id)
}

/// A reference's hashed id: 2^51 plus the first 51 bits of its XXH3-64.
pub fn hashed_id(reference: &str) -> u64 {
    HASHED + (xxhash_rust::xxh3::xxh3_64(reference.as_bytes()) >> 13)
}

/// What an id is made from: an OSM id the point is (used only when no other point has it), its
/// reference otherwise, and its record's canonical JSON (orders the points sharing a reference).
pub struct IdSource {
    pub osm: Option<u64>,
    pub reference: String,
    pub canon: String,
}

/// Unique ids for a group of points (docs/phase5.md "Ids"), in the input order.
pub fn assign_ids(src: &[IdSource]) -> Result<Vec<u64>> {
    let mut osm_count: HashMap<u64, u32> = HashMap::new();
    for s in src {
        if let Some(o) = s.osm {
            *osm_count.entry(o).or_default() += 1;
        }
    }
    let mut ids = vec![0u64; src.len()];
    let mut taken: HashMap<u64, String> = HashMap::new();
    // References, with `#2`, `#3`, … for the second and later point sharing one (by canonical JSON).
    let mut by_ref: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, s) in src.iter().enumerate() {
        match s.osm {
            Some(o) if osm_count[&o] == 1 => {
                ensure!(o < HASHED, "OSM id {o} too large");
                ids[i] = o;
                taken.insert(o, String::new());
            }
            _ => by_ref.entry(&s.reference).or_default().push(i),
        }
    }
    let mut refs: BTreeMap<String, usize> = BTreeMap::new();
    for (r, mut v) in by_ref {
        v.sort_by(|&a, &b| src[a].canon.cmp(&src[b].canon).then(a.cmp(&b)));
        for (k, &i) in v.iter().enumerate() {
            let name = if k == 0 { r.to_string() } else { format!("{r}#{}", k + 1) };
            refs.insert(name, i);
        }
    }
    // Hash, in byte order of the references; a clash moves the later one to `#h1`, `#h2`, … until
    // none remain.
    for (r, i) in &refs {
        let mut k = 0;
        let mut name = r.clone();
        loop {
            let id = hashed_id(&name);
            if !taken.contains_key(&id) {
                taken.insert(id, name);
                ids[*i] = id;
                break;
            }
            k += 1;
            name = format!("{r}#h{k}");
        }
    }
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    ensure!(sorted.windows(2).all(|w| w[0] != w[1]), "ids aren't unique");
    Ok(ids)
}

// ---- tiles ----------------------------------------------------------------------------------------

/// Web Mercator in 0–1 (as the app's dot layout: clamped just inside).
pub fn merc(lon: f64, lat: f64) -> (f64, f64) {
    let s = (lat * std::f64::consts::PI / 180.0).dsin();
    let x = ((lon + 180.0) / 360.0).clamp(0.0, 1.0 - 1e-9);
    let y = (0.5 - ((1.0 + s) / (1.0 - s)).dln() / (4.0 * std::f64::consts::PI)).clamp(0.0, 1.0 - 1e-9);
    (x, y)
}

/// The tile at zoom `z` holding a point.
pub fn tile_at(lon: f64, lat: f64, z: u8) -> (u32, u32) {
    let (x, y) = merc(lon, lat);
    let n = (1u64 << z) as f64;
    ((x * n).floor() as u32, (y * n).floor() as u32)
}

/// The 2·b-bit Morton code of (x, y), b bits each: x in the even bits.
pub fn morton(x: u32, y: u32) -> u32 {
    let spread = |mut v: u32| {
        v &= 0xffff;
        v = (v | (v << 8)) & 0x00ff_00ff;
        v = (v | (v << 4)) & 0x0f0f_0f0f;
        v = (v | (v << 2)) & 0x3333_3333;
        (v | (v << 1)) & 0x5555_5555
    };
    spread(x) | (spread(y) << 1)
}

// ---- the marks tile format (RDMT) -----------------------------------------------------------------

pub const RDMT_MAGIC: &[u8; 4] = b"RDMT";
pub const RDMT_VERSION: u32 = 1;

/// A speck cell: points a thinned tile doesn't keep, counted per cell of 1,024² per tile (heritage
/// per cell and tier).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    /// Morton code of the cell within the tile.
    pub code: u32,
    pub tier: u8,
    pub count: u32,
}

/// What a marks tile holds: points (thinned tiles, z6 blocks, extras) and speck cells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkTile {
    pub ids: Vec<u64>,
    /// Per field (the kind's [`fields`]), a value per point (NaN: none).
    pub fvals: Vec<Vec<f64>>,
    pub pts: Vec<MarkPt>,
    pub cells: Vec<Cell>,
    /// Per point, its lean properties as one JSON object.
    pub props: Vec<Vec<u8>>,
}

fn pad8(b: &mut Vec<u8>) {
    while b.len() % 8 != 0 {
        b.push(0);
    }
}

impl MarkTile {
    pub fn encode(&self) -> Vec<u8> {
        let n = self.ids.len();
        assert!(self.pts.len() == n && self.props.len() == n && self.fvals.iter().all(|f| f.len() == n));
        let props_len: usize = self.props.iter().map(Vec::len).sum();
        let mut b = Vec::with_capacity(64 + n * (40 + 8 * self.fvals.len()) + props_len + self.cells.len() * 9);
        b.extend_from_slice(RDMT_MAGIC);
        for v in [RDMT_VERSION, n as u32, self.fvals.len() as u32, self.cells.len() as u32, props_len as u32] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&0u64.to_le_bytes());
        for id in &self.ids {
            b.extend_from_slice(&(*id as f64).to_le_bytes());
        }
        for f in &self.fvals {
            for v in f {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        let col = |b: &mut Vec<u8>, f: &dyn Fn(&MarkPt) -> [u8; 4]| {
            for p in &self.pts {
                b.extend_from_slice(&f(p));
            }
            pad8(b);
        };
        col(&mut b, &|p| p.lon.to_le_bytes());
        col(&mut b, &|p| p.lat.to_le_bytes());
        col(&mut b, &|p| p.fa.to_le_bytes());
        col(&mut b, &|p| p.ia.to_le_bytes());
        col(&mut b, &|p| p.mz.to_le_bytes());
        col(&mut b, &|p| p.rank.to_le_bytes());
        for f in [|p: &MarkPt| p.kz, |p: &MarkPt| p.class, |p: &MarkPt| p.tier, |p: &MarkPt| p.flags] {
            b.extend(self.pts.iter().map(f));
            pad8(&mut b);
        }
        for c in &self.cells {
            b.extend_from_slice(&c.code.to_le_bytes());
        }
        pad8(&mut b);
        for c in &self.cells {
            b.extend_from_slice(&c.count.to_le_bytes());
        }
        pad8(&mut b);
        b.extend(self.cells.iter().map(|c| c.tier));
        pad8(&mut b);
        let mut off = 0u32;
        b.extend_from_slice(&off.to_le_bytes());
        for p in &self.props {
            off += p.len() as u32;
            b.extend_from_slice(&off.to_le_bytes());
        }
        pad8(&mut b);
        for p in &self.props {
            b.extend_from_slice(p);
        }
        b
    }

    pub fn decode(b: &[u8]) -> Result<MarkTile> {
        ensure!(b.len() >= 32 && &b[..4] == RDMT_MAGIC, "not a marks tile");
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        ensure!(u(4) == RDMT_VERSION, "marks tile version {}", u(4));
        let (n, nf, nc, plen) = (u(8) as usize, u(12) as usize, u(16) as usize, u(20) as usize);
        let mut at = 32usize;
        let mut take = |len: usize| -> Result<&[u8]> {
            ensure!(at + len <= b.len(), "marks tile truncated");
            let s = &b[at..at + len];
            at = (at + len).div_ceil(8) * 8;
            Ok(s)
        };
        let f64s = |s: &[u8]| s.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect::<Vec<_>>();
        let u32s = |s: &[u8]| s.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect::<Vec<_>>();
        let ids: Vec<u64> = f64s(take(n * 8)?).into_iter().map(|v| v as u64).collect();
        let mut fvals = Vec::with_capacity(nf);
        for _ in 0..nf {
            fvals.push(f64s(take(n * 8)?));
        }
        let lon = u32s(take(n * 4)?);
        let lat = u32s(take(n * 4)?);
        let fa = u32s(take(n * 4)?);
        let ia = u32s(take(n * 4)?);
        let mz = u32s(take(n * 4)?);
        let rank = u32s(take(n * 4)?);
        let kz = take(n)?.to_vec();
        let class = take(n)?.to_vec();
        let tier = take(n)?.to_vec();
        let flags = take(n)?.to_vec();
        let codes = u32s(take(nc * 4)?);
        let counts = u32s(take(nc * 4)?);
        let tiers = take(nc)?.to_vec();
        let offs = u32s(take((n + 1) * 4)?);
        let pb = take(plen)?;
        ensure!(offs.last().copied() == Some(plen as u32), "marks tile props");
        let pts = (0..n)
            .map(|i| MarkPt {
                lon: lon[i] as i32,
                lat: lat[i] as i32,
                fa: f32::from_bits(fa[i]),
                ia: f32::from_bits(ia[i]),
                mz: f32::from_bits(mz[i]),
                rank: rank[i],
                kz: kz[i],
                class: class[i],
                tier: tier[i],
                flags: flags[i],
            })
            .collect();
        let cells = (0..nc).map(|i| Cell { code: codes[i], tier: tiers[i], count: counts[i] }).collect();
        let props = (0..n).map(|i| pb[offs[i] as usize..offs[i + 1] as usize].to_vec()).collect();
        Ok(MarkTile { ids, fvals, pts, cells, props })
    }
}

// ---- the keep rule --------------------------------------------------------------------------------

/// A point for the keep rule.
pub struct KeepPt<'a> {
    pub lon: f64,
    pub lat: f64,
    pub pt: &'a MarkPt,
    /// The kind's size filters' values (NaN: none), as [`size_fields`] lists them.
    pub sizes: Vec<f64>,
}

/// A kind's range filters other than years: the thinned tiles keep each one's largest.
pub fn size_fields(kind: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for d in FILTERS.iter().filter(|d| d.ov == kind) {
        if matches!(d.kind, FilterKind::Range { year: false, .. }) && !v.contains(&d.prop) {
            v.push(d.prop);
        }
    }
    v
}

/// Each point's [`MarkPt::kz`]: per zoom 0–5 and tile, the named points with `mz ≤ z − 3`, the top
/// [`KEEP_TOP`] by score at five balances, and the top [`KEEP_TOP_FILTER`] by each size filter
/// (ties by rank, so each rule is monotone in the zoom). World Heritage components: none.
pub fn keep_zooms(pts: &[KeepPt]) -> Vec<u8> {
    let mut kz = vec![KZ_NONE; pts.len()];
    let nsizes = pts.first().map_or(0, |p| p.sizes.len());
    for z in 0..=THIN_MAX_Z {
        let mut tiles: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, p) in pts.iter().enumerate() {
            if p.pt.flags & flag::COMPONENT == 0 {
                tiles.entry(tile_at(p.lon, p.lat, z)).or_default().push(i);
            }
        }
        for idx in tiles.values() {
            let mut keep = |i: usize| kz[i] = kz[i].min(z);
            for &i in idx {
                let p = pts[i].pt;
                if p.flags & flag::NAMED != 0 && !p.mz.is_nan() && p.mz as f64 <= z as f64 - 3.0 {
                    keep(i);
                }
            }
            let mut by = idx.clone();
            for b in KEEP_BALANCES {
                let s = |i: usize| score(pts[i].pt.fa as f64, pts[i].pt.ia as f64, b);
                by.sort_by(|&a, &c| s(c).total_cmp(&s(a)).then(pts[a].pt.rank.cmp(&pts[c].pt.rank)));
                by.iter().take(KEEP_TOP).for_each(|&i| keep(i));
            }
            for f in 0..nsizes {
                let mut has: Vec<usize> = idx.iter().copied().filter(|&i| !pts[i].sizes[f].is_nan()).collect();
                has.sort_by(|&a, &c| pts[c].sizes[f].total_cmp(&pts[a].sizes[f]).then(pts[a].pt.rank.cmp(&pts[c].pt.rank)));
                has.iter().take(KEEP_TOP_FILTER).for_each(|&i| keep(i));
            }
        }
    }
    kz
}

/// The speck cells of tile z/x/y: the points in it (`idx`) that it doesn't keep (kz > z), counted
/// per cell at zoom z + [`CELL_DZ`] (heritage: and tier). Sorted by code, then tier.
pub fn speck_cells(pts: &[KeepPt], idx: &[usize], z: u8, x: u32, y: u32) -> Vec<Cell> {
    let mut counts: BTreeMap<(u32, u8), u32> = BTreeMap::new();
    let n = (1u64 << (z + CELL_DZ)) as f64;
    let side = 1u32 << CELL_DZ;
    for &i in idx {
        let p = &pts[i];
        if p.pt.kz <= z || p.pt.flags & flag::COMPONENT != 0 {
            continue;
        }
        let (mx, my) = merc(p.lon, p.lat);
        let cx = ((mx * n).floor() as u32).saturating_sub(x * side).min(side - 1);
        let cy = ((my * n).floor() as u32).saturating_sub(y * side).min(side - 1);
        *counts.entry((morton(cx, cy), p.pt.tier)).or_default() += 1;
    }
    counts.into_iter().map(|((code, tier), count)| Cell { code, tier, count }).collect()
}

// ---- markdata ------------------------------------------------------------------------------------

/// Points per zstd block of `props` and `info`.
pub const BLOCK: usize = 256;

/// One point for markdata.
pub struct Row {
    pub id: u64,
    pub pt: MarkPt,
    /// The kind's [`fields`] values.
    pub fvals: Vec<f64>,
    /// Lean properties: one JSON object.
    pub props: Vec<u8>,
    /// The popup record: one JSON object.
    pub info: Vec<u8>,
}

/// A named peak with a height, as the summits list has it, and its place in that list.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct SummitRec {
    pub rank: u32,
    pub lon: i32,
    pub lat: i32,
    pub pad: u32,
    pub ele: f64,
}

/// JSON objects back to back with their offsets (u32 count, u32 × (count + 1) offsets, bytes):
/// one parses alone.
pub fn objects(items: &[&[u8]]) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + 4 * (items.len() + 1) + items.iter().map(|i| i.len()).sum::<usize>());
    b.extend_from_slice(&(items.len() as u32).to_le_bytes());
    let mut off = 0u32;
    b.extend_from_slice(&off.to_le_bytes());
    for i in items {
        off += i.len() as u32;
        b.extend_from_slice(&off.to_le_bytes());
    }
    for i in items {
        b.extend_from_slice(i);
    }
    b
}

/// The `k`-th object of an [`objects`] block.
pub fn object(block: &[u8], k: usize) -> Result<&[u8]> {
    let u = |o: usize| -> Result<usize> { Ok(u32::from_le_bytes(block.get(o..o + 4).context("objects block")?.try_into().unwrap()) as usize) };
    let n = u(0)?;
    ensure!(k < n, "object {k} of {n}");
    let base = 4 + 4 * (n + 1);
    let (a, b) = (u(4 + 4 * k)?, u(8 + 4 * k)?);
    block.get(base + a..base + b).context("objects block")
}

/// Writes a z6 tile's markdata: `rows[k]` the points of kind `KINDS[k]` sorted by id.
pub fn write_markdata(path: &Path, tile: (u32, u32), rows: &[Vec<Row>], summits: &[(SummitRec, String)]) -> Result<()> {
    ensure!(rows.len() == KINDS.len(), "rows per kind");
    let mut kinds: Vec<[u32; 2]> = Vec::new();
    let (mut ids, mut pts, mut fvals) = (Vec::new(), Vec::new(), Vec::<f64>::new());
    let mut first = 0u32;
    let mut all: Vec<&Row> = Vec::new();
    for (k, rs) in rows.iter().enumerate() {
        ensure!(rs.windows(2).all(|w| w[0].id < w[1].id), "{}: ids not sorted and unique", KINDS[k]);
        let nf = fields(KINDS[k]).len();
        kinds.push([first, rs.len() as u32]);
        first += rs.len() as u32;
        for r in rs {
            ensure!(r.fvals.len() == nf, "{}: {} values, not {nf}", KINDS[k], r.fvals.len());
            ids.push(r.id);
            pts.push(r.pt);
            all.push(r);
        }
        for f in 0..nf {
            fvals.extend(rs.iter().map(|r| r.fvals[f]));
        }
    }
    let blocks = |get: &dyn Fn(&Row) -> &[u8]| -> Result<(Vec<u8>, Vec<[u64; 2]>)> {
        let (mut data, mut idx) = (Vec::new(), Vec::new());
        for chunk in all.chunks(BLOCK) {
            let items: Vec<&[u8]> = chunk.iter().map(|r| get(r)).collect();
            let z = zstd::bulk::compress(&objects(&items), 9)?;
            idx.push([data.len() as u64, z.len() as u64]);
            data.extend_from_slice(&z);
        }
        Ok((data, idx))
    };
    let (props, props_idx) = blocks(&|r| &r.props)?;
    let (info, info_idx) = blocks(&|r| &r.info)?;
    let srecs: Vec<SummitRec> = summits.iter().map(|s| s.0).collect();
    let snames: Vec<&[u8]> = summits.iter().map(|s| s.1.as_bytes()).collect();
    let meta = serde_json::json!({
        "fmt": 1, "tile": format!("6/{}/{}", tile.0, tile.1), "block": BLOCK,
        "fields": KINDS.iter().map(|k| (k.to_string(), fields(k))).collect::<BTreeMap<_, _>>(),
    });
    let mut w = store::sect::SectWriter::create(path, meta)?;
    w.add_pod("kinds", &kinds)?;
    w.add_pod("ids", &ids)?;
    w.add_pod("pts", &pts)?;
    w.add_pod("fvals", &fvals)?;
    w.add("props", &props)?;
    w.add_pod("props_idx", &props_idx)?;
    w.add("info", &info)?;
    w.add_pod("info_idx", &info_idx)?;
    w.add_pod("summits", &srecs)?;
    w.add("summit_names", &objects(&snames))?;
    w.finish()?;
    Ok(())
}

/// Checks markdata's sections against what [`write_markdata`] writes (the reader's assumptions).
pub fn check_markdata(sections: &dyn Fn(&str) -> Option<u64>) -> Result<()> {
    for (s, size) in [("kinds", 8u64), ("ids", 8), ("pts", 28), ("fvals", 8), ("props_idx", 16), ("info_idx", 16), ("summits", 24)] {
        let Some(len) = sections(s) else { bail!("markdata without {s}") };
        ensure!(len % size == 0, "markdata {s}: {len} bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_pt_is_28_bytes() {
        assert_eq!(std::mem::size_of::<MarkPt>(), 28);
        assert_eq!(std::mem::size_of::<SummitRec>(), 24);
    }

    #[test]
    fn log10_as_node_on_this_mac() {
        // Math.log10 in Node (V8, arm64) where the platform's log10 differs in the last bit.
        for (x, y) in [(0x4027000000000000, 0x3ff0f89e4c741b00), (0x4057000000000000, 0x3fff6bacc8ebb67f), (0x4012ccccc0000000, 0x3fe581d3547b2aa4), (0x3ff4ccccc0000000, 0x3fbd2b63f7561d47), (0x40d3880000000000, 0x401134413509f7a0), (0x3fa999999999999a, 0xbff4d104d427de80)] {
            assert_eq!(log10_js(f64::from_bits(x)).to_bits(), y, "log10({})", f64::from_bits(x));
        }
    }

    #[test]
    fn e7_round_trips_the_json_double() {
        for v in [-2.129421, 53.763206, -140.40569, 179.9999999, -0.127891, 51.535698] {
            assert_eq!(deg(e7(v)), v, "{v}");
        }
    }

    #[test]
    fn osm_and_hashed_ids() {
        assert_eq!(osm_id("n251191"), Some(251191 * 4));
        assert_eq!(osm_id("w5"), Some(21));
        assert_eq!(osm_id("r9"), Some(38));
        assert_eq!(osm_id("x1"), None);
        assert_eq!(osm_id("n"), None);
        let h = hashed_id("legacy:poi|peak|1,2|");
        assert!((HASHED..HASHED * 2).contains(&h));
    }

    #[test]
    fn ids_unique_with_duplicates() {
        let s = |osm: Option<u64>, r: &str, c: &str| IdSource { osm, reference: r.into(), canon: c.into() };
        let src = vec![
            s(Some(40), "a", "1"),
            s(Some(44), "b", "2"),
            s(Some(44), "c", "3"), // a repeated OSM id: neither keeps it
            s(None, "d", "z"),
            s(None, "d", "y"), // same reference: "d" for the first by canonical JSON, "d#2" next
            s(None, "d", "y"), // an exact duplicate: "d#3"
        ];
        let ids = assign_ids(&src).unwrap();
        assert_eq!(ids[0], 40);
        assert_eq!(ids[1], hashed_id("b"));
        assert_eq!(ids[2], hashed_id("c"));
        assert_eq!(ids[4], hashed_id("d"));
        assert_eq!(ids[5], hashed_id("d#2"));
        assert_eq!(ids[3], hashed_id("d#3"));
        // Deterministic.
        assert_eq!(assign_ids(&src).unwrap(), ids);
    }

    #[test]
    fn tile_round_trip() {
        let pt = |lon: f64, rank: u32| MarkPt { lon: e7(lon), lat: e7(51.5), fa: 1.5, ia: 0.25, mz: f32::NAN, rank, kz: 3, class: 0, tier: 0, flags: flag::NAMED };
        let t = MarkTile {
            ids: vec![4, HASHED + 77],
            fvals: vec![vec![1.0, f64::NAN], vec![0.45, 2.0]],
            pts: vec![pt(-0.1, 0), pt(0.2, 1)],
            cells: vec![Cell { code: 5, tier: 0, count: 3 }, Cell { code: 9, tier: 1, count: 1 }],
            props: vec![br#"{"name":"A"}"#.to_vec(), br#"{"name":"B","ele":3}"#.to_vec()],
        };
        let b = t.encode();
        let d = MarkTile::decode(&b).unwrap();
        assert_eq!(d.ids, t.ids);
        assert_eq!(d.props, t.props);
        assert_eq!(d.cells, t.cells);
        assert_eq!(d.fvals[0][0], 1.0);
        assert!(d.fvals[0][1].is_nan());
        assert_eq!(d.fvals[1], vec![0.45, 2.0]);
        assert_eq!(d.pts[1].lon, e7(0.2));
        assert!(d.pts[0].mz.is_nan());
        assert_eq!(d.pts[1].rank, 1);
        // The ids column at 32, so it's a Float64Array in place.
        assert_eq!(f64::from_le_bytes(b[40..48].try_into().unwrap()), (HASHED + 77) as f64);
    }

    #[test]
    fn objects_parse_alone() {
        let b = objects(&[b"{}", br#"{"a":1}"#, b""]);
        assert_eq!(object(&b, 1).unwrap(), br#"{"a":1}"#);
        assert_eq!(object(&b, 2).unwrap(), b"");
        assert!(object(&b, 3).is_err());
    }

    #[test]
    fn keep_rule_is_monotone_and_counts_the_rest() {
        // 600 points along a line in one z0 tile: the top 256 by fame (and by isolation) are kept at
        // every zoom; named ones with mz ≤ z − 3 too.
        let mut pts: Vec<MarkPt> = Vec::new();
        for i in 0..600u32 {
            let named = i % 50 == 0;
            pts.push(MarkPt { lon: e7(-10.0 + i as f64 * 0.01), lat: e7(45.0), fa: (i % 97) as f32 / 20.0, ia: 0.06 + (i % 89) as f32, mz: if named { 2.5 } else { f32::NAN }, rank: i, kz: 0, class: 0, tier: 0, flags: if named { flag::NAMED } else { 0 } });
        }
        let kp: Vec<KeepPt> = pts.iter().map(|p| KeepPt { lon: deg(p.lon), lat: deg(p.lat), pt: p, sizes: vec![] }).collect();
        let kz = keep_zooms(&kp);
        // At zoom 0 one tile holds all 600: at most 5 × 256 kept by score, plus names (mz 2.5 ≤ z − 3
        // from zoom 6 only: none).
        let at0 = kz.iter().filter(|&&k| k == 0).count();
        assert!(at0 > 256 && at0 <= 5 * 256, "{at0}");
        // Monotone: a point kept at z is kept deeper (its tile there holds a subset).
        for z in 0..=THIN_MAX_Z {
            let kept = kz.iter().filter(|&&k| k <= z).count();
            assert!(kept >= at0);
        }
        // Cells count exactly the points not kept.
        let mut pts2 = pts.clone();
        for (p, k) in pts2.iter_mut().zip(&kz) {
            p.kz = *k;
        }
        let kp2: Vec<KeepPt> = pts2.iter().map(|p| KeepPt { lon: deg(p.lon), lat: deg(p.lat), pt: p, sizes: vec![] }).collect();
        let idx: Vec<usize> = (0..kp2.len()).collect();
        let cells = speck_cells(&kp2, &idx, 0, 0, 0);
        assert_eq!(cells.iter().map(|c| c.count as usize).sum::<usize>(), 600 - at0);
    }
}
