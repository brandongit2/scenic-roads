//! A row of a tree cover piece's zoom-8 blocks as a task (kind `treeblock`; docs/workers.md §3,
//! docs/plan.md §6 Trees): the blocks of one z6 tile in one zoom-8 row, made together by the
//! `trees` program (`--blocks`, natively or as WebAssembly in a page). A row's blocks read the same
//! canopy rows (a canopy square's strips are rows of its whole 10° width), so one worker reads each
//! strip once for all of them.
//!
//! A task's folder (`u/` on a worker; docs/formats.md) holds only `coverage.json`: the piece's
//! coverage rings whose box meets the row's (with a margin), as the trees program reads a
//! coverage. Everything else the program reads where it lies: the canopy squares (`{chm}`, the
//! NAS's `sources/canopy/`) and the leaf-type squares (`{sources}/trees/leaf`), a range at a time.
//! It's told the canopy squares the job found there (`--squares`): one a worker doesn't find fails
//! the task, never a block without its trees. It writes `8-<x>-<y>/` for each block
//! (`trees-{cover,height,leaf}.tiles`, `trees-tops.bin`: `trees::write_block`'s), which the job
//! takes into the piece in the blocks' turn, as if made here: the same bytes (`trees::blocks`).

use super::{block_dir, mask, Inputs, Tile, LAYERS, TOPS, ZBLOCK, ZMAX};
use crate::offload::{Offered, Offload, Patience, Settled};
use anyhow::{ensure, Context, Result};
use roadcore::archive::Archive;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The task's kind (and what the coordinator keeps its cost under, with its row's first block).
pub const KIND: &str = "treeblock";
/// The coverage's file in a task's folder.
pub const COVERAGE: &str = "coverage.json";
/// At most this many rows of a piece out at once.
pub const MOST: usize = 3;

/// Blocks named `8/x/y,…`.
pub fn parse_blocks(s: &str) -> Result<Vec<(u32, u32)>> {
    s.split(',')
        .map(|b| crate::legacy::Unit::parse(b.trim()).filter(|u| u.z == ZBLOCK).map(|u| (u.x, u.y)).with_context(|| format!("not a zoom-8 block: {b}")))
        .collect()
}

/// Blocks as `parse_blocks` reads them.
pub fn blocks_arg(list: &[(u32, u32)]) -> String {
    list.iter().map(|(x, y)| format!("{ZBLOCK}/{x}/{y}")).collect::<Vec<_>>().join(",")
}

/// Canopy squares named `top,left;…` (none: an empty list).
pub fn parse_squares(s: &str) -> Result<Vec<(i32, i32)>> {
    s.split(';')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (t, l) = p.split_once(',').with_context(|| format!("a square is top,left: {p}"))?;
            Ok((t.trim().parse()?, l.trim().parse()?))
        })
        .collect()
}

/// Squares as `parse_squares` reads them.
pub fn squares_arg(sq: &[(i32, i32)]) -> String {
    sq.iter().map(|(t, l)| format!("{t},{l}")).collect::<Vec<_>>().join(";")
}

/// A piece's blocks (in `blocks_of`'s order) by row: each row's blocks west to east, the rows
/// north to south.
pub fn rows_of(blocks: &[(u32, u32)]) -> Vec<Vec<(u32, u32)>> {
    let mut by: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
    for &b in blocks {
        by.entry(b.1).or_default().push(b);
    }
    by.into_values()
        .map(|mut r| {
            r.sort_unstable();
            r
        })
        .collect()
}

/// A row's box in degrees (west, south, east, north).
fn row_box(row: &[(u32, u32)]) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for &(x, y) in row {
        let t = super::tile_bounds(ZBLOCK, x, y);
        b = [b[0].min(t[0]), b[1].min(t[1]), b[2].max(t[2]), b[3].max(t[3])];
    }
    b
}

/// The canopy squares of `there` that a row's blocks meet.
pub fn row_squares(row: &[(u32, u32)], there: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = row.iter().flat_map(|&(x, y)| super::squares_of(super::tile_bounds(ZBLOCK, x, y))).filter(|s| there.contains(s)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// A coverage (the trees program's JSON) cut to the rings whose box meets `b` with a margin (a
/// hundredth of a degree): a block there sees the same rings (`mask::Shapes::meeting`, the rings'
/// boxes against the block's), so the same pixels inside. Its shapes with no ring left go.
pub fn cut_coverage(json: &str, b: [f64; 4]) -> Result<String> {
    const MARGIN: f64 = 0.01;
    let v: serde_json::Value = serde_json::from_str(json).context("a coverage")?;
    let shapes = v["shapes"].as_array().context("a coverage's shapes")?;
    let meets = |ring: &serde_json::Value| -> bool {
        let pts = ring.as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)));
        let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in pts {
            (w, s, e, n) = (w.min(x), s.min(y), e.max(x), n.max(y));
        }
        w <= b[2] + MARGIN && e >= b[0] - MARGIN && s <= b[3] + MARGIN && n >= b[1] - MARGIN
    };
    let kept: Vec<Vec<&serde_json::Value>> = shapes.iter().map(|rings| rings.as_array().into_iter().flatten().filter(|r| meets(r)).collect()).filter(|r: &Vec<_>| !r.is_empty()).collect();
    Ok(serde_json::to_string(&serde_json::json!({ "shapes": kept }))?)
}

/// A row's task folder `root` (its `u/`): the piece's coverage (`coverage`, its JSON) cut to the
/// row (`cut_coverage`). Its files (path in it, size).
pub fn cut(coverage: &str, row: &[(u32, u32)], root: &Path) -> Result<BTreeMap<String, u64>> {
    let u = root.join("u");
    std::fs::create_dir_all(&u)?;
    let c = cut_coverage(coverage, row_box(row))?;
    std::fs::write(u.join(COVERAGE), &c)?;
    Ok([(format!("u/{COVERAGE}"), c.len() as u64)].into())
}

/// A row's task spec (the shape of a tail's, crate::offload): the program `trees --blocks` over its
/// folder, the squares where they lie; `unit` its first block (what its cost is kept under), and
/// its blocks.
pub fn spec(row: &[(u32, u32)], squares: &[(i32, i32)], version: &str, inputs: &BTreeMap<String, u64>) -> serde_json::Value {
    let args = ["--blocks", &blocks_arg(row), "--coverage", &format!("{{dir}}/{COVERAGE}"), "--chm", "{chm}", "--leaf", "{sources}/trees/leaf", "--squares", &squares_arg(squares), "--out", "{dir}"];
    let run = crate::unit::Run { what: KIND.into(), prog: "trees".into(), args: args.iter().map(|a| a.to_string()).collect(), env: vec![], reads: vec![format!("{{dir}}/{COVERAGE}")] };
    let list: Vec<serde_json::Value> = inputs.iter().map(|(p, n)| serde_json::json!([p, n])).collect();
    let first = row.first().map(|(x, y)| format!("{ZBLOCK}/{x}/{y}")).unwrap_or_default();
    serde_json::json!({ "unit": first, "blocks": blocks_arg(row), "version": version, "runs": [run], "inputs": list, "places": crate::offload::places() })
}

/// A row's predicted memory on a worker (MB), until a worker measures it (the coordinator's
/// `treeblock 8/x/y`): its program's as WebAssembly (measured 141–160 MB for a block, 233 for
/// three, 365–505 for four, the most where they meet four canopy squares: 120 MB and 100 a block,
/// with room), the blocks' archives it writes, which a page holds (3–14 MB a block: 20), and a
/// page's 64 MB of squares' blocks read where they lie.
pub fn mem_mb(blocks: usize) -> u64 {
    120 + 120 * blocks as u64 + 64
}

/// A block's results in its folder (`trees::write_block`'s): its tiles, layer by layer in the
/// order it made them, and its zoom-8 values as written. A tile outside the block, or not of zoom
/// 8–12, or values of another block, refused.
pub fn read_block(dir: &Path, (bx, by): (u32, u32)) -> Result<(Vec<Tile>, Vec<u8>)> {
    let mut tiles = Vec::new();
    for (i, l) in LAYERS.iter().enumerate() {
        let p = dir.join(format!("trees-{l}.tiles"));
        let a = Archive::open(&p).with_context(|| p.display().to_string())?;
        let mut e = a.entries().to_vec();
        e.sort_unstable_by_key(|e| e.offset);
        for e in e {
            let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
            ensure!((ZBLOCK..=ZMAX).contains(&z) && (x >> (z - ZBLOCK), y >> (z - ZBLOCK)) == (bx, by), "block 8/{bx}/{by}'s {l} tiles hold {z}/{x}/{y}");
            tiles.push(Tile { layer: i as u8, z, x, y, webp: a.get_entry(&e).to_vec() });
        }
    }
    let tops = std::fs::read(dir.join(TOPS)).with_context(|| dir.join(TOPS).display().to_string())?;
    ensure!(super::pyramid::Tops::block_of(&tops)? == (bx, by), "block 8/{bx}/{by}'s values are another block's");
    Ok((tiles, tops))
}

/// The files of a block's results.
fn block_files() -> Vec<String> {
    LAYERS.iter().map(|l| format!("trees-{l}.tiles")).chain([TOPS.to_string()]).collect()
}

/// Whether two rows' results (folders of `block_dir`s) are the same bytes, every file of every
/// block.
pub fn same_rows(a: &Path, b: &Path, row: &[(u32, u32)]) -> bool {
    row.iter().all(|&at| block_files().iter().all(|f| matches!((std::fs::read(block_dir(a, at).join(f)), std::fs::read(block_dir(b, at).join(f))), (Ok(x), Ok(y)) if x == y)))
}

/// A row out: its task and blocks.
type Out = (Offered, Vec<(u32, u32)>);

/// What the job runs a row with here: the row's blocks made into a folder (`trees::blocks_files`).
pub type Here<'a> = &'a (dyn Fn(&[(u32, u32)], &Path) -> Result<()> + Sync);

/// A piece's rows out as tasks (crate::offload), as a `bldtiles` job's areas are: offered from
/// the far end (its last rows) when the run begins, one per worker that takes them around, at most
/// `MOST`, never every row; each settled when the run comes to its first block, waiting for no
/// worker longer than this Mac would take (`Patience`: its blocks at the pace of the blocks made
/// here so far, as many at once as there are threads), its result taken, or checked against this
/// Mac's run of the row (the same function) byte for byte, a difference marking the worker bad.
pub struct Offers<'a> {
    offload: Option<&'a Offload>,
    piece: (u32, u32),
    /// Where results wait for their blocks' turn.
    dir: PathBuf,
    /// The rows out, by their first block.
    out: std::sync::Mutex<BTreeMap<(u32, u32), Out>>,
    /// The blocks of the rows out (block → its row's first block).
    of: BTreeMap<(u32, u32), (u32, u32)>,
    /// The blocks made here so far: their time (seconds, on their threads) and count.
    here: std::sync::Mutex<(f64, u64)>,
    threads: usize,
}

impl<'a> Offers<'a> {
    /// Offers some of piece `piece`'s rows (its blocks `blocks`, the canopy squares there `there`,
    /// its coverage's JSON `coverage`), results under `dir`, while workers that take them are
    /// around: none without `offload`.
    pub fn offer(offload: Option<&'a Offload>, piece: (u32, u32), coverage: &str, blocks: &[(u32, u32)], there: &[(i32, i32)], dir: &Path) -> Offers<'a> {
        let threads = rayon::current_num_threads().max(1);
        let mut o = Offers { offload, piece, dir: dir.to_path_buf(), out: Default::default(), of: BTreeMap::new(), here: Default::default(), threads };
        let Some(off) = offload else { return o };
        std::fs::remove_dir_all(dir).ok();
        let rows = rows_of(blocks);
        let depth = off.workers(KIND).min(MOST).min(rows.len().saturating_sub(1));
        for row in rows.iter().rev().take(depth) {
            let offered = (|| -> Result<Offered> {
                let root = off.task_root(&format!("treeblock-{}-{}", row[0].0, row[0].1));
                let inputs = cut(coverage, row, &root)?;
                let sq = row_squares(row, there);
                off.offer_spec(KIND, spec(row, &sq, off.version(), &inputs), &root, inputs, mem_mb(row.len()))
            })();
            match offered {
                Ok(t) => {
                    for &b in row {
                        o.of.insert(b, row[0]);
                    }
                    o.out.get_mut().unwrap().insert(row[0], (t, row.clone()));
                }
                Err(e) => {
                    eprintln!("trees 6/{}/{}: row {} not offered ({e:#}); made here", piece.0, piece.1, blocks_arg(row));
                    break;
                }
            }
        }
        o
    }

    /// Whether block `b` is out (its row offered): the run doesn't make it, it takes it
    /// (`take`).
    pub fn is_out(&self, b: (u32, u32)) -> bool {
        self.of.contains_key(&b)
    }

    /// A block made here took `secs` (on its thread).
    pub fn made_here(&self, secs: f64) {
        let mut h = self.here.lock().unwrap();
        *h = (h.0 + secs, h.1 + 1);
    }

    /// About how long `n` blocks would take here (seconds), at the pace of those made here so far,
    /// as many at once as there are threads.
    fn here_s(&self, n: usize) -> Option<f64> {
        let (secs, k) = *self.here.lock().unwrap();
        (k > 0).then(|| secs / k as f64 * n as f64 / n.min(self.threads).max(1) as f64)
    }

    /// Block `b`'s results, its row settled first if it isn't yet (with the patience its time here
    /// allows; `here` runs a row here): its tiles and values (`read_block`), its folder removed.
    pub fn take(&self, b: (u32, u32), here: Here) -> Result<(Vec<Tile>, Vec<u8>)> {
        let first = *self.of.get(&b).context("a block not out")?;
        let out = self.out.lock().unwrap().remove(&first);
        if let Some((task, row)) = out {
            self.settle(task, row, here)?;
        }
        let d = block_dir(&self.dir.join(format!("{}-{}", first.0, first.1)), b);
        let r = read_block(&d, b)?;
        std::fs::remove_dir_all(&d).ok();
        Ok(r)
    }

    /// Settles row `row`'s task: its results into `dir/<x>-<y>/` (its first block's).
    fn settle(&self, task: Offered, row: Vec<(u32, u32)>, here: Here) -> Result<()> {
        let Some(o) = self.offload else { return Ok(()) };
        let first = row[0];
        let mine = self.dir.join(format!("{}-{}", first.0, first.1));
        let what = format!("trees 6/{}/{}: row {}", self.piece.0, self.piece.1, blocks_arg(&row));
        let run_here = || -> Result<()> {
            std::fs::remove_dir_all(&mine).ok();
            here(&row, &mine)
        };
        let theirs = |st: &serde_json::Value| st["out"].as_str().map(|o| Path::new(o).join("u"));
        let mut take = |st: &serde_json::Value| -> Result<()> {
            // (A worker's files moved here and read through once; else made here.)
            let ok = (|| -> Result<()> {
                let from = theirs(st).context("no outputs' folder")?;
                std::fs::remove_dir_all(&mine).ok();
                for &b in &row {
                    let (src, dst) = (block_dir(&from, b), block_dir(&mine, b));
                    std::fs::create_dir_all(&dst)?;
                    for f in block_files() {
                        if std::fs::rename(src.join(&f), dst.join(&f)).is_err() {
                            store::sys::copy_data(src.join(&f), dst.join(&f)).with_context(|| format!("take {f}"))?;
                        }
                    }
                    read_block(&dst, b)?;
                }
                Ok(())
            })();
            if let Err(e) = ok {
                eprintln!("{what}: a worker's result doesn't read ({e:#}); made here");
                run_here()?;
            }
            Ok(())
        };
        let mut same = |st: &serde_json::Value, _: std::time::SystemTime| -> Result<bool> {
            let same = theirs(st).is_some_and(|d| same_rows(&d, &mine, &row));
            if !same {
                eprintln!("{what}: a worker's tiles differ from this Mac's");
            }
            Ok(same)
        };
        let p = Patience { here_s: self.here_s(row.len()) };
        let how = match o.settle_with(&task, true, p, &mut || run_here(), &mut take, &mut same) {
            Ok(Some(h)) => h,
            Ok(None) => unreachable!("a task waited on is settled"),
            Err(e) => {
                // (The coordinator gone: made here.)
                eprintln!("{what}: its task can't be settled ({e:#}); made here");
                run_here()?;
                std::fs::remove_dir_all(&task.root).ok();
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
        Ok(())
    }
}

/// The inputs a piece's run reads its rows with here: `inp`'s (the canopy squares in this Mac's
/// cache) with the squares found there.
pub fn here_run<'a>(shapes: &'a mask::Shapes, inp: &'a Inputs<'a>) -> impl Fn(&[(u32, u32)], &Path) -> Result<()> + Sync + 'a {
    move |row: &[(u32, u32)], out: &Path| super::blocks_files(shapes, inp, row, out).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::client::Client;
    use crate::trees::{blocks_files, block_files as one_block, mask::Shapes, tests::squares_dir, Run, Source, MID};

    /// Over blocks 8/132–133, rows 88 and 89: two rows of two in piece 6/33/22, the squares'
    /// data in 8/132/88.
    const COV: &str = r#"{"shapes": [[[[5.7, 47.3], [8.3, 47.3], [8.3, 48.9], [5.7, 48.9]]], [[[20.0, 10.0], [21.0, 10.0], [21.0, 11.0]]]]}"#;

    #[test]
    fn a_rows_blocks_are_their_own_blocks_bytes() {
        let d = squares_dir();
        let o = tempfile::tempdir().unwrap();
        let shapes = Shapes::parse(COV).unwrap();
        let fetch = crate::fetch::MapFetch::default();
        let inp = Inputs { chm: Source::Dir(d.path().into()), leaf: Source::Dir(d.path().into()), fetch: &fetch, record: None, there: Some(vec![(50, 0)]) };
        let row = [(132, 88), (133, 88)];
        let pool = |n: usize| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
        pool(1).install(|| blocks_files(&shapes, &inp, &row, &o.path().join("one"))).unwrap();
        pool(4).install(|| blocks_files(&shapes, &inp, &row, &o.path().join("four"))).unwrap();
        assert!(same_rows(&o.path().join("one"), &o.path().join("four"), &row));
        for &b in &row {
            let alone = o.path().join(format!("alone-{}", b.0));
            one_block(&shapes, &inp, b.0, b.1, &alone).unwrap();
            for f in block_files() {
                assert_eq!(std::fs::read(block_dir(&o.path().join("one"), b).join(&f)).unwrap(), std::fs::read(alone.join(&f)).unwrap(), "{f} of 8/{}/{}", b.0, b.1);
            }
            assert!(!read_block(&alone, b).unwrap().0.is_empty() || b != (132, 88));
            assert!(read_block(&alone, (b.0 + 1, b.1)).is_err(), "another block's");
        }
        // A square the job found that the worker doesn't: the task fails.
        let gone = Inputs { there: Some(vec![(50, 0), (50, 10)]), chm: Source::Dir(o.path().join("none")), ..inp };
        assert!(blocks_files(&shapes, &gone, &row, &o.path().join("x")).is_err());
        // The rows, the coverage cut to one, its squares, the program's arguments read back.
        assert_eq!(rows_of(&[(132, 88), (132, 89), (133, 88), (133, 89)]), [vec![(132, 88), (133, 88)], vec![(132, 89), (133, 89)]]);
        let cut = cut_coverage(COV, row_box(&row)).unwrap();
        assert_eq!(Shapes::parse(&cut).unwrap().shapes.len(), 1, "the far shape goes");
        assert_eq!(row_squares(&row, &[(50, 0), (50, 10), (60, 0)]), [(50, 0)]);
        assert_eq!(parse_blocks(&blocks_arg(&row)).unwrap(), row);
        assert_eq!(parse_squares(&squares_arg(&[(50, 0), (-0, -10)])).unwrap(), [(50, 0), (0, -10)]);
        assert!(parse_squares("").unwrap().is_empty() && parse_blocks("6/1/1").is_err());
    }

    /// Piece 6/33/22's run in `dir/out`, with `offload`.
    fn piece(d: &Path, sq: &Path, out: &str, offload: Option<&Offload>) -> std::time::Duration {
        std::fs::write(d.join("cov.json"), COV).unwrap();
        let a = Run { tile: (6, 33, 22), coverage: d.join("cov.json"), chm: sq.into(), chm_store: d.join("store"), leaf: sq.into(), out: d.join(out), dem: d.into() };
        let t = std::time::Instant::now();
        rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap().install(|| super::super::z6_with(&a, offload)).unwrap();
        t.elapsed()
    }

    /// Whether two pieces' runs wrote the same bytes.
    fn same_piece(a: &Path, b: &Path) -> bool {
        LAYERS.iter().map(|l| format!("trees-{l}.tiles")).chain([MID.to_string()]).all(|f| std::fs::read(a.join(&f)).unwrap() == std::fs::read(b.join(&f)).unwrap())
    }

    /// A worker in a thread: asks until it's given a row (each 0.2 s, up to 20 s), runs the program's
    /// code over it as `trees --blocks` does (its squares in `sq`), sends what it wrote (`spoil`: a
    /// tile spoilt first), done; or, `hold`, holds it and never says.
    fn worker(url: &str, token: &str, name: &str, sq: PathBuf, home: PathBuf, spoil: bool, hold: bool) -> std::thread::JoinHandle<Option<serde_json::Value>> {
        let m = Client::at(vec![url.to_string()], token.to_string(), name);
        std::thread::spawn(move || {
            let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
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
            if hold {
                return Some(task);
            }
            let u = home.join("u");
            std::fs::create_dir_all(&u).unwrap();
            for i in task["inputs"].as_array().unwrap() {
                let p = i[0].as_str().unwrap();
                std::fs::write(home.join(p), m.get_bytes(&format!("/work/in/{}/{p}", g.lease)).unwrap()).unwrap();
            }
            let args: Vec<String> = task["runs"][0]["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().replace("{dir}", &u.to_string_lossy()).replace("{sources}/trees/leaf", &sq.to_string_lossy()).replace("{chm}", &sq.to_string_lossy())).collect();
            let arg = |k: &str| args[args.iter().position(|a| a == k).unwrap() + 1].clone();
            let shapes = Shapes::parse(&std::fs::read_to_string(arg("--coverage")).unwrap()).unwrap();
            let fetch = crate::fetch::MapFetch::default();
            let inp = Inputs { chm: Source::Dir(arg("--chm").into()), leaf: Source::Dir(arg("--leaf").into()), fetch: &fetch, record: None, there: Some(parse_squares(&arg("--squares")).unwrap()) };
            let row = parse_blocks(&arg("--blocks")).unwrap();
            blocks_files(&shapes, &inp, &row, Path::new(&arg("--out"))).unwrap();
            let mut outputs = Vec::new();
            for &b in &row {
                for f in block_files() {
                    let mut bytes = std::fs::read(block_dir(&u, b).join(&f)).unwrap();
                    if spoil && f == TOPS && b == row[0] {
                        let n = bytes.len();
                        bytes[n / 2] ^= 1;
                    }
                    let path = format!("u/{}-{}-{}/{f}", ZBLOCK, b.0, b.1);
                    m.put_bytes(&format!("/work/out/{}/{path}", g.lease), &bytes).unwrap();
                    outputs.push(crate::coord::task::Output { path, size: bytes.len() as u64 });
                }
            }
            let done = crate::coord::Done { lease: g.lease, outputs, peak_mb: 400, secs: 0.5, ..Default::default() };
            assert_eq!(m.done(&done).unwrap(), crate::coord::client::Handed::Taken);
            Some(task)
        })
    }

    #[test]
    fn a_pieces_rows_go_to_workers_and_come_back_the_same() {
        let sq = squares_dir();
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        piece(p, sq.path(), "alone", None);
        let (c, port) = crate::coord::start_for_test(&p.join("coord"), "m4", "");
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload::at(url.clone(), c.job_token.clone(), &p.join("job"));
        let tok = c.contact.token.clone();
        let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
        // No worker around: nothing offered, the same piece.
        piece(p, sq.path(), "none", Some(&o));
        assert!(same_piece(&p.join("alone"), &p.join("none")));
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());

        // A worker measured fast (a fifth of this Mac's time), its results trusted: its row (the
        // last, never both) waited for and taken, the same piece.
        let m1 = Client::at(vec![url.clone()], tok.clone(), "m1");
        assert!(m1.ask(&ask).unwrap().is_none());
        {
            let mut s = c.shared.lock().unwrap();
            s.tasks.paces.insert(("m1".into(), KIND.into()), 0.2);
            s.workers.get_mut("m1").unwrap().checked = 3;
        }
        let w = worker(&url, &tok, "m1", sq.path().into(), p.join("m1"), false, false);
        piece(p, sq.path(), "taken", Some(&o));
        let task = w.join().unwrap().unwrap();
        assert_eq!((task["unit"].as_str(), task["blocks"].as_str()), (Some("8/132/89"), Some("8/132/89,8/133/89")));
        assert_eq!(task["runs"][0]["prog"], "trees");
        assert!(same_piece(&p.join("alone"), &p.join("taken")));
        {
            let s = c.shared.lock().unwrap();
            assert_eq!((s.workers["m1"].checked, s.workers["m1"].bad), (3, false), "taken, not checked");
            assert_eq!(s.costs["treeblock 8/132/89"].peak_mb, 400, "its cost under its kind and first block");
            assert!(s.tasks.by_id.is_empty());
        }

        // Its first results checked (the coordinator says): a spoilt one is found, the row made
        // here, the worker gets no more work.
        c.shared.lock().unwrap().workers.get_mut("m1").unwrap().checked = 0;
        let w = worker(&url, &tok, "m1", sq.path().into(), p.join("m1b"), true, false);
        piece(p, sq.path(), "spoilt", Some(&o));
        w.join().unwrap().unwrap();
        assert!(same_piece(&p.join("alone"), &p.join("spoilt")));
        assert!(c.shared.lock().unwrap().workers["m1"].bad);
    }

    #[test]
    fn a_row_waits_on_no_worker_but_one_measured_faster() {
        let sq = squares_dir();
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        // (The piece made with no one offered anything: the time the others are held to, with room
        // for a loaded Mac, rather than a fixed one: 10 s held on this Mac, not on the build Mac
        // with a build beside it, where the piece alone took ~19 s.)
        let alone = piece(p, sq.path(), "alone", None);
        let no_wait = alone.mul_f64(1.5) + std::time::Duration::from_secs(5);
        let (c, port) = crate::coord::start_for_test(&p.join("coord"), "m4", "");
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload::at(url.clone(), c.job_token.clone(), &p.join("job"));
        let tok = c.contact.token.clone();
        let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
        // A page measured slower than this Mac, asking: never given the row, never waited for.
        let slow = Client::at(vec![url.clone()], tok.clone(), "slow");
        assert!(slow.ask(&ask).unwrap().is_none());
        c.shared.lock().unwrap().tasks.paces.insert(("slow".into(), KIND.into()), 2.0);
        let took = piece(p, sq.path(), "slow", Some(&o));
        assert!(took < no_wait, "not waited for: {took:?} (alone {alone:?})");
        assert!(slow.ask(&ask).unwrap().is_none(), "given none");
        assert!(same_piece(&p.join("alone"), &p.join("slow")));
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());
        // One not measured takes the row and holds it: the row made here at its turn, at once (no
        // wait on it); the coordinator keeps it for that worker to finish, to measure it.
        let new = Client::at(vec![url.clone()], tok.clone(), "new");
        assert!(new.ask(&ask).unwrap().is_none());
        let w = worker(&url, &tok, "new", sq.path().into(), p.join("new"), false, true);
        let took = piece(p, sq.path(), "held", Some(&o));
        assert!(w.join().unwrap().is_some(), "it took the row");
        assert!(took < no_wait, "raced at once: {took:?} (alone {alone:?})");
        assert!(same_piece(&p.join("alone"), &p.join("held")));
        let s = c.shared.lock().unwrap();
        assert!(s.tasks.by_id.values().all(|t| t.measuring.is_some()), "kept only to measure it");
        assert_eq!(s.tasks.by_id.len(), 1);
    }
}
