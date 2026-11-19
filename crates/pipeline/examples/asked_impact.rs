//! What task #1473's keys do on the build's real records, read only (the z6 tiles' coverage lines
//! of pois, unit, trees and bldtiles from what each run asked of the coverage: buildstore::asked).
//!
//! - `keys <root>`: every target of the four steps now, its coverage line beside the geometric
//!   line its key had before (`idents::clip_geo`, `Reach::coverage_key`, `inside_geo`,
//!   `shapes_geo`): how many differ (none while a target has no record: its key is the same, so the
//!   change re-keys nothing and runs nothing again), and how many records there are.
//! - `case <root> <scratch> <old regions index> <new regions index> <step>:<z/x/y> …`: each tile's
//!   run's questions asked of the old regions' coverage as its run asks them (a pois or unit tile's
//!   extract made by `scenic-build tool extract` beside this program, its candidates or its ways;
//!   a bldtiles tile's buildings from the normalized files; a trees tile's mask in its blocks'
//!   live cells, from the trees program run under the old coverage: `trees_live`), its record put
//!   under the line it then had (its geometry's), and the line the new regions give it: kept or
//!   moved. Beside it: whether the geometry its key had before moved, and whether what its run
//!   reads of the coverage changed, exactly (its candidates kept, its ways kept, its own buildings'
//!   countries, its mask in its live cells), with the record's size and the time to ask again.
//!   `<step>:moved[@k]`: each (k-th) of its targets whose geometry moved, with a
//!   sum; a trees tile's mid made beside the build's (made under the new regions once run again):
//!   one differing, its output changed, with its line kept would be unsound. The
//!   indexes are content hashes (`sources/inputs/regions/@index.<hash>.json`); the coverages are
//!   kept in `<scratch>/coverage` (mapped, not held).
//!
//! usage: asked_impact keys <root>
//!        asked_impact case <root> <scratch> <old> <new> <step>:<z/x/y> …

use anyhow::{bail, Context, Result};
use buildstore::asked::{target_line, Record};
use buildstore::coverage::{Asking, Coverage};
use codecs::tile::TileId;
use pipeline::agent::build::KeyCtx;
use pipeline::agent::idents;
use pipeline::agent::tiles::TerrainTiles;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;
use store::nasfs::NasRoot;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keys") if args.len() == 2 => keys(&NasRoot::from_arg(&args[1])),
        Some("case") if args.len() >= 6 => {
            case(&NasRoot::from_arg(&args[1]), Path::new(&args[2]), &args[3], &args[4], &args[5..])
        }
        _ => bail!("usage: asked_impact keys <root> | case <root> <scratch> <old> <new> <step>:<z/x/y> …"),
    }
}

fn pass(root: &NasRoot) -> Result<String> {
    inputs::osmpass::latest_pass_by(root, false).context("no complete pass")
}

/// The regions' coverage of index `hash`, kept in `cache`.
fn coverage(root: &NasRoot, m: &BTreeMap<String, String>, hash: &str, cache: Option<&Path>) -> Result<Coverage> {
    use inputs::inputs::regions;
    let mut m = m.clone();
    m.insert(inputs::inputs::logical(regions::UNIT), format!("sources/inputs/regions/@index.{hash}.json"));
    let a = inputs::inputs::open(root, &m, regions::UNIT)?.context("no regions index")?;
    let t = Instant::now();
    let c = regions::read::accepted_coverage(root, &a, cache)?;
    eprintln!("coverage {hash}: {} shapes ({:.0?})", c.shapes.len(), t.elapsed());
    Ok(c)
}

fn keys(root: &NasRoot) -> Result<()> {
    let m = buildstore::records::read_manifest(root)?;
    let date = pass(root)?;
    let a = inputs::inputs::regions::read::source(root, &m)?;
    let cov = inputs::inputs::regions::read::accepted_coverage(root, &a, None)?;
    let reach = roads::reach::Reaches::load(root, &m, &date).ok().flatten().context("no reaches")?;
    let records: usize = ["pois", "unit", "trees", "bldtiles"]
        .iter()
        .map(|s| {
            let p = format!("work/asked/{s}/");
            m.range(p.clone()..).take_while(|(l, _)| l.starts_with(&p)).count()
        })
        .sum();
    let (inputs, tiles) = (BTreeMap::new(), TerrainTiles::default());
    let x = KeyCtx::new(&cov, &date, &m, &inputs, Some(&reach), &tiles);
    let fine = tiles.spacing(&m);
    let mut out: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut count = |step: &'static str, now: Option<Vec<String>>, before: String| {
        let e = out.entry(step).or_default();
        e.0 += 1;
        if now.as_ref().and_then(|v| v.first()) != Some(&before) {
            e.1 += 1;
            eprintln!("{step}: {now:?} beside {before}");
        }
    };
    for (u, _, _) in pipeline::agent::build::pois_units(&cov, &date, &m) {
        count("pois", idents::coverage_clip(&u.slash(), &x), idents::clip_geo(&cov, u));
    }
    for x6 in 0..64u32 {
        for y6 in 0..64u32 {
            let u = TileId { z: 6, x: x6, y: y6 };
            let t = u.slash();
            if let Some(r) = reach.get(u).filter(|r| r.builds(&cov, &*fine)) {
                let now = idents::ways(&t, &x).map(|v| v.into_iter().take(1).collect());
                count("unit", now, format!("coverage {}", r.coverage_key(&cov, u, &*fine)));
            }
            if cov.meets_rect(codecs::hipack::tile_bounds(6, x6, y6)) {
                count("trees", idents::coverage_inside(&t, &x), idents::inside_geo(&cov, u));
            }
            if let Some(now) = idents::coverage_shapes(&t, &x) {
                count("bldtiles", Some(now), idents::shapes_geo(&cov, u));
            }
        }
    }
    println!("records of what a target asked: {records}");
    for (step, (n, moved)) in out {
        println!("{step}: {n} targets, {moved} whose coverage line isn't the one it had before");
    }
    Ok(())
}

/// What a tile's run read of the coverage, as `case` measures it.
struct Asked {
    /// The run's record of what it asked the old coverage, under its line then.
    record: Record,
    /// What its run reads of the coverage, exactly, under the old regions and the new.
    exact: (String, String),
    /// A trees tile's: whether its run under the old regions made the mid the build has (made
    /// under the new, once it ran again since).
    same_mid: Option<bool>,
}

/// `step`'s questions of tile `u` asked of `old`, and what its run reads exactly of `old` and `new`.
fn asked(
    root: &NasRoot,
    m: &BTreeMap<String, String>,
    date: &str,
    scratch: &Path,
    step: &str,
    u: TileId,
    old: &Coverage,
    new: &Coverage,
) -> Result<Asked> {
    let digest = |v: &dyn std::fmt::Debug| store::naming::hash16(format!("{v:?}").as_bytes());
    let geo = |c: &Coverage| -> Result<String> {
        Ok(match step {
            "pois" => idents::clip_geo(c, u),
            "trees" => idents::inside_geo(c, u),
            "bldtiles" => idents::shapes_geo(c, u),
            "unit" => {
                let reach = roads::reach::Reaches::load(root, m, date).ok().flatten().context("no reaches")?;
                let fine = inputs::dem::spacing::Fine::of_records(root, m)?;
                reach.get(u).context("no reach")?.coverage_key(c, u, &fine)
            }
            s => bail!("{s}: not a step keyed on what each target asked"),
        })
    };
    let g = geo(old)?;
    let a = Asking::new(old);
    let mut also = Vec::new();
    let mut same_mid = None;
    let exact = match step {
        "trees" => {
            let (live, same) = trees_live(root, m, scratch, u, old)?;
            same_mid = Some(same);
            let q = trees::treepacks::mask_question(&live);
            let ans = |c: &Coverage| trees::treepacks::mask_answer(c, u, &q).context("a mask question");
            let (o, n) = (ans(old)?, ans(new)?);
            let whole = trees::treepacks::mask_question(&Default::default());
            let all = |c: &Coverage| trees::treepacks::mask_answer(c, u, &whole).context("a mask question");
            if all(old)? != all(new)? {
                let cells: usize = live.values().flatten().map(|w| w.count_ones() as usize).sum();
                eprintln!("trees {}: its whole mask moved: {}; {cells} live cells", u.slash(), mask_diff(old, new, u)?);
            }
            also.push((q, o.clone()));
            (o, n)
        }
        "pois" | "unit" => {
            let dir = scratch.join(format!("{step}-{}", u.dash()));
            std::fs::create_dir_all(&dir)?;
            let piece = dir.join("piece.osm.pbf");
            let p = m.get(&format!("sources/osm/{date}/pieces/{}", u.dash())).context("no piece")?;
            root.join(p).copy_data_to_local(&piece)?;
            let spacing = dir.join(inputs::dem::spacing::FILE);
            inputs::dem::spacing::Fine::of_records(root, m)?.write(&spacing)?;
            let bin = std::env::current_exe()?.parent().context("bin")?.join("scenic-build");
            let mut c = store::spawn::command(&bin);
            c.args(["tool", "extract"]).arg(&dir).arg(&spacing);
            if step == "pois" {
                let ends = m.get(&format!("work/trailends/{date}")).context("no trailends")?;
                c.arg("--candidates").arg("--trailends").arg(root.join(ends).unchecked_path());
            }
            let t = Instant::now();
            c.arg(&piece);
            let o = store::spawn::output(&mut c).context("run extract")?;
            anyhow::ensure!(o.status.success(), "extract: {}", String::from_utf8_lossy(&o.stderr));
            eprintln!("{step} {}: extract {:.0?}", u.slash(), t.elapsed());
            let e = if step == "pois" {
                let pois = std::fs::read(dir.join("pois.json"))?;
                let keys =
                    |cands: Vec<codecs::marks::Cand>| digest(&cands.iter().map(|c| c.key.clone()).collect::<Vec<_>>());
                let o = keys(roads::candidates::from_pois(&pois, u, &a)?);
                (o, keys(roads::candidates::from_pois(&pois, u, &Asking::new(new))?))
            } else {
                let w = roadcore::Ways::open(&dir)?;
                let (ways, verts) = (w.ways(), w.verts());
                let kept = |ask: &Asking| -> Vec<i64> {
                    ways.iter()
                        .filter(|r| ask.touches(&verts[r.vstart as usize..(r.vstart + r.vcount as u64) as usize]))
                        .map(|r| r.id)
                        .collect()
                };
                let o = kept(&a);
                eprintln!("unit {}: {} ways, {} kept of the old coverage", u.slash(), ways.len(), o.len());
                (digest(&o), digest(&kept(&Asking::new(new))))
            };
            std::fs::remove_dir_all(&dir).ok();
            e
        }
        _ => {
            let mut files: Vec<Option<bld::bld::work::WorkFile>> = Vec::new();
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (x, y) = (u.x as i64 + dx, u.y as i64 + dy);
                    let f = (0..64).contains(&x) && (0..64).contains(&y);
                    let c = f.then(|| m.get(&bld::bld::work_logical(x as u32, y as u32))).flatten();
                    files.push(c.map(|c| bld::bld::work::WorkFile::open(&root.join(c))).transpose()?);
                }
            }
            let t = Instant::now();
            bld::bld::job::ask(&files, &a, u)?;
            eprintln!("bldtiles {}: asked ({:.0?})", u.slash(), t.elapsed());
            // (Its own buildings' countries, as the fill reads them: shape_of.)
            let own = |c: &Coverage| -> Result<String> {
                let mut h = Vec::new();
                if let Some(f) = &files[4] {
                    for e in &f.index {
                        let b = f.block(e)?;
                        for i in 0..b.len() {
                            let s =
                                c.country_at(b.cen[i]).or_else(|| b.verts_of(i).iter().find_map(|&p| c.country_at(p)));
                            h.push(s.map(str::to_string));
                        }
                    }
                }
                Ok(digest(&h))
            };
            (own(old)?, own(new)?)
        }
    };
    let record = Record::read_as(old, g.clone(), a.asked(), also, g);
    Ok(Asked { record, exact, same_mid })
}

fn case(root: &NasRoot, scratch: &Path, old: &str, new: &str, tiles: &[String]) -> Result<()> {
    std::fs::create_dir_all(scratch)?;
    let m = buildstore::records::read_manifest(root)?;
    let date = pass(root)?;
    let cache = scratch.to_path_buf();
    let (old_c, new_c) = (coverage(root, &m, old, Some(&cache))?, coverage(root, &m, new, Some(&cache))?);
    // (`<step>:moved`, trees or bldtiles: each of its targets whose geometry moved.)
    let mut each: Vec<(String, TileId)> = Vec::new();
    for st in tiles {
        let (step, t) = st.split_once(':').context("<step>:<z/x/y>")?;
        let Some(every) = t.strip_prefix("moved") else {
            each.push((step.to_string(), TileId::parse(t).context("a tile")?));
            continue;
        };
        // (`moved@k`: every k-th of them.)
        let every: usize = match every.strip_prefix('@') {
            Some(k) => k.parse().context("moved@<k>")?,
            None => 1,
        };
        let mut n = 0usize;
        let geo: Box<dyn Fn(&Coverage, TileId) -> String> = match step {
            "trees" => Box::new(idents::inside_geo),
            "bldtiles" => Box::new(idents::shapes_geo),
            "pois" => Box::new(idents::clip_geo),
            "unit" => {
                let reach = roads::reach::Reaches::load(root, &m, &date).ok().flatten().context("no reaches")?;
                let fine = inputs::dem::spacing::Fine::of_records(root, &m)?;
                Box::new(move |c: &Coverage, u: TileId| {
                    reach.get(u).map(|r| r.coverage_key(c, u, &fine)).unwrap_or_default()
                })
            }
            s => bail!("{s}:moved: not a step keyed on what each target asked"),
        };
        eprintln!("{step}:moved: every {every}");
        for x in 0..64u32 {
            for y in 0..64u32 {
                let u = TileId { z: 6, x, y };
                if new_c.meets_rect(codecs::hipack::tile_bounds(6, x, y)) && geo(&old_c, u) != geo(&new_c, u) {
                    if n % every == 0 {
                        each.push((step.to_string(), u));
                    }
                    n += 1;
                }
            }
        }
    }
    println!("step tile | geometry moved | read exactly changed | key line moved | record (bytes: points, cells, countries, country cells) | ask again | trees: the build's mid (made under the new regions once run again) differs from the old regions' run's");
    // By step: tiles, geometry moved, read changed, line moved, line moved with nothing read changed,
    // read changed with the line kept (never: it would be unsound); trees: mids differing, those
    // with the line kept (never), those with it moved.
    let mut sums: BTreeMap<String, [usize; 9]> = BTreeMap::new();
    for (step, u) in each {
        let (step, u) = (step.as_str(), u);
        // (One that can't be asked here, left out, said: a trees tile whose run would fetch or
        // make squares on the NAS.)
        let a = match asked(root, &m, &date, scratch, step, u, &old_c, &new_c) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{step} {}: left out: {e:#}", u.slash());
                continue;
            }
        };
        let geo_new = match step {
            "pois" => idents::clip_geo(&new_c, u),
            "trees" => idents::inside_geo(&new_c, u),
            "bldtiles" => idents::shapes_geo(&new_c, u),
            _ => {
                let reach = roads::reach::Reaches::load(root, &m, &date).ok().flatten().context("no reaches")?;
                let fine = inputs::dem::spacing::Fine::of_records(root, &m)?;
                reach.get(u).context("no reach")?.coverage_key(&new_c, u, &fine)
            }
        };
        let bytes = a.record.to_bytes();
        let content = store::naming::hash16(&bytes);
        let t0 = Instant::now();
        let reask = |q: &str| trees::treepacks::mask_answer(&new_c, u, q);
        let line = target_line(&new_c, &geo_new, Some((&content, &a.record)), &reask).context("can't ask again")?;
        let r = &a.record.asked;
        let (geo_moved, read, moved) = (geo_new != a.record.version, a.exact.0 != a.exact.1, line != a.record.line);
        let s = sums.entry(step.to_string()).or_default();
        let differs = a.same_mid == Some(false);
        let flags = [
            true,
            geo_moved,
            read,
            moved,
            moved && !read,
            read && !moved,
            differs,
            differs && !moved,
            differs && moved,
        ];
        for (k, b) in flags.into_iter().enumerate() {
            s[k] += usize::from(b);
        }
        println!(
            "{step} {} | {} | {} | {} | {} ({}, {}, {}, {}) | {:.0?} | {}",
            u.slash(),
            yes(geo_moved),
            yes(read),
            yes(moved),
            bytes.len(),
            r.points.len(),
            r.cells.len(),
            r.countries.len(),
            r.country_cells.len(),
            t0.elapsed(),
            a.same_mid.map_or("-", |s| yes(!s))
        );
    }
    for (step, s) in sums {
        println!(
            "{step}: {} tiles, geometry moved {}, read changed {}, line moved {} ({} with nothing read changed, {} read changed and kept); trees: mid differs {} ({} with the line kept, {} moved)",
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8]
        );
    }
    Ok(())
}

fn yes(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Where a trees tile's masks under the two coverages part: its blocks (`blocks_of`) under each,
/// those only one has and how many of their pixels are inside, and the shared ones whose pixels
/// differ, with how many.
fn mask_diff(old: &Coverage, new: &Coverage, u: TileId) -> Result<String> {
    use trees::trees::{blocks_of, mask, ZBLOCK};
    let masks = |c: &Coverage| -> Result<BTreeMap<(u32, u32), Vec<u64>>> {
        let s = mask::Shapes::parse(&trees::treepacks::coverage_json(c, u).to_string())?;
        Ok(blocks_of(&s, u.z, u.x, u.y)
            .into_iter()
            .map(|(bx, by)| {
                let b = roadcore::geo::tile_box(ZBLOCK, bx, by);
                ((bx, by), mask::inside(&s.meeting(b), b))
            })
            .collect())
    };
    let (o, n) = (masks(old)?, masks(new)?);
    let ones = |m: &[u64]| m.iter().map(|w| w.count_ones() as u64).sum::<u64>();
    let only = |a: &BTreeMap<(u32, u32), Vec<u64>>, b: &BTreeMap<(u32, u32), Vec<u64>>| {
        let k: Vec<_> = a.keys().filter(|k| !b.contains_key(k)).collect();
        (k.len(), k.iter().map(|k| ones(&a[k])).sum::<u64>())
    };
    let (oo, no) = (only(&o, &n), only(&n, &o));
    let mut differ = (0, 0u64, 0u64);
    for (k, m) in &o {
        if let Some(m2) = n.get(k) {
            if m != m2 {
                differ.0 += 1;
                differ.1 += m.iter().zip(m2).map(|(a, b)| (a ^ b).count_ones() as u64).sum::<u64>();
                differ.2 += ones(m);
            }
        }
    }
    Ok(format!(
        "blocks {} then {}; only before {} ({} pixels inside), only now {} ({} pixels inside); shared and differing {} ({} pixels differ of {} inside)",
        o.len(),
        n.len(),
        oo.0,
        oo.1,
        no.0,
        no.1,
        differ.0,
        differ.1,
        differ.2
    ))
}

/// Trees z6 tile `u`'s blocks' live cells under coverage `old` (trees::trees::BlockOut::live), from
/// the trees program (`scenic-build tool trees` beside this program, this tree's) run in
/// `scratch` as its job runs it, reading the NAS's canopy and leaf-type squares where they lie.
/// Refused unless every square it reads is there already (the program would download or make it
/// on the NAS) and its leaf-type squares are made. Beside them, whether the mid it made is
/// the one the build has.
fn trees_live(
    root: &NasRoot,
    m: &BTreeMap<String, String>,
    scratch: &Path,
    u: TileId,
    old: &Coverage,
) -> Result<(trees::trees::Live, bool)> {
    use trees::trees::{blocks_of, chm_name, mask, squares_of, RunSources, ZBLOCK};
    let dir = scratch.join(format!("trees-{}", u.dash()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir)?;
    let cj = trees::treepacks::coverage_json(old, u);
    std::fs::write(dir.join("coverage.json"), serde_json::to_vec(&cj)?)?;
    let sources = RunSources::of(&inputs::inputs::sources::read(root, m)?)?;
    std::fs::write(dir.join("sources.json"), serde_json::to_vec(&sources)?)?;
    let shapes = mask::Shapes::parse(&cj.to_string())?;
    let mut sqs: Vec<(i32, i32)> = blocks_of(&shapes, u.z, u.x, u.y)
        .into_iter()
        .flat_map(|(bx, by)| squares_of(roadcore::geo::tile_box(ZBLOCK, bx, by)))
        .collect();
    sqs.sort_unstable();
    sqs.dedup();
    let store = root.join("sources/canopy");
    let meets = |b: [f64; 4], (top, left): (i32, i32)| {
        (left as f64) < b[2] && (left + 10) as f64 > b[0] && ((top - 10) as f64) < b[3] && top as f64 > b[1]
    };
    for &sq in &sqs {
        for kind in ["cover5m", "p95"] {
            let f = store.join(chm_name(sq.0, sq.1, kind));
            anyhow::ensure!(f.exists(), "{}: {} isn't on the NAS (the run would download it)", u.slash(), f.display());
        }
        let leaf = trees::treepacks::leaf_dir(root).join(trees::trees::leaf_name(sq.0, sq.1)).unchecked_path();
        anyhow::ensure!(
            // (Made only for a square with canopy, `squares::leaf_types`.)
            !(meets(sources.leaf.europe_box, sq) || meets(sources.leaf.north_america_box, sq))
                || store.join(chm_name(sq.0, sq.1, "cover5m")).metadata()?.len() == 0
                || trees::trees::squares::complete(&leaf),
            "{}: square {sq:?}'s leaf types aren't made (the run would make them on the NAS)",
            u.slash()
        );
    }
    let bin = std::env::current_exe()?.parent().context("bin")?.join("scenic-build");
    let mut c = store::spawn::command(&bin);
    c.args(["tool", "trees", "--tile", &u.slash()]);
    c.arg("--coverage").arg(dir.join("coverage.json")).arg("--sources").arg(dir.join("sources.json"));
    c.arg("--chm").arg(dir.join("chm")).arg("--chm-store").arg(store.unchecked_path());
    c.arg("--leaf").arg(trees::treepacks::leaf_dir(root).unchecked_path()).arg("--out").arg(&dir);
    c.args(["--workers", "4"]);
    c.env_remove("SCENIC_CACHE");
    let t = Instant::now();
    let st = store::spawn::status(&mut c)?;
    anyhow::ensure!(st.success(), "{}: the trees program: {st}", u.slash());
    let live = trees::trees::read_live(&dir.join(trees::trees::LIVE))?;
    let l = trees::treepacks::mid_logical(u.x, u.y);
    let made = store::naming::content_name(&l, &store::naming::hash16_file(&dir.join(trees::trees::MID))?, "sect");
    eprintln!("trees {}: its run under the old coverage ({:.0?})", u.slash(), t.elapsed());
    std::fs::remove_dir_all(&dir).ok();
    Ok((live, m.get(&l) == Some(&made)))
}
