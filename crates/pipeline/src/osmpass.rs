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
    "nwr/highway", "nwr/railway", "r/route", "w/route=ferry", "nwr/public_transport", "nwr/amenity", "nwr/tourism", "nwr/historic",
    "nwr/heritage", "nwr/natural", "nwr/waterway", "nwr/water", "nwr/man_made", "nwr/leisure", "nwr/boundary",
    "nwr/building=train_station,church,cathedral,temple,shrine,mosque,synagogue,castle", "nwr/landuse=forest,reservoir,basin,salt_pond",
    "nwr/military", "nwr/place", "n/barrier", "nwr/aerialway", "nwr/mountain_pass", "nwr/wikidata",
];

/// The worldwide sets, each a filter of the filtered planet, with its filter's version: a changed
/// filter gets a new version, and so a new logical name (`set_name`), which the latest pass makes
/// from its kept filtered planet (`scenic-build pass-sets`); readers ask for the current one.
pub const SETS: &[(&str, u32, &[&str])] = &[
    ("rail", 1, &["nwr/railway", "nwr/public_transport", "r/route=train,subway,tram,light_rail,monorail,funicular,railway"]),
    ("ferries", 1, &["w/route=ferry", "r/route=ferry", "nwr/amenity=ferry_terminal"]),
    ("areas", 1, &["wr/boundary=national_park,protected_area,aboriginal_lands", "wr/leisure=nature_reserve"]),
    ("places", 1, &["n/place"]),
    ("outlines", 1, &["r/boundary=administrative", "r/ISO3166-1", "r/ISO3166-2"]),
    // The labels by importance (dem/labels.py): places, seas, bays and straits, water and parks.
    ("labels", 1, &["n/place", "n/natural=bay,strait", "wr/natural=water,bay,strait", "wr/boundary=national_park,protected_area", "wr/leisure=nature_reserve"]),
    // (The landmark candidates come from the units' pieces, extract `--candidates`, not a set.)
    // Summits, worldwide, for prominence and isolation: peaks and volcanoes, nodes and ways (2: ways
    // too, as the peaks among the candidates are).
    ("summits", 2, &["nw/natural=peak,volcano"]),
    // Hiking and foot routes with their member ways, for the routes' ends (pipeline::trailends).
    ("hikes", 1, &["r/route=hiking,foot"]),
    // What the basemap draws as water, and the coastline: the small islands and lakes it leaves
    // out zoomed out (pipeline::smallwater).
    ("water", 1, crate::smallwater::SET_FILTER),
    // Today's heritage filter (Makefile: named.osm.pbf), and World Heritage objects, for locating
    // register records.
    ("named", 1, &[
        "nwr/historic", "nwr/heritage", "nwr/tourism=museum,attraction,viewpoint", "nwr/man_made=lighthouse", "nwr/railway=station",
        "nwr/building=train_station,church,cathedral", "nwr/amenity=place_of_worship", "nwr/boundary=protected_area,national_park",
        "nwr/leisure=park", "nwr/military", "nwr/ref:whc", "nwr/heritage:operator=whc",
    ]),
];

/// A set's logical name in its current version (version 1 is the plain name).
pub fn set_name(date: &str, name: &str) -> String {
    match SETS.iter().find(|s| s.0 == name).map(|s| s.1).unwrap_or(1) {
        1 => format!("sources/osm/{date}/sets/{name}"),
        v => format!("sources/osm/{date}/sets/{name}-v{v}"),
    }
}

/// The sets a pass lacks in their current versions.
pub fn missing_sets(out: &Out, date: &str) -> Vec<&'static (&'static str, u32, &'static [&'static str])> {
    SETS.iter().filter(|s| out.get(&set_name(date, s.0)).is_none()).collect()
}

/// Makes the sets a pass lacks (`missing_sets`) from its filtered planet `src`, reporting each.
pub fn make_missing_sets(out: &mut Out, date: &str, src: &Path, scratch: &Path) -> Result<usize> {
    let missing = missing_sets(out, date);
    for (k, (name, _, exprs)) in missing.iter().enumerate() {
        crate::agent::jobs::stage(k as u64, missing.len() as u64, &format!("sets made ({name} now)"));
        let o = scratch.join(format!("set-{name}.osm.pbf"));
        let mut c = osmium();
        c.args(["tags-filter", "--overwrite", "-o"]).arg(&o).arg(src).args(*exprs);
        run_osmium(c, &format!("osmium tags-filter (set {name})"))?;
        out.put_file(&set_name(date, name), "osm.pbf", &o)?;
        out.save()?;
        std::fs::remove_file(&o).ok();
    }
    crate::agent::jobs::report(missing.len() as u64, missing.len() as u64, "sets made");
    Ok(missing.len())
}

/// The basemap's input (Planetiler's OpenMapTiles layers water, waterway, boundary, place,
/// water_name, park).
pub const FILTER_BASEMAP: &[&str] = &[
    "nwr/natural=water,bay,strait,wetland,glacier", "nwr/water", "nwr/waterway", "nwr/landuse=reservoir,basin,salt_pond",
    "nwr/leisure=nature_reserve,park", "nwr/boundary=administrative,national_park,protected_area,disputed", "nwr/place",
];

/// Room needed to copy the planet before filtering it: the copy, the filtered file it's deleted
/// after (allowed up to 75 % of the planet: 68 % measured on 2026-09-28's, 60.6 of 88.6 GB) and
/// 10 GB. Reading the planet straight from the NAS
/// instead is fragile: osmium reads it twice over an hour or more, and one I/O error on the SMB
/// mount ends the filter (seen on 2026-10-03), whereas the copy resumes after any interruption.
pub fn copy_room(planet_len: u64) -> u64 {
    planet_len + planet_len / 4 * 3 + (10 << 30)
}

/// Room the basemap's filter needs (its output, ~16 GB for the planet); short of it, the local
/// filtered file goes and the filter reads the NAS's copy.
const BASEMAP_ROOM: u64 = 50 << 30;

/// Room Planetiler needs for the basemap, from its input: it asked for 81 GB (54 GB of feature
/// storage) for the planet's 16.3 GB input, so six times the input. On 2026-10-03 it had 84 GB
/// beside the 60 GB local filtered file and ran out three times, hours in.
pub fn planetiler_room(input_len: u64) -> u64 {
    input_len * 6
}

/// Piece buffer around a unit, km.
pub const BUFFER_KM: f64 = 10.0;

pub fn osmium() -> Command {
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

/// Runs an osmium command (`run`), how far it is (its `--progress`) said as the stage's under way
/// (crate::agent::jobs::within).
fn run_osmium(mut c: Command, what: &str) -> Result<()> {
    eprintln!("$ {what}");
    c.arg("--progress");
    let st = crate::agent::jobs::run_watched(&mut c, crate::agent::jobs::within).with_context(|| format!("run {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// Runs Planetiler (`run`), how far it is (`planetiler_fraction` of each line of its log, which
/// goes on to this job's) said as the part's.
fn run_planetiler(mut c: Command, what: &str) -> Result<()> {
    eprintln!("$ {what}");
    let mut child = c.stdout(std::process::Stdio::piped()).spawn().with_context(|| format!("run {what}"))?;
    let mut far = 0.0f64;
    if let Some(o) = child.stdout.take() {
        crate::agent::jobs::each_line(o, |l| {
            eprintln!("{l}");
            if let Some(f) = planetiler_fraction(l).filter(|f| *f > far) {
                far = f;
                crate::agent::jobs::within(f);
            }
        });
    }
    let st = child.wait().with_context(|| format!("wait for {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// How far Planetiler is (0–1) by a line of its log: its phases weighted by their times on the
/// build Mac (2026-09-28's pass: its sources 4.5 min, the first pass over the planet 3, the second
/// 20, the sort 3, the tiles 15 of 46), the second pass and the tiles each through by the share
/// they say they're done.
pub fn planetiler_fraction(line: &str) -> Option<f64> {
    // (Its log is coloured: the escapes go.)
    let mut l = String::with_capacity(line.len());
    let mut esc = false;
    for c in line.chars() {
        match (esc, c) {
            (false, '\u{1b}') => esc = true,
            (true, c) if c.is_ascii_alphabetic() => esc = false,
            (true, _) => {}
            (false, c) => l.push(c),
        }
    }
    let phase = l.split("INF [").nth(1)?.split([']', ':']).next()?;
    let pct = |of: &str| -> Option<f64> {
        let inner = l.split(of).nth(1)?.split(']').next()?;
        inner.split_whitespace().find_map(|w| w.strip_suffix('%')?.parse::<f64>().ok()).map(|p| (p / 100.0).clamp(0.0, 1.0))
    };
    let (from, to, within) = match phase {
        "lake_centerlines" | "water_polygons" | "natural_earth" | "ne_lakes" => (0.0, 0.1, None),
        "osm_pass1" => (0.1, 0.16, None),
        "osm_pass2" => (0.16, 0.6, pct("blocks: ")),
        "boundaries" => (0.6, 0.61, None),
        "sort" => (0.61, 0.67, None),
        "archive" => (0.67, 1.0, pct("features: ")),
        _ => return None,
    };
    Some(from + within.unwrap_or(0.0) * (to - from))
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
    copy_resume_with(src, dst, &mut |_, _| {})
}

/// `copy_resume`, telling `progress` the bytes copied and the total as it goes (each 1 %).
pub fn copy_resume_with(src: &Path, dst: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
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
        let step = (total / 100).max(1);
        if (done + n as u64) / step != done / step {
            progress(done + n as u64, total);
        }
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
/// The pass's filtered planet on the NAS.
fn filtered_nas(out: &Out, date: &str) -> Result<PathBuf> {
    Ok(out.path(out.get(&format!("sources/osm/{date}/filtered")).context("the filtered planet on the NAS")?))
}

/// Cuts `input` (the data of tile `t` and its buffer) down to the z6 pieces below `t`, a quarter at
/// a time and depth first, calling `each` on every non-empty z6 piece. osmium keeps id sets per
/// output that span the whole id range (about 4 GB each on a planet-sized input, measured), so a
/// run makes four. Each piece goes as soon as its quarters are cut, so the tree's files on disk add
/// up to about one copy of the input (plus the buffers' overlap), however uneven the data.
/// Resumable: `<tile>.cut` marks a tile whose quarters were cut, `<tile>.done` one whose whole
/// subtree is done.
fn cut_tree(input: &Path, t: Unit, consume: bool, work: &Path, each: &mut dyn FnMut(Unit, &Path) -> Result<()>) -> Result<()> {
    let kids: Vec<Unit> = (0..2u32).flat_map(|i| (0..2u32).map(move |j| Unit { z: t.z + 1, x: t.x * 2 + i, y: t.y * 2 + j })).collect();
    let dir = work.join(format!("z{}", t.z + 1));
    let marker = work.join(format!("{}.cut", t.dash()));
    if !marker.exists() {
        cut(input, &kids, &dir, 4)?;
        std::fs::write(&marker, b"")?;
    }
    // Its quarters are on disk: the input goes (`consume`: a piece of this tree, not the root's).
    if consume {
        std::fs::remove_file(input).ok();
    }
    for k in kids {
        let done = work.join(format!("{}.done", k.dash()));
        if done.exists() {
            continue;
        }
        let f = dir.join(format!("{}.osm.pbf", k.dash()));
        let cut_already = work.join(format!("{}.cut", k.dash())).exists();
        if f.exists() && is_empty_piece(&f) {
            std::fs::remove_file(&f)?;
        } else if k.z == 6 {
            // Gone without its mark: done just before an interruption (uploaded, links kept).
            if f.exists() {
                each(k, &f)?;
                std::fs::remove_file(&f)?;
            }
        } else if f.exists() || cut_already {
            cut_tree(&f, k, true, work, each)?;
        }
        std::fs::write(&done, b"")?;
    }
    Ok(())
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
#[derive(Default, Debug, PartialEq)]
pub struct UnitLinks {
    pub ways: Vec<(u64, f32)>,
    /// (way a << 1 | end a, way b << 1 | end b) by OSM id.
    pub pairs: Vec<(u64, u64)>,
}

const LINKS_MAGIC: &[u8; 8] = b"RDLINK01";

impl UnitLinks {
    /// Kept between the cut and the walk: the counts, then 16 bytes per way (id, length as f64
    /// bits) and per pair.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut b = Vec::with_capacity(24 + 16 * (self.ways.len() + self.pairs.len()));
        b.extend_from_slice(LINKS_MAGIC);
        b.extend_from_slice(&(self.ways.len() as u64).to_le_bytes());
        b.extend_from_slice(&(self.pairs.len() as u64).to_le_bytes());
        for &(id, len) in &self.ways {
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&(len as f64).to_bits().to_le_bytes());
        }
        for &(a, c) in &self.pairs {
            b.extend_from_slice(&a.to_le_bytes());
            b.extend_from_slice(&c.to_le_bytes());
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &b)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<UnitLinks> {
        let b = std::fs::read(path)?;
        anyhow::ensure!(b.len() >= 24 && &b[..8] == LINKS_MAGIC, "{}: not a links file", path.display());
        let u = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let (nw, np) = (u(8) as usize, u(16) as usize);
        anyhow::ensure!(b.len() == 24 + 16 * (nw + np), "{}: truncated", path.display());
        let ways = (0..nw).map(|k| (u(24 + 16 * k), f64::from_bits(u(32 + 16 * k)) as f32)).collect();
        let at = 24 + 16 * nw;
        let pairs = (0..np).map(|k| (u(at + 16 * k), u(at + 8 + 16 * k))).collect();
        Ok(UnitLinks { ways, pairs })
    }
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
/// The worldwide walk over every unit's chaining inputs: the ways (OSM id, length), sorted by id,
/// and each one's road value in the same order. Each unit's pairs are dropped once used: at planet
/// scale (some 250 M ways) the walk then stays near 20 GB.
pub fn walk_all(units: &mut [UnitLinks]) -> (Vec<(u64, f32)>, Vec<RoadVal>) {
    let mut ways: Vec<(u64, f32)> = units.iter().flat_map(|u| u.ways.iter().copied()).collect();
    ways.par_sort_unstable_by_key(|w| w.0);
    ways.dedup_by_key(|w| w.0);
    let index = |id: u64| ways.binary_search_by_key(&id, |w| w.0).ok();
    let mut partner = vec![[chain::NONE; 2]; ways.len()];
    for u in units.iter_mut() {
        for &(a, b) in &std::mem::take(&mut u.pairs) {
            let (Some(ia), Some(ib)) = (index(a >> 1), index(b >> 1)) else { continue };
            partner[ia][(a & 1) as usize] = (ib as u32) << 1 | (b & 1) as u32;
            partner[ib][(b & 1) as usize] = (ia as u32) << 1 | (a & 1) as u32;
        }
    }
    // Every way gets a road, of its own when it pairs with nothing.
    let vals = chain::walk_by(ways.len(), |i| ways[i].0, |i| ways[i].1, |_| true, &partner);
    (ways, vals)
}

/// Stage list, in order.
pub const STAGES: &[&str] = &["copy", "filter", "sets", "outlines", "basemap", "cut", "roads"];

/// The pass's parts, for the status (crate::agent::jobs::part): each says how far it is.
const PARTS: [&str; 10] = [
    "Copying the planet here from the NAS",
    "Filtering the planet to the build's tags (osmium)",
    "Uploading the filtered planet to the NAS",
    "Making the worldwide sets (osmium)",
    "Assembling the outlines",
    "Filtering the basemap's input (osmium)",
    "Drawing the worldwide basemap (Planetiler)",
    "Cutting the planet into areas",
    "Reading the areas' road links",
    "Walking the world's roads and writing their values",
];

/// Part `i` of the pass begins (`PARTS`), its progress one item's (`one`) until it says its own.
fn part(i: usize, one: &str) {
    crate::agent::jobs::part(i, &PARTS);
    crate::agent::jobs::stage(0, 1, one);
}

/// Run (or resume) the pass for `date` from `planet` (on the NAS).
pub fn run_pass(out: &mut Out, planet: &Path, date: &str, scratch: &Path, extract_bin: &Path, planetiler: &Path) -> Result<()> {
    use crate::agent::jobs::{report, stage};
    std::fs::create_dir_all(scratch)?;
    let local_planet = scratch.join("planet.osm.pbf");
    let filtered = scratch.join("filtered.osm.pbf");
    // The planet is read once (by the filter): copied first when there's room for it (resumable),
    // else read from the NAS as it streams (an interruption then repeats the filter).
    let planet_len = std::fs::metadata(planet).map(|m| m.len()).unwrap_or(u64::MAX);
    let room = crate::agent::cond::free_bytes(scratch).unwrap_or(0);
    let copy_first = done(scratch, "copy").exists() || room > copy_room(planet_len);
    if copy_first && !done(scratch, "copy").exists() && !done(scratch, "filter").exists() {
        part(0, "the planet copied");
        copy_resume_with(planet, &local_planet, &mut |d, t| report(d >> 20, t >> 20, "MB of the planet copied"))?;
        mark(scratch, "copy")?;
    }
    if !done(scratch, "filter").exists() {
        part(1, "the planet filtered (osmium)");
        let src = if copy_first { local_planet.clone() } else { planet.to_path_buf() };
        let mut c = osmium();
        c.args(["tags-filter", "--overwrite", "-o"]).arg(&filtered).arg(&src).args(FILTER_A);
        run_osmium(c, "osmium tags-filter (the pipeline's tags)")?;
        part(2, "the filtered planet uploaded");
        out.put_file_with(&format!("sources/osm/{date}/filtered"), "osm.pbf", &copy_keep(&filtered, scratch)?, &|d, t| report(d >> 20, t >> 20, "MB of the filtered planet moved (read, sent, checked)"))?;
        out.save()?;
        std::fs::remove_file(&local_planet).ok();
        mark(scratch, "filter")?;
    }
    if !done(scratch, "sets").exists() {
        part(3, "sets made");
        for (k, (name, _, exprs)) in SETS.iter().enumerate() {
            stage(k as u64, SETS.len() as u64, &format!("sets made ({name} now)"));
            let o = scratch.join(format!("set-{name}.osm.pbf"));
            let mut c = osmium();
            c.args(["tags-filter", "--overwrite", "-o"]).arg(&o).arg(&filtered).args(*exprs);
            run_osmium(c, &format!("osmium tags-filter (set {name})"))?;
            // (Kept: the outlines stage below reads its set from here.)
            out.put_file(&set_name(date, name), "osm.pbf", &o)?;
        }
        report(SETS.len() as u64, SETS.len() as u64, "sets made");
        out.save()?;
        mark(scratch, "sets")?;
    }
    // Sets added or changed since this pass began (a pass resumed by a newer app): made now, from
    // the same filtered file (the local copy while it's there).
    if !done(scratch, "sets-added").exists() {
        if !missing_sets(out, date).is_empty() {
            part(3, "sets made");
            let src = if filtered.exists() { filtered.clone() } else { filtered_nas(out, date)? };
            make_missing_sets(out, date, &src, scratch)?;
        }
        mark(scratch, "sets-added")?;
    }
    if !done(scratch, "outlines").exists() {
        part(4, "the outlines assembled");
        // Administrative and ISO 3166 outlines from the outline set (crate::outlines).
        let set = scratch.join("set-outlines.osm.pbf");
        let set = if set.exists() { set } else { out.path(out.get(&set_name(date, "outlines")).context("the outline set")?) };
        let file = scratch.join("outlines.sect");
        let s = crate::outlines::assemble(&set, &scratch.join("outlines-work"), &file)?;
        eprintln!("outlines: {} ({} points, {} simplified); by level {:?}", s.outlines, s.points, s.simplified_points, s.by_level);
        out.put_file(&format!("sources/osm/{date}/outlines"), "sect", &file)?;
        out.save()?;
        mark(scratch, "outlines")?;
    }
    // From here the filtered file is read twice more (the basemap's filter, the first cut): from
    // the local copy while there's room, else from the NAS's.
    let free = |p: &Path| crate::agent::cond::free_bytes(p).unwrap_or(0);
    if !done(scratch, "basemap").exists() {
        check_planetiler(planetiler)?;
        part(5, "the basemap's input filtered (osmium)");
        if filtered.exists() && free(scratch) < BASEMAP_ROOM {
            eprintln!("basemap: {} GB free; reading the filtered planet from the NAS", free(scratch) >> 30);
            std::fs::remove_file(&filtered)?;
        }
        let b = scratch.join("basemap-input.osm.pbf");
        // (Kept until the basemap is made: a failed Planetiler run doesn't filter the planet again.)
        if !(b.exists() && done(scratch, "basemap-input").exists()) {
            let src = if filtered.exists() { filtered.clone() } else { filtered_nas(out, date)? };
            let mut c = osmium();
            c.args(["tags-filter", "--overwrite", "-o"]).arg(&b).arg(&src).args(FILTER_BASEMAP);
            run_osmium(c, "osmium tags-filter (basemap)")?;
            mark(scratch, "basemap-input")?;
        }
        part(6, "the basemap drawn (Planetiler)");
        // Planetiler's room: the local filtered file goes first (the cut reads the NAS's copy then).
        let need = planetiler_room(std::fs::metadata(&b)?.len());
        if free(scratch) < need && filtered.exists() {
            eprintln!("basemap: {} GB free, Planetiler needs ~{} GB; the cut will read the filtered planet from the NAS", free(scratch) >> 30, need >> 30);
            std::fs::remove_file(&filtered)?;
        }
        // (A run that failed may have left its temporary files.)
        std::fs::remove_dir_all(scratch.join("planetiler-tmp")).ok();
        ensure!(free(scratch) >= need, "basemap: Planetiler needs ~{} GB free on this Mac, {} GB are", need >> 30, free(scratch) >> 30);
        let pm = scratch.join("basemap.pmtiles");
        let downloads = out.root().join("sources/basemap");
        std::fs::create_dir_all(&downloads)?;
        // Run in the scratch folder, its files named relative to it: Planetiler reads `--output` as
        // a URI, which a space in the path ("Application Support") breaks.
        let mut j = Command::new("/opt/homebrew/opt/openjdk@21/bin/java");
        j.current_dir(scratch);
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
        j.arg("--tmpdir=planetiler-tmp");
        j.arg("--osm-path=basemap-input.osm.pbf");
        j.arg("--output=basemap.pmtiles");
        run_planetiler(j, "planetiler (basemap)")?;
        out.put_file(&format!("layers/basemap/world-{date}"), "pmtiles", &pm)?;
        out.save()?;
        std::fs::remove_file(&b).ok();
        std::fs::remove_file(done(scratch, "basemap-input")).ok();
        std::fs::remove_dir_all(scratch.join("planetiler-tmp")).ok();
        mark(scratch, "basemap")?;
    }
    let tree = scratch.join("cut");
    let links = scratch.join("links");
    if !done(scratch, "cut").exists() {
        // The first cut writes about the filtered file again (its four quarters); below it each
        // piece goes as soon as it's cut, so the tree stays near that size. The local filtered
        // file is kept for the first cut only when there's room for both, and goes after it.
        let flen = std::fs::metadata(&filtered).or_else(|_| std::fs::metadata(filtered_nas(out, date)?).map_err(anyhow::Error::from))?.len();
        if filtered.exists() && !tree.join("0-0-0.cut").exists() && free(scratch) < flen / 8 * 10 + (20 << 30) {
            eprintln!("cut: {} GB free; reading the filtered planet from the NAS", free(scratch) >> 30);
            std::fs::remove_file(&filtered)?;
        }
        let local = filtered.exists();
        let src = if local { filtered.clone() } else { filtered_nas(out, date)? };
        let work = scratch.join("extract-work");
        // Progress by data: the pieces' sizes against the filtered planet's (their buffers overlap a
        // little, so it's capped just short of the end), the pieces uploaded before a restart too.
        let total_mb = std::fs::metadata(&src)?.len() >> 20;
        let prefix = format!("sources/osm/{date}/pieces/");
        let mut done_mb: u64 = out.manifest.range(prefix.clone()..).take_while(|(l, _)| l.starts_with(&prefix)).filter_map(|(_, c)| std::fs::metadata(out.path(c)).ok()).map(|m| m.len() >> 20).sum();
        part(7, "the planet cut into areas");
        let cut_report = |mb: u64| report(mb.min(total_mb.saturating_sub(1)), total_mb, "MB cut into areas");
        cut_report(done_mb);
        cut_tree(&src, Unit { z: 0, x: 0, y: 0 }, local, &tree, &mut |u, f| {
            let logical = format!("sources/osm/{date}/pieces/{}", u.dash());
            if out.get(&logical).is_none() {
                let mb = std::fs::metadata(f).map(|m| m.len() >> 20).unwrap_or(0);
                out.put_file(&logical, "osm.pbf", &copy_keep(f, scratch)?)?;
                out.save()?;
                done_mb += mb;
                cut_report(done_mb);
            }
            let lf = links.join(format!("{}.bin", u.dash()));
            if !lf.exists() {
                let ul = unit_links(extract_bin, f, u, &work)?;
                eprintln!("roads: {} {} ways, {} pairs", u.slash(), ul.ways.len(), ul.pairs.len());
                ul.save(&lf)?;
            }
            Ok(())
        })?;
        let mut pieces = Pieces { date: date.to_string(), ..Default::default() };
        let prefix = format!("sources/osm/{date}/pieces/");
        for k in out.manifest.keys().filter(|k| k.starts_with(&prefix)) {
            if let Some(u) = Unit::parse(&k[prefix.len()..]) {
                pieces.pieces.insert(u.slash(), k.clone());
            }
        }
        out.put_bytes(&format!("sources/osm/{date}/pieces"), "json", &serde_json::to_vec_pretty(&pieces)?)?;
        out.save()?;
        // Marked first: the clean-up below is safe to repeat, the cut isn't cheap to.
        mark(scratch, "cut")?;
    }
    std::fs::remove_dir_all(&tree).ok();
    std::fs::remove_file(&filtered).ok();
    let name = out.get(&format!("sources/osm/{date}/pieces")).context("pieces list")?.to_string();
    let pieces: Pieces = serde_json::from_slice(&std::fs::read(out.path(&name))?)?;
    if !done(scratch, "roads").exists() {
        // Every unit's chaining inputs (kept by the cut, else worked out from its piece), then the
        // worldwide walk; values sliced by owner unit.
        let mut all: Vec<(Unit, UnitLinks)> = Vec::new();
        let n = pieces.pieces.len() as u64;
        part(8, "areas' road links read");
        for (k, (u, _)) in pieces.pieces.iter().enumerate() {
            report(k as u64, n, "areas' road links read");
            let unit = Unit::parse(u).context("unit")?;
            let lf = links.join(format!("{}.bin", unit.dash()));
            let ul = match UnitLinks::load(&lf) {
                Ok(ul) => ul,
                Err(_) => {
                    let src = out.path(out.get(&format!("sources/osm/{date}/pieces/{}", unit.dash())).context("piece")?);
                    unit_links(extract_bin, &src, unit, &scratch.join("extract-work"))?
                }
            };
            all.push((unit, ul));
        }
        report(n, n, "areas' road links read");
        let (units, mut lists): (Vec<Unit>, Vec<UnitLinks>) = all.into_iter().unzip();
        part(9, "the roads walked");
        let (ways, vals) = walk_all(&mut lists);
        for (k, (unit, ul)) in units.iter().zip(&lists).enumerate() {
            report(k as u64, n, "areas' road values written");
            let mut recs: Vec<u8> = Vec::with_capacity(ul.ways.len() * 32);
            let mut ids: Vec<u64> = ul.ways.iter().map(|w| w.0).collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                let Ok(k) = ways.binary_search_by_key(&id, |w| w.0) else { continue };
                let v = vals[k];
                // (u64 way id, then the RoadRec layout)
                recs.extend_from_slice(&id.to_le_bytes());
                recs.extend_from_slice(bytemuck::bytes_of(&roadcore::packs::RoadRec { road: v.road, len: v.len, offset: v.offset, dir: v.dir, _pad: [0; 7] }));
            }
            out.put_bytes(&format!("sources/osm/{date}/roads/{}", unit.dash()), "bin", &recs)?;
        }
        report(n, n, "areas' road values written");
        out.save()?;
        mark(scratch, "roads")?;
    }
    if done(scratch, "roads").exists() {
        std::fs::remove_dir_all(&links).ok();
    }
    // The pass is complete: its summary marks it so (the agent's `pass_done`).
    if out.get(&format!("sources/osm/{date}/pass")).is_none() {
        let summary = serde_json::json!({ "date": date, "units": pieces.pieces.len() });
        out.put_bytes(&format!("sources/osm/{date}/pass"), "json", &serde_json::to_vec_pretty(&summary)?)?;
        out.save()?;
    }
    let n = retire_older(out, date);
    if n > 0 {
        out.save()?;
        eprintln!("osm-pass: {n} entries of older passes retired");
    }
    Ok(())
}

/// A pass date (YYYY-MM-DD).
pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// Removes the manifest's entries of passes older than `date` (their planet's pieces, sets, road
/// values and outlines; the summits, route ends, items and heritage made from them) once `date`'s
/// pass is complete: every job reads the newest pass, so nothing reads them again. (Their files
/// go with GC: its sweep of retired passes' sources, agent::gc.) How many went.
pub fn retire_older(out: &mut Out, date: &str) -> usize {
    let old: Vec<String> = out
        .manifest
        .keys()
        .filter(|l| {
            let s: Vec<&str> = l.split('/').collect();
            match s.as_slice() {
                ["sources", "osm", d, _, ..] | ["sources", "items", d, ..] | ["work", "heritage", d, ..] | ["work", "summits", d] | ["work", "trailends", d] => is_date(d) && *d < date,
                _ => false,
            }
        })
        .cloned()
        .collect();
    for l in &old {
        out.remove(l);
    }
    old.len()
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
        store::sys::copy_data(p, &d)?;
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

/// Puts into the pass's pieces the ferry ways its filtered planet lacks. The 2026-09-28 pass's was
/// filtered without `w/route=ferry`: it has a ferry way only where another of the filter's tags
/// kept it (a route relation's member, a `wikidata` tag, …), so its pieces lack the rest
/// (Kobe–Miyazaki, Tanger–Tarifa, …). Its ferries set is the planet's own, with them. Each piece
/// gets the set's ferry ways it meets as the cut kept ways (its tile and buffer, the smart
/// strategy: whole, with their nodes) and lacks, merged in (an object both have, at the one
/// version the planet has, is written once).
/// A piece that lacks none stays as it is, so only the units whose pieces change are built again,
/// and a run after it changes nothing. Only the pieces of the units `only` takes. The pieces changed
/// and the ways they gained. The records are saved once, at the end: a piece changed goes stale the
/// roads' reach, and with it the units' planning on both Macs, so saving each as it changed would
/// hold the units up for the whole run (a run stopped part way saves none, and is run again).
pub fn patch_ferries(out: &mut Out, date: &str, scratch: &Path, only: &dyn Fn(Unit) -> bool) -> Result<(usize, usize)> {
    let set = out.path(out.get(&set_name(date, "ferries")).context("the pass's ferries set")?);
    let work = scratch.join("patch-ferries");
    std::fs::create_dir_all(&work)?;
    let local_set = work.join("ferries.osm.pbf");
    store::sys::copy_data(&set, &local_set)?;
    let prefix = format!("sources/osm/{date}/pieces/");
    let pieces: Vec<(Unit, String, String)> = out
        .manifest
        .range(prefix.clone()..)
        .take_while(|(l, _)| l.starts_with(&prefix))
        .filter_map(|(l, c)| Some((Unit::parse(&l[prefix.len()..])?, l.clone(), c.clone())))
        .filter(|(u, _, _)| only(*u))
        .collect();
    let (mut changed, mut gained) = (0, 0);
    for (k, (u, logical, content)) in pieces.iter().enumerate() {
        crate::agent::jobs::report(k as u64, pieces.len() as u64, "pieces checked for ferries");
        let near = work.join("near.osm.pbf");
        let b = grow(tile_bounds(u.z, u.x, u.y), BUFFER_KM);
        let d = |v: i32| v as f64 * 1e-7;
        let mut c = osmium();
        c.args(["extract", "--no-progress", "--strategy", "smart", "-S", "types=multipolygon", "--overwrite", "-b"]).arg(format!("{},{},{},{}", d(b[0]), d(b[1]), d(b[2]), d(b[3]))).arg(&local_set).arg("-o").arg(&near);
        quiet(c, "osmium extract (the ferries near a piece)")?;
        let ids = way_ids(&near, None)?;
        if ids.is_empty() {
            continue;
        }
        // (Read where it is: copied only when it lacks some.)
        let lacks = ids.len() - way_ids(&out.path(content), Some(&ids))?.len();
        if lacks == 0 {
            continue;
        }
        let piece = work.join("piece.osm.pbf");
        store::sys::copy_data(out.path(content), &piece)?;
        let merged = work.join(format!("{}.osm.pbf", u.dash()));
        let mut c = osmium();
        c.args(["merge", "--no-progress", "--overwrite", "--output-header", "sorting=Type_then_ID", "-o"]).arg(&merged).arg(&piece).arg(&near);
        quiet(c, "osmium merge (a piece and its ferries)")?;
        out.put_file(logical, "osm.pbf", &merged)?;
        eprintln!("ferries: {} lacked {lacks} of the {} ferry ways it meets", u.slash(), ids.len());
        (changed, gained) = (changed + 1, gained + lacks);
    }
    out.save()?;
    std::fs::remove_dir_all(&work).ok();
    Ok((changed, gained))
}

/// The ids of the ways in an OSM file, or of those among `among` (osmium getid).
fn way_ids(file: &Path, among: Option<&std::collections::BTreeSet<i64>>) -> Result<std::collections::BTreeSet<i64>> {
    let mut c = osmium();
    match among {
        None => {
            c.args(["cat", "--no-progress", "-t", "way", "-f", "opl"]).arg(file);
        }
        Some(ids) => {
            let list = std::env::temp_dir().join(format!("scenic-way-ids-{}", std::process::id()));
            std::fs::write(&list, ids.iter().map(|i| format!("w{i}\n")).collect::<String>())?;
            c.args(["getid", "--no-progress", "-f", "opl", "-i"]).arg(&list).arg(file);
        }
    }
    let o = c.output().context("run osmium");
    if among.is_some() {
        std::fs::remove_file(std::env::temp_dir().join(format!("scenic-way-ids-{}", std::process::id()))).ok();
    }
    let o = o?;
    // (getid exits 1, saying nothing, when some of the ids aren't there: the answer, not a failure;
    // a file it can't read says why.)
    let quiet_miss = among.is_some() && o.status.code() == Some(1) && o.stderr.iter().all(u8::is_ascii_whitespace);
    ensure!(o.status.success() || quiet_miss, "osmium on {} failed: {}: {}", file.display(), o.status, String::from_utf8_lossy(&o.stderr).trim());
    Ok(String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.strip_prefix('w')?.split(' ').next()?.parse().ok()).collect())
}

/// Runs a command whose output isn't wanted, failing with its name.
fn quiet(mut c: Command, what: &str) -> Result<()> {
    let o = c.output().with_context(|| format!("run {what}"))?;
    ensure!(o.status.success(), "{what} failed: {}: {}", o.status, String::from_utf8_lossy(&o.stderr).trim());
    Ok(())
}

/// The `version=` of a Planetiler jar's `buildinfo.properties`.
fn buildinfo_version(props: &str) -> Option<&str> {
    props.lines().find_map(|l| l.trim().strip_prefix("version=")).map(str::trim)
}

/// Stops on a Planetiler jar other than the one the small islands and lakes' rule is read from
/// (pipeline::smallwater::PLANETILER_VERSION: its own `buildinfo.properties` says which it is), so
/// a new one's basemap isn't drawn until the rule is checked against it and pinned again.
pub fn check_planetiler(jar: &Path) -> Result<()> {
    let o = Command::new("/usr/bin/unzip").arg("-p").arg(jar).arg("buildinfo.properties").output().with_context(|| format!("read {}", jar.display()))?;
    ensure!(o.status.success(), "no buildinfo.properties in {}", jar.display());
    let props = String::from_utf8_lossy(&o.stdout);
    let v = buildinfo_version(&props).with_context(|| format!("no version in {}'s buildinfo.properties", jar.display()))?;
    let pinned = crate::smallwater::PLANETILER_VERSION;
    ensure!(v == pinned, "{} is Planetiler {v}, but the small islands and lakes' rule is Planetiler {pinned}'s (pipeline::smallwater, docs/plan.md §6): check the rule against {v}'s source and tiles, then pin it", jar.display());
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planetilers_version_is_read_from_its_buildinfo() {
        let props = "githash=0e5588c4a6e8c29a270a33afe8df62027d889604\ntimestamp=1774708536815\nversion=0.10.2\n";
        assert_eq!(buildinfo_version(props), Some("0.10.2"));
        assert_eq!(buildinfo_version(props), Some(crate::smallwater::PLANETILER_VERSION));
        assert_eq!(buildinfo_version("githash=x\n"), None);
    }

    #[test]
    fn planetilers_log_says_how_far_it_is() {
        let l = "\u{1b}[m\u{1b}[0m0:07:29 INF [osm_pass2] -  nodes: \u{1b}[32m[ 228M  11% 9.9M/s ]\u{1b}[0m 31G   ways: [    0   0%    0/s ] blocks: [  28k  10% 1.2k/s ]";
        assert!((planetiler_fraction(l).unwrap() - (0.16 + 0.1 * 0.44)).abs() < 1e-9);
        let a = "0:30:43 INF [archive] -  features: \u{1b}[32m[ 3.5M  50% 359k/s ]\u{1b}[0m 71G   tiles: [ 6.6k  668/s ] 42M";
        assert!((planetiler_fraction(a).unwrap() - (0.67 + 0.5 * 0.33)).abs() < 1e-9);
        assert_eq!(planetiler_fraction("0:00:34 INF [lake_centerlines] - Starting..."), Some(0.0));
        assert_eq!(planetiler_fraction("0:04:26 INF [osm_pass1:process] - "), Some(0.1));
        assert_eq!(planetiler_fraction("some other line"), None);
    }

    /// An OSM file from OPL lines.
    fn osm(dir: &Path, name: &str, opl: &str) -> PathBuf {
        let (src, dest) = (dir.join(format!("{name}.opl")), dir.join(format!("{name}.osm.pbf")));
        std::fs::write(&src, opl).unwrap();
        let st = osmium().args(["cat", "--no-progress", "--overwrite", "-o"]).arg(&dest).arg(&src).status().unwrap();
        assert!(st.success());
        dest
    }

    #[test]
    fn pieces_gain_the_ferries_they_lack() {
        let d = tempfile::tempdir().unwrap();
        let mut out = Out::open(d.path(), &d.path().join("scratch")).unwrap();
        // Unit 6/32/21 (lon 0 to 5.6, lat 48.9 to 52.5): a road and a ferry the filter kept (a route
        // relation's member) in its piece; unit 6/40/20's piece has a road only.
        let shared = "n1 v1 x1.0 y50.0\nn2 v1 x1.1 y50.0\nn3 v1 x1.2 y50.1\nn4 v1 x1.5 y50.5\n";
        let piece = osm(d.path(), "piece", &format!("{shared}w10 v1 Thighway=primary Nn1,n2\nw20 v1 Troute=ferry,motor_vehicle=yes Nn2,n3\n"));
        let other = osm(d.path(), "other", "n9 v1 x45.0 y45.0\nn8 v1 x45.1 y45.0\nw90 v1 Thighway=primary Nn9,n8\n");
        // The ferries set: that ferry, and a standalone one from the piece's coast far out to sea.
        let set = osm(d.path(), "set", &format!("{shared}n5 v1 x-3.0 y52.0\nw20 v1 Troute=ferry,motor_vehicle=yes Nn2,n3\nw30 v1 Troute=ferry,motor_vehicle=yes Nn3,n4,n5\n"));
        out.put_file("sources/osm/2026-09-28/pieces/6-32-21", "osm.pbf", &piece).unwrap();
        out.put_file("sources/osm/2026-09-28/pieces/6-40-20", "osm.pbf", &other).unwrap();
        out.put_file("sources/osm/2026-09-28/sets/ferries", "osm.pbf", &set).unwrap();
        let (before, untouched) = (out.get("sources/osm/2026-09-28/pieces/6-32-21").unwrap().to_string(), out.get("sources/osm/2026-09-28/pieces/6-40-20").unwrap().to_string());
        assert_eq!(patch_ferries(&mut out, "2026-09-28", &d.path().join("scratch"), &|_| true).unwrap(), (1, 1));
        // The piece gained the standalone ferry, whole (its node at sea too), and kept the rest once.
        let after = out.path(out.get("sources/osm/2026-09-28/pieces/6-32-21").unwrap());
        assert_ne!(out.get("sources/osm/2026-09-28/pieces/6-32-21").unwrap(), before);
        assert_eq!(way_ids(&after, None).unwrap().into_iter().collect::<Vec<_>>(), [10, 20, 30]);
        let opl = String::from_utf8(osmium().args(["cat", "--no-progress", "-f", "opl"]).arg(&after).output().unwrap().stdout).unwrap();
        assert_eq!(opl.lines().filter(|l| l.starts_with('n')).count(), 5);
        // A piece with no ferries near it stays as it was; a second run changes nothing.
        assert_eq!(out.get("sources/osm/2026-09-28/pieces/6-40-20").unwrap(), untouched);
        assert_eq!(patch_ferries(&mut out, "2026-09-28", &d.path().join("scratch"), &|_| true).unwrap(), (0, 0));
        // Units left out stay as they are.
        assert_eq!(patch_ferries(&mut out, "2026-09-28", &d.path().join("scratch"), &|u| u.x == 40).unwrap(), (0, 0));
    }

    #[test]
    fn older_passes_retire() {
        let d = tempfile::tempdir().unwrap();
        let mut out = Out::open(d.path(), &d.path().join("scratch")).unwrap();
        for l in [
            "sources/osm/2026-03-28/pieces/6-1-2",
            "sources/osm/2026-03-28/sets/summits",
            "sources/osm/2026-03-28/outlines",
            "sources/osm/2026-09-28/pieces/6-1-2",
            "sources/osm/2026-09-28/outlines",
            "work/summits/2026-03-28",
            "work/summits/2026-09-28",
            "work/trailends/2026-03-28",
            "sources/items/2026-03-28/facts",
            "work/heritage/2026-03-28/layer-heritage",
            "work/pois/6-1-2",
            "base/6-1-2",
            "global/roads/6-1-2",
        ] {
            out.put_bytes(l, "json", b"{}").unwrap();
        }
        assert_eq!(retire_older(&mut out, "2026-09-28"), 7);
        let left: Vec<&str> = out.manifest.keys().map(String::as_str).collect();
        assert_eq!(left, ["base/6-1-2", "global/roads/6-1-2", "sources/osm/2026-09-28/outlines", "sources/osm/2026-09-28/pieces/6-1-2", "work/pois/6-1-2", "work/summits/2026-09-28"]);
        assert!(is_date("2026-09-28") && !is_date("2026-9-28") && !is_date("6-1-2"));
    }

    #[test]
    fn links_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let ul = UnitLinks { ways: vec![(1, 12.5), (u64::MAX - 3, 0.25)], pairs: vec![(2, 3), (7 << 1 | 1, 9 << 1)] };
        let p = d.path().join("l/6-1-2.bin");
        ul.save(&p).unwrap();
        assert_eq!(UnitLinks::load(&p).unwrap(), ul);
        std::fs::write(&p, &std::fs::read(&p).unwrap()[..30]).unwrap();
        assert!(UnitLinks::load(&p).is_err());
    }
}
