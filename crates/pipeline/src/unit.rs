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

/// The per-unit DEM samples (`dem-units/<unit>.<box>.dem`, in the NAS's `cache/` so both Macs'
/// units read them; `Tools::dem_units`): one file per unit, replaced after each of its runs (written
/// whole, then renamed; the old one deleted after): "RDDEM002", the count (u64), the box of its
/// points (4 × i32, E7), the versions of `rules::DEM_RULES` it was sampled under (4 × u32), then the
/// sorted keys (u64), elevations (f32) and sources (u8). The box is in the name too (`box_tag`), so
/// a unit finds the files near it from one listing of the folder, without opening each.
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
pub fn dem_samples_keep(dir: &Path, u: Unit, from: &Path) -> Result<usize> {
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
    std::fs::create_dir_all(dir)?;
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
    let name = format!("{}.{}.dem", u.dash(), box_tag(bb));
    crate::whole::write(&dir.join(&name), &f)?;
    // Its earlier file (another box), gone.
    let prefix = format!("{}.", u.dash());
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let other = e.file_name().to_string_lossy().into_owned();
        if other.starts_with(&prefix) && other.ends_with(".dem") && other != name {
            std::fs::remove_file(e.path()).ok();
        }
    }
    Ok(n)
}

/// A box (w, s, e, n, E7) as it's named: 32 hex digits.
fn box_tag(b: [i32; 4]) -> String {
    b.iter().map(|v| format!("{:08x}", *v as u32)).collect()
}

/// A DEM samples file's unit and box, from its name (`<unit>.<box>.dem`); the box None for a name
/// without one.
fn dem_file(name: &str) -> Option<(String, Option<[i32; 4]>)> {
    let stem = name.strip_suffix(".dem")?;
    match stem.split_once('.') {
        Some((u, tag)) if tag.len() == 32 => {
            let v = |i: usize| u32::from_str_radix(&tag[i * 8..i * 8 + 8], 16).ok().map(|x| x as i32);
            Some((u.to_string(), Some([v(0)?, v(1)?, v(2)?, v(3)?])))
        }
        None => Some((stem.to_string(), None)),
        _ => None,
    }
}

/// Moves what units kept in this Mac's own cache (`dem-units/`, `scenic-units/`) to the shared one
/// (`Tools::shared`), once: both Macs' units read them there.
pub fn move_kept_to_shared(cache: &Path, shared: &Path) -> Result<usize> {
    let mut moved = 0;
    let local = cache.join(DEM_UNITS);
    for e in std::fs::read_dir(&local).into_iter().flatten().flatten() {
        let p = e.path();
        let Some((u, _)) = dem_file(&e.file_name().to_string_lossy()) else { continue };
        let Ok(m) = roadcore::mmap(&p) else { continue };
        let Some(bb) = dem_head(&m).map(|h| h.1) else { continue };
        drop(m);
        std::fs::create_dir_all(shared.join(DEM_UNITS))?;
        crate::whole::copy(&p, &shared.join(DEM_UNITS).join(format!("{u}.{}.dem", box_tag(bb))))?;
        std::fs::remove_file(&p)?;
        moved += 1;
    }
    let local = cache.join("scenic-units");
    for e in std::fs::read_dir(&local).into_iter().flatten().flatten() {
        let (from, to) = (e.path(), shared.join("scenic-units").join(e.file_name()));
        if !from.is_dir() || crate::whole::is_tmp(&from) {
            continue;
        }
        if !to.exists() {
            let tmp = to.with_extension(format!("{}.tmp", crate::agent::cond::host()));
            std::fs::remove_dir_all(&tmp).ok();
            std::fs::create_dir_all(&tmp)?;
            for f in std::fs::read_dir(&from)?.flatten() {
                crate::whole::copy(&f.path(), &tmp.join(f.file_name()))?;
            }
            std::fs::rename(&tmp, &to)?;
        }
        std::fs::remove_dir_all(&from)?;
        moved += 1;
    }
    Ok(moved)
}

/// A DEM samples file's header, read from its bytes: (count, box, versions), when it's one of this
/// version and whole.
fn dem_head(m: &[u8]) -> Option<(usize, [i32; 4], [u32; 4])> {
    if m.len() < DEM_HEAD || &m[..8] != DEM_MAGIC {
        return None;
    }
    let n = u64::from_le_bytes(m[8..16].try_into().ok()?) as usize;
    let i32_at = |i: usize| i32::from_le_bytes(m[i..i + 4].try_into().unwrap());
    let u32_at = |i: usize| u32::from_le_bytes(m[i..i + 4].try_into().unwrap());
    (m.len() == DEM_HEAD + 13 * n).then(|| (n, [i32_at(16), i32_at(20), i32_at(24), i32_at(28)], [u32_at(32), u32_at(36), u32_at(40), u32_at(44)]))
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
/// in `units` whose box meets `b` (`dem_samples_keep`), which win over the seed's (they're newer); entries
/// sampled under a DEM rule's earlier version are left out. With none, nothing is written (sample.py
/// samples every vertex). The files near `b` are found by the boxes in their names (one listing);
/// a unit's newest file counts, and one that isn't whole is passed over (it's only a cache).
pub fn dem_cache_slice(cache: &Path, units_dir: &Path, b: [i32; 4], dst: &Path) -> Result<usize> {
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
    // Each unit's file, by its name: the newest when there are two (one being replaced).
    let mut files: std::collections::BTreeMap<String, (PathBuf, Option<[i32; 4]>)> = Default::default();
    for e in std::fs::read_dir(units_dir).into_iter().flatten().flatten() {
        let Some((u, bx)) = dem_file(&e.file_name().to_string_lossy()) else { continue };
        let p = e.path();
        if let Some((q, _)) = files.get(&u) {
            let t = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            if t(q) >= t(&p) {
                continue;
            }
        }
        files.insert(u, (p, bx));
    }
    let meets = |ub: [i32; 4]| !(ub[0] > b[2] || ub[2] < b[0] || ub[1] > b[3] || ub[3] < b[1]);
    let mut newer: Vec<(u64, f32, u8)> = Vec::new();
    for (p, bx) in files.values() {
        if bx.is_some_and(|ub| !meets(ub)) {
            continue;
        }
        // (Read, not mapped: they're on the NAS, where a mapped page lost with the share would end
        // the job.)
        let Ok(m) = std::fs::read(p) else {
            eprintln!("  DEM cache: {} can't be read now; passed over", p.display());
            continue;
        };
        let Some((n, ub, made)) = dem_head(&m) else {
            eprintln!("  DEM cache: {} isn't whole; passed over", p.display());
            continue;
        };
        if !meets(ub) {
            continue;
        }
        let keys: Vec<u64> = bytemuck::pod_collect_to_vec(&m[DEM_HEAD..DEM_HEAD + 8 * n]);
        let elev: Vec<f32> = bytemuck::pod_collect_to_vec(&m[DEM_HEAD + 8 * n..DEM_HEAD + 12 * n]);
        dem_valid_in_box(&keys, &elev, &m[DEM_HEAD + 12 * n..], b, made, &mut newer);
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
/// (`sources/dem-cache/`): once per Mac. Without one the units sample every vertex anew.
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
#[derive(Clone)]
pub struct Tools {
    /// `extract`, `tile`, `scenic-metrics`.
    pub bin: PathBuf,
    /// The repository's `dem/` folder (run with `uv run python`).
    pub dem: PathBuf,
    /// This Mac's caches: `chm10/` (canopy 10° files) and `dem-cache.*` (today's per-vertex
    /// elevations, the seed); and the units' kept results when there's no `shared`.
    pub cache: PathBuf,
    /// Overture building boxes: a folder of `.f32` files (the unit's tiles staged by
    /// `buildtiles::stage`), when there are any.
    pub buildings: Option<PathBuf>,
    /// Taiwan's MOI DTM GeoTIFFs (the NAS's `inputs/moi-dtm/`), for sample.py.
    pub moi_dtm: Option<PathBuf>,
    /// The NAS's `sources/`, where downloads are kept, each downloaded once: Meta's canopy squares
    /// (`canopy/`) and FABDEM's tiles (`fabdem/`); the local caches fill from it. None: local
    /// caches alone.
    pub sources: Option<PathBuf>,
    /// The NAS's `cache/`, where what a unit keeps for its later runs is shared by both Macs: its DEM
    /// samples (`dem-units/`) and scenic results (`scenic-units/`). None: the local cache.
    pub shared: Option<PathBuf>,
    /// Densification spacing (m).
    pub spacing_m: u32,
}

impl Tools {
    /// The units' kept DEM samples (`Tools::shared`, else the local cache).
    pub fn dem_units(&self) -> PathBuf {
        self.shared.as_ref().unwrap_or(&self.cache).join(DEM_UNITS)
    }

    /// Where unit `u`'s scenic results are kept between its runs (crate::scache::Carry).
    pub fn scenic_kept(&self, u: Unit) -> PathBuf {
        self.shared.as_ref().unwrap_or(&self.cache).join("scenic-units").join(u.dash())
    }
}

/// The most memory (resident, bytes) one of the steps' programs took since `take_peak` last read it.
static PEAK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The most memory one of the steps' programs took since the last call (scenic-build's log line per
/// unit), and starts again.
pub fn take_peak() -> u64 {
    PEAK.swap(0, std::sync::atomic::Ordering::Relaxed)
}

fn run(mut c: Command, what: &str, log: &Path) -> Result<()> {
    use std::os::unix::process::ExitStatusExt;
    let f = std::fs::File::options().create(true).append(true).open(log)?;
    let t = std::time::Instant::now();
    let child = c.stdout(f.try_clone()?).stderr(f).spawn().with_context(|| format!("start {what}"))?;
    // Waited for here, not by `Child::wait`, for what it used: its (and its programs') peak memory.
    let pid = child.id() as libc::pid_t;
    let (mut status, mut ru): (libc::c_int, libc::rusage) = (0, unsafe { std::mem::zeroed() });
    loop {
        // SAFETY: wait4 on our own child, into values we own.
        if unsafe { libc::wait4(pid, &mut status, 0, &mut ru) } == pid {
            break;
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e).with_context(|| format!("wait for {what}"));
        }
    }
    // (Bytes on macOS.)
    PEAK.fetch_max(ru.ru_maxrss as u64, std::sync::atomic::Ordering::Relaxed);
    let st = std::process::ExitStatus::from_raw(status);
    if !st.success() {
        // The end of its log into the job's: the unit's folder, its log with it, goes when the next
        // job starts.
        eprintln!("{what}, the end of its log:\n{}", log_tail(log, 40));
        bail!("{what} failed ({st}); see {}", log.display());
    }
    eprintln!("  {what}: {:.0?}", t.elapsed());
    Ok(())
}

/// The last `n` lines of a log (at most its last 64 KB).
fn log_tail(log: &Path, n: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut b = Vec::new();
    if let Ok(mut f) = std::fs::File::open(log) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if f.seek(SeekFrom::Start(len.saturating_sub(64 << 10))).is_ok() {
            f.read_to_end(&mut b).ok();
        }
    }
    let s = String::from_utf8_lossy(&b);
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
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
/// layers on the NAS (`src`), and its scenic results from its last run (`carry`). Leaves the build
/// folder ready for conversion.
#[allow(clippy::too_many_arguments)]
pub fn build_folder(u: Unit, piece: &Path, dir: &Path, cov: &Coverage, src: &crate::stage::Source, tools: &Tools, heritage: HeritageInputs, carry: Option<&crate::scache::Carry>) -> Result<Report> {
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
    rep.dem_cache = dem_cache_slice(&tools.cache, &tools.dem_units(), slice, &dir.join("dem-cache"))?;
    let mut c = Command::new("uv");
    c.current_dir(&tools.dem).args(["run", "python", "sample.py"]).arg(dir).arg("--cache").arg(dir.join("dem-cache"));
    if let Some(m) = &tools.moi_dtm {
        c.env("SCENIC_MOI_DTM", m);
    }
    if let Some(s) = &tools.sources {
        c.env("SCENIC_FABDEM_STORE", s.join("fabdem"));
    }
    run(c, "elevations (sample.py)", &log)?;
    // Its samples, kept for its later runs and its neighbours' (new ones aren't sampled twice). A
    // cache: not keeping them (the NAS away) only costs sampling them again.
    if let Err(e) = dem_samples_keep(&tools.dem_units(), u, &dir.join("dem-cache")) {
        eprintln!("unit {}: its DEM samples not kept: {e:#}", u.slash());
    }
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
    let own = format!("{},{},{},{}", tb[0], tb[1], tb[2], tb[3]);
    for step in ["prep", "canopy", "view"] {
        // The last run's results, as the canopy and view steps' previous run.
        if let (Some(c), "canopy") = (carry, step) {
            match c.restore(dir) {
                Ok(Some(n)) => eprintln!("unit {}: {n} samples' scenic results from its last run", u.slash()),
                Ok(None) => {}
                // (Without the cache's record the steps start afresh, whatever was copied.)
                Err(e) => {
                    eprintln!("unit {}: its last run's scenic results not used: {e:#}", u.slash());
                    std::fs::remove_dir_all(crate::scache::unit_dir(dir)).ok();
                }
            }
        }
        let mut c = Command::new(tools.bin.join("scenic-metrics"));
        c.arg(dir).arg(step).env("SCENIC_OWN", &own).env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", crate::scache::unit_dir(dir)).envs(tools.sources.as_ref().map(|s| ("SCENIC_CANOPY_STORE", s.join("canopy"))));
        run(c, &format!("scenic {step}"), &log)?;
    }
    if let Some(bd) = &tools.buildings {
        let mut c = Command::new(tools.bin.join("scenic-metrics"));
        c.arg(dir).arg("buildings").arg(bd).env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", crate::scache::unit_dir(dir)).envs(tools.sources.as_ref().map(|s| ("SCENIC_CANOPY_STORE", s.join("canopy"))));
        run(c, "scenic buildings", &log)?;
    }
    let mut c = Command::new(tools.bin.join("scenic-metrics"));
    c.arg(dir).arg("flags").env("SCENIC_CACHE", &tools.cache).env("SCENIC_SCACHE", crate::scache::unit_dir(dir)).envs(tools.sources.as_ref().map(|s| ("SCENIC_CANOPY_STORE", s.join("canopy"))));
    run(c, "scenic flags", &log)?;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_logs_last_lines() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("steps.log");
        std::fs::write(&log, (1..=100).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        assert_eq!(log_tail(&log, 3), "line 98\nline 99\nline 100");
        assert_eq!(log_tail(&d.path().join("none.log"), 3), "");
        // A failing step: its log's end is kept, and the error names it.
        let mut c = Command::new("sh");
        c.args(["-c", "echo why it failed; exit 3"]);
        let e = run(c, "a step", &log).unwrap_err().to_string();
        assert!(e.contains("a step failed") && log_tail(&log, 1) == "why it failed");
    }

    #[test]
    fn kept_dem_samples_named_by_their_box() {
        let d = tempfile::tempdir().unwrap();
        let k = |lon: i32, lat: i32| (((lon as i64 + (1i64 << 31)) as u64) << 32) | ((lat as i64 + (1i64 << 31)) as u64);
        let run = d.path().join("run");
        let units = d.path().join("shared").join(DEM_UNITS);
        let keep = |pts: &[(i32, i32)]| {
            std::fs::create_dir_all(&run).unwrap();
            let keys: Vec<u64> = pts.iter().map(|&(a, b)| k(a, b)).collect();
            std::fs::write(run.join("dem-cache.keys.u64"), bytemuck::cast_slice(&keys)).unwrap();
            std::fs::write(run.join("dem-cache.elev.f32"), bytemuck::cast_slice(&vec![7.0f32; pts.len()])).unwrap();
            std::fs::write(run.join("dem-cache.src.u8"), vec![1u8; pts.len()]).unwrap();
            dem_samples_keep(&units, Unit { z: 6, x: 1, y: 2 }, &run).unwrap()
        };
        let names = || {
            let mut v: Vec<String> = std::fs::read_dir(&units).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        keep(&[(-3, 1), (2, 4)]);
        assert_eq!(names(), vec![format!("6-1-2.{}.dem", box_tag([-3, 1, 2, 4]))]);
        assert_eq!(dem_file(&names()[0]), Some(("6-1-2".to_string(), Some([-3, 1, 2, 4]))));
        // Kept again with another box: the earlier file goes.
        keep(&[(5, 5), (6, 6)]);
        assert_eq!(names(), vec![format!("6-1-2.{}.dem", box_tag([5, 5, 6, 6]))]);
        let slice = |b: [i32; 4]| dem_cache_slice(&d.path().join("no-seed"), &units, b, &d.path().join("s")).unwrap();
        assert_eq!(slice([0, 0, 10, 10]), 2);
        assert_eq!(slice([-10, -10, 0, 0]), 0, "a box away from it");
        // A file that isn't whole is passed over, not an error.
        std::fs::write(units.join(format!("6-1-3.{}.dem", box_tag([0, 0, 9, 9]))), b"RDDEM002 cut").unwrap();
        assert_eq!(slice([0, 0, 10, 10]), 2);
        // What a Mac kept in its own cache (named without a box) moves to the shared one, named by its
        // box; its scenic results too.
        let local = d.path().join("local");
        std::fs::create_dir_all(local.join(DEM_UNITS)).unwrap();
        std::fs::copy(units.join(format!("6-1-2.{}.dem", box_tag([5, 5, 6, 6]))), local.join(DEM_UNITS).join("6-9-9.dem")).unwrap();
        std::fs::create_dir_all(local.join("scenic-units/6-9-9")).unwrap();
        std::fs::write(local.join("scenic-units/6-9-9/basis.json"), b"{}").unwrap();
        assert_eq!(move_kept_to_shared(&local, &d.path().join("shared")).unwrap(), 2);
        assert!(names().contains(&format!("6-9-9.{}.dem", box_tag([5, 5, 6, 6]))));
        assert!(d.path().join("shared/scenic-units/6-9-9/basis.json").exists());
        assert!(!local.join(DEM_UNITS).join("6-9-9.dem").exists() && !local.join("scenic-units/6-9-9").exists());
    }

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
        let n = dem_cache_slice(d.path(), &d.path().join(DEM_UNITS), [-5, -5, 10, 10], &d.path().join("s")).unwrap();
        // (0,0), (3,3), (5,-1)
        assert_eq!(n, 3);
        let got: Vec<u64> = bytemuck::pod_collect_to_vec(&std::fs::read(d.path().join("s/dem-cache.keys.u64")).unwrap());
        assert_eq!(got, vec![k(0, 0), k(3, 3), k(5, -1)]);
        // No cache: an empty slice, and no files (sample.py can't map an empty one).
        assert_eq!(dem_cache_slice(&d.path().join("none"), &d.path().join("none").join(DEM_UNITS), [0, 0, 1, 1], &d.path().join("t")).unwrap(), 0);
        assert!(!d.path().join("t/dem-cache.keys.u64").exists());
        // A unit's kept samples: in the slices of boxes meeting them, over the seed's.
        let run = d.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let mine = [k(3, 3), k(4, 4)];
        std::fs::write(run.join("dem-cache.keys.u64"), bytemuck::cast_slice(&mine)).unwrap();
        std::fs::write(run.join("dem-cache.elev.f32"), bytemuck::cast_slice(&[30.0f32, 40.0])).unwrap();
        std::fs::write(run.join("dem-cache.src.u8"), [1u8, 1]).unwrap();
        assert_eq!(dem_samples_keep(&d.path().join(DEM_UNITS), Unit { z: 6, x: 1, y: 2 }, &run).unwrap(), 2);
        assert_eq!(dem_cache_slice(d.path(), &d.path().join(DEM_UNITS), [-5, -5, 10, 10], &d.path().join("u")).unwrap(), 4);
        let got: Vec<u64> = bytemuck::pod_collect_to_vec(&std::fs::read(d.path().join("u/dem-cache.keys.u64")).unwrap());
        let el: Vec<f32> = bytemuck::pod_collect_to_vec(&std::fs::read(d.path().join("u/dem-cache.elev.f32")).unwrap());
        assert_eq!(got, vec![k(0, 0), k(3, 3), k(4, 4), k(5, -1)]);
        assert_eq!(el[1], 30.0, "the unit's sample, not the seed's");
        // A box away from them: the seed's only.
        assert_eq!(dem_cache_slice(d.path(), &d.path().join(DEM_UNITS), [-15, -15, -5, 10], &d.path().join("v")).unwrap(), 2);
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
