//! A terrain z6 tile's z8 subtrees as tasks (kind `terrainsub`; docs/workers.md §3, docs/steps.md
//! Terrain): a z6 tile (a z6 tile's levels z12 → z9, crate::terrain_pack::build_piece) is 16 z8
//! subtrees, each a z8 tile's z9–12 tiles near the coverage. A tile's making reads only its own raw
//! tile (the pool's base tile: crate::terrain_base) and its children, all within its subtree: so a
//! subtree made alone (a group of one, `groups`) makes each of its tiles as the whole z6 tile's run
//! does, and the z6 tile's run of the others the same as of the whole.
//!
//! A task's one file (`u/in.sect`; docs/formats.md) holds everything its group reads, cut by the
//! job from what it reads itself: the raw tiles (`r-<z>-<x>-<y>`, as the job's sampled them; one
//! no dataset has data in, absent), its meta the z6 tile, the group's levels and its tiles at their
//! place's finest zoom. A worker reads nothing else: no NAS, no network. Its run (the program `terrainsub`) writes the group's tiles (`u/hi.sect`) and
//! its mid's part (`u/mid.sect`, crate::terrain_pack::write_mid's), which the job takes into the
//! z6 tile as if made here: the same bytes.

use crate::offload::{Offered, Offload, Patience, Settled};
use crate::terrain_pack::{Made, Mid, RawSource, Sources};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

/// The task's kind (and what the coordinator keeps its cost under, with its group's first
/// subtree).
pub const KIND: &str = "terrainsub";
/// A task's input file in its folder.
pub const INPUT: &str = "in.sect";
/// Its outputs: the group's tiles, and its mid's part.
pub const HI: &str = "hi.sect";
pub const MID: &str = "mid.sect";
/// At most this many groups of a z6 tile out at once.
pub const MOST: usize = 3;
/// Whether a terrain job offers its next z6 tile's groups as it begins a z6 tile (a whole z6 tile's time
/// for a worker to make them, not the moments before the run needs them): while its agent's
/// placement is on (jobrt::jobs::ahead_on). Each group then says its head start (the
/// tiles the run makes before it needs the group, over the group's: `Offers::offer_after`), and the
/// coordinator gives it to a worker slower than this Mac only while that covers it
/// (crate::coord::task's `covered`). Off: the waiting rule gives a worker only what it would make
/// sooner than this Mac (`task::beats`), as before.
pub fn offer_ahead() -> bool {
    jobrt::jobs::ahead_on()
}
/// The files' format (4: the raw tiles by the rule, crate::terrain_pack::Fills, and the tiles at
/// their place's finest zoom, `Meta::native`. 5: the water's sea alone, as the terrain reads it:
/// no lake, river or course. 6: no water, task #975: the terrain keeps the source's heights at sea).
const FMT: u64 = 6;

/// A z6 tile's levels, or a part of them: (zoom, its tiles), z12 → z9.
pub type Levels = Vec<(u8, Vec<(u32, u32)>)>;

/// The z8 subtree tile z/x/y (z ≥ 8) is in.
pub fn sub_of(z: u8, x: u32, y: u32) -> (u32, u32) {
    (x >> (z - 8), y >> (z - 8))
}

/// Subtrees named `8/x/y,…`.
pub fn parse_subtrees(s: &str) -> Result<Vec<(u32, u32)>> {
    s.split(',')
        .map(|b| {
            codecs::legacy::TileId::parse(b.trim())
                .filter(|u| u.z == 8)
                .map(|u| (u.x, u.y))
                .with_context(|| format!("not a z8 subtree: {b}"))
        })
        .collect()
}

/// Subtrees as `parse_subtrees` reads them.
pub fn subtrees_arg(list: &[(u32, u32)]) -> String {
    list.iter().map(|(x, y)| format!("8/{x}/{y}")).collect::<Vec<_>>().join(",")
}

/// A z6 tile's subtrees, each a group of its own (a tile reads nothing past its subtree), in column
/// then row order.
pub fn groups(levels: &Levels) -> Vec<Vec<(u32, u32)>> {
    let subs: BTreeSet<(u32, u32)> =
        levels.iter().flat_map(|(z, t)| t.iter().map(move |&(x, y)| sub_of(*z, x, y))).collect();
    subs.into_iter().map(|s| vec![s]).collect()
}

/// `levels` cut to the subtrees `subs` (`keep`), or to the others (`!keep`).
pub fn levels_of(levels: &Levels, subs: &BTreeSet<(u32, u32)>, keep: bool) -> Levels {
    levels
        .iter()
        .map(|(z, t)| (*z, t.iter().copied().filter(|&(x, y)| subs.contains(&sub_of(*z, x, y)) == keep).collect()))
        .collect()
}

/// The tiles of `levels`.
fn count(levels: &Levels) -> usize {
    levels.iter().map(|(_, t)| t.len()).sum()
}

/// The raw tiles a run of `levels` reads: their own.
fn raw_read(levels: &Levels) -> BTreeSet<(u8, u32, u32)> {
    levels.iter().flat_map(|(z, t)| t.iter().map(move |&(x, y)| (*z, x, y))).collect()
}

/// A tile's section name (`prefix-z-x-y`).
fn name(prefix: &str, z: u8, x: u32, y: u32) -> String {
    format!("{prefix}-{z}-{x}-{y}")
}

/// What a task's file says, but for its sections.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    pub fmt: u64,
    pub step: String,
    /// The terrain's version (crate::terrain_pack::TERRAIN_V): a task of another isn't run.
    pub v: u64,
    pub piece: String,
    pub subtrees: String,
    pub levels: Levels,
    /// Its tiles at their place's finest zoom (crate::terrain_pack::Sources::native: rounded to
    /// the centimetre).
    pub native: Vec<(u8, u32, u32)>,
}

/// What `cut` wrote: the file's bytes, its tiles, the raw tiles in it, and the tiles a run of it
/// makes (those with a raw tile).
pub struct CutReport {
    pub bytes: u64,
    pub tiles: usize,
    pub raw: usize,
    pub made: BTreeSet<(u8, u32, u32)>,
}

/// Group `group`'s task file at `path`, of z6 tile `piece` (`levels`: the group's part of its levels),
/// from what the job's run reads (`raw`, `src`).
pub fn cut(
    piece: (u32, u32),
    group: &[(u32, u32)],
    levels: &Levels,
    raw: &dyn RawSource,
    src: &Sources,
    path: &Path,
) -> Result<CutReport> {
    use rayon::prelude::*;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let reads: Vec<(u8, u32, u32)> = raw_read(levels).into_iter().collect();
    let got: Vec<Result<Option<Vec<u8>>>> = reads.par_iter().map(|&(z, x, y)| raw.get(z, x, y).map(|r| r.0)).collect();
    let tiles: Vec<(u8, u32, u32)> = levels.iter().flat_map(|(z, t)| t.iter().map(move |&(x, y)| (*z, x, y))).collect();
    let meta = Meta {
        fmt: FMT,
        step: KIND.into(),
        v: crate::terrain_pack::TERRAIN_V as u64,
        piece: format!("6/{}/{}", piece.0, piece.1),
        subtrees: subtrees_arg(group),
        levels: levels.clone(),
        native: tiles.iter().copied().filter(|&(z, x, y)| src.native.is_some_and(|n| n(z, x, y))).collect(),
    };
    let mut w = store::sect::SectWriter::create(path, serde_json::to_value(&meta)?)?;
    let mut n_raw = 0;
    let mut made = BTreeSet::new();
    let own: BTreeSet<&(u8, u32, u32)> = tiles.iter().collect();
    for (&(z, x, y), g) in reads.iter().zip(got) {
        if let Some(b) = g.with_context(|| format!("raw tile {z}/{x}/{y}"))? {
            w.add(&name("r", z, x, y), &b)?;
            n_raw += 1;
            if own.contains(&(z, x, y)) {
                made.insert((z, x, y));
            }
        }
    }
    // (A tile with no raw tile is made from its children where one is: `prepare`.)
    let mut zs: Vec<u8> = levels.iter().map(|l| l.0).collect();
    zs.sort_unstable_by(|a, b| b.cmp(a));
    for z in zs {
        for &(tz, x, y) in tiles.iter().filter(|t| t.0 == z) {
            if (0..4u32).any(|k| made.contains(&(tz + 1, 2 * x + (k & 1), 2 * y + (k >> 1)))) {
                made.insert((tz, x, y));
            }
        }
    }
    let bytes = w.finish()?;
    Ok(CutReport { bytes, tiles: tiles.len(), raw: n_raw, made })
}

/// A task's file, open: what its group reads (crate::terrain_pack's sources, from it alone).
pub struct Inputs {
    r: store::sect::SectReader<store::range::PlainFile>,
    pub meta: Meta,
    /// The raw tiles it may read (`raw_read`): one asked for outside them is an error, never "none".
    reads: BTreeSet<(u8, u32, u32)>,
}

impl Inputs {
    pub fn open(path: &Path) -> Result<Inputs> {
        let what = || path.display().to_string();
        let r = store::sect::SectReader::open(store::range::PlainFile::open(path).with_context(what)?)
            .with_context(what)?;
        let meta: Meta =
            serde_json::from_value(r.meta().clone()).with_context(|| format!("{}: not a terrain task", what()))?;
        ensure!(meta.fmt == FMT && meta.step == KIND, "{}: not a terrain task of format {FMT}", what());
        ensure!(
            meta.v == crate::terrain_pack::TERRAIN_V as u64,
            "{}: a task of terrain version {}, not {}",
            what(),
            meta.v,
            crate::terrain_pack::TERRAIN_V
        );
        let reads = raw_read(&meta.levels);
        Ok(Inputs { r, meta, reads })
    }

    /// The z6 tile and the group's subtrees.
    pub fn piece(&self) -> Result<(u32, u32)> {
        let u = codecs::legacy::TileId::parse(&self.meta.piece)
            .filter(|u| u.z == 6)
            .context("a task's piece isn't a z6 tile")?;
        Ok((u.x, u.y))
    }

    /// The group's tiles made (crate::terrain_pack::make_piece over its levels, with the sources
    /// the job's run has): its tiles, sorted, and its mid's part.
    pub fn make(&self) -> Result<(Vec<Made>, Mid)> {
        let native: std::collections::BTreeSet<(u8, u32, u32)> = self.meta.native.iter().copied().collect();
        let is = |z: u8, x: u32, y: u32| native.contains(&(z, x, y));
        let src = Sources { native: Some(&is), sets: &[] };
        crate::terrain_pack::make_piece(self, &src, self.meta.levels.clone())
    }
}

impl RawSource for Inputs {
    fn get(&self, z: u8, x: u32, y: u32) -> Result<(Option<Vec<u8>>, bool)> {
        ensure!(self.reads.contains(&(z, x, y)), "raw tile {z}/{x}/{y} isn't one of the task's");
        let n = name("r", z, x, y);
        Ok((if self.r.section(&n).is_some() { Some(self.r.read(&n)?) } else { None }, false))
    }
    fn prefetch_counted(
        &self,
        _z: u8,
        tiles: &[(u32, u32)],
        _threads: usize,
        done: &std::sync::atomic::AtomicU64,
    ) -> Result<usize> {
        done.fetch_add(tiles.len() as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(0)
    }
}

/// Writes a group's tiles (`hi`, sorted) to `path`: a sectioned file, meta `{"fmt": 1, "step":
/// "terrainsub", "z6 tile": "6/x/y", "subtrees": "8/x/y,…"}`, a section `t-<z>-<x>-<y>` a tile.
pub fn write_hi(path: &Path, piece: &str, subtrees: &str, hi: &[Made]) -> Result<()> {
    // (`piece`: the key of a z6 tile in the task file's meta, kept: workers' older builds read it.)
    let meta = serde_json::json!({ "fmt": FMT, "step": KIND, "piece": piece, "subtrees": subtrees });
    let mut w = store::sect::SectWriter::create(path, meta)?;
    for (z, x, y, b) in hi {
        w.add(&name("t", *z, *x, *y), b)?;
    }
    w.finish()?;
    Ok(())
}

/// A group's tiles as a worker wrote them (`write_hi`), checked: its z6 tile's and group's, each
/// tile one of the group's levels, and every one whose raw tile the task had (`raw`: it makes each
/// of those, and no other).
pub fn read_hi(path: &Path, piece: &str, subtrees: &str, raw: &BTreeSet<(u8, u32, u32)>) -> Result<Vec<Made>> {
    let what = || path.display().to_string();
    let r =
        store::sect::SectReader::open(store::range::PlainFile::open(path).with_context(what)?).with_context(what)?;
    let m = r.meta();
    ensure!(
        m["fmt"].as_u64() == Some(FMT) && m["step"] == KIND && m["piece"] == piece && m["subtrees"] == subtrees,
        "{}: not {piece}'s {subtrees}",
        what()
    );
    let mut out = Vec::new();
    for s in r.sections() {
        let k: Vec<u32> = s
            .name
            .strip_prefix("t-")
            .map(|n| n.split('-').filter_map(|v| v.parse().ok()).collect())
            .unwrap_or_default();
        ensure!(k.len() == 3 && k[0] <= 12, "{}: a section {:?}", what(), s.name);
        let t = (k[0] as u8, k[1], k[2]);
        ensure!(raw.contains(&t), "{}: {}/{}/{} isn't a tile of the task's", what(), t.0, t.1, t.2);
        out.push((t.0, t.1, t.2, r.read(&s.name).with_context(what)?));
    }
    ensure!(out.len() == raw.len(), "{}: {} tiles, not {}", what(), out.len(), raw.len());
    out.sort_by_key(|t| (t.0, t.1, t.2));
    Ok(out)
}

/// Whether two runs' tiles and mids are the same bytes (the mids' values bit for bit).
#[cfg(test)]
fn same(a: &(Vec<Made>, Mid), b: &(Vec<Made>, Mid)) -> bool {
    let bits = |m: &Mid| {
        m.quads.iter().map(|(k, v)| (*k, v.iter().map(|f| f.to_bits()).collect::<Vec<_>>())).collect::<Vec<_>>()
    };
    a.0 == b.0 && bits(&a.1) == bits(&b.1)
}

/// A group's task spec (the shape of a tail's, crate::offload): the program `terrainsub` over its
/// folder; `unit` its first subtree (what its cost is kept under), and its subtrees.
pub fn spec(group: &[(u32, u32)], version: &str, input: u64) -> serde_json::Value {
    let args = ["--in", &format!("{{dir}}/{INPUT}"), "--out", "{dir}"];
    let run = crate::unit::Run {
        what: KIND.into(),
        prog: "terrainsub".into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        env: vec![],
        reads: vec![format!("{{dir}}/{INPUT}")],
    };
    let first = group.first().map(|(x, y)| format!("8/{x}/{y}")).unwrap_or_default();
    // (`unit`: the task spec's key for its tile, kept: docs/workers.md §3.)
    serde_json::json!({ "unit": first, "subtrees": subtrees_arg(group), "version": version, "runs": [run], "inputs": [[format!("u/{INPUT}"), input]], "places": crate::offload::places() })
}

/// A group's predicted memory on a worker (MB), until a worker measures it (the coordinator's
/// `terrainsub 8/x/y`): its file twice (the worker holds it, and the program reads its sections), and
/// its levels' tiles as the run holds them, a level at a time (each its raw tile, its elevations
/// three times over and AWS's z9 tile over it: ~1.3 MB at most), with room.
pub fn mem_mb(input: u64, levels: &Levels) -> u64 {
    let most = levels.iter().map(|(_, t)| t.len()).max().unwrap_or(0) as u64;
    (input >> 20) * 2 + most * 13 / 10 + 120
}

/// A group out: its task, subtrees, levels, and the tiles it makes.
struct Out {
    task: Offered,
    group: Vec<(u32, u32)>,
    levels: Levels,
    made: BTreeSet<(u8, u32, u32)>,
}

/// What the job runs a group's levels with here (crate::terrain_pack::make_piece over them).
pub type Here<'a> = &'a dyn Fn(Levels) -> Result<(Vec<Made>, Mid)>;

/// A z6 tile's groups out as tasks (crate::offload), as tree cover's rows are: offered from the far
/// end (its last groups) when the run begins, one per worker
/// that takes them around, at most `MOST`, never every group; the run makes the others (`rest`),
/// then settles each, waiting for no worker longer than this Mac would take (`Patience`: its tiles
/// at the pace of the run's here), its result taken.
pub struct Offers<'a> {
    offload: Option<&'a Offload>,
    piece: (u32, u32),
    out: Vec<Out>,
    /// The run's own tiles here: their time (seconds, wall) and count.
    here: std::sync::Mutex<(f64, usize)>,
}

impl<'a> Offers<'a> {
    /// Offers some of z6 tile `piece`'s groups (its levels `levels`, read from `raw` and `src`),
    /// their folders under the job's tasks' folder, while workers that take them are around: none
    /// without `offload`.
    pub fn offer(
        offload: Option<&'a Offload>,
        piece: (u32, u32),
        levels: &Levels,
        raw: &dyn RawSource,
        src: &Sources,
    ) -> Offers<'a> {
        Self::offer_after(offload, piece, levels, raw, src, 0)
    }

    /// `offer`, the run making `before` tiles of other z6 tiles before it begins this one (a job's
    /// next extract offered ahead: the extract under way's): each group's head start, told with it when
    /// the job offers ahead (`offer_ahead`), is the tiles made here before the run needs it (those,
    /// and this z6 tile's own it makes first) over the group's.
    pub fn offer_after(
        offload: Option<&'a Offload>,
        piece: (u32, u32),
        levels: &Levels,
        raw: &dyn RawSource,
        src: &Sources,
        before: usize,
    ) -> Offers<'a> {
        let mut o = Offers { offload, piece, out: Vec::new(), here: Default::default() };
        let Some(off) = offload else { return o };
        let workers = off.workers(KIND);
        if workers == 0 {
            return o;
        }
        let _p = jobrt::timings::phase("offering the z6 tile's subtrees to other workers");
        let groups = groups(levels);
        let depth = workers.min(MOST).min(groups.len().saturating_sub(1));
        let chosen: Vec<&Vec<(u32, u32)>> = groups.iter().rev().take(depth).collect();
        // (The run's own tiles of this z6 tile, made before it settles any group out.)
        let out_subs: BTreeSet<(u32, u32)> = chosen.iter().flat_map(|g| g.iter().copied()).collect();
        let own = count(&levels_of(levels, &out_subs, false));
        for g in chosen {
            let offered = (|| -> Result<Out> {
                let lv = levels_of(levels, &g.iter().copied().collect(), true);
                let root = off.task_root(&format!("terrainsub-{}-{}", g[0].0, g[0].1));
                let c = cut(piece, g, &lv, raw, src, &root.join("u").join(INPUT))?;
                let made = c.made;
                let ahead = (count(&lv) > 0).then(|| (before + own) as f64 / count(&lv) as f64);
                let task = off.offer_spec_ahead(
                    KIND,
                    spec(g, off.version(), c.bytes),
                    &root,
                    [(format!("u/{INPUT}"), c.bytes)].into(),
                    mem_mb(c.bytes, &lv),
                    ahead,
                )?;
                eprintln!(
                    "terrain 6/{}/{}: subtrees {} offered ({} tiles, {:.1} MB)",
                    piece.0,
                    piece.1,
                    subtrees_arg(g),
                    c.tiles,
                    c.bytes as f64 / 1e6
                );
                Ok(Out { task, group: g.clone(), levels: lv, made })
            })();
            match offered {
                Ok(x) => o.out.push(x),
                Err(e) => {
                    eprintln!(
                        "terrain 6/{}/{}: subtrees {} not offered ({e:#}); made here",
                        piece.0,
                        piece.1,
                        subtrees_arg(g)
                    );
                    break;
                }
            }
        }
        o
    }

    /// The z6 tile offered.
    pub fn piece(&self) -> (u32, u32) {
        self.piece
    }

    /// The levels the run makes itself: those of the groups not out.
    pub fn rest(&self, levels: &Levels) -> Levels {
        let out: BTreeSet<(u32, u32)> = self.out.iter().flat_map(|o| o.group.iter().copied()).collect();
        levels_of(levels, &out, false)
    }

    /// The run made `tiles` of its own in `secs`.
    pub fn made_here(&self, secs: f64, tiles: usize) {
        let mut h = self.here.lock().unwrap();
        *h = (h.0 + secs, h.1 + tiles);
    }

    /// About how long `n` tiles would take here (seconds), at the pace of the run's own.
    fn here_s(&self, n: usize) -> Option<f64> {
        let (secs, k) = *self.here.lock().unwrap();
        (k > 0).then(|| secs / k as f64 * n as f64)
    }

    /// Settles each group out (`here` runs a group's levels here), its tiles added to `hi` and its
    /// mid's to `mid`.
    pub fn settle(self, here: Here, hi: &mut Vec<Made>, mid: &mut Mid) -> Result<()> {
        for o in &self.out {
            let (h, m) = self.settle_one(o, here)?;
            hi.extend(h);
            mid.quads.extend(m.quads);
        }
        Ok(())
    }

    fn settle_one(&self, o: &Out, here: Here) -> Result<(Vec<Made>, Mid)> {
        let Some(off) = self.offload else { return here(o.levels.clone()) };
        let (piece, subs) = (format!("6/{}/{}", self.piece.0, self.piece.1), subtrees_arg(&o.group));
        let what = format!("terrain {piece}: subtrees {subs}");
        let theirs = |st: &serde_json::Value| -> Result<(Vec<Made>, Mid)> {
            let d = Path::new(st["out"].as_str().context("no outputs' folder")?).join("u");
            let hi = read_hi(&d.join(HI), &piece, &subs, &o.made)?;
            let (t, mid) = crate::terrain_pack::read_mid(&d.join(MID))?;
            ensure!(t == self.piece, "a mid of 6/{}/{}", t.0, t.1);
            ensure!(
                mid.quads.keys().all(|&(x, y)| o.group.contains(&sub_of(9, x, y))),
                "a mid with another group's tiles"
            );
            Ok((hi, mid))
        };
        let got: std::cell::RefCell<Option<(Vec<Made>, Mid)>> = Default::default();
        let mut run_here = || -> Result<()> {
            *got.borrow_mut() = Some(here(o.levels.clone())?);
            Ok(())
        };
        let mut take = |st: &serde_json::Value| -> Result<()> {
            match theirs(st) {
                Ok(r) => *got.borrow_mut() = Some(r),
                Err(e) => {
                    eprintln!("{what}: a worker's result doesn't read ({e:#}); made here");
                    *got.borrow_mut() = Some(here(o.levels.clone())?);
                }
            }
            Ok(())
        };
        let p = Patience { here_s: self.here_s(count(&o.levels)) };
        let how = match off.settle_with(&o.task, true, p, &mut run_here, &mut take) {
            Ok(Some(h)) => h,
            Ok(None) => unreachable!("a task waited on is settled"),
            Err(e) => {
                // (The coordinator gone: made here.)
                eprintln!("{what}: its task can't be settled ({e:#}); made here");
                std::fs::remove_dir_all(&o.task.root).ok();
                *got.borrow_mut() = Some(here(o.levels.clone())?);
                Settled::Here
            }
        };
        eprintln!("{what} made {}", how.how());
        let r = got.into_inner();
        match r {
            Some(r) => Ok(r),
            None => here(o.levels.clone()),
        }
    }
}

#[cfg(test)]
pub mod testkit {
    use super::*;
    use crate::terrain_pack::RawTiles;
    use buildstore::coverage::Coverage;

    /// A z6 tile in the far north, the coverage at a corner of four of its subtrees (12.656°E, 70.10°N: z8 columns 136–137, rows 56–57, in z6
    /// tile 6/34/14), and its raw tiles under `d`: the z6 tile, its coverage, its raw tiles' cache.
    pub fn north_piece(d: &Path) -> ((u32, u32), Coverage, RawTiles) {
        let cov = Coverage::fixture(
            &[buildstore::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:12.656,70.10,1".into()],
            }],
            None,
            d,
        )
        .unwrap();
        let by_q = buildstore::coverage::coverage_tiles(&cov);
        let (&q, ts) = by_q.iter().next().unwrap();
        assert!(ts.contains(&(34, 14)), "{ts:?}");
        crate::terrain_pack::testkit::synthetic_raw(&d.join("local"), &cov, q, &[(34, 14)]);
        (
            (34, 14),
            cov,
            RawTiles::with_store(
                &d.join("local"),
                &store::nasfs::NasPath::local_for_test(&d.join("store")),
                crate::terrain_pack::TEST_BASE,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::north_piece;
    use super::*;
    use crate::terrain_pack::piece_levels;
    use buildstore::out::Out;

    /// Paused mid-sampling (task #705: jobrt::control::Paused, between the pool's chunks), a z6 tile
    /// saves nothing, its blocks made so far kept in its log; resumed, the packs of a run never
    /// paused.
    #[test]
    fn a_z6_tile_paused_mid_sampling_resumes_to_the_same_packs() {
        use crate::terrain_pack::tests::{files_under, PauseAfter};
        let d = tempfile::tempdir().unwrap();
        let (t, cov, raw) = north_piece(d.path());
        let src = Sources { sets: crate::terrain_zoom::test_world_ref(), ..Default::default() };
        let saved = |root: &Path, scratch: &Path| -> Vec<(String, String)> {
            let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(root), scratch).unwrap();
            out.manifest
                .into_iter()
                .filter(|(l, _)| l.starts_with("layers/terrain/") || l.starts_with("work/terrain-mid/"))
                .collect()
        };
        let fresh = {
            let (root, scratch) = (d.path().join("fresh"), d.path().join("fresh-scratch"));
            let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            crate::terrain_pack::build_piece_keeping(&mut out, &raw, t, &cov, &src).unwrap();
            saved(&root, &scratch)
        };
        assert_eq!(fresh.len(), 2, "{fresh:?}");
        let (mut paused, mut logged) = (0, 0);
        for left in [0, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233, 377] {
            let name = format!("paused-{left}");
            let (root, scratch) = (d.path().join(&name), d.path().join(format!("{name}-scratch")));
            let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            let p = PauseAfter::new(&raw, left);
            let Err(e) = crate::terrain_pack::build_piece_keeping(&mut out, &p, t, &cov, &src) else { break };
            assert!(jobrt::control::is_paused(&e), "{e:#}");
            assert!(saved(&root, &scratch).is_empty(), "paused after {left}: nothing saved");
            let packs = files_under(&root).into_iter().filter(|f| f.to_string_lossy().contains("terrain")).count();
            assert_eq!(packs, 0, "paused after {left}: no pack");
            paused += 1;
            logged += jobrt::resume_log::path(&scratch, (6, t.0, t.1)).exists() as usize;
            // Resumed: the same packs.
            let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            crate::terrain_pack::build_piece_keeping(&mut out, &raw, t, &cov, &src).unwrap();
            assert_eq!(saved(&root, &scratch), fresh, "paused after {left}, resumed");
        }
        assert!(paused >= 3 && logged >= 1, "{paused} paused, {logged} with blocks kept");
    }

    /// A z6 tile stopped after its first safe point (a block kept, or a level's end), its
    /// second, midway, before its last and after it, each resumed: the same packs as a run never
    /// stopped. (After each one in turn before #969, 143 s on the test Mac, a whole run each.)
    #[test]
    fn a_z6_tile_stopped_and_resumed_makes_the_same_packs() {
        use jobrt::resume_log::STOP_AFTER;
        let d = tempfile::tempdir().unwrap();
        let (t, cov, raw) = north_piece(d.path());
        let levels = piece_levels(&cov, &crate::terrain_zoom::test_world(12), t);
        // (Its packs and mid as saved, their content names the bytes' digests; None when stopped.
        // Not stopped, also the safe points it passed: a stop never reached counts them.)
        let counted = std::cell::Cell::new(0);
        let run = |name: &str, src: &Sources, stop: Option<usize>| -> Option<Vec<(String, String)>> {
            let root = d.path().join(name);
            let scratch = d.path().join(format!("{name}-scratch"));
            let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            STOP_AFTER.set(Some(stop.unwrap_or(usize::MAX)));
            let r = crate::terrain_pack::build_piece_keeping(&mut out, &raw, t, &cov, src);
            let left = STOP_AFTER.take();
            let log = jobrt::resume_log::path(&scratch, (6, t.0, t.1));
            if let Err(e) = r {
                assert!(jobrt::control::is_paused(&e) && log.exists(), "{e:#}");
                return None;
            }
            if stop.is_none() {
                counted.set(usize::MAX - left.unwrap());
            }
            assert!(!log.exists(), "the log gone once written");
            let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            Some(
                out.manifest
                    .into_iter()
                    .filter(|(l, _)| l.starts_with("layers/terrain/") || l.starts_with("work/terrain-mid/"))
                    .collect(),
            )
        };
        let with = Sources { sets: crate::terrain_zoom::test_world_ref(), ..Default::default() };
        let every: &(dyn Fn(u8, u32, u32) -> bool + Sync) = &|_, _, _| true;
        let without = Sources { native: Some(every), sets: crate::terrain_zoom::test_world_ref() };
        let fresh = run("fresh", &with, None).unwrap();
        assert_eq!(fresh.len(), 2, "{fresh:?}");
        let kept = counted.get();
        assert_eq!(kept, 11, "its blocks and level ends (z12–9), but the last");
        // Stopped after its first safe point, its second, midway, before its last and after it,
        // then resumed: the same bytes.
        for stop in std::collections::BTreeSet::from([1, 2, kept / 2, kept - 1, kept]) {
            let name = format!("stop-{stop}");
            assert!(run(&name, &with, Some(stop)).is_none(), "stopped after {stop}");
            assert_eq!(run(&name, &with, None).as_ref(), Some(&fresh), "stopped after {stop}");
        }
        // Stopped twice, the second time further on.
        assert!(run("twice", &with, Some(2)).is_none() && run("twice", &with, Some(3)).is_none());
        assert_eq!(run("twice", &with, None).as_ref(), Some(&fresh), "stopped twice");
        // Another input (every tile at its finest zoom) voids the log: that run's bytes, not the log's.
        let dry = run("dry", &without, None).unwrap();
        assert_ne!(dry, fresh);
        assert!(run("voided", &with, Some(kept - 1)).is_none());
        assert_eq!(run("voided", &without, None), Some(dry));
    }

    #[test]
    fn a_groups_tiles_made_from_its_file_are_the_pieces_bytes() {
        let d = tempfile::tempdir().unwrap();
        let (t, cov, raw) = north_piece(d.path());
        let levels = piece_levels(&cov, &crate::terrain_zoom::test_world(12), t);
        let gs = groups(&levels);
        assert_eq!(gs, [vec![(136, 56)], vec![(136, 57)], vec![(137, 56)], vec![(137, 57)]]);
        let src = Sources { sets: crate::terrain_zoom::test_world_ref(), ..Default::default() };
        // The whole z6 tile, and each group from its file: the same tiles, the same mid.
        let whole = crate::terrain_pack::make_piece(&raw, &src, levels.clone()).unwrap();
        // Each level's tiles all made at once (the way before task #447), and in blocks of one
        // tile: the same bytes.
        assert!(same(&crate::terrain_pack::make_piece_whole(&raw, &src, levels.clone()).unwrap(), &whole), "whole");
        for up in 0..4 {
            let by = crate::terrain_pack::make_piece_up(&raw, &src, levels.clone(), up).unwrap();
            assert!(same(&by, &whole), "blocks {up} levels up");
        }
        let (mut hi, mut mid) = (Vec::new(), Mid::default());
        for g in &gs {
            let lv = levels_of(&levels, &g.iter().copied().collect(), true);
            let path = d.path().join(format!("t-{}-{}", g[0].0, g[0].1)).join(INPUT);
            let c = cut(t, g, &lv, &raw, &src, &path).unwrap();
            let inp = Inputs::open(&path).unwrap();
            let one = inp.make().unwrap();
            // (Written and read back as the job takes them.)
            write_hi(&d.path().join("hi.sect"), &inp.meta.piece, &inp.meta.subtrees, &one.0).unwrap();
            assert_eq!(
                read_hi(&d.path().join("hi.sect"), &inp.meta.piece, &inp.meta.subtrees, &c.made).unwrap(),
                one.0
            );
            assert!(read_hi(&d.path().join("hi.sect"), &inp.meta.piece, "8/1/1", &c.made).is_err(), "another group's");
            let alone = crate::terrain_pack::make_piece(&raw, &src, lv).unwrap();
            assert!(same(&one, &alone), "group {}: from its file as made here", subtrees_arg(g));
            hi.extend(one.0);
            mid.quads.extend(one.1.quads);
        }
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        assert!(same(&(hi, mid), &whole), "the groups make the piece");
        // A tile the task wasn't cut for is refused, not "none".
        let g = &gs[0];
        let inp = Inputs::open(&d.path().join(format!("t-{}-{}", g[0].0, g[0].1)).join(INPUT)).unwrap();
        assert!(RawSource::get(&inp, 12, 0, 0).is_err());
        let (a, b) = (gs[0][0], gs[1][0]);
        assert_eq!(parse_subtrees(&subtrees_arg(&[a, b])).unwrap(), [a, b]);
    }
}
