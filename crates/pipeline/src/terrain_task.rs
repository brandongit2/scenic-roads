//! A terrain z6 tile's z8 subtrees as tasks (kind `terrainsub`; docs/workers.md §3, docs/steps.md
//! Terrain): a z6 tile (a z6 tile's levels z12 → z9, crate::terrain_pack::build_piece) is 16 z8
//! subtrees, each a z8 tile's z9–12 tiles near the coverage. A tile's making reads only its own raw
//! tile (the pool's base tile: crate::terrain_base) and its children, all within its subtree, and
//! the water at its place; but a lake's level is one, from all its shore in the z6 tile's level that
//! first has it. So the subtrees go out in groups closed under the lakes they share (a lake whose
//! water reaches two subtrees' tiles puts them in one group): a group made alone then makes each of
//! its tiles, and its lakes' levels, as the whole z6 tile's run does, and the z6 tile's run of the
//! others the same as of the whole (`groups`).
//!
//! A task's one file (`u/in.sect`; docs/formats.md) holds everything its group reads, cut by the
//! job from what it reads itself: the raw tiles (`r-<z>-<x>-<y>`, as the job's sampled them; one
//! no dataset has data in, absent), the water's polygons as the terrain reads them (`w-…`: `WaterSource::terrain_polys`), its meta
//! the z6 tile, the group's levels and whether the job's run has the water. A worker reads nothing else: no
//! NAS, no network. Its run (the program `terrainsub`) writes the group's tiles (`u/hi.sect`) and
//! its mid's part (`u/mid.sect`, crate::terrain_pack::write_mid's), which the job takes into the
//! z6 tile as if made here: the same bytes.

use crate::offload::{Offered, Offload, Patience, Settled};
use crate::terrain_pack::{Made, Mid, RawSource, Sources};
use crate::terrain_water::{Kind, Poly, WaterSource};
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// placement is on (crate::agent::placement::ahead_on). Each group then says its head start (the
/// tiles the run makes before it needs the group, over the group's: `Offers::offer_after`), and the
/// coordinator gives it to a worker slower than this Mac only while that covers it
/// (crate::coord::task's `covered`). Off: the waiting rule gives a worker only what it would make
/// sooner than this Mac (`task::beats`), as before.
pub fn offer_ahead() -> bool {
    crate::agent::placement::ahead_on()
}
/// A group of more subtrees than this isn't offered (its memory, and the z6 tile's run waiting on it).
pub const MOST_SUBTREES: usize = 2;
/// The files' format (2: no GLO-30 windows: the raw tiles are the pool's, crate::terrain_base; 3:
/// the z10 tiles deeper than the base, `Meta::deeper`).
const FMT: u64 = 3;

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
            crate::legacy::TileId::parse(b.trim())
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

/// A z6 tile's subtrees in groups closed under the lakes they share (any lake key of a tile's water,
/// its own keys too: the z6 tile's run gathers a lake's shore by its key, whatever it is), each
/// group's subtrees in column then row order, the groups by their first. Without water, each
/// subtree alone.
pub fn groups(levels: &Levels, water: Option<&dyn WaterSource>) -> Result<Vec<Vec<(u32, u32)>>> {
    use rayon::prelude::*;
    let subs: BTreeSet<(u32, u32)> =
        levels.iter().flat_map(|(z, t)| t.iter().map(move |&(x, y)| sub_of(*z, x, y))).collect();
    let index: BTreeMap<(u32, u32), usize> = subs.iter().enumerate().map(|(i, s)| (*s, i)).collect();
    let mut parent: Vec<usize> = (0..subs.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    if let Some(w) = water {
        let tiles: Vec<(u8, u32, u32)> =
            levels.iter().flat_map(|(z, t)| t.iter().map(move |&(x, y)| (*z, x, y))).collect();
        let lakes: Vec<Result<Vec<u64>>> = tiles
            .par_iter()
            .map(|&(z, x, y)| {
                Ok(w.polys(z, x, y)?.into_iter().filter(|p| p.kind == Kind::Lake).map(|p| p.id).collect())
            })
            .collect();
        let mut first: HashMap<u64, usize> = HashMap::new();
        for (&(z, x, y), ids) in tiles.iter().zip(lakes) {
            let s = index[&sub_of(z, x, y)];
            for id in ids.with_context(|| format!("the water of {z}/{x}/{y}"))? {
                let f = *first.entry(id).or_insert(s);
                let (a, b) = (root(&mut parent, f), root(&mut parent, s));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
    }
    let mut by: BTreeMap<usize, Vec<(u32, u32)>> = BTreeMap::new();
    for (s, &i) in &index {
        let r = root(&mut parent, i);
        by.entry(r).or_default().push(*s);
    }
    // (Roots are their group's least index, so the groups come by their first subtree.)
    Ok(by.into_values().collect())
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

/// Polygons as a task holds them: each its kind (1 sea, 2 lake, 3 river, 4 a river's course), key, rings, each ring's points
/// (x, y f64), little-endian, counts u32.
pub fn polys_bytes(polys: &[Poly]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend((polys.len() as u32).to_le_bytes());
    for p in polys {
        b.push(match p.kind {
            Kind::Sea => 1,
            Kind::Lake => 2,
            Kind::River => 3,
            Kind::Flow => 4,
        });
        b.extend(p.id.to_le_bytes());
        b.extend((p.rings.len() as u32).to_le_bytes());
        for r in &p.rings {
            b.extend((r.len() as u32).to_le_bytes());
            for pt in r {
                b.extend(pt[0].to_le_bytes());
                b.extend(pt[1].to_le_bytes());
            }
        }
    }
    b
}

/// `polys_bytes` read back.
pub fn polys_from(b: &[u8]) -> Result<Vec<Poly>> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let s = b.get(at..at + n).context("water polygons cut short")?;
        at += n;
        Ok(s)
    };
    let u32_ = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap()) as usize;
    let n = u32_(take(4)?);
    let mut out = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let kind = match take(1)?[0] {
            1 => Kind::Sea,
            2 => Kind::Lake,
            3 => Kind::River,
            4 => Kind::Flow,
            k => bail!("a water polygon of kind {k}"),
        };
        let id = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let nr = u32_(take(4)?);
        let mut rings = Vec::with_capacity(nr.min(1 << 16));
        for _ in 0..nr {
            let np = u32_(take(4)?);
            let mut r = Vec::with_capacity(np.min(1 << 20));
            for _ in 0..np {
                let x = f64::from_le_bytes(take(8)?.try_into().unwrap());
                let y = f64::from_le_bytes(take(8)?.try_into().unwrap());
                r.push([x, y]);
            }
            rings.push(r);
        }
        out.push(Poly { kind, id, rings });
    }
    ensure!(at == b.len(), "water polygons with {} bytes more", b.len() - at);
    Ok(out)
}

/// What a task's file says, but for its sections.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    pub fmt: u64,
    pub step: String,
    /// The terrain's version (crate::agent::build::TERRAIN_V): a task of another isn't run.
    pub v: u64,
    pub piece: String,
    pub subtrees: String,
    pub levels: Levels,
    /// Whether the job's run has the water (crate::terrain_pack::Sources).
    pub water: bool,
    /// The z10 tiles of its z6 tile whose native zoom is past the base's
    /// (crate::terrain_pack::Sources::deeper: its tiles there rounded as coarser than native).
    pub deeper: Vec<(u32, u32)>,
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
    let water: Vec<Result<Vec<Poly>>> = match src.water {
        Some(w) => tiles.par_iter().map(|&(z, x, y)| w.terrain_polys(z, x, y)).collect(),
        None => Vec::new(),
    };
    let meta = Meta {
        fmt: FMT,
        step: KIND.into(),
        v: crate::agent::build::TERRAIN_V as u64,
        piece: format!("6/{}/{}", piece.0, piece.1),
        subtrees: subtrees_arg(group),
        levels: levels.clone(),
        water: src.water.is_some(),
        deeper: src
            .deeper
            .map(|d| d.iter().copied().filter(|&(x, y)| (x >> 4, y >> 4) == piece).collect())
            .unwrap_or_default(),
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
    for (&(z, x, y), p) in tiles.iter().zip(water) {
        let p = p.with_context(|| format!("the water of {z}/{x}/{y}"))?;
        if !p.is_empty() {
            w.add(&name("w", z, x, y), &polys_bytes(&p))?;
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
            meta.v == crate::agent::build::TERRAIN_V as u64,
            "{}: a task of terrain version {}, not {}",
            what(),
            meta.v,
            crate::agent::build::TERRAIN_V
        );
        let reads = raw_read(&meta.levels);
        Ok(Inputs { r, meta, reads })
    }

    /// The z6 tile and the group's subtrees.
    pub fn piece(&self) -> Result<(u32, u32)> {
        let u = crate::legacy::TileId::parse(&self.meta.piece)
            .filter(|u| u.z == 6)
            .context("a task's piece isn't a z6 tile")?;
        Ok((u.x, u.y))
    }

    /// The group's tiles made (crate::terrain_pack::make_piece over its levels, with the sources
    /// the job's run has): its tiles, sorted, and its mid's part.
    pub fn make(&self) -> Result<(Vec<Made>, Mid)> {
        let deeper: std::collections::BTreeSet<(u32, u32)> = self.meta.deeper.iter().copied().collect();
        let src = Sources { water: self.meta.water.then_some(self as &dyn WaterSource), deeper: Some(&deeper) };
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

impl WaterSource for Inputs {
    /// The terrain's water less its rivers.
    fn polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
        let mut p = self.terrain_polys(z, x, y)?;
        p.retain(|p| matches!(p.kind, Kind::Sea | Kind::Lake));
        Ok(p)
    }
    /// The job's `terrain_polys`, as cut.
    fn terrain_polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
        let n = name("w", z, x, y);
        match self.r.section(&n) {
            Some(_) => polys_from(&self.r.read(&n)?),
            None => Ok(Vec::new()),
        }
    }
    fn pin(&self) -> String {
        "a terrain task's".into()
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
pub fn same(a: &(Vec<Made>, Mid), b: &(Vec<Made>, Mid)) -> bool {
    let bits = |m: &Mid| {
        (
            m.quads.iter().map(|(k, v)| (*k, v.iter().map(|f| f.to_bits()).collect::<Vec<_>>())).collect::<Vec<_>>(),
            m.levels.iter().map(|(k, v)| (*k, v.to_bits())).collect::<Vec<_>>(),
        )
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
/// `terrainsub 8/x/y`): its file twice (the page holds it, and the program reads its sections), and
/// its levels' tiles as the run holds them, a level at a time (each its raw tile, its elevations
/// three times over, its water and AWS's z9 tile over it: ~1.3 MB at most), with room.
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
/// end (its last groups of at most `MOST_SUBTREES` subtrees) when the run begins, one per worker
/// that takes them around, at most `MOST`, never every group; the run makes the others (`rest`),
/// then settles each, waiting for no worker longer than this Mac would take (`Patience`: its tiles
/// at the pace of the run's here), its result taken, or checked against this Mac's run of the group
/// byte for byte, a difference marking the worker bad.
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
        let _p =
            crate::timings::phase("offering the z6 tile's subtrees to other workers", crate::timings::Class::Mixed);
        let groups = match groups(levels, src.water) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("terrain 6/{}/{}: its subtrees' lakes can't be read ({e:#}): none offered", piece.0, piece.1);
                return o;
            }
        };
        let depth = workers.min(MOST).min(groups.len().saturating_sub(1));
        let chosen: Vec<&Vec<(u32, u32)>> =
            groups.iter().rev().filter(|g| g.len() <= MOST_SUBTREES).take(depth).collect();
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
            mid.levels.extend(m.levels);
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
        let mut same_ = |st: &serde_json::Value, _: std::time::SystemTime| -> Result<bool> {
            let s = match (theirs(st), got.borrow().as_ref()) {
                (Ok(t), Some(m)) => same(&t, m),
                _ => false,
            };
            if !s {
                eprintln!("{what}: a worker's tiles differ from this Mac's");
            }
            Ok(s)
        };
        let p = Patience { here_s: self.here_s(count(&o.levels)) };
        let how = match off.settle_with(&o.task, true, p, &mut run_here, &mut take, &mut same_) {
            Ok(Some(h)) => h,
            Ok(None) => unreachable!("a task waited on is settled"),
            Err(e) => {
                // (The coordinator gone: made here.)
                eprintln!("{what}: its task can't be settled ({e:#}); made here");
                std::fs::remove_dir_all(&o.task.root).ok();
                *got.borrow_mut() = Some(here(o.levels.clone())?);
                Settled::Here(None)
            }
        };
        let how = match how {
            Settled::Remote(w) => format!("by {w}"),
            Settled::Here(None) => "here".into(),
            Settled::Here(Some((w, true))) => format!("here, and by {w} the same"),
            Settled::Here(Some((w, false))) => format!("here: {w}'s differed, and it gets no more work"),
        };
        eprintln!("{what} made {how}");
        let r = got.into_inner();
        match r {
            Some(r) => Ok(r),
            None => here(o.levels.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::client::Client;
    use crate::coverage::Coverage;
    use crate::out::Out;
    use crate::terrain_pack::{build_piece_with, mid_logical, piece_levels, read_mid, RawTiles};
    use std::path::PathBuf;

    /// Water across two subtrees' edge (`a` west or north of `b`): a lake straddling it (key 9),
    /// one inside `a` alone (key 11), and the sea in the far north-west corner of `a`, in every
    /// tile from z6 (as the basemap draws them).
    struct Across {
        a: (u32, u32),
        b: (u32, u32),
    }

    /// A z8 tile's box in degrees (west, south, east, north).
    fn z8_box((x, y): (u32, u32)) -> [f64; 4] {
        let n = 256.0;
        let lat = |v: f64| (std::f64::consts::PI * (1.0 - 2.0 * v / n)).sinh().atan().to_degrees();
        [x as f64 / n * 360.0 - 180.0, lat(y as f64 + 1.0), (x + 1) as f64 / n * 360.0 - 180.0, lat(y as f64)]
    }

    impl WaterSource for Across {
        fn polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
            if z < 6 {
                return Ok(Vec::new());
            }
            let n = (1u64 << z) as f64 * 256.0;
            let px = |lon: f64| (lon + 180.0) / 360.0 * n - x as f64 * 256.0;
            let py =
                |lat: f64| (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n - y as f64 * 256.0;
            let rect = |kind, id, [w, s, e, nn]: [f64; 4]| Poly {
                kind,
                id,
                rings: vec![vec![[px(w), py(nn)], [px(e), py(nn)], [px(e), py(s)], [px(w), py(s)]]],
            };
            let (a, b) = (z8_box(self.a), z8_box(self.b));
            // (Near the coverage, at `a`'s south-east corner: the edge `b` east of `a`, or south.)
            let _ = b;
            let (ce, cs) = (a[2], a[1]);
            let lake = if self.b.0 > self.a.0 {
                [ce - 0.03, cs + 0.01, ce + 0.03, cs + 0.03]
            } else {
                [ce - 0.08, cs - 0.01, ce - 0.03, cs + 0.01]
            };
            let inner = [ce - 0.12, cs + 0.03, ce - 0.08, cs + 0.05];
            let sea = [ce - 0.2, cs + 0.06, ce - 0.15, cs + 0.08];
            let all = [rect(Kind::Lake, 9, lake), rect(Kind::Lake, 11, inner), rect(Kind::Sea, 0, sea)];
            // (A tile has the polygons that meet it.)
            let n2 = (1u64 << z) as f64;
            let tb = [
                x as f64 / n2 * 360.0 - 180.0,
                (std::f64::consts::PI * (1.0 - 2.0 * (y + 1) as f64 / n2)).sinh().atan().to_degrees(),
                (x + 1) as f64 / n2 * 360.0 - 180.0,
                (std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n2)).sinh().atan().to_degrees(),
            ];
            let meets = |r: [f64; 4]| r[0] < tb[2] && r[2] > tb[0] && r[1] < tb[3] && r[3] > tb[1];
            Ok([(lake, 0usize), (inner, 1), (sea, 2)]
                .into_iter()
                .filter(|(r, _)| meets(*r))
                .map(|(_, i)| {
                    let p = &all[i];
                    Poly { kind: p.kind, id: p.id, rings: p.rings.clone() }
                })
                .collect())
        }
        fn pin(&self) -> String {
            "test".into()
        }
    }

    /// A z6 tile in the far north, the coverage at a corner of four of its subtrees (12.656°E, 70.10°N: z8 columns 136–137, rows 56–57, in z6
    /// tile 6/34/14), and its raw tiles under `d`: the z6 tile, its coverage, its raw tiles' cache.
    fn north_piece(d: &Path) -> ((u32, u32), Coverage, RawTiles) {
        let cov = Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:12.656,70.10,1".into()],
            }],
            None,
            d,
        )
        .unwrap();
        let by_q = crate::agent::build::coverage_tiles(&cov);
        let (&q, ts) = by_q.iter().next().unwrap();
        assert!(ts.contains(&(34, 14)), "{ts:?}");
        crate::terrain_pack::tests::synthetic_raw(&d.join("local"), &cov, q, &[(34, 14)]);
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

    /// Raw tiles with the pool's shortcut (crate::terrain_base::Tiles): a tile whose four children
    /// were read with data isn't read, a flat tile stands for it (its four quarters are its
    /// children's, so the bytes don't show it; the tiles read do).
    struct Shortcut<'a> {
        inner: &'a RawTiles,
        with: std::sync::Mutex<std::collections::HashSet<(u8, u32, u32)>>,
        flat: Vec<u8>,
        stood: std::sync::atomic::AtomicUsize,
        read: std::sync::Mutex<BTreeSet<(u8, u32, u32)>>,
    }
    impl<'a> Shortcut<'a> {
        fn new(inner: &'a RawTiles) -> Self {
            let flat = roadcore::grid::encode_terrain_png(&vec![1000f32; 65536], 256, 256).unwrap();
            Shortcut { inner, with: Default::default(), flat, stood: Default::default(), read: Default::default() }
        }
    }
    impl RawSource for Shortcut<'_> {
        fn made_before(&self, z: u8, tiles: &[(u32, u32)]) {
            self.with.lock().unwrap().extend(tiles.iter().map(|&(x, y)| (z, x, y)));
        }
        fn get(&self, z: u8, x: u32, y: u32) -> Result<(Option<Vec<u8>>, bool)> {
            let covered = {
                let w = self.with.lock().unwrap();
                (0..4u32).all(|k| w.contains(&(z + 1, 2 * x + (k & 1), 2 * y + (k >> 1))))
            };
            let got = if covered {
                self.stood.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(self.flat.clone())
            } else {
                self.read.lock().unwrap().insert((z, x, y));
                self.inner.get(z, x, y)?.0
            };
            if got.is_some() {
                self.with.lock().unwrap().insert((z, x, y));
            }
            Ok((got, false))
        }
        fn prefetch_counted(
            &self,
            z: u8,
            tiles: &[(u32, u32)],
            threads: usize,
            done: &std::sync::atomic::AtomicU64,
        ) -> Result<usize> {
            self.inner.prefetch_counted(z, tiles, threads, done)
        }
    }

    /// With the pool's shortcut (`Shortcut`), a z6 tile stopped after each of its blocks and level
    /// ends makes the same packs, and reads no tile its first run didn't: a resumed run takes the
    /// tiles kept as read with data (`RawSource::made_before`), so a tile over four of them isn't
    /// sampled, as when they were read.
    #[test]
    fn a_z6_tile_resumed_takes_the_pools_shortcut_as_its_first_run() {
        use crate::resume_log::STOP_AFTER;
        let d = tempfile::tempdir().unwrap();
        let (t, cov, inner) = north_piece(d.path());
        let src = Sources::default();
        #[allow(clippy::type_complexity)]
        let run =
            |name: &str, stop: Option<usize>| -> (Option<Vec<(String, String)>>, usize, BTreeSet<(u8, u32, u32)>) {
                let raw = Shortcut::new(&inner);
                let root = d.path().join(name);
                let scratch = d.path().join(format!("{name}-scratch"));
                let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
                STOP_AFTER.set(stop);
                let r = crate::terrain_pack::build_piece_keeping(&mut out, &raw, t, &cov, &src);
                STOP_AFTER.set(None);
                let stood = raw.stood.load(std::sync::atomic::Ordering::Relaxed);
                let read = raw.read.into_inner().unwrap();
                if let Err(e) = r {
                    assert!(e.to_string().contains("a test's interruption"), "{e:#}");
                    return (None, stood, read);
                }
                let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
                let packs = out.manifest.into_iter().filter(|(l, _)| l.starts_with("layers/terrain/")).collect();
                (Some(packs), stood, read)
            };
        let (fresh, stood, first) = run("fresh", None);
        let fresh = fresh.unwrap();
        assert!(stood > 0, "the shortcut taken");
        let mut stops = 0;
        for stop in 1.. {
            let name = format!("stop-{stop}");
            if run(&name, Some(stop)).0.is_some() {
                break;
            }
            let (packs, _, read) = run(&name, None);
            assert_eq!(packs.as_ref(), Some(&fresh), "stopped after {stop}");
            let extra: Vec<_> = read.difference(&first).collect();
            assert!(extra.is_empty(), "stopped after {stop}, read again: {extra:?}");
            stops = stop;
        }
        assert!(stops >= 2, "{stops}");
    }

    /// Paused mid-sampling (task #705: crate::control::Paused, between the pool's chunks), a z6 tile
    /// saves nothing, its blocks made so far kept in its log; resumed, the packs of a run never
    /// paused.
    #[test]
    fn a_z6_tile_paused_mid_sampling_resumes_to_the_same_packs() {
        use crate::terrain_pack::tests::{files_under, PauseAfter};
        let d = tempfile::tempdir().unwrap();
        let (t, cov, raw) = north_piece(d.path());
        let src = Sources::default();
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
            assert!(crate::control::is_paused(&e), "{e:#}");
            assert!(saved(&root, &scratch).is_empty(), "paused after {left}: nothing saved");
            let packs = files_under(&root).into_iter().filter(|f| f.to_string_lossy().contains("terrain")).count();
            assert_eq!(packs, 0, "paused after {left}: no pack");
            paused += 1;
            logged += crate::resume_log::path(&scratch, (6, t.0, t.1)).exists() as usize;
            // Resumed: the same packs.
            let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            crate::terrain_pack::build_piece_keeping(&mut out, &raw, t, &cov, &src).unwrap();
            assert_eq!(saved(&root, &scratch), fresh, "paused after {left}, resumed");
        }
        assert!(paused >= 3 && logged >= 1, "{paused} paused, {logged} with blocks kept");
    }

    #[test]
    fn a_z6_tile_stopped_and_resumed_makes_the_same_packs() {
        use crate::resume_log::STOP_AFTER;
        let d = tempfile::tempdir().unwrap();
        let (t, cov, raw) = north_piece(d.path());
        let levels = piece_levels(&cov, t);
        let subs = groups(&levels, None).unwrap();
        let water = Across { a: subs[0][0], b: subs[1][0] };
        // (Its packs and mid as saved, their content names the bytes' digests; None when stopped.)
        let run = |name: &str, src: &Sources, stop: Option<usize>| -> Option<Vec<(String, String)>> {
            let root = d.path().join(name);
            let scratch = d.path().join(format!("{name}-scratch"));
            let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            STOP_AFTER.set(stop);
            let r = crate::terrain_pack::build_piece_keeping(&mut out, &raw, t, &cov, src);
            STOP_AFTER.set(None);
            let log = crate::resume_log::path(&scratch, (6, t.0, t.1));
            if let Err(e) = r {
                assert!(e.to_string().contains("a test's interruption") && log.exists(), "{e:#}");
                return None;
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
        let with = Sources { water: Some(&water), deeper: None };
        let without = Sources::default();
        let fresh = run("fresh", &with, None).unwrap();
        assert_eq!(fresh.len(), 2, "{fresh:?}");
        // Stopped after each of its blocks and level ends in turn, then resumed: the same bytes.
        let mut kept = 0;
        for stop in 1.. {
            let name = format!("stop-{stop}");
            if run(&name, &with, Some(stop)).is_some() {
                break;
            }
            assert_eq!(run(&name, &with, None).as_ref(), Some(&fresh), "stopped after {stop}");
            kept = stop;
        }
        assert_eq!(kept, 7, "its blocks and level ends, but the last");
        // Stopped twice, the second time further on.
        assert!(run("twice", &with, Some(2)).is_none() && run("twice", &with, Some(3)).is_none());
        assert_eq!(run("twice", &with, None).as_ref(), Some(&fresh), "stopped twice");
        // Another input (no water) voids the log: the dry run's bytes, not the wet log's.
        let dry = run("dry", &without, None).unwrap();
        assert_ne!(dry, fresh);
        assert!(run("voided", &with, Some(kept - 1)).is_none());
        assert_eq!(run("voided", &without, None), Some(dry));
    }

    #[test]
    fn a_groups_tiles_made_from_its_file_are_the_pieces_bytes() {
        let d = tempfile::tempdir().unwrap();
        let (t, cov, raw) = north_piece(d.path());
        let levels = piece_levels(&cov, t);
        let subs = groups(&levels, None).unwrap();
        assert_eq!(subs, [vec![(136, 56)], vec![(136, 57)], vec![(137, 56)], vec![(137, 57)]]);
        // Two subtrees side by side (in a column, or a row): a lake across their edge.
        let (a, b) = subs
            .iter()
            .flatten()
            .flat_map(|&a| subs.iter().flatten().map(move |&b| (a, b)))
            .find(|(a, b)| (b.0 == a.0 + 1 && b.1 == a.1) || (b.0 == a.0 && b.1 == a.1 + 1))
            .unwrap();
        let water = Across { a, b };
        let src = Sources { water: Some(&water), deeper: None };
        let gs = groups(&levels, Some(&water)).unwrap();
        assert_eq!(gs.len(), subs.len() - 1, "the lake's two subtrees in one group");
        let joint = gs.iter().find(|g| g.contains(&a)).unwrap();
        assert_eq!(joint, &vec![a, b]);
        // The whole z6 tile, and each group from its file: the same tiles, the same mid.
        let whole = crate::terrain_pack::make_piece(&raw, &src, levels.clone()).unwrap();
        assert!(whole.1.levels.contains_key(&9) && whole.1.levels.contains_key(&11), "{:?}", whole.1.levels);
        // Each level's tiles all prepared at once (the way before task #447), and in blocks of one
        // tile (the lakes 9, across blocks, and 11, in one block's tiles or more): the same bytes.
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
            mid.levels.extend(one.1.levels);
        }
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        assert!(same(&(hi, mid), &whole), "the groups make the piece");
        // Not closed under its lakes, the z6 tile isn't the same: the lake's level differs.
        let split = levels_of(&levels, &[a].into(), true);
        let rest = levels_of(&levels, &[a].into(), false);
        let (x, y) = (
            crate::terrain_pack::make_piece(&raw, &src, split).unwrap(),
            crate::terrain_pack::make_piece(&raw, &src, rest).unwrap(),
        );
        assert!(
            x.1.levels.get(&9) != whole.1.levels.get(&9) || y.1.levels.get(&9) != whole.1.levels.get(&9),
            "the test's lake has a level of each side's own"
        );
        // A tile the task wasn't cut for is refused, not "none".
        let g = &gs[0];
        let inp = Inputs::open(&d.path().join(format!("t-{}-{}", g[0].0, g[0].1)).join(INPUT)).unwrap();
        assert!(RawSource::get(&inp, 12, 0, 0).is_err());
        // Polygons read back bit for bit.
        let p = water.polys(12, (a.0 << 4) + 15, (a.1 << 4) + 15).unwrap();
        assert!(!p.is_empty(), "the lake at its corner");
        let back = polys_from(&polys_bytes(&p)).unwrap();
        assert_eq!(
            back.iter().map(|q| (q.kind, q.id, q.rings.clone())).collect::<Vec<_>>(),
            p.iter().map(|q| (q.kind, q.id, q.rings.clone())).collect::<Vec<_>>()
        );
        assert!(polys_from(&polys_bytes(&p)[..10]).is_err());
        assert_eq!(parse_subtrees(&subtrees_arg(&[a, b])).unwrap(), [a, b]);
    }

    /// The z6 tile's run with `offload` into root `name` under `d`: its hi pack's and mid's content
    /// names.
    fn piece(
        d: &Path,
        name: &str,
        t: (u32, u32),
        cov: &Coverage,
        raw: &RawTiles,
        src: &Sources,
        offload: Option<&Offload>,
    ) -> (Option<String>, Option<String>) {
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.join(name)), &d.join(format!("{name}-scratch")))
                .unwrap();
        rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap()
            .install(|| build_piece_with(&mut out, raw, t, cov, src, false, &|_, _, _| {}, offload))
            .unwrap();
        (
            out.get(&format!("layers/terrain/hi/6-{}-{}", t.0, t.1)).map(str::to_string),
            out.get(&mid_logical(t.0, t.1)).map(|c| {
                let (_, m) = read_mid(&out.path(c)).unwrap();
                format!(
                    "{:?}",
                    (
                        m.quads
                            .iter()
                            .map(|(k, v)| (*k, v.iter().map(|f| f.to_bits()).collect::<Vec<_>>()))
                            .collect::<Vec<_>>(),
                        m.levels.iter().map(|(k, v)| (*k, v.to_bits())).collect::<Vec<_>>()
                    )
                )
            }),
        )
    }

    /// A worker in a thread: asks until it's given a task (each 0.2 s, up to 20 s), runs it as the
    /// program `terrainsub` does, sends what it wrote (`spoil`: a tile spoilt first), done.
    fn worker(
        url: &str,
        token: &str,
        name: &str,
        home: PathBuf,
        spoil: bool,
    ) -> std::thread::JoinHandle<Option<serde_json::Value>> {
        let m = Client::at(vec![url.to_string()], token.to_string(), name);
        std::thread::spawn(move || {
            let ask =
                crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
            let mut g = None;
            for _ in 0..100 {
                if let Some(x) = m.ask(&ask).unwrap() {
                    g = Some(x);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let g = g?;
            let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
            let u = home.join("u");
            std::fs::create_dir_all(&u).unwrap();
            for i in task["inputs"].as_array().unwrap() {
                let p = i[0].as_str().unwrap();
                std::fs::write(home.join(p), m.get_bytes(&format!("/work/in/{}/{p}", g.lease)).unwrap()).unwrap();
            }
            let args: Vec<String> = task["runs"][0]["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap().replace("{dir}", &u.to_string_lossy()))
                .collect();
            assert_eq!(args[0], "--in");
            let inp = Inputs::open(Path::new(&args[1])).unwrap();
            let (mut hi, mid) = inp.make().unwrap();
            if spoil {
                let n = hi[0].3.len();
                hi[0].3[n / 2] ^= 1;
            }
            write_hi(&u.join(HI), &inp.meta.piece, &inp.meta.subtrees, &hi).unwrap();
            crate::terrain_pack::write_mid(&u.join(MID), inp.piece().unwrap(), &mid).unwrap();
            let mut outputs = Vec::new();
            for f in [HI, MID] {
                let bytes = std::fs::read(u.join(f)).unwrap();
                let path = format!("u/{f}");
                m.put_bytes(&format!("/work/out/{}/{path}", g.lease), &bytes).unwrap();
                outputs.push(crate::coord::task::Output { path, size: bytes.len() as u64 });
            }
            let done = crate::coord::Done { lease: g.lease, outputs, peak_mb: 300, secs: 0.5, ..Default::default() };
            assert_eq!(m.done(&done).unwrap(), crate::coord::client::Handed::Taken);
            Some(task)
        })
    }

    #[test]
    fn a_pieces_subtrees_go_to_workers_and_come_back_the_same() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        let (t, cov, raw) = north_piece(p);
        let levels = piece_levels(&cov, t);
        let subs = groups(&levels, None).unwrap();
        let (a, b) = subs
            .iter()
            .flatten()
            .flat_map(|&a| subs.iter().flatten().map(move |&b| (a, b)))
            .find(|(a, b)| (b.0 == a.0 + 1 && b.1 == a.1) || (b.0 == a.0 && b.1 == a.1 + 1))
            .unwrap();
        let water = Across { a, b };
        let src = Sources { water: Some(&water), deeper: None };
        let alone = piece(p, "alone", t, &cov, &raw, &src, None);
        assert!(alone.0.is_some() && alone.1.is_some());
        let (c, port) = crate::coord::start_for_test(&p.join("coord"), "m4", "");
        let url = format!("http://127.0.0.1:{port}");
        let tok = c.contact.token.clone();
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
        // No worker around: nothing offered, the same z6 tile.
        let o = Offload::at(url.clone(), c.job_token.clone(), &p.join("job"));
        assert_eq!(piece(p, "none", t, &cov, &raw, &src, Some(&o)), alone);
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());

        // A worker measured fast, its results trusted: its group (the last) waited for and taken,
        // the same z6 tile.
        let m1 = Client::at(vec![url.clone()], tok.clone(), "m1");
        assert!(m1.ask(&ask).unwrap().is_none());
        let fast = || {
            let mut s = c.shared.lock().unwrap();
            s.tasks.paces.insert(("m1".into(), KIND.into()), 0.2);
            s.workers.get_mut("m1").unwrap().checked = 3;
        };
        fast();
        // (Waited on until it's back: this is the exchange, not the waiting rule, and the worker is
        // a thread of this test.)
        let patient = Offload::at(url.clone(), c.job_token.clone(), &p.join("job-patient"))
            .waiting(crate::offload::Waiting::UntilDone);
        let w = worker(&url, &tok, "m1", p.join("m1"), false);
        assert_eq!(piece(p, "taken", t, &cov, &raw, &src, Some(&patient)), alone);
        let task = w.join().unwrap().unwrap();
        let last = groups(&levels, Some(&water)).unwrap().into_iter().rev().find(|g| g.len() <= MOST_SUBTREES).unwrap();
        assert_eq!(task["subtrees"].as_str(), Some(subtrees_arg(&last).as_str()));
        assert_eq!(task["runs"][0]["prog"], "terrainsub");
        {
            let s = c.shared.lock().unwrap();
            assert_eq!((s.workers["m1"].checked, s.workers["m1"].bad), (3, false), "taken, not checked");
            assert_eq!(s.costs[&format!("{KIND} {}", task["unit"].as_str().unwrap())].peak_mb, 300);
            assert!(s.tasks.by_id.is_empty());
        }

        // Its first results checked: a spoilt one is found, the group made here, the worker gets
        // no more work.
        fast();
        c.shared.lock().unwrap().workers.get_mut("m1").unwrap().checked = 0;
        let w = worker(&url, &tok, "m1", p.join("m1b"), true);
        assert_eq!(piece(p, "spoilt", t, &cov, &raw, &src, Some(&patient)), alone);
        w.join().unwrap().unwrap();
        assert!(c.shared.lock().unwrap().workers["m1"].bad);
    }
    /// By hand, read-only (gap 11): over every z6 tile near the live coverage (`TT_ROOT`: the
    /// NAS's project folder, read only, as `scenic-build terrain-same` reads it), the lakes keyed by
    /// their tile's own (no OSM id), and those of a tile's feature 16 on (`own_key`'s bit 62: their
    /// key differs from the one before gap 11), each said with its tile.
    #[test]
    #[ignore]
    fn own_lake_keys_in_live_pieces() {
        use crate::terrain_water::{tile_water, WaterSource};
        use rayon::prelude::*;
        let root = store::nasfs::NasRoot::from_arg(std::env::var("TT_ROOT").expect("TT_ROOT"));
        let out = Out::unbound(&root, &std::env::temp_dir().join("tt-own-keys")).unwrap();
        let cov = live_coverage(&out);
        let water = crate::terrain_pack::open_water(&out).unwrap();
        let pieces: Vec<(u32, u32)> = crate::agent::build::coverage_tiles(&cov).into_values().flatten().collect();
        let (mut n_tiles, mut own, mut moved) = (0usize, 0usize, 0usize);
        for (k, &t) in pieces.iter().enumerate() {
            for (z, tiles) in piece_levels(&cov, t) {
                n_tiles += tiles.len();
                let ids: Vec<Vec<u64>> = tiles
                    .par_iter()
                    .map(|&(x, y)| {
                        tile_water(&water as &dyn WaterSource, z, x, y, false)
                            .unwrap()
                            .map(|w| w.ids.iter().copied().filter(|i| i >> 63 == 1).collect())
                            .unwrap_or_default()
                    })
                    .collect();
                for (&(x, y), v) in tiles.iter().zip(ids) {
                    own += v.len();
                    for i in v.into_iter().filter(|i| i >> 62 & 1 == 1) {
                        moved += 1;
                        println!("piece 6/{}/{} tile {z}/{x}/{y}: key {i:#x}, feature 16 on", t.0, t.1);
                    }
                }
            }
            if k % 50 == 0 {
                eprintln!(
                    "{k} of {} pieces, {n_tiles} tiles, {own} own keys, {moved} from feature 16 on",
                    pieces.len()
                );
            }
        }
        println!(
            "{} pieces, {n_tiles} tiles, {own} lakes keyed by their tile's own, {moved} of them from feature 16 on",
            pieces.len()
        );
    }

    /// The coverage of `out`'s regions, as scenic-build's steps read it (the latest pass's outlines).
    fn live_coverage(out: &Out) -> Coverage {
        let root = out.root();
        let a = crate::inputs::regions::read::source(root, &out.manifest).unwrap();
        Coverage::accepted(root, &a, None).unwrap()
    }

    /// By hand, on the build's own data (docs/workers.md §3): z6 tiles `TT_PIECES` ("6/x/y,…") made
    /// again in a scratch root over the NAS (`TT_ROOT`: its terrain hi packs' and mids' folders its
    /// own, the rest read only; `TT_SCRATCH`, `TT_RAW` the raw tiles' cache), expecting the same as
    /// the manifest has them (hi pack and mid), some of each z6 tile's subtrees made by another worker
    /// through a coordinator listening on `TT_PORT` (its WebAssembly programs from `TT_WASM`):
    /// `TT_WORKER=native`, a thread running each task as `scenic run-task` does (the programs in
    /// `TT_BIN`); `page`, a page that opens `http://127.0.0.1:<port>/work/` (waited for). The first
    /// z6 tile's group is checked against this Mac's run (as a worker's first results are), the
    /// others' taken (the worker's pace and checks set so: this is the exchange, not the waiting
    /// rule). Each group's worker, time and peak memory said.
    #[test]
    #[ignore]
    fn live_pieces_with_a_worker() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} isn't set"));
        let (root, scratch) = (PathBuf::from(env("TT_ROOT")), PathBuf::from(env("TT_SCRATCH")));
        let port: u16 = env("TT_PORT").parse().unwrap();
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch.join("job")).unwrap();
        let cov = live_coverage(&out);
        let raw = RawTiles::with_store(
            &PathBuf::from(env("TT_RAW")),
            &store::nasfs::NasPath::local_for_test(&root.join("sources/aws-terrarium")),
            &crate::terrain_base::terrarium_template(&store::nasfs::NasRoot::local_for_test(&root), &out.manifest)
                .unwrap(),
        );
        let water = crate::terrain_pack::open_water(&out).unwrap();
        let src = Sources { water: Some(&water), deeper: None };
        let c = crate::coord::Coordinator::start(
            &scratch.join("coord"),
            Some(PathBuf::from(env("TT_WASM"))),
            port,
            "m4",
            "",
        )
        .unwrap();
        let url = format!("http://127.0.0.1:{port}");
        let off = Offload::at(url.clone(), c.job_token.clone(), &scratch.join("offload"))
            .waiting(crate::offload::Waiting::UntilDone);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let native = env("TT_WORKER") == "native";
        let w = native.then(|| {
            let (m, bin, dir, stop) = (
                Client::at(vec![url.clone()], c.contact.token.clone(), "m1"),
                PathBuf::from(env("TT_BIN")),
                scratch.join("native"),
                stop.clone(),
            );
            std::thread::spawn(move || {
                let ask = crate::coord::Ask {
                    kind: "native".into(),
                    can: vec![KIND.into()],
                    mem_mb: 8192,
                    cores: 8,
                    ..Default::default()
                };
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let Some(g) = m.ask(&ask).unwrap() else {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        continue;
                    };
                    let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
                    let r = crate::offload::run_task(&m, g.lease, &task, &dir, &bin, None).unwrap();
                    let done = crate::coord::Done {
                        lease: g.lease,
                        outputs: serde_json::from_value(r["outputs"].clone()).unwrap(),
                        secs: r["secs"].as_f64().unwrap(),
                        peak_mb: r["peak_mb"].as_u64().unwrap(),
                        ..Default::default()
                    };
                    m.done(&done).unwrap();
                }
            })
        });
        // (A page: waited for, up to ten minutes.)
        let began = std::time::Instant::now();
        let who = loop {
            let found = c
                .shared
                .lock()
                .unwrap()
                .workers
                .iter()
                .find(|(_, w)| w.can.iter().any(|k| k == KIND))
                .map(|(n, _)| n.clone());
            if let Some(n) = found {
                break n;
            }
            assert!(began.elapsed().as_secs() < 600, "no worker came");
            std::thread::sleep(std::time::Duration::from_millis(500));
        };
        eprintln!("worker: {who}");
        let pieces: Vec<(u32, u32)> = env("TT_PIECES")
            .split(',')
            .map(|p| crate::legacy::TileId::parse(p).filter(|u| u.z == 6).map(|u| (u.x, u.y)).unwrap())
            .collect();
        for (k, &t) in pieces.iter().enumerate() {
            let (hi_l, mid_l) = (format!("layers/terrain/hi/6-{}-{}", t.0, t.1), mid_logical(t.0, t.1));
            let (hi_was, mid_was) = (out.get(&hi_l).map(str::to_string), out.get(&mid_l).map(str::to_string));
            assert!(hi_was.is_some() && mid_was.is_some(), "6/{}/{}: no hi pack or mid in the manifest", t.0, t.1);
            {
                let mut s = c.shared.lock().unwrap();
                s.tasks.paces.insert((who.clone(), KIND.into()), 0.2);
                s.workers.get_mut(&who).unwrap().checked = if k == 0 { 0 } else { 3 };
            }
            let t0 = std::time::Instant::now();
            out.remove(&mid_l);
            crate::terrain_pack::build_piece_with(&mut out, &raw, t, &cov, &src, true, &|_, _, _| {}, Some(&off))
                .unwrap();
            assert_eq!(out.get(&hi_l).map(str::to_string), hi_was, "6/{}/{}'s hi pack", t.0, t.1);
            assert_eq!(out.get(&mid_l).map(str::to_string), mid_was, "6/{}/{}'s mid", t.0, t.1);
            let s = c.shared.lock().unwrap();
            let costs: Vec<String> = s
                .costs
                .iter()
                .filter(|(k, _)| k.starts_with(KIND))
                .map(|(k, v)| format!("{k}: {} MB", v.peak_mb))
                .collect();
            println!("6/{}/{}: the same hi pack ({}) and mid ({}) in {:.1} s; {who}: done {}, checked {}, bad {}; costs {costs:?}", t.0, t.1, hi_was.unwrap(), mid_was.unwrap(), t0.elapsed().as_secs_f64(), s.workers[&who].done, s.workers[&who].checked, s.workers[&who].bad);
            assert!(!s.workers[&who].bad);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(w) = w {
            w.join().unwrap();
        }
    }
}
