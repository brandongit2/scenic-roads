//! The tree cover layers (docs/plan.md §6, Trees): tree cover, canopy height and leaf type, zoom 4–12,
//! clipped to the coverage, made by the `trees` program (crate::trees, dem/trees.py's port) from
//! Meta's canopy squares, kept on the NAS (`sources/canopy/`, each downloaded once, and copied into
//! the agent's cache the units read too) and the leaf-type squares on the NAS
//! (`sources/trees/leaf/`, made whole by `dem/leaftype.py` where missing), packed as the layers
//! `trees-cover`, `trees-height` and `trees-leaf` (a lo pack per z3 tile, hi packs per z6 tile).
//!
//! The build makes them in two steps (`targets`):
//! - **trees**, a piece per z6 tile the coverage meets (`build_piece`): its hi packs (zoom 9–12) and
//!   its mid (`work/trees-mid/6-x-y`, crate::trees::MID: its blocks' zoom-8 tiles and values, which
//!   its z3 tile's assembly reads); a z6 tile the coverage has left drops its hi packs and mid;
//! - **trees-lo**, an assembly per z3 tile with a piece (`build_lo`): its lo packs (zoom 4–8) from
//!   its pieces' mids; a z3 tile with lo packs and no piece drops them.
//!
//! Together the same packs, byte for byte, as a z3 tile's whole run (`build`: by hand, and for a
//! lease of the old scheme; with SCENIC_TREES_PY=1 trees.py itself, to compare the two).

use crate::coverage::Coverage;
use crate::legacy::Unit;
use crate::out::Out;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Bumped when the layers' pixels change (every piece and assembly is made again). (The `trees`
/// program, which took trees.py's place, makes the same pixels in other WebP bytes: the packs as good
/// as they were.)
pub const TREES_V: u32 = 2;
/// The assembly's own version (its key: crate::treepacks::targets).
pub const TREES_LO_V: u32 = 1;
pub const LAYERS: [&str; 3] = ["trees-cover", "trees-height", "trees-leaf"];

/// The leaf-type squares on the NAS (`lat<top>_lon<left>.tif`).
pub fn leaf_dir(root: &Path) -> PathBuf {
    root.join("sources/trees/leaf")
}

/// Z6 tile (`x`, `y`)'s mid in the manifest (crate::trees::MID).
pub fn mid_logical(x: u32, y: u32) -> String {
    format!("work/trees-mid/6-{x}-{y}")
}

/// Tree cover's targets, each with its key, done or not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    /// The pieces ("6/x/y", key, none): each z6 tile the coverage meets, keyed on the version and
    /// the coverage there (Meta's canopy and the leaf-type sources are fixed datasets); and each z6
    /// tile with tree hi packs or a mid that the coverage no longer meets, "none", whose run drops
    /// them. By z3 tile, then column, then row.
    pub pieces: Vec<(String, String, bool)>,
    /// The assemblies ("3/x/y", key, none): each z3 tile with a piece the coverage meets, keyed on
    /// those pieces' mids by content ("-" for a piece without one: it can't be assembled until each
    /// has), so a piece made again to the same bytes changes nothing here; and each z3 tile with
    /// tree lo packs and no such piece, "none", whose run drops them.
    pub lo: Vec<(String, String, bool)>,
}

impl Targets {
    /// The pieces of z3 tile `q` ("3/x/y") the coverage meets.
    pub fn pieces_of<'a>(&'a self, q: &'a str) -> impl Iterator<Item = &'a (String, String, bool)> + 'a {
        self.pieces.iter().filter(move |p| !p.2 && area_of(&p.0).as_deref() == Some(q))
    }
}

/// The z3 tile ("3/x/y") a piece ("6/x/y") is in.
pub fn area_of(piece: &str) -> Option<String> {
    Unit::parse(piece).filter(|u| u.z == 6).map(|u| format!("3/{}/{}", u.x >> 3, u.y >> 3))
}

/// The tree cover's targets (`Targets`) for the coverage and the build manifest `m`.
pub fn targets(cov: &Coverage, m: &BTreeMap<String, String>) -> Targets {
    let key = |parts: &[&str]| store::naming::hash16(parts.join("|").as_bytes());
    // The z6 tiles the coverage meets, by z3 tile (none in a z3 tile it doesn't meet).
    let mut met: BTreeSet<(u32, u32, u32, u32)> = BTreeSet::new();
    for qx in 0..8u32 {
        for qy in 0..8u32 {
            if !cov.meets_rect(crate::hipack::tile_bounds(3, qx, qy)) {
                continue;
            }
            for x in qx * 8..(qx + 1) * 8 {
                for y in qy * 8..(qy + 1) * 8 {
                    if cov.meets_rect(crate::hipack::tile_bounds(6, x, y)) {
                        met.insert((qx, qy, x, y));
                    }
                }
            }
        }
    }
    // Those with tree hi packs or a mid, the coverage met or not.
    let mut had: BTreeSet<(u32, u32, u32, u32)> = BTreeSet::new();
    let mut lo_had: BTreeSet<(u32, u32)> = BTreeSet::new();
    for l in m.keys() {
        let hi = LAYERS.iter().find_map(|layer| l.strip_prefix(&format!("layers/{layer}/hi/"))).or_else(|| l.strip_prefix("work/trees-mid/"));
        if let Some(u) = hi.and_then(Unit::parse).filter(|u| u.z == 6) {
            had.insert((u.x >> 3, u.y >> 3, u.x, u.y));
        }
        if let Some(q) = LAYERS.iter().find_map(|layer| l.strip_prefix(&format!("layers/{layer}/lo/"))).and_then(Unit::parse).filter(|u| u.z == 3) {
            lo_had.insert((q.x, q.y));
        }
    }
    let mut out = Targets::default();
    for &(qx, qy, x, y) in met.union(&had) {
        let t = format!("6/{x}/{y}");
        let none = !met.contains(&(qx, qy, x, y));
        let k = if none { key(&[&format!("trees {TREES_V}"), &t, "none"]) } else { key(&[&format!("trees {TREES_V}"), &t, &cov.fingerprint(crate::hipack::tile_bounds(6, x, y))]) };
        out.pieces.push((t, k, none));
    }
    let areas: BTreeSet<(u32, u32)> = met.iter().map(|&(qx, qy, _, _)| (qx, qy)).collect();
    for &(qx, qy) in areas.union(&lo_had) {
        let q = format!("3/{qx}/{qy}");
        let head = [format!("trees-lo {TREES_LO_V}"), format!("trees {TREES_V}"), q.clone()];
        let k = if areas.contains(&(qx, qy)) {
            let mids: Vec<String> = met.range((qx, qy, 0, 0)..=(qx, qy, u32::MAX, u32::MAX)).map(|&(_, _, x, y)| format!("6/{x}/{y}={}", m.get(&mid_logical(x, y)).map(String::as_str).unwrap_or("-"))).collect();
            key(&[&head[0], &head[1], &head[2], &mids.join(",")])
        } else {
            key(&[&head[0], &head[1], &head[2], "none"])
        };
        out.lo.push((q, k, !areas.contains(&(qx, qy))));
    }
    out
}

/// The z3 tiles the coverage meets, each with its key: the step's version and the coverage there;
/// and the z3 tiles with tree packs (`m`, the build manifest) the coverage no longer meets, whose
/// run drops them. (A z3 tile's whole run, `build`: how the build made them before the pieces and
/// assemblies.)
pub fn area_targets(cov: &Coverage, m: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for x in 0..8 {
        for y in 0..8 {
            let b = crate::hipack::tile_bounds(3, x, y);
            let t = format!("3/{x}/{y}");
            let had = || LAYERS.iter().any(|l| m.contains_key(&format!("layers/{l}/lo/3-{x}-{y}")) || m.range(format!("layers/{l}/hi/6-")..).take_while(|(k, _)| k.starts_with(&format!("layers/{l}/hi/6-"))).any(|(k, _)| Unit::parse(&k[format!("layers/{l}/hi/").len()..]).is_some_and(|u| (u.x >> 3, u.y >> 3) == (x, y))));
            if cov.meets_rect(b) {
                out.push((t.clone(), store::naming::hash16(format!("trees {TREES_V}|{t}|{}", cov.fingerprint(b)).as_bytes())));
            } else if had() {
                out.push((t.clone(), store::naming::hash16(format!("trees {TREES_V}|{t}|none").as_bytes())));
            }
        }
    }
    out
}

/// Whether dem/trees.py makes a z3 tile's layers rather than its port (SCENIC_TREES_PY=1), to
/// compare them.
fn by_python() -> bool {
    std::env::var("SCENIC_TREES_PY").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The coverage's shapes inside tile `t` (a z3 or z6 tile), for the trees program: each shape's rings
/// whose box meets the tile, in degrees (inside by even–odd, as the shape has them; a ring that
/// doesn't meet the tile can't change which of its points are inside).
pub fn coverage_json(cov: &Coverage, t: Unit) -> serde_json::Value {
    let b = crate::hipack::tile_bounds(t.z, t.x, t.y);
    let deg = |v: i32| v as f64 * 1e-7;
    let shapes: Vec<Vec<Vec<[f64; 2]>>> = cov
        .shapes
        .iter()
        .map(|s| {
            s.rings
                .iter()
                .filter(|r| {
                    let (mut w, mut so, mut e, mut n) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                    for p in r.iter() {
                        (w, so, e, n) = (w.min(p[0]), so.min(p[1]), e.max(p[0]), n.max(p[1]));
                    }
                    w <= b[2] && e >= b[0] && so <= b[3] && n >= b[1]
                })
                .map(|r| r.iter().map(|p| [deg(p[0]), deg(p[1])]).collect())
                .collect()
        })
        .filter(|rings: &Vec<Vec<[f64; 2]>>| !rings.is_empty())
        .collect();
    serde_json::json!({ "shapes": shapes })
}

/// A fresh scratch folder for tile `t`'s run, with its coverage file (`coverage_json`); None (and
/// no folder) when no ring of the coverage meets the tile, unless `always`.
fn fresh(cov: &Coverage, t: Unit, scratch: &Path, always: bool) -> Result<Option<PathBuf>> {
    let dir = scratch.join(format!("trees-{}", t.dash()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    let cj = coverage_json(cov, t);
    if !always && cj["shapes"].as_array().is_none_or(|s| s.is_empty()) {
        return Ok(None);
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("coverage.json"), serde_json::to_vec(&cj)?)?;
    Ok(Some(dir))
}

/// The trees program (`trees`, beside this one), or with SCENIC_TREES_PY=1 (a z3 tile's run alone)
/// trees.py, run in the Python steps' folder `dem` (the port runs leaftype.py there for leaf-type
/// squares to make) with `args`.
fn run_trees(dem: &Path, python: bool, args: &[std::ffi::OsString]) -> Result<()> {
    let (mut c, prog) = if python {
        let mut c = std::process::Command::new("uv");
        c.args(["run", "python", "trees.py"]);
        (c, "trees.py")
    } else {
        (std::process::Command::new(std::env::current_exe()?.parent().context("the programs' folder")?.join("trees")), "the trees program")
    };
    let st = c.current_dir(dem).args(args).status().with_context(|| format!("run {prog}"))?;
    anyhow::ensure!(st.success(), "{prog}: {st}");
    Ok(())
}

/// A run's arguments for the trees program: tile `t` (`--z3` or `--z6`), its coverage in `dir`, the
/// squares (`chm`: this Mac's cache; the NAS's store and leaf types), its archives into `dir`.
fn run_args(out: &Out, t: Unit, dir: &Path, chm: &Path, workers: usize) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = vec![format!("--z{}", t.z).into(), format!("{},{}", t.x, t.y).into(), "--coverage".into(), dir.join("coverage.json").into()];
    for (k, v) in [("--chm", chm.to_path_buf()), ("--chm-store", out.root().join("sources/canopy")), ("--leaf", leaf_dir(out.root())), ("--out", dir.to_path_buf())] {
        a.push(k.into());
        a.push(v.into());
    }
    a.extend(["--workers".into(), workers.to_string().into()]);
    a
}

/// Makes z3 tile `q`'s tree layers in one run and uploads them, dropping its packs it no longer makes
/// (all of them when the coverage has left it): by hand, and for a lease of the old scheme.
pub fn build(out: &mut Out, cov: &Coverage, q: Unit, dem: &Path, chm: &Path, scratch: &Path, workers: usize) -> Result<()> {
    build_with(out, cov, q, dem, chm, scratch, workers, &|| {})
}

/// `build`, telling `writing` when it begins writing the packs (after the trees program), whose
/// progress it says as `layers' packs written`.
#[allow(clippy::too_many_arguments)]
pub fn build_with(out: &mut Out, cov: &Coverage, q: Unit, dem: &Path, chm: &Path, scratch: &Path, workers: usize, writing: &dyn Fn()) -> Result<()> {
    let Some(dir) = fresh(cov, q, scratch, false)? else {
        drop_packs(out, q, &crate::layers::LayerOut::default())?;
        return Ok(());
    };
    run_trees(dem, by_python(), &run_args(out, q, &dir, chm, workers)).with_context(|| format!("z3 tile {}", q.slash()))?;
    writing();
    put_area(out, q, &dir)?;
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// Uploads z3 tile `q`'s packs from its whole run's archives in `dir`, dropping its packs it no
/// longer makes.
pub fn put_area(out: &mut Out, q: Unit, dir: &Path) -> Result<()> {
    for (i, layer) in LAYERS.iter().enumerate() {
        let arc = roadcore::archive::Archive::open(&dir.join(format!("{layer}.tiles")))?;
        let n = LAYERS.len() as u64;
        let made = crate::layers::split_archive_with(out, &arc, layer, "terrarium-webp", false, 12, &|k, t| crate::agent::jobs::report_f(i as f64 + k as f64 / t.max(1) as f64, n, "layers' packs written"))?;
        let gone = drop_layer_packs(out, q, layer, &made)?;
        eprintln!("trees {}: {layer}: {} lo and {} hi packs, {gone} dropped", q.slash(), made.lo.len(), made.hi.len());
    }
    Ok(())
}

/// Drops z3 tile `q`'s packs of every tree layer that `made` doesn't have.
fn drop_packs(out: &mut Out, q: Unit, made: &crate::layers::LayerOut) -> Result<()> {
    for layer in LAYERS {
        let n = drop_layer_packs(out, q, layer, made)?;
        eprintln!("trees {}: {layer}: the coverage has left; {n} packs dropped", q.slash());
    }
    Ok(())
}

/// Drops z3 tile `q`'s packs of `layer` (its lo pack, its z6 tiles' hi packs) that `made` doesn't
/// have; how many.
fn drop_layer_packs(out: &mut Out, q: Unit, layer: &str, made: &crate::layers::LayerOut) -> Result<usize> {
    let mut gone = Vec::new();
    let lo = format!("layers/{layer}/lo/{}", q.dash());
    if !made.lo.contains_key(&q.slash()) && out.get(&lo).is_some() {
        gone.push(lo);
    }
    for x in q.x * 8..(q.x + 1) * 8 {
        for y in q.y * 8..(q.y + 1) * 8 {
            let hi = format!("layers/{layer}/hi/6-{x}-{y}");
            if !made.hi.contains_key(&format!("6/{x}/{y}")) && out.get(&hi).is_some() {
                gone.push(hi);
            }
        }
    }
    for l in &gone {
        out.remove(l);
    }
    out.save()?;
    Ok(gone.len())
}

/// Makes z6 tile `t`'s tree cover (a piece) and uploads it: its hi packs and its mid (`put_piece`),
/// dropping those it no longer makes; all of them when the coverage has left it. `expect_same`: a
/// piece made again as it is (its mid backfilled), whose packs must come out as the manifest has
/// them (`put_piece`). `writing` is told when it begins writing.
#[allow(clippy::too_many_arguments)]
pub fn build_piece(out: &mut Out, cov: &Coverage, t: Unit, dem: &Path, chm: &Path, scratch: &Path, workers: usize, expect_same: bool, writing: &dyn Fn()) -> Result<()> {
    anyhow::ensure!(t.z == 6, "a piece is a z6 tile, not {}", t.slash());
    // (A piece of the coverage, as `targets` has it, has a mid, made with no block if no ring's box
    // meets one; one the coverage has left, none.)
    if !cov.meets_rect(crate::hipack::tile_bounds(6, t.x, t.y)) {
        anyhow::ensure!(!expect_same, "6/{}/{}: the coverage has left it, so it can't be made as it is", t.x, t.y);
        let n = drop_piece(out, t)?;
        eprintln!("trees {}: the coverage has left it; {n} files dropped", t.slash());
        return Ok(());
    }
    let dir = fresh(cov, t, scratch, true)?.context("a piece's scratch folder")?;
    run_trees(dem, false, &run_args(out, t, &dir, chm, workers)).with_context(|| format!("z6 tile {}", t.slash()))?;
    writing();
    put_piece(out, t, &dir, expect_same)?;
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// A tile as a pack takes it: zoom, column, row, its bytes and their length.
type PackTile = (u8, u32, u32, Vec<u8>, u32);

/// The tiles of the archive `p` (in key order), as a pack takes them.
fn archive_tiles(p: &Path) -> Result<Vec<PackTile>> {
    let arc = roadcore::archive::Archive::open(p).with_context(|| p.display().to_string())?;
    Ok(arc
        .entries()
        .iter()
        .map(|e| {
            let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
            (z, x, y, arc.get_entry(e).to_vec(), e.raw_len)
        })
        .collect())
}

/// What a run makes of each of its files: (logical, its local file, made now: None for none).
type Made = Vec<(String, Option<PathBuf>)>;

/// Checks a run's files against the manifest when they're expected the same (`expect_same`): each
/// with the content name the manifest has, or none where it has none (a mid not yet made aside,
/// which is what the run is for); an error naming those that differ.
fn check_same(out: &Out, made: &[(String, Option<String>)], what: &str) -> Result<()> {
    let differ: Vec<String> = made
        .iter()
        .filter(|(l, c)| match (c, out.get(l)) {
            (None, None) => false,
            (Some(_), None) => !l.starts_with("work/trees-mid/"),
            (c, have) => c.as_deref() != have,
        })
        .map(|(l, c)| format!("{l}: made {}, the manifest has {}", c.as_deref().unwrap_or("none"), out.get(l).unwrap_or("none")))
        .collect();
    anyhow::ensure!(differ.is_empty(), "{what} was expected the same as the manifest has it, and isn't (nothing uploaded): {}", differ.join("; "));
    Ok(())
}

/// Uploads z6 tile `t`'s files from its run in `dir` (the trees program's `--z6`): each layer's hi
/// pack (its zoom 9–12 tiles; the manifest's dropped when it has none) and its mid. `expect_same`:
/// each must come out with the content name the manifest has (its mid, when it has one), else an
/// error and nothing uploaded.
pub fn put_piece(out: &mut Out, t: Unit, dir: &Path, expect_same: bool) -> Result<()> {
    let mut made: Made = Vec::new();
    for layer in LAYERS {
        let tiles = archive_tiles(&dir.join(format!("{layer}.tiles")))?;
        anyhow::ensure!(tiles.iter().all(|&(z, x, y, _, _)| z >= 9 && (x >> (z - 6), y >> (z - 6)) == (t.x, t.y)), "{layer}: a piece's run made tiles outside 6/{}/{}'s zoom 9 to 12", t.x, t.y);
        let p = crate::layers::write_pack_local(out, layer, "terrarium-webp", false, "hi", (6, t.x, t.y), &mut tiles.into_iter())?;
        made.push((format!("layers/{layer}/hi/{}", t.dash()), p.map(|p| p.local)));
    }
    let mid = out.scratch_file(&format!("{}.sect", mid_logical(t.x, t.y)));
    std::fs::copy(dir.join(crate::trees::MID), &mid).with_context(|| format!("{}'s mid", t.slash()))?;
    made.push((mid_logical(t.x, t.y), Some(mid)));
    let what = format!("piece {}", t.slash());
    put_made(out, made, expect_same.then_some(what.as_str()))?;
    eprintln!("trees {}: {} hi packs and its mid", t.slash(), LAYERS.iter().filter(|l| out.get(&format!("layers/{l}/hi/{}", t.dash())).is_some()).count());
    Ok(())
}

/// Uploads a run's files (`made`), dropping from the manifest those it made none of; with `same`,
/// once each is checked against the manifest (`check_same`), else nothing.
fn put_made(out: &mut Out, made: Made, same: Option<&str>) -> Result<()> {
    if let Some(what) = same {
        let names: Vec<(String, Option<String>)> = made
            .iter()
            .map(|(l, p)| {
                let ext = if l.starts_with("work/") { "sect" } else { "pack" };
                Ok((l.clone(), p.as_ref().map(|p| store::naming::hash16_file(p).map(|h| store::naming::content_name(l, &h, ext))).transpose()?))
            })
            .collect::<Result<_>>()?;
        if let Err(e) = check_same(out, &names, what) {
            for (_, p) in &made {
                if let Some(p) = p {
                    std::fs::remove_file(p).ok();
                }
            }
            return Err(e);
        }
    }
    for (l, p) in made {
        match p {
            Some(p) => {
                let ext = if l.starts_with("work/") { "sect" } else { "pack" };
                out.put_file(&l, ext, &p)?;
            }
            None if out.get(&l).is_some() => out.remove(&l),
            None => {}
        }
    }
    out.save()
}

/// Drops z6 tile `t`'s hi packs and mid; how many.
fn drop_piece(out: &mut Out, t: Unit) -> Result<usize> {
    let all: Vec<String> = LAYERS.iter().map(|l| format!("layers/{l}/hi/{}", t.dash())).chain([mid_logical(t.x, t.y)]).collect();
    let gone: Vec<&String> = all.iter().filter(|l| out.get(l).is_some()).collect();
    for l in &gone {
        out.remove(l);
    }
    out.save()?;
    Ok(gone.len())
}

/// Makes z3 tile `q`'s zoomed-out tree cover (an assembly: its lo packs, zoom 4–8) from its pieces'
/// mids (`targets`: the z6 tiles of it the coverage meets, each with its mid in the manifest), and
/// uploads it (`put_lo`); with no piece in it, drops its lo packs. `workers`: the trees program's
/// threads.
pub fn build_lo(out: &mut Out, cov: &Coverage, q: Unit, scratch: &Path, workers: usize) -> Result<()> {
    anyhow::ensure!(q.z == 3, "an assembly is a z3 tile's, not {}", q.slash());
    let tt = targets(cov, &out.manifest);
    let area = q.slash();
    let pieces: Vec<&(String, String, bool)> = tt.pieces_of(&area).collect();
    if pieces.is_empty() {
        let gone: Vec<String> = LAYERS.iter().map(|l| format!("layers/{l}/lo/{}", q.dash())).filter(|l| out.get(l).is_some()).collect();
        for l in &gone {
            out.remove(l);
        }
        out.save()?;
        eprintln!("trees-lo {}: no piece; {} lo packs dropped", q.slash(), gone.len());
        return Ok(());
    }
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    for (t, _, _) in &pieces {
        let u = Unit::parse(t).context("a piece")?;
        let c = out.get(&mid_logical(u.x, u.y)).with_context(|| format!("{t} has no mid yet: its z3 tile can't be assembled"))?;
        args.push(out.path(c).into());
    }
    let dir = scratch.join(format!("trees-lo-{}", q.dash()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    let mut a: Vec<std::ffi::OsString> = vec!["--assemble-lo".into(), "--out".into(), dir.clone().into(), "--workers".into(), workers.to_string().into()];
    a.extend(args);
    run_trees(Path::new("."), false, &a).with_context(|| format!("z3 tile {}'s assembly", q.slash()))?;
    put_lo(out, q, &dir, false)?;
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// Uploads z3 tile `q`'s lo packs from its assembly in `dir` (the trees program's `--assemble-lo`),
/// dropping the manifest's of a layer it made none of. `expect_same`: each as the manifest has it,
/// else an error and nothing uploaded.
pub fn put_lo(out: &mut Out, q: Unit, dir: &Path, expect_same: bool) -> Result<()> {
    let mut made: Made = Vec::new();
    for layer in LAYERS {
        let tiles = archive_tiles(&dir.join(format!("{layer}.tiles")))?;
        anyhow::ensure!(tiles.iter().all(|&(z, x, y, _, _)| (4..=8).contains(&z) && (x >> (z - 3), y >> (z - 3)) == (q.x, q.y)), "{layer}: an assembly made tiles outside 3/{}/{}'s zoom 4 to 8", q.x, q.y);
        let p = crate::layers::write_pack_local(out, layer, "terrarium-webp", false, "lo", (3, q.x, q.y), &mut tiles.into_iter())?;
        made.push((format!("layers/{layer}/lo/{}", q.dash()), p.map(|p| p.local)));
    }
    let what = format!("assembly {}", q.slash());
    put_made(out, made, expect_same.then_some(what.as_str()))?;
    eprintln!("trees-lo {}: {} lo packs", q.slash(), LAYERS.iter().filter(|l| out.get(&format!("layers/{l}/lo/{}", q.dash())).is_some()).count());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;

    fn cov(outline: &str) -> Coverage {
        let d = tempfile::tempdir().unwrap();
        Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec![outline.into()] }], None, d.path()).unwrap()
    }

    #[test]
    fn the_z3_tiles_the_coverage_meets() {
        // Reykjavik's 20 km circle: in z3 tile 3/3/2 only.
        let c = cov("place:-21.9,64.13,20");
        let m = std::collections::BTreeMap::new();
        let t = area_targets(&c, &m);
        assert_eq!(t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2"]);
        // The coverage there is the key: a bigger circle, another key.
        assert_ne!(area_targets(&cov("place:-21.9,64.13,25"), &m)[0].1, t[0].1);
        assert_eq!(area_targets(&cov("place:-21.9,64.13,20"), &m)[0].1, t[0].1);
        // Tree packs where the coverage no longer is (a hi pack of z3 tile 3/4/2): its run drops them.
        let m: std::collections::BTreeMap<String, String> = [("layers/trees-cover/hi/6-33-23".to_string(), "x".to_string())].into();
        let t = area_targets(&c, &m);
        assert_eq!(t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2", "3/4/2"]);
    }

    #[test]
    fn pieces_and_assemblies_keyed_on_what_they_read() {
        // Reykjavik's 20 km circle: z6 tiles 6/28/16 and 6/28/17 (it crosses 64.17° N), in 3/3/2.
        let c = cov("place:-21.9,64.13,20");
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let t = targets(&c, &m);
        let names = |v: &[(String, String, bool)]| v.iter().map(|p| format!("{}{}", p.0, if p.2 { " none" } else { "" })).collect::<Vec<_>>();
        assert_eq!((names(&t.pieces), names(&t.lo)), (vec!["6/28/16".to_string(), "6/28/17".into()], vec!["3/3/2".to_string()]));
        assert_eq!(t.pieces_of("3/3/2").count(), 2);
        // A piece's key is the coverage in it: grown where it reaches one of them, the other's stays.
        let wider = targets(&cov("place:-21.9,64.13,21"), &m);
        assert!(wider.pieces.iter().zip(&t.pieces).all(|(a, b)| a.1 != b.1), "the circle crosses both");
        // An assembly's key is its pieces' mids, by content: one made, another key; made again to the
        // same bytes, the same.
        m.insert(mid_logical(28, 16), "work/trees-mid/6-28-16.1111111111111111.sect".into());
        let with = targets(&c, &m);
        assert_eq!(with.pieces, t.pieces, "the pieces' keys don't read the mids");
        assert_ne!(with.lo[0].1, t.lo[0].1);
        assert_eq!(targets(&c, &m).lo, with.lo);
        // Packs and a mid where the coverage no longer is (6/33/23, in 3/4/2): a "none" piece, whose
        // run drops them, and a "none" assembly for 3/4/2's lo pack.
        m.insert("layers/trees-leaf/hi/6-33-23".into(), "x".into());
        m.insert(mid_logical(33, 22), "y".into());
        m.insert("layers/trees-cover/lo/3-4-2".into(), "z".into());
        let t2 = targets(&c, &m);
        assert_eq!(names(&t2.pieces), ["6/28/16", "6/28/17", "6/33/22 none", "6/33/23 none"]);
        assert_eq!(names(&t2.lo), ["3/3/2", "3/4/2 none"]);
        assert_eq!(t2.pieces_of("3/4/2").count(), 0);
        assert_eq!(area_of("6/33/23").as_deref(), Some("3/4/2"));
    }

    #[test]
    fn a_z3_tiles_shapes_for_the_trees_program() {
        let c = cov("place:-21.9,64.13,20");
        let j = coverage_json(&c, Unit { z: 3, x: 3, y: 2 });
        let rings = j["shapes"][0].as_array().unwrap();
        assert_eq!(rings.len(), 1);
        let p = &rings[0][0];
        assert!((p[0].as_f64().unwrap() + 21.9).abs() < 1.0 && (p[1].as_f64().unwrap() - 64.13).abs() < 1.0);
        // Another z3 tile: no shape there.
        assert!(coverage_json(&c, Unit { z: 3, x: 0, y: 0 })["shapes"].as_array().unwrap().is_empty());
        // A z6 tile it meets, and one it doesn't.
        assert_eq!(coverage_json(&c, Unit { z: 6, x: 28, y: 16 })["shapes"].as_array().unwrap().len(), 1);
        assert!(coverage_json(&c, Unit { z: 6, x: 30, y: 16 })["shapes"].as_array().unwrap().is_empty());
    }

    /// A z3 tile's packs from one run (as `build`), and from its pieces' runs and their assembly
    /// (as `build_piece` and `build_lo`), on the trees tests' squares: the same content names.
    #[test]
    fn pieces_and_their_assembly_make_the_z3_runs_packs() {
        let sq = crate::trees::tests::squares_dir();
        let d = tempfile::tempdir().unwrap();
        // Over z8 blocks 131–134, rows 87 and 88: z6 tiles 6/32/21, 6/32/22, 6/33/21 and 6/33/22 of
        // z3 tile 3/4/2, the squares' data in each.
        std::fs::write(d.path().join("t.poly"), "t\n1\n 5.5 48.85\n 9.5 48.85\n 9.5 48.95\n 5.5 48.95\nEND\nEND\n").unwrap();
        let c = Coverage::from_recipes(&[Recipe { id: "t".into(), name: "T".into(), outline: vec!["poly:t.poly".into()] }], None, d.path()).unwrap();
        let run = |t: Unit, dir: &Path| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("coverage.json"), serde_json::to_vec(&coverage_json(&c, t)).unwrap()).unwrap();
            let r = crate::trees::Run { tile: (t.z, t.x, t.y), coverage: dir.join("coverage.json"), chm: sq.path().into(), chm_store: d.path().join("store"), leaf: sq.path().into(), out: dir.into(), dem: d.path().into() };
            (if t.z == 3 { crate::trees::z3(&r) } else { crate::trees::z6(&r) }).unwrap();
        };
        let out = |name: &str| Out::open(&d.path().join(name), &d.path().join(format!("{name}-scratch"))).unwrap();
        // The whole run.
        let mut whole = out("whole");
        let q = Unit { z: 3, x: 4, y: 2 };
        run(q, &d.path().join("z3"));
        put_area(&mut whole, q, &d.path().join("z3")).unwrap();
        // The pieces, then the assembly from their mids.
        let mut parts = out("parts");
        let tt = targets(&c, &parts.manifest);
        let pieces: Vec<Unit> = tt.pieces_of("3/4/2").map(|p| Unit::parse(&p.0).unwrap()).collect();
        assert_eq!(pieces.iter().map(|u| u.slash()).collect::<Vec<_>>(), ["6/32/21", "6/32/22", "6/33/21", "6/33/22"]);
        for &t in &pieces {
            let dir = d.path().join(format!("p-{}", t.dash()));
            run(t, &dir);
            put_piece(&mut parts, t, &dir, false).unwrap();
        }
        let mids: Vec<PathBuf> = pieces.iter().map(|t| parts.path(parts.get(&mid_logical(t.x, t.y)).unwrap())).collect();
        crate::trees::assemble_lo(&mids, &d.path().join("lo"), &|_, _| {}).unwrap();
        put_lo(&mut parts, q, &d.path().join("lo"), false).unwrap();
        let trees = |o: &Out| o.manifest.iter().filter(|(l, _)| l.starts_with("layers/trees-")).map(|(l, c)| (l.clone(), c.clone())).collect::<BTreeMap<_, _>>();
        assert_eq!(trees(&whole).len(), 15, "a lo pack and four hi packs a layer: {:?}", trees(&whole).keys());
        assert_eq!(trees(&parts), trees(&whole), "the same packs, by content");
        // Each piece's mid, uploaded beside them.
        assert!(pieces.iter().all(|t| parts.get(&mid_logical(t.x, t.y)).is_some()));
        // Made again expecting the same (a mid backfilled): nothing changes.
        let before = parts.manifest.clone();
        let t = pieces[3];
        let dir = d.path().join("again");
        run(t, &dir);
        let mid = mid_logical(t.x, t.y);
        parts.remove(&mid);
        put_piece(&mut parts, t, &dir, true).unwrap();
        assert_eq!(parts.manifest, before, "its packs as they were, its mid made again");
        put_lo(&mut parts, q, &d.path().join("lo"), true).unwrap();
        // A pack that isn't what the manifest has: refused, nothing uploaded.
        run(t, &dir);
        let hi = format!("layers/trees-cover/hi/{}", t.dash());
        parts.manifest.insert(hi.clone(), "layers/trees-cover/hi/6-33-22.0000000000000000.pack".into());
        parts.remove(&mid);
        let e = put_piece(&mut parts, t, &dir, true).unwrap_err().to_string();
        assert!(e.contains(&hi) && e.contains("nothing uploaded"), "{e}");
        assert!(parts.get(&mid).is_none(), "its mid wasn't uploaded");
        // Without expecting the same: uploaded as it is now.
        put_piece(&mut parts, t, &dir, false).unwrap();
        assert_eq!(parts.get(&hi), before.get(&hi).map(String::as_str));
        // The coverage gone from a piece: its packs and mid dropped.
        let gone = Coverage::from_recipes(&[Recipe { id: "f".into(), name: "F".into(), outline: vec!["place:-21.9,64.13,20".into()] }], None, d.path()).unwrap();
        build_piece(&mut parts, &gone, t, d.path(), sq.path(), &d.path().join("s"), 1, false, &|| {}).unwrap();
        assert!(LAYERS.iter().all(|l| parts.get(&format!("layers/{l}/hi/6-33-22")).is_none()) && parts.get(&mid).is_none());
        assert!(build_piece(&mut parts, &gone, pieces[0], d.path(), sq.path(), &d.path().join("s"), 1, true, &|| {}).is_err(), "not as it is: it's gone");
    }

    /// The packs a run's archives in `dir` make (as layers::split_archive_with groups and writes them),
    /// logical → content name: each written in `out`'s scratch folder, hashed and deleted.
    fn pack_names(out: &Out, dir: &Path) -> BTreeMap<String, String> {
        let mut names = BTreeMap::new();
        for layer in LAYERS {
            let mut groups: BTreeMap<(&'static str, u8, u32, u32), Vec<PackTile>> = BTreeMap::new();
            for t in archive_tiles(&dir.join(format!("{layer}.tiles"))).unwrap() {
                groups.entry(crate::layers::pack_of(t.0, t.1, t.2)).or_default().push(t);
            }
            for ((scope, z, x, y), tiles) in groups {
                let p = crate::layers::write_pack_local(out, layer, "terrarium-webp", false, scope, (z, x, y), &mut tiles.into_iter()).unwrap().unwrap();
                names.insert(p.logical.clone(), p.content_name().unwrap());
                std::fs::remove_file(&p.local).unwrap();
            }
        }
        names
    }

    /// A tile's pixels, as RGBA.
    fn pixels(webp: &[u8]) -> Vec<u8> {
        let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(webp)).unwrap();
        let mut px = vec![0u8; d.output_buffer_size().unwrap()];
        d.read_image(&mut px).unwrap();
        if d.has_alpha() {
            px
        } else {
            px.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        }
    }

    /// Byte identity on the build's own data (docs/plan.md §6, Trees), by hand on the build Mac, the
    /// NAS only read: for each z3 tile of `P5T_AREAS` ("3/7/3,3/7/2"), its whole run as the build
    /// makes it today (the baseline), and its pieces' runs and their assembly, from the live coverage
    /// and squares (`P5T_ROOT`, the NAS's project folder; the canopy squares read in place where
    /// `P5T_CHM`, the agent's cache, has them, else on the NAS), into `P5T_DATA`: every pack the same
    /// content name. And against the live packs: which have the same bytes, and whether every tile
    /// has the same pixels.
    #[test]
    #[ignore]
    fn p5_trees_real() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} isn't set"));
        let (root, data, cache) = (PathBuf::from(env("P5T_ROOT")), PathBuf::from(env("P5T_DATA")), PathBuf::from(env("P5T_CHM")));
        let areas: Vec<Unit> = env("P5T_AREAS").split(',').map(|a| Unit::parse(a).filter(|u| u.z == 3).unwrap()).collect();
        let t0 = std::time::Instant::now();
        let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
        let date = crate::osmpass::latest_pass(&root).unwrap();
        let (recipes, bad) = crate::agent::recipes::load(&root.join("inputs/regions"));
        assert!(bad.is_empty(), "{bad:?}");
        let outlines = m.get(&format!("sources/osm/{date}/outlines")).map(|c| crate::outlines::Outlines::open(&root.join(c)).unwrap());
        let cov = Coverage::from_recipes(&recipes, outlines.as_ref(), &root.join("inputs/outlines")).unwrap();
        let tt = targets(&cov, &m);
        eprintln!("pass {date}, {} regions, {} pieces, {} assemblies ({:.0} s)", recipes.len(), tt.pieces.len(), tt.lo.len(), t0.elapsed().as_secs_f64());
        let out = Out::open(&data.join("no-root"), &data.join("scratch")).unwrap();
        let chm = data.join("chm");
        std::fs::create_dir_all(&chm).unwrap();
        let run = |t: Unit, dir: &Path| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("coverage.json"), serde_json::to_vec(&coverage_json(&cov, t)).unwrap()).unwrap();
            let r = crate::trees::Run { tile: (t.z, t.x, t.y), coverage: dir.join("coverage.json"), chm: chm.clone(), chm_store: data.join("store"), leaf: leaf_dir(&root), out: dir.into(), dem: data.clone() };
            (if t.z == 3 { crate::trees::z3(&r) } else { crate::trees::z6(&r) }).unwrap();
        };
        for q in areas {
            let t1 = std::time::Instant::now();
            // The canopy squares its blocks read, linked into a cache of its own: the agent's copies
            // read in place, else the NAS's.
            let shapes = crate::trees::mask::Shapes::parse(&coverage_json(&cov, q).to_string()).unwrap();
            let mut sqs: Vec<(i32, i32)> = crate::trees::z3_blocks(&shapes, q.x, q.y).iter().flat_map(|&(bx, by)| crate::trees::squares_of(crate::trees::tile_bounds(8, bx, by))).collect();
            sqs.sort_unstable();
            sqs.dedup();
            for &(top, left) in &sqs {
                for kind in ["cover5m", "p95"] {
                    let name = crate::trees::chm_name(top, left, kind);
                    let src = if cache.join(&name).exists() { cache.join(&name) } else { root.join("sources/canopy").join(&name) };
                    if !chm.join(&name).exists() {
                        std::os::unix::fs::symlink(&src, chm.join(&name)).unwrap();
                    }
                }
            }
            // The baseline: the z3 tile's whole run.
            let base_dir = data.join(format!("{}-whole", q.dash()));
            run(q, &base_dir);
            let base = pack_names(&out, &base_dir);
            // The live packs against it: their bytes, and their tiles' pixels.
            let live: BTreeMap<&String, &String> = m.iter().filter(|(l, _)| LAYERS.iter().any(|layer| l.starts_with(&format!("layers/{layer}/"))) && (l.ends_with(&format!("/lo/{}", q.dash())) || l.rsplit('/').next().and_then(Unit::parse).is_some_and(|u| u.z == 6 && (u.x >> 3, u.y >> 3) == (q.x, q.y)))).collect();
            let same_bytes = live.iter().filter(|(l, c)| base.get(**l) == Some(**c)).count();
            let (mut tiles, mut same_px) = (0usize, 0usize);
            for (l, c) in &live {
                let layer = l.split('/').nth(1).unwrap();
                let f = store::range::PlainFile::open(&root.join(c)).unwrap();
                let idx = store::pack::PackIndex::read_from(&f).unwrap();
                let arc = roadcore::archive::Archive::open(&base_dir.join(format!("{layer}.tiles"))).unwrap();
                for e in &idx.entries {
                    let (z, x, y) = e.zxy();
                    tiles += 1;
                    let theirs = idx.read_blob(&f, e).unwrap();
                    if arc.get(z, x, y).is_some_and(|ours| pixels(ours) == pixels(&theirs)) {
                        same_px += 1;
                    }
                }
            }
            let ours_tiles: usize = LAYERS.iter().map(|l| roadcore::archive::Archive::open(&base_dir.join(format!("{l}.tiles"))).unwrap().entries().len()).sum();
            std::fs::remove_dir_all(&base_dir).unwrap();
            // The pieces, then their assembly from their mids.
            let mut made: BTreeMap<String, String> = BTreeMap::new();
            let mut mids: Vec<PathBuf> = Vec::new();
            let pieces: Vec<Unit> = tt.pieces_of(&q.slash()).map(|p| Unit::parse(&p.0).unwrap()).collect();
            for &t in &pieces {
                let d = data.join(format!("{}-piece", t.dash()));
                run(t, &d);
                made.extend(pack_names(&out, &d));
                let mid = data.join(format!("mid-{}.sect", t.dash()));
                std::fs::rename(d.join(crate::trees::MID), &mid).unwrap();
                mids.push(mid);
                std::fs::remove_dir_all(&d).unwrap();
            }
            let lo = data.join(format!("{}-lo", q.dash()));
            crate::trees::assemble_lo(&mids, &lo, &|_, _| {}).unwrap();
            made.extend(pack_names(&out, &lo));
            std::fs::remove_dir_all(&lo).unwrap();
            let mid_bytes: u64 = mids.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
            for p in &mids {
                std::fs::remove_file(p).unwrap();
            }
            let differ: Vec<&String> = base.keys().chain(made.keys()).filter(|l| base.get(*l) != made.get(*l)).collect();
            eprintln!(
                "{}: {} pieces, {} canopy squares; the whole run's {} packs, the pieces' and assembly's {}: {} differ{}; mids {:.1} MB; the live {} packs: {same_bytes} with the same bytes; their {tiles} tiles (the run's {ours_tiles}): {same_px} with the same pixels ({:.0} s)",
                q.slash(),
                pieces.len(),
                sqs.len(),
                base.len(),
                made.len(),
                differ.len(),
                if differ.is_empty() { String::new() } else { format!(" ({differ:?})") },
                mid_bytes as f64 / 1e6,
                live.len(),
                t1.elapsed().as_secs_f64()
            );
            assert!(differ.is_empty(), "{}: the pieces and their assembly differ from the whole run", q.slash());
        }
    }
}
