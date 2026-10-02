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
            pack(&mut out, &cache, &positional(&args))?
        }
        "lo" => {
            let cache = PathBuf::from(opt(&args, "--cache").unwrap_or_else(|| "/tmp/scenic-cache".into()));
            lo(&mut out, &cache, &positional(&args))?
        }
        "osm-pass" => {
            let planet = PathBuf::from(opt(&args, "--planet").context("--planet <path>")?);
            let date = opt(&args, "--date").context("--date YYYY-MM-DD")?;
            let extract = PathBuf::from(opt(&args, "--extract").unwrap_or_else(|| "target/release/extract".into()));
            let planetiler = PathBuf::from(opt(&args, "--planetiler").unwrap_or_else(|| "tools/planetiler.jar".into()));
            pipeline::osmpass::check_tools(&extract, &planetiler)?;
            pipeline::osmpass::run_pass(&mut out, &planet, &date, &scratch, &extract, &planetiler)?
        }
        "verify" => {
            let n = out.verify(&SSH, NAS_ROOT)?;
            eprintln!("verified {n} uploads");
        }
        "catalog" => catalog(&mut out)?,
        s => bail!("unknown step {s:?} (convert-legacy, pack, lo, osm-pass, verify, catalog)"),
    }
    out.save()?;
    eprintln!("{step}: done in {:.0?}", t0.elapsed());
    Ok(())
}

// ---- convert-legacy ------------------------------------------------------------------------

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
        put_sect(out, &format!("global/roads/{}", u.dash()), serde_json::json!({"fmt": 1, "unit": u.slash()}), &[("roads", b(&recs))])?;
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
fn open_units(out: &Out, cache: &Path) -> Result<Vec<BasePack>> {
    std::fs::create_dir_all(cache)?;
    let mut units: Vec<(String, String, String)> = Vec::new();
    for (k, v) in &out.manifest {
        if let Some(u) = k.strip_prefix("base/") {
            let roads = out.get(&format!("global/roads/{u}")).with_context(|| format!("no road values for {u}"))?;
            units.push((u.to_string(), v.clone(), roads.to_string()));
        }
    }
    let mut packs = Vec::with_capacity(units.len());
    for (u, base, roads) in units {
        let mut local = Vec::new();
        for name in [&base, &roads] {
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

fn pack(out: &mut Out, cache: &Path, only: &[String]) -> Result<()> {
    let packs = open_units(out, cache)?;
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
        put_sect(
            out,
            &format!("hidata/{}", t.dash()),
            serde_json::json!({"fmt": 1, "tile": t.slash(), "here": hd.here.len(), "parts": hd.parts.len(), "climbs": hd.climbs.len()}),
            &[
                ("here", b(&hd.here)),
                ("ends", b(&hd.ends)),
                ("parts", b(&hd.parts)),
                ("psamples", b(&hd.psamples)),
                ("pch", b(&hd.pch)),
                ("climbs", b(&hd.climbs)),
                ("climbgeom", b(&hd.climbgeom)),
            ],
        )?;
        out.save()?;
        eprintln!("pack {} ({}/{}): {} ways, {} road tiles, {} parts, {} climbs ({:.0?})", t.slash(), k + 1, ts.len(), in_t.len(), roads.len(), hd.parts.len(), hd.climbs.len(), t0.elapsed());
    }
    Ok(())
}

// ---- lo packs -------------------------------------------------------------------------------

fn lo(out: &mut Out, cache: &Path, only: &[String]) -> Result<()> {
    let packs = open_units(out, cache)?;
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

fn catalog(out: &mut Out) -> Result<()> {
    let mut layers: BTreeMap<String, LayerOut> = BTreeMap::new();
    let (mut base, mut roads, mut hidata, mut global, mut basemap) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), Vec::new());
    for logical in out.manifest.keys() {
        let parts: Vec<&str> = logical.split('/').collect();
        match parts.as_slice() {
            ["layers", "basemap", _] => basemap.push(logical.clone()),
            ["layers", layer, scope, key] => {
                let u = Unit::parse(key).context("pack key")?;
                let enc = match *layer {
                    "roads" | "rails" => "rt7",
                    "terrain" => "terrarium-png",
                    "slope" => "slope4-png",
                    "labels" => "mvt",
                    l if l.starts_with("trees-") => "terrarium-webp",
                    l if l.starts_with("grid-") => "u8-zstd",
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
            ["global", ..] => {
                global.insert(logical.trim_start_matches("global/").to_string(), logical.clone());
            }
            _ => {}
        }
    }
    // Zoom ranges as the layers really have them (from the legacy conversion's record, else the scopes).
    if let Some(name) = out.get("global/legacy/layers") {
        if let Ok(b) = std::fs::read(out.path(name)) {
            if let Ok(rec) = serde_json::from_slice::<BTreeMap<String, LayerOut>>(&b) {
                for (l, r) in rec {
                    if let Some(x) = layers.get_mut(&l) {
                        x.minzoom = r.minzoom;
                        x.maxzoom = r.maxzoom;
                    }
                }
            }
        }
    }
    if let Some(r) = layers.get_mut("roads") {
        (r.minzoom, r.maxzoom) = (4, 14);
    }
    if let Some(r) = layers.get_mut("rails") {
        (r.minzoom, r.maxzoom) = (4, 14);
    }
    let meta: serde_json::Value = out.get("global/legacy/roads").and_then(|n| std::fs::read(out.path(n)).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let units: Vec<String> = base.keys().cloned().collect();
    let files: BTreeMap<String, serde_json::Value> = out.manifest.iter().map(|(l, n)| {
        let size = std::fs::metadata(out.path(n)).map(|m| m.len()).unwrap_or(0);
        (l.clone(), serde_json::json!({"file": n, "size": size, "fmt": 1}))
    }).collect();
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
