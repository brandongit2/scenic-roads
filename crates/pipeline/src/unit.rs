//! base(U) (docs/plan.md §6): one unit's base pack, from its OSM piece, by today's steps run on a
//! unit-sized build folder (`extract`, `dem/sample.py`, `tile … elev`, `scenic-metrics`), then the
//! same conversion as today's data (`legacy::base_sections`) for the ways the unit owns (first
//! vertex inside it) that touch the coverage, with the pass's worldwide road values.
//!
//! Before the expensive steps the folder is cut down to the ways that touch the coverage (whoever
//! owns them: a way just outside still gives the clean-up its junction context at shared nodes).
//! The global-source layers come from the catalog's packs (`stage`). Each unit reads a slice of the
//! per-vertex DEM cache: today's cache (the seed, copied once from the NAS's `sources/dem-cache/`)
//! and the samples every unit kept from its last run (`dem_samples_keep`), so a vertex is sampled
//! from the DEM servers once.

use crate::coverage::Coverage;
use crate::legacy::Unit;
use anyhow::{bail, ensure, Context, Result};
use roadcore::WayRec;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A DEM cache key: `(lon + 2³¹) << 32 | (lat + 2³¹)` (E7), so sorted keys keep a longitude range
/// contiguous.
fn dem_key(lon: i32, lat: i32) -> u64 {
    (((lon as i64 + (1i64 << 31)) as u64) << 32) | ((lat as i64 + (1i64 << 31)) as u64)
}

fn dem_lon_lat(x: u64) -> (i32, i32) {
    (((x >> 32) as i64 - (1i64 << 31)) as i32, ((x & 0xffff_ffff) as i64 - (1i64 << 31)) as i32)
}

/// The entries of sorted DEM cache arrays inside `b` (w, s, e, n, E7).
fn dem_in_box(keys: &[u64], elev: &[f32], srcs: &[u8], b: [i32; 4], out: &mut Vec<(u64, f32, u8)>) {
    let lo = keys.partition_point(|&x| x < dem_key(b[0], i32::MIN));
    let hi = keys.partition_point(|&x| x <= dem_key(b[2], i32::MAX));
    for i in lo..hi {
        let lat = dem_lon_lat(keys[i]).1;
        if lat >= b[1] && lat <= b[3] {
            out.push((keys[i], elev[i], srcs[i]));
        }
    }
}

/// The per-unit DEM samples (`<cache>/dem-units/<unit>.dem`): one file per unit, replaced after each
/// of its runs (written whole, then renamed): "RDDEM002", the count (u64), the box of its points
/// (4 × i32, E7), the versions of `rules::DEM_RULES` it was sampled under (4 × u32), then the
/// sorted keys (u64), elevations (f32) and sources (u8).
const DEM_UNITS: &str = "dem-units";
const DEM_MAGIC: &[u8; 8] = b"RDDEM002";
const DEM_HEAD: usize = 48;

/// The DEM rules' versions today's cache (the seed) was sampled under: the first of each.
const SEED_DEM_VERSIONS: [u32; 4] = [1, 1, 1, 1];

fn current_dem_versions() -> [u32; 4] {
    crate::rules::DEM_RULES.map(crate::rules::version)
}

/// Keeps a unit's DEM samples (the `dem-cache.*` sample.py left in `from`: its vertices, cached or
/// sampled anew) for later runs of it and of its neighbours. Returns how many.
pub fn dem_samples_keep(cache: &Path, u: Unit, from: &Path) -> Result<usize> {
    let read = |n: &str| std::fs::read(from.join(format!("dem-cache.{n}")));
    let (Ok(kb), Ok(eb), Ok(sb)) = (read("keys.u64"), read("elev.f32"), read("src.u8")) else { return Ok(0) };
    let keys: Vec<u64> = bytemuck::pod_collect_to_vec(&kb);
    let n = keys.len();
    ensure!(eb.len() == 4 * n && sb.len() == n, "{}: DEM cache files out of step", from.display());
    let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for &k in &keys {
        let (lon, lat) = dem_lon_lat(k);
        bb = [bb[0].min(lon), bb[1].min(lat), bb[2].max(lon), bb[3].max(lat)];
    }
    let dir = cache.join(DEM_UNITS);
    std::fs::create_dir_all(&dir)?;
    let mut f = Vec::with_capacity(DEM_HEAD + 13 * n);
    f.extend_from_slice(DEM_MAGIC);
    f.extend_from_slice(&(n as u64).to_le_bytes());
    for v in bb {
        f.extend_from_slice(&v.to_le_bytes());
    }
    for v in current_dem_versions() {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&kb);
    f.extend_from_slice(&eb);
    f.extend_from_slice(&sb);
    let p = dir.join(format!("{}.dem", u.dash()));
    let tmp = p.with_extension("dem.tmp");
    std::fs::write(&tmp, &f)?;
    std::fs::rename(&tmp, &p)?;
    Ok(n)
}

/// The entries of sorted DEM cache arrays inside `b` still valid: those whose DEM rules (by their
/// source and place, `rules::dem_rules_of`) have the versions they were sampled under (`made`) now;
/// the rest are left out, so sample.py samples them again under the changed rule.
fn dem_valid_in_box(keys: &[u64], elev: &[f32], srcs: &[u8], b: [i32; 4], made: [u32; 4], out: &mut Vec<(u64, f32, u8)>) {
    let now = current_dem_versions();
    if made == now {
        dem_in_box(keys, elev, srcs, b, out);
        return;
    }
    let mut all = Vec::new();
    dem_in_box(keys, elev, srcs, b, &mut all);
    out.extend(all.into_iter().filter(|&(k, _, src)| {
        let (lon, lat) = dem_lon_lat(k);
        crate::rules::dem_rules_of(src, lon, lat).into_iter().all(|r| made[r] == now[r])
    }));
}

/// Copies the DEM cache entries inside `b` (w, s, e, n, E7) into `dst` (`dem-cache.*` files, for
/// sample.py): the seed's (`cache`'s `dem-cache.*`, today's cache), then every unit's kept samples
/// whose box meets `b` (`dem_samples_keep`), which win over the seed's (they're newer); entries
/// sampled under a DEM rule's earlier version are left out. With none, nothing is written (sample.py
/// samples every vertex). Each unit's file is opened by its header first, and mapped only when its
/// box meets `b`.
pub fn dem_cache_slice(cache: &Path, b: [i32; 4], dst: &Path) -> Result<usize> {
    use std::io::Read;
    std::fs::create_dir_all(dst)?;
    for n in ["keys.u64", "elev.f32", "src.u8"] {
        std::fs::remove_file(dst.join(format!("dem-cache.{n}"))).ok();
    }
    let mut all: Vec<(u64, f32, u8)> = Vec::new();
    let open = |n: &str| roadcore::mmap(&cache.join(format!("dem-cache.{n}")));
    if let (Ok(km), Ok(em), Ok(sm)) = (open("keys.u64"), open("elev.f32"), open("src.u8")) {
        let (keys, elev): (&[u64], &[f32]) = (bytemuck::cast_slice(&km[..]), bytemuck::cast_slice(&em[..]));
        ensure!(keys.len() == elev.len() && keys.len() == sm.len(), "DEM cache files out of step");
        dem_valid_in_box(keys, elev, &sm[..], b, SEED_DEM_VERSIONS, &mut all);
    }
    let seed = all.len();
    let mut units: Vec<PathBuf> = std::fs::read_dir(cache.join(DEM_UNITS)).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "dem")).collect()).unwrap_or_default();
    units.sort();
    let mut newer: Vec<(u64, f32, u8)> = Vec::new();
    for p in &units {
        let mut head = [0u8; DEM_HEAD];
        std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut head)).with_context(|| format!("{}: its header", p.display()))?;
        ensure!(&head[..8] == DEM_MAGIC, "{}: not a DEM samples file of this version", p.display());
        let n = u64::from_le_bytes(head[8..16].try_into().unwrap()) as usize;
        let i32_at = |i: usize| i32::from_le_bytes(head[i..i + 4].try_into().unwrap());
        let ub = [i32_at(16), i32_at(20), i32_at(24), i32_at(28)];
        if ub[0] > b[2] || ub[2] < b[0] || ub[1] > b[3] || ub[3] < b[1] {
            continue;
        }
        let u32_at = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().unwrap());
        let made = [u32_at(32), u32_at(36), u32_at(40), u32_at(44)];
        let m = roadcore::mmap(p)?;
        ensure!(m.len() == DEM_HEAD + 13 * n, "{}: truncated", p.display());
        let keys: &[u64] = bytemuck::cast_slice(&m[DEM_HEAD..DEM_HEAD + 8 * n]);
        let elev: &[f32] = bytemuck::cast_slice(&m[DEM_HEAD + 8 * n..DEM_HEAD + 12 * n]);
        dem_valid_in_box(keys, elev, &m[DEM_HEAD + 12 * n..], b, made, &mut newer);
    }
    // The units' samples first, so a stable dedup keeps theirs (by file name order among them).
    newer.extend(all);
    newer.sort_by_key(|e| e.0);
    newer.dedup_by_key(|e| e.0);
    if newer.is_empty() {
        return Ok(0);
    }
    let (ok, oe, os): (Vec<u64>, Vec<f32>, Vec<u8>) = (newer.iter().map(|e| e.0).collect(), newer.iter().map(|e| e.1).collect(), newer.iter().map(|e| e.2).collect());
    for (n, bytes) in [("keys.u64", bytemuck::cast_slice::<u64, u8>(&ok)), ("elev.f32", bytemuck::cast_slice(&oe)), ("src.u8", &os[..])] {
        let tmp = dst.join(format!("dem-cache.{n}.tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, dst.join(format!("dem-cache.{n}")))?;
    }
    if ok.len() > seed {
        eprintln!("  DEM cache: {} of the slice's {} entries from units' kept samples", ok.len() - seed, ok.len());
    }
    Ok(ok.len())
}

/// Puts today's DEM cache (the seed) in `cache` when it isn't there whole, from the NAS's copy
/// (`sources/dem-cache/`): once per build Mac. Without one the units sample every vertex anew.
pub fn dem_seed(root: &Path, cache: &Path) -> Result<()> {
    let names = ["keys.u64", "elev.f32", "src.u8"];
    let len = |p: PathBuf| std::fs::metadata(p).map(|m| m.len()).ok();
    let here: Vec<Option<u64>> = names.iter().map(|n| len(cache.join(format!("dem-cache.{n}")))).collect();
    let whole = |l: &[Option<u64>]| matches!(l, [Some(k), Some(e), Some(s)] if *k == 8 * *s && *e == 4 * *s);
    if whole(&here) {
        return Ok(());
    }
    let src = root.join("sources/dem-cache");
    let there: Vec<Option<u64>> = names.iter().map(|n| len(src.join(format!("dem-cache.{n}")))).collect();
    if !whole(&there) {
        eprintln!("unit: no DEM cache to start from on the NAS ({}); sampling every vertex anew", src.display());
        return Ok(());
    }
    std::fs::create_dir_all(cache)?;
    let t = std::time::Instant::now();
    // Each file whole before the next (a half-copied one is copied again: its length is wrong).
    for n in names {
        let (from, to) = (src.join(format!("dem-cache.{n}")), cache.join(format!("dem-cache.{n}")));
        let tmp = to.with_extension(format!("{}.tmp", to.extension().unwrap().to_string_lossy()));
        std::fs::copy(&from, &tmp).with_context(|| format!("copy {}", from.display()))?;
        std::fs::rename(&tmp, &to)?;
    }
    eprintln!("unit: DEM cache copied from the NAS ({:.0?})", t.elapsed());
    Ok(())
}

/// Rewrites a build folder's `ways.bin` and `verts.bin` keeping the ways `keep` says (in order);
/// run right after `extract`, before any per-vertex array exists. Returns (ways, vertices) kept.
pub fn subset(dir: &Path, keep: impl Fn(&WayRec, &[[i32; 2]]) -> bool) -> Result<(usize, usize)> {
    let w = roadcore::Ways::open(dir)?;
    let (ways, verts) = (w.ways(), w.verts());
    let mut out_ways: Vec<WayRec> = Vec::new();
    let mut out_verts: Vec<[i32; 2]> = Vec::new();
    for r in ways {
        let vs = &verts[r.vstart as usize..(r.vstart + r.vcount as u64) as usize];
        if keep(r, vs) {
            let mut n = *r;
            n.vstart = out_verts.len() as u64;
            out_verts.extend_from_slice(vs);
            out_ways.push(n);
        }
    }
    let mut head = Vec::with_capacity(16);
    head.extend_from_slice(roadcore::WAYS_MAGIC);
    head.extend_from_slice(&(out_ways.len() as u64).to_le_bytes());
    let mut wb = head;
    wb.extend_from_slice(bytemuck::cast_slice(&out_ways));
    drop(w);
    std::fs::write(roadcore::tmp(dir, "ways.bin"), &wb)?;
    std::fs::write(roadcore::tmp(dir, "verts.bin"), bytemuck::cast_slice(&out_verts))?;
    roadcore::commit(dir, &["ways.bin", "verts.bin"])?;
    Ok((out_ways.len(), out_verts.len()))
}

/// Whether the unit with bounds `tb` (w, s, e, n, E7) owns a way starting at `p`.
pub fn owns(tb: [i32; 4], p: [i32; 2]) -> bool {
    p[0] >= tb[0] && p[0] < tb[2] && p[1] >= tb[1] && p[1] < tb[3]
}

/// Where a unit's steps find their programs and caches.
pub struct Tools {
    /// `extract`, `tile`, `scenic-metrics`.
    pub bin: PathBuf,
    /// The repository's `dem/` folder (run with `uv run python`).
    pub dem: PathBuf,
    /// Shared caches: `chm10/` (canopy 10° files), `dem-cache.*` (today's per-vertex elevations, the
    /// seed) and `dem-units/` (the units' own samples).
    pub cache: PathBuf,
    /// Overture building boxes (`data/buildings`), when there are any.
    pub buildings: Option<PathBuf>,
    /// Taiwan's MOI DTM GeoTIFFs (the NAS's `inputs/moi-dtm/`), for sample.py.
    pub moi_dtm: Option<PathBuf>,
    /// Densification spacing (m).
    pub spacing_m: u32,
}

fn run(mut c: Command, what: &str, log: &Path) -> Result<()> {
    let f = std::fs::File::options().create(true).append(true).open(log)?;
    let t = std::time::Instant::now();
    let st = c.stdout(f.try_clone()?).stderr(f).status().with_context(|| format!("start {what}"))?;
    if !st.success() {
        bail!("{what} failed ({st}); see {}", log.display());
    }
    eprintln!("  {what}: {:.0?}", t.elapsed());
    Ok(())
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub unit: String,
    pub piece_ways: usize,
    pub kept_ways: usize,
    pub kept_verts: usize,
    pub owned: usize,
    pub dem_cache: usize,
    pub staged: crate::stage::Staged,
    /// Heritage sites and designated-area polygons around the unit.
    pub heritage: usize,
    pub areas: usize,
}

/// Writes a unit's heritage inputs for its box (degrees) into its folder: `heritage.json` and
/// `area-shapes.geojsonseq` (crate::heritage::unit_inputs); the sites and polygons written.
pub type HeritageInputs<'a> = &'a dyn Fn([f64; 4], &Path) -> Result<(usize, usize)>;

/// Runs today's steps for unit `u` in `dir` from `piece`, with the coverage and the global-source
/// layers on the NAS (`src`). Leaves the build folder ready for conversion.
pub fn build_folder(u: Unit, piece: &Path, dir: &Path, cov: &Coverage, src: &crate::stage::Source, tools: &Tools, heritage: HeritageInputs) -> Result<Report> {
    std::fs::create_dir_all(dir)?;
    let log = dir.join("steps.log");
    let mut rep = Report { unit: u.slash(), ..Default::default() };
    // 1. Every way of the piece, densified.
    let mut c = Command::new(tools.bin.join("extract"));
    c.arg(dir).arg(tools.spacing_m.to_string()).arg(piece);
    run(c, "extract", &log)?;
    rep.piece_ways = roadcore::Ways::open(dir)?.ways().len();
    // 2. Only what touches the coverage goes on.
    let (kw, kv) = subset(dir, |_, vs| cov.touches(vs))?;
    (rep.kept_ways, rep.kept_verts) = (kw, kv);
    if kw == 0 {
        return Ok(rep);
    }
    let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
    {
        let w = roadcore::Ways::open(dir)?;
        let verts = w.verts();
        rep.owned = w.ways().iter().filter(|r| owns(tb, verts[r.vstart as usize])).count();
    }
    // 3. Elevations: the DEM cache's slice over every vertex the folder kept (its long ways too).
    let slice = {
        let w = roadcore::Ways::open(dir)?;
        w.verts().iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])])
    };
    rep.dem_cache = dem_cache_slice(&tools.cache, slice, &dir.join("dem-cache"))?;
    let mut c = Command::new("uv");
    c.current_dir(&tools.dem).args(["run", "python", "sample.py"]).arg(dir).arg("--cache").arg(dir.join("dem-cache"));
    if let Some(m) = &tools.moi_dtm {
        c.env("SCENIC_MOI_DTM", m);
    }
    run(c, "elevations (sample.py)", &log)?;
    // Its samples, kept for its later runs and its neighbours' (new ones aren't sampled twice).
    dem_samples_keep(&tools.cache, u, &dir.join("dem-cache"))?;
    // 4. The global-source layers the steps read, from the packs.
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    rep.staged = crate::stage::stage(src, b, dir)?;
    // The heritage sites around the unit (the flags step's `heritage.json`), and the designated
    // areas rasterised onto its grid (`grid.areas.u8`), from the heritage-sites job's slices.
    (rep.heritage, rep.areas) = heritage(b, dir)?;
    let mut c = Command::new("uv");
    c.current_dir(&tools.dem).args(["run", "python", "areaflags.py"]).arg(dir).arg(dir.join("area-shapes.geojsonseq"));
    run(c, "area flags (areaflags.py)", &log)?;
    // Land cover the packs lack (new coverage): ESA WorldCover for those grid tiles only; the
    // rest stays as staged.
    if rep.staged.missing.get("class").copied().unwrap_or(0) > 0 {
        let mut c = Command::new("uv");
        c.current_dir(&tools.dem).args(["run", "python", "landcover.py"]).arg(dir).arg("--only").arg(dir.join("grid.class.missing.u32"));
        run(c, "land cover (landcover.py)", &log)?;
    }
    // 5. Clean-up and grade; road samples; canopy; views; buildings; flags.
    let mut c = Command::new(tools.bin.join("tile"));
    c.arg(dir).arg("elev");
    run(c, "clean-up and grade (tile elev)", &log)?;
    for step in ["prep", "canopy", "view"] {
        let mut c = Command::new(tools.bin.join("scenic-metrics"));
        c.arg(dir).arg(step).env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", dir.join("scache"));
        run(c, &format!("scenic {step}"), &log)?;
    }
    if let Some(bd) = &tools.buildings {
        let mut c = Command::new(tools.bin.join("scenic-metrics"));
        c.arg(dir).arg("buildings").arg(bd).env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", dir.join("scache"));
        run(c, "scenic buildings", &log)?;
    }
    let mut c = Command::new(tools.bin.join("scenic-metrics"));
    c.arg(dir).arg("flags").env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", dir.join("scache"));
    run(c, "scenic flags", &log)?;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dem_slice_by_box() {
        let d = tempfile::tempdir().unwrap();
        let k = |lon: i32, lat: i32| (((lon as i64 + (1i64 << 31)) as u64) << 32) | ((lat as i64 + (1i64 << 31)) as u64);
        let mut pts = vec![(-10, -10), (-10, 5), (0, 0), (0, 20), (3, 3), (5, -1), (20, 0)];
        pts.sort_by_key(|&(a, b)| k(a, b));
        let keys: Vec<u64> = pts.iter().map(|&(a, b)| k(a, b)).collect();
        let elev: Vec<f32> = (0..pts.len()).map(|i| i as f32).collect();
        std::fs::write(d.path().join("dem-cache.keys.u64"), bytemuck::cast_slice(&keys)).unwrap();
        std::fs::write(d.path().join("dem-cache.elev.f32"), bytemuck::cast_slice(&elev)).unwrap();
        std::fs::write(d.path().join("dem-cache.src.u8"), vec![4u8; pts.len()]).unwrap();
        let n = dem_cache_slice(d.path(), [-5, -5, 10, 10], &d.path().join("s")).unwrap();
        // (0,0), (3,3), (5,-1)
        assert_eq!(n, 3);
        let got: Vec<u64> = bytemuck::pod_collect_to_vec(&std::fs::read(d.path().join("s/dem-cache.keys.u64")).unwrap());
        assert_eq!(got, vec![k(0, 0), k(3, 3), k(5, -1)]);
        // No cache: an empty slice, and no files (sample.py can't map an empty one).
        assert_eq!(dem_cache_slice(&d.path().join("none"), [0, 0, 1, 1], &d.path().join("t")).unwrap(), 0);
        assert!(!d.path().join("t/dem-cache.keys.u64").exists());
        // A unit's kept samples: in the slices of boxes meeting them, over the seed's.
        let run = d.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let mine = [k(3, 3), k(4, 4)];
        std::fs::write(run.join("dem-cache.keys.u64"), bytemuck::cast_slice(&mine)).unwrap();
        std::fs::write(run.join("dem-cache.elev.f32"), bytemuck::cast_slice(&[30.0f32, 40.0])).unwrap();
        std::fs::write(run.join("dem-cache.src.u8"), [1u8, 1]).unwrap();
        assert_eq!(dem_samples_keep(d.path(), Unit { z: 6, x: 1, y: 2 }, &run).unwrap(), 2);
        assert_eq!(dem_cache_slice(d.path(), [-5, -5, 10, 10], &d.path().join("u")).unwrap(), 4);
        let got: Vec<u64> = bytemuck::pod_collect_to_vec(&std::fs::read(d.path().join("u/dem-cache.keys.u64")).unwrap());
        let el: Vec<f32> = bytemuck::pod_collect_to_vec(&std::fs::read(d.path().join("u/dem-cache.elev.f32")).unwrap());
        assert_eq!(got, vec![k(0, 0), k(3, 3), k(4, 4), k(5, -1)]);
        assert_eq!(el[1], 30.0, "the unit's sample, not the seed's");
        // A box away from them: the seed's only.
        assert_eq!(dem_cache_slice(d.path(), [-15, -15, -5, 10], &d.path().join("v")).unwrap(), 2);
    }

    #[test]
    fn samples_of_a_changed_dem_rule_are_dropped() {
        let k = dem_key;
        let e7 = |x: f64| (x * 1e7) as i32;
        // Québec from HRDEM (1), Québec from FABDEM standing in (4), Tokyo from GSI (5), Paris from
        // FABDEM (4).
        let pts = [(e7(-71.2), e7(46.8), 1u8), (e7(-71.3), e7(46.9), 4), (e7(139.7), e7(35.7), 5), (e7(2.35), e7(48.85), 4)];
        let mut v: Vec<(u64, f32, u8)> = pts.iter().map(|&(x, y, s)| (k(x, y), 1.0, s)).collect();
        v.sort_by_key(|e| e.0);
        let (keys, elev, srcs): (Vec<u64>, Vec<f32>, Vec<u8>) = (v.iter().map(|e| e.0).collect(), v.iter().map(|e| e.1).collect(), v.iter().map(|e| e.2).collect());
        let world = [i32::MIN, i32::MIN, i32::MAX, i32::MAX];
        let now = current_dem_versions();
        let kept = |made: [u32; 4]| {
            let mut out = Vec::new();
            dem_valid_in_box(&keys, &elev, &srcs, world, made, &mut out);
            out.iter().map(|e| e.2).collect::<Vec<u8>>()
        };
        assert_eq!(kept(now).len(), 4);
        // North America's rule changed since: both Québec samples go (FABDEM stood in for it there).
        let mut older = now;
        older[0] = now[0].wrapping_sub(1);
        let mut got = kept(older);
        got.sort();
        assert_eq!(got, vec![4, 5], "Paris's FABDEM and Tokyo's GSI stay");
        // FABDEM's: Paris's and Québec's FABDEM sample go, HRDEM's and GSI's stay.
        let mut older = now;
        older[3] = now[3].wrapping_sub(1);
        let mut got = kept(older);
        got.sort();
        assert_eq!(got, vec![1, 5]);
    }

    #[test]
    fn subset_rewrites_ways_and_verts() {
        let d = tempfile::tempdir().unwrap();
        let mk = |id: i64, vstart: u64, vcount: u32| WayRec { id, vstart, vcount, ..bytemuck::Zeroable::zeroed() };
        let ways = [mk(1, 0, 2), mk(2, 2, 3), mk(3, 5, 2)];
        let verts: Vec<[i32; 2]> = (0..7).map(|i| [i, i]).collect();
        let mut wb = roadcore::WAYS_MAGIC.to_vec();
        wb.extend_from_slice(&3u64.to_le_bytes());
        wb.extend_from_slice(bytemuck::cast_slice(&ways));
        std::fs::write(d.path().join("ways.bin"), wb).unwrap();
        std::fs::write(d.path().join("verts.bin"), bytemuck::cast_slice(&verts)).unwrap();
        let (n, v) = subset(d.path(), |w, _| w.id != 2).unwrap();
        assert_eq!((n, v), (2, 4));
        let w = roadcore::Ways::open(d.path()).unwrap();
        assert_eq!(w.ways().iter().map(|r| (r.id, r.vstart, r.vcount)).collect::<Vec<_>>(), vec![(1, 0, 2), (3, 2, 2)]);
        assert_eq!(w.verts(), &[[0, 0], [1, 1], [5, 5], [6, 6]]);
    }
}
