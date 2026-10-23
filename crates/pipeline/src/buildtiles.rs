//! Roadside buildings (docs/steps.md §6): the declared buildings source's release (its
//! `version`: crate::inputs::sources, the `buildings` role, Overture's), as bounding boxes, in z8
//! tiles by their centre: `sources/buildings/<release>/8/<x>-<y>.f32` (the release's dot a dash: logical names have none), f32 `[xmin, ymin, xmax, ymax]` per building,
//! degrees, sorted, each once (no file for a tile without any), with their index
//! `sources/buildings/<release>/index`, written last.
//!
//! One worldwide job makes them, once per release (`build`): `scan` reads the release's bbox
//! columns from Overture's bucket (each file's footer, then its row groups' bbox chunks alone), its
//! files in parallel, into local parts by tile, which are then merged per tile onto the NAS. A unit
//! reads the tiles near its roads (`tiles_for`, `stage`).

use crate::legacy::TileId;
use crate::out::Out;
use crate::reach::Reach;
use anyhow::{Context, Result};
use det::Det;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The release the tests' records name (the staged declaration's).
#[cfg(test)]
pub const TEST_RELEASE: &str = "2026-09-23.1";
/// The tiles' zoom.
pub const ZOOM: u8 = 8;
/// How far past a unit's roads its buildings are read (km): a building counts within 80 m of the
/// road, and is kept by its centre's cell (`buildings`, ~165 m), a cell or so from a sample's.
const MARGIN_KM: f64 = 1.0;
/// The release's files scanned at once: ~350 MB each in flight (8 peaked at 2.8 GB on the Mac
/// mini, 23 Oct, task #579), 512 files in ~30 min.
pub const WORKERS: usize = 8;
/// The free space the scan's local parts need (the release's boxes, ~40 GB).
#[cfg(not(target_os = "wasi"))]
const PARTS_ROOM: u64 = 45 << 30;

/// Release `release` as the NAS's names have it ("2026-09-23-1").
fn release_tag(release: &str) -> String {
    release.replace('.', "-")
}

/// Release `release`'s folder on the NAS.
pub fn dir(root: &Path, release: &str) -> PathBuf {
    root.join("sources/buildings").join(release_tag(release))
}

/// The logical name of release `release`'s index (in the build manifest once the tiles are made).
pub fn index_logical(release: &str) -> String {
    format!("sources/buildings/{}/index", release_tag(release))
}

/// Where release `release`'s tile `t` is.
pub fn tile_path(root: &Path, release: &str, t: TileId) -> PathBuf {
    dir(root, release).join(format!("{}/{}-{}.f32", t.z, t.x, t.y))
}

/// The declared buildings source's release (its `version`) in the accepted sources the records `m`
/// name (crate::inputs::sources), or why there's none.
pub fn release_of(root: &Path, m: &BTreeMap<String, String>) -> Result<String> {
    Ok(crate::inputs::sources::read(root, m)?.role(crate::inputs::sources::BUILDINGS)?.version()?.to_string())
}

/// The tiles made, with their buildings ("8/x/y": count; a tile without any isn't listed).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub fmt: u32,
    pub release: String,
    pub zoom: u8,
    pub tiles: BTreeMap<String, u64>,
    /// The index's content hash (its name's), which names the tiles' local copies (`local_tile`).
    #[serde(skip)]
    pub tag: String,
}

impl Index {
    /// The release's index as the build manifest has it (None before the buildings job made it).
    pub fn load(out: &Out, release: &str) -> Result<Option<Index>> {
        let Some(c) = out.get(&index_logical(release)) else { return Ok(None) };
        let p = out.path(c);
        let mut ix: Index = serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("{}", p.display()))?)
            .with_context(|| format!("{}", p.display()))?;
        ix.tag = c.rsplit('.').nth(1).unwrap_or("").to_string();
        Ok(Some(ix))
    }

    /// The bytes of tile `t`'s file (16 a building), when it has buildings.
    pub fn bytes(&self, t: TileId) -> Option<u64> {
        self.tiles.get(&t.slash()).map(|n| 16 * n)
    }
}

/// Tile `t`'s buildings in `cache` (`blobs/buildings/<index's hash>/<x>-<y>.f32`: the files an index
/// was made with; among the copies of the NAS's files, which room-making may take), copied from the
/// NAS whole when it isn't there and held for the job (store::cachefile): a unit's buildings step
/// reads its tiles whole, and units side by side read the same ones. None when the tile has no
/// buildings.
pub fn local_tile(root: &Path, index: &Index, t: TileId, cache: &Path) -> Result<Option<PathBuf>> {
    let Some((src, local, want)) = tile_copy(root, index, t, cache) else { return Ok(None) };
    fetch_tile(&src, &local, want)?;
    Ok(Some(local))
}

/// Where tile `t` is on the NAS, where its copy in `cache` goes, and its length (`local_tile`).
pub fn tile_copy(root: &Path, index: &Index, t: TileId, cache: &Path) -> Option<(PathBuf, PathBuf, u64)> {
    let want = index.bytes(t)?;
    Some((
        tile_path(root, &index.release, t),
        cache.join("blobs/buildings").join(&index.tag).join(format!("{}-{}.f32", t.x, t.y)),
        want,
    ))
}

/// The copy of `src` at `local`, `want` bytes, made when it isn't there whole, and held.
pub fn fetch_tile(src: &Path, local: &Path, want: u64) -> Result<()> {
    let copy = &mut |tmp: &Path| -> std::io::Result<()> {
        let n = store::sys::copy_data(src, tmp)?;
        crate::timings::count(n, 1);
        if n != want {
            return Err(std::io::Error::other(format!("{}: {n} bytes, not the index's {want}", src.display())));
        }
        Ok(())
    };
    let p = store::cachefile::hold(local, copy).with_context(|| format!("copy {}", src.display()))?;
    // (Named by its index: one of another length was cut short.)
    if std::fs::metadata(&p).map(|m| m.len()).ok() != Some(want) {
        store::cachefile::discard(&p);
        store::cachefile::hold(local, copy).with_context(|| format!("copy {}", src.display()))?;
    }
    Ok(())
}

/// The tiles unit `u` reads buildings from: those within `MARGIN_KM` of its tile grown by
/// `reach::LONG_KM`, where its ordinary ways lie, or of its own long roads, which it keeps whole
/// however far they go (`reach`; ferries have no roadside buildings).
pub fn tiles_for(u: TileId, reach: Option<&Reach>) -> Vec<TileId> {
    let mut tiles = BTreeSet::new();
    add_tiles(&mut tiles, crate::reach::near_box(u));
    for w in reach.into_iter().flat_map(|r| &r.long).filter(|w| w.owned && !w.ferry) {
        // Each stretch between two nodes: one cutting a tile's corner reads that tile too.
        let mut last: Option<[i32; 2]> = None;
        for &p in &w.verts {
            let q = last.unwrap_or(p);
            add_tiles(&mut tiles, [p[0].min(q[0]), p[1].min(q[1]), p[0].max(q[0]), p[1].max(q[1])]);
            last = Some(p);
        }
    }
    tiles.into_iter().collect()
}

/// Adds the tiles within `MARGIN_KM` of box `b` (E7) to `out`.
fn add_tiles(out: &mut BTreeSet<TileId>, b: [i32; 4]) {
    let b = crate::hipack::grow(b, MARGIN_KM);
    let n = 1i64 << ZOOM;
    let col = |lon: i32| ((lon as i64 + 1_800_000_000) * n / 3_600_000_000).clamp(0, n - 1) as u32;
    // The row holding latitude `lat` (E7): the edges run north to south.
    let edges = row_edges();
    let row = |lat: i32| (edges.partition_point(|&e| e > lat) as u32).saturating_sub(1).min(n as u32 - 1);
    for x in col(b[0])..=col(b[2]) {
        for y in row(b[3])..=row(b[1]) {
            out.insert(TileId { z: ZOOM, x, y });
        }
    }
}

/// The rows' edges (E7 latitude), north to south: row y lies between edges y + 1 and y.
fn row_edges() -> &'static [i32] {
    static EDGES: std::sync::OnceLock<Vec<i32>> = std::sync::OnceLock::new();
    EDGES.get_or_init(|| {
        let n = (1u32 << ZOOM) as f64;
        (0..=1u32 << ZOOM)
            .map(|y| {
                ((std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n)).dsinh().datan().to_degrees() * 1e7).round() as i32
            })
            .collect()
    })
}

/// Calls `f` with each box of a file of them, read in blocks.
#[cfg(not(target_os = "wasi"))]
fn each_box(path: &Path, mut f: impl FnMut([f32; 4])) -> Result<()> {
    use std::io::Read;
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    anyhow::ensure!(file.metadata()?.len() % 16 == 0, "{}: not whole boxes", path.display());
    let mut r = std::io::BufReader::with_capacity(16 << 20, file);
    let mut buf = vec![0u8; 16 << 20];
    loop {
        let mut n = 0;
        while n < buf.len() {
            match r.read(&mut buf[n..])? {
                0 => break,
                k => n += k,
            }
        }
        for c in buf[..n].chunks_exact(16) {
            let v = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
            f([v(0), v(4), v(8), v(12)]);
        }
        if n < buf.len() {
            return Ok(());
        }
    }
}

/// The z-`z` tile holding a point (Web Mercator, as the agent's tiles), as numpy computed it for
/// the scan before: the latitude clipped to Mercator's, `ln(tan φ + sec φ)`.
#[cfg(not(target_os = "wasi"))]
fn tile_xy(lon: f64, lat: f64, z: u8) -> (u64, u64) {
    let n = (1u64 << z) as f64;
    let x = ((lon + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0);
    let r = lat.clamp(-85.05112878, 85.05112878).to_radians();
    let y = ((1.0 - (r.dtan() + 1.0 / r.dcos()).dln() / std::f64::consts::PI) / 2.0 * n).floor().clamp(0.0, n - 1.0);
    (x as u64, y as u64)
}

/// One file of the release (`url`, `size` bytes) into `out`: its buildings' boxes (f32
/// `[xmin, ymin, xmax, ymax]`, degrees) by the zoom-`z` tile holding their centre (`<z>-<x>-<y>.f32`,
/// each tile's in the file's order), `.done` once all are written. Its buildings.
#[cfg(not(target_os = "wasi"))]
pub fn scan_file(http: &crate::bld::fetch::Http, url: &str, size: u64, out: &Path, z: u8) -> Result<usize> {
    use rayon::prelude::*;
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    std::fs::create_dir_all(out)?;
    let mut tail = http.get_bytes(url, Some((size.saturating_sub(65536), size - 1)))?;
    if let Some(need) = crate::bld::overture::footer_need(&tail).with_context(|| url.to_string())? {
        tail = http.get_bytes(url, Some((size - need, size - 1)))?;
    }
    let md =
        std::sync::Arc::new(crate::bld::overture::footer_meta(&tail).with_context(|| format!("{url}: its footer"))?);
    let (_, spans) = crate::bld::overture::bbox_spans(&md)?;
    let got: Vec<Result<(u64, Vec<u8>)>> =
        spans.par_iter().map(|&(s, e)| Ok((s, http.get_bytes(url, Some((s, e - 1)))?))).collect();
    let mut chunks = crate::bld::overture::Spans::new(size);
    for g in got {
        let (s, b) = g?;
        chunks.add(s, b);
    }
    let boxes = crate::bld::overture::boxes(chunks, md).with_context(|| url.to_string())?;
    let (n, count) = (1u64 << z, boxes.len());
    let mut keyed: Vec<(u64, [f32; 4])> = boxes
        .into_iter()
        .map(|b| {
            let (x, y) = tile_xy(((b[0] + b[2]) * 0.5) as f64, ((b[1] + b[3]) * 0.5) as f64, z);
            (x * n + y, b)
        })
        .collect();
    keyed.sort_by_key(|k| k.0);
    for run in keyed.chunk_by(|a, b| a.0 == b.0) {
        let (tx, ty) = (run[0].0 / n, run[0].0 % n);
        let bytes: Vec<u8> = run.iter().flat_map(|(_, b)| b.iter().flat_map(|v| v.to_le_bytes())).collect();
        std::fs::write(out.join(format!("{z}-{tx}-{ty}.f32")), bytes)?;
    }
    std::fs::write(out.join(".done"), b"")?;
    Ok(count)
}

/// Scans release `release`'s building files (the declared buildings source: `bucket`, its URL,
/// and `s3`, its name as the parts' list keeps it) into `parts/<file>/<z>-<x>-<y>.f32`, `workers`
/// files at once: a folder per file of the release (in name order), marked `.done` once written
/// (folders done are skipped), and the release's files in `parts/files.json`. An empty listing is a
/// release Overture no longer serves, or a failed listing: never an empty world.
#[cfg(not(target_os = "wasi"))]
pub fn scan(parts: &Path, z: u8, workers: usize, release: &str, bucket: &str, s3: &str) -> Result<()> {
    use crate::pyjson::Py;
    use crate::timings::{phase, Class};
    use rayon::prelude::*;
    std::fs::create_dir_all(parts)?;
    let http = crate::bld::fetch::Http::new();
    let prefix = format!("release/{release}/theme=buildings/type=building/");
    let p = phase("the release's files listed on S3", Class::Net);
    let objs = crate::bld::fetch::s3_list(&http, bucket, &prefix)?;
    drop(p);
    let files: Vec<String> = objs.iter().map(|o| format!("{s3}/{}", o.key)).collect();
    anyhow::ensure!(!files.is_empty(), "buildings: no files for release {release} ({s3}/{prefix})");
    let p = phase("the file list checked and written", Class::Disk);
    let listed = parts.join("files.json");
    if let Ok(b) = std::fs::read(&listed) {
        let was: Vec<String> = serde_json::from_slice(&b).with_context(|| listed.display().to_string())?;
        anyhow::ensure!(
            was == files,
            "buildings: release {release}'s files aren't those its parts were made from ({})",
            listed.display()
        );
    }
    let list = Py::List(files.iter().map(|f| Py::Str(f.clone())).collect());
    std::fs::write(&listed, crate::pyjson::dumps_spaced(&list))?;
    let todo: Vec<usize> = (0..files.len()).filter(|i| !parts.join(format!("{i:04}/.done")).exists()).collect();
    drop(p);
    eprintln!("buildings: {} files in release {release}, {} to scan", files.len(), todo.len());
    let done = std::sync::atomic::AtomicU64::new((files.len() - todo.len()) as u64);
    let n = std::sync::atomic::AtomicU64::new(0);
    let t0 = std::time::Instant::now();
    let _p = phase("the files' boxes scanned from S3", Class::Net);
    // (Four threads a file: a file's chunks are fetched side by side too.)
    let pool = rayon::ThreadPoolBuilder::new().num_threads(workers.max(1) * 4).build()?;
    pool.install(|| {
        todo.par_iter().with_max_len(1).try_for_each(|&i| -> Result<()> {
            let k =
                scan_file(&http, &format!("{bucket}{}", objs[i].key), objs[i].size, &parts.join(format!("{i:04}")), z)?;
            let total = n.fetch_add(k as u64, std::sync::atomic::Ordering::Relaxed) + k as u64;
            let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            eprintln!("buildings: {total} so far ({:.0} s)", t0.elapsed().as_secs_f64());
            crate::agent::jobs::report(d, files.len() as u64, "files");
            Ok(())
        })
    })
}

/// Makes the release's tiles: `scan` reads the release into
/// `scratch/parts-<release>/<file>/<z>-<x>-<y>.f32`, a file of the release at a time, each
/// marked done when written (a run cut short goes on from there); then each tile's parts are
/// merged onto the NAS (a tile already there whole from a run cut short is left), and the index
/// written last.
#[cfg(not(target_os = "wasi"))]
pub fn build(out: &mut Out, scratch: &Path, workers: usize) -> Result<()> {
    let sources = crate::inputs::sources::read(out.root(), &out.manifest)?;
    let b = sources.role(crate::inputs::sources::BUILDINGS)?;
    let release = b.version()?.to_string();
    let parts = scratch.join(format!("parts-{release}"));
    std::fs::create_dir_all(&parts)?;
    let free = crate::agent::room::disk_free(&parts)?;
    let started = std::fs::read_dir(&parts)?.flatten().filter(|e| e.path().join(".done").exists()).count();
    anyhow::ensure!(
        free >= PARTS_ROOM || started > 0,
        "the scan's parts need ~{} GB free; {} GB free",
        PARTS_ROOM >> 30,
        free >> 30
    );
    use crate::timings::{phase, Class};
    let t0 = std::time::Instant::now();
    scan(&parts, ZOOM, workers, &release, b.url()?, b.param_str("s3")?)?;
    eprintln!("buildings: the release scanned ({:.0?})", t0.elapsed());
    // Every file of the release scanned whole.
    let files: Vec<String> =
        serde_json::from_slice(&std::fs::read(parts.join("files.json")).context("the release's files (files.json)")?)?;
    let done = (0..files.len()).filter(|i| parts.join(format!("{i:04}/.done")).exists()).count();
    anyhow::ensure!(!files.is_empty() && done == files.len(), "{done} of the release's {} files scanned", files.len());
    // Each tile's parts, from every file of the release.
    let p = phase("the scan's parts listed", Class::Disk);
    let mut by_tile: BTreeMap<TileId, Vec<PathBuf>> = BTreeMap::new();
    for d in std::fs::read_dir(&parts)? {
        let d = d?.path();
        if !d.is_dir() {
            continue;
        }
        anyhow::ensure!(d.join(".done").exists(), "{}: not scanned whole", d.display());
        for f in std::fs::read_dir(&d)? {
            let f = f?.path();
            let Some(t) = f.file_stem().and_then(|s| s.to_str()).and_then(TileId::parse) else { continue };
            by_tile.entry(t).or_default().push(f);
        }
    }
    drop(p);
    let total = by_tile.len() as u64;
    // (Each tile's parts read and sorted, then written to the NAS and read back: a phase each,
    // over the tiles.)
    let mut index = Index { fmt: 1, release: release.clone(), zoom: ZOOM, tiles: BTreeMap::new(), tag: String::new() };
    let t1 = std::time::Instant::now();
    for (k, (t, files)) in by_tile.iter().enumerate() {
        let p = phase("each tile's parts read and sorted", Class::Disk);
        let mut v: Vec<[f32; 4]> = Vec::new();
        for f in files {
            each_box(f, |b| v.push(b))?;
        }
        let v = canonical(v);
        let bytes: &[u8] = bytemuck::cast_slice(&v);
        drop(p);
        let p = phase("tiles written to the NAS and read back", Class::NasWrite);
        let dest = tile_path(out.root(), &release, *t);
        // (Written whole by a run cut short: as it is.)
        if std::fs::metadata(&dest).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
            std::fs::create_dir_all(dest.parent().context("tile folder")?)?;
            let tmp = dest.with_extension(format!("f32.{}.tmp", std::process::id()));
            std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
            // Read back, as every write to the NAS is.
            let back = std::fs::read(&tmp).with_context(|| format!("read back {}", tmp.display()))?;
            anyhow::ensure!(
                store::naming::hash16(&back) == store::naming::hash16(bytes),
                "{}: read back differs",
                tmp.display()
            );
            std::fs::rename(&tmp, &dest)?;
            p.count(bytes.len() as u64, 1);
        }
        drop(p);
        index.tiles.insert(t.slash(), v.len() as u64);
        crate::agent::jobs::report(k as u64 + 1, total, "tiles");
    }
    let n: u64 = index.tiles.values().sum();
    anyhow::ensure!(n > 0, "no buildings in the release's {} files", files.len());
    eprintln!("buildings: {n} buildings in {} tiles ({:.0?})", index.tiles.len(), t1.elapsed());
    let local = scratch.join("index.json");
    std::fs::write(&local, serde_json::to_vec(&index)?)?;
    let p = phase("the index uploaded", Class::NasWrite);
    out.put_file(&index_logical(&release), "json", &local)?;
    drop(p);
    out.save()?;
    let _p = phase("the scan's parts removed", Class::Disk);
    std::fs::remove_dir_all(&parts).ok();
    Ok(())
}

/// A tile's buildings in a fixed order, and boxes that are bit for bit the same once (Overture
/// holds a few such), so the same buildings make the same file.
#[cfg(not(target_os = "wasi"))]
fn canonical(mut v: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
    let key = |b: &[f32; 4]| b.map(f32::to_bits);
    v.sort_unstable_by(|a, b| {
        a.iter().zip(b).map(|(x, y)| x.total_cmp(y)).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)
    });
    v.dedup_by(|a, b| key(a) == key(b));
    v
}

/// A folder of the buildings unit `u` reads (`scenic-metrics buildings` reads every `.f32` in
/// it): links to its tiles' files (`tiles_for` with its reach), those without buildings left out.
/// The number of tiles linked.
/// With `cache`, the links are to the tiles' copies there (`local_tile`), else to the NAS's files.
pub fn stage(
    root: &Path,
    index: &Index,
    u: TileId,
    reach: Option<&Reach>,
    dir: &Path,
    cache: Option<&Path>,
) -> Result<usize> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    let mut n = 0;
    for t in tiles_for(u, reach) {
        if !index.tiles.contains_key(&t.slash()) {
            continue;
        }
        let at = match cache {
            Some(c) => local_tile(root, index, t, c)?.context("a tile the index lists")?,
            None => tile_path(root, &index.release, t),
        };
        crate::sys::symlink(&at, &dir.join(format!("{}.f32", t.dash())))?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reach::LongWay;

    #[test]
    fn a_unit_reads_the_tiles_near_its_roads() {
        let u = TileId { z: 6, x: 32, y: 21 };
        let near = tiles_for(u, None);
        // Its tile's z8 tiles and those within 20 km (+ 1 km): the same as the tiles meeting that box.
        let b = crate::hipack::grow(crate::reach::near_box(u), MARGIN_KM);
        let deg = |v: i32| v as f64 * 1e-7;
        let want: Vec<TileId> = crate::stage::tiles_in(ZOOM, [deg(b[0]), deg(b[1]), deg(b[2]), deg(b[3])])
            .into_iter()
            .map(|(x, y)| TileId { z: ZOOM, x, y })
            .collect();
        assert_eq!(near, want);
        assert!(near.contains(&TileId { z: ZOOM, x: 128, y: 84 }) && near.len() == 36, "{near:?}");
        // Its own long road out to the next z6 tile but one reads the z8 tiles along it; a ferry,
        // or a road it doesn't own, doesn't.
        let tb = crate::hipack::tile_bounds(6, 32, 21);
        let mid = (tb[1] + tb[3]) / 2;
        let ftb = crate::hipack::tile_bounds(6, 34, 21);
        let road = |owned, ferry| LongWay { owned, ferry, verts: vec![[tb[0] + 1000, mid], [ftb[0] + 100_000, mid]] };
        let with = |w: LongWay| tiles_for(u, Some(&Reach { cells: vec![], owned: None, long: vec![w] }));
        let t = with(road(true, false));
        let along = |x: u32| t.iter().any(|v| v.x == x);
        assert!((132..=136).all(along) && !along(137), "{t:?}");
        assert_eq!(with(road(true, true)), near);
        assert_eq!(with(road(false, false)), near);
    }

    #[test]
    fn rows_as_tiles_have_them() {
        // The row of a latitude, as Unit::of_point finds it.
        for lat in [-85.0, -60.5, -0.01, 0.01, 23.4, 45.0, 64.13, 84.9] {
            let p = [100_000_000, (lat * 1e7) as i32];
            let mut s = BTreeSet::new();
            add_tiles(&mut s, [p[0], p[1], p[0], p[1]]);
            assert!(s.contains(&TileId::of_point(ZOOM, p)), "{lat}: {s:?}");
        }
    }

    #[test]
    fn a_tiles_buildings_in_order_once() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("b.f32");
        let boxes: Vec<[f32; 4]> = vec![[2.0, 45.0, 2.1, 45.1], [1.0, 45.0, 1.1, 45.1], [2.0, 45.0, 2.1, 45.1]];
        std::fs::write(&f, bytemuck::cast_slice::<[f32; 4], u8>(&boxes)).unwrap();
        let mut read = Vec::new();
        each_box(&f, |b| read.push(b)).unwrap();
        assert_eq!(read, boxes);
        assert_eq!(canonical(read), vec![[1.0, 45.0, 1.1, 45.1], [2.0, 45.0, 2.1, 45.1]]);
    }

    #[test]
    fn a_unit_links_its_tiles_with_buildings() {
        let d = tempfile::tempdir().unwrap();
        let u = TileId { z: 6, x: 32, y: 21 };
        let near = tiles_for(u, None);
        let index = Index {
            fmt: 1,
            release: TEST_RELEASE.into(),
            zoom: ZOOM,
            tiles: [(near[0].slash(), 5), (near[7].slash(), 9)].into(),
            tag: String::new(),
        };
        let staged = d.path().join("staged");
        assert_eq!(stage(d.path(), &index, u, None, &staged, None).unwrap(), 2);
        let link = std::fs::read_link(staged.join(format!("{}.f32", near[7].dash()))).unwrap();
        assert_eq!(link, tile_path(d.path(), TEST_RELEASE, near[7]));
        // With a cache: links to its copies, named by the index's hash, the same bytes.
        let mut index = index;
        index.tag = "0123456789abcdef".into();
        for (t, n) in [(near[0], 5usize), (near[7], 9)] {
            let p = tile_path(d.path(), TEST_RELEASE, t);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![t.x as u8; 16 * n]).unwrap();
        }
        let cache = d.path().join("cache");
        assert_eq!(stage(d.path(), &index, u, None, &staged, Some(&cache)).unwrap(), 2);
        let link = std::fs::read_link(staged.join(format!("{}.f32", near[7].dash()))).unwrap();
        assert_eq!(link, cache.join(format!("blobs/buildings/0123456789abcdef/{}-{}.f32", near[7].x, near[7].y)));
        assert_eq!(std::fs::read(&link).unwrap(), std::fs::read(tile_path(d.path(), TEST_RELEASE, near[7])).unwrap());
        // A copy cut short (another length than the index's) is made again; a tile on the NAS
        // that isn't the index's length is an error, not a short copy.
        std::fs::write(&link, b"short").unwrap();
        assert_eq!(local_tile(d.path(), &index, near[7], &cache).unwrap().unwrap(), link);
        assert_eq!(std::fs::metadata(&link).unwrap().len(), 16 * 9);
        std::fs::write(tile_path(d.path(), TEST_RELEASE, near[0]), b"cut").unwrap();
        std::fs::remove_file(cache.join(format!("blobs/buildings/0123456789abcdef/{}-{}.f32", near[0].x, near[0].y)))
            .unwrap();
        assert!(local_tile(d.path(), &index, near[0], &cache).is_err());
        assert!(local_tile(d.path(), &index, near[1], &cache).unwrap().is_none());
    }
}
