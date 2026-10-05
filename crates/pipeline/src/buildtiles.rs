//! Roadside buildings (docs/plan.md §6, base(U)): Overture's buildings of one pinned release, as
//! bounding boxes, in z8 tiles by their centre: `sources/buildings/<release>/8/<x>-<y>.f32` (the
//! release's dot a dash: logical names have none), f32 `[xmin, ymin, xmax, ymax]` per building,
//! degrees, sorted, each once (no file for a tile without any), with their index
//! `sources/buildings/<release>/index`, written last.
//!
//! One worldwide job makes them, once per release (`build`): `dem/buildings.py --world` scans the
//! release's bbox columns, its files in parallel, into local parts by tile, which are then merged
//! per tile onto the NAS. A unit reads the tiles near its roads (`tiles_for`, `stage`).

use det::Det;
use crate::legacy::Unit;
use crate::out::Out;
use crate::reach::Reach;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Overture's release the buildings come from.
pub const RELEASE: &str = "2026-09-23.1";
/// The tiles' zoom.
pub const ZOOM: u8 = 8;
/// How far past a unit's roads its buildings are read (km): a building counts within 80 m of the
/// road, and is kept by its centre's cell (`buildings`, ~165 m), a cell or so from a sample's.
const MARGIN_KM: f64 = 1.0;
/// The free space the scan's local parts need (the release's boxes, ~40 GB).
const PARTS_ROOM: u64 = 45 << 30;

/// The release as the NAS's names have it ("2026-09-23-1").
fn release_tag() -> String {
    RELEASE.replace('.', "-")
}

/// The release's folder on the NAS.
pub fn dir(root: &Path) -> PathBuf {
    root.join("sources/buildings").join(release_tag())
}

/// The logical name of the release's index (in the build manifest once the tiles are all made).
pub fn index_logical() -> String {
    format!("sources/buildings/{}/index", release_tag())
}

/// Where tile `t`'s buildings are.
pub fn tile_path(root: &Path, t: Unit) -> PathBuf {
    dir(root).join(format!("{}/{}-{}.f32", t.z, t.x, t.y))
}

/// The tiles made, with their buildings ("8/x/y": count; a tile without any isn't listed).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub fmt: u32,
    pub release: String,
    pub zoom: u8,
    pub tiles: BTreeMap<String, u64>,
}

impl Index {
    /// The release's index as the build manifest has it (None before the buildings job made it).
    pub fn load(out: &Out) -> Result<Option<Index>> {
        let Some(c) = out.get(&index_logical()) else { return Ok(None) };
        let p = out.path(c);
        Ok(Some(serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("{}", p.display()))?).with_context(|| format!("{}", p.display()))?))
    }
}

/// The tiles unit `u` reads buildings from: those within `MARGIN_KM` of its tile grown by
/// `reach::LONG_KM`, where its ordinary ways lie, or of its own long roads, which it keeps whole
/// however far they go (`reach`; ferries have no roadside buildings).
pub fn tiles_for(u: Unit, reach: Option<&Reach>) -> Vec<Unit> {
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
fn add_tiles(out: &mut BTreeSet<Unit>, b: [i32; 4]) {
    let b = crate::hipack::grow(b, MARGIN_KM);
    let n = 1i64 << ZOOM;
    let col = |lon: i32| ((lon as i64 + 1_800_000_000) * n / 3_600_000_000).clamp(0, n - 1) as u32;
    // The row holding latitude `lat` (E7): the edges run north to south.
    let edges = row_edges();
    let row = |lat: i32| (edges.partition_point(|&e| e > lat) as u32).saturating_sub(1).min(n as u32 - 1);
    for x in col(b[0])..=col(b[2]) {
        for y in row(b[3])..=row(b[1]) {
            out.insert(Unit { z: ZOOM, x, y });
        }
    }
}

/// The rows' edges (E7 latitude), north to south: row y lies between edges y + 1 and y.
fn row_edges() -> &'static [i32] {
    static EDGES: std::sync::OnceLock<Vec<i32>> = std::sync::OnceLock::new();
    EDGES.get_or_init(|| {
        let n = (1u32 << ZOOM) as f64;
        (0..=1u32 << ZOOM).map(|y| ((std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n)).dsinh().datan().to_degrees() * 1e7).round() as i32).collect()
    })
}

/// Calls `f` with each box of a file of them, read in blocks.
fn each_box(path: &Path, mut f: impl FnMut([f32; 4])) -> Result<()> {
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

/// Makes the release's tiles: `dem/buildings.py --world` (run with uv in `dem`) scans the release
/// into `scratch/parts-<release>/<file>/<z>-<x>-<y>.f32`, a file of the release at a time, each
/// marked done when written (a run cut short goes on from there); then each tile's parts are
/// merged onto the NAS (a tile already there whole from a run cut short is left), and the index
/// written last.
pub fn build(out: &mut Out, dem: &Path, scratch: &Path, workers: usize) -> Result<()> {
    let parts = scratch.join(format!("parts-{RELEASE}"));
    std::fs::create_dir_all(&parts)?;
    let free = crate::agent::room::disk_free(&parts)?;
    let started = std::fs::read_dir(&parts)?.flatten().filter(|e| e.path().join(".done").exists()).count();
    anyhow::ensure!(free >= PARTS_ROOM || started > 0, "the scan's parts need ~{} GB free; {} GB free", PARTS_ROOM >> 30, free >> 30);
    let t0 = std::time::Instant::now();
    let st = std::process::Command::new("uv")
        .current_dir(dem)
        .args(["run", "python", "buildings.py", "--world"])
        .arg(&parts)
        .args(["--zoom", &ZOOM.to_string(), "--workers", &workers.to_string(), "--release", RELEASE])
        .status()
        .context("run buildings.py")?;
    anyhow::ensure!(st.success(), "buildings.py: {st}");
    eprintln!("buildings: the release scanned ({:.0?})", t0.elapsed());
    // Every file of the release scanned whole.
    let files: Vec<String> = serde_json::from_slice(&std::fs::read(parts.join("files.json")).context("the release's files (files.json)")?)?;
    let done = (0..files.len()).filter(|i| parts.join(format!("{i:04}/.done")).exists()).count();
    anyhow::ensure!(!files.is_empty() && done == files.len(), "{done} of the release's {} files scanned", files.len());
    // Each tile's parts, from every file of the release.
    let mut by_tile: BTreeMap<Unit, Vec<PathBuf>> = BTreeMap::new();
    for d in std::fs::read_dir(&parts)? {
        let d = d?.path();
        if !d.is_dir() {
            continue;
        }
        anyhow::ensure!(d.join(".done").exists(), "{}: not scanned whole", d.display());
        for f in std::fs::read_dir(&d)? {
            let f = f?.path();
            let Some(t) = f.file_stem().and_then(|s| s.to_str()).and_then(Unit::parse) else { continue };
            by_tile.entry(t).or_default().push(f);
        }
    }
    let total = by_tile.len() as u64;
    let mut index = Index { fmt: 1, release: RELEASE.into(), zoom: ZOOM, tiles: BTreeMap::new() };
    let t1 = std::time::Instant::now();
    for (k, (t, files)) in by_tile.iter().enumerate() {
        let mut v: Vec<[f32; 4]> = Vec::new();
        for f in files {
            each_box(f, |b| v.push(b))?;
        }
        let v = canonical(v);
        let bytes: &[u8] = bytemuck::cast_slice(&v);
        let dest = tile_path(out.root(), *t);
        // (Written whole by a run cut short: as it is.)
        if std::fs::metadata(&dest).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
            std::fs::create_dir_all(dest.parent().context("tile folder")?)?;
            let tmp = dest.with_extension(format!("f32.{}.tmp", std::process::id()));
            std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
            // Read back, as every write to the NAS is.
            let back = std::fs::read(&tmp).with_context(|| format!("read back {}", tmp.display()))?;
            anyhow::ensure!(store::naming::hash16(&back) == store::naming::hash16(bytes), "{}: read back differs", tmp.display());
            std::fs::rename(&tmp, &dest)?;
        }
        index.tiles.insert(t.slash(), v.len() as u64);
        crate::agent::jobs::report(k as u64 + 1, total, "tiles");
    }
    let n: u64 = index.tiles.values().sum();
    anyhow::ensure!(n > 0, "no buildings in the release's {} files", files.len());
    eprintln!("buildings: {n} buildings in {} tiles ({:.0?})", index.tiles.len(), t1.elapsed());
    let local = scratch.join("index.json");
    std::fs::write(&local, serde_json::to_vec(&index)?)?;
    out.put_file(&index_logical(), "json", &local)?;
    out.save()?;
    std::fs::remove_dir_all(&parts).ok();
    Ok(())
}

/// A tile's buildings in a fixed order, and boxes that are bit for bit the same once (Overture
/// holds a few such), so the same buildings make the same file.
fn canonical(mut v: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
    let key = |b: &[f32; 4]| b.map(f32::to_bits);
    v.sort_unstable_by(|a, b| a.iter().zip(b).map(|(x, y)| x.total_cmp(y)).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal));
    v.dedup_by(|a, b| key(a) == key(b));
    v
}

/// A folder of the buildings unit `u` reads (`scenic-metrics buildings` reads every `.f32` in
/// it): links to its tiles' files (`tiles_for` with its reach), those without buildings left out.
/// The number of tiles linked.
pub fn stage(root: &Path, index: &Index, u: Unit, reach: Option<&Reach>, dir: &Path) -> Result<usize> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    let mut n = 0;
    for t in tiles_for(u, reach) {
        if !index.tiles.contains_key(&t.slash()) {
            continue;
        }
        crate::sys::symlink(&tile_path(root, t), &dir.join(format!("{}.f32", t.dash())))?;
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
        let u = Unit { z: 6, x: 32, y: 21 };
        let near = tiles_for(u, None);
        // Its tile's z8 tiles and those within 20 km (+ 1 km): the same as the tiles meeting that box.
        let b = crate::hipack::grow(crate::reach::near_box(u), MARGIN_KM);
        let deg = |v: i32| v as f64 * 1e-7;
        let want: Vec<Unit> = crate::stage::tiles_in(ZOOM, [deg(b[0]), deg(b[1]), deg(b[2]), deg(b[3])]).into_iter().map(|(x, y)| Unit { z: ZOOM, x, y }).collect();
        assert_eq!(near, want);
        assert!(near.contains(&Unit { z: ZOOM, x: 128, y: 84 }) && near.len() == 36, "{near:?}");
        // Its own long road out to the next z6 tile but one reads the z8 tiles along it; a ferry,
        // or a road it doesn't own, doesn't.
        let tb = crate::hipack::tile_bounds(6, 32, 21);
        let mid = (tb[1] + tb[3]) / 2;
        let ftb = crate::hipack::tile_bounds(6, 34, 21);
        let road = |owned, ferry| LongWay { owned, ferry, verts: vec![[tb[0] + 1000, mid], [ftb[0] + 100_000, mid]] };
        let with = |w: LongWay| tiles_for(u, Some(&Reach { owned: None, long: vec![w] }));
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
            assert!(s.contains(&Unit::of_point(ZOOM, p)), "{lat}: {s:?}");
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
        let u = Unit { z: 6, x: 32, y: 21 };
        let near = tiles_for(u, None);
        let index = Index { fmt: 1, release: RELEASE.into(), zoom: ZOOM, tiles: [(near[0].slash(), 5), (near[7].slash(), 9)].into() };
        let staged = d.path().join("staged");
        assert_eq!(stage(d.path(), &index, u, None, &staged).unwrap(), 2);
        let link = std::fs::read_link(staged.join(format!("{}.f32", near[7].dash()))).unwrap();
        assert_eq!(link, tile_path(d.path(), near[7]));
    }
}
