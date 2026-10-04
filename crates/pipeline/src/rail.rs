//! Rail service (docs/plan.md §6, Rail service): trains a day on every rail way of the coverage,
//! from the operators' timetables (GTFS) and the MTR's hand-researched lines, as `global/railfreq`.
//!
//! Two jobs. `rail-feeds` finds the feeds for the coverage (`dem/railfeeds.py`: the Mobility
//! Database catalogue's feeds of the countries the coverage is in, by their boxes, the ones that run
//! rail, and national operators' own) and fetches each once into the rail sources. `rail` counts
//! their trains (`dem/railgtfs.py`), marks the stops beyond the coverage, and matches the stop pairs
//! onto the rail ways of the pass's rail set that touch the coverage (`extract`, then `railfreq`).
//!
//! The rail sources (`sources/rail/`, content-named, in the build manifest; docs/formats.md):
//!
//! | logical | what |
//! |---|---|
//! | `catalogue` | the Mobility Database catalogue (`feeds_v2.csv`), as downloaded once |
//! | `checked` | every catalogue feed checked for rail routes so far (railfeeds.py's answers) |
//! | `gtfs/<feed id>` | each feed's zip, as fetched |
//! | `fetched` | the day each zip's timetable counts from, by its content name |
//! | `feeds` | the coverage's feeds, in reading order, each with its zip and day |
//! | `mtr-pairs`, `mtr` | the MTR's lines as stop pairs, and the research they were made from |
//!
//! A zip's typical weekday is looked for around the day it was fetched, not the day it's read, so
//! its counts never change and it's never fetched again just because time passed: only one that was
//! already out of date when fetched is, a week later (railfeeds.py). The sources start from the
//! legacy build's (`rail-seed`), whose zips count from the day today's figures were counted.

use crate::coverage::{Coverage, Shape};
use crate::out::Out;
use crate::outlines::{flag, Outlines};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const CATALOGUE: &str = "sources/rail/catalogue";
pub const CHECKED: &str = "sources/rail/checked";
pub const FETCHED: &str = "sources/rail/fetched";
pub const FEEDS: &str = "sources/rail/feeds";
pub const MTR_PAIRS: &str = "sources/rail/mtr-pairs";
pub const MTR: &str = "sources/rail/mtr";
/// Trains a day per rail way: sorted `(u32 OSM way id, f32 trains a day each way)`, negative for a
/// lower bound (docs/formats.md, `/api/railfreq`).
pub const RAILFREQ: &str = "global/railfreq";

/// A feed's zip, by its id (the catalogue's ids, and ours, are letters, digits, `-` and `_`).
pub fn zip_logical(id: &str) -> String {
    format!("sources/rail/gtfs/{id}")
}

/// A stop-pair record (railgtfs.py, mtrpairs.py): f32 lon_a, lat_a, lon_b, lat_b, u8 mode, f32 trains.
pub const PAIR: usize = 21;
/// Mode bits: stop A, stop B beyond the coverage.
pub const BEYOND_A: u8 = 0x20;
pub const BEYOND_B: u8 = 0x40;

/// Marks each pair's stops beyond the coverage (`BEYOND_A`, `BEYOND_B`: a cross-border service,
/// which railfreq runs as far as the track goes toward the stop) and leaves out pairs with both
/// beyond. Returns the pairs kept and how many have a stop beyond.
pub fn mark_beyond(pairs: &[u8], cov: &Coverage) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(pairs.len());
    let mut beyond = 0;
    let e7 = |b: &[u8]| (f32::from_le_bytes(b[..4].try_into().unwrap()) as f64 * 1e7).round() as i32;
    for r in pairs.chunks_exact(PAIR) {
        let out_a = !cov.contains([e7(&r[0..]), e7(&r[4..])]);
        let out_b = !cov.contains([e7(&r[8..]), e7(&r[12..])]);
        if out_a && out_b {
            continue;
        }
        beyond += (out_a || out_b) as usize;
        let mode = (r[16] & !(BEYOND_A | BEYOND_B)) | if out_a { BEYOND_A } else { 0 } | if out_b { BEYOND_B } else { 0 };
        out.extend_from_slice(&r[..16]);
        out.push(mode);
        out.extend_from_slice(&r[17..PAIR]);
    }
    (out, beyond)
}

/// railfreq's `rail-freq.bin` (u32 way index into `ways`, f32 trains) as `global/railfreq`: by OSM
/// way id, sorted, each way once.
pub fn by_way_id(ways: &[roadcore::WayRec], freq: &[u8]) -> Vec<u8> {
    let mut recs: Vec<(u32, f32)> = freq
        .chunks_exact(8)
        .filter_map(|c| {
            let i = u32::from_le_bytes(c[..4].try_into().unwrap()) as usize;
            ways.get(i).map(|w| (w.id as u32, f32::from_le_bytes(c[4..].try_into().unwrap())))
        })
        .collect();
    recs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    recs.dedup_by_key(|r| r.0);
    recs.iter().flat_map(|(w, f)| w.to_le_bytes().into_iter().chain(f.to_le_bytes())).collect()
}

/// The coverage for railfeeds.py: a feature per outline, its rings in degrees (under the even–odd
/// rule) and its buffer in metres.
pub fn coverage_geojson(cov: &Coverage) -> Value {
    let deg = |p: &[i32; 2]| [p[0] as f64 * 1e-7, p[1] as f64 * 1e-7];
    let feats: Vec<Value> = cov
        .shapes
        .iter()
        .map(|s| {
            let polys: Vec<Value> = s.rings.iter().map(|r| serde_json::json!([r.iter().map(deg).collect::<Vec<_>>()])).collect();
            serde_json::json!({"type": "Feature", "properties": {"buffer_m": s.buffer_m}, "geometry": {"type": "MultiPolygon", "coordinates": polys}})
        })
        .collect();
    serde_json::json!({"type": "FeatureCollection", "features": feats})
}

/// Of an outline's points, the share a country must hold for the coverage to be in it; of a
/// country's own points, the share the coverage must hold.
const OF_OUTLINE: f64 = 0.05;
const OF_COUNTRY: f64 = 0.5;
/// Points along each side of a box sampled for those shares.
const LATTICE: i64 = 128;

/// The points of a regular lattice over a box (E7: cell centres, `LATTICE` a side).
fn lattice(b: [i32; 4]) -> impl Iterator<Item = [i32; 2]> {
    let (w, h) = (b[2] as i64 - b[0] as i64, b[3] as i64 - b[1] as i64);
    let n = if w < 0 || h < 0 { 0 } else { LATTICE };
    (0..n).flat_map(move |j| (0..n).map(move |i| [(b[0] as i64 + (2 * i + 1) * w / (2 * LATTICE)) as i32, (b[1] as i64 + (2 * j + 1) * h / (2 * LATTICE)) as i32]))
}

/// A territory's ISO 3166-1 code. An outline keeps its ISO 3166-2 code when it has both (Hong
/// Kong's "CN-HK", Saint-Pierre-et-Miquelon's "FR-PM"), whose second part is then the code.
fn alpha2(iso: &str) -> Option<String> {
    let c = iso.rsplit('-').next()?;
    (c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).then(|| c.to_string())
}

/// The countries the coverage is in, as ISO 3166-1 codes (where the rail feeds come from), from the
/// pass's territories (outlines with an ISO 3166-1 code, simplified): each holding at least 5 % of
/// one of its outlines' points (a lattice over the outline, without its buffer, each point given to
/// the smallest territory holding it, so Hong Kong's points are Hong Kong's even if China's outline
/// holds them too, and only points some territory holds count), or half or more of whose own points
/// the coverage holds (a microstate or an island inside a larger outline). A neighbour that only the
/// margin of an outline reaches (Geofabrik's outlines run a few kilometres past borders: 1 % of
/// Ontario's is the US) is neither; a small outline's wide margin may be (Monaco's is 44 % France,
/// Taiwan's 11 % China, around Kinmen and Matsu).
pub fn countries(cov: &Coverage, outlines: &Outlines) -> Result<Vec<String>> {
    let mut terr: Vec<(&crate::outlines::OutlineRec, String)> = outlines
        .recs
        .iter()
        .filter(|r| r.flags & flag::ISO1 != 0 && cov.meets_box(r.bbox))
        .filter_map(|r| alpha2(outlines.string(r.iso)).map(|c| (r, c)))
        .collect();
    terr.sort_by(|a, b| a.0.area_km2.total_cmp(&b.0.area_km2).then(a.0.id.cmp(&b.0.id)));
    // Each outline's lattice points inside its rings, until a territory takes them.
    let mut left: Vec<Vec<[i32; 2]>> = cov
        .shapes
        .iter()
        .map(|s| {
            let bare = Shape::new(s.source.clone(), s.rings.clone(), 0.0);
            lattice(bare.bbox).filter(|p| bare.contains(*p)).collect()
        })
        .collect();
    let mut held: BTreeMap<(usize, String), usize> = BTreeMap::new();
    let mut found: BTreeSet<String> = BTreeSet::new();
    for (r, code) in &terr {
        let t = Shape::new(code.clone(), outlines.simple_rings(r)?, 0.0);
        for (i, pts) in left.iter_mut().enumerate() {
            let before = pts.len();
            pts.retain(|p| !t.contains(*p));
            if pts.len() < before {
                *held.entry((i, code.clone())).or_default() += before - pts.len();
            }
        }
        let own: Vec<[i32; 2]> = lattice(t.bbox).filter(|p| t.contains(*p)).collect();
        if !own.is_empty() && own.iter().filter(|p| cov.contains(**p)).count() as f64 >= OF_COUNTRY * own.len() as f64 {
            found.insert(code.clone());
        }
    }
    let mut in_some = vec![0usize; cov.shapes.len()];
    for ((i, _), n) in &held {
        in_some[*i] += n;
    }
    for ((i, code), n) in &held {
        if *n as f64 >= OF_OUTLINE * in_some[*i] as f64 {
            found.insert(code.clone());
        }
    }
    Ok(found.into_iter().collect())
}

/// The day a zip's timetable counts from, by the zip's content name (`sources/rail/fetched`).
pub type Fetched = BTreeMap<String, String>;

pub fn read_fetched(out: &Out) -> Result<Fetched> {
    match out.get(FETCHED) {
        Some(c) => Ok(serde_json::from_slice(&std::fs::read(out.path(c)).with_context(|| format!("read {c}"))?)?),
        None => Ok(Fetched::new()),
    }
}

/// The UTC day of a time, `YYYY-MM-DD`.
pub fn day_of(t: std::time::SystemTime) -> String {
    let s = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    // Civil from days (Howard Hinnant's algorithm).
    let z = s.div_euclid(86400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}")
}

/// The NAS's zips for railfeeds.py's `--cache`: `{feed id: {path, fetched}}`, every
/// `sources/rail/gtfs/<id>` with its day (else its file's).
pub fn cache_index(out: &Out, fetched: &Fetched) -> Value {
    let mut idx = serde_json::Map::new();
    let prefix = "sources/rail/gtfs/";
    for (l, c) in out.manifest.range(prefix.to_string()..).take_while(|(l, _)| l.starts_with(prefix)) {
        let day = fetched.get(c).cloned().or_else(|| std::fs::metadata(out.path(c)).and_then(|m| m.modified()).ok().map(day_of));
        let Some(day) = day else { continue };
        idx.insert(l[prefix.len()..].to_string(), serde_json::json!({"path": out.path(c), "fetched": day}));
    }
    Value::Object(idx)
}

/// The rail sources from the legacy build's rail folders (`from`, the first one's files first:
/// `feeds_v2.csv`, `feeds-checked.json`, `gtfs/<id>.zip`, `pairs-mtr.bin`, `mtr.json`), adding only
/// what the sources lack and replacing nothing, so it can run again. A zip's day is the day the
/// folder's figures were counted (its `feeds-used.json`), else the day it was downloaded.
pub fn seed(out: &mut Out, from: &[PathBuf]) -> Result<SeedReport> {
    let mut rep = SeedReport::default();
    let first = |name: &str| from.iter().map(|d| d.join(name)).find(|p| p.is_file());
    for (logical, ext, name) in [(CATALOGUE, "csv", "feeds_v2.csv"), (MTR_PAIRS, "bin", "pairs-mtr.bin"), (MTR, "json", "mtr.json")] {
        if out.get(logical).is_none() {
            if let Some(p) = first(name) {
                out.put_file(logical, ext, &copy_local(out, &p)?)?;
                rep.files.push(logical.to_string());
            }
        }
    }
    // Every catalogue feed checked, by id: the sources' answers first.
    let mut checked: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(c) = out.get(CHECKED) {
        for v in serde_json::from_slice::<Vec<Value>>(&std::fs::read(out.path(c))?)? {
            checked.insert(v["id"].as_str().unwrap_or_default().to_string(), v);
        }
    }
    let had = checked.len();
    for d in from {
        let Ok(b) = std::fs::read(d.join("feeds-checked.json")) else { continue };
        for v in serde_json::from_slice::<Vec<Value>>(&b).with_context(|| format!("{}", d.join("feeds-checked.json").display()))? {
            if let Some(id) = v["id"].as_str() {
                checked.entry(id.to_string()).or_insert(v);
            }
        }
    }
    if checked.len() > had {
        out.put_bytes(CHECKED, "json", &serde_json::to_vec_pretty(&checked.values().collect::<Vec<_>>())?)?;
        rep.checked = checked.len() - had;
    }
    // The zips the sources lack, the newest copy of each; and the day of one seeded by a run cut
    // short before it recorded it.
    let mut fetched = read_fetched(out)?;
    let mut zips: BTreeMap<String, (std::time::SystemTime, PathBuf, String)> = BTreeMap::new();
    let mut repaired = false;
    for d in from {
        let counted = std::fs::metadata(d.join("feeds-used.json")).and_then(|m| m.modified()).ok().map(day_of);
        let Ok(rd) = std::fs::read_dir(d.join("gtfs")) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Some(id) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".zip")).map(str::to_string) else { continue };
            let t = e.metadata()?.modified()?;
            let day = counted.clone().unwrap_or_else(|| day_of(t));
            if let Some(c) = out.get(&zip_logical(&id)).map(str::to_string) {
                if !fetched.contains_key(&c) && store::naming::parse_content_name(&c).is_some_and(|n| store::naming::hash16_file(&p).is_ok_and(|h| h == n.hash16)) {
                    fetched.insert(c, day);
                    repaired = true;
                }
                continue;
            }
            if zips.get(&id).is_none_or(|z| t > z.0) {
                zips.insert(id, (t, p, day));
            }
        }
    }
    let n = zips.len() as u64;
    for (k, (id, (_, p, day))) in zips.into_iter().enumerate() {
        crate::agent::jobs::report(k as u64, n, "zips copied");
        let name = out.put_file(&zip_logical(&id), "zip", &copy_local(out, &p)?)?;
        fetched.insert(name, day);
        rep.zips += 1;
        if k % 20 == 19 {
            out.put_bytes(FETCHED, "json", &serde_json::to_vec_pretty(&fetched)?)?;
            out.save()?;
        }
    }
    if rep.zips > 0 || repaired {
        out.put_bytes(FETCHED, "json", &serde_json::to_vec_pretty(&fetched)?)?;
    }
    out.save()?;
    Ok(rep)
}

#[derive(Debug, Default)]
pub struct SeedReport {
    pub files: Vec<String>,
    pub checked: usize,
    pub zips: usize,
}

/// A copy of `p` in scratch (`put_file` consumes its input).
fn copy_local(out: &Out, p: &Path) -> Result<PathBuf> {
    let dest = out.scratch_file(&format!("rail-{}", p.file_name().context("file name")?.to_string_lossy()));
    std::fs::copy(p, &dest).with_context(|| format!("copy {}", p.display()))?;
    Ok(dest)
}

/// railfeeds.py's result as `sources/rail/feeds`: each feed with a zip given its zip's content name
/// (`zip`) and day (`fetched`), `new` ones uploaded first from `gtfs` (railfeeds.py's `--out/gtfs`).
/// Returns how many zips were new.
pub fn put_feeds(out: &mut Out, found: &Path, gtfs: &Path, fetched: &mut Fetched) -> Result<usize> {
    let mut v: Value = serde_json::from_slice(&std::fs::read(found)?)?;
    let mut new = 0;
    for f in v["feeds"].as_array_mut().context("feeds")? {
        let id = f["id"].as_str().context("a feed without its id")?.to_string();
        let zip = match f.get("zip").and_then(Value::as_str) {
            Some("new") => {
                let name = out.put_file(&zip_logical(&id), "zip", &gtfs.join(format!("{id}.zip")))?;
                fetched.insert(name.clone(), f["fetched"].as_str().context("a new zip without its day")?.to_string());
                new += 1;
                name
            }
            Some(_) => out.get(&zip_logical(&id)).with_context(|| format!("{id}: no zip in the sources"))?.to_string(),
            None => continue,
        };
        // (A zip put there another way counts from the day railfeeds.py was given: its file's.)
        let day = fetched.get(&zip).cloned().or_else(|| f["fetched"].as_str().map(str::to_string)).with_context(|| format!("{id}: its zip's day"))?;
        fetched.insert(zip.clone(), day.clone());
        f["fetched"] = Value::from(day);
        f["zip"] = Value::from(zip);
    }
    out.put_bytes(FETCHED, "json", &serde_json::to_vec_pretty(fetched)?)?;
    out.put_bytes(FEEDS, "json", &serde_json::to_vec_pretty(&serde_json::json!({"fmt": 1, "feeds": v["feeds"]}))?)?;
    Ok(new)
}

/// What railfeeds.py downloaded before failing (`gtfs`, its `--out/gtfs`): kept in the sources, each
/// counting from the day it was downloaded, so the next run has them.
pub fn keep_downloads(out: &mut Out, gtfs: &Path, fetched: &mut Fetched) -> Result<usize> {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(gtfs) else { return Ok(0) };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "zip")).collect();
    files.sort();
    for p in files {
        let id = p.file_stem().context("zip")?.to_string_lossy().into_owned();
        let day = day_of(std::fs::metadata(&p)?.modified()?);
        let name = out.put_file(&zip_logical(&id), "zip", &p)?;
        fetched.insert(name, day);
        n += 1;
    }
    if n > 0 {
        out.put_bytes(FETCHED, "json", &serde_json::to_vec_pretty(fetched)?)?;
    }
    Ok(n)
}

/// `sources/rail/feeds` for railgtfs.py: each zip's content name as its path on the NAS.
pub fn feeds_for_counting(out: &Out) -> Result<Value> {
    let c = out.get(FEEDS).context("no rail feeds yet (the rail-feeds step)")?;
    let mut v: Value = serde_json::from_slice(&std::fs::read(out.path(c)).with_context(|| format!("read {c}"))?)?;
    for f in v["feeds"].as_array_mut().context("feeds")? {
        if let Some(z) = f.get("zip").and_then(Value::as_str) {
            f["path"] = Value::from(out.path(z).to_string_lossy().into_owned());
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;

    fn pair(a: (f32, f32), b: (f32, f32), mode: u8, n: f32) -> Vec<u8> {
        let mut v = Vec::new();
        for x in [a.0, a.1, b.0, b.1] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.push(mode);
        v.extend_from_slice(&n.to_le_bytes());
        v
    }

    fn square_cov(d: &Path) -> Coverage {
        std::fs::write(d.join("sq.poly"), "sq\n1\n 0 0\n 1 0\n 1 1\n 0 1\nEND\nEND\n").unwrap();
        Coverage::from_recipes(&[Recipe { id: "s".into(), name: "S".into(), outline: vec!["poly:sq.poly".into()] }], None, d).unwrap()
    }

    #[test]
    fn stops_beyond_the_coverage() {
        let d = tempfile::tempdir().unwrap();
        let cov = square_cov(d.path());
        let pairs = [pair((0.5, 0.5), (0.6, 0.6), 2, 10.0), pair((0.5, 0.5), (1.5, 0.5), 0x80 | 1, 4.0), pair((1.5, 0.5), (0.5, 0.5), 2, 3.0), pair((2.0, 2.0), (3.0, 3.0), 2, 7.0)].concat();
        let (kept, beyond) = mark_beyond(&pairs, &cov);
        assert_eq!((kept.len() / PAIR, beyond), (3, 2), "both beyond: left out");
        let modes: Vec<u8> = kept.chunks_exact(PAIR).map(|r| r[16]).collect();
        assert_eq!(modes, vec![2, 0x80 | 1 | BEYOND_B, 2 | BEYOND_A], "the lower bound kept, beyond marked");
        assert_eq!(&kept[PAIR + 17..2 * PAIR], &4.0f32.to_le_bytes(), "trains as they were");
        // Marked again (a coverage that grew): the marks follow the coverage.
        let (again, _) = mark_beyond(&kept, &cov);
        assert_eq!(again, kept);
    }

    #[test]
    fn freq_by_way_id() {
        let way = |id: i64| roadcore::WayRec { id, ..bytemuck::Zeroable::zeroed() };
        let ways = [way(30), way(10), way(20)];
        let rec = |i: u32, f: f32| i.to_le_bytes().into_iter().chain(f.to_le_bytes()).collect::<Vec<u8>>();
        let freq = [rec(0, 5.0), rec(1, -2.5), rec(2, 1.0), rec(7, 9.0)].concat();
        let out = by_way_id(&ways, &freq);
        let got: Vec<(u32, f32)> = out.chunks_exact(8).map(|c| (u32::from_le_bytes(c[..4].try_into().unwrap()), f32::from_le_bytes(c[4..].try_into().unwrap()))).collect();
        assert_eq!(got, vec![(10, -2.5), (20, 1.0), (30, 5.0)], "sorted by way id; an index past the ways dropped");
    }

    #[test]
    fn days_of_times() {
        let t = |s: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s);
        assert_eq!(day_of(t(0)), "1970-01-01");
        // 2026-09-30 12:42 UTC, and the last second of a leap day.
        assert_eq!(day_of(t(1_790_772_120)), "2026-09-30");
        assert_eq!(day_of(t(1_709_251_199)), "2024-02-29");
    }

    #[test]
    fn country_codes() {
        assert_eq!(alpha2("FR").as_deref(), Some("FR"));
        assert_eq!(alpha2("CN-HK").as_deref(), Some("HK"));
        assert_eq!(alpha2("FR-PM").as_deref(), Some("PM"));
        assert_eq!(alpha2("NO-21"), None);
        assert_eq!(alpha2(""), None);
    }

    /// Outlines of four territories: A (10° square), its neighbour B, C inside A (and inside A's
    /// outline too, as Hong Kong's is in China's), and D, a small island in A's waters with its own
    /// code.
    fn outlines(d: &Path) -> Outlines {
        let feat = |id: u64, iso: &str, w: f64, s: f64, e: f64, n: f64| {
            format!(
                "\u{1e}{{\"type\":\"Feature\",\"geometry\":{{\"type\":\"Polygon\",\"coordinates\":[[[{w},{s}],[{e},{s}],[{e},{n}],[{w},{n}],[{w},{s}]]]}},\"properties\":{{\"@type\":\"relation\",\"@id\":{id},\"boundary\":\"administrative\",\"admin_level\":\"2\",\"name\":\"{iso}\",\"ISO3166-1\":\"{iso}\"}}}}\n"
            )
        };
        let geo = d.join("o.geojsonseq");
        std::fs::write(&geo, [feat(1, "AA", 0.0, 0.0, 10.0, 10.0), feat(2, "BB", 10.0, 0.0, 20.0, 10.0), feat(3, "CC", 4.0, 4.0, 5.0, 5.0), feat(4, "DD", 9.0, 9.0, 9.2, 9.2)].concat()).unwrap();
        crate::outlines::assemble_geojsonseq(&geo, &d.join("o.sect")).unwrap();
        Outlines::open(&d.join("o.sect")).unwrap()
    }

    #[test]
    fn countries_the_coverage_is_in() {
        let d = tempfile::tempdir().unwrap();
        let o = outlines(d.path());
        let poly = |name: &str, w: f64, s: f64, e: f64, n: f64| {
            std::fs::write(d.path().join(name), format!("p\n1\n {w} {s}\n {e} {s}\n {e} {n}\n {w} {n}\nEND\nEND\n")).unwrap();
            format!("poly:{name}")
        };
        let cov = |outline: Vec<String>| Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline }], None, d.path()).unwrap();
        // Most of A, running 0.2° into B: A's, and C's and D's (each all inside, though a small part
        // of the outline); not B's (a margin).
        assert_eq!(countries(&cov(vec![poly("a.poly", 0.5, 0.5, 10.2, 9.5)]), &o).unwrap(), vec!["AA", "CC", "DD"]);
        // An outline of C alone: C's, not A's, though A's outline holds it too.
        assert_eq!(countries(&cov(vec![poly("c.poly", 4.1, 4.1, 4.9, 4.9)]), &o).unwrap(), vec!["CC"]);
        // Two outlines, one in each: both.
        assert_eq!(countries(&cov(vec![poly("a2.poly", 1.0, 1.0, 2.0, 2.0), poly("b.poly", 15.0, 5.0, 16.0, 6.0)]), &o).unwrap(), vec!["AA", "BB"]);
        // Out at sea: none.
        assert!(countries(&cov(vec![poly("sea.poly", 30.0, 30.0, 31.0, 31.0)]), &o).unwrap().is_empty());
    }

    #[test]
    fn seeding_adds_what_the_sources_lack() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("root"), d.path().join("scratch"));
        let legacy = |name: &str, zips: &[(&str, &[u8])], checked: &str| {
            let l = d.path().join(name);
            std::fs::create_dir_all(l.join("gtfs")).unwrap();
            for (id, b) in zips {
                std::fs::write(l.join("gtfs").join(format!("{id}.zip")), b).unwrap();
            }
            std::fs::write(l.join("feeds-checked.json"), checked).unwrap();
            std::fs::write(l.join("feeds_v2.csv"), "id,data_type\n").unwrap();
            std::fs::write(l.join("pairs-mtr.bin"), [0u8; PAIR]).unwrap();
            l
        };
        // m4 holds more than m1, and m1 one zip of its own.
        let m4 = legacy("m4", &[("mdb-1", b"one"), ("jbda-x", b"jp")], r#"[{"id": "mdb-1", "rail_routes": 2}, {"id": "jbda-x", "rail_routes": 1}]"#);
        let m1 = legacy("m1", &[("mdb-1", b"one"), ("tld-9", b"nine")], r#"[{"id": "mdb-1", "rail_routes": 2}, {"id": "mdb-5", "rail_routes": 0}]"#);
        std::fs::write(m4.join("feeds-used.json"), "[]").unwrap();
        let mut out = Out::open(&root, &scratch).unwrap();
        let rep = seed(&mut out, &[m4.clone(), m1.clone()]).unwrap();
        assert_eq!((rep.zips, rep.checked), (3, 3));
        assert_eq!(rep.files, vec![CATALOGUE.to_string(), MTR_PAIRS.to_string()], "no mtr.json there");
        for id in ["mdb-1", "jbda-x", "tld-9"] {
            assert!(out.get(&zip_logical(id)).is_some(), "{id}");
        }
        let fetched = read_fetched(&out).unwrap();
        assert_eq!(fetched.len(), 3);
        // A run cut short before it recorded a zip's day: the next one records it.
        let jp = out.get(&zip_logical("jbda-x")).unwrap().to_string();
        let mut short = fetched.clone();
        short.remove(&jp);
        out.put_bytes(FETCHED, "json", &serde_json::to_vec(&short).unwrap()).unwrap();
        assert_eq!(seed(&mut out, &[m4.clone(), m1.clone()]).unwrap().zips, 0);
        assert_eq!(read_fetched(&out).unwrap(), fetched);
        // A zip refetched since stays as it is, and a second run adds nothing.
        let newer = d.path().join("newer.zip");
        std::fs::write(&newer, b"one, newer").unwrap();
        let name = out.put_file(&zip_logical("mdb-1"), "zip", &newer).unwrap();
        let rep = seed(&mut out, &[m4, m1]).unwrap();
        assert_eq!((rep.zips, rep.checked, rep.files.len()), (0, 0, 0));
        assert_eq!(out.get(&zip_logical("mdb-1")), Some(name.as_str()));
        // Its index for railfeeds.py: every zip with its day.
        let idx = cache_index(&out, &read_fetched(&out).unwrap());
        assert_eq!(idx.as_object().unwrap().keys().collect::<Vec<_>>(), vec!["jbda-x", "mdb-1", "tld-9"]);
        assert!(idx["tld-9"]["fetched"].as_str().is_some_and(|s| s.len() == 10));
    }

    #[test]
    fn the_feeds_list_names_each_zip() {
        let d = tempfile::tempdir().unwrap();
        let mut out = Out::open(&d.path().join("root"), &d.path().join("scratch")).unwrap();
        let cached = d.path().join("c.zip");
        std::fs::write(&cached, b"cached").unwrap();
        let cname = out.put_file(&zip_logical("mdb-1"), "zip", &cached).unwrap();
        let mut fetched: Fetched = [(cname.clone(), "2026-09-30".to_string())].into();
        let gtfs = d.path().join("gtfs");
        std::fs::create_dir_all(&gtfs).unwrap();
        std::fs::write(gtfs.join("sncf.zip"), b"new").unwrap();
        let found = d.path().join("feeds.json");
        std::fs::write(&found, r#"{"feeds": [{"id": "sncf", "zip": "new", "fetched": "2026-10-04"}, {"id": "mdb-1", "zip": "cache", "fetched": "2026-09-30"}, {"id": "mdb-2", "status": "replaced by sncf"}]}"#).unwrap();
        assert_eq!(put_feeds(&mut out, &found, &gtfs, &mut fetched).unwrap(), 1);
        let v: Value = serde_json::from_slice(&std::fs::read(out.path(out.get(FEEDS).unwrap())).unwrap()).unwrap();
        let f = v["feeds"].as_array().unwrap();
        assert_eq!(f[0]["zip"], Value::from(out.get(&zip_logical("sncf")).unwrap()));
        assert_eq!((f[0]["fetched"].as_str(), f[1]["fetched"].as_str()), (Some("2026-10-04"), Some("2026-09-30")));
        assert_eq!(f[1]["zip"], Value::from(cname));
        assert!(f[2].get("zip").is_none());
        // For counting: the zips' paths on the NAS.
        let c = feeds_for_counting(&out).unwrap();
        assert!(c["feeds"][0]["path"].as_str().unwrap().ends_with(".zip") && c["feeds"][2].get("path").is_none());
    }
}
