//! base(U) (docs/plan.md §6): one unit's base pack, from its OSM piece, by the unit's programs run on
//! a unit folder (`extract`, `elev`, `areaflags`, `tile … elev`, `scenic-metrics`), then its arrays as
//! a base pack (`legacy::base_sections`) for the ways the unit owns (first
//! vertex inside it) that touch the coverage, with the pass's worldwide road values.
//!
//! Before the expensive steps the folder is cut down to the ways that touch the coverage (whoever
//! owns them: a way just outside still gives the clean-up its junction context at shared nodes).
//! The global-source layers come from the catalog's packs (`stage`). Each unit reads a slice of the
//! per-vertex DEM cache: the seed (copied once from the NAS's `sources/dem-cache/`)
//! and the samples every unit kept from its last run (`dem_samples_keep`), so a vertex is sampled
//! from the DEM servers once.

use crate::coverage::Coverage;
use crate::legacy::Unit;
use anyhow::{bail, ensure, Context, Result};
use roadcore::WayRec;
use std::collections::BTreeMap;
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
pub const DEM_UNITS: &str = "dem-units";
const DEM_MAGIC: &[u8; 8] = b"RDDEM002";
const DEM_HEAD: usize = 48;

/// The DEM rules' versions the seed was sampled under: the first of each.
const SEED_DEM_VERSIONS: [u32; 4] = [1, 1, 1, 1];

fn current_dem_versions() -> [u32; 4] {
    crate::rules::DEM_RULES.map(crate::rules::version)
}

/// Keeps a unit's DEM samples (the `dem-cache.*` elev left in `from`: its vertices, cached or
/// sampled anew) for later runs of it and of its neighbours. Returns how many.
pub fn dem_samples_keep(dir: &Path, u: Unit, from: &Path) -> Result<usize> {
    dem_samples_keep_with(dir, u, from, None)
}

/// `dem_samples_keep`, a copy of the file kept in `copies` too (`keep_dem_copy`), which its
/// neighbours' slices on this Mac read instead of the NAS's.
pub fn dem_samples_keep_with(dir: &Path, u: Unit, from: &Path, copies: Option<&Path>) -> Result<usize> {
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
    if let Some(c) = copies {
        keep_dem_copy(&dir.join(&name), c, &f);
    }
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
/// (`Tools::shared`), once: both Macs' units read them there. Two unit jobs on one Mac may do this
/// at once (crate::agent, Two jobs at once): what the other moved first is passed over.
pub fn move_kept_to_shared(cache: &Path, shared: &Path) -> Result<usize> {
    let gone = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    let mut moved = 0;
    let local = cache.join(DEM_UNITS);
    for e in std::fs::read_dir(&local).into_iter().flatten().flatten() {
        let p = e.path();
        let Some((u, _)) = dem_file(&e.file_name().to_string_lossy()) else { continue };
        let Ok(m) = roadcore::mmap(&p) else { continue };
        let Some(bb) = dem_head(&m).map(|h| h.1) else { continue };
        drop(m);
        std::fs::create_dir_all(shared.join(DEM_UNITS))?;
        match crate::whole::copy(&p, &shared.join(DEM_UNITS).join(format!("{u}.{}.dem", box_tag(bb)))) {
            // (Moved by the other meanwhile.)
            Err(_) if !p.exists() => continue,
            r => r?,
        };
        match std::fs::remove_file(&p) {
            Err(e) if gone(&e) => continue,
            r => r?,
        }
        moved += 1;
    }
    let local = cache.join("scenic-units");
    for e in std::fs::read_dir(&local).into_iter().flatten().flatten() {
        let (from, to) = (e.path(), shared.join("scenic-units").join(e.file_name()));
        if !from.is_dir() || crate::whole::is_tmp(&from) {
            continue;
        }
        if !to.exists() {
            // (Named by this process too: another on this Mac may be copying the same folder.)
            let tmp = to.with_extension(format!("{}-{}.tmp", crate::agent::cond::host(), std::process::id()));
            std::fs::remove_dir_all(&tmp).ok();
            std::fs::create_dir_all(&tmp)?;
            let files = match std::fs::read_dir(&from) {
                Ok(rd) => rd,
                Err(e) if gone(&e) => {
                    std::fs::remove_dir_all(&tmp).ok();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            for f in files.flatten() {
                crate::whole::copy(&f.path(), &tmp.join(f.file_name()))?;
            }
            if let Err(e) = std::fs::rename(&tmp, &to) {
                std::fs::remove_dir_all(&tmp).ok();
                if !to.exists() {
                    return Err(e.into());
                }
            }
        }
        match std::fs::remove_dir_all(&from) {
            Err(e) if gone(&e) => continue,
            r => r?,
        }
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

/// The part of a units' DEM samples file (`dem_samples_keep`) whose longitudes are within box `b`'s:
/// its box and versions, and those entries (its keys sorted, lon first, so a run of them), with the
/// bytes read for them.
struct DemRange {
    bx: [i32; 4],
    made: [u32; 4],
    keys: Vec<u64>,
    elev: Vec<f32>,
    srcs: Vec<u8>,
    read: u64,
    /// The file's entries (all of them read: `whole`).
    n: u64,
}

impl DemRange {
    /// Whether every entry of the file was read.
    fn whole(&self) -> bool {
        self.keys.len() as u64 == self.n
    }

    /// The file's bytes, when it was read whole (`dem_samples_keep`'s layout).
    fn bytes(&self) -> Option<Vec<u8>> {
        self.whole().then(|| {
            let mut f = Vec::with_capacity(DEM_HEAD + 13 * self.keys.len());
            f.extend_from_slice(DEM_MAGIC);
            f.extend_from_slice(&self.n.to_le_bytes());
            for v in self.bx {
                f.extend_from_slice(&v.to_le_bytes());
            }
            for v in self.made {
                f.extend_from_slice(&v.to_le_bytes());
            }
            f.extend_from_slice(bytemuck::cast_slice(&self.keys));
            f.extend_from_slice(bytemuck::cast_slice(&self.elev));
            f.extend_from_slice(&self.srcs);
            f
        })
    }
}

/// This Mac's copy of units' DEM samples file `p` (on the NAS: `<name>`, replaced whole by each of
/// its unit's runs) in `copies`, named by the file's name, length and modification time, so a copy
/// is of that file as it is: `<name>.<length>.<mtime ns>`.
fn dem_copy_name(p: &Path, copies: &Path) -> Option<PathBuf> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(copies.join(format!("{}.{}.{t}", p.file_name()?.to_string_lossy(), m.len())))
}

/// Keeps `bytes`, units' DEM samples file `p` as written or read whole, as this Mac's copy of it in
/// `copies` (`dem_copy_name`), and lets the unit's earlier copies go: a cache, the copies of the
/// NAS's files', so failing costs only reading it again.
fn keep_dem_copy(p: &Path, copies: &Path, bytes: &[u8]) {
    let Some(at) = dem_copy_name(p, copies) else { return };
    if store::cachefile::create_bytes(&at, bytes).is_ok() {
        store::cachefile::release(&at);
    }
    let Some(unit) = p.file_name().map(|n| n.to_string_lossy().split('.').next().unwrap_or("").to_string()) else { return };
    for e in std::fs::read_dir(copies).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.split('.').next() == Some(unit.as_str()) && e.path() != at {
            store::cachefile::try_remove(&e.path());
        }
    }
}

/// A units' DEM samples file read for box `b`: its header, the bounds of the entries within `b`'s
/// longitudes found by a binary search over its keys (read a page at a time), then those entries
/// alone, from each of its three arrays. None when it isn't whole (its length not its header's).
/// On the NAS a file is read in a few large reads where reading it whole moved hundreds of MB that
/// the box doesn't need (a unit's neighbours to the east and west).
fn dem_range(p: &Path, b: [i32; 4]) -> std::io::Result<Option<DemRange>> {
    use store::sys::PosIo;
    let f = std::fs::File::open(p)?;
    let len = f.metadata()?.len();
    let mut head = [0u8; DEM_HEAD];
    if len < DEM_HEAD as u64 {
        return Ok(None);
    }
    f.read_exact_at(&mut head, 0)?;
    if &head[..8] != DEM_MAGIC {
        return Ok(None);
    }
    let n = u64::from_le_bytes(head[8..16].try_into().unwrap());
    if len != DEM_HEAD as u64 + 13 * n {
        return Ok(None);
    }
    let i32_at = |i: usize| i32::from_le_bytes(head[i..i + 4].try_into().unwrap());
    let u32_at = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().unwrap());
    let bx = [i32_at(16), i32_at(20), i32_at(24), i32_at(28)];
    let made = [u32_at(32), u32_at(36), u32_at(40), u32_at(44)];
    let mut read = DEM_HEAD as u64;
    // (Pages of keys, 64 KB: a search's last steps fall in one.)
    const PAGE: u64 = 8192;
    let mut pages: std::collections::HashMap<u64, Vec<u64>> = Default::default();
    let mut key = |i: u64| -> std::io::Result<u64> {
        let pg = i / PAGE;
        if !pages.contains_key(&pg) {
            let (from, to) = (pg * PAGE, ((pg + 1) * PAGE).min(n));
            let mut buf = vec![0u8; 8 * (to - from) as usize];
            f.read_exact_at(&mut buf, DEM_HEAD as u64 + 8 * from)?;
            read += buf.len() as u64;
            pages.insert(pg, bytemuck::pod_collect_to_vec(&buf));
        }
        Ok(pages[&pg][(i % PAGE) as usize])
    };
    // The first entry whose key isn't below `k`.
    let mut first = |k: u64| -> std::io::Result<u64> {
        let (mut lo, mut hi) = (0u64, n);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if key(mid)? < k {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    };
    let lo = first(dem_key(b[0], i32::MIN))?;
    let hi = match dem_key(b[2], i32::MAX).checked_add(1) {
        Some(k) => first(k)?,
        None => n,
    };
    drop(first);
    drop(key);
    let m = hi.saturating_sub(lo);
    let mut take = |at: u64, size: u64| -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0u8; (size * m) as usize];
        f.read_exact_at(&mut buf, at + size * lo)?;
        read += buf.len() as u64;
        Ok(buf)
    };
    let keys = bytemuck::pod_collect_to_vec(&take(DEM_HEAD as u64, 8)?);
    let elev = bytemuck::pod_collect_to_vec(&take(DEM_HEAD as u64 + 8 * n, 4)?);
    let srcs = take(DEM_HEAD as u64 + 12 * n, 1)?;
    Ok(Some(DemRange { bx, made, keys, elev, srcs, read, n }))
}

/// The entries of sorted DEM cache arrays inside `b` still valid: those whose DEM rules (by their
/// source and place, `rules::dem_rules_of`) have the versions they were sampled under (`made`) now;
/// the rest are left out, so elev samples them again under the changed rule.
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
/// elev): the seed's (`cache`'s `dem-cache.*`), then every unit's kept samples
/// in `units` whose box meets `b` (`dem_samples_keep`), which win over the seed's (they're newer); entries
/// sampled under a DEM rule's earlier version are left out. With none, nothing is written (elev
/// samples every vertex). The files near `b` are found by the boxes in their names (one listing);
/// a unit's newest file counts, and one that isn't whole is passed over (it's only a cache).
pub fn dem_cache_slice(cache: &Path, units_dir: &Path, b: [i32; 4], dst: &Path) -> Result<usize> {
    dem_cache_slice_with(cache, units_dir, b, dst, None)
}

/// `dem_cache_slice`, the files read whole kept in `copies` and read from there while they're as
/// they are on the NAS (`keep_dem_copy`): a unit's own file and its neighbours' above and below it
/// are read whole by each of them.
pub fn dem_cache_slice_with(cache: &Path, units_dir: &Path, b: [i32; 4], dst: &Path, copies: Option<&Path>) -> Result<usize> {
    std::fs::create_dir_all(dst)?;
    for n in ["keys.u64", "elev.f32", "src.u8"] {
        std::fs::remove_file(dst.join(format!("dem-cache.{n}"))).ok();
    }
    use crate::timings::{sub, Class};
    let p = sub("the seed's samples", Class::Disk);
    let mut all: Vec<(u64, f32, u8)> = Vec::new();
    let open = |n: &str| roadcore::mmap(&cache.join(format!("dem-cache.{n}")));
    if let (Ok(km), Ok(em), Ok(sm)) = (open("keys.u64"), open("elev.f32"), open("src.u8")) {
        let (keys, elev): (&[u64], &[f32]) = (bytemuck::cast_slice(&km[..]), bytemuck::cast_slice(&em[..]));
        ensure!(keys.len() == elev.len() && keys.len() == sm.len(), "DEM cache files out of step");
        dem_valid_in_box(keys, elev, &sm[..], b, SEED_DEM_VERSIONS, &mut all);
    }
    let seed = all.len();
    drop(p);
    let reading = sub("units' kept samples read", Class::NasRead);
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
        // the job. Only the box's longitudes: `dem_range`.)
        // (This Mac's copy of it as it is, when there's one: `keep_dem_copy`.)
        let copy = copies.and_then(|c| dem_copy_name(p, c)).and_then(|c| store::cachefile::hold_existing(&c).ok().flatten());
        let got = match &copy {
            Some(c) => dem_range(c, b),
            None => dem_range(p, b),
        };
        match got {
            Ok(Some(r)) => {
                if copy.is_none() {
                    reading.count(r.read, 1);
                    if let (Some(c), Some(bytes)) = (copies, r.bytes()) {
                        keep_dem_copy(p, c, &bytes);
                    }
                }
                if !meets(r.bx) {
                    continue;
                }
                dem_valid_in_box(&r.keys, &r.elev, &r.srcs, b, r.made, &mut newer);
            }
            Ok(None) => eprintln!("  DEM cache: {} isn't whole; passed over", p.display()),
            Err(_) => eprintln!("  DEM cache: {} can't be read now; passed over", p.display()),
        }
    }
    drop(reading);
    let _p = sub("merged and written", Class::Disk);
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

/// Puts the DEM cache's seed in `cache` when it isn't there whole, from the NAS's copy
/// (`sources/dem-cache/`), and holds it (store::cachefile: room-making may delete it while no job
/// holds it, the NAS having it whole; it's copied again then). Without one the units sample every
/// vertex anew.
pub fn dem_seed(root: &Path, cache: &Path) -> Result<()> {
    let names = ["keys.u64", "elev.f32", "src.u8"];
    let len = |p: PathBuf| std::fs::metadata(p).map(|m| m.len()).ok();
    let whole = |l: &[Option<u64>]| matches!(l, [Some(k), Some(e), Some(s)] if *k == 8 * *s && *e == 4 * *s);
    let local = |n: &str| cache.join(format!("dem-cache.{n}"));
    // (This Mac's, whole: held as it is.)
    let held: Vec<Option<PathBuf>> = names.iter().map(|n| store::cachefile::hold_existing(&local(n)).ok().flatten()).collect();
    if whole(&names.map(|n| len(local(n)))) && held.iter().all(Option::is_some) {
        return Ok(());
    }
    let src = root.join("sources/dem-cache");
    let there: Vec<Option<u64>> = names.iter().map(|n| len(src.join(format!("dem-cache.{n}")))).collect();
    if !whole(&there) {
        eprintln!("unit: no DEM cache to start from on the NAS ({}); sampling every vertex anew", src.display());
        return Ok(());
    }
    let t = std::time::Instant::now();
    // (Some 9 GB: its progress said, MB by MB.)
    let total: u64 = there.iter().flatten().sum();
    let mut before = 0u64;
    let mut copied = false;
    // Each file whole before the next (one here of another length, from an older seed, goes first:
    // its fellows are copied again too). Made under this process's own temporary name: two unit
    // jobs on a Mac without the seed copy it at once, the first named wins.
    for attempt in 0..2 {
        for (n, want) in names.iter().zip(&there) {
            let (from, to) = (src.join(format!("dem-cache.{n}")), local(n));
            store::cachefile::hold(&to, &mut |tmp| {
                copied = true;
                crate::osmpass::copy_resume_with(&from, tmp, &mut |d, _| crate::agent::jobs::report((before + d) >> 20, total >> 20, "MB of the DEM cache copied here (once a Mac)")).map_err(std::io::Error::other)
            })
            .with_context(|| format!("copy {}", from.display()))?;
            before += want.unwrap_or(0);
        }
        if names.iter().map(|n| len(local(n))).collect::<Vec<_>>() == there {
            break;
        }
        anyhow::ensure!(attempt == 0, "the DEM cache here isn't the NAS's after it was copied again");
        for n in names {
            store::cachefile::discard(&local(n));
        }
    }
    if copied {
        eprintln!("unit: DEM cache copied from the NAS ({:.0?})", t.elapsed());
    }
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
    /// This Mac's caches: `chm10/` (canopy 10° files) and `dem-cache.*` (the seed's per-vertex
    /// elevations); and the units' kept results when there's no `shared`.
    pub cache: PathBuf,
    /// Overture building boxes: a folder of `.f32` files (the unit's tiles staged by
    /// `buildtiles::stage`), when there are any.
    pub buildings: Option<PathBuf>,
    /// The NAS's `sources/`, where downloads are kept, each downloaded once: Meta's canopy squares
    /// (`canopy/`) and FABDEM's tiles (`fabdem/`); the local caches fill from it. None: local
    /// caches alone.
    pub sources: Option<PathBuf>,
    /// The NAS's `cache/`, where what a unit keeps for its later runs is shared by both Macs: its DEM
    /// samples (`dem-units/`) and scenic results (`scenic-units/`). None: the local cache.
    pub shared: Option<PathBuf>,
    /// The canopy squares read where they lie (the NAS's `sources/canopy/`), for a worker that
    /// keeps no copies of them (a task's); None: this Mac's copies (`cache/chm10/`), from the NAS.
    pub chm: Option<PathBuf>,
    /// The NAS's stores (`sources/`) only read: a task's worker reads them where they lie, and
    /// neither downloads into them nor removes from them (docs/workers.md §2); its programs are
    /// told so (`SCENIC_STORES_READ_ONLY`).
    pub stores_read_only: bool,
    /// Densification spacing (m).
    pub spacing_m: u32,
    /// Where to keep a copy of the folder before and after each of the steps' programs, with the
    /// command that ran it (`<n> <step> before|after` and `….cmd`): `scenic-build unit-snap`, for
    /// running the steps again elsewhere (natively at other thread counts, as WebAssembly) and
    /// comparing bytes (tools/check/same.py). Copies are APFS clones: no space until they differ.
    pub snap: Option<PathBuf>,
}

impl Tools {
    /// The units' kept DEM samples (`Tools::shared`, else the local cache).
    pub fn dem_units(&self) -> PathBuf {
        self.shared.as_ref().unwrap_or(&self.cache).join(DEM_UNITS)
    }

    /// Where this Mac keeps copies of the shared units' DEM samples it read or wrote whole (among the
    /// copies of the NAS's files, which room-making may take: `blobs/dem-units/`); none when they're
    /// this Mac's own (no shared cache).
    pub fn dem_copies(&self) -> Option<PathBuf> {
        self.shared.as_ref().map(|_| self.cache.join("blobs").join(DEM_UNITS))
    }

    /// Where unit `u`'s scenic results are kept between its runs (crate::scache::Carry).
    pub fn scenic_kept(&self, u: Unit) -> PathBuf {
        self.shared.as_ref().unwrap_or(&self.cache).join("scenic-units").join(u.dash())
    }
}

/// Where a unit's build says how far it is (`on_stage`): each stage it finishes (a lap, a program)
/// with the time it took, and a program's own progress within its stage (its `progress:` lines),
/// as (stage, fraction of it done, its time when finished).
pub type StageHook = Box<dyn Fn(&str, f64, Option<std::time::Duration>) + Send + Sync>;

static HOOK: std::sync::RwLock<Option<StageHook>> = std::sync::RwLock::new(None);

/// Has a unit's stages said to `h` from now on (None: to nothing): the unit job's progress
/// (scenic-build).
pub fn on_stage(h: Option<StageHook>) {
    if let Ok(mut w) = HOOK.write() {
        *w = h;
    }
}

/// Says how far a stage is (`on_stage`).
pub fn stage_said(stage: &str, frac: f64, took: Option<std::time::Duration>) {
    if let Ok(r) = HOOK.read() {
        if let Some(h) = r.as_ref() {
            h(stage, frac, took);
        }
    }
}

/// A unit's stages in the order they run (`Laps::lap`'s and the programs' names), each with about
/// how long it takes (seconds: the build Mac's, over its areas of 2026-10): how far an area is, by
/// the stages it's through (`Areas`), until this Mac has timed its own.
pub const STAGES: [(&str, f64); 25] = [
    ("piece copied from the NAS", 10.0),
    ("buildings staged", 1.0),
    ("extract", 10.0),
    ("subset to the coverage", 1.0),
    ("DEM cache slice", 30.0),
    ("elevations (elev)", 150.0),
    ("DEM samples kept", 40.0),
    ("layers staged from the packs", 60.0),
    ("heritage inputs", 8.0),
    ("area flags (areaflags)", 5.0),
    ("land cover (landcover)", 4.0),
    ("scenic results restored", 20.0),
    ("clean-up and grade (tile elev)", 1.0),
    ("scenic prep", 1.0),
    ("scenic canopy", 25.0),
    ("scenic view", 12.0),
    ("scenic buildings", 19.0),
    ("scenic flags", 1.0),
    ("missing grids written", 2.0),
    ("scenic results kept", 8.0),
    ("base pack made", 1.0),
    ("base pack written to the NAS", 60.0),
    ("road values made and written", 20.0),
    ("records saved", 5.0),
    ("its folders removed", 1.0),
];

/// A unit job's progress (scenic-build's unit step), area by area: those finished, and each under
/// way by how far through its stages it is (`STAGES`), each stage weighted by about how long it
/// takes here (learned as areas finish, and kept: `file`). Its line says `<areas>/<n> areas`.
pub struct Areas {
    n: usize,
    /// The areas finished (a stage said after, such as its folders' removal, counts for nothing).
    finished: std::collections::BTreeSet<String>,
    /// The areas under way (prepared here, or their last steps out with other workers): how far each is.
    under_way: BTreeMap<String, f64>,
    /// The area whose stages are said now.
    current: Option<String>,
    /// About how long each stage takes here (seconds), and those this Mac has timed (kept or now).
    secs: BTreeMap<String, f64>,
    learned: std::collections::BTreeSet<String>,
    file: Option<PathBuf>,
    said_at: Option<std::time::Instant>,
}

impl Areas {
    /// `n` areas, their stages' times learned in `file` (when there is one).
    pub fn new(n: usize, file: Option<PathBuf>) -> Areas {
        let mut secs: BTreeMap<String, f64> = STAGES.iter().map(|(s, t)| (s.to_string(), *t)).collect();
        let mut learned = std::collections::BTreeSet::new();
        if let Some(kept) = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice::<BTreeMap<String, f64>>(&b).ok()) {
            for (s, t) in kept.into_iter().filter(|(s, t)| STAGES.iter().any(|x| x.0 == s) && t.is_finite() && *t >= 0.0) {
                learned.insert(s.clone());
                secs.insert(s, t);
            }
        }
        Areas { n, finished: Default::default(), under_way: BTreeMap::new(), current: None, secs, learned, file, said_at: None }
    }

    /// About how long `stages` take here together (seconds), when this Mac has timed each.
    pub fn here_s(&self, stages: &[&str]) -> Option<f64> {
        stages.iter().map(|s| self.learned.contains(*s).then(|| self.secs.get(*s).copied()).flatten()).sum()
    }

    /// Stages said from now on are area `u`'s.
    pub fn on(&mut self, u: &str) {
        self.current = Some(u.to_string());
    }

    /// The current area's `stage` is `frac` done (1: finished, in `took`): it counts that far, and
    /// the time teaches this Mac's weight for the stage.
    pub fn stage(&mut self, stage: &str, frac: f64, took: Option<std::time::Duration>) {
        let Some(i) = STAGES.iter().position(|(s, _)| *s == stage) else { return };
        let Some(u) = self.current.clone() else { return };
        if self.finished.contains(&u) {
            return;
        }
        if let (Some(t), true) = (took, frac >= 1.0) {
            let e = self.secs.entry(stage.to_string()).or_insert(t.as_secs_f64());
            *e = 0.75 * *e + 0.25 * t.as_secs_f64();
            self.learned.insert(stage.to_string());
        }
        let w = |s: &str| self.secs.get(s).copied().unwrap_or(1.0).max(0.1);
        let total: f64 = STAGES.iter().map(|(s, _)| w(s)).sum();
        let before: f64 = STAGES[..i].iter().map(|(s, _)| w(s)).sum();
        // (Never back: a stage said again, or one an area skipped, is no step back.)
        let f = ((before + frac.clamp(0.0, 1.0) * w(stage)) / total).min(0.999);
        let e = self.under_way.entry(u).or_insert(0.0);
        *e = e.max(f);
        self.say(frac >= 1.0);
    }

    /// Area `u` is done (built and saved, or nothing to build).
    pub fn finished(&mut self, u: &str) {
        self.under_way.remove(u);
        self.finished.insert(u.to_string());
        self.say(true);
        self.keep();
    }

    /// The areas done, counting each under way by how far it is.
    pub fn done(&self) -> f64 {
        (self.finished.len() as f64 + self.under_way.values().sum::<f64>()).min(self.n as f64)
    }

    /// The progress line (`crate::agent::jobs::report_f`): at once at a stage's end, else at most
    /// once a second.
    pub fn say(&mut self, now: bool) {
        if !now && self.said_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1)) {
            return;
        }
        self.said_at = Some(std::time::Instant::now());
        crate::agent::jobs::report_f(self.done(), self.n as u64, "areas");
    }

    /// The stages' times learned, kept for this Mac's next job (a cache: not keeping them costs
    /// nothing but the weights).
    fn keep(&self) {
        if let Some(f) = &self.file {
            if let Ok(b) = serde_json::to_vec(&self.secs) {
                crate::whole::write(f, &b).ok();
            }
        }
    }
}

/// A part of a unit's build done in-process: a phase of the job's (crate::timings), logged as its
/// programs' times are, "  <what>: <time>", and said as a stage finished (`on_stage`) when it
/// `end`s. (Ended by an error instead, its time is the phase's alone.)
pub struct Stage {
    what: String,
    t: std::time::Instant,
    _p: crate::timings::Phase,
}

/// Starts stage `what` of class `class`.
pub fn stage(what: &str, class: crate::timings::Class) -> Stage {
    Stage { what: what.to_string(), t: std::time::Instant::now(), _p: crate::timings::phase(what, class) }
}

impl Stage {
    pub fn end(self) {
        let took = self.t.elapsed();
        eprintln!("  {}: {took:.0?}", self.what);
        stage_said(&self.what, 1.0, Some(took));
    }
}

/// What a unit's step program mostly waits on, for its phase (crate::timings): its elevations read
/// the DEM servers' files (and say their own phases), the land cover downloads ESA's, the canopy
/// reads the canopy squares; the rest compute over the unit's folder.
pub fn program_class(what: &str) -> crate::timings::Class {
    use crate::timings::Class;
    match what {
        "elevations (elev)" => Class::Mixed,
        "land cover (landcover)" => Class::Net,
        _ => Class::Compute,
    }
}

/// The most memory (resident, bytes) one of the steps' programs took since `take_peak` last read it.
static PEAK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The most memory one of the steps' programs took since the last call (scenic-build's log line per
/// unit), and starts again.
pub fn take_peak() -> u64 {
    PEAK.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// The folder copied (cloned) to `<snap>/<n> <what> <when>`, with the command in `….cmd` (program,
/// then `arg` and `env` lines).
fn snapshot(snap: &Path, n: usize, c: &Command, dir: &Path, what: &str, when: &str) -> Result<()> {
    let to = snap.join(format!("{n:02} {what} {when}"));
    std::fs::create_dir_all(snap)?;
    let st = Command::new("cp").arg("-c").arg("-R").arg(dir).arg(&to).status()?;
    anyhow::ensure!(st.success(), "clone {} to {}", dir.display(), to.display());
    let mut cmd = format!("prog {}\n", c.get_program().to_string_lossy());
    for a in c.get_args() {
        cmd.push_str(&format!("arg {}\n", a.to_string_lossy()));
    }
    for (k, v) in c.get_envs() {
        cmd.push_str(&format!("env {}={}\n", k.to_string_lossy(), v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    std::fs::write(snap.join(format!("{n:02} {what} {when}.cmd")), cmd)?;
    Ok(())
}

/// A step's program run in `dir` (logged to `log`), its folder snapshotted around it when
/// `tools.snap` asks.
fn run_in(c: Command, what: &str, log: &Path, dir: &Path, tools: &Tools) -> Result<()> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let Some(snap) = &tools.snap else { return run(c, what, log) };
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    snapshot(snap, n, &c, dir, what, "before")?;
    // (A copy of the command for the snapshot after: `run` takes it.)
    let mut shown = Command::new(c.get_program());
    shown.args(c.get_args());
    for (k, v) in c.get_envs() {
        if let Some(v) = v {
            shown.env(k, v);
        }
    }
    run(c, what, log)?;
    snapshot(snap, n, &shown, dir, what, "after")
}

fn run(mut c: Command, what: &str, log: &Path) -> Result<()> {
    let f = std::fs::File::options().create(true).append(true).open(log)?;
    // (Its phase, the program's own phases under it where it says them.)
    let _p = crate::timings::phase(what, program_class(what));
    crate::timings::child(&mut c);
    let t = std::time::Instant::now();
    let mut child = c.stdout(f.try_clone()?).stderr(std::process::Stdio::piped()).spawn().with_context(|| format!("start {what}"))?;
    // Its errors into its log as they come, and how far it says it is passed on as its stage's
    // (a bar's redraws only that).
    let err = child.stderr.take();
    let name = what.to_string();
    let read = std::thread::spawn(move || {
        let mut f = f;
        if let Some(err) = err {
            crate::agent::jobs::each_line(err, |l| {
                let frac = crate::agent::jobs::fraction_of(l);
                if let Some(x) = frac {
                    stage_said(&name, x, None);
                }
                if frac.is_none() || !l.trim_start().starts_with('[') {
                    std::io::Write::write_all(&mut f, format!("{l}\n").as_bytes()).ok();
                }
            });
        }
    });
    // Waited for with what it used: its (and its programs') peak memory.
    let waited = crate::sys::wait_with_peak(child).with_context(|| format!("wait for {what}"));
    read.join().ok();
    let (st, peak) = waited?;
    PEAK.fetch_max(peak, std::sync::atomic::Ordering::Relaxed);
    if !st.success() {
        // The end of its log into the job's: the unit's folder, its log with it, goes when the next
        // job starts.
        eprintln!("{what}, the end of its log:\n{}", log_tail(log, 40));
        bail!("{what} failed ({st}); see {}", log.display());
    }
    eprintln!("  {what}: {:.0?}", t.elapsed());
    stage_said(what, 1.0, Some(t.elapsed()));
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

/// Runs the unit's programs for unit `u` in `dir` from `piece`, with the coverage and the global-source
/// layers on the NAS (`src`), and its scenic results from its last run (`carry`): `prepare_folder`,
/// then its `tail`. Leaves the unit folder ready for its base pack.
#[allow(clippy::too_many_arguments)]
pub fn build_folder(u: Unit, piece: &Path, dir: &Path, cov: &Coverage, src: &crate::stage::Source, tools: &Tools, heritage: HeritageInputs, carry: Option<&crate::scache::Carry>) -> Result<Report> {
    let rep = prepare_folder(u, piece, dir, cov, src, tools, heritage, carry)?;
    if rep.kept_ways > 0 {
        run_tail(&tail(u, tools.buildings.is_some(), tools.sources.is_some()), dir, tools)?;
        keep_dem_samples(u, dir, tools);
    }
    Ok(rep)
}

/// The DEM samples a unit's elevations made (its tail's first step), kept for its later runs and
/// its neighbours' (new ones aren't sampled twice). A cache: not keeping them (the NAS away) only
/// costs sampling them again.
pub fn keep_dem_samples(u: Unit, dir: &Path, tools: &Tools) {
    if let Err(e) = dem_samples_keep_with(&tools.dem_units(), u, &dir.join("dem-cache"), tools.dem_copies().as_deref()) {
        eprintln!("unit {}: its DEM samples not kept: {e:#}", u.slash());
    }
}

/// A unit's build up to its tail (docs/workers.md: what needs the NAS): its ways from the piece,
/// those touching the coverage, the DEM cache's slice over them, the global-source layers staged
/// from the packs, the heritage inputs, area flags and land cover; then its scenic results from its
/// last run restored (`carry`), as the tail's canopy and view steps read them. (Its elevations are
/// the tail's first step: they read the DEM servers' files where they lie, so any worker may.)
#[allow(clippy::too_many_arguments)]
pub fn prepare_folder(u: Unit, piece: &Path, dir: &Path, cov: &Coverage, src: &crate::stage::Source, tools: &Tools, heritage: HeritageInputs, carry: Option<&crate::scache::Carry>) -> Result<Report> {
    std::fs::create_dir_all(dir)?;
    let log = dir.join("steps.log");
    let mut rep = Report { unit: u.slash(), ..Default::default() };
    // 1. Every way of the piece, densified.
    let mut c = Command::new(tools.bin.join("extract"));
    c.arg(dir).arg(tools.spacing_m.to_string()).arg(piece);
    run_in(c, "extract", &log, dir, tools)?;
    use crate::timings::Class;
    let s = stage("subset to the coverage", Class::Compute);
    rep.piece_ways = roadcore::Ways::open(dir)?.ways().len();
    // 2. Only what touches the coverage goes on.
    let (kw, kv) = subset(dir, |_, vs| cov.touches(vs))?;
    (rep.kept_ways, rep.kept_verts) = (kw, kv);
    if kw == 0 {
        s.end();
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
    s.end();
    let s = stage("DEM cache slice", Class::NasRead);
    // (The seed held for this unit, copied again if room-making took it: store::cachefile.)
    if let Some(root) = tools.sources.as_deref().and_then(Path::parent) {
        dem_seed(root, &tools.cache)?;
    }
    rep.dem_cache = dem_cache_slice_with(&tools.cache, &tools.dem_units(), slice, &dir.join("dem-cache"), tools.dem_copies().as_deref())?;
    s.end();
    // 4. The global-source layers the steps read, from the packs.
    let s = stage("layers staged from the packs", Class::NasRead);
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    rep.staged = crate::stage::stage(src, b, dir)?;
    s.end();
    // The heritage sites around the unit (the flags step's `heritage.json`), and the designated
    // areas rasterised onto its grid (`grid.areas.u8`), from the heritage-sites job's slices.
    let s = stage("heritage inputs", Class::NasRead);
    (rep.heritage, rep.areas) = heritage(b, dir)?;
    s.end();
    let mut c = Command::new(tools.bin.join("areaflags"));
    c.arg(dir).arg(dir.join("area-shapes.geojsonseq"));
    run_in(c, "area flags (areaflags)", &log, dir, tools)?;
    // Land cover the packs lack (new coverage): ESA WorldCover for those grid tiles only; the
    // rest stays as staged.
    if rep.staged.missing.get("class").copied().unwrap_or(0) > 0 {
        let mut c = Command::new(tools.bin.join("landcover"));
        c.arg(dir).arg("--only").arg(dir.join("grid.class.missing.u32"));
        run_in(c, "land cover (landcover)", &log, dir, tools)?;
    }
    // Its last run's scenic results (pipeline::scache: the canopy and view steps copy what's
    // unchanged), restored over the staged grids they were made from.
    if let Some(c) = carry {
        let s = stage("scenic results restored", Class::NasRead);
        // (From the copy made ahead of it, while the unit before it built: `ahead_carry`.)
        let ahead = ahead_carry(dir);
        match c.restore_ahead(dir, ahead.is_dir().then_some(ahead.as_path())) {
            Ok(Some(n)) => eprintln!("unit {}: {n} samples' scenic results from its last run", u.slash()),
            Ok(None) => {}
            // (Without the cache's record the steps start afresh, whatever was copied.)
            Err(e) => {
                eprintln!("unit {}: its last run's scenic results not used: {e:#}", u.slash());
                std::fs::remove_dir_all(crate::scache::unit_dir(dir)).ok();
            }
        }
        s.end();
    }
    Ok(rep)
}

/// Where the unit job copies the scenic results kept for the unit whose folder is `dir`, ahead of
/// it (scache::Carry::fetch): beside its folder.
pub fn ahead_carry(dir: &Path) -> PathBuf {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dir.with_file_name(format!("{name}-carry"))
}

/// One program a task runs (docs/workers.md §3): its name (the build's `bin/<prog>`, or
/// `<prog>.wasm` in a web worker), its arguments and environment, in which `{dir}` is the unit's
/// folder, `{cache}` the canopy cache, `{scache}` the folder's scenic cache, `{buildings}` its
/// roadside buildings, `{store}` the NAS's canopy store to download into, and what's read where it
/// lies: `{sources}` the NAS's sources, `{chm}` its canopy squares, `{net}`
/// the DEM servers' files (crate::offload::places; an environment variable whose value names a
/// place the worker doesn't have is left out).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    pub what: String,
    pub prog: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The files it reads of the unit's own (`{dir}/<name>`, `{buildings}/<name>`; a trailing `*`
    /// for any name so begun), earlier steps' outputs too: a task sends those there when it
    /// starts, so it may run on any worker (crate::offload). None: it stays here (`keep_here`).
    #[serde(default)]
    pub reads: Vec<String>,
}

/// The files a unit job saves for a unit (its tile's dash name: `6-31-20`): its base pack, road
/// values, roads' English and the grids its packs lacked (scenic-build's `commit_unit`). All a
/// helper's hand-off may change (crate::coord).
pub fn saved_files(dash: &str) -> [String; 6] {
    ["base", "global/roads", "global/roaden", "layers/grid-class/hi", "layers/grid-canopy/hi", "layers/grid-cover/hi"].map(|p| format!("{p}/{dash}"))
}

/// A tail split: the runs that stay here, then those any worker may run (what they read of the
/// unit's folder listed: it's sent them), the longest such end.
pub fn split(runs: &[Run]) -> (&[Run], &[Run]) {
    let i = runs.iter().rposition(|r| r.reads.is_empty()).map_or(0, |i| i + 1);
    runs.split_at(i)
}

/// The steps through `what` kept here: neither it nor those before go to another worker.
pub fn keep_here(runs: &mut [Run], what: &str) {
    if let Some(i) = runs.iter().position(|r| r.what == what) {
        for r in &mut runs[..=i] {
            r.reads.clear();
        }
    }
}

/// The 10° canopy squares the canopy step reads for grid tiles `tiles` (those of each tile's
/// corners), by (top latitude, left longitude).
pub fn canopy_squares(tiles: &[[u32; 2]]) -> std::collections::BTreeSet<(i32, i32)> {
    use det::Det;
    let mut need = std::collections::BTreeSet::new();
    for t in tiles {
        for (dx, dy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = ((t[0] + dx) as f64 * 256.0, (t[1] + dy) as f64 * 256.0);
            let lon = x / roadcore::grid::WORLD * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / roadcore::grid::WORLD)).dsinh().datan().to_degrees();
            need.insert(((lat / 10.0).ceil() as i32 * 10, (lon / 10.0).floor() as i32 * 10));
        }
    }
    need
}

/// A canopy square's files, its median, p95 and cover (named by its top and left).
pub fn canopy_square_files((top, left): (i32, i32)) -> [String; 3] {
    ["median", "p95", "cover5m"].map(|st| format!("meta_chm_lat={top}.0_lon={left}.0_{st}.tif"))
}

/// Whether the NAS's store (`<sources>/canopy/`) has every canopy square the canopy step of the
/// unit in `dir` reads (a file, or Meta's "none there"): a worker reads them where they lie, and
/// can't download one.
pub fn canopy_stored(dir: &Path, sources: &Path) -> bool {
    let Ok(idx) = roadcore::grid::GridIndex::load(dir) else { return false };
    canopy_squares(&idx.tiles).into_iter().all(|sq| canopy_square_files(sq).iter().all(|n| sources.join("canopy").join(n).exists()))
}

/// The tail of unit `u`'s build: its elevations, its clean-up and grade, then its road samples,
/// canopy, views, buildings (with `buildings`) and flags. Every program is Rust, and gives the same
/// bytes natively and as WebAssembly (tools/check/same.py, tail.mjs), so any worker can run it. The
/// data the elevations and canopy read is read where it lies: the DEM servers' files and the NAS's
/// (`{net}`, `{sources}`, `{chm}`: a browser's through the coordinator, a Mac's as this
/// Mac reads them).
pub fn tail(u: Unit, buildings: bool, store: bool) -> Vec<Run> {
    // What each step reads of the unit's folder, earlier steps' outputs too: a task sends what's
    // there when it starts (traced in WebAssembly, the outputs compared: tools/check/tail.mjs).
    let mine = |names: &[&str]| names.iter().map(|n| if n.starts_with('{') { n.to_string() } else { format!("{{dir}}/{n}") }).collect::<Vec<_>>();
    let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
    let own = format!("{},{},{},{}", tb[0], tb[1], tb[2], tb[3]);
    let scenic = |step: &str, with_own: bool| {
        let mut e = Vec::new();
        if with_own {
            e.push(("SCENIC_OWN".to_string(), own.clone()));
        }
        e.push(("SCENIC_CACHE".to_string(), "{cache}".to_string()));
        e.push(("SCENIC_SCACHE".to_string(), "{scache}".to_string()));
        if store {
            e.push(("SCENIC_CANOPY_STORE".to_string(), "{store}".to_string()));
        }
        // (The canopy squares read where they lie: the NAS's, for a worker that has no copies.)
        if step == "canopy" {
            e.push(("SCENIC_CHM".to_string(), "{chm}".to_string()));
        }
        let mut args = vec!["{dir}".to_string(), step.to_string()];
        if step == "buildings" {
            args.push("{buildings}".into());
        }
        let reads = match step {
            "prep" => mine(&["ways.bin", "verts.bin", "final.u16", "terrain.tiles"]),
            // (Its last run's results, restored: scache::Carry.)
            "canopy" => mine(&["grid.idx", "grid.terrain.i16", "samples.bin", "scache/canopy*", "scache/changed.tiles", "near.i8", "roadside.u8", "grid.canopy.u8", "grid.cover.u8"]),
            "view" => mine(&["grid.idx", "grid.terrain.i16", "grid.canopy.u8", "grid.class.u8", "grid.areas.u8", "samples.bin", "near.i8", "roadside.u8", "pois.json", "heritage.json", "ways.bin", "verts.bin", "scache/view*", "scache/changed.tiles", "samples.metrics.u8"]),
            "buildings" => mine(&["samples.bin", "ways.bin", "verts.bin", "{buildings}/*"]),
            "flags" => mine(&["grid.idx", "grid.terrain.i16", "grid.areas.u8", "samples.bin", "samples.metrics.u8", "samples.bld.u8", "pois.json", "heritage.json", "ways.bin", "verts.bin"]),
            _ => Vec::new(),
        };
        Run { what: format!("scenic {step}"), prog: "scenic-metrics".into(), args, env: e, reads }
    };
    let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    let elev = Run {
        what: "elevations (elev)".into(),
        prog: "elev".into(),
        args: vec!["{dir}".into(), "--cache".into(), "{dir}/dem-cache".into()],
        env: env(&[("SCENIC_FABDEM_STORE", "{sources}/fabdem"), ("SCENIC_FETCH_MIRROR", "{net}")]),
        reads: mine(&["verts.bin", "dem-cache/*"]),
    };
    let grade = Run { what: "clean-up and grade (tile elev)".into(), prog: "tile".into(), args: vec!["{dir}".into(), "elev".into()], env: Vec::new(), reads: mine(&["ways.bin", "verts.bin", "elev.f32", "strings.txt"]) };
    let mut runs = vec![elev, grade];
    for step in ["prep", "canopy", "view"] {
        runs.push(scenic(step, true));
    }
    if buildings {
        runs.push(scenic("buildings", false));
    }
    runs.push(scenic("flags", false));
    runs
}

/// Runs a task's programs here, natively, in `dir` (snapshotted around each when `tools.snap`
/// asks).
pub fn run_tail(runs: &[Run], dir: &Path, tools: &Tools) -> Result<()> {
    let log = dir.join("steps.log");
    let scache = crate::scache::unit_dir(dir);
    let store = tools.sources.as_ref().filter(|_| !tools.stores_read_only).map(|s| s.join("canopy"));
    // (A place this worker hasn't, or that no worker here fills, `{net}`: its variable left out.)
    let fill = |v: &str| -> Option<String> {
        let put = |s: &str, k: &str, p: Option<&Path>| -> Option<String> { if s.contains(k) { Some(s.replace(k, &p?.to_string_lossy())) } else { Some(s.to_string()) } };
        let v = put(v, "{dir}", Some(dir))?;
        let v = put(&v, "{cache}", Some(&tools.cache))?;
        let v = put(&v, "{scache}", Some(&scache))?;
        let v = put(&v, "{buildings}", tools.buildings.as_deref())?;
        let v = put(&v, "{sources}", tools.sources.as_deref())?;
        let v = put(&v, "{chm}", tools.chm.as_deref())?;
        let v = put(&v, "{store}", store.as_deref())?;
        (!v.contains("{net}")).then_some(v)
    };
    for r in runs {
        let mut c = Command::new(tools.bin.join(&r.prog));
        for a in &r.args {
            c.arg(fill(a).with_context(|| format!("{}: no place for {a}", r.what))?);
        }
        for (k, v) in &r.env {
            if let Some(v) = fill(v) {
                c.env(k, v);
            }
        }
        if tools.stores_read_only {
            c.env("SCENIC_STORES_READ_ONLY", "1");
        }
        run_in(c, &r.what, &log, dir, tools)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_counts_by_the_stages_it_is_through() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("unit-stages.json");
        let mut a = Areas::new(4, Some(file.clone()));
        let total: f64 = STAGES.iter().map(|s| s.1).sum();
        a.on("6/1/1");
        a.stage("piece copied from the NAS", 1.0, None);
        assert!((a.done() - 10.0 / total).abs() < 1e-9);
        // Halfway through its elevations: the stages before, and half of that one.
        a.stage("elevations (elev)", 0.5, None);
        let before: f64 = STAGES[..5].iter().map(|s| s.1).sum();
        assert!((a.done() - (before + 75.0) / total).abs() < 1e-9);
        // A stage said again, or an earlier one, is no step back; one it doesn't know, nothing.
        a.stage("extract", 1.0, None);
        a.stage("something else", 1.0, None);
        assert!((a.done() - (before + 75.0) / total).abs() < 1e-9);
        // Its last steps out with another worker while the next area builds: both count.
        a.on("6/1/2");
        a.stage("extract", 1.0, None);
        let two = a.done();
        assert!(two > (before + 75.0) / total);
        a.on("6/1/1");
        a.finished("6/1/1");
        // (Its folders removed after: nothing more.)
        a.stage("its folders removed", 1.0, None);
        assert!((a.done() - (1.0 + (STAGES[..3].iter().map(|s| s.1).sum::<f64>()) / total)).abs() < 1e-9);
        // A stage's time teaches its weight, kept for the next job.
        a.on("6/1/2");
        a.stage("DEM cache slice", 1.0, Some(std::time::Duration::from_secs(70)));
        a.finished("6/1/2");
        let kept: BTreeMap<String, f64> = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(kept["DEM cache slice"], 0.75 * 30.0 + 0.25 * 70.0);
        assert_eq!(Areas::new(4, Some(file.clone())).secs["DEM cache slice"], 40.0);
        assert_eq!(a.done(), 2.0);
        // How long stages take here, once this Mac has timed each (kept or now): a tail's patience.
        assert_eq!(a.here_s(&["DEM cache slice"]), Some(40.0));
        assert_eq!(a.here_s(&["DEM cache slice", "scenic view"]), None);
        assert_eq!(Areas::new(4, Some(file)).here_s(&["DEM cache slice"]), Some(40.0));
    }

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
    fn a_whole_tail_may_run_anywhere_but_the_canopy_squares_a_worker_cant_have() {
        let u = Unit::parse("6/20/22").unwrap();
        let what = |runs: &[Run]| runs.iter().map(|r| r.what.clone()).collect::<Vec<_>>();
        let mut runs = tail(u, true, true);
        let (here, anywhere) = split(&runs);
        assert!(here.is_empty());
        assert_eq!(what(anywhere), ["elevations (elev)", "clean-up and grade (tile elev)", "scenic prep", "scenic canopy", "scenic view", "scenic buildings", "scenic flags"]);
        assert!(anywhere.iter().all(|r| r.reads.iter().all(|p| p.starts_with("{dir}/") || p.starts_with("{buildings}/"))));
        // A canopy square the NAS's store lacks: the steps through the canopy stay here.
        keep_here(&mut runs, "scenic canopy");
        let (here, anywhere) = split(&runs);
        assert_eq!(what(here), ["elevations (elev)", "clean-up and grade (tile elev)", "scenic prep", "scenic canopy"]);
        assert_eq!(what(anywhere), ["scenic view", "scenic buildings", "scenic flags"]);
        // Without the roadside buildings, flags still follows view.
        let runs = tail(u, false, false);
        assert_eq!(split(&runs).1.len(), 6);
    }

    #[test]
    fn the_canopy_squares_a_unit_reads_are_known_from_its_grid() {
        let d = tempfile::tempdir().unwrap();
        // Two grid tiles (z11) either side of 50°N at 7°E, and one at the antimeridian's west.
        let z11 = |lon: f64, lat: f64| {
            let (gx, gy) = roadcore::grid::cell_of(lon, lat);
            [(gx / 256.0) as u32, (gy / 256.0) as u32]
        };
        let tiles = vec![z11(7.0, 50.05), z11(7.0, 49.95), z11(-179.99, 0.5)];
        assert_eq!(canopy_squares(&tiles).into_iter().collect::<Vec<_>>(), [(10, -180), (60, 0), (50, 0)].into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>());
        roadcore::grid::GridIndex::new(tiles).save(&d.path().join("grid.idx")).unwrap();
        let sources = d.path().join("sources");
        std::fs::create_dir_all(sources.join("canopy")).unwrap();
        assert!(!canopy_stored(d.path(), &sources));
        for sq in canopy_squares(&roadcore::grid::GridIndex::load(d.path()).unwrap().tiles) {
            for n in canopy_square_files(sq) {
                assert!(!canopy_stored(d.path(), &sources));
                // (A file, or Meta's "none there", empty.)
                std::fs::write(sources.join("canopy").join(n), b"").unwrap();
            }
        }
        assert!(canopy_stored(d.path(), &sources));
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
        store::sys::copy_data(units.join(format!("6-1-2.{}.dem", box_tag([5, 5, 6, 6]))), local.join(DEM_UNITS).join("6-9-9.dem")).unwrap();
        std::fs::create_dir_all(local.join("scenic-units/6-9-9")).unwrap();
        std::fs::write(local.join("scenic-units/6-9-9/basis.json"), b"{}").unwrap();
        assert_eq!(move_kept_to_shared(&local, &d.path().join("shared")).unwrap(), 2);
        assert!(names().contains(&format!("6-9-9.{}.dem", box_tag([5, 5, 6, 6]))));
        assert!(d.path().join("shared/scenic-units/6-9-9/basis.json").exists());
        assert!(!local.join(DEM_UNITS).join("6-9-9.dem").exists() && !local.join("scenic-units/6-9-9").exists());
    }

    /// Room-making mid-job (docs/plan.md §8, store::cachefile): the DEM seed, deleted as fast as it
    /// can be while a unit job slices it, is copied again from the NAS and held for each unit: every
    /// slice is what it is left alone.
    #[test]
    fn a_seed_deleted_mid_job_is_copied_again_and_every_slice_is_alike() {
        // (No network.)
        crate::fetch::go_offline();
        let d = tempfile::tempdir().unwrap();
        let (root, cache) = (d.path().join("nas"), d.path().join("cache"));
        let src = root.join("sources/dem-cache");
        std::fs::create_dir_all(&src).unwrap();
        let k = |lon: i32, lat: i32| (((lon as i64 + (1i64 << 31)) as u64) << 32) | ((lat as i64 + (1i64 << 31)) as u64);
        let mut keys: Vec<u64> = (-40..40).flat_map(|a| (-40..40).map(move |b| k(a * 3, b * 3))).collect();
        keys.sort_unstable();
        let elev: Vec<f32> = (0..keys.len()).map(|i| i as f32 * 0.5).collect();
        std::fs::write(src.join("dem-cache.keys.u64"), bytemuck::cast_slice(&keys)).unwrap();
        std::fs::write(src.join("dem-cache.elev.f32"), bytemuck::cast_slice(&elev)).unwrap();
        std::fs::write(src.join("dem-cache.src.u8"), vec![4u8; keys.len()]).unwrap();
        let seed: Vec<PathBuf> = ["keys.u64", "elev.f32", "src.u8"].iter().map(|n| cache.join(format!("dem-cache.{n}"))).collect();
        let boxes: Vec<[i32; 4]> = (0..60).map(|i| [-100 + i, -90 + i, 10 + i, 20 + i]).collect();
        let job = |name: &str| -> Vec<Vec<u8>> {
            let mut v = Vec::new();
            for (i, b) in boxes.iter().enumerate() {
                dem_seed(&root, &cache).unwrap();
                let dst = d.path().join(format!("{name}/{i}"));
                dem_cache_slice(&cache, &d.path().join(DEM_UNITS), *b, &dst).unwrap();
                v.push(["keys.u64", "elev.f32", "src.u8"].iter().flat_map(|n| std::fs::read(dst.join(format!("dem-cache.{n}"))).unwrap()).collect());
                // (The unit done: its hold let go, as the job's loop does.)
                seed.iter().for_each(|p| store::cachefile::release(p));
            }
            v
        };
        let calm = job("calm");
        let stop = std::sync::atomic::AtomicBool::new(false);
        let (busy, freed) = std::thread::scope(|s| {
            let deleter = s.spawn(|| {
                let mut n = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    n += store::cachefile::try_remove_all(&seed).unwrap_or(0) + store::cachefile::remove_tree(&cache).freed;
                }
                n
            });
            // (A failure lets the deleter go too: the scope waits for it.)
            let busy = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job("busy")));
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            (busy.unwrap_or_else(|e| std::panic::resume_unwind(e)), deleter.join().unwrap())
        });
        assert!(freed > 0, "the deleter took nothing");
        assert_eq!(busy, calm);
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
        // No cache: an empty slice, and no files (elev can't map an empty one).
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
    fn a_kept_file_read_by_its_range_is_its_whole_read() {
        let d = tempfile::tempdir().unwrap();
        let run = d.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        // (Points in clumps, many on one longitude, over several pages of keys.)
        let mut s = 1u64;
        let mut r = |m: i64| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as i64 % m) as i32
        };
        for n in [0usize, 1, 5, 8191, 8192, 8193, 40_000] {
            let mut keys: Vec<u64> = (0..n).map(|_| dem_key(r(2000) - 1000 + if r(3) == 0 { 0 } else { r(50) }, r(2000) - 1000)).collect();
            keys.sort_unstable();
            keys.dedup();
            let m = keys.len();
            std::fs::write(run.join("dem-cache.keys.u64"), bytemuck::cast_slice(&keys)).unwrap();
            std::fs::write(run.join("dem-cache.elev.f32"), bytemuck::cast_slice(&(0..m).map(|i| i as f32 * 0.5).collect::<Vec<_>>())).unwrap();
            std::fs::write(run.join("dem-cache.src.u8"), (0..m).map(|i| (i % 7) as u8).collect::<Vec<_>>()).unwrap();
            let units = d.path().join(format!("u{n}"));
            dem_samples_keep(&units, Unit { z: 6, x: 1, y: 2 }, &run).unwrap();
            let file = std::fs::read_dir(&units).unwrap().flatten().next().map(|e| e.path());
            let Some(file) = file else { continue };
            let whole = std::fs::read(&file).unwrap();
            let (wn, wbx, wmade) = dem_head(&whole).unwrap();
            for _ in 0..50 {
                let (w, e) = (r(2400) - 1200, r(2400) - 1200);
                let (s, nn) = (r(2400) - 1200, r(2400) - 1200);
                let b = [w.min(e), s.min(nn), w.max(e), s.max(nn)];
                for b in [b, [i32::MIN, i32::MIN, i32::MAX, i32::MAX], [0, 0, 0, 0]] {
                    let mut want = Vec::new();
                    let kk: Vec<u64> = bytemuck::pod_collect_to_vec(&whole[DEM_HEAD..DEM_HEAD + 8 * wn]);
                    let ee: Vec<f32> = bytemuck::pod_collect_to_vec(&whole[DEM_HEAD + 8 * wn..DEM_HEAD + 12 * wn]);
                    dem_valid_in_box(&kk, &ee, &whole[DEM_HEAD + 12 * wn..], b, wmade, &mut want);
                    let got = dem_range(&file, b).unwrap().unwrap();
                    assert_eq!((got.bx, got.made), (wbx, wmade));
                    let mut have = Vec::new();
                    dem_valid_in_box(&got.keys, &got.elev, &got.srcs, b, got.made, &mut have);
                    assert_eq!(have, want, "{n} points, box {b:?}");
                    // (The search's pages of keys at most again.)
                    assert!(got.read <= whole.len() as u64 + 8 * wn as u64);
                }
            }
        }
        // Not whole: None.
        let p = d.path().join("cut.dem");
        let f = std::fs::read_dir(d.path().join("u40000")).unwrap().flatten().next().unwrap().path();
        let b = std::fs::read(&f).unwrap();
        std::fs::write(&p, &b[..b.len() - 1]).unwrap();
        assert!(dem_range(&p, [0, 0, 1, 1]).unwrap().is_none());
    }

    #[test]
    fn kept_samples_read_whole_are_copied_here_and_read_from_the_copy_while_it_is_the_file() {
        let d = tempfile::tempdir().unwrap();
        let (run, units, copies) = (d.path().join("run"), d.path().join("shared").join(DEM_UNITS), d.path().join("cache/blobs").join(DEM_UNITS));
        std::fs::create_dir_all(&run).unwrap();
        let keep = |pts: &[(i32, i32)], elev: f32, copies: Option<&Path>| {
            let keys: Vec<u64> = pts.iter().map(|&(a, b)| dem_key(a, b)).collect();
            std::fs::write(run.join("dem-cache.keys.u64"), bytemuck::cast_slice(&keys)).unwrap();
            std::fs::write(run.join("dem-cache.elev.f32"), bytemuck::cast_slice(&vec![elev; pts.len()])).unwrap();
            std::fs::write(run.join("dem-cache.src.u8"), vec![1u8; pts.len()]).unwrap();
            dem_samples_keep_with(&units, Unit { z: 6, x: 1, y: 2 }, &run, copies).unwrap()
        };
        let slice = |b: [i32; 4], to: &str, copies: Option<&Path>| {
            let n = dem_cache_slice_with(&d.path().join("no-seed"), &units, b, &d.path().join(to), copies).unwrap();
            let el: Vec<f32> = std::fs::read(d.path().join(to).join("dem-cache.elev.f32")).map(|b| bytemuck::pod_collect_to_vec(&b)).unwrap_or_default();
            (n, el)
        };
        let copied = || std::fs::read_dir(&copies).map(|r| r.count()).unwrap_or(0);
        // Kept elsewhere: read whole from the NAS, then copied here; a slice of a few longitudes
        // isn't.
        keep(&[(1, 1), (5, 5), (9, 9)], 7.0, None);
        assert_eq!(slice([4, 0, 6, 10], "a", Some(&copies)), (1, vec![7.0]));
        assert_eq!(copied(), 0);
        assert_eq!(slice([0, 0, 10, 10], "b", Some(&copies)), (3, vec![7.0; 3]));
        assert_eq!(copied(), 1);
        let copy = std::fs::read_dir(&copies).unwrap().next().unwrap().unwrap().path();
        let nas = std::fs::read_dir(&units).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&nas).unwrap());
        // Read from the copy while it's the file (a copy changed here shows it was read)...
        let mut b = std::fs::read(&copy).unwrap();
        b[DEM_HEAD + 8 * 3..DEM_HEAD + 12 * 3].copy_from_slice(bytemuck::cast_slice(&[8.0f32; 3]));
        std::fs::write(&copy, &b).unwrap();
        assert_eq!(slice([0, 0, 10, 10], "c", Some(&copies)), (3, vec![8.0; 3]));
        assert_eq!(slice([0, 0, 10, 10], "d", None), (3, vec![7.0; 3]));
        // ...not once the unit kept other samples (a new file); kept here, the copy is it, and the
        // earlier one goes (once no job holds it: the unit job lets its files go unit by unit).
        store::cachefile::release(&copy);
        std::thread::sleep(std::time::Duration::from_millis(20));
        keep(&[(1, 1), (5, 5), (9, 9), (9, 10)], 9.0, Some(&copies));
        assert_eq!(copied(), 1);
        assert_eq!(slice([0, 0, 10, 10], "e", Some(&copies)), (4, vec![9.0; 4]));
        let nas = std::fs::read_dir(&units).unwrap().next().unwrap().unwrap().path();
        let copy = std::fs::read_dir(&copies).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&nas).unwrap());
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
