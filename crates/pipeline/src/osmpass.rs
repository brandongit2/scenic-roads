//! The OSM pass (docs/plan.md §6): the planet, fetched onto the NAS, turned on the build Mac into
//! what every later step reads: the filtered planet, pieces per z6 unit (10 km buffer, ways whole,
//! multipolygons complete), the worldwide sets, the basemap, and worldwide road values.
//!
//! Each stage leaves a marker in the scratch folder, so a pass interrupted by sleep, a reboot or a
//! full disk resumes at the stage it was in; a stage that touched the NAS is redone from its inputs.

use crate::chain::{self, Interner, Link, RoadVal};
use crate::hipack::{grow, tile_bounds};
use crate::legacy::Unit;
use crate::out::Out;
use anyhow::{bail, ensure, Context, Result};
use rayon::prelude::*;
use roadcore::{class, flag, Ways};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the pipeline reads from OSM: the union of every step's tags, generous (a tag left out costs a
/// new planet download). Referenced nodes and members come along.
pub const FILTER_A: &[&str] = &[
    "nwr/highway", "nwr/railway", "r/route", "nwr/public_transport", "nwr/amenity", "nwr/tourism", "nwr/historic",
    "nwr/heritage", "nwr/natural", "nwr/waterway", "nwr/water", "nwr/man_made", "nwr/leisure", "nwr/boundary",
    "nwr/building=train_station,church,cathedral,temple,shrine,mosque,synagogue,castle", "nwr/landuse=forest,reservoir,basin,salt_pond",
    "nwr/military", "nwr/place", "n/barrier", "nwr/aerialway", "nwr/mountain_pass", "nwr/wikidata",
];

/// The worldwide sets, each a filter of the filtered planet.
pub const SETS: &[(&str, &[&str])] = &[
    ("rail", &["nwr/railway", "nwr/public_transport", "r/route=train,subway,tram,light_rail,monorail,funicular,railway"]),
    ("ferries", &["w/route=ferry", "r/route=ferry", "nwr/amenity=ferry_terminal"]),
    ("areas", &["wr/boundary=national_park,protected_area,aboriginal_lands", "wr/leisure=nature_reserve"]),
    ("places", &["n/place"]),
    ("outlines", &["r/boundary=administrative", "r/ISO3166-1", "r/ISO3166-2"]),
];

/// The basemap's input (Planetiler's OpenMapTiles layers water, waterway, boundary, place,
/// water_name, park).
pub const FILTER_BASEMAP: &[&str] = &[
    "nwr/natural=water,bay,strait,wetland,glacier", "nwr/water", "nwr/waterway", "nwr/landuse=reservoir,basin,salt_pond",
    "nwr/leisure=nature_reserve,park", "nwr/boundary=administrative,national_park,protected_area,disputed", "nwr/place",
];

/// Piece buffer around a unit, km.
pub const BUFFER_KM: f64 = 10.0;

fn osmium() -> Command {
    let mut c = Command::new("osmium");
    c.env("PATH", format!("/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default()));
    c
}

fn run(mut c: Command, what: &str) -> Result<()> {
    eprintln!("$ {what}");
    let st = c.status().with_context(|| format!("run {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// A stage's completion marker.
fn done(scratch: &Path, stage: &str) -> PathBuf {
    scratch.join(format!("{stage}.done"))
}

fn mark(scratch: &Path, stage: &str) -> Result<()> {
    std::fs::write(done(scratch, stage), b"")?;
    Ok(())
}

/// Copy a (large) file, resuming a partial copy: what's there already is kept when its size is
/// shorter than the source's, and the rest appended.
pub fn copy_resume(src: &Path, dst: &Path) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let total = std::fs::metadata(src)?.len();
    let have = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
    if have == total {
        return Ok(());
    }
    ensure!(have < total, "{} is larger than its source", dst.display());
    let mut s = std::fs::File::open(src)?;
    s.seek(SeekFrom::Start(have))?;
    let mut d = std::fs::OpenOptions::new().create(true).append(true).open(dst)?;
    let mut buf = vec![0u8; 16 << 20];
    let mut done = have;
    let t0 = std::time::Instant::now();
    loop {
        let n = s.read(&mut buf)?;
        if n == 0 {
            break;
        }
        d.write_all(&buf[..n])?;
        done += n as u64;
        if done % (2 << 30) < n as u64 {
            eprintln!("copy {}: {:.1}/{:.1} GB ({:.0} MB/s)", src.display(), done as f64 / 1e9, total as f64 / 1e9, (done - have) as f64 / 1e6 / t0.elapsed().as_secs_f64());
        }
    }
    d.sync_all()?;
    ensure!(done == total, "short copy of {}", src.display());
    Ok(())
}

/// Every z3 tile, with its pieces' bounds (the tile grown by the buffer).
fn z3_tiles() -> Vec<Unit> {
    (0..8u32).flat_map(|x| (0..8u32).map(move |y| Unit { z: 3, x, y })).collect()
}

/// An osmium extract config: one bbox per tile (grown by the buffer) → `dir/<z-x-y>.osm.pbf`.
fn extract_config(tiles: &[Unit], dir: &Path) -> serde_json::Value {
    let ex: Vec<serde_json::Value> = tiles
        .iter()
        .map(|u| {
            let b = grow(tile_bounds(u.z, u.x, u.y), BUFFER_KM);
            let d = |v: i32| v as f64 * 1e-7;
            serde_json::json!({"output": format!("{}.osm.pbf", u.dash()), "bbox": [d(b[0]), d(b[1]), d(b[2]), d(b[3])]})
        })
        .collect();
    serde_json::json!({"directory": dir, "extracts": ex})
}

/// Cut `input` into one piece per tile, in batches (osmium keeps an id set per output).
fn cut(input: &Path, tiles: &[Unit], dir: &Path, batch: usize) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for (i, chunk) in tiles.chunks(batch).enumerate() {
        let cfg = dir.join(format!("extract-{i}.json"));
        std::fs::write(&cfg, serde_json::to_vec_pretty(&extract_config(chunk, dir))?)?;
        let mut c = osmium();
        c.args(["extract", "--strategy", "smart", "-S", "types=multipolygon", "--overwrite", "-c"]).arg(&cfg).arg(input);
        run(c, &format!("osmium extract {} tiles of {}", chunk.len(), input.display()))?;
        std::fs::remove_file(&cfg).ok();
    }
    Ok(())
}

/// A piece with nothing in it (osmium writes a header-only file for an empty bbox).
fn is_empty_piece(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() < 1024).unwrap_or(true)
}

/// The pieces of one pass, as the build manifest records them.
#[derive(serde::Serialize, serde::Deserialize, Default, Debug)]
pub struct Pieces {
    pub date: String,
    /// Unit "6/x/y" → logical name of its piece.
    pub pieces: BTreeMap<String, String>,
}

/// One unit's chaining inputs: its owned ways (OSM id, length) and the pairs made at its nodes.
#[derive(Default)]
pub struct UnitLinks {
    pub ways: Vec<(u64, f32)>,
    /// (way a << 1 | end a, way b << 1 | end b) by OSM id.
    pub pairs: Vec<(u64, u64)>,
}

/// Run `extract` on a piece and work out its unit's chaining inputs.
pub fn unit_links(extract: &Path, piece: &Path, unit: Unit, work: &Path) -> Result<UnitLinks> {
    std::fs::create_dir_all(work)?;
    let mut c = Command::new(extract);
    c.arg(work).arg("8").arg(piece);
    c.stderr(std::process::Stdio::null());
    run(c, &format!("extract {}", piece.display()))?;
    let wv = Ways::open(work)?;
    let strings = roadcore::read_strings(work)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let tb = tile_bounds(unit.z, unit.x, unit.y);
    let inside = |p: [i32; 2]| p[0] >= tb[0] && p[0] < tb[2] && p[1] >= tb[1] && p[1] < tb[3];
    let mut names = Interner::default();
    let mut refs = Interner::default();
    let mut pool: Vec<u32> = Vec::new();
    let mut links: Vec<Link> = Vec::with_capacity(ways.len());
    for w in ways {
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        let (ends, heading, len) = chain::shape(&verts[r]);
        let kind = if w.vcount < 2 || w.class == class::FERRY {
            chain::KIND_NONE
        } else if class::is_rail(w.class) {
            chain::KIND_RAIL
        } else {
            chain::KIND_ROAD
        };
        let (name, (refs_at, refs_len)) = if kind == chain::KIND_RAIL {
            let raw = if w.name != 0 { strings[w.name as usize].as_str() } else { strings[w.route as usize].split(" · ").next().unwrap_or("") };
            (names.id(raw.split(':').next().unwrap_or("").trim()), (pool.len() as u32, 0))
        } else {
            (names.id(&strings[w.name as usize]), chain::intern_refs(&strings[w.ref_ as usize], &mut refs, &mut pool))
        };
        links.push(Link { id: w.id as u64, kind, class: w.class, oneway: w.flags & flag::ONEWAY != 0, name, refs_at, refs_len, ends, heading, len });
    }
    // Pairs at this unit's nodes only: the piece holds every way through them.
    let partner = chain::pair(&links, &pool, &inside);
    let mut out = UnitLinks::default();
    for (i, l) in links.iter().enumerate() {
        if inside(l.ends[0]) {
            out.ways.push((l.id, l.len));
        }
        for e in 0..2 {
            let p = partner[i][e];
            if p != chain::NONE && inside(l.ends[e]) {
                let (j, f) = ((p >> 1) as usize, (p & 1) as u64);
                let a = l.id << 1 | e as u64;
                let b = links[j].id << 1 | f;
                if a < b {
                    out.pairs.push((a, b));
                }
            }
        }
    }
    std::fs::remove_dir_all(work).ok();
    Ok(out)
}

/// The worldwide walk over every unit's chaining inputs: road values by OSM way id.
pub fn walk_all(units: &[UnitLinks]) -> Vec<(u64, RoadVal)> {
    let mut ways: Vec<(u64, f32)> = units.iter().flat_map(|u| u.ways.iter().copied()).collect();
    ways.par_sort_unstable_by_key(|w| w.0);
    ways.dedup_by_key(|w| w.0);
    let index = |id: u64| ways.binary_search_by_key(&id, |w| w.0).ok();
    let mut links: Vec<Link> = ways.iter().map(|&(id, len)| Link { id, kind: chain::KIND_ROAD, len, ..Default::default() }).collect();
    let mut partner = vec![[chain::NONE; 2]; links.len()];
    for u in units {
        for &(a, b) in &u.pairs {
            let (Some(ia), Some(ib)) = (index(a >> 1), index(b >> 1)) else { continue };
            partner[ia][(a & 1) as usize] = (ib as u32) << 1 | (b & 1) as u32;
            partner[ib][(b & 1) as usize] = (ia as u32) << 1 | (a & 1) as u32;
        }
    }
    // Ways that pair with nothing still get a road of their own.
    for l in &mut links {
        l.kind = chain::KIND_ROAD;
    }
    let vals = chain::walk(&links, &partner);
    ways.iter().map(|w| w.0).zip(vals).collect()
}

/// Stage list, in order.
pub const STAGES: &[&str] = &["copy", "filter", "sets", "outlines", "basemap", "cut3", "cut6", "roads"];

/// Run (or resume) the pass for `date` from `planet` (on the NAS).
pub fn run_pass(out: &mut Out, planet: &Path, date: &str, scratch: &Path, extract_bin: &Path, planetiler: &Path) -> Result<()> {
    std::fs::create_dir_all(scratch)?;
    let local_planet = scratch.join("planet.osm.pbf");
    let filtered = scratch.join("filtered.osm.pbf");
    if !done(scratch, "copy").exists() && !done(scratch, "filter").exists() {
        copy_resume(planet, &local_planet)?;
        mark(scratch, "copy")?;
    }
    if !done(scratch, "filter").exists() {
        let mut c = osmium();
        c.args(["tags-filter", "--overwrite", "-o"]).arg(&filtered).arg(&local_planet).args(FILTER_A);
        run(c, "osmium tags-filter (the pipeline's tags)")?;
        out.put_file(&format!("sources/osm/{date}/filtered"), "osm.pbf", &copy_keep(&filtered, scratch)?)?;
        out.save()?;
        std::fs::remove_file(&local_planet).ok();
        mark(scratch, "filter")?;
    }
    if !done(scratch, "sets").exists() {
        for (name, exprs) in SETS {
            let o = scratch.join(format!("set-{name}.osm.pbf"));
            let mut c = osmium();
            c.args(["tags-filter", "--overwrite", "-o"]).arg(&o).arg(&filtered).args(*exprs);
            run(c, &format!("osmium tags-filter (set {name})"))?;
            out.put_file(&format!("sources/osm/{date}/sets/{name}"), "osm.pbf", &o)?;
        }
        out.save()?;
        mark(scratch, "sets")?;
    }
    if !done(scratch, "outlines").exists() {
        // Administrative and ISO 3166 outlines from the outline set (crate::outlines).
        let set = scratch.join("set-outlines.osm.pbf");
        let set = if set.exists() { set } else { out.path(out.get(&format!("sources/osm/{date}/sets/outlines")).context("the outline set")?) };
        let file = scratch.join("outlines.sect");
        let s = crate::outlines::assemble(&set, &scratch.join("outlines-work"), &file)?;
        eprintln!("outlines: {} ({} points, {} simplified); by level {:?}", s.outlines, s.points, s.simplified_points, s.by_level);
        out.put_file(&format!("sources/osm/{date}/outlines"), "sect", &file)?;
        out.save()?;
        mark(scratch, "outlines")?;
    }
    if !done(scratch, "basemap").exists() {
        let b = scratch.join("basemap-input.osm.pbf");
        let mut c = osmium();
        c.args(["tags-filter", "--overwrite", "-o"]).arg(&b).arg(&filtered).args(FILTER_BASEMAP);
        run(c, "osmium tags-filter (basemap)")?;
        let pm = scratch.join("basemap.pmtiles");
        let downloads = out.root().join("sources/basemap");
        std::fs::create_dir_all(&downloads)?;
        let mut j = Command::new("/opt/homebrew/opt/openjdk@21/bin/java");
        j.args(["-Xmx24g", "-jar"]).arg(planetiler).args([
            "--download",
            "--storage=mmap",
            "--force",
            "--nodemap-type=sparsearray",
            "--only-layers=water,waterway,boundary,place,water_name,park",
            "--languages=en,fr",
            "--maxzoom=14",
        ]);
        j.arg(format!("--download-dir={}", downloads.display()));
        j.arg(format!("--tmpdir={}", scratch.join("planetiler-tmp").display()));
        j.arg(format!("--osm-path={}", b.display()));
        j.arg(format!("--output={}", pm.display()));
        run(j, "planetiler (basemap)")?;
        out.put_file(&format!("layers/basemap/world-{date}"), "pmtiles", &pm)?;
        out.save()?;
        std::fs::remove_file(&b).ok();
        std::fs::remove_dir_all(scratch.join("planetiler-tmp")).ok();
        mark(scratch, "basemap")?;
    }
    let p3 = scratch.join("pieces3");
    if !done(scratch, "cut3").exists() {
        cut(&filtered, &z3_tiles(), &p3, 32)?;
        mark(scratch, "cut3")?;
    }
    let mut pieces = Pieces { date: date.to_string(), ..Default::default() };
    let p6 = scratch.join("pieces6");
    if !done(scratch, "cut6").exists() {
        for q in z3_tiles() {
            let src = p3.join(format!("{}.osm.pbf", q.dash()));
            if is_empty_piece(&src) {
                continue;
            }
            let kids: Vec<Unit> = (0..8).flat_map(|i| (0..8).map(move |j| Unit { z: 6, x: q.x * 8 + i, y: q.y * 8 + j })).collect();
            let dir = p6.join(q.dash());
            if !done(&dir, "cut").exists() {
                cut(&src, &kids, &dir, 64)?;
                mark(&dir, "cut")?;
            }
            for u in kids {
                let f = dir.join(format!("{}.osm.pbf", u.dash()));
                if is_empty_piece(&f) {
                    std::fs::remove_file(&f).ok();
                    continue;
                }
                let logical = format!("sources/osm/{date}/pieces/{}", u.dash());
                if out.get(&logical).is_none() {
                    out.put_file(&logical, "osm.pbf", &copy_keep(&f, scratch)?)?;
                }
                pieces.pieces.insert(u.slash(), logical);
            }
            out.save()?;
        }
        out.put_bytes(&format!("sources/osm/{date}/pieces"), "json", &serde_json::to_vec_pretty(&pieces)?)?;
        out.save()?;
        std::fs::remove_dir_all(&p3).ok();
        mark(scratch, "cut6")?;
    } else {
        let name = out.get(&format!("sources/osm/{date}/pieces")).context("pieces list")?.to_string();
        pieces = serde_json::from_slice(&std::fs::read(out.path(&name))?)?;
    }
    if !done(scratch, "roads").exists() {
        // Every unit's chaining inputs, then the worldwide walk; values sliced by owner unit.
        let mut all: Vec<(Unit, UnitLinks)> = Vec::new();
        for (u, _) in &pieces.pieces {
            let unit = Unit::parse(u).context("unit")?;
            let local = p6.join(Unit { z: 3, x: unit.x >> 3, y: unit.y >> 3 }.dash()).join(format!("{}.osm.pbf", unit.dash()));
            let src = if local.exists() { local } else { out.path(out.get(&format!("sources/osm/{date}/pieces/{}", unit.dash())).context("piece")?) };
            let ul = unit_links(extract_bin, &src, unit, &scratch.join("extract-work"))?;
            eprintln!("roads: {} {} ways, {} pairs", u, ul.ways.len(), ul.pairs.len());
            all.push((unit, ul));
        }
        let lists: Vec<UnitLinks> = all.iter().map(|(_, l)| UnitLinks { ways: l.ways.clone(), pairs: l.pairs.clone() }).collect();
        let vals = walk_all(&lists);
        for (unit, ul) in &all {
            let mut recs: Vec<u8> = Vec::with_capacity(ul.ways.len() * 32);
            let mut ids: Vec<u64> = ul.ways.iter().map(|w| w.0).collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                let Ok(k) = vals.binary_search_by_key(&id, |v| v.0) else { continue };
                let v = vals[k].1;
                // (u64 way id, then the RoadRec layout)
                recs.extend_from_slice(&id.to_le_bytes());
                recs.extend_from_slice(bytemuck::bytes_of(&roadcore::packs::RoadRec { road: v.road, len: v.len, offset: v.offset, dir: v.dir, _pad: [0; 7] }));
            }
            out.put_bytes(&format!("sources/osm/{date}/roads/{}", unit.dash()), "bin", &recs)?;
        }
        out.save()?;
        mark(scratch, "roads")?;
    }
    if done(scratch, "roads").exists() {
        std::fs::remove_file(&filtered).ok();
        std::fs::remove_dir_all(&p6).ok();
    }
    // The pass is complete: its summary marks it so (the agent's `pass_done`).
    if out.get(&format!("sources/osm/{date}/pass")).is_none() {
        let summary = serde_json::json!({ "date": date, "units": pieces.pieces.len() });
        out.put_bytes(&format!("sources/osm/{date}/pass"), "json", &serde_json::to_vec_pretty(&summary)?)?;
        out.save()?;
    }
    Ok(())
}

/// Whether the pass from the planet of `date` is complete on the NAS (its `pass.<hash>.json`).
pub fn pass_done(root: &Path, date: &str) -> bool {
    std::fs::read_dir(root.join("sources/osm").join(date))
        .map(|rd| rd.flatten().any(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("pass.") && n.ends_with(".json"))))
        .unwrap_or(false)
}

/// The date of the newest complete pass on the NAS.
pub fn latest_pass(root: &Path) -> Option<String> {
    let dir = root.join("sources/osm");
    let mut dates: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() == 10 && n.as_bytes()[4] == b'-' && pass_done(root, n))
        .collect();
    dates.sort();
    dates.pop()
}

/// A copy of a scratch file for `put_file`, which consumes its input.
fn copy_keep(p: &Path, scratch: &Path) -> Result<PathBuf> {
    let d = scratch.join(format!("upload-{}", p.file_name().unwrap().to_string_lossy()));
    // A clone on APFS: instant, no extra space.
    let st = Command::new("cp").arg("-c").arg(p).arg(&d).status()?;
    if !st.success() {
        std::fs::copy(p, &d)?;
    }
    Ok(d)
}

/// Whether the NAS has a planet newer than `have` (a pass date), and its path and date.
pub fn newer_planet(root: &Path, have: Option<&str>) -> Result<Option<(PathBuf, String)>> {
    let dir = root.join("sources/osm");
    let mut dates: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() == 10 && n.as_bytes()[4] == b'-' && dir.join(n).join("planet.osm.pbf").exists())
        .collect();
    dates.sort();
    let Some(last) = dates.pop() else { return Ok(None) };
    if have.is_some_and(|h| h >= last.as_str()) {
        return Ok(None);
    }
    Ok(Some((dir.join(&last).join("planet.osm.pbf"), last)))
}

pub fn check_tools(extract_bin: &Path, planetiler: &Path) -> Result<()> {
    if !extract_bin.exists() {
        bail!("no extract binary at {}", extract_bin.display());
    }
    if !planetiler.exists() {
        bail!("no Planetiler at {}", planetiler.display());
    }
    Ok(())
}
