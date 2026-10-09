//! A z8 area of `bldtiles T` as a task (docs/buildings3d.md §3.6, docs/workers.md §3): its files cut
//! from T's and its neighbours' work files where the job runs, the program `bldtile` (natively, or
//! as WebAssembly in a page) making the area's tiles from them, and the job taking them into T's
//! pack in the area's turn.
//!
//! A task's folder (`u/` on a worker; docs/formats.md):
//! - `6-<x>-<y>.sect`: of T's and its 8 neighbours' work files, those with a block the area reads
//!   (`job::area_blocks`: its own, and those within the margin around it), each a work file of the
//!   same format and meta holding only those blocks, their bytes as stored (not compressed again);
//! - `coverage.sect`: the coverage's shapes that can answer for a point the area asks about (any of
//!   its blocks' records' centroids and vertices, and the blocks' boxes), in the recipes' order,
//!   each whole with its buffer and country: a building's shape is then the same shape (another
//!   index), so its country and the first-match order are the same.
//!
//! What the program writes there: `area.tiles` (an RDTILES archive of the area's z12–14 tiles, as
//! `job::area_tiles` makes them) and `area.json` (its `job::Summary`). The job's own run of an area
//! (one no worker finished, or a worker's result checked) writes the same two files the same way,
//! from the whole work files: a worker's are compared with them byte for byte.

use super::job::{self, AddTile, Summary};
use super::work::{self, WorkFile};
use crate::coverage::{Coverage, Shape};
use crate::legacy::Unit;
use anyhow::{ensure, Context, Result};
use roadcore::archive::{Archive, ArchiveWriter};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The coverage's file in a task's folder.
pub const COVERAGE: &str = "coverage.sect";
/// What the program writes: the area's tiles, and what it made.
pub const TILES: &str = "area.tiles";
pub const SUMMARY: &str = "area.json";
/// The task's kind and its program.
pub const KIND: &str = "bldtile";

/// A work file's name in a task's folder.
fn file_name(x: i64, y: i64) -> String {
    format!("6-{x}-{y}.sect")
}

/// What a cut wrote: its files' bytes, and the area's own buildings and parts (its blocks' counts).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cut {
    pub bytes: u64,
    pub records: u64,
}

impl Cut {
    /// A task's predicted memory (MB): its files three times over (a page holds them, the program
    /// reads them in, and room as much again), and the model of `bldtiles` (§3.6: 0.25 GB and
    /// 280 B a building of the area).
    pub fn mem_mb(&self) -> u64 {
        (self.bytes >> 20) * 3 + 256 + ((self.records * 280) >> 20)
    }
}

/// Writes area `a`'s task files (of T, `files` its and its neighbours' work files as
/// `job::work_files` has them) into `dir`.
pub fn cut(files: &[Option<WorkFile>], cov: &Coverage, t: Unit, a: (u32, u32), dir: &Path) -> Result<Cut> {
    ensure!(files.len() == 9, "T and its 8 neighbours");
    std::fs::create_dir_all(dir)?;
    let (list, n_own) = job::area_blocks(files, t, a);
    let records = list[..n_own].iter().map(|(_, e)| e.count as u64).sum();
    // Each file's blocks, by key; and the box of every point the area asks the coverage about.
    let mut by_file: BTreeMap<usize, Vec<work::IndexEntry>> = BTreeMap::new();
    let mut ext = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    let mut grow = |p: [i32; 2]| ext = [ext[0].min(p[0]), ext[1].min(p[1]), ext[2].max(p[0]), ext[3].max(p[1])];
    for (fi, e) in &list {
        by_file.entry(*fi).or_default().push(*e);
        let b = job::box14(e.key);
        grow([b[0], b[1]]);
        grow([b[2], b[3]]);
        let blk = files[*fi].as_ref().unwrap().block(e)?;
        for p in blk.cen.iter().chain(&blk.verts) {
            grow(*p);
        }
    }
    let mut bytes = 0u64;
    for (fi, mut es) in by_file {
        let f = files[fi].as_ref().unwrap();
        es.sort_unstable_by_key(|e| e.key);
        let (dx, dy) = ((fi % 3) as i64 - 1, (fi / 3) as i64 - 1);
        let path = dir.join(file_name(t.x as i64 + dx, t.y as i64 + dy));
        work::write(&path, &f.meta, &mut es.iter().map(|e| Ok((e.key, f.stored(e)?, e.count))))?;
        bytes += std::fs::metadata(&path)?.len();
    }
    let path = dir.join(COVERAGE);
    let kept: Vec<&Shape> = cov.shapes.iter().filter(|s| s.bbox[0] <= ext[2] && s.bbox[2] >= ext[0] && s.bbox[1] <= ext[3] && s.bbox[3] >= ext[1]).collect();
    write_coverage(&path, &kept)?;
    bytes += std::fs::metadata(&path)?.len();
    Ok(Cut { bytes, records })
}

/// Writes shapes as a task's coverage (RDSECT: meta `{"fmt": 1, "shapes": [{source, country,
/// buffer_m, rings: [vertices a ring]}]}`, section `verts` the rings' vertices, E7).
fn write_coverage(path: &Path, shapes: &[&Shape]) -> Result<()> {
    let meta: Vec<serde_json::Value> = shapes.iter().map(|s| serde_json::json!({ "source": s.source, "country": s.country, "buffer_m": s.buffer_m, "rings": s.rings.iter().map(Vec::len).collect::<Vec<_>>() })).collect();
    let mut w = store::sect::SectWriter::create(path, serde_json::json!({ "fmt": 1, "shapes": meta }))?;
    let verts: Vec<[i32; 2]> = shapes.iter().flat_map(|s| s.rings.iter().flatten().copied()).collect();
    w.add_pod("verts", &verts)?;
    w.finish()?;
    Ok(())
}

/// A task's coverage (`write_coverage`'s), its shapes made as the whole coverage's were.
pub fn read_coverage(path: &Path) -> Result<Coverage> {
    let f = store::range::PlainFile::open(path).with_context(|| format!("open {}", path.display()))?;
    let r = store::sect::SectReader::open(f).with_context(|| format!("read {}", path.display()))?;
    #[derive(serde::Deserialize)]
    struct S {
        source: String,
        country: String,
        buffer_m: f64,
        rings: Vec<usize>,
    }
    #[derive(serde::Deserialize)]
    struct M {
        fmt: u32,
        shapes: Vec<S>,
    }
    let m: M = serde_json::from_value(r.meta().clone()).context("a task's coverage's meta")?;
    ensure!(m.fmt == 1, "a task's coverage fmt {} (this reads 1)", m.fmt);
    let verts: Vec<[i32; 2]> = r.read_pod("verts")?;
    ensure!(m.shapes.iter().flat_map(|s| &s.rings).sum::<usize>() == verts.len(), "a task's coverage's vertices don't add up");
    let mut at = 0usize;
    let mut shapes = Vec::with_capacity(m.shapes.len());
    for s in m.shapes {
        let rings = s.rings.iter().map(|&n| {
            at += n;
            verts[at - n..at].to_vec()
        }).collect();
        let mut sh = Shape::new(s.source, rings, s.buffer_m);
        sh.country = s.country;
        shapes.push(sh);
    }
    Ok(Coverage { shapes })
}

/// A task folder's work files, as `job::work_files` has T's and its neighbours' (those not there:
/// none).
pub fn open_files(dir: &Path, t: Unit) -> Result<Vec<Option<WorkFile>>> {
    let mut files = Vec::with_capacity(9);
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let p = dir.join(file_name(t.x as i64 + dx, t.y as i64 + dy));
            files.push(if p.is_file() { Some(WorkFile::open(&p)?) } else { None });
        }
    }
    Ok(files)
}

/// The z6 tile of area `a` (a z8 tile).
pub fn tile_of(a: Unit) -> Unit {
    Unit { z: 6, x: a.x >> 2, y: a.y >> 2 }
}

/// Area `a`'s tiles and summary written into `dir` (`TILES`, `SUMMARY`), from `files` and `cov`
/// (the whole work files and coverage, or a task's): the program's run and the job's own.
pub fn write_area(files: &[Option<WorkFile>], cov: &Coverage, a: Unit, dir: &Path) -> Result<Summary> {
    ensure!(a.z == 8, "an area is a z8 tile ({} isn't one)", a.slash());
    std::fs::create_dir_all(dir)?;
    let meta = serde_json::json!({ "layer": super::LAYER, "area": a.slash(), "encoding": "mvt" }).to_string();
    let mut w = ArchiveWriter::create(&dir.join(TILES), &meta)?;
    let sum = job::area_tiles(files, cov, tile_of(a), (a.x, a.y), &mut |z, x, y, gz, raw| w.add(z, x, y, gz, raw as usize))?;
    w.finish()?;
    std::fs::write(dir.join(SUMMARY), serde_json::to_vec(&sum)?)?;
    Ok(sum)
}

/// The program's run: area `a`'s task folder `dir` read, its tiles and summary written there.
pub fn run(dir: &Path, a: Unit) -> Result<Summary> {
    let files = open_files(dir, tile_of(a))?;
    let cov = read_coverage(&dir.join(COVERAGE))?;
    write_area(&files, &cov, a, dir)
}

/// An area's results in `dir` (`write_area`'s, a worker's or this Mac's), each tile given to `add`
/// in order (zoom, x, y: as `area_tiles` gives them); its summary. A tile outside the area, or of
/// another zoom, is refused.
pub fn take_area(dir: &Path, a: Unit, add: &mut AddTile) -> Result<Summary> {
    let sum: Summary = serde_json::from_slice(&std::fs::read(dir.join(SUMMARY))?).context("an area's summary")?;
    let ar = Archive::open(&dir.join(TILES))?;
    for e in ar.entries() {
        let (z, x, y) = super::key_zxy(e.key);
        ensure!((super::MINZOOM..=super::MAXZOOM).contains(&z) && (x >> (z - 8), y >> (z - 8)) == (a.x, a.y), "area {}'s tiles hold {z}/{x}/{y}", a.slash());
        add(z, x, y, ar.get_entry(e), e.raw_len)?;
    }
    Ok(sum)
}

/// A task's spec (the shape of a tail's: crate::offload) for area `a`: the program `bldtile` over
/// its folder.
pub fn spec(a: Unit, version: &str, inputs: &BTreeMap<String, u64>) -> serde_json::Value {
    let run = crate::unit::Run { what: KIND.into(), prog: KIND.into(), args: vec!["{dir}".into(), a.slash()], env: vec![], reads: vec!["{dir}/*".into()] };
    let list: Vec<serde_json::Value> = inputs.iter().map(|(p, n)| serde_json::json!([p, n])).collect();
    serde_json::json!({ "unit": a.slash(), "version": version, "runs": [run], "inputs": list, "places": crate::offload::places() })
}

/// The files of a task's folder `root` (its `u/`): (path in it, size).
pub fn inputs_of(root: &Path) -> Result<BTreeMap<String, u64>> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(root.join("u"))?.flatten() {
        out.insert(format!("u/{}", e.file_name().to_string_lossy()), e.metadata()?.len());
    }
    Ok(out)
}

/// Whether two areas' results are the same bytes (both files).
pub fn same_files(a: &Path, b: &Path) -> bool {
    [TILES, SUMMARY].iter().all(|f| matches!((std::fs::read(a.join(f)), std::fs::read(b.join(f))), (Ok(x), Ok(y)) if x == y))
}

/// Where an area's results wait for its turn in the pack.
pub fn results_dir(scratch: &Path, a: Unit) -> PathBuf {
    scratch.join("bldtile-results").join(a.dash())
}

/// A `bldtiles` job's areas out as tasks (crate::offload), as a unit job's tails are: offered from
/// the far end of T's list while workers that take them are around (at most three out at once),
/// settled in their turn. Nothing waits on a worker longer than this Mac would take: an area no one
/// took is given a moment, then taken back and run here; one a worker holds is waited on while the
/// worker will be back with it before this Mac's run would end (crate::offload::Patience: its
/// buildings at the pace of the areas made here), then raced here; a worker's result is taken, or
/// (the coordinator says when) checked against this Mac's run byte for byte, a difference marking
/// the worker bad.
pub struct Offers<'a> {
    offload: Option<&'a crate::offload::Offload>,
    t: Unit,
    /// Where results wait for their turn (`results_dir`).
    scratch: PathBuf,
    /// The areas out (by their place in T's list), and those settled before their turn.
    out: BTreeMap<usize, crate::offload::Offered>,
    ready: BTreeMap<usize, PathBuf>,
    /// The areas from here to the end were offered (or kept here).
    far: usize,
    /// At most this many out at once.
    most: usize,
    /// The buildings each area out has (its work files' records), and the areas made here so far:
    /// their time and buildings (what an area takes here, `Patience`).
    records: BTreeMap<usize, u64>,
    here: (f64, u64),
}

impl<'a> Offers<'a> {
    pub fn new(offload: Option<&'a crate::offload::Offload>, scratch: &Path, t: Unit, areas: usize) -> Offers<'a> {
        let scratch = scratch.to_path_buf();
        std::fs::remove_dir_all(scratch.join("bldtile-results")).ok();
        Offers { offload, t, scratch, out: BTreeMap::new(), ready: BTreeMap::new(), far: areas, most: 3, records: BTreeMap::new(), here: (0.0, 0) }
    }

    /// An area made here took `secs` for its `records` buildings (and parts, and those outside).
    pub fn made_here(&mut self, records: u64, secs: f64) {
        self.here = (self.here.0 + secs, self.here.1 + records);
    }

    /// About how long area `i` would take here (seconds), at the pace of those made here so far.
    fn here_s(&self, i: usize) -> Option<f64> {
        let (secs, n) = self.here;
        (n > 0).then(|| secs / n as f64 * self.records.get(&i).copied().unwrap_or(0) as f64)
    }

    fn area(&self, areas: &[(u32, u32)], i: usize) -> Unit {
        Unit { z: 8, x: areas[i].0, y: areas[i].1 }
    }

    /// Before area `k` runs: the areas workers finished meanwhile settled, and more offered from the
    /// far end (beyond `k`) while fewer are out than workers around (at most three).
    pub fn top_up(&mut self, files: &[Option<WorkFile>], cov: &Coverage, areas: &[(u32, u32)], k: usize) -> Result<()> {
        let Some(o) = self.offload else { return Ok(()) };
        for i in self.out.keys().copied().collect::<Vec<_>>() {
            self.settle(files, cov, areas, i, false)?;
        }
        if self.far <= k + 1 || self.out.len() >= self.most {
            return Ok(());
        }
        let depth = o.workers(KIND).min(self.most);
        while self.out.len() < depth && self.far > k + 1 {
            self.far -= 1;
            let a = self.area(areas, self.far);
            let offered = (|| {
                let root = o.task_root(&format!("bldtile-{}", a.dash()));
                let c = cut(files, cov, self.t, (a.x, a.y), &root.join("u"))?;
                let inputs = inputs_of(&root)?;
                o.offer_spec(KIND, spec(a, o.version(), &inputs), &root, inputs, c.mem_mb()).map(|t| (t, c.records))
            })();
            match offered {
                Ok((task, records)) => {
                    self.out.insert(self.far, task);
                    self.records.insert(self.far, records);
                }
                Err(e) => {
                    // (Run here in its turn, and none offered after it.)
                    eprintln!("bldtiles {}: area {} not offered ({e:#}); run here", self.t.slash(), a.slash());
                    self.most = 0;
                    break;
                }
            }
        }
        Ok(())
    }

    /// Area `k`'s results, if it was offered: settled now (waiting for no one), in a folder to take
    /// into the pack (`take_area`) and remove. None: it's to run here.
    pub fn result(&mut self, files: &[Option<WorkFile>], cov: &Coverage, areas: &[(u32, u32)], k: usize) -> Result<Option<PathBuf>> {
        if self.out.contains_key(&k) {
            self.settle(files, cov, areas, k, true)?;
        }
        Ok(self.ready.remove(&k))
    }

    /// Settles area `i`'s task (with `wait` false, only when a worker finished or failed it; else
    /// with the patience its time here allows): its results put in its folder.
    fn settle(&mut self, files: &[Option<WorkFile>], cov: &Coverage, areas: &[(u32, u32)], i: usize, wait: bool) -> Result<()> {
        let (Some(o), Some(task)) = (self.offload, self.out.get(&i)) else { return Ok(()) };
        let a = self.area(areas, i);
        let mine = results_dir(&self.scratch, a);
        let here = || -> Result<()> {
            std::fs::remove_dir_all(&mine).ok();
            write_area(files, cov, a, &mine).map(|_| ())
        };
        let theirs = |st: &serde_json::Value| st["out"].as_str().map(|o| Path::new(o).join("u"));
        let mut take = |st: &serde_json::Value| -> Result<()> {
            // (A worker's files moved here whole, and read through once; else run here.)
            let ok = (|| -> Result<()> {
                let from = theirs(st).context("no outputs' folder")?;
                std::fs::remove_dir_all(&mine).ok();
                std::fs::create_dir_all(&mine)?;
                for f in [TILES, SUMMARY] {
                    if std::fs::rename(from.join(f), mine.join(f)).is_err() {
                        store::sys::copy_data(from.join(f), mine.join(f)).with_context(|| format!("take {f}"))?;
                    }
                }
                take_area(&mine, a, &mut |_, _, _, _, _| Ok(())).map(|_| ())
            })();
            if let Err(e) = ok {
                eprintln!("bldtiles {}: area {}: a worker's result doesn't read ({e:#}); run here", self.t.slash(), a.slash());
                here()?;
            }
            Ok(())
        };
        let mut same = |st: &serde_json::Value, _: std::time::SystemTime| -> Result<bool> {
            let same = theirs(st).is_some_and(|d| same_files(&d, &mine));
            if !same {
                eprintln!("bldtiles {}: area {}: a worker's tiles differ from this Mac's", self.t.slash(), a.slash());
            }
            Ok(same)
        };
        let Some(how) = o.settle_with(task, wait, crate::offload::Patience { here_s: self.here_s(i) }, &mut || here(), &mut take, &mut same)? else { return Ok(()) };
        let how = match how {
            crate::offload::Settled::Remote(w) => format!("by {w}"),
            crate::offload::Settled::Here(None) => "here".into(),
            crate::offload::Settled::Here(Some((w, true))) => format!("here, and by {w} the same"),
            crate::offload::Settled::Here(Some((w, false))) => format!("here: {w}'s differed, and it gets no more work"),
        };
        eprintln!("bldtiles {}: area {} made {how}", self.t.slash(), a.slash());
        self.out.remove(&i);
        self.records.remove(&i);
        self.ready.insert(i, mine);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;
    use crate::bld::prep::{self, tests::{end, frame, square, B}};

    /// Tile `t`'s work file in `dir`, from buildings and parts.
    fn work(dir: &Path, t: Unit, bs: &[B], parts: &[B]) -> WorkFile {
        let mut s = prep::MAGIC.to_vec();
        frame(&mut s, false, bs);
        if !parts.is_empty() {
            frame(&mut s, true, parts);
        }
        end(&mut s);
        let p = prep::read_stream(&mut &s[..], t).unwrap();
        let path = dir.join(format!("w-{}.sect", t.dash()));
        p.write(t, "2026-09-23.1", &path).unwrap();
        WorkFile::open(&path).unwrap()
    }

    /// Three shapes in the recipes' order: a circle far west (Paris, "GB": in no task of the east
    /// edge), then two overlapping circles at T's east edge, France's and Spain's (where they
    /// overlap, France's: the first).
    fn coverage() -> Coverage {
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, o: &str| Recipe { id: id.into(), name: id.into(), outline: vec![o.into()] };
        let mut c = Coverage::from_recipes(&[r("far", "place:2.35,48.85,2"), r("fr", "place:5.615,48.80,3"), r("es", "place:5.66,48.80,3")], None, d.path()).unwrap();
        for (s, cc) in c.shapes.iter_mut().zip(["GB", "FR", "ES"]) {
            s.country = cc.into();
        }
        c
    }

    /// T (6/32/22) and its east neighbour (6/33/22, lon 5.625 on): buildings on both sides of
    /// their edge (measured, with floors, unmeasured among measured ones, one in the neighbour's
    /// file reaching into T's area, an outline with a part), one in the circles' overlap with
    /// floors, and a few in Paris (another area).
    fn files(d: &Path) -> Vec<Option<WorkFile>> {
        let (t, e) = (Unit { z: 6, x: 32, y: 22 }, Unit { z: 6, x: 33, y: 22 });
        let lat = 48.80;
        let mut mine = vec![
            B { id: 1, rings: vec![square(5.6235, lat, 0.0004)], height: 45.0, ..Default::default() },
            // In the circles' overlap, with floors.
            B { id: 2, rings: vec![square(5.6243, lat + 0.001, 0.0002)], floors: 5, ..Default::default() },
            B { id: 3, rings: vec![square(5.6400 - 0.0200, lat - 0.004, 0.0002)], class: "house", ..Default::default() },
            B { id: 4, rings: vec![square(5.6246, lat - 0.002, 0.0002)], ..Default::default() },
            B { id: 5, rings: vec![square(5.6200, lat + 0.003, 0.0006)], has_parts: true, ..Default::default() },
            // Paris: 8/129/88.
            B { id: 6, rings: vec![square(2.3470, 48.858, 0.0004)], floors: 7, ..Default::default() },
            B { id: 7, rings: vec![square(2.3490, 48.858, 0.0002)], ..Default::default() },
        ];
        for k in 0..8 {
            mine.push(B { id: 20 + k, rings: vec![square(5.6230 + 0.0003 * (k % 4) as f64, lat - 0.0015 - 0.0003 * (k / 4) as f64, 0.0002)], height: 11.0, ..Default::default() });
        }
        let parts = [B { id: 50, parent: 5, rings: vec![square(5.6201, lat + 0.0031, 0.0003)], height: 30.0, ..Default::default() }];
        let mut theirs = vec![
            // Its centroid in the neighbour's tile, reaching west into T's area: copied there.
            B { id: 100, rings: vec![square(5.6240, lat, 0.0025)], height: 18.0, ..Default::default() },
            // In the overlap, and in Spain's alone, with floors.
            B { id: 101, rings: vec![square(5.6380, lat, 0.0002)], floors: 4, ..Default::default() },
            B { id: 102, rings: vec![square(5.6900, lat, 0.0002)], floors: 4, ..Default::default() },
            // Unmeasured, among the measured on the other side of the edge (300 m).
            B { id: 103, rings: vec![square(5.6255, lat - 0.0017, 0.0002)], ..Default::default() },
        ];
        for k in 0..6 {
            theirs.push(B { id: 120 + k, rings: vec![square(5.6262 + 0.0003 * k as f64, lat - 0.001, 0.0002)], height: 14.0, ..Default::default() });
        }
        let mut files: Vec<Option<WorkFile>> = (0..9).map(|_| None).collect();
        files[4] = Some(work(d, t, &mine, &parts));
        files[5] = Some(work(d, e, &theirs, &[]));
        files
    }

    type Tiles = Vec<(u8, u32, u32, Vec<u8>, u32)>;

    fn collect(out: &mut Tiles) -> impl FnMut(u8, u32, u32, &[u8], u32) -> Result<()> + '_ {
        |z, x, y, gz, raw| {
            out.push((z, x, y, gz.to_vec(), raw));
            Ok(())
        }
    }

    #[test]
    fn an_areas_task_makes_the_tiles_the_whole_files_make() {
        let d = tempfile::tempdir().unwrap();
        let t = Unit { z: 6, x: 32, y: 22 };
        let files = files(d.path());
        let cov = coverage();
        let areas = job::areas_of(&files);
        assert_eq!(areas, [(129, 88), (131, 88)]);
        let mut all: Tiles = Vec::new();
        let whole = job::tiles_of(&files, &cov, t, &mut collect(&mut all)).unwrap();
        let mut merged = Summary::default();
        let mut by_area: Tiles = Vec::new();
        for &a in &areas {
            let a8 = Unit { z: 8, x: a.0, y: a.1 };
            let mut here: Tiles = Vec::new();
            let s = job::area_tiles(&files, &cov, t, a, &mut collect(&mut here)).unwrap();
            merged.merge(&s);
            // Cut, run as the program runs, taken back: the same tiles and summary.
            let dir = d.path().join(a8.dash());
            let c = cut(&files, &cov, t, a, &dir).unwrap();
            let got = run(&dir, a8).unwrap();
            assert_eq!(got, s);
            let mut back: Tiles = Vec::new();
            assert_eq!(take_area(&dir, a8, &mut collect(&mut back)).unwrap(), s);
            assert_eq!(back, here, "area {}", a8.slash());
            assert!(!here.is_empty());
            // Only the shapes that can answer there, in order; the area's own records counted.
            let tc = read_coverage(&dir.join(COVERAGE)).unwrap();
            let countries: Vec<&str> = tc.shapes.iter().map(|s| s.country.as_str()).collect();
            if a == (131, 88) {
                assert_eq!(countries, ["FR", "ES"]);
                assert!(dir.join("6-33-22.sect").is_file(), "the neighbour's blocks");
                assert_eq!(c.records, 14);
                // The neighbour's building copied into the area; the floors of one in the circles'
                // overlap by France's storeys (the first shape), not Spain's.
                assert!(here.iter().any(|tl| props(&tl.3).contains(&(180, 0, 1))), "a copy of the neighbour's");
                let fr = fill::floors_dm(5, fill::storey("FR")) as u64;
                assert_ne!(fr, fill::floors_dm(5, fill::storey("ES")) as u64);
                assert!(here.iter().any(|tl| tl.0 == 14 && props(&tl.3).contains(&(fr, 1, 0))));
            } else {
                assert_eq!(countries, ["GB"]);
                assert!(!dir.join("6-33-22.sect").exists());
            }
            assert!(c.mem_mb() >= 256);
            // On one thread: the same bytes.
            let one = d.path().join(format!("{}-1", a8.dash()));
            cut(&files, &cov, t, a, &one).unwrap();
            let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
            pool.install(|| run(&one, a8)).unwrap();
            assert!(same_files(&one, &dir));
            by_area.extend(here);
        }
        // tiles_of: the areas' tiles in order, their summaries merged.
        assert_eq!(all, by_area);
        assert_eq!(merged, whole);
        // A worker's tile outside its area is refused.
        let a8 = Unit { z: 8, x: 131, y: 88 };
        let bad = d.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::copy(d.path().join(a8.dash()).join(SUMMARY), bad.join(SUMMARY)).unwrap();
        let mut w = ArchiveWriter::create(&bad.join(TILES), "{}").unwrap();
        w.add(14, 1, 1, b"x", 1).unwrap();
        w.finish().unwrap();
        assert!(take_area(&bad, a8, &mut |_, _, _, _, _| Ok(())).is_err());
    }

    use crate::bld::fill;

    /// A tile's features' (h, s, o).
    fn props(gz: &[u8]) -> Vec<(u64, u64, u64)> {
        let t = names::mvt::Tile::decode(&names::mvt::gunzip_if_gzip(gz).unwrap()).unwrap();
        let l = &t.layers[0];
        l.features
            .iter()
            .map(|f| {
                let get = |k: &str| f.tags.chunks(2).find(|kv| l.keys[kv[0] as usize] == k).map_or(0, |kv| match l.values[kv[1] as usize] { names::mvt::Value::Uint(v) => v, _ => 0 });
                (get("h"), get("s"), get("o"))
            })
            .collect()
    }

    /// A worker in this process: asks, fetches the task's files, runs the program's code over them,
    /// sends back what it wrote (`spoil`: its tiles spoilt first), done.
    fn worker_runs(m1: &crate::coord::client::Client, spoil: bool, dir: &Path) -> Option<serde_json::Value> {
        let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
        let g = m1.ask(&ask).unwrap()?;
        let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
        assert_eq!(task["runs"][0]["prog"], KIND);
        assert_eq!(task["runs"][0]["args"][0], "{dir}");
        let a = Unit::parse(task["unit"].as_str().unwrap()).unwrap();
        std::fs::remove_dir_all(dir).ok();
        let u = dir.join("u");
        std::fs::create_dir_all(&u).unwrap();
        for i in task["inputs"].as_array().unwrap() {
            let p = i[0].as_str().unwrap();
            std::fs::write(dir.join(p), m1.get_bytes(&format!("/work/in/{}/{p}", g.lease)).unwrap()).unwrap();
        }
        run(&u, a).unwrap();
        let mut outputs = Vec::new();
        for f in [TILES, SUMMARY] {
            let mut b = std::fs::read(u.join(f)).unwrap();
            if spoil && f == TILES {
                let n = b.len();
                b[n / 2] ^= 1;
            }
            m1.put_bytes(&format!("/work/out/{}/u/{f}", g.lease), &b).unwrap();
            outputs.push(crate::coord::task::Output { path: format!("u/{f}"), size: b.len() as u64 });
        }
        let done = crate::coord::Done { lease: g.lease, outputs, peak_mb: 300, secs: 1.0, ..Default::default() };
        assert_eq!(m1.done(&done).unwrap(), crate::coord::client::Handed::Taken);
        Some(task)
    }

    #[test]
    fn areas_go_to_workers_and_come_back_the_same() {
        let d = tempfile::tempdir().unwrap();
        let t = Unit { z: 6, x: 32, y: 22 };
        let files = files(d.path());
        let cov = coverage();
        let areas = job::areas_of(&files);
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        // (The coordinator and the job on a virtual clock: the job's waits are counted in it.)
        let clock = store::clock::Virtual::new();
        c.shared.lock().unwrap().clock = clock.clone();
        let url = format!("http://127.0.0.1:{port}");
        let o = crate::offload::Offload::at(url.clone(), c.job_token.clone(), &d.path().join("job")).clock(clock.clone());
        let scratch = d.path().join("job");
        // No worker around: nothing offered, every area run here.
        let mut offers = Offers::new(Some(&o), &scratch, t, areas.len());
        offers.top_up(&files, &cov, &areas, 0).unwrap();
        assert!(offers.out.is_empty());
        // A worker that takes them asks (nothing yet), and is counted.
        let m1 = crate::coord::client::Client::at(vec![url], c.contact.token.clone(), "m1");
        assert!(worker_runs(&m1, false, &d.path().join("m1")).is_none());
        offers.top_up(&files, &cov, &areas, 0).unwrap();
        // The far end's area offered (one worker: one out), never the one under way.
        assert_eq!(offers.out.keys().copied().collect::<Vec<_>>(), [1]);
        let task = worker_runs(&m1, false, &d.path().join("m1")).unwrap();
        assert_eq!(task["unit"], "8/131/88");
        // Before area 0 runs: the finished one settled (a first result: checked here, the same).
        offers.top_up(&files, &cov, &areas, 0).unwrap();
        assert!(offers.out.is_empty());
        let dir = offers.result(&files, &cov, &areas, 1).unwrap().unwrap();
        let mut got: Tiles = Vec::new();
        let a8 = Unit { z: 8, x: 131, y: 88 };
        take_area(&dir, a8, &mut collect(&mut got)).unwrap();
        let mut want: Tiles = Vec::new();
        job::area_tiles(&files, &cov, t, (131, 88), &mut collect(&mut want)).unwrap();
        assert_eq!(got, want);
        assert!(offers.result(&files, &cov, &areas, 0).unwrap().is_none(), "area 0 runs here");
        {
            let s = c.shared.lock().unwrap();
            assert_eq!(s.workers["m1"].checked, 1);
            assert!(!s.workers["m1"].bad);
            // Its cost learned under its kind and area.
            assert_eq!(s.costs["bldtile 8/131/88"].peak_mb, 300);
        }
        // Offered again (another job), with the cost learned; a spoilt result: checked, run here,
        // and the worker gets no more work. (Its pace forgotten first: measured slower than this
        // Mac's run on its first area, it would be given none, coord::task::Tasks::pick.)
        c.shared.lock().unwrap().tasks.paces.clear();
        let mut offers = Offers::new(Some(&o), &scratch, t, areas.len());
        offers.top_up(&files, &cov, &areas, 0).unwrap();
        assert_eq!(offers.out.len(), 1);
        assert_eq!(c.shared.lock().unwrap().tasks.by_id.values().next().unwrap().mem_mb, 330);
        worker_runs(&m1, true, &d.path().join("m1")).unwrap();
        let dir = offers.result(&files, &cov, &areas, 1).unwrap().unwrap();
        let mut again: Tiles = Vec::new();
        take_area(&dir, a8, &mut collect(&mut again)).unwrap();
        assert_eq!(again, want, "this Mac's");
        assert!(c.shared.lock().unwrap().workers["m1"].bad);
        // A task no one took (a worker asking, sparing too little for it): taken back and run here
        // at once. (What it would take here: its buildings at the pace of those made here.)
        let m2 = crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m2");
        m2.ask(&crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 10, ..Default::default() }).unwrap();
        let mut offers = Offers::new(Some(&o), &scratch, t, areas.len());
        offers.top_up(&files, &cov, &areas, 0).unwrap();
        assert_eq!(offers.out.len(), 1);
        assert_eq!(offers.here_s(1), None);
        offers.made_here(10, 2.0);
        assert!(offers.records[&1] > 0);
        assert_eq!(offers.here_s(1), Some(0.2 * offers.records[&1] as f64));
        let dir = offers.result(&files, &cov, &areas, 1).unwrap().unwrap();
        assert_eq!(clock.slept().0, 0, "never waited");
        let mut third: Tiles = Vec::new();
        take_area(&dir, a8, &mut collect(&mut third)).unwrap();
        assert_eq!(third, want);
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty(), "closed");
    }
}
