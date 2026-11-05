//! Lakes across the deep targets' edges (task #942), on the build's real records, writing nothing but
//! the Terrarium tiles' local cache.
//!
//! - `list <6/x/y …>`: the z6 tiles' terrain-deep targets (crate::terrain_deep::targets_in), the lakes
//!   (by OSM id) in their tiles at the zoom their levels are decided at (the finest of z13–14 they
//!   make: the basemap's water stops at z14), and those in two targets or more.
//! - `levels <6/x/y | 7/x/y | 8/x/y> [<id> …]`: each lake across the z10 tiles' edges that the tile's targets
//!   reach (crate::terrain_lakes::Lakes::across; or only lakes `id`), its level in each of its
//!   targets there as the target's run gives it, before (#929: decided from the target's own tiles)
//!   and after (the terrain-lakes step's level, decided once, given to the run): the target's tiles
//!   holding the lake made as its run makes them (crate::terrain_pack::make_piece_logged over their
//!   subtrees: a tile's heights are its subtree's alone), and how far apart a lake's levels are.
//!
//! usage: [LAKE_REGIONS=<id,…>] cargo run -p pipeline --example lake_steps -- <root> <raw cache
//! dir> <max MB> <mode> …

use anyhow::Context;
use std::collections::{BTreeMap, BTreeSet};

fn mb() -> u64 {
    jobrt::sys::footprint().unwrap_or(0) / 1_000_000
}

fn tile(s: &str, z: u8) -> anyhow::Result<(u32, u32)> {
    let v: Vec<u32> = s.split('/').map(|v| v.parse()).collect::<Result<_, _>>()?;
    anyhow::ensure!(v.len() == 3 && v[0] == z as u32, "{s}: not a z{z} tile");
    Ok((v[1], v[2]))
}

type Levels = Vec<(u8, Vec<(u32, u32)>)>;

/// The zoom a target's lakes are levelled at: its finest level with water (z14 at most).
fn level_zoom(levels: &Levels) -> Option<(u8, Vec<(u32, u32)>)> {
    levels.iter().find(|(z, ts)| *z <= 14 && !ts.is_empty()).cloned()
}

/// By lake (OSM id), its targets and their tiles holding it at their level zoom; and each target's
/// levels.
#[allow(clippy::type_complexity)]
fn lakes_of(
    targets: &[(u32, u32)],
    cov: &pipeline::coverage::Coverage,
    sets: &[pipeline::terrain_zoom::Set],
    water: &dyn pipeline::terrain_water::WaterSource,
) -> anyhow::Result<(BTreeMap<u64, BTreeMap<(u32, u32), Vec<(u8, u32, u32)>>>, BTreeMap<(u32, u32), Levels>)> {
    use pipeline::terrain_water::Kind;
    let mut by_lake: BTreeMap<u64, BTreeMap<(u32, u32), Vec<(u8, u32, u32)>>> = BTreeMap::new();
    let mut all = BTreeMap::new();
    for &t in targets {
        let lv = pipeline::terrain_deep::levels(cov, sets, t);
        let Some((z, ts)) = level_zoom(&lv) else { continue };
        all.insert(t, lv);
        let asked: Vec<(u8, u32, u32)> = ts.iter().map(|&(x, y)| (z, x, y)).collect();
        for (polys, &u) in water.depth_polys_many(&asked)?.into_iter().zip(&asked) {
            let ids: BTreeSet<u64> =
                polys.iter().filter(|p| p.kind == Kind::Lake && p.id >> 63 == 0).map(|p| p.id).collect();
            for id in ids {
                by_lake.entry(id).or_default().entry(t).or_default().push(u);
            }
        }
    }
    Ok((by_lake, all))
}

/// Target `lv`'s levels cut to the subtrees of `under` (tiles of one zoom), down from its finest.
fn cut(lv: &Levels, under: &BTreeSet<(u8, u32, u32)>) -> Levels {
    let zl = under.iter().next().map_or(0, |u| u.0);
    lv.iter()
        .filter(|(z, _)| *z >= zl)
        .map(|(z, ts)| {
            let k = z - zl;
            (*z, ts.iter().copied().filter(|&(x, y)| under.contains(&(zl, x >> k, y >> k))).collect())
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 6, "usage: lake_steps <root> <raw cache dir> <max MB> list|levels …");
    let (root, raw, max, mode, rest) = (&args[1], &args[2], &args[3], args[4].as_str(), &args[5..]);
    jobrt::sys::footprint_guard("lake_steps", max.parse()?);
    let nas = store::nasfs::NasRoot::from_arg(root);
    let scratch = std::path::Path::new(raw).join("scratch");
    let out = pipeline::out::Out::unbound(&nas, &scratch)?;
    let m = out.manifest.clone();
    // (`LAKE_REGIONS`: some regions' ids, comma-separated, their coverage alone kept: the whole
    // world's holds ~1.8 GB.)
    let src = pipeline::inputs::regions::read::source(&nas, &m)?;
    let mut cov = pipeline::inputs::regions::read::accepted_coverage(&nas, &src, None)?;
    if let Ok(ids) = std::env::var("LAKE_REGIONS") {
        let ids: Vec<&str> = ids.split(',').collect();
        cov.shapes.retain(|s| ids.contains(&s.source.split_once(": ").map_or(s.source.as_str(), |(id, _)| id)));
        cov.shapes.shrink_to_fit();
        cov.depth_only = Vec::new();
    }
    let sets = pipeline::terrain_zoom::Declared::read(&nas, &m)?.map(|d| d.sets).unwrap_or_default();
    let water = pipeline::terrain_pack::open_water(&out)?;
    eprintln!("read: {} MB", mb());
    match mode {
        "list" => {
            for t6 in rest {
                let t6 = tile(t6, 6)?;
                let t0 = std::time::Instant::now();
                let targets = pipeline::terrain_deep::targets_in(&sets, &cov, t6);
                let (by_lake, lv) = lakes_of(&targets, &cov, &sets, &water)?;
                let mut zs: BTreeMap<u8, usize> = BTreeMap::new();
                for l in lv.values() {
                    *zs.entry(level_zoom(l).unwrap().0).or_default() += 1;
                }
                let across: Vec<_> = by_lake.iter().filter(|(_, ts)| ts.len() > 1).collect();
                println!(
                    "6/{}/{}: {} targets (level zooms {zs:?}), {} lakes, {} in two targets or more ({:.0} s, {} MB)",
                    t6.0,
                    t6.1,
                    targets.len(),
                    by_lake.len(),
                    across.len(),
                    t0.elapsed().as_secs_f64(),
                    mb()
                );
                let mut hist: BTreeMap<usize, usize> = BTreeMap::new();
                for (_, ts) in &across {
                    *hist.entry(ts.len()).or_default() += 1;
                }
                println!("  targets a lake: {hist:?}");
                let mut big: Vec<_> =
                    across.iter().map(|(id, ts)| (ts.values().map(Vec::len).sum::<usize>(), **id, *ts)).collect();
                big.sort_by(|a, b| b.0.cmp(&a.0));
                for (n, id, ts) in big.iter().take(15) {
                    let names: Vec<String> = ts.iter().map(|((x, y), k)| format!("10/{x}/{y}:{}", k.len())).collect();
                    println!("  lake {id}: {n} tiles in {} targets: {}", ts.len(), names.join(" "));
                }
            }
        }
        "levels" => {
            // (A z6 tile's targets, or those under a z7 or z8 tile of it: a run's memory is its area's.)
            let u = pipeline::legacy::TileId::parse(&rest[0])
                .filter(|u| (6..=8).contains(&u.z))
                .context("6/x/y, 7/x/y or 8/x/y")?;
            let t6 = (u.x >> (u.z - 6), u.y >> (u.z - 6));
            let only: BTreeSet<u64> = rest[1..].iter().map(|s| s.parse()).collect::<Result<_, _>>()?;
            let k = pipeline::terrain_deep::PACK_Z - u.z;
            let targets: Vec<(u32, u32)> = pipeline::terrain_deep::targets_in(&sets, &cov, t6)
                .into_iter()
                .filter(|t| (t.0 >> k, t.1 >> k) == (u.x, u.y))
                .collect();
            let (by_lake, lv) = lakes_of(&targets, &cov, &sets, &water)?;
            // (The lakes across: as the terrain-lakes step finds them, in two z10 tiles or more.)
            let lakes = pipeline::terrain_lakes::Lakes::new(&water);
            let mut across = lakes.across(&targets)?;
            if !only.is_empty() {
                let keep: Vec<bool> = across.lakes.iter().map(|(id, _)| only.contains(id)).collect();
                for ns in across.of.values_mut() {
                    ns.retain(|&n| keep[n]);
                }
                across.of.retain(|_, ns| !ns.is_empty());
            }
            // (Each target's tiles holding a lake across, at its level zoom.)
            let mut want: BTreeMap<(u32, u32), BTreeSet<(u8, u32, u32)>> = BTreeMap::new();
            for (t, ns) in &across.of {
                for &n in ns {
                    if let Some(us) = by_lake.get(&across.lakes[n].0).and_then(|ts| ts.get(t)) {
                        want.entry(*t).or_default().extend(us.iter().copied());
                    }
                }
            }
            let ids: BTreeSet<u64> = across.of.values().flatten().map(|&n| across.lakes[n].0).collect();
            let ts: Vec<(u32, u32)> = want.keys().copied().collect();
            // (Only the lakes measured sampled for their levels.)
            let shore: BTreeMap<(u32, u32), Vec<usize>> = lakes
                .to_sample(&across)?
                .into_iter()
                .filter_map(|(t, ns)| {
                    let ns: Vec<usize> = ns.into_iter().filter(|&n| ids.contains(&across.lakes[n].0)).collect();
                    (!ns.is_empty()).then_some((t, ns))
                })
                .collect();
            eprintln!(
                "{} lakes across {} targets' edges, {} shore tiles; {} MB",
                ids.len(),
                ts.len(),
                shore.len(),
                mb()
            );
            let rt = pipeline::terrain_pack::RawTiles::with_store(
                std::path::Path::new(raw),
                &nas.join("sources/aws-terrarium"),
                &pipeline::terrain_base::terrarium_template(&nas, &m)?,
            );
            let mut bx = pipeline::terrain_base::tiles_box(pipeline::terrain_deep::PACK_Z, &ts);
            if let Some(b) = pipeline::terrain_lakes::sample_box(&shore) {
                bx = [bx[0].min(b[0]), bx[1].min(b[1]), bx[2].max(b[2]), bx[3].max(b[3])];
            }
            let pool = pipeline::terrain_base::Pool::read(&nas, &m, bx)?;
            let relief = pool.relief();
            let none = |_: u32, _: u32| false;
            let fill = pipeline::terrain_pack::fill_of(&cov, &sets, &none);
            let native = |z: u8, x: u32, y: u32| pipeline::terrain_pack::native_of(&fill, z, x, y);
            let s = pipeline::terrain_pack::Sources { water: Some(&water), native: Some(&native), sets: &sets };
            let reaching = |z: u8, x: u32, y: u32| pipeline::terrain_zoom::reaching(&sets, z, x, y);
            let (mut before, mut after): (Got, Got) = Default::default();
            pool.with_tiles(&rt, s.water, |tiles| {
                relief.with_tiles(&rt, s.water, |rel| -> anyhow::Result<()> {
                    let t0 = std::time::Instant::now();
                    let src = pipeline::terrain_pack::Sources { water: Some(&water), native: None, sets: &sets };
                    let decided = pipeline::terrain_lakes::decide(tiles, &src, &across, &shore, &|d, n| {
                        eprintln!("sampled {d} of {n} shore tiles, {} MB", mb())
                    })?;
                    let given = pipeline::terrain_lakes::by_target(&across, &decided);
                    eprintln!("levels decided in {:.0} s, {} MB", t0.elapsed().as_secs_f64(), mb());
                    for (k, (t, under)) in want.iter().enumerate() {
                        let t0 = std::time::Instant::now();
                        let runs =
                            [(&mut before, BTreeMap::new()), (&mut after, given.get(t).cloned().unwrap_or_default())];
                        for (got, lakes) in runs {
                            // (A fresh rule's reader a run, as a job's run has.)
                            let fills = pipeline::terrain_pack::Fills::new(tiles, rel, &fill, &reaching);
                            let (_, mid) =
                                pipeline::terrain_pack::make_piece_logged(&fills, &s, cut(&lv[t], under), None, lakes)?;
                            for &n in &across.of[t] {
                                let id = across.lakes[n].0;
                                got.entry(id).or_default().insert(*t, mid.levels.get(&id).copied().unwrap_or(f32::NAN));
                            }
                        }
                        eprintln!(
                            "{}/{} 10/{}/{}: {} tiles under, {:.0} s, {} MB",
                            k + 1,
                            want.len(),
                            t.0,
                            t.1,
                            under.len(),
                            t0.elapsed().as_secs_f64(),
                            mb()
                        );
                    }
                    Ok(())
                })
            })???;
            println!("## before (each target its own level, #929)");
            report(&before);
            println!("## after (one level a lake, terrain-lakes)");
            report(&after);
        }
        other => anyhow::bail!("mode {other}?"),
    }
    Ok(())
}

/// Each lake's level by target.
type Got = BTreeMap<u64, BTreeMap<(u32, u32), f32>>;

/// Each lake's levels by target, and how often and how far they're apart.
fn report(got: &Got) {
    let mut spreads: Vec<f32> = Vec::new();
    for (id, ts) in got {
        let v: Vec<f32> = ts.values().copied().collect();
        let spread = v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min);
        spreads.push(spread);
        let lv: Vec<String> = ts.iter().map(|((x, y), l)| format!("10/{x}/{y} {l:.2}")).collect();
        println!("lake {id}: apart {spread:.2} m: {}", lv.join(", "));
    }
    spreads.sort_by(f32::total_cmp);
    let n = spreads.len();
    let over = |m: f32| spreads.iter().filter(|&&s| !(s <= m)).count();
    println!(
        "{n} lakes: apart over 1 cm {}, over 10 cm {}, over 50 cm {}, over 1 m {}; median {:.2} m, 90th {:.2} m, most {:.2} m",
        over(0.01),
        over(0.1),
        over(0.5),
        over(1.0),
        spreads.get(n / 2).copied().unwrap_or(0.0),
        spreads.get(n * 9 / 10).copied().unwrap_or(0.0),
        spreads.last().copied().unwrap_or(0.0)
    );
}
