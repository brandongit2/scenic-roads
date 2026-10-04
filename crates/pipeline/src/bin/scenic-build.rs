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
        // catalog [--held]: held, it goes to catalog-held/, which no server reads (a build to compare
        // before it's switched to: inputs/hold-catalog).
        "catalog" => catalog(&mut out, args.iter().any(|a| a == "--held"))?,
        "unit" => unit_step(&mut out, &args, &scratch)?,
        "pois" => pois_step(&mut out, &args, &scratch)?,
        "roadunits" => roadunits(&mut out)?,
        "terrain" => terrain_step(&mut out, &args)?,
        "terrain-z8" => {
            // terrain-z8 [--raw dir]: AWS's z8 worldwide, repaired (pipeline::terrain_z8), once.
            if out.get(&pipeline::terrain_z8::logical()).is_some() {
                eprintln!("terrain-z8: already made ({})", pipeline::terrain_z8::logical());
            } else {
                let raw_dir = PathBuf::from(opt(&args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
                let (n, none) = pipeline::terrain_z8::build(&mut out, &pipeline::terrain_pack::RawTiles::new(&raw_dir))?;
                eprintln!("terrain-z8: {n} tiles ({none} of open sea)");
            }
        }
        "summits" => summits_step(&mut out, &args, &scratch)?,
        "peaks" => peaks_step(&mut out, &args, &scratch)?,
        "marks" => marks_step(&mut out, &args)?,
        "items" => items_step(&mut out, &args, &scratch)?,
        "heritage" => heritage_step(&mut out, &args, &scratch)?,
        "heritage-sites" => heritage_sites_step(&mut out, &args, &scratch)?,
        "overlays" => {
            // overlays [--pass <date>]: the area overlays and parks from the pass's heritage
            // outputs, the World Heritage outlines with the marks job's dots (ovconv::overlays).
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
            let src = pipeline::markconv::heritage_source(&out, &date);
            let dots = pipeline::ovconv::marks_dots(&out)?;
            pipeline::ovconv::overlays(&mut out, &src, &dots)?;
            out.save()?;
        }
        "registers-import" => registers_import(&mut out, &args, &scratch)?,
        "slope" => slope_step(&mut out, &args)?,
        "labels" => labels_step(&mut out, &args, &scratch)?,
        "pass-sets" => {
            // pass-sets [--pass <date>]: the sets the pass lacks in their current filters
            // (osmpass::SETS versions), from its kept filtered planet.
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
            let src = out.path(out.get(&format!("sources/osm/{date}/filtered")).context("the pass's filtered planet")?);
            std::fs::create_dir_all(&scratch)?;
            let n = pipeline::osmpass::make_missing_sets(&mut out, &date, &src, &scratch)?;
            eprintln!("pass-sets: {n} made");
        }
        "trailends" => {
            // trailends [--pass <date>]: every hiking route's ends, worldwide, from the hikes set.
            let date = opt(&args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
            let set = out.path(out.get(&pipeline::osmpass::set_name(&date, "hikes")).context("the pass has no hikes set (pass-sets makes it)")?);
            std::fs::create_dir_all(&scratch)?;
            let local = scratch.join("set-hikes.osm.pbf");
            std::fs::copy(&set, &local).with_context(|| format!("copy {}", set.display()))?;
            let ends = pipeline::trailends::ends(&local)?;
            let file = scratch.join("trailends.jsonl.zst");
            pipeline::trailends::write(&file, &ends)?;
            out.put_file(&format!("work/trailends/{date}"), "jsonl.zst", &file)?;
            out.save()?;
            std::fs::remove_file(&local).ok();
            eprintln!("trailends: {} ends", ends.len());
        }
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
        pipeline::agent::jobs::report(k as u64, ts.len() as u64, "map tiles");
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
        pipeline::agent::jobs::report(k as u64, qs.len() as u64, "zoomed-out tiles");
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
                None => {
                    let (ways, verts) = (r.read_pod::<roadcore::WayRec>("ways")?, r.read_pod::<[i32; 2]>("verts")?);
                    if r.section("elevu").is_some() {
                        Summary::of(&ways, &verts, roadcore::elev::Elevs::U16(&r.read_pod::<u16>("elevu")?))
                    } else {
                        Summary::of(&ways, &verts, roadcore::elev::Elevs::I16(&r.read_pod::<i16>("elev")?))
                    }
                }
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

fn catalog(out: &mut Out, held: bool) -> Result<()> {
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
    let dir = out.root().join(if held { "catalog-held" } else { "catalog" });
    std::fs::create_dir_all(&dir)?;
    let n = store::catalog::next_n(&dir)?;
    let cat = serde_json::json!({
        "fmt": 1,
        "n": n,
        "created": chrono_now(),
        // The published app that made it (its folder's name), or "development".
        "app": std::env::current_exe().ok().and_then(|e| e.parent().map(pipeline::agent::app_version)).unwrap_or_else(|| "development".into()),
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

/// pois <units…> [--pass <date>]: each unit's landmark candidates (pipeline::candidates), from
/// its piece through `extract --candidates` with the pass's hiking-route ends, as `work/pois/<u>`.
fn pois_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date = match opt(args, "--pass") {
        Some(d) => d,
        None => pipeline::osmpass::latest_pass(out.root()).context("no complete OSM pass on the NAS (--pass)")?,
    };
    let regions = opt(args, "--regions").map(PathBuf::from).unwrap_or_else(|| out.root().join("inputs/regions"));
    let (recipes, _) = pipeline::agent::recipes::load(&regions);
    anyhow::ensure!(!recipes.is_empty(), "no regions in {}", regions.display());
    let outlines_file = out.get(&format!("sources/osm/{date}/outlines")).map(|n| out.path(n));
    let outlines = outlines_file.as_deref().map(pipeline::outlines::Outlines::open).transpose()?;
    let cov = pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &out.root().join("inputs/outlines"))?;
    let trailends = out.get(&format!("work/trailends/{date}")).map(|c| out.path(c)).context("no hiking-route ends for the pass (the trailends step)")?;
    let extract = std::env::current_exe()?.parent().context("bin")?.join("extract");
    std::fs::create_dir_all(scratch)?;
    let units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    for (k, &u) in units.iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas");
        let t = std::time::Instant::now();
        let Some(piece) = out.get(&format!("sources/osm/{date}/pieces/{}", u.dash())).map(|n| out.path(n)) else {
            eprintln!("pois {}: no piece", u.slash());
            continue;
        };
        let local = scratch.join(format!("piece-{}.osm.pbf", u.dash()));
        std::fs::copy(&piece, &local).with_context(|| format!("copy {}", piece.display()))?;
        let dir = scratch.join(format!("pois-{}", u.dash()));
        let mut c = std::process::Command::new(&extract);
        c.arg(&dir).arg("8").arg("--candidates").arg("--trailends").arg(&trailends).arg(&local);
        let o = c.output().with_context(|| format!("run {}", extract.display()))?;
        anyhow::ensure!(o.status.success(), "extract --candidates for {}: {}\n{}", u.slash(), o.status, String::from_utf8_lossy(&o.stderr).lines().rev().take(8).collect::<Vec<_>>().join("\n"));
        let cands = pipeline::candidates::from_pois(&std::fs::read(dir.join("pois.json"))?, u, &cov)?;
        let file = scratch.join(format!("pois-{}.jsonl.zst", u.dash()));
        pipeline::candidates::write(&file, &cands)?;
        out.put_file(&format!("work/pois/{}", u.dash()), "jsonl.zst", &file)?;
        out.save()?;
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(&local).ok();
        std::fs::remove_file(&file).ok();
        eprintln!("pois {}: {} candidates ({:.0?})", u.slash(), cands.len(), t.elapsed());
    }
    Ok(())
}

/// A local copy of a NAS file by logical name, made once under `cache/<logical>/` (content-named,
/// so never stale; older copies go).
fn local_copy(out: &Out, logical: &str, cache: &Path) -> Result<PathBuf> {
    let content = out.get(logical).with_context(|| format!("no {logical} in the manifest"))?;
    let src = out.path(content);
    let name = Path::new(content).file_name().context("file name")?.to_string_lossy().into_owned();
    let dir = cache.join(logical.replace('/', "-"));
    let local = dir.join(&name);
    let size = std::fs::metadata(&src)?.len();
    if std::fs::metadata(&local).is_ok_and(|m| m.len() == size) {
        return Ok(local);
    }
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::copy(&src, &tmp).with_context(|| format!("copy {}", src.display()))?;
    std::fs::rename(&tmp, &local)?;
    for e in std::fs::read_dir(&dir)?.flatten() {
        if e.file_name().to_string_lossy() != name {
            std::fs::remove_file(e.path()).ok();
        }
    }
    // Older passes' copies of the same file go too.
    if let Some(d) = logical.split('/').find(|s| pipeline::osmpass::is_date(s)) {
        let me = logical.replace('/', "-");
        let (pre, post) = me.split_once(d).context("date")?;
        for e in std::fs::read_dir(cache)?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n != me && n.len() == me.len() && n.starts_with(pre) && n.ends_with(post) && pipeline::osmpass::is_date(&n[pre.len()..pre.len() + 10]) {
                std::fs::remove_dir_all(e.path()).ok();
            }
        }
    }
    Ok(local)
}

/// The z8 artifact, from local copies under `cache`.
fn open_z8(out: &Out, cache: &Path) -> Result<pipeline::terrain_z8::Z8> {
    let pack = local_copy(out, &pipeline::terrain_z8::logical(), cache)?;
    let maxes = local_copy(out, &pipeline::terrain_z8::max_logical(), cache)?;
    pipeline::terrain_z8::Z8::open(&pack, &maxes, 4096)
}

/// summits [--pass <date>] [--cache dir]: every summit worldwide with its z8 height (pipeline::summits).
fn summits_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let t = std::time::Instant::now();
    let set = local_copy(out, &pipeline::osmpass::set_name(&date, "summits"), &cache)?;
    let mut summits = pipeline::summits::read_set(&set)?;
    let z8 = open_z8(out, &cache)?;
    let raised = pipeline::summits::add_z8(&mut summits, &z8)?;
    std::fs::create_dir_all(scratch)?;
    let file = scratch.join("summits.jsonl.zst");
    pipeline::summits::write(&file, &summits)?;
    out.put_file(&format!("work/summits/{date}"), "jsonl.zst", &file)?;
    out.save()?;
    std::fs::remove_file(&file).ok();
    eprintln!("summits: {} ({} with a z8 height) ({:.0?})", summits.len(), raised, t.elapsed());
    Ok(())
}

/// peaks <units…> [--pass <date>] [--raw dir] [--cache dir] [--coarse-threads n]: prominence and
/// isolation of each unit's peak candidates (pipeline::peaks::unit), as `work/peaks/<u>`.
fn peaks_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::peaks::unit;
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let raw = pipeline::terrain_pack::RawTiles::new(&PathBuf::from(opt(args, "--raw").unwrap_or_else(|| cache.join("aws-terrarium").to_string_lossy().into_owned())));
    let coarse_threads: usize = opt(args, "--coarse-threads").map(|s| s.parse()).transpose()?.unwrap_or(4);
    let summits = pipeline::summits::read(&local_copy(out, &format!("work/summits/{date}"), &cache)?)?;
    let base8 = unit::Z8Base::new(&summits);
    let z8 = open_z8(out, &cache)?;
    std::fs::create_dir_all(scratch)?;
    let units: Vec<Unit> = positional(args).iter().filter_map(|s| Unit::parse(s)).collect();
    for (k, &u) in units.iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, units.len() as u64, "areas");
        let t = std::time::Instant::now();
        let pois = out.get(&format!("work/pois/{}", u.dash())).map(|c| out.path(c)).with_context(|| format!("no candidates for {} (the pois step)", u.slash()))?;
        let peaks: Vec<unit::UnitPeak> = pipeline::candidates::read(&pois)?
            .into_iter()
            .filter(|c| c.kind == "peak")
            .filter_map(|c| Some(unit::UnitPeak { id: c.osm.clone()?, key: c.key, lon: c.lon, lat: c.lat, ele: c.ele }))
            .collect();
        let want = unit::tiles_wanted(&peaks.iter().map(|p| (p.lon as f64 * 1e-7, p.lat as f64 * 1e-7)).collect::<Vec<_>>(), 29.5);
        let z12 = unit::UnitZ12::load(out, &raw, &want)?;
        let res = unit::run(&peaks, &summits, &base8, &z12, &z8, coarse_threads)?;
        let file = scratch.join(format!("peaks-{}.jsonl.zst", u.dash()));
        {
            use std::io::Write;
            let mut w = zstd::Encoder::new(std::fs::File::create(&file)?, 9)?.auto_finish();
            for (key, o) in &res {
                let mut v = o.json();
                v.as_object_mut().unwrap().remove("i");
                v["key"] = key.clone().into();
                serde_json::to_writer(&mut w, &v)?;
                w.write_all(b"\n")?;
            }
        }
        out.put_file(&format!("work/peaks/{}", u.dash()), "jsonl.zst", &file)?;
        out.save()?;
        std::fs::remove_file(&file).ok();
        eprintln!("peaks {}: {} peaks; z12 tiles {} from the packs, {} from AWS, {} sea ({:.0?})", u.slash(), res.len(), z12.from.0, z12.from.1, z12.from.2, t.elapsed());
    }
    Ok(())
}

/// marks [--pass <date>] [--facts file] [--views file]: the landmark points from the current units'
/// candidates and peaks (pipeline::marksjob), with today's heritage sites, as markdata and the
/// marks packs. Facts (Wikidata, by QID) and monthly pageviews: the items job's when given, else
/// today's (sources/legacy/m1/poi/wikidata.json, sources/legacy/m1/pageviews/items.json).
fn marks_step(out: &mut Out, args: &[String]) -> Result<()> {
    use serde_json::Value;
    use std::collections::HashMap;
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let read_json = |p: PathBuf| -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?)?) };
    // The items job's for this pass, else today's.
    let items = |name: &str, legacy: &str| -> PathBuf {
        match out.get(&format!("sources/items/{date}/{name}")) {
            Some(c) => out.path(c),
            None => out.root().join(legacy),
        }
    };
    let facts_file = opt(args, "--facts").map(PathBuf::from).unwrap_or_else(|| items("facts", "sources/legacy/m1/poi/wikidata.json"));
    let views_file = opt(args, "--views").map(PathBuf::from).unwrap_or_else(|| items("views", "sources/legacy/m1/pageviews/items.json"));
    let facts: HashMap<String, Value> = serde_json::from_value(read_json(facts_file)?)?;
    let views: HashMap<String, f64> = serde_json::from_value(read_json(views_file)?)?;
    let units = pipeline::agent::build::pois_keys(&cov, &date, &out.manifest);
    let mut cands: Vec<(String, pipeline::marksjob::Candidate)> = Vec::new();
    let (mut with_peaks, mut missing_peaks) = (0, 0);
    for (u, _) in &units {
        let Some(pc) = out.get(&format!("work/pois/{}", u.dash())).map(|c| out.path(c)) else { continue };
        let mut peaks: HashMap<String, Value> = HashMap::new();
        if let Some(pk) = out.get(&format!("work/peaks/{}", u.dash())).map(|c| out.path(c)) {
            use std::io::BufRead;
            for line in std::io::BufReader::new(zstd::Decoder::new(std::fs::File::open(&pk)?)?).lines() {
                let mut v: Value = serde_json::from_str(&line?)?;
                let key = v.as_object_mut().and_then(|o| o.remove("key")).and_then(|k| k.as_str().map(str::to_string)).context("a peak without its key")?;
                peaks.insert(key, v);
            }
        }
        for c in pipeline::candidates::read(&pc)? {
            let pk = peaks.get(&c.key).cloned();
            if c.kind == "peak" {
                if pk.is_some() { with_peaks += 1 } else { missing_peaks += 1 }
            }
            cands.push((c.key.clone(), pipeline::marksjob::from_unit(&c, pk, &facts)));
        }
    }
    anyhow::ensure!(missing_peaks == 0, "marks: {missing_peaks} peaks have no prominence yet (the peaks step)");
    // One order for every candidate, whichever unit it came from.
    cands.sort_by(|a, b| a.0.cmp(&b.0));
    let cands: Vec<pipeline::marksjob::Candidate> = cands.into_iter().map(|c| c.1).collect();
    eprintln!("marks: {} candidates from {} units ({with_peaks} peaks)", cands.len(), units.len());
    let pts = pipeline::marksjob::poi_points(&cands, &views);
    let summits = pipeline::marksjob::summits_list(&pts);
    let mut all = pts;
    // The pass's heritage (the heritage job's outputs), else today's.
    let src = pipeline::markconv::heritage_source(out, &date);
    eprintln!("marks: heritage from {src}");
    all.extend(pipeline::markconv::heritage_marks(out, &src)?);
    let c = pipeline::markconv::write(out, all, summits)?;
    eprintln!("marks: {} points, {} markdata tiles, {} thinned tiles", c.points, c.tiles, c.thinned);
    Ok(())
}

/// items [--pass <date>] [--dem dir] [--cache dir]: facts and pageviews for the current units'
/// candidates' Wikidata items (dem/items.py, per pass epoch), as sources/items/<date>/{facts,views}.
fn items_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into()));
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned())).join("items");
    let (mut facts, mut views) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    let is_qid = |q: &str| q.len() > 1 && q.starts_with('Q') && q[1..].bytes().all(|b| b.is_ascii_digit());
    for (u, _) in pipeline::agent::build::pois_keys(&cov, &date, &out.manifest) {
        let Some(pc) = out.get(&format!("work/pois/{}", u.dash())).map(|c| out.path(c)) else { continue };
        for c in pipeline::candidates::read(&pc)? {
            let Some(q) = c.qid.as_deref() else { continue };
            if is_qid(q) {
                facts.insert(q.to_string());
            }
            let first = q.split(';').next().unwrap_or("").trim();
            if is_qid(first) {
                views.insert(first.to_string());
            }
        }
    }
    std::fs::create_dir_all(scratch)?;
    let qfile = scratch.join("qids.json");
    std::fs::write(&qfile, serde_json::to_vec(&serde_json::json!({"facts": facts, "views": views}))?)?;
    let dir = scratch.join("items-out");
    let mut c = std::process::Command::new("uv");
    c.current_dir(&dem).args(["run", "python", "items.py", "--qids"]).arg(&qfile).arg("--epoch").arg(&date).arg("--cache").arg(&cache).arg("--out").arg(&dir);
    let st = c.status().context("run items.py")?;
    anyhow::ensure!(st.success(), "items.py failed: {st}");
    for name in ["facts", "views", "meta"] {
        out.put_file(&format!("sources/items/{date}/{name}"), "json", &dir.join(format!("{name}.json")))?;
    }
    out.save()?;
    eprintln!("items: {} items with facts asked, {} for views", facts.len(), views.len());
    Ok(())
}

/// heritage [--pass <date>] [--dem dir] [--cache dir]: the rest of today's heritage chain
/// (heritagewd, heritagedetails, areadetails, whsshapes, filterprops' and interest's heritage parts,
/// pageviews, layers) in the stand-in root on the heritage-sites job's outputs (docs/phase5.md
/// "Heritage and area flags"), over the same cover: the pass's areas and named objects within it
/// (named with today's filter), and for the World Heritage parts the kept filtered planet within it
/// (one clip per pass and cover, kept in the cache), as today's regional extracts were. Today's park
/// facts and pageview months seed the caches (`sources/registers/legacy-seeds`); the pageview months
/// are the items job's cache, the epoch's months; the layers' English names use today's names table.
/// No stops & sights (the marks job's). Its outputs go to `work/heritage/<date>/<file>`.
fn heritage_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::heritage::{base_logical, cover_tiles, tiles_geojson, COVER_Z};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(scratch)?;
    let epoch = heritage_epoch(out, &date, &cache)?;
    let seeds = registers_extract(out, "sources/registers/legacy-seeds", &cache)?;
    let root = heritage_root(scratch, &dem, &epoch)?;
    let b = root.join("data/build");
    // The heritage-sites job's outputs, as heritage.py left them.
    for stem in ["heritage", "heritage-areas", "special", "indigenous", "heritage-sources"] {
        let c = out.get(&base_logical(&date, stem)).with_context(|| format!("no {stem} for the pass of {date} (the heritage-sites step)"))?;
        std::fs::copy(out.path(c), b.join(format!("{stem}.json")))?;
    }
    // No stops & sights.
    std::fs::write(b.join("pois.json"), br#"{"type":"FeatureCollection","features":[]}"#)?;
    std::fs::write(b.join("details-poi.jsonl"), b"")?;
    std::fs::write(b.join("peaks.json"), b"[]")?;
    let inputs = ["pois.json", "details-poi.jsonl", "peaks.json"];
    // The cover, and the pass's sets within it.
    let tiles = cover_tiles(&cov);
    let poly = scratch.join("cover.geojson");
    std::fs::write(&poly, serde_json::to_vec(&tiles_geojson(COVER_Z, &tiles))?)?;
    areas_over_cover(out, &date, &poly, scratch, &root.join("data/areas/areas.geojsonseq"))?;
    let named = osmium_clip(&out.path(out.get(&pipeline::osmpass::set_name(&date, "named")).context("the pass's named set")?), &poly, &scratch.join("named-cover.osm.pbf"))?;
    // Today's filter (Makefile: named.osm.pbf; the set also keeps the World Heritage tags).
    let named_today = scratch.join("named.osm.pbf");
    let mut c = pipeline::osmpass::osmium();
    c.arg("tags-filter").arg(&named).args([
        "nwr/historic",
        "nwr/heritage",
        "nwr/tourism=museum,attraction,viewpoint",
        "nwr/man_made=lighthouse",
        "nwr/railway=station",
        "nwr/building=train_station,church,cathedral",
        "nwr/amenity=place_of_worship",
        "nwr/boundary=protected_area,national_park",
        "nwr/leisure=park",
        "nwr/military",
        "-o",
    ]);
    c.arg(&named_today).arg("--overwrite");
    osmium_run(c, "osmium tags-filter (named)")?;
    std::fs::create_dir_all(epoch.join("osm"))?;
    let mut c = pipeline::osmpass::osmium();
    c.arg("export").arg(&named_today).args(["-f", "geojsonseq", "--overwrite", "-o"]).arg(epoch.join("osm/named.geojsonseq"));
    osmium_run(c, "osmium export (named)")?;
    std::fs::remove_file(&named).ok();
    std::fs::remove_file(&named_today).ok();
    // The kept filtered planet within the cover, once per pass and cover: today's merged extract.
    let merged = merged_over_cover(out, &date, &poly, &cache)?;
    std::os::unix::fs::symlink(&merged, root.join("data/osm/merged.osm.pbf"))?;
    // Today's park facts, seeding this pass's cache of them.
    let facts = epoch.join("areas-wikidata.json");
    if !facts.exists() {
        std::fs::copy(seeds.join("areas/wikidata.json"), &facts)?;
    }
    std::os::unix::fs::symlink(&facts, root.join("data/areas/wikidata.json"))?;
    // The pageview months: the items job's cache (the same files), today's months seeding it.
    let pv = cache.join("items");
    std::fs::create_dir_all(pv.join("months"))?;
    for e in std::fs::read_dir(seeds.join("pageviews/months"))?.flatten() {
        let dest = pv.join("months").join(e.file_name());
        if !dest.exists() {
            std::fs::copy(e.path(), &dest)?;
        }
    }
    std::os::unix::fs::symlink(&pv, root.join("data/pageviews"))?;
    // Today's names table, for the layers' English names.
    std::fs::create_dir_all(root.join("data/names"))?;
    std::os::unix::fs::symlink(seeds.join("names/english.json"), root.join("data/names/english.json"))?;
    // Today's chain.
    let chain = [
        ("heritagewd.py", vec![]),
        ("heritagedetails.py", vec![]),
        ("areadetails.py", vec![]),
        ("whsshapes.py", vec![]),
        ("filterprops.py", vec![]),
        ("pageviews.py", vec!["--epoch", date.as_str()]),
        ("interest.py", vec![]),
        ("layers.py", vec![]),
    ];
    for (k, (script, sargs)) in chain.iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, chain.len() as u64, &format!("scripts ({script})"));
        heritage_script(&root, &cache, script, sargs)?;
    }
    // Outputs (not the stops & sights' stand-ins, nor the layers made from them), the pass's
    // earlier ones this run didn't make dropped.
    let mut wrote: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&b)?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    for p in files {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let (stem, ext) = name.split_once('.').unwrap_or((name.as_str(), "bin"));
        if inputs.contains(&name.as_str()) || name.ends_with(".tmp") || name.starts_with('.') || stem.starts_with("layer-pois") || stem == "layer-summits" {
            continue;
        }
        let l = format!("work/heritage/{date}/{stem}");
        out.put_file(&l, ext, &p)?;
        wrote.insert(l);
    }
    let prefix = format!("work/heritage/{date}/");
    let stale: Vec<String> = out
        .manifest
        .range(prefix.clone()..)
        .take_while(|(l, _)| l.starts_with(&prefix))
        .map(|(l, _)| l.clone())
        .filter(|l| !l[prefix.len()..].contains('/') && !wrote.contains(l))
        .collect();
    for l in stale {
        out.remove(&l);
    }
    out.save()?;
    eprintln!("heritage: {} files ({:.0?})", wrote.len(), t0.elapsed());
    Ok(())
}

/// Runs an osmium command, failing with its name.
fn osmium_run(mut c: std::process::Command, what: &str) -> Result<()> {
    let st = c.status().with_context(|| format!("run {what}"))?;
    anyhow::ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// `src` within the polygon file (whole relations: smart), into `dest`.
fn osmium_clip(src: &Path, poly: &Path, dest: &Path) -> Result<PathBuf> {
    let mut c = pipeline::osmpass::osmium();
    c.args(["extract", "--strategy", "smart", "--overwrite", "-p"]).arg(poly).arg(src).arg("-o").arg(dest);
    osmium_run(c, &format!("osmium extract ({})", src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))?;
    Ok(dest.to_path_buf())
}

/// The pass's protected areas and Indigenous lands within the cover, as today's areas.geojsonseq.
fn areas_over_cover(out: &Out, date: &str, poly: &Path, scratch: &Path, dest: &Path) -> Result<()> {
    let set = out.path(out.get(&pipeline::osmpass::set_name(date, "areas")).context("the pass's areas set")?);
    let clip = osmium_clip(&set, poly, &scratch.join("areas-cover.osm.pbf"))?;
    let mut c = pipeline::osmpass::osmium();
    c.args(["export", "-f", "geojsonseq", "--geometry-types=polygon", "-a", "type,id", "--overwrite", "-o"]).arg(dest).arg(&clip);
    osmium_run(c, "osmium export (areas)")?;
    std::fs::remove_file(&clip).ok();
    Ok(())
}

/// The pass's kept filtered planet within the cover (whole relations), kept in the cache for the
/// pass and cover (`heritage-merged-<date>-<cover>.osm.pbf`; others go): what today's regional
/// extracts were to whsshapes.py. Read from the NAS (an hour or so for the 60 GB file: osmium reads
/// it twice), once per pass and cover.
fn merged_over_cover(out: &Out, date: &str, poly: &Path, cache: &Path) -> Result<PathBuf> {
    let id = store::naming::hash16(&std::fs::read(poly)?)[..12].to_string();
    let name = format!("heritage-merged-{date}-{id}.osm.pbf");
    let dest = cache.join(&name);
    if dest.exists() {
        return Ok(dest);
    }
    let src = out.path(out.get(&format!("sources/osm/{date}/filtered")).context("the pass's filtered planet")?);
    let tmp = cache.join(format!("{name}.tmp.osm.pbf"));
    osmium_clip(&src, poly, &tmp)?;
    std::fs::rename(&tmp, &dest)?;
    for e in std::fs::read_dir(cache)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("heritage-merged-") && n != name {
            std::fs::remove_file(e.path()).ok();
        }
    }
    Ok(dest)
}

/// registers-import --from <dir> [--name legacy]: a registers' snapshot (today's: the build Mac's
/// data/heritage, without osm/, which the pass's sets replace) as one archive in the manifest,
/// `sources/registers/<name>` (tar.zst, files in sorted order): what the heritage jobs extract,
/// once per archive (thousands of small files are slow to copy over SMB one by one).
fn registers_import(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    let from = std::fs::canonicalize(opt(args, "--from").context("--from <dir>")?)?;
    let name = opt(args, "--name").unwrap_or_else(|| "legacy".into());
    std::fs::create_dir_all(scratch)?;
    let (list, tar) = (scratch.join("registers.list"), scratch.join("registers.tar.zst"));
    let mut c = std::process::Command::new("sh");
    c.env("PATH", format!("/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default()));
    c.arg("-c")
        .arg(r#"set -e; cd "$1"; find . -type f ! -path './osm/*' ! -name .DS_Store | LC_ALL=C sort > "$2"; tar -cf - -T "$2" | zstd -T0 -10 -q -f -o "$3""#)
        .arg("sh")
        .arg(&from)
        .arg(&list)
        .arg(&tar);
    let st = c.status().context("tar | zstd")?;
    anyhow::ensure!(st.success(), "archiving {} failed: {st}", from.display());
    let files = std::fs::read_to_string(&list)?.lines().count();
    let size = std::fs::metadata(&tar)?.len();
    let got = out.put_file(&format!("sources/registers/{name}"), "tar.zst", &tar)?;
    out.save()?;
    eprintln!("registers-import: {files} files, {} MB → {got}", size >> 20);
    Ok(())
}

/// This pass's working copy of the registers' snapshot, which the heritage scripts add their caches
/// to: the archive extracted once (`cache/registers-<id>`), then cloned per pass
/// (`cache/heritage-<date>-<id>`, APFS clones cost nothing), so a pass's runs share their caches and
/// a new pass or snapshot starts again from the snapshot. Other passes' and snapshots' copies go.
fn heritage_epoch(out: &Out, date: &str, cache: &Path) -> Result<PathBuf> {
    let snap = registers_extract(out, "sources/registers/legacy", cache)?;
    let id = snap.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("registers-")).context("registers folder")?.to_string();
    let epoch = cache.join(format!("heritage-{date}-{id}"));
    if !epoch.join(".done").exists() {
        std::fs::remove_dir_all(&epoch).ok();
        let st = std::process::Command::new("cp").arg("-c").arg("-R").arg(&snap).arg(&epoch).status()?;
        if !st.success() {
            std::fs::remove_dir_all(&epoch).ok();
            let st = std::process::Command::new("cp").arg("-R").arg(&snap).arg(&epoch).status()?;
            anyhow::ensure!(st.success(), "copying {} failed: {st}", snap.display());
        }
        std::fs::write(epoch.join(".done"), b"")?;
    }
    for e in std::fs::read_dir(cache)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let other_epoch = n.strip_prefix("heritage-").is_some_and(|r| r.len() > 11 && pipeline::osmpass::is_date(&r[..10])) && e.path() != epoch;
        // (heritage-data: the first heritage job's rsync'd copy.)
        if other_epoch || n == "heritage-data" {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    Ok(epoch)
}

/// An archive of `sources/registers/` extracted once into the cache (`registers-<id>`, by its
/// content), the current ones kept: the snapshot and the seeds.
fn registers_extract(out: &Out, logical: &str, cache: &Path) -> Result<PathBuf> {
    let content = out.get(logical).with_context(|| format!("no {logical} in the manifest (scenic-build registers-import)"))?.to_string();
    let id = store::naming::hash16(content.as_bytes())[..12].to_string();
    std::fs::create_dir_all(cache)?;
    let dir = cache.join(format!("registers-{id}"));
    if !dir.join(".done").exists() {
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir)?;
        let mut c = std::process::Command::new("sh");
        c.env("PATH", format!("/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default()));
        c.arg("-c").arg(r#"set -e; zstd -dcq "$1" | tar -xf - -C "$2""#).arg("sh").arg(out.path(&content)).arg(&dir);
        let st = c.status().context("zstd | tar")?;
        anyhow::ensure!(st.success(), "extracting {content} failed: {st}");
        std::fs::write(dir.join(".done"), b"")?;
    }
    // Extracts of archives the manifest no longer names go.
    let current: std::collections::BTreeSet<String> = out
        .manifest
        .range("sources/registers/".to_string()..)
        .take_while(|(l, _)| l.starts_with("sources/registers/"))
        .map(|(_, c)| format!("registers-{}", &store::naming::hash16(c.as_bytes())[..12]))
        .collect();
    for e in std::fs::read_dir(cache)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("registers-") && !current.contains(&n) {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    Ok(dir)
}

/// A stand-in root laid out as the repository for today's heritage scripts: `dem/` the app's
/// scripts and their Python project, `data/heritage` this pass's copy of the registers' snapshot.
fn heritage_root(scratch: &Path, dem: &Path, epoch: &Path) -> Result<PathBuf> {
    let root = scratch.join("heritage-root");
    std::fs::remove_dir_all(&root).ok();
    for d in ["dem", "data/build", "data/areas", "data/osm"] {
        std::fs::create_dir_all(root.join(d))?;
    }
    for e in std::fs::read_dir(dem)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if e.path().is_file() && (n.ends_with(".py") || n == "pyproject.toml" || n == "uv.lock" || n == ".python-version") {
            std::fs::copy(e.path(), root.join("dem").join(&n))?;
        }
    }
    std::os::unix::fs::symlink(epoch, root.join("data/heritage"))?;
    Ok(root)
}

/// Runs one of today's scripts in a stand-in root (its venv kept in the cache; the lock file as is).
fn heritage_script(root: &Path, cache: &Path, script: &str, args: &[&str]) -> Result<()> {
    let t = std::time::Instant::now();
    let mut c = std::process::Command::new("uv");
    c.current_dir(root.join("dem")).env("UV_PROJECT_ENVIRONMENT", cache.join("heritage-venv")).args(["run", "--frozen", "python", script]).args(args);
    let st = c.status().with_context(|| format!("run {script}"))?;
    anyhow::ensure!(st.success(), "{script} failed: {st}");
    eprintln!("heritage: {script} done ({:.0?})", t.elapsed());
    Ok(())
}

/// heritage-sites [--pass <date>] [--dem dir] [--cache dir]: the heritage sites and designated
/// areas the units read (pipeline::heritage): today's heritage.py in a stand-in root, over the
/// tiles within 20 km of the coverage, on the registers' snapshot and the pass's protected areas
/// (its `areas` set within those tiles, as today's areas.geojsonseq). Its outputs go to
/// `work/heritage/<date>/base/<file>`, and per z6 tile the sites' positions and the area polygons.
fn heritage_sites_step(out: &mut Out, args: &[String], scratch: &Path) -> Result<()> {
    use pipeline::heritage::{base_logical, cover_tiles, put_slices, slice_areas, slice_sites, tiles_bytes, tiles_geojson, COVER_Z};
    let date = opt(args, "--pass").or_else(|| pipeline::osmpass::latest_pass(out.root())).context("no complete OSM pass")?;
    let cov = coverage_of(out, args)?;
    let dem = std::fs::canonicalize(opt(args, "--dem").unwrap_or_else(|| "dem".into()))?;
    let cache = PathBuf::from(opt(args, "--cache").unwrap_or_else(|| scratch.join("cache").to_string_lossy().into_owned()));
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(scratch)?;
    let epoch = heritage_epoch(out, &date, &cache)?;
    let root = heritage_root(scratch, &dem, &epoch)?;
    let b = root.join("data/build");
    // The cover: its tiles for heritage.py, as rectangles for osmium.
    let tiles = cover_tiles(&cov);
    std::fs::write(b.join("cover.idx"), tiles_bytes(&tiles))?;
    let poly = scratch.join("cover.geojson");
    std::fs::write(&poly, serde_json::to_vec(&tiles_geojson(COVER_Z, &tiles))?)?;
    eprintln!("heritage-sites: {} z{COVER_Z} tiles within 20 km of the coverage ({:.0?})", tiles.len(), t0.elapsed());
    // The pass's protected areas and Indigenous lands within them (whole relations: smart).
    areas_over_cover(out, &date, &poly, scratch, &root.join("data/areas/areas.geojsonseq"))?;
    heritage_script(&root, &cache, "heritage.py", &["../data/build", "--tiles", "../data/build/cover.idx", "--zoom", &COVER_Z.to_string(), "--date", &date])?;
    // The units' slices, then the whole files.
    let sites = slice_sites(&std::fs::read(b.join("heritage.json"))?)?;
    let areas = slice_areas(&std::fs::read_to_string(b.join("area-shapes.geojsonseq"))?)?;
    let (ns, na) = put_slices(out, &date, &sites, &areas)?;
    for (stem, ext) in [("heritage", "json"), ("heritage-areas", "json"), ("special", "json"), ("indigenous", "json"), ("heritage-sources", "json"), ("area-shapes", "geojsonseq")] {
        out.put_file(&base_logical(&date, stem), ext, &b.join(format!("{stem}.{ext}")))?;
    }
    out.save()?;
    eprintln!("heritage-sites: {} sites' and {} areas' slices ({:.0?})", ns, na, t0.elapsed());
    Ok(())
}

/// The unit step's global-source layers: the manifest's, or a pilot's published catalog.
fn layers_source<'a>(out: &'a Out, pilot: &'a Option<(PathBuf, store::catalog::Catalog)>) -> pipeline::stage::Source<'a> {
    match pilot {
        Some((r, c)) => pipeline::stage::Source::Catalog(r, c),
        None => pipeline::stage::Source::Manifest(out),
    }
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
    // Global-source layers: as this build's manifest has them now (what the unit keys hash: a
    // terrain job of the same plan is published only with its catalog, at the end), or another
    // root's published catalog (a pilot builds against the real one).
    let pilot = match opt(args, "--layers-root").map(PathBuf::from) {
        Some(r) => {
            let c = store::catalog::latest(&r.join("catalog"))?.context("no catalog for the global-source layers")?;
            Some((r, c))
        }
        None => None,
    };
    let bin = std::env::current_exe()?.parent().context("bin")?.to_path_buf();
    let tools = Tools {
        bin,
        dem: PathBuf::from(opt(args, "--dem").unwrap_or_else(|| "dem".into())),
        cache: PathBuf::from(opt(args, "--cache-dir").unwrap_or_else(|| "data/cache".into())),
        buildings: opt(args, "--buildings").map(PathBuf::from),
        spacing_m: 8,
    };
    // The pass's heritage sites and designated areas (the heritage-sites step), for every unit's
    // flags.
    anyhow::ensure!(
        out.get(&pipeline::heritage::base_logical(&date, "heritage-sources")).is_some(),
        "no heritage sites for the pass of {date} (the heritage-sites step)"
    );
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
    let n = units.len() as u64;
    for (k, u) in units.into_iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, n, "areas");
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
        let rep = {
            let o: &Out = out;
            let heritage = |b: [f64; 4], d: &Path| pipeline::heritage::unit_inputs(o, &date, b, d);
            build_folder(u, &local_piece, &dir, &cov, &layers_source(o, &pilot), &tools, &heritage)?
        };
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
        eprintln!("unit {}: {} of {} ways touch the coverage, {} owned; {} heritage sites, {} area polygons", u.slash(), rep.kept_ways, rep.piece_ways, rep.owned, rep.heritage, rep.areas);
        if rep.kept_ways == 0 || rep.owned == 0 {
            // None of its ways in the coverage (any more): a base pack and road values from an
            // earlier coverage go, so the map and the map tiles stop showing them.
            let gone: Vec<String> = [format!("base/{}", u.dash()), format!("global/roads/{}", u.dash())].into_iter().filter(|l| out.get(l).is_some()).collect();
            if !gone.is_empty() {
                for l in &gone {
                    out.remove(l);
                }
                out.save()?;
                eprintln!("unit {}: removed its earlier {}", u.slash(), gone.join(" and "));
            }
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
        let r = store::sect::SectReader::open(store::range::PlainFile::open(&out.path(content))?)?;
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

/// The z6 tiles a terrain or slope run makes, by z3 pack: those near the coverage (as the agent's
/// keys list them, `build::coverage_tiles`) of each z3 pack named (`3/x/y`, as the agent asks), or
/// the z6 tiles named, or with none named every z6 tile near the coverage.
fn terrain_targets(cov: &pipeline::coverage::Coverage, args: &[String]) -> Result<BTreeMap<(u32, u32), Vec<(u32, u32)>>> {
    let near = pipeline::agent::build::coverage_tiles(cov);
    let named: Vec<Unit> = positional(args).iter().map(|s| Unit::parse(s).with_context(|| format!("not a tile: {s}"))).collect::<Result<_>>()?;
    if named.is_empty() {
        return Ok(near);
    }
    let mut by_q: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for t in named {
        match t.z {
            3 => {
                let list = near.get(&(t.x, t.y)).with_context(|| format!("3/{}/{}: no tile of it is near the coverage", t.x, t.y))?;
                by_q.entry((t.x, t.y)).or_default().extend(list);
            }
            6 => by_q.entry((t.x >> 3, t.y >> 3)).or_default().push((t.x, t.y)),
            z => anyhow::bail!("{z}/{}/{}: terrain and slope take z3 packs or z6 tiles", t.x, t.y),
        }
    }
    for list in by_q.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    Ok(by_q)
}

fn terrain_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let by_q = terrain_targets(&cov, args)?;
    eprintln!("terrain: {} z6 tiles in {} z3 packs", by_q.values().map(Vec::len).sum::<usize>(), by_q.len());
    // AWS's raw tiles, kept on this Mac (the build cache).
    let raw_dir = PathBuf::from(opt(args, "--raw").unwrap_or_else(|| out.scratch.join("aws-terrarium").to_string_lossy().into_owned()));
    let raw = pipeline::terrain_pack::RawTiles::new(&raw_dir);
    let n = by_q.len() as u64;
    for (k, (q, list)) in by_q.into_iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, n, "parts");
        let t = std::time::Instant::now();
        let r = pipeline::terrain_pack::build_q(out, &raw, q, &list, &cov)?;
        eprintln!("terrain 3/{}/{}: {r:?} ({:.0?})", q.0, q.1, t.elapsed());
    }
    Ok(())
}

fn slope_step(out: &mut Out, args: &[String]) -> Result<()> {
    let cov = coverage_of(out, args)?;
    let by_q = terrain_targets(&cov, args)?;
    eprintln!("slope: {} z6 tiles in {} z3 packs", by_q.values().map(Vec::len).sum::<usize>(), by_q.len());
    let n = by_q.len() as u64;
    for (k, (q, list)) in by_q.into_iter().enumerate() {
        pipeline::agent::jobs::report(k as u64, n, "parts");
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
    let set = out.get(&pipeline::osmpass::set_name(&date, "labels")).context("the pass has no labels set (passes before 2026-10-02)")?.to_string();
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
