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
//!   a bldtiles tile's buildings from the normalized files; a trees tile's mask), its record put
//!   under the line it then had (its geometry's), and the line the new regions give it: kept or
//!   moved. Beside it: whether the geometry its key had before moved, and whether what its run
//!   reads of the coverage changed, exactly (its candidates kept, its ways kept, its own buildings'
//!   countries, its mask), with the record's size and the times to ask and to ask again. The
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
    let exact = match step {
        "trees" => {
            let (o, n) = (trees::treepacks::mask_digest(old, u), trees::treepacks::mask_digest(new, u));
            also.push((idents::MASK_QUESTION.to_string(), o.clone()));
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
    Ok(Asked { record, exact })
}

fn case(root: &NasRoot, scratch: &Path, old: &str, new: &str, tiles: &[String]) -> Result<()> {
    std::fs::create_dir_all(scratch)?;
    let m = buildstore::records::read_manifest(root)?;
    let date = pass(root)?;
    let cache = scratch.to_path_buf();
    let (old_c, new_c) = (coverage(root, &m, old, Some(&cache))?, coverage(root, &m, new, Some(&cache))?);
    println!("step tile | geometry moved | read exactly changed | key line moved | record (bytes: points, cells, countries, country cells) | ask again");
    for st in tiles {
        let (step, t) = st.split_once(':').context("<step>:<z/x/y>")?;
        let u = TileId::parse(t).context("a tile")?;
        let a = asked(root, &m, &date, scratch, step, u, &old_c, &new_c)?;
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
        let reask = |q: &str| idents::mask_answer(&new_c, u, q);
        let line = target_line(&new_c, &geo_new, Some((&content, &a.record)), &reask).context("can't ask again")?;
        let r = &a.record.asked;
        println!(
            "{step} {} | {} | {} | {} | {} ({}, {}, {}, {}) | {:.0?}",
            u.slash(),
            yes(geo_new != a.record.version),
            yes(a.exact.0 != a.exact.1),
            yes(line != a.record.line),
            bytes.len(),
            r.points.len(),
            r.cells.len(),
            r.countries.len(),
            r.country_cells.len(),
            t0.elapsed()
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
