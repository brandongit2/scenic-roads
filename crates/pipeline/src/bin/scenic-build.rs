//! The build steps of the new pipeline (docs/plan.md §6), writing to the NAS project folder.
//!
//! usage: scenic-build <step> --root <nas project folder> --scratch <local dir> [options]
//!
//!   convert-legacy <build_dir> [--only 6/x/y,…] [--skip-layers]
//!                                today's data/build → base packs per unit (z6), road values by the
//!                                one chaining, the road → units index, today's tile archives as
//!                                packs, the legacy global files, the legacy basemap
//!   pack [--cache dir] [T …]     pack(T) for z6 tiles T (default: every tile with ways): road and
//!                                rail hi packs (z9–14) and hidata
//!   lo [--cache dir] [Q …]       lo packs (z4–8 road and rail tiles) for z3 tiles Q (default: all)
//!   osm-pass --planet <p> --date <d>  the OSM pass (pieces, sets, basemap, road values); resumable
//!   unit [U …] [--pass d] [--layers-root r] [--regions dir] [--dem dir] [--cache-dir dir] [--buildings dir]
//!                                base(U) from the pass's pieces (today's steps on a unit folder):
//!                                default every unit whose piece meets the coverage
//!   roadunits                    the road → units index from every unit's road values
//!   terrain [T …] [--regions dir] [--pass d] [--raw dir]  terrain packs for z6 tiles T near the
//!                                coverage (default: all of them): hi z9–12, then their z3 lo packs,
//!                                from AWS's raw tiles (cached in --raw)
//!   slope [T …] [--regions dir]  slope packs (z3–11) of z6 tiles T from the terrain packs
//!                                (default: every z6 tile near the coverage)
//!   terrain-root, slope-root     their z0–2 root packs, from the lo packs' z3 tiles
//!   labels [--pass d] [--dem dir]  the labels by importance, worldwide, from the pass's labels set
//!                                (dem/labels.py), as the labels layer's packs
//!   convert-legacy-marks         today's stops & sights (global/legacy) as markdata per z6 tile
//!                                and thinned tiles per kind (docs/phase5.md)
//!   convert-legacy-overlays      today's area overlays as vector tiles (ov-*), their details and
//!                                the parks' as ovdata per z3 tile (docs/phase5.md)
//!   stations --pass d [--geojson f]  the rail stops of the pass's rail set within the coverage,
//!                                as the stations' tiles
//!   ferries --pass d [--dem dir]  the pass's ferries set through ferries.py (with
//!                                inputs/ferries/freq), as the ferries' blocks
//!   put <logical> <ext> <file>   upload a file under a logical name
//!   verify                       check every unverified upload on the NAS (SHA-256 over SSH)
//!   catalog                      publish a catalog of the build manifest
//!
//! `--cache` is where base packs are kept locally (copied from the NAS when missing).

use anyhow::{bail, Context, Result};
use pipeline::basepack::BasePack;
use pipeline::hipack::{self, b, grow, meets, tile_bounds};
use pipeline::layers::{self, LayerOut};
use pipeline::legacy::{self, Legacy, Unit};
use pipeline::out::Out;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SSH: [&str; 8] = ["ssh", "-o", "ControlMaster=auto", "-o", "ControlPath=~/.ssh/cm-%r@%h:%p", "-o", "ControlPersist=12h", "brandontsang@fishandchips.local"];
const NAS_ROOT: &str = "/volume1/personal/projects/scenic-roads";

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

/// Positional arguments after the step (not options or their values).
fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 2;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
        } else {
            out.push(args[i].clone());
            i += 1;
        }
    }
    out
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let step = args.get(1).cloned().unwrap_or_default();
    let root = PathBuf::from(opt(&args, "--root").context("--root <nas project folder>")?);
    let scratch = PathBuf::from(opt(&args, "--scratch").unwrap_or_else(|| "/tmp/scenic-build".into()));
    let mut out = Out::open(&root, &scratch)?;
    let t0 = std::time::Instant::now();
    match step.as_str() {
        "convert-legacy" => {
            let only: Vec<Unit> = opt(&args, "--only").map(|s| s.split(',').filter_map(Unit::parse).collect()).unwrap_or_default();
            convert_legacy(&mut out, Path::new(positional(&args).first().context("build dir")?), args.iter().any(|a| a == "--skip-layers"), &only)?
        }
        "pack" => {
            let cache = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| "/tmp/scenic-cache".into()));
            let mirror = opt(&args, "--mirror").map(PathBuf::from);
            pack(&mut out, &cache, mirror.as_deref(), &positional(&args))?
        }
        "lo" => {
            let cache = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| "/tmp/scenic-cache".into()));
            let mirror = opt(&args, "--mirror").map(PathBuf::from);
            lo(&mut out, &cache, mirror.as_deref(), &positional(&args))?
        }
        "osm-pass" => {
            let planet = PathBuf::from(opt(&args, "--planet").context("--planet <path>")?);
            let date = opt(&args, "--date").context("--date YYYY-MM-DD")?;
            let extract = PathBuf::from(opt(&args, "--extract").unwrap_or_else(|| "target/release/extract".into()));
            let planetiler = PathBuf::from(opt(&args, "--planetiler").unwrap_or_else(|| "tools/planetiler.jar".into()));
            pipeline::osmpass::check_tools(&extract, &planetiler)?;
            // Room first: caches the pass makes worthless or can refill (the pack cache).
            for dir in args.windows(2).filter(|w| w[0] == "--clear").map(|w| PathBuf::from(&w[1])) {
                if dir.exists() {
                    eprintln!("osm-pass: clearing {}", dir.display());
                    std::fs::remove_dir_all(&dir)?;
                }
            }
            pipeline::osmpass::run_pass(&mut out, &planet, &date, &scratch, &extract, &planetiler)?
        }
        "verify" => {
            let n = out.verify(&SSH, NAS_ROOT)?;
            eprintln!("verified {n} uploads");
        }
        "catalog" => catalog(&mut out)?,
        "unit" => unit_step(&mut out, &args, &scratch)?,
        "roadunits" => roadunits(&mut out)?,
        "terrain" => terrain_step(&mut out, &args)?,
        "slope" => slope_step(&mut out, &args)?,
        "labels" => labels_step(&mut out, &args, &scratch)?,
        "convert-legacy-marks" => {
            let c = pipeline::markconv::convert(&mut out)?;
            eprintln!("marks: {} points, {} markdata tiles, {} thinned tiles", c.points, c.tiles, c.thinned);
        }
        "stations" => {
            // stations --pass <date> [--geojson file]: the pass's rail set's stops as the stations' tiles.
            let date = opt(&args, "--pass").context("--pass <date>")?;
            let gj = opt(&args, "--geojson").map(PathBuf::from);
            let (n, tiles) = pipeline::ovconv::stations_job(&mut out, &date, gj.as_deref())?;
            eprintln!("stations: {n} stops in {tiles} tiles");
        }
        "ferries" => {
            // ferries --pass <date> [--dem dir]: the pass's ferries set through ferries.py, as blocks.
            let date = opt(&args, "--pass").context("--pass <date>")?;
            let dem = PathBuf::from(opt(&args, "--dem").unwrap_or_else(|| "dem".into()));
            let n = pipeline::ovconv::ferries_job(&mut out, &date, &dem)?;
            eprintln!("ferries: {n} blocks");
        }
        "convert-legacy-overlays" => {
            let c = pipeline::ovconv::convert(&mut out)?;
            eprintln!("overlays: {} areas, {} stations and {} ferry blocks in {} tiles, {} ovdata, {} parks", c.areas, c.stations, c.ferries, c.tiles, c.ovdata, c.parks);
        }
        "terrain-root" => {
            let raw_dir = PathBuf::from(opt(&args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
            let n = pipeline::terrain_pack::build_root(&mut out, &pipeline::terrain_pack::RawTiles::new(&raw_dir))?;
            eprintln!("terrain root: {n} tiles");
        }
        "slope-root" => {
            let n = pipeline::slope_pack::build_root(&mut out)?;
            eprintln!("slope root: {n} tiles");
        }
        "put" => {
            // put <logical> <ext> <file>: upload a file under a logical name (manual operations).
            let p = positional(&args);
            let (Some(l), Some(ext), Some(f)) = (p.first(), p.get(1), p.get(2)) else { bail!("put <logical> <ext> <file>") };
            let tmp = scratch.join(format!("put-{}", std::path::Path::new(f).file_name().unwrap().to_string_lossy()));
            std::fs::create_dir_all(&scratch)?;
            std::fs::copy(f, &tmp)?;
            let name = out.put_file(l, ext, &tmp)?;
            eprintln!("{l} -> {name}");
        }
        s => bail!("unknown step {s:?} (convert-legacy, pack, lo, osm-pass, verify, catalog)"),
    }
    out.save()?;
    eprintln!("{step}: done in {:.0?}", t0.elapsed());
    Ok(())
}

// ---- convert-legacy ------------------------------------------------------------------------

/// A unit's road values file: the values per way, and the ways by road (sorted (road, way index)
/// pairs, for the server's whole-road lookups).
fn put_roads(out: &mut Out, u: Unit, recs: &[pipeline::legacy::RoadRec]) -> Result<String> {
    let mut byroad: Vec<[u64; 2]> = recs.iter().enumerate().map(|(i, r)| [r.road, i as u64]).collect();
    byroad.sort_unstable();
    put_sect(out, &format!("global/roads/{}", u.dash()), serde_json::json!({"fmt": 1, "unit": u.slash()}), &[("roads", b(recs)), ("byroad", b(&byroad))])
}

fn put_sect(out: &mut Out, logical: &str, meta: serde_json::Value, sections: &[(&str, &[u8])]) -> Result<String> {
    let local = out.scratch_file(&format!("{logical}.sect"));
    let mut w = store::sect::SectWriter::create(&local, meta)?;
    for (n, bytes) in sections {
        w.add(n, bytes)?;
    }
    w.finish()?;
    out.put_file(logical, "sect", &local)
}

fn convert_legacy(out: &mut Out, dir: &Path, skip_layers: bool, only: &[Unit]) -> Result<()> {
    let lg = Legacy::open(dir)?;
    let built = format!("legacy:{}", std::fs::metadata(dir.join("ways.bin"))?.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs());
    eprintln!("{} ways, {} vertices, {} samples", lg.ways.ways().len(), lg.ways.verts().len(), lg.samples.get().len());
    let t = std::time::Instant::now();
    let vals = lg.road_values();
    let nroads = vals.iter().map(|v| v.road).collect::<BTreeSet<_>>().len();
    eprintln!("road values: {nroads} roads ({:.0?})", t.elapsed());
    let mut units = lg.units();
    if !only.is_empty() {
        units.retain(|u, _| only.contains(u));
    }
    eprintln!("{} units", units.len());
    // Base packs and road values per unit; the road → units index.
    let mut road_units: Vec<(u64, u64)> = Vec::new();
    for (k, (u, idx)) in units.iter().enumerate() {
        let bs = legacy::base_sections(&lg, *u, idx, &built);
        let secs: Vec<(&str, &[u8])> = bs.sections.iter().map(|(n, v)| (*n, v.as_slice())).collect();
        put_sect(out, &format!("base/{}", u.dash()), bs.meta, &secs)?;
        let recs = legacy::road_records(&vals, idx);
        road_units.extend(recs.iter().map(|r| (r.road, u.key())));
        put_roads(out, *u, &recs)?;
        if k % 10 == 0 {
            out.save()?;
            eprintln!("base packs: {}/{} ({:.0?})", k + 1, units.len(), t.elapsed());
        }
    }
    road_units.sort_unstable();
    road_units.dedup();
    let flat: Vec<u64> = road_units.iter().flat_map(|&(r, u)| [r, u]).collect();
    put_sect(out, "global/roadunits", serde_json::json!({"fmt": 1, "pairs": road_units.len()}), &[("pairs", b(&flat))])?;
    out.save()?;
    // Rail service keyed by OSM way id: (u32 way id, f32 trains a day), sorted.
    if let Ok(bytes) = std::fs::read(dir.join("rail-freq.bin")) {
        let ways = lg.ways.ways();
        let mut recs: Vec<(u32, f32)> = bytes
            .chunks_exact(8)
            .filter_map(|c| {
                let i = u32::from_le_bytes(c[..4].try_into().unwrap()) as usize;
                let f = f32::from_le_bytes(c[4..].try_into().unwrap());
                ways.get(i).map(|w| (w.id as u32, f))
            })
            .collect();
        recs.sort_unstable_by_key(|r| r.0);
        let mut buf = Vec::with_capacity(recs.len() * 8);
        for (w, f) in recs {
            buf.extend_from_slice(&w.to_le_bytes());
            buf.extend_from_slice(&f.to_le_bytes());
        }
        out.put_bytes("global/railfreq", "bin", &buf)?;
    }
    // Today's small files, as they are.
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let n = e.file_name().to_string_lossy().into_owned();
        let keep = (n.ends_with(".json") || n.ends_with(".jsonl")) && !n.starts_with('.') && n != "names-en.json";
        if keep {
            let (stem, ext) = n.rsplit_once('.').unwrap();
            out.put_file(&format!("global/legacy/{}", stem.replace('.', "_")), ext, &copy_to_scratch(out, &e.path())?)?;
        }
    }
    out.save()?;
    if skip_layers {
        return Ok(());
    }
    // Tile layers.
    let mut all: BTreeMap<String, LayerOut> = BTreeMap::new();
    for (file, layer, enc, gzip, maxz) in [
        ("terrain.tiles", "terrain", "terrarium-png", false, 12u8),
        ("slope.tiles", "slope", "slope4-png", false, 11),
        ("trees-cover.tiles", "trees-cover", "terrarium-webp", false, 14),
        ("trees-height.tiles", "trees-height", "terrarium-webp", false, 14),
        ("trees-leaf.tiles", "trees-leaf", "terrarium-webp", false, 14),
        ("labels.tiles", "labels", "mvt", true, 14),
    ] {
        let p = dir.join(file);
        if !p.exists() {
            continue;
        }
        let arc = roadcore::archive::Archive::open(&p)?;
        let lo = layers::split_archive(out, &arc, layer, enc, gzip, maxz)?;
        eprintln!("{layer}: root {:?}, {} lo, {} hi packs ({:.0?})", lo.root.is_some(), lo.lo.len(), lo.hi.len(), t.elapsed());
        all.insert(layer.to_string(), lo);
    }
    for var in ["canopy", "class", "cover", "areas"] {
        if dir.join(format!("grid.{var}.u8")).exists() {
            all.insert(format!("grid-{var}"), layers::split_grid(out, dir, var)?);
        }
    }
    out.put_bytes("global/legacy/layers", "json", &serde_json::to_vec_pretty(&all)?)?;
    // The basemap and its parts, as they are (PMTiles).
    out.put_file("layers/basemap/legacy-base", "pmtiles", &copy_to_scratch(out, &dir.join("base.pmtiles"))?)?;
    if let Ok(rd) = std::fs::read_dir(dir.join("base-parts")) {
        let mut parts: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "pmtiles")).collect();
        parts.sort();
        for p in parts {
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            out.put_file(&format!("layers/basemap/legacy-{stem}"), "pmtiles", &copy_to_scratch(out, &p)?)?;
        }
    }
    out.save()?;
    Ok(())
}

/// Copy a file into scratch (`put_file` consumes its input).
fn copy_to_scratch(out: &Out, p: &Path) -> Result<PathBuf> {
    let dest = out.scratch_file(&format!("copy-{}", p.file_name().unwrap().to_string_lossy()));
    std::fs::copy(p, &dest).with_context(|| format!("copy {}", p.display()))?;
    Ok(dest)
}

// ---- base packs, locally ------------------------------------------------------------------

/// Every unit's base pack and road values, copied into `cache` when missing, opened.
/// Every unit's base pack and road values, opened from local copies: this Mac's mirror's when it
/// has them (`mirror`, the server's: same content names, so no second copy), else the cache's,
/// copied from the NAS when missing. Cached files no unit uses any more go first, so the cache
/// holds at most one copy of what the mirror lacks.
fn open_units(out: &Out, cache: &Path, mirror: Option<&Path>) -> Result<Vec<BasePack>> {
    std::fs::create_dir_all(cache)?;
    let mut units: Vec<(String, String, String)> = Vec::new();
    for (k, v) in &out.manifest {
        if let Some(u) = k.strip_prefix("base/") {
            let roads = out.get(&format!("global/roads/{u}")).with_context(|| format!("no road values for {u}"))?;
            units.push((u.to_string(), v.clone(), roads.to_string()));
        }
    }
    let needed: BTreeSet<&str> = units.iter().flat_map(|(_, b, r)| [b.as_str(), r.as_str()]).collect();
    prune_cache(cache, &needed)?;
    let mut packs = Vec::with_capacity(units.len());
    for (u, base, roads) in &units {
        let mut local = Vec::new();
        for name in [base, roads] {
            // (The mirror renames a file into place only once it's copied and checked.)
            if let Some(m) = mirror.map(|m| m.join(name)).filter(|m| m.is_file()) {
                local.push(m);
                continue;
            }
            let p = cache.join(name);
            if !p.exists() {
                std::fs::create_dir_all(p.parent().unwrap())?;
                let tmp = p.with_extension("tmp");
                std::fs::copy(out.path(name), &tmp).with_context(|| format!("copy {name} from the NAS"))?;
                anyhow::ensure!(store::naming::hash16_file(&tmp)? == name.rsplit('.').nth(1).unwrap_or(""), "{name}: hash mismatch after copy");
                std::fs::rename(&tmp, &p)?;
            }
            local.push(p);
        }
        packs.push(BasePack::open(&local[0], &local[1]).with_context(|| format!("unit {u}"))?);
    }
    Ok(packs)
}

/// Removes from `cache` every file not in `keep` (content names relative to it), and leftovers of
/// interrupted copies.
fn prune_cache(cache: &Path, keep: &BTreeSet<&str>) -> Result<()> {
    let mut stack = vec![cache.to_path_buf()];
    let mut freed = 0u64;
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir)?.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(cache).unwrap_or(&p).to_string_lossy().into_owned();
            if !keep.contains(rel.as_str()) {
                freed += e.metadata().map(|m| m.len()).unwrap_or(0);
                std::fs::remove_file(&p).ok();
            }
        }
    }
    if freed > 0 {
        eprintln!("cache: {} MB of replaced base packs removed", freed >> 20);
    }
    Ok(())
}

/// The z-level tiles any way of `packs` touches.
fn tiles_touched(packs: &[BasePack], z: u8) -> Result<BTreeSet<Unit>> {
    let mut set = BTreeSet::new();
    for bp in packs {
        for bb in bp.bboxes()? {
            let a = Unit::of_point(z, [bb[0], bb[3]]);
            let c = Unit::of_point(z, [bb[2], bb[1]]);
            for x in a.x..=c.x {
                for y in a.y..=c.y {
                    set.insert(Unit { z, x, y });
                }
            }
        }
    }
    Ok(set)
}

fn parse_tiles(list: &[String], z: u8) -> Result<Vec<Unit>> {
    list.iter().map(|s| Unit::parse(s).filter(|u| u.z == z).with_context(|| format!("not a z{z} tile: {s}"))).collect()
}

// ---- pack(T) --------------------------------------------------------------------------------

fn pack(out: &mut Out, cache: &Path, mirror: Option<&Path>, only: &[String]) -> Result<()> {
    let packs = open_units(out, cache, mirror)?;
    let refs: Vec<&BasePack> = packs.iter().collect();
    let ts: Vec<Unit> = if only.is_empty() { tiles_touched(&packs, 6)?.into_iter().collect() } else { parse_tiles(only, 6)? };
    eprintln!("pack: {} tiles from {} units", ts.len(), packs.len());
    let t0 = std::time::Instant::now();
    for (k, t) in ts.iter().enumerate() {
        let tb = tile_bounds(t.z, t.x, t.y);
        let halo_b = grow(tb, 100.0);
        let near: Vec<&BasePack> = refs.iter().copied().filter(|bp| meets(bp.extent, halo_b)).collect();
        let halo = hipack::ways_in(&near, halo_b)?;
        let in_t: Vec<hipack::Staged> = halo.iter().copied().filter(|s| meets(s.bbox, tb)).collect();
        if in_t.is_empty() {
            continue;
        }
        let win = hipack::way_inputs(&near, &in_t)?;
        let (roads, rails) = hipack::tiles(&win, 6, t.x, t.y, 9..=14, false);
        for (layer, enc) in [("roads", &roads), ("rails", &rails)] {
            let mut it = enc.iter().map(|e| {
                let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                (z, x, y, e.gz.clone(), e.raw_len as u32)
            });
            layers::write_pack(out, layer, "rt7", true, "hi", (6, t.x, t.y), &mut it)?;
        }
        let hd = hipack::hidata(*t, &near, &in_t, &halo)?;
        // The zoomed-out summaries (docs/phase5.md), from the same parts.
        let ls = roadcore::lsum::build(&hd.parts, &hd.psamples, &hd.pch, &hd.here, &hd.railinfo);
        put_sect(
            out,
            &format!("hidata/{}", t.dash()),
            serde_json::json!({"fmt": 1, "tile": t.slash(), "here": hd.here.len(), "parts": hd.parts.len(), "climbs": hd.climbs.len(), "lsum": roadcore::lsum::LSUM_V}),
            &[
                ("here", b(&hd.here)),
                ("ends", b(&hd.ends)),
                ("parts", b(&hd.parts)),
                ("psamples", b(&hd.psamples)),
                ("pch", b(&hd.pch)),
                ("climbs", b(&hd.climbs)),
                ("climbgeom", b(&hd.climbgeom)),
                ("railinfo", b(&hd.railinfo)),
                ("railstr", &hd.railstr),
                ("lparts", b(&ls.lparts)),
                ("lbins", b(&ls.lbins)),
                ("lrparts", b(&ls.lrparts)),
                ("lrbins", b(&ls.lrbins)),
            ],
        )?;
        out.save()?;
        eprintln!("pack {} ({}/{}): {} ways, {} road tiles, {} parts, {} climbs ({:.0?})", t.slash(), k + 1, ts.len(), in_t.len(), roads.len(), hd.parts.len(), hd.climbs.len(), t0.elapsed());
    }
    Ok(())
}

// ---- lo packs -------------------------------------------------------------------------------

fn lo(out: &mut Out, cache: &Path, mirror: Option<&Path>, only: &[String]) -> Result<()> {
    let packs = open_units(out, cache, mirror)?;
    let refs: Vec<&BasePack> = packs.iter().collect();
    let qs: Vec<Unit> = if only.is_empty() { tiles_touched(&packs, 3)?.into_iter().collect() } else { parse_tiles(only, 3)? };
    eprintln!("lo: {} tiles from {} units", qs.len(), packs.len());
    let t0 = std::time::Instant::now();
    for (k, q) in qs.iter().enumerate() {
        let qb = tile_bounds(q.z, q.x, q.y);
        let near: Vec<&BasePack> = refs.iter().copied().filter(|bp| meets(bp.extent, qb)).collect();
        let staged = hipack::ways_in(&near, qb)?;
        if staged.is_empty() {
            continue;
        }
        let win = hipack::way_inputs(&near, &staged)?;
        let (roads, rails) = hipack::tiles(&win, 3, q.x, q.y, 4..=8, true);
        for (layer, enc) in [("roads", &roads), ("rails", &rails)] {
            let mut it = enc.iter().map(|e| {
                let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                (z, x, y, e.gz.clone(), e.raw_len as u32)
            });
            layers::write_pack(out, layer, "rt7", true, "lo", (3, q.x, q.y), &mut it)?;
        }
        out.save()?;
        eprintln!("lo {} ({}/{}): {} ways, {} road tiles ({:.0?})", q.slash(), k + 1, qs.len(), staged.len(), roads.len(), t0.elapsed());
    }
    Ok(())
}

// ---- catalog --------------------------------------------------------------------------------

/// The map's meta, added up from the units' summaries: each base pack's own, else worked out from
/// its sections once (today's converted packs) and kept in `state/build/summaries.json` by content
/// name (a content name never changes).
fn units_meta(out: &Out, base: &BTreeMap<String, String>) -> Result<serde_json::Value> {
    use pipeline::summary::Summary;
    let path = out.root().join("state/build/summaries.json");
    let mut known: BTreeMap<String, Summary> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut total = Summary::default();
    let mut used: std::collections::BTreeSet<String> = Default::default();
    let mut made = 0;
    for logical in base.values() {
        let content = out.get(logical).with_context(|| format!("no file for {logical}"))?.to_string();
        if !known.contains_key(&content) {
            let r = store::sect::SectReader::open(store::range::PlainFile::open(&out.path(&content))?)?;
            let s = match r.meta().get("summary").and_then(|v| serde_json::from_value::<Summary>(v.clone()).ok()) {
                Some(s) => s,
                None => Summary::of(&r.read_pod::<roadcore::WayRec>("ways")?, &r.read_pod::<[i32; 2]>("verts")?, &r.read_pod::<i16>("elev")?),
            };
            known.insert(content.clone(), s);
            made += 1;
        }
        total.add(&known[&content]);
        used.insert(content);
    }
    if made > 0 || known.len() != used.len() {
        known.retain(|c, _| used.contains(c));
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&known)?)?;
        std::fs::rename(&tmp, &path)?;
        eprintln!("catalog: {made} unit summaries made from their packs");
    }
    let mut m = total.meta();
    m["built"] = serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs());
    Ok(m)
}

/// A layer's zoom range as its builders make it (and today's converted data has it).
fn layer_zooms(layer: &str) -> Option<(u8, u8)> {
    Some(match layer {
        "roads" | "rails" => (4, 14),
        "terrain" | "labels" => (0, 12),
        // Stored to z11; the server makes z12 on demand.
        "slope" => (0, 11),
        l if l.starts_with("trees-") => (4, 12),
        // Analysis grids, not served.
        l if l.starts_with("grid-") => (11, 11),
        // Thinned tiles; z6 blocks come from markdata.
        l if l.starts_with("marks-") => (0, 5),
        l if l.starts_with("ov-") || l == "stations" => (0, pipeline::ovconv::MAXZ),
        // Blocks at zooms 0, 3 and 6 (the app picks one by the view's zoom).
        "ferries" => (0, 6),
        _ => return None,
    })
}

fn catalog(out: &mut Out) -> Result<()> {
    let mut layers: BTreeMap<String, LayerOut> = BTreeMap::new();
    let (mut base, mut roads, mut hidata, mut global, mut basemap) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), Vec::new());
    let mut markdata = BTreeMap::new();
    let mut ovdata = BTreeMap::new();
    // The basemap: the newest pass's worldwide archive when there is one, else today's (legacy)
    // basemap and its parts; never both (the server merges every archive listed).
    let world = out.manifest.keys().filter(|k| k.starts_with("layers/basemap/world-")).max().cloned();
    for logical in out.manifest.keys() {
        let parts: Vec<&str> = logical.split('/').collect();
        match parts.as_slice() {
            ["layers", "basemap", name] => {
                if world.as_deref() == Some(logical.as_str()) || (world.is_none() && name.starts_with("legacy-")) {
                    basemap.push(logical.clone());
                }
            }
            ["layers", layer, scope, key] => {
                let u = Unit::parse(key).context("pack key")?;
                let enc = match *layer {
                    "roads" | "rails" => "rt7",
                    "terrain" => "terrarium-png",
                    "slope" => "slope4-png",
                    "labels" => "mvt",
                    l if l.starts_with("trees-") => "terrarium-webp",
                    l if l.starts_with("grid-") => "u8-zstd",
                    l if l.starts_with("marks-") => "rdmt",
                    l if l.starts_with("ov-") || l == "stations" => "mvt",
                    "ferries" => "geojson-gz",
                    _ => "unknown",
                };
                let zs = match *scope {
                    "root" => (0, 2),
                    "lo" => (3, 8),
                    _ => (9, 14),
                };
                layers.entry(layer.to_string()).or_insert_with(|| LayerOut::new(enc)).add(scope, (u.z, u.x, u.y), logical.clone(), zs);
            }
            ["base", u] => {
                base.insert(Unit::parse(u).context("unit")?.slash(), logical.clone());
            }
            ["global", "roads", u] => {
                roads.insert(Unit::parse(u).context("unit")?.slash(), logical.clone());
            }
            ["hidata", t] => {
                hidata.insert(Unit::parse(t).context("tile")?.slash(), logical.clone());
            }
            ["markdata", t] => {
                markdata.insert(Unit::parse(t).context("tile")?.slash(), logical.clone());
            }
            ["ovdata", t] => {
                ovdata.insert(Unit::parse(t).context("tile")?.slash(), logical.clone());
            }
            ["global", ..] => {
                global.insert(logical.trim_start_matches("global/").to_string(), logical.clone());
            }
            _ => {}
        }
    }
    // Zoom ranges as each layer is defined (whichever packs exist yet), else the packs' scopes.
    for (l, x) in layers.iter_mut() {
        if let Some(zs) = layer_zooms(l) {
            (x.minzoom, x.maxzoom) = zs;
        }
    }
    // The latest pass's outlines, for the Regions panel and the modules.
    // (Exactly sources/osm/<date>/outlines: the pass's outline set, sets/outlines, is its input.)
    if let Some(l) = out.manifest.keys().filter(|k| matches!(k.split('/').collect::<Vec<_>>()[..], ["sources", "osm", _, "outlines"])).max() {
        global.insert("outlines".to_string(), l.clone());
    }
    let meta = units_meta(out, &base)?;
    let units: Vec<String> = base.keys().cloned().collect();
    // Only what the map reads: build sources (the planet's pieces, sets and road values) stay out,
    // or every Mac's mirror would copy them. A file missing on the NAS stops the publish.
    let served = |l: &str| {
        (l.starts_with("layers/") && (!l.starts_with("layers/basemap/") || basemap.iter().any(|b| b == l)))
            || l.starts_with("base/")
            || l.starts_with("hidata/")
            || l.starts_with("markdata/")
            || l.starts_with("ovdata/")
            || l.starts_with("global/")
            || global.values().any(|g| g == l)
    };
    let mut files: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for (l, n) in out.manifest.iter().filter(|(l, _)| served(l)) {
        let size = std::fs::metadata(out.path(n)).with_context(|| format!("{l}: {n} is missing on the NAS"))?.len();
        files.insert(l.clone(), serde_json::json!({"file": n, "size": size, "fmt": 1}));
    }
    let dir = out.root().join("catalog");
    std::fs::create_dir_all(&dir)?;
    let n = store::catalog::next_n(&dir)?;
    let cat = serde_json::json!({
        "fmt": 1,
        "n": n,
        "created": chrono_now(),
        "app": env!("CARGO_PKG_VERSION"),
        "files": files,
        "units": units,
        "layers": layers,
        "basemap": basemap,
        "base": base,
        "roads": roads,
        "hidata": hidata,
        "markdata": markdata,
        "ovdata": ovdata,
        "global": global,
        "meta": meta,
        "credits": [],
        "coverage": {"regions": []},
    });
    let catalog: store::catalog::Catalog = serde_json::from_value(cat)?;
    let path = store::catalog::write(&dir, &catalog)?;
    eprintln!("published {}", path.display());
    Ok(())
}

/// UTC now as RFC 3339 (no chrono dependency).
fn chrono_now() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

// ---- base(U) from the pass ------------------------------------------------------------------

/// The pass's road values of a unit: (OSM way id, record), sorted by id.
fn pass_roads(out: &Out, date: &str, u: Unit) -> Result<Vec<(u64, pipeline::legacy::RoadRec)>> {
    let Some(name) = out.get(&format!("sources/osm/{date}/roads/{}", u.dash())) else { return Ok(Vec::new()) };
    let b = std::fs::read(out.path(name))?;
    Ok(b.chunks_exact(32).map(|c| (u64::from_le_bytes(c[..8].try_into().unwrap()), bytemuck::pod_read_unaligned(&c[8..32]))).collect())
}

fn unit_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::coverage::Coverage;
    use pipeline::unit::{build_folder, owns, Tools};
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass on the NAS (--pass)")?,
    };
    let regions = opt(args, "--regions").map(PathBuf::from).unwrap_or_else(|| out.root().join("inputs/regions"));
    let (recipes, bad) = pipeline::agent::recipes::load(&regions);
    for (f, e) in &bad {
        eprintln!("unit: skipping region {f}: {e}");
    }
    anyhow::ensure!(!recipes.is_empty(), "no regions in {}", regions.display());
    let outlines_file = out.get(&format!("sources/osm/{date}/outlines")).map(|n| out.path(n));
    let outlines = outlines_file.as_deref().map(pipeline::outlines::Outlines::open).transpose()?;
    let cov = Coverage::from_recipes(&recipes, outlines.as_ref(), &out.root().join("inputs/outlines"))?;
    // Global-source layers: this root's catalog, or another's (a pilot builds against the real one).
    let layers_root = opt(args, "--layers-root").map(PathBuf::from).unwrap_or_else(|| out.root().to_path_buf());
    let cat = store::catalog::latest(&layers_root.join("catalog"))?.context("no catalog for the global-source layers")?;
    let bin = std::env::current_exe()?.parent().context("bin")?.to_path_buf();
    let tools = Tools {
        bin,
        dem: PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into())),
        cache: PathBuf::from(opt(args, "--cache-dir").unwrap_or_else(|| "data/cache".into())),
        buildings: opt(args, "--buildings").map(PathBuf::from),
        spacing_m: 8,
    };
    // Today's heritage sites, for every unit's flags.
    let heritage = pipeline::stage::Heritage::load(&layers_root, &cat, &tools.cache)?;
    match &heritage {
        Some(h) => eprintln!("unit: {} heritage sites", h.len()),
        None => eprintln!("unit: no heritage sites in the catalog"),
    }
    // The units: as asked, else every unit whose piece meets the coverage.
    let pieces: serde_json::Value = serde_json::from_slice(&std::fs::read(out.path(out.get(&format!("sources/osm/{date}/pieces")).context("the pass's pieces list")?))?)?;
    let mut units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    if units.is_empty() {
        for k in pieces["pieces"].as_object().context("pieces")?.keys() {
            let u = Unit::parse(k).context("unit")?;
            if cov.meets_box(tile_bounds(u.z, u.x, u.y)) {
                units.push(u);
            }
        }
    }
    eprintln!("unit: pass {date}, {} region(s), {} unit(s)", recipes.len(), units.len());
    for u in units {
        let t = std::time::Instant::now();
        let dir = scratch.join("units").join(u.dash());
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        let piece_logical = format!("sources/osm/{date}/pieces/{}", u.dash());
        let Some(piece) = out.get(&piece_logical).map(|n| out.path(n)) else {
            eprintln!("unit {}: no piece (nothing there)", u.slash());
            continue;
        };
        let local_piece = scratch.join(format!("piece-{}.osm.pbf", u.dash()));
        std::fs::copy(&piece, &local_piece).with_context(|| format!("copy {}", piece.display()))?;
        let rep = build_folder(u, &local_piece, &dir, &cov, &layers_root, &cat, &tools, heritage.as_ref())?;
        std::fs::remove_file(&local_piece).ok();
        // Grids its packs lacked (new coverage), made in the folder: the unit's own z6 tile's go up,
        // for later units and packs. (The canopy step made canopy and cover for every tile.)
        for var in ["class", "canopy", "cover"] {
            if rep.staged.missing.get(var).copied().unwrap_or(0) == 0 {
                continue;
            }
            let mut tiles = pipeline::stage::grid_tiles_in(&dir, var, u.x, u.y)?.into_iter();
            layers::write_pack(out, &format!("grid-{var}"), "u8-zstd", false, "hi", (6, u.x, u.y), &mut tiles)?;
        }
        eprintln!("unit {}: {} of {} ways touch the coverage, {} owned", u.slash(), rep.kept_ways, rep.piece_ways, rep.owned);
        if rep.kept_ways == 0 || rep.owned == 0 {
            continue;
        }
        // The owned ways, in base-pack order, with the pass's road values.
        let lg = Legacy::open(&dir)?;
        let tb = tile_bounds(u.z, u.x, u.y);
        let idx: Vec<u32> = lg.units().remove(&u).unwrap_or_default().into_iter().filter(|&i| owns(tb, lg.first_vertex(&lg.ways.ways()[i as usize]))).collect();
        let built = format!("pass:{date}");
        let bs = legacy::base_sections(&lg, u, &idx, &built);
        let secs: Vec<(&str, &[u8])> = bs.sections.iter().map(|(n, v)| (*n, v.as_slice())).collect();
        put_sect(out, &format!("base/{}", u.dash()), bs.meta, &secs)?;
        let vals = pass_roads(out, &date, u)?;
        let ways = lg.ways.ways();
        let verts = lg.ways.verts();
        let recs: Vec<pipeline::legacy::RoadRec> = idx
            .iter()
            .map(|&i| {
                let w = &ways[i as usize];
                match vals.binary_search_by_key(&(w.id as u64), |v| v.0) {
                    Ok(k) => vals[k].1,
                    Err(_) => {
                        // Not chained (a ferry, a one-vertex way): its own road.
                        let vs = &verts[Legacy::range(w)];
                        let len: f64 = vs.windows(2).map(|p| roadcore::dist_m(p[0][0] as f64 * 1e-7, p[0][1] as f64 * 1e-7, p[1][0] as f64 * 1e-7, p[1][1] as f64 * 1e-7)).sum();
                        pipeline::legacy::RoadRec { road: w.id as u64, len: len as f32, offset: 0.0, dir: 0, _pad: [0; 7] }
                    }
                }
            })
            .collect();
        put_roads(out, u, &recs)?;
        out.save()?;
        eprintln!("unit {}: base pack of {} ways in {:.0?}", u.slash(), idx.len(), t.elapsed());
    }
    Ok(())
}

/// The road → units index from every unit's road values in the manifest.
fn roadunits(out: &mut Out) -> Result<()> {
    let mut pairs: Vec<(u64, u64)> = Vec::new();
    let units: Vec<(Unit, String)> = out.manifest.iter().filter_map(|(k, v)| k.strip_prefix("global/roads/").and_then(Unit::parse).map(|u| (u, v.clone()))).collect();
    for (u, content) in &units {
        let r = store::sect::SectReader::open(store::range::MmapFile::open(&out.path(content))?)?;
        let recs: Vec<pipeline::legacy::RoadRec> = r.read_pod("roads")?;
        pairs.extend(recs.iter().map(|x| (x.road, u.key())));
    }
    pairs.sort_unstable();
    pairs.dedup();
    let flat: Vec<u64> = pairs.iter().flat_map(|&(r, u)| [r, u]).collect();
    put_sect(out, "global/roadunits", serde_json::json!({"fmt": 1, "pairs": pairs.len()}), &[("pairs", b(&flat))])?;
    eprintln!("roadunits: {} roads×units over {} units", pairs.len(), units.len());
    Ok(())
}

/// The coverage of the regions (recipes in --regions, else inputs/regions; outlines from --pass,
/// else the latest complete pass).
fn coverage_of(out: &Out, args: &[String]) -> Result<pipeline::coverage::Coverage> {
    let regions = opt(args, "--regions").map(PathBuf::from).unwrap_or_else(|| out.root().join("inputs/regions"));
    let (recipes, bad) = pipeline::agent::recipes::load(&regions);
    for (f, e) in &bad {
        eprintln!("skipping region {f}: {e}");
    }
    anyhow::ensure!(!recipes.is_empty(), "no regions in {}", regions.display());
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root()));
    let outlines = date
        .as_deref()
        .and_then(|d| out.get(&format!("sources/osm/{d}/outlines")).map(|n| out.path(n)))
        .map(|p| pipeline::outlines::Outlines::open(&p))
        .transpose()?;
    pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &out.root().join("inputs/outlines"))
}

fn terrain_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let mut ts: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    if ts.is_empty() {
        for x in 0..64u32 {
            for y in 0..64u32 {
                if pipeline::terrain_pack::near_coverage(&cov, 6, x, y, 20.0) {
                    ts.push(Unit { z: 6, x, y });
                }
            }
        }
    }
    let mut by_q: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for t in &ts {
        by_q.entry((t.x >> 3, t.y >> 3)).or_default().push((t.x, t.y));
    }
    eprintln!("terrain: {} z6 tiles in {} z3 packs", ts.len(), by_q.len());
    // AWS's raw tiles, kept on this Mac (the build cache).
    let raw_dir = PathBuf::from(opt(args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
    let raw = pipeline::terrain_pack::RawTiles::new(&raw_dir);
    for (q, list) in by_q {
        let t = std::time::Instant::now();
        let r = pipeline::terrain_pack::build_q(out, &raw, q, &list, &cov)?;
        eprintln!("terrain 3/{}/{}: {r:?} ({:.0?})", q.0, q.1, t.elapsed());
    }
    Ok(())
}

fn slope_step(out: &mut Out, args: &[String]) -> Result<()> {
    let mut ts: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    if ts.is_empty() {
        let cov = coverage_of(out, args)?;
        for x in 0..64u32 {
            for y in 0..64u32 {
                if pipeline::terrain_pack::near_coverage(&cov, 6, x, y, 20.0) {
                    ts.push(Unit { z: 6, x, y });
                }
            }
        }
    }
    let mut by_q: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for t in &ts {
        by_q.entry((t.x >> 3, t.y >> 3)).or_default().push((t.x, t.y));
    }
    for (q, list) in by_q {
        let t = std::time::Instant::now();
        let r = pipeline::slope_pack::build_q(out, q, &list)?;
        eprintln!("slope 3/{}/{}: {r:?} ({:.0?})", q.0, q.1, t.elapsed());
    }
    Ok(())
}

/// The labels by importance, worldwide (dem/labels.py on the pass's labels set), split into the
/// labels layer's packs.
fn labels_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass")?,
    };
    let set = out.get(&format!("sources/osm/{date}/sets/labels")).context("the pass has no labels set (passes before 2026-10-02)")?.to_string();
    let work = scratch.join("labels");
    std::fs::create_dir_all(&work)?;
    let local = work.join("labels-set.osm.pbf");
    std::fs::copy(out.path(&set), &local).with_context(|| format!("copy {set}"))?;
    let tiles = work.join("labels.tiles");
    let dem = PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into()));
    let st = std::process::Command::new("uv")
        .current_dir(&dem)
        .args(["run", "python", "labels.py", "--src"])
        .arg(&local)
        .arg("--out")
        .arg(&tiles)
        .arg("--work")
        .arg(&work)
        .arg("--own-english")
        .status()
        .context("run labels.py")?;
    anyhow::ensure!(st.success(), "labels.py failed: {st}");
    let arc = roadcore::archive::Archive::open(&tiles)?;
    let lo = layers::split_archive(out, &arc, "labels", "mvt", true, 14)?;
    eprintln!("labels: root {:?}, {} lo, {} hi packs", lo.root.is_some(), lo.lo.len(), lo.hi.len());
    // Packs of an earlier labels layer that this one doesn't have go from the manifest.
    let keep: std::collections::BTreeSet<String> = lo.root.iter().chain(lo.lo.values()).chain(lo.hi.values()).cloned().collect();
    let gone: Vec<String> = out.manifest.keys().filter(|k| k.starts_with("layers/labels/") && !keep.contains(*k)).cloned().collect();
    for k in gone {
        out.remove(&k);
    }
    out.save()?;
    std::fs::remove_dir_all(&work).ok();
    Ok(())
}
