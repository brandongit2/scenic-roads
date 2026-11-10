//! A z8 tile of `bldtiles T` as a task (docs/buildings3d.md §3.6, docs/workers.md §3): its files cut
//! from T's and its neighbours' work files where the job runs, the program `bldtile` making the z8
//! tile's tiles from them, and the job taking them into T's
//! pack in the z8 tile's turn.
//!
//! A task's folder (`u/` on a worker; docs/formats.md):
//! - `6-<x>-<y>.sect`: of T's and its 8 neighbours' work files, those with a block the z8 tile reads
//!   (`job::z8_blocks`: its own, and those within the margin around it), each a work file of the
//!   same format and meta holding only those blocks, their bytes as stored (not compressed again);
//! - `coverage.sect`: the coverage's shapes that can answer for a point the z8 tile asks about (any of
//!   its blocks' records' centroids and vertices, and the blocks' boxes), in the recipes' order,
//!   each whole with its buffer and country: a building's shape is then the same shape (another
//!   index), so its country and the first-match order are the same.
//!
//! - `heritage.bin`: T's heritage sites (`job::sites`), E7 pairs (i32 LE), sorted.
//!
//! What the program writes there: `area.tiles` (an RDTILES archive of the z8 tile's z14 tiles, as
//! `job::z8_tiles` makes them), `area.json` (its `job::Summary`), `area.field` (an RDTILES
//! archive of its field's tiles, levels 14–9) and `area.mid` (its level-8 planes, `field::write_mid`;
//! none without a building). The job's own run of a z8 tile (one no worker finished) writes the
//! same files the same way, from the whole work files.

use super::field;
use super::job::{self, AddTile, Summary};
use super::work::{self, WorkFile};
use anyhow::{ensure, Context, Result};
use buildstore::coverage::{Coverage, Shape};
use codecs::legacy::TileId;
use roadcore::archive::{Archive, ArchiveWriter};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The coverage's file in a task's folder.
pub const COVERAGE: &str = "coverage.sect";
/// What the program writes: the z8 tile's tiles, and what it made. (The files' names keep "area", from
/// when a z8 tile was called an area, as does "area" in their meta: a worker running an earlier
/// build reads them, and archives made so are stored; renaming would break both.)
pub const TILES: &str = "area.tiles";
pub const SUMMARY: &str = "area.json";
pub const FIELD: &str = "area.field";
pub const MID: &str = "area.mid";
/// T's heritage sites, in a task's folder.
pub const SITES: &str = "heritage.bin";
/// What the program writes (`MID` only with a building).
pub const OUTPUTS: [&str; 4] = [TILES, SUMMARY, FIELD, MID];
/// The task's kind and its program.
pub const KIND: &str = "bldtile";

/// A work file's name in a task's folder.
fn file_name(x: i64, y: i64) -> String {
    format!("6-{x}-{y}.sect")
}

/// What a cut wrote: its files' bytes, and the z8 tile's own buildings and parts (its blocks' counts).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cut {
    pub bytes: u64,
    pub records: u64,
}

impl Cut {
    /// What a task of it is offered at (MB): its files, what its worker holds at least. The
    /// coordinator sizes it by its kind's measured runs (agentcore::coord's `/task/offer`, task #979).
    pub fn files_mb(&self) -> u64 {
        self.bytes.div_ceil(1 << 20)
    }
}

/// Writes z8 tile `a`'s task files (of T, `files` its and its neighbours' work files as
/// `job::work_files` has them) into `dir`.
pub fn cut(
    files: &[Option<WorkFile>],
    cov: &Coverage,
    t: TileId,
    a: (u32, u32),
    sites: &[[i32; 2]],
    dir: &Path,
) -> Result<Cut> {
    ensure!(files.len() == 9, "T and its 8 neighbours");
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(SITES), sites_bytes(sites))?;
    let (list, n_own) = job::z8_blocks(files, t, a);
    let records = list[..n_own].iter().map(|(_, e)| e.count as u64).sum();
    // Each file's blocks, by key; and the box of every point the z8 tile asks the coverage about.
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
    let kept: Vec<&Shape> = cov
        .shapes
        .iter()
        .filter(|s| s.bbox[0] <= ext[2] && s.bbox[2] >= ext[0] && s.bbox[1] <= ext[3] && s.bbox[3] >= ext[1])
        .collect();
    write_coverage(&path, &kept)?;
    bytes += std::fs::metadata(&path)?.len();
    Ok(Cut { bytes, records })
}

/// Sites as `SITES` holds them.
pub fn sites_bytes(sites: &[[i32; 2]]) -> Vec<u8> {
    sites.iter().flat_map(|p| p[0].to_le_bytes().into_iter().chain(p[1].to_le_bytes())).collect()
}

fn read_sites(path: &Path) -> Result<Vec<[i32; 2]>> {
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    ensure!(b.len() % 8 == 0, "{}: not E7 pairs", path.display());
    Ok(b.chunks_exact(8)
        .map(|c| [i32::from_le_bytes(c[0..4].try_into().unwrap()), i32::from_le_bytes(c[4..8].try_into().unwrap())])
        .collect())
}

/// Writes shapes as a task's coverage (RDSECT: meta `{"fmt": 1, "shapes": [{source, country,
/// buffer_m, rings: [vertices a ring]}]}`, section `verts` the rings' vertices, E7).
fn write_coverage(path: &Path, shapes: &[&Shape]) -> Result<()> {
    let meta: Vec<serde_json::Value> = shapes.iter().map(|s| serde_json::json!({ "source": s.source, "country": s.country, "buffer_m": s.buffer_m, "rings": s.rings.iter().map(<[_]>::len).collect::<Vec<_>>() })).collect();
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
    ensure!(
        m.shapes.iter().flat_map(|s| &s.rings).sum::<usize>() == verts.len(),
        "a task's coverage's vertices don't add up"
    );
    let mut at = 0usize;
    let mut shapes = Vec::with_capacity(m.shapes.len());
    for s in m.shapes {
        let rings = s
            .rings
            .iter()
            .map(|&n| {
                at += n;
                verts[at - n..at].to_vec()
            })
            .collect();
        let mut sh = Shape::new(s.source, rings, s.buffer_m);
        sh.country = s.country;
        shapes.push(sh);
    }
    Ok(Coverage { shapes, ..Default::default() })
}

/// A task folder's work files, as `job::work_files` has T's and its neighbours' (those not there:
/// none).
pub fn open_files(dir: &Path, t: TileId) -> Result<Vec<Option<WorkFile>>> {
    let mut files = Vec::with_capacity(9);
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let p = dir.join(file_name(t.x as i64 + dx, t.y as i64 + dy));
            files.push(if p.is_file() { Some(WorkFile::open(&p)?) } else { None });
        }
    }
    Ok(files)
}

/// The z6 tile of z8 tile `a` (a z8 tile).
pub fn tile_of(a: TileId) -> TileId {
    TileId { z: 6, x: a.x >> 2, y: a.y >> 2 }
}

/// Area `a`'s tiles and summary written into `dir` (`TILES`, `SUMMARY`), from `files` and `cov`
/// (the whole work files and coverage, or a task's): the program's run and the job's own.
pub fn write_z8(
    files: &[Option<WorkFile>],
    cov: &Coverage,
    a: TileId,
    sites: &[[i32; 2]],
    dir: &Path,
) -> Result<Summary> {
    ensure!(a.z == 8, "{} isn't a z8 tile", a.slash());
    std::fs::create_dir_all(dir)?;
    let meta = serde_json::json!({ "layer": super::LAYER, "area": a.slash(), "encoding": "mvt" }).to_string();
    let mut w = ArchiveWriter::create(&dir.join(TILES), &meta)?;
    let (sum, f) = job::z8_tiles(files, cov, tile_of(a), (a.x, a.y), sites, &mut |z, x, y, gz, raw| {
        w.add(z, x, y, gz, raw as usize)
    })?;
    w.finish()?;
    let meta = serde_json::json!({ "layer": field::LAYER, "area": a.slash(), "encoding": field::ENCODING }).to_string();
    let mut w = ArchiveWriter::create(&dir.join(FIELD), &meta)?;
    for (z, x, y, b) in &f.tiles {
        w.add(*z, *x, *y, b, b.len())?;
    }
    w.finish()?;
    std::fs::remove_file(dir.join(MID)).ok();
    if let Some(m) = f.mid {
        std::fs::write(dir.join(MID), field::write_mid(&BTreeMap::from([((a.x, a.y), m)]))?)?;
    }
    std::fs::write(dir.join(SUMMARY), serde_json::to_vec(&sum)?)?;
    Ok(sum)
}

/// The program's run: z8 tile `a`'s task folder `dir` read, its tiles and summary written there.
pub fn run(dir: &Path, a: TileId) -> Result<Summary> {
    let files = open_files(dir, tile_of(a))?;
    let cov = read_coverage(&dir.join(COVERAGE))?;
    write_z8(&files, &cov, a, &read_sites(&dir.join(SITES))?, dir)
}

/// A z8 tile's results in `dir` (`write_z8`'s, a worker's or this Mac's), each tile given to `add`
/// in order (zoom, x, y: as `z8_tiles` gives them); its summary and field. A tile outside the
/// z8 tile, or of another zoom or level, is refused.
pub fn take_z8(dir: &Path, a: TileId, add: &mut AddTile) -> Result<(Summary, field::Z8Field)> {
    let sum: Summary = serde_json::from_slice(&std::fs::read(dir.join(SUMMARY))?).context("a z8 tile's summary")?;
    let ar = Archive::open(&dir.join(TILES))?;
    for e in ar.entries() {
        let (z, x, y) = super::key_zxy(e.key);
        ensure!(
            z == super::ZOOM && (x >> (z - 8), y >> (z - 8)) == (a.x, a.y),
            "z8 tile {}'s tiles hold {z}/{x}/{y}",
            a.slash()
        );
        add(z, x, y, ar.get_entry(e), e.raw_len)?;
    }
    let mut f = field::Z8Field::default();
    let ar = Archive::open(&dir.join(FIELD))?;
    for e in ar.entries() {
        let (z, x, y) = super::key_zxy(e.key);
        ensure!(
            (field::LEVEL_HI_MIN..=field::LEVEL_MAX).contains(&z) && (x >> (z - 8), y >> (z - 8)) == (a.x, a.y),
            "z8 tile {}'s field holds {z}/{x}/{y}",
            a.slash()
        );
        f.tiles.push((z, x, y, ar.get_entry(e).to_vec()));
    }
    // (In the field's order: the finest level first.)
    f.tiles.sort_by(|p, q| q.0.cmp(&p.0).then((p.1, p.2).cmp(&(q.1, q.2))));
    match std::fs::read(dir.join(MID)) {
        Ok(b) => {
            let mut m = field::read_mid(&b)?;
            ensure!(m.len() == 1 && m.contains_key(&(a.x, a.y)), "z8 tile {}'s mid holds another z8 tile", a.slash());
            f.mid = m.remove(&(a.x, a.y));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok((sum, f))
}

/// A task's spec (the shape of a tail's: the offload crate's) for z8 tile `a`: the program `bldtile` over
/// its folder.
pub fn spec(a: TileId, version: &str, inputs: &BTreeMap<String, u64>) -> serde_json::Value {
    let run = offload::Run {
        what: KIND.into(),
        prog: KIND.into(),
        args: vec!["{dir}".into(), a.slash()],
        env: vec![],
        reads: vec!["{dir}/*".into()],
    };
    let list: Vec<serde_json::Value> = inputs.iter().map(|(p, n)| serde_json::json!([p, n])).collect();
    serde_json::json!({ "unit": a.slash(), "version": version, "runs": [run], "inputs": list, "places": offload::places() })
}

/// The files of a task's folder `root` (its `u/`): (path in it, size).
pub fn inputs_of(root: &Path) -> Result<BTreeMap<String, u64>> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(root.join("u"))?.flatten() {
        out.insert(format!("u/{}", e.file_name().to_string_lossy()), e.metadata()?.len());
    }
    Ok(out)
}

/// Whether two z8 tiles' results are the same bytes (every file, `MID` there in both or neither).
#[cfg(test)]
fn same_files(a: &Path, b: &Path) -> bool {
    OUTPUTS.iter().all(|f| match (std::fs::read(a.join(f)), std::fs::read(b.join(f))) {
        (Ok(x), Ok(y)) => x == y,
        (Err(x), Err(y)) => {
            *f == MID && x.kind() == std::io::ErrorKind::NotFound && y.kind() == std::io::ErrorKind::NotFound
        }
        _ => false,
    })
}

/// Where a z8 tile's results wait for its turn in the pack.
pub fn results_dir(scratch: &Path, a: TileId) -> PathBuf {
    scratch.join("bldtile-results").join(a.dash())
}

/// A `bldtiles` job's z8 tiles out as tasks (the offload crate), as a unit job's tails are: offered from
/// the far end of T's list while workers that take them are around (at most three out at once),
/// settled in their turn. Nothing waits on a worker longer than this Mac would take: a z8 tile no one
/// took is given a moment, then taken back and run here; one a worker holds is waited on while the
/// worker will be back with it before this Mac's run would end (offload::Patience: its
/// buildings at the pace of the z8 tiles made here), then raced here; a worker's result is taken.
pub struct Offers<'a> {
    offload: Option<&'a offload::Offload>,
    t: TileId,
    /// Where results wait for their turn (`results_dir`).
    scratch: PathBuf,
    /// The z8 tiles out (by their place in T's list), and those settled before their turn.
    out: BTreeMap<usize, offload::Offered>,
    ready: BTreeMap<usize, PathBuf>,
    /// The z8 tiles from here to the end were offered (or kept here).
    far: usize,
    /// At most this many out at once.
    most: usize,
    /// The buildings each z8 tile out has (its work files' records), and the z8 tiles made here so far:
    /// their time and buildings (what a z8 tile takes here, `Patience`).
    records: BTreeMap<usize, u64>,
    here: (f64, u64),
    /// T's heritage sites (`SITES`).
    sites: Vec<[i32; 2]>,
}

impl<'a> Offers<'a> {
    pub fn new(
        offload: Option<&'a offload::Offload>,
        scratch: &Path,
        t: TileId,
        z8s: usize,
        sites: &[[i32; 2]],
    ) -> Offers<'a> {
        let scratch = scratch.to_path_buf();
        std::fs::remove_dir_all(scratch.join("bldtile-results")).ok();
        Offers {
            offload,
            t,
            scratch,
            out: BTreeMap::new(),
            ready: BTreeMap::new(),
            far: z8s,
            most: 3,
            records: BTreeMap::new(),
            here: (0.0, 0),
            sites: sites.to_vec(),
        }
    }

    /// A z8 tile made here took `secs` for its `records` buildings (and parts, and those outside).
    pub fn made_here(&mut self, records: u64, secs: f64) {
        self.here = (self.here.0 + secs, self.here.1 + records);
    }

    /// About how long z8 tile `i` would take here (seconds), at the pace of those made here so far.
    fn here_s(&self, i: usize) -> Option<f64> {
        let (secs, n) = self.here;
        (n > 0).then(|| secs / n as f64 * self.records.get(&i).copied().unwrap_or(0) as f64)
    }

    fn z8(&self, z8s: &[(u32, u32)], i: usize) -> TileId {
        TileId { z: 8, x: z8s[i].0, y: z8s[i].1 }
    }

    /// Before z8 tile `k` runs: the z8 tiles workers finished meanwhile settled, and more offered from the
    /// far end (beyond `k`) while fewer are out than workers around (at most three).
    pub fn top_up(&mut self, files: &[Option<WorkFile>], cov: &Coverage, z8s: &[(u32, u32)], k: usize) -> Result<()> {
        let Some(o) = self.offload else { return Ok(()) };
        for i in self.out.keys().copied().collect::<Vec<_>>() {
            self.settle(files, cov, z8s, i, false)?;
        }
        if self.far <= k + 1 || self.out.len() >= self.most {
            return Ok(());
        }
        let depth = o.workers(KIND).min(self.most);
        while self.out.len() < depth && self.far > k + 1 {
            self.far -= 1;
            let a = self.z8(z8s, self.far);
            let offered = (|| {
                let root = o.task_root(&format!("bldtile-{}", a.dash()));
                let c = cut(files, cov, self.t, (a.x, a.y), &self.sites, &root.join("u"))?;
                let inputs = inputs_of(&root)?;
                o.offer_spec(KIND, spec(a, o.version(), &inputs), &root, inputs, c.files_mb()).map(|t| (t, c.records))
            })();
            match offered {
                Ok((task, records)) => {
                    self.out.insert(self.far, task);
                    self.records.insert(self.far, records);
                }
                Err(e) => {
                    // (Run here in its turn, and none offered after it.)
                    eprintln!("bldtiles {}: z8 tile {} not offered ({e:#}); run here", self.t.slash(), a.slash());
                    self.most = 0;
                    break;
                }
            }
        }
        Ok(())
    }

    /// Area `k`'s results, if it was offered: settled now (waiting for no one), in a folder to take
    /// into the pack (`take_z8`) and remove. None: it's to run here.
    pub fn result(
        &mut self,
        files: &[Option<WorkFile>],
        cov: &Coverage,
        z8s: &[(u32, u32)],
        k: usize,
    ) -> Result<Option<PathBuf>> {
        if self.out.contains_key(&k) {
            self.settle(files, cov, z8s, k, true)?;
        }
        Ok(self.ready.remove(&k))
    }

    /// Settles z8 tile `i`'s task (with `wait` false, only when a worker finished or failed it; else
    /// with the patience its time here allows): its results put in its folder.
    fn settle(
        &mut self,
        files: &[Option<WorkFile>],
        cov: &Coverage,
        z8s: &[(u32, u32)],
        i: usize,
        wait: bool,
    ) -> Result<()> {
        let (Some(o), Some(task)) = (self.offload, self.out.get(&i)) else { return Ok(()) };
        let a = self.z8(z8s, i);
        let mine = results_dir(&self.scratch, a);
        let here = || -> Result<()> {
            std::fs::remove_dir_all(&mine).ok();
            write_z8(files, cov, a, &self.sites, &mine).map(|_| ())
        };
        let theirs = |st: &serde_json::Value| st["out"].as_str().map(|o| Path::new(o).join("u"));
        let mut take = |st: &serde_json::Value| -> Result<()> {
            // (A worker's files moved here whole, and read through once; else run here.)
            let ok = (|| -> Result<()> {
                let from = theirs(st).context("no outputs' folder")?;
                std::fs::remove_dir_all(&mine).ok();
                std::fs::create_dir_all(&mine)?;
                for f in OUTPUTS {
                    if f == MID && !from.join(f).exists() {
                        continue;
                    }
                    if std::fs::rename(from.join(f), mine.join(f)).is_err() {
                        store::sys::copy_data(from.join(f), mine.join(f)).with_context(|| format!("take {f}"))?;
                    }
                }
                take_z8(&mine, a, &mut |_, _, _, _, _| Ok(())).map(|_| ())
            })();
            if let Err(e) = ok {
                eprintln!(
                    "bldtiles {}: z8 tile {}: a worker's result doesn't read ({e:#}); run here",
                    self.t.slash(),
                    a.slash()
                );
                here()?;
            }
            Ok(())
        };
        let Some(how) =
            o.settle_with(task, wait, offload::Patience { here_s: self.here_s(i) }, &mut || here(), &mut take)?
        else {
            return Ok(());
        };
        eprintln!("bldtiles {}: z8 tile {} made {}", self.t.slash(), a.slash(), how.how());
        self.out.remove(&i);
        self.records.remove(&i);
        self.ready.insert(i, mine);
        Ok(())
    }
}

/// Work files, a coverage and their tiles for tests (this module's, pipeline's bld_tests), and what
/// an `Offers` holds, read (its fields and `here_s` stay private).
#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    use super::*;
    use crate::bld::prep::{
        self,
        testkit::{end, frame, square, B},
    };
    use buildstore::recipes::Recipe;

    /// Tile `t`'s work file in `dir`, from buildings and parts.
    fn work(dir: &Path, t: TileId, bs: &[B], parts: &[B]) -> WorkFile {
        let mut s = Vec::new();
        frame(&mut s, false, bs);
        if !parts.is_empty() {
            frame(&mut s, true, parts);
        }
        end(&mut s);
        let p = prep::Prepped::from_frames(s, t).unwrap();
        let path = dir.join(format!("w-{}.sect", t.dash()));
        p.write(t, "2026-09-23.1", &path).unwrap();
        WorkFile::open(&path).unwrap()
    }

    /// Three shapes in the recipes' order: a circle far west (Paris, "GB": in no task of the east
    /// edge), then two overlapping circles at T's east edge, France's and Spain's (where they
    /// overlap, France's: the first).
    pub fn coverage() -> Coverage {
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, o: &str| Recipe { id: id.into(), name: id.into(), outline: vec![o.into()] };
        let mut c = Coverage::fixture(
            &[r("far", "place:2.35,48.85,2"), r("fr", "place:5.615,48.80,3"), r("es", "place:5.66,48.80,3")],
            None,
            d.path(),
        )
        .unwrap();
        for (s, cc) in c.shapes.iter_mut().zip(["GB", "FR", "ES"]) {
            s.country = cc.into();
        }
        c
    }

    /// T (6/32/22) and its east neighbour (6/33/22, lon 5.625 on): buildings on both sides of
    /// their edge (measured, with floors, unmeasured among measured ones, one in the neighbour's
    /// file reaching into T's z8 tile, an outline with a part), one in the circles' overlap with
    /// floors, and a few in Paris (another z8 tile).
    pub fn files(d: &Path) -> Vec<Option<WorkFile>> {
        let (t, e) = (TileId { z: 6, x: 32, y: 22 }, TileId { z: 6, x: 33, y: 22 });
        let lat = 48.80;
        let mut mine = vec![
            B { id: 1, rings: vec![square(5.6235, lat, 0.0004)], height: 45.0, ..Default::default() },
            // In the circles' overlap, with floors.
            B { id: 2, rings: vec![square(5.6243, lat + 0.001, 0.0002)], floors: 5, ..Default::default() },
            B {
                id: 3,
                rings: vec![square(5.6400 - 0.0200, lat - 0.004, 0.0002)],
                class: "house",
                ..Default::default()
            },
            B { id: 4, rings: vec![square(5.6246, lat - 0.002, 0.0002)], ..Default::default() },
            B { id: 5, rings: vec![square(5.6200, lat + 0.003, 0.0006)], has_parts: true, ..Default::default() },
            // Paris: 8/129/88.
            B { id: 6, rings: vec![square(2.3470, 48.858, 0.0004)], floors: 7, ..Default::default() },
            B { id: 7, rings: vec![square(2.3490, 48.858, 0.0002)], ..Default::default() },
        ];
        for k in 0..8 {
            mine.push(B {
                id: 20 + k,
                rings: vec![square(5.6230 + 0.0003 * (k % 4) as f64, lat - 0.0015 - 0.0003 * (k / 4) as f64, 0.0002)],
                height: 11.0,
                ..Default::default()
            });
        }
        let parts = [B {
            id: 50,
            parent: 5,
            rings: vec![square(5.6201, lat + 0.0031, 0.0003)],
            height: 30.0,
            ..Default::default()
        }];
        let mut theirs = vec![
            // Its centroid in the neighbour's tile, reaching west into T's z8 tile: copied there.
            B { id: 100, rings: vec![square(5.6240, lat, 0.0025)], height: 18.0, ..Default::default() },
            // In the overlap, and in Spain's alone, with floors.
            B { id: 101, rings: vec![square(5.6380, lat, 0.0002)], floors: 4, ..Default::default() },
            B { id: 102, rings: vec![square(5.6900, lat, 0.0002)], floors: 4, ..Default::default() },
            // Unmeasured, among the measured on the other side of the edge (300 m).
            B { id: 103, rings: vec![square(5.6255, lat - 0.0017, 0.0002)], ..Default::default() },
        ];
        for k in 0..6 {
            theirs.push(B {
                id: 120 + k,
                rings: vec![square(5.6262 + 0.0003 * k as f64, lat - 0.001, 0.0002)],
                height: 14.0,
                ..Default::default()
            });
        }
        let mut files: Vec<Option<WorkFile>> = (0..9).map(|_| None).collect();
        files[4] = Some(work(d, t, &mine, &parts));
        files[5] = Some(work(d, e, &theirs, &[]));
        files
    }

    pub type Tiles = Vec<(u8, u32, u32, Vec<u8>, u32)>;

    pub fn collect(out: &mut Tiles) -> impl FnMut(u8, u32, u32, &[u8], u32) -> Result<()> + '_ {
        |z, x, y, gz, raw| {
            out.push((z, x, y, gz.to_vec(), raw));
            Ok(())
        }
    }

    /// The z8 tiles out (`Offers::out`).
    pub fn out<'b>(o: &'b Offers) -> &'b BTreeMap<usize, offload::Offered> {
        &o.out
    }

    /// The buildings each z8 tile out has (`Offers::records`).
    pub fn records<'b>(o: &'b Offers) -> &'b BTreeMap<usize, u64> {
        &o.records
    }

    /// About how long z8 tile `i` would take here (`Offers::here_s`).
    pub fn here_s(o: &Offers, i: usize) -> Option<f64> {
        o.here_s(i)
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{collect, coverage, files, Tiles};
    use super::*;

    #[test]
    fn a_z8_tiles_task_makes_the_tiles_the_whole_files_make() {
        let d = tempfile::tempdir().unwrap();
        let t = TileId { z: 6, x: 32, y: 22 };
        let files = files(d.path());
        let cov = coverage();
        let z8s = job::z8_tiles_of(&files);
        assert_eq!(z8s, [(129, 88), (131, 88)]);
        // A heritage site in the 45 m tower.
        let sites = [[crate::bld::e7(5.6235), crate::bld::e7(48.80)]];
        let mut all: Tiles = Vec::new();
        let whole = job::tiles_of(&files, &cov, t, &sites, &mut collect(&mut all)).unwrap();
        let mut merged = Summary::default();
        let mut by_z8: Tiles = Vec::new();
        for &a in &z8s {
            let a8 = TileId { z: 8, x: a.0, y: a.1 };
            let mut here: Tiles = Vec::new();
            let (s, f) = job::z8_tiles(&files, &cov, t, a, &sites, &mut collect(&mut here)).unwrap();
            merged.merge(&s);
            assert!(!f.tiles.is_empty() && f.mid.is_some());
            // Cut, run as the program runs, taken back: the same tiles and summary.
            let dir = d.path().join(a8.dash());
            let c = cut(&files, &cov, t, a, &sites, &dir).unwrap();
            let got = run(&dir, a8).unwrap();
            assert_eq!(got, s);
            let mut back: Tiles = Vec::new();
            assert_eq!(take_z8(&dir, a8, &mut collect(&mut back)).unwrap(), (s, f));
            assert_eq!(back, here, "z8 tile {}", a8.slash());
            assert!(!here.is_empty());
            // Only the shapes that can answer there, in order; the z8 tile's own records counted.
            let tc = read_coverage(&dir.join(COVERAGE)).unwrap();
            let countries: Vec<&str> = tc.shapes.iter().map(|s| s.country.as_str()).collect();
            if a == (131, 88) {
                assert_eq!(countries, ["FR", "ES"]);
                assert!(dir.join("6-33-22.sect").is_file(), "the neighbour's blocks");
                assert_eq!(c.records, 14);
                // The neighbour's building copied into the z8 tile; the floors of one in the circles'
                // overlap by France's storeys (the first shape), not Spain's.
                assert!(here.iter().any(|tl| props(&tl.3).contains(&(180, 0, 1))), "a copy of the neighbour's");
                let fr = fill::floors_dm(5, fill::storey("FR")) as u64;
                assert_ne!(fr, fill::floors_dm(5, fill::storey("ES")) as u64);
                assert!(here.iter().any(|tl| tl.0 == 14 && props(&tl.3).contains(&(fr, 1, 0))));
            } else {
                assert_eq!(countries, ["GB"]);
                assert!(!dir.join("6-33-22.sect").exists());
            }
            assert!(c.files_mb() >= 1 && c.files_mb() == c.bytes.div_ceil(1 << 20));
            // On one thread: the same bytes.
            let one = d.path().join(format!("{}-1", a8.dash()));
            cut(&files, &cov, t, a, &sites, &one).unwrap();
            let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
            pool.install(|| run(&one, a8)).unwrap();
            assert!(same_files(&one, &dir));
            by_z8.extend(here);
        }
        // tiles_of: the z8 tiles' tiles in order, their summaries merged.
        assert_eq!(all, by_z8);
        assert_eq!(merged, whole);
        assert!(whole.tiles > 0 && whole.features > 0);
        assert_eq!(whole.heritage, 1, "the tower holding the site");
        // A worker's tile outside its z8 tile is refused.
        let a8 = TileId { z: 8, x: 131, y: 88 };
        let bad = d.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::copy(d.path().join(a8.dash()).join(SUMMARY), bad.join(SUMMARY)).unwrap();
        let mut w = ArchiveWriter::create(&bad.join(TILES), "{}").unwrap();
        w.add(14, 1, 1, b"x", 1).unwrap();
        w.finish().unwrap();
        assert!(take_z8(&bad, a8, &mut |_, _, _, _, _| Ok(())).is_err());
    }

    use crate::bld::fill;

    /// A tile's features' (h, s, o).
    fn props(gz: &[u8]) -> Vec<(u64, u64, u64)> {
        let t = names::mvt::Tile::decode(&names::mvt::gunzip_if_gzip(gz).unwrap()).unwrap();
        let l = &t.layers[0];
        l.features
            .iter()
            .map(|f| {
                let get = |k: &str| {
                    f.tags.chunks(2).find(|kv| l.keys[kv[0] as usize] == k).map_or(0, |kv| {
                        match l.values[kv[1] as usize] {
                            names::mvt::Value::Uint(v) => v,
                            _ => 0,
                        }
                    })
                };
                (get("h"), get("s"), get("o"))
            })
            .collect()
    }
}
