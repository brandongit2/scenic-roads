//! Prototype of the far buildings' field (task #102): a z6 tile's buildings pack (z14 tiles, every
//! building) reduced to per-texel aggregates, levels 14..4, 512 x 512 texels a tile, for each of five
//! height classes (by a building's or part's top: under 8 m, 8-20, 20-40, 40-100, 100 m and up):
//!   f  roof coverage: the share of the texel under the class's roofs (exact area, as font renderers
//!      rasterize; overlapping footprints summed, then clamped to 1)
//!   w  wall area per ground area: sum over walls of length x (top - base) / texel area
//!   v  sum of roof share x top (m), so v / f is the class's mean roof height
//! All are densities, so a coarser level is the mean of its 4 children (exact).
//! Level 14 (drawn at map zoom 13-14 over the z13 mesh tiles) holds only the buildings those leave
//! out (under 20 m and under 2,000 m2), level 13 (over the z12 tiles) those under 40 m, level 12
//! and coarser every building.
//!
//! Output: <out>/<z>/<x>/<y>.bin: gzip of 15 planes of 512 x 512 u16 LE (f of each class x 65535,
//! log2(1 + w) / 12 x 65535 of each, v x 64 of each), all-zero tiles left out; <out>/stats.json.

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::Mutex;

const T: usize = 512; // texels a tile side
const B: usize = 4096; // texels a z11 block side, at level 14's texel size (8 x 512)
const STRIDE: usize = B + 2;
const QF: f32 = 65535.0;
const QV: f32 = 64.0;
/// w is stored as log2(1 + w) / WLOG x 65535 (a tower's edge texel holds walls 100 times its area).
const WLOG: f32 = 12.0;

fn tile_key(z: u64, x: u64, y: u64) -> u64 {
    (z << 58) | (x << 29) | y
}

struct Pack {
    data: Vec<u8>,
    index: BTreeMap<u64, (usize, usize)>,
    root: (u32, u32, u32),
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn read_pack(path: &str) -> Pack {
    let data = std::fs::read(path).expect("read pack");
    assert_eq!(&data[0..8], b"RDPACK01");
    let idx_off = u64le(&data, 16) as usize;
    let n = u64le(&data, 24) as usize;
    let ml = u32le(&data, 32) as usize;
    let meta = std::str::from_utf8(&data[36..36 + ml]).unwrap().to_string();
    let r = meta.split("\"root\":\"").nth(1).unwrap().split('"').next().unwrap();
    let p: Vec<u32> = r.split('/').map(|s| s.parse().unwrap()).collect();
    let mut index = BTreeMap::new();
    for i in 0..n {
        let o = idx_off + i * 32;
        index.insert(u64le(&data, o), (u64le(&data, o + 8) as usize, u32le(&data, o + 16) as usize));
    }
    Pack { data, index, root: (p[0], p[1], p[2]) }
}

// ---- a minimal MVT reader ----------------------------------------------------------------------
struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Rd<'a> {
    fn varint(&mut self) -> u64 {
        let mut v = 0u64;
        let mut s = 0;
        loop {
            let c = self.b[self.p];
            self.p += 1;
            v |= ((c & 0x7f) as u64) << s;
            if c < 0x80 {
                return v;
            }
            s += 7;
        }
    }
    fn done(&self) -> bool {
        self.p >= self.b.len()
    }
    /// (field number, wire type, varint value or bytes)
    fn field(&mut self) -> (u64, u64, u64, &'a [u8]) {
        let k = self.varint();
        let (num, wt) = (k >> 3, k & 7);
        match wt {
            0 => (num, wt, self.varint(), &[]),
            2 => {
                let l = self.varint() as usize;
                let s = &self.b[self.p..self.p + l];
                self.p += l;
                (num, wt, 0, s)
            }
            1 => {
                self.p += 8;
                (num, wt, 0, &[])
            }
            5 => {
                self.p += 4;
                (num, wt, 0, &[])
            }
            _ => panic!("wire type {wt}"),
        }
    }
}
fn packed(b: &[u8]) -> Vec<u32> {
    let mut r = Rd { b, p: 0 };
    let mut v = Vec::new();
    while !r.done() {
        v.push(r.varint() as u32);
    }
    v
}

struct Feat {
    h: u32,
    m: u32,
    k: u32,
    o: bool,
    rings: Vec<Vec<[i32; 2]>>,
}

fn decode_tile(gz: &[u8]) -> Vec<Feat> {
    let mut raw = Vec::new();
    GzDecoder::new(gz).read_to_end(&mut raw).expect("gunzip");
    let mut out = Vec::new();
    let mut r = Rd { b: &raw, p: 0 };
    while !r.done() {
        let (num, _, _, bytes) = r.field();
        if num != 3 {
            continue;
        }
        let mut l = Rd { b: bytes, p: 0 };
        let mut keys: Vec<String> = Vec::new();
        let mut vals: Vec<u64> = Vec::new();
        let mut feats: Vec<(Vec<u32>, Vec<u32>)> = Vec::new();
        while !l.done() {
            let (n, _, _, b) = l.field();
            match n {
                2 => {
                    let mut f = Rd { b, p: 0 };
                    let (mut tags, mut geom) = (Vec::new(), Vec::new());
                    while !f.done() {
                        let (fn_, _, _, fb) = f.field();
                        match fn_ {
                            2 => tags = packed(fb),
                            4 => geom = packed(fb),
                            _ => {}
                        }
                    }
                    feats.push((tags, geom));
                }
                3 => keys.push(String::from_utf8(b.to_vec()).unwrap()),
                4 => {
                    let mut v = Rd { b, p: 0 };
                    let mut val = 0u64;
                    while !v.done() {
                        let (vn, wt, x, _) = v.field();
                        if wt == 0 && (vn == 4 || vn == 5) {
                            val = x;
                        }
                    }
                    vals.push(val);
                }
                _ => {}
            }
        }
        for (tags, geom) in feats {
            let mut f = Feat { h: 0, m: 0, k: 0, o: false, rings: Vec::new() };
            for t in tags.chunks(2) {
                let (key, v) = (keys[t[0] as usize].as_str(), vals[t[1] as usize] as u32);
                match key {
                    "h" => f.h = v,
                    "m" => f.m = v,
                    "k" => f.k = v,
                    "o" => f.o = v != 0,
                    _ => {}
                }
            }
            let (mut x, mut y) = (0i32, 0i32);
            let mut i = 0;
            let mut ring: Vec<[i32; 2]> = Vec::new();
            while i < geom.len() {
                let c = geom[i];
                i += 1;
                let (id, cnt) = (c & 7, (c >> 3) as usize);
                match id {
                    1 | 2 => {
                        for _ in 0..cnt {
                            let dx = ((geom[i] >> 1) as i32) ^ -((geom[i] & 1) as i32);
                            let dy = ((geom[i + 1] >> 1) as i32) ^ -((geom[i + 1] & 1) as i32);
                            i += 2;
                            x += dx;
                            y += dy;
                            if id == 1 && !ring.is_empty() {
                                f.rings.push(std::mem::take(&mut ring));
                            }
                            ring.push([x, y]);
                        }
                    }
                    7 => f.rings.push(std::mem::take(&mut ring)),
                    _ => panic!("command {id}"),
                }
            }
            if !ring.is_empty() {
                f.rings.push(ring);
            }
            out.push(f);
        }
    }
    out
}

// ---- exact-area rasterization (accumulation, as font-rs) ----------------------------------------
fn draw_line(acc: &mut [f32], p0: (f64, f64), p1: (f64, f64), wgt: f64) {
    if p0.1 == p1.1 {
        return;
    }
    let (dir, p0, p1) = if p0.1 < p1.1 { (1.0, p0, p1) } else { (-1.0, p1, p0) };
    if p1.1 <= 0.0 || p0.1 >= B as f64 {
        return;
    }
    let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
    let mut x = p0.0;
    if p0.1 < 0.0 {
        x -= p0.1 * dxdy;
    }
    let y0 = p0.1.max(0.0) as usize;
    let y1 = (p1.1.ceil() as usize).min(B);
    for y in y0..y1 {
        let ls = y * STRIDE;
        let dy = ((y + 1) as f64).min(p1.1) - (y as f64).max(p0.1);
        let xnext = x + dxdy * dy;
        let d = dy * dir * wgt;
        let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
        let x0f = x0.floor();
        let x0i = x0f as usize;
        let x1c = x1.ceil();
        let x1i = x1c as usize;
        if x1i <= x0i + 1 {
            let xmf = 0.5 * (x + xnext) - x0f;
            acc[ls + x0i] += (d - d * xmf) as f32;
            acc[ls + x0i + 1] += (d * xmf) as f32;
        } else {
            let s = 1.0 / (x1 - x0);
            let x0r = x0 - x0f;
            let a0 = 0.5 * s * (1.0 - x0r) * (1.0 - x0r);
            let x1r = x1 - x1c + 1.0;
            let am = 0.5 * s * x1r * x1r;
            acc[ls + x0i] += (d * a0) as f32;
            if x1i == x0i + 2 {
                acc[ls + x0i + 1] += (d * (1.0 - a0 - am)) as f32;
            } else {
                let a1 = s * (1.5 - x0r);
                acc[ls + x0i + 1] += (d * (a1 - a0)) as f32;
                for xi in x0i + 2..x1i - 1 {
                    acc[ls + xi] += (d * s) as f32;
                }
                let a2 = a1 + (x1i - x0i - 3) as f64 * s;
                acc[ls + x1i - 1] += (d * (1.0 - a2 - am)) as f32;
            }
            acc[ls + x1i] += (d * am) as f32;
        }
        x = xnext;
    }
}

/// An edge clipped to x in [0, B]: the parts left of 0 drawn on x = 0 (they cover every texel to
/// their right), those right of B on x = B (they cover none).
fn edge(acc: &mut [f32], a: (f64, f64), b: (f64, f64), wgt: f64) {
    let w = B as f64;
    let mut pts = vec![a];
    let mut ts: Vec<f64> = Vec::new();
    for xc in [0.0, w] {
        if (a.0 - xc) * (b.0 - xc) < 0.0 {
            ts.push((xc - a.0) / (b.0 - a.0));
        }
    }
    ts.sort_by(|p, q| p.partial_cmp(q).unwrap());
    for t in ts {
        pts.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
    }
    pts.push(b);
    for p in pts.windows(2) {
        let c = |q: (f64, f64)| (q.0.clamp(0.0, w), q.1);
        draw_line(acc, c(p[0]), c(p[1]), wgt);
    }
}

/// Deposits a wall's length x height into the texels it crosses (exact, piece by piece).
fn wall(wd: &mut [f32], a: (f64, f64), b: (f64, f64), len_m: f64, wh: f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let mut ts = vec![0.0, 1.0];
    let (xa, xb) = (a.0.min(b.0), a.0.max(b.0));
    let (ya, yb) = (a.1.min(b.1), a.1.max(b.1));
    if dx != 0.0 {
        let mut g = xa.floor() + 1.0;
        while g < xb {
            ts.push((g - a.0) / dx);
            g += 1.0;
        }
    }
    if dy != 0.0 {
        let mut g = ya.floor() + 1.0;
        while g < yb {
            ts.push((g - a.1) / dy);
            g += 1.0;
        }
    }
    ts.sort_by(|p, q| p.partial_cmp(q).unwrap());
    for t in ts.windows(2) {
        let (t0, t1) = (t[0], t[1]);
        if t1 <= t0 {
            continue;
        }
        let tm = 0.5 * (t0 + t1);
        let (mx, my) = (a.0 + dx * tm, a.1 + dy * tm);
        if mx < 0.0 || my < 0.0 || mx >= B as f64 || my >= B as f64 {
            continue;
        }
        wd[my as usize * B + mx as usize] += ((t1 - t0) * len_m * wh) as f32;
    }
}

/// Metres a texel side at global texel row `y` of a grid of `n` rows (Web Mercator).
fn side_m(y: f64, n: f64) -> f64 {
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan();
    40075016.686 * lat.cos() / n
}

/// Height classes (by a building's or part's top, dm): under 8 m, 8–20, 20–40, 40–100, 100 m and up.
const K: usize = 5;
const CLASS_TOP: [u32; 4] = [80, 200, 400, 1000];
/// Planes: f of each class, then w, then v.
const P: usize = 3 * K;

/// A level's arrays: P planes, `n` x `n`.
struct Lv {
    n: usize,
    p: Vec<Vec<f32>>,
}
impl Lv {
    fn zero(n: usize) -> Lv {
        Lv { n, p: (0..P).map(|_| vec![0.0; n * n]).collect() }
    }
    fn half(&self) -> Lv {
        let n = self.n / 2;
        let mut o = Lv::zero(n);
        for (a, b) in self.p.iter().zip(o.p.iter_mut()) {
            for y in 0..n {
                for x in 0..n {
                    let i = |dx: usize, dy: usize| (2 * y + dy) * self.n + 2 * x + dx;
                    b[y * n + x] = 0.25 * (a[i(0, 0)] + a[i(1, 0)] + a[i(0, 1)] + a[i(1, 1)]);
                }
            }
        }
        o
    }
    /// The 512 x 512 tile at (tx, ty) of this level's array, encoded; None if empty.
    fn tile(&self, tx: usize, ty: usize) -> Option<Vec<u8>> {
        let mut raw = vec![0u8; T * T * 2 * P];
        let mut any = false;
        for (c, arr) in self.p.iter().enumerate() {
            let kind = c / K; // 0 f, 1 w, 2 v
            for y in 0..T {
                for x in 0..T {
                    let val = arr[(ty * T + y) * self.n + tx * T + x];
                    let u = match kind {
                        0 => (val * QF).round().clamp(0.0, 65535.0) as u16,
                        1 => ((1.0 + val.max(0.0)).log2() / WLOG * 65535.0).round().clamp(0.0, 65535.0) as u16,
                        _ => (val * QV).round().clamp(0.0, 65535.0) as u16,
                    };
                    if u != 0 {
                        any = true;
                    }
                    let o = (c * T * T + y * T + x) * 2;
                    raw[o..o + 2].copy_from_slice(&u.to_le_bytes());
                }
            }
        }
        if !any {
            return None;
        }
        let mut e = GzEncoder::new(Vec::new(), Compression::new(6));
        e.write_all(&raw).unwrap();
        Some(e.finish().unwrap())
    }
}

struct Out {
    dir: String,
    stats: Mutex<BTreeMap<u32, (u64, u64, u64)>>, // z -> (tiles, bytes, max)
}
impl Out {
    fn put(&self, z: u32, x: u32, y: u32, bytes: Option<Vec<u8>>) {
        let Some(b) = bytes else { return };
        let d = format!("{}/{}/{}", self.dir, z, x);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(format!("{d}/{y}.bin"), &b).unwrap();
        let mut s = self.stats.lock().unwrap();
        let e = s.entry(z).or_default();
        e.0 += 1;
        e.1 += b.len() as u64;
        e.2 = e.2.max(b.len() as u64);
    }
}

/// Accumulation groups: the classes, and classes 0 and 1 again for footprints of 2,000 m2 or more
/// (which z13's meshes draw).
const NG: usize = 7;
/// Group -> class.
const GCLASS: [usize; NG] = [0, 1, 0, 1, 2, 3, 4];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pack = read_pack(&args[1]);
    let out = Out { dir: args[2].clone(), stats: Mutex::new(BTreeMap::new()) };
    let threads: usize = std::env::var("BLDFIELD_THREADS").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let (rz, rx, ry) = pack.root;
    assert_eq!(rz, 6);
    let t0 = std::time::Instant::now();
    // z11 blocks of the z6 tile.
    let blocks: Vec<(u32, u32)> = (0..32).flat_map(|j| (0..32).map(move |i| (rx * 32 + i, ry * 32 + j))).collect();
    let nb = Mutex::new(0u64);
    let l11: Vec<((u32, u32), Lv)> = pool.install(|| {
        blocks
            .par_iter()
            .filter_map(|&(bx, by)| {
                // z14 tiles of the block and one around it (buildings reach past their tile).
                let mut tiles = Vec::new();
                let mut own = false;
                for y14 in (by * 8).saturating_sub(1)..=by * 8 + 8 {
                    for x14 in (bx * 8).saturating_sub(1)..=bx * 8 + 8 {
                        if let Some(&(o, l)) = pack.index.get(&tile_key(14, x14 as u64, y14 as u64)) {
                            own |= x14 / 8 == bx && y14 / 8 == by;
                            tiles.push((x14, y14, o, l));
                        }
                    }
                }
                if !own {
                    return None;
                }
                let ny = (1u64 << 23) as f64;
                let mut accf: Vec<Vec<f32>> = (0..NG).map(|_| Vec::new()).collect();
                let mut accv: Vec<Vec<f32>> = (0..NG).map(|_| Vec::new()).collect();
                let mut wd: Vec<Vec<f32>> = (0..NG).map(|_| Vec::new()).collect();
                let mut nfeat = 0u64;
                for (x14, y14, o, l) in tiles {
                    for f in decode_tile(&pack.data[o..o + l]) {
                        if f.k == 2 || f.o || f.h == 0 {
                            continue;
                        }
                        // Texel coordinates in the block: z14 units are 2^-26 of the world, texels 2^-23.
                        let ox = (x14 as f64 * 4096.0 - bx as f64 * 32768.0) / 8.0;
                        let oy = (y14 as f64 * 4096.0 - by as f64 * 32768.0) / 8.0;
                        let rings: Vec<Vec<(f64, f64)>> = f
                            .rings
                            .iter()
                            .map(|r| r.iter().map(|p| (ox + p[0] as f64 / 8.0, oy + p[1] as f64 / 8.0)).collect())
                            .collect();
                        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                        for r in &rings {
                            for p in r {
                                x0 = x0.min(p.0);
                                y0 = y0.min(p.1);
                                x1 = x1.max(p.0);
                                y1 = y1.max(p.1);
                            }
                        }
                        if x1 < 0.0 || y1 < 0.0 || x0 >= B as f64 || y0 >= B as f64 {
                            continue;
                        }
                        nfeat += 1;
                        let cy = 0.5 * (y0 + y1);
                        let side = side_m(by as f64 * B as f64 + cy, ny);
                        let mut a2 = 0.0;
                        for r in &rings {
                            for i in 0..r.len() {
                                let (p, q) = (r[i], r[(i + 1) % r.len()]);
                                a2 += p.0 * q.1 - q.0 * p.1;
                            }
                        }
                        let area = 0.5 * a2.abs() * side * side;
                        let class = CLASS_TOP.iter().filter(|&&t| f.h >= t).count();
                        let g = if class <= 1 && area >= 2000.0 {
                            2 + class
                        } else if class <= 1 {
                            class
                        } else {
                            2 + class
                        };
                        if accf[g].is_empty() {
                            accf[g] = vec![0f32; STRIDE * B];
                            accv[g] = vec![0f32; STRIDE * B];
                            wd[g] = vec![0f32; B * B];
                        }
                        // Each roof at its top (a part's above the podium it stands on), its walls from
                        // its base.
                        let top = f.h as f64 / 10.0;
                        let wh = (f.h.saturating_sub(f.m)) as f64 / 10.0;
                        for r in &rings {
                            for i in 0..r.len() {
                                let (p, q) = (r[i], r[(i + 1) % r.len()]);
                                edge(&mut accf[g], p, q, 1.0);
                                edge(&mut accv[g], p, q, top);
                                let len = ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt() * side;
                                wall(&mut wd[g], p, q, len, wh);
                            }
                        }
                    }
                }
                *nb.lock().unwrap() += nfeat;
                // Prefix sums per group: coverage (clamped to 1), share x top, walls per area.
                let mut groups: Vec<Option<[Vec<f32>; 3]>> = (0..NG).map(|_| None).collect();
                for g in 0..NG {
                    if accf[g].is_empty() {
                        continue;
                    }
                    let (mut fa, mut wa, mut va) = (vec![0f32; B * B], vec![0f32; B * B], vec![0f32; B * B]);
                    for y in 0..B {
                        let a = side_m(by as f64 * B as f64 + y as f64 + 0.5, ny);
                        let ta = a * a;
                        let (mut cf, mut cv) = (0f64, 0f64);
                        for x in 0..B {
                            cf += accf[g][y * STRIDE + x] as f64;
                            cv += accv[g][y * STRIDE + x] as f64;
                            // (overlapping roofs: the share clamped to 1, their tops' sum scaled with it,
                            // so v / f stays their mean top)
                            let (fr, vr) = (cf.abs() as f32, cv.abs() as f32);
                            fa[y * B + x] = fr.min(1.0);
                            va[y * B + x] = if fr > 1.0 { vr / fr } else { vr };
                            wa[y * B + x] = (wd[g][y * B + x] as f64 / ta) as f32;
                        }
                    }
                    accf[g] = Vec::new();
                    accv[g] = Vec::new();
                    wd[g] = Vec::new();
                    groups[g] = Some([fa, wa, va]);
                }
                // A level of the block from a set of groups (each class's f clamped to 1).
                let comb = |gs: &[usize]| {
                    let mut lv = Lv::zero(B);
                    for &g in gs {
                        let Some([fa, wa, va]) = &groups[g] else { continue };
                        let c = GCLASS[g];
                        for i in 0..B * B {
                            let (f0, v0) = (lv.p[c][i] + fa[i], lv.p[2 * K + c][i] + va[i]);
                            lv.p[c][i] = f0.min(1.0);
                            lv.p[K + c][i] += wa[i];
                            lv.p[2 * K + c][i] = if f0 > 1.0 { v0 / f0 } else { v0 };
                        }
                    }
                    lv
                };
                // Level 14: what z13's meshes leave out (under 20 m and under 2,000 m2).
                let l14 = comb(&[0, 1]);
                for ty in 0..8 {
                    for tx in 0..8 {
                        out.put(14, bx * 8 + tx as u32, by * 8 + ty as u32, l14.tile(tx, ty));
                    }
                }
                drop(l14);
                // Level 13: what z12's meshes leave out (under 40 m).
                let l13 = comb(&[0, 1, 2, 3, 4]).half();
                for ty in 0..4 {
                    for tx in 0..4 {
                        out.put(13, bx * 4 + tx as u32, by * 4 + ty as u32, l13.tile(tx, ty));
                    }
                }
                drop(l13);
                let l12 = comb(&[0, 1, 2, 3, 4, 5, 6]).half().half();
                drop(groups);
                for ty in 0..2 {
                    for tx in 0..2 {
                        out.put(12, bx * 2 + tx as u32, by * 2 + ty as u32, l12.tile(tx, ty));
                    }
                }
                let l11 = l12.half();
                out.put(11, bx, by, l11.tile(0, 0));
                Some(((bx, by), l11))
            })
            .collect()
    });
    eprintln!("blocks done: {} features rasterized, {:.1} s", nb.lock().unwrap(), t0.elapsed().as_secs_f64());
    // Levels 10..4: each tile from a 1024 x 1024 mosaic of its 4 children, halved.
    let mut cur: BTreeMap<(u32, u32), Lv> = l11.into_iter().collect();
    for z in (4..11).rev() {
        let mut next: BTreeMap<(u32, u32), Lv> = BTreeMap::new();
        let parents: std::collections::BTreeSet<(u32, u32)> = cur.keys().map(|&(x, y)| (x / 2, y / 2)).collect();
        for (px, py) in parents {
            let mut m = Lv::zero(2 * T);
            for dy in 0..2 {
                for dx in 0..2 {
                    if let Some(c) = cur.get(&(px * 2 + dx, py * 2 + dy)) {
                        for (a, b) in c.p.iter().zip(m.p.iter_mut()) {
                            for y in 0..T {
                                for x in 0..T {
                                    b[(dy as usize * T + y) * 2 * T + dx as usize * T + x] = a[y * T + x];
                                }
                            }
                        }
                    }
                }
            }
            let h = m.half();
            out.put(z, px, py, h.tile(0, 0));
            next.insert((px, py), h);
        }
        cur = next;
    }
    let s = out.stats.lock().unwrap();
    let mut js = String::from("{");
    for (i, (z, (n, b, mx))) in s.iter().enumerate() {
        js += &format!(
            "{}\"{}\": {{\"tiles\": {}, \"bytes\": {}, \"max\": {}}}",
            if i > 0 { ", " } else { "" },
            z,
            n,
            b,
            mx
        );
    }
    js += "}";
    std::fs::write(format!("{}/stats.json", out.dir), &js).unwrap();
    eprintln!("{js}\n{:.1} s", t0.elapsed().as_secs_f64());
}
