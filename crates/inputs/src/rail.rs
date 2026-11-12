//! Rail service (docs/steps.md §10): trains a day on every rail way of the coverage,
//! from the operators' timetables (GTFS) and the static timetables compiled by hand
//! (crate::inputs::gtfs, docs/inputs.md §6.7.2: the MTR's lines among them), as `global/railfreq`;
//! and the timetable feeds the ferries are counted from.
//!
//! Two jobs. `rail-feeds` finds the feeds (crate::railfeeds) from the accepted declarations of
//! `inputs/timetables/feeds/` (crate::inputs::feeds, docs/inputs.md §6.7.1): the Mobility Database
//! catalogue's feeds of the countries the coverage is in, by their boxes, the ones that run rail,
//! and the declared feeds (operators' own, feeds behind an API key, the ferries'), and fetches
//! each once into the rail sources. `rail` counts their trains and the accepted static files'
//! (crate::gtfs::railcount: the static files' count rows as given, apart from the timed trains), marks
//! the stops beyond the coverage, and matches the stop pairs onto the rail ways of the pass's rail
//! set that touch the coverage (`extract`, then `railfreq`). The `ferries` job counts the ferry
//! list's sailings and the static files' (crate::ferries, run by crate::ovconv).
//!
//! The rail sources (`sources/rail/`, content-named, in the build manifest; docs/formats.md):
//!
//! | logical | what |
//! |---|---|
//! | `catalogue` | the Mobility Database catalogue (`feeds_v2.csv`), as fetched once |
//! | `checked` | every catalogue feed checked for rail routes so far (crate::railfeeds' answers) |
//! | `gtfs/<feed id>` | each feed's zip, as fetched (the ferry feeds' too) |
//! | `fetched` | the day each zip's timetable counts from, by its content name |
//! | `feeds` | the coverage's rail feeds, in reading order, each with its zip and day |
//! | `ferry-feeds` | the ferry feeds, each with its zip and day |
//!
//! A zip's typical weekday is looked for around the day it counts from (the day it was fetched),
//! not the day it's read, so its counts don't change with the day they're made, and a zip with
//! service is never fetched again. Only one already out of date when fetched is: the next time
//! rail-feeds runs (when its key changes: a new pass's outlines, the coverage, the declarations,
//! the catalogue or the keys) at least its declaration's `refresh` days after its day, and the same
//! file then counts from that run's day (crate::railfeeds). The catalogue is fetched from its
//! declaration's URL when there's no copy.

use anyhow::{Context, Result};
use buildstore::coverage::{Asking, Coverage, Shape};
use buildstore::out::Out;
use buildstore::outlines::{flag, Outlines};
use jobrt::timings::day_of;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const CATALOGUE: &str = "sources/rail/catalogue";
pub const CHECKED: &str = "sources/rail/checked";
pub const FETCHED: &str = "sources/rail/fetched";
pub const FEEDS: &str = "sources/rail/feeds";
pub const FERRY_FEEDS: &str = "sources/rail/ferry-feeds";
/// Trains a day per rail way: sorted `(u32 OSM way id, f32 trains a day each way)`, negative for a
/// lower bound (docs/formats.md, `/api/railfreq`).
pub const RAILFREQ: &str = "global/railfreq";

/// A feed's zip, by its id (the catalogue's ids, and ours, are letters, digits, `-` and `_`).
pub fn zip_logical(id: &str) -> String {
    format!("sources/rail/gtfs/{id}")
}

/// A stop-pair record's size (`Pair`).
pub const PAIR: usize = 21;

/// A stop-pair record (crate::gtfs::railcount writes them, railfreq reads them): f32 lon_a, lat_a,
/// lon_b, lat_b, u8 mode, f32 trains, little-endian.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    /// Stop A's and stop B's lon, lat.
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// The mode in its low 5 bits, `BEYOND_A`, `BEYOND_B`, and 0x80: `trains` a lower bound.
    pub mode: u8,
    pub trains: f32,
}

impl Pair {
    /// Record `r` (`PAIR` bytes).
    pub fn read(r: &[u8]) -> Pair {
        let f = |i: usize| f32::from_le_bytes(r[i..i + 4].try_into().unwrap());
        Pair { a: [f(0), f(4)], b: [f(8), f(12)], mode: r[16], trains: f(17) }
    }

    /// The records of `b`, in order.
    pub fn all(b: &[u8]) -> impl Iterator<Item = Pair> + '_ {
        b.chunks_exact(PAIR).map(Pair::read)
    }

    /// Appends its record to `out`.
    pub fn write(&self, out: &mut Vec<u8>) {
        for x in [self.a[0], self.a[1], self.b[0], self.b[1]] {
            out.extend(x.to_le_bytes());
        }
        out.push(self.mode);
        out.extend(self.trains.to_le_bytes());
    }
}
/// Mode bits: stop A, stop B beyond the coverage.
pub const BEYOND_A: u8 = 0x20;
pub const BEYOND_B: u8 = 0x40;

/// Marks each pair's stops beyond the coverage (`BEYOND_A`, `BEYOND_B`: a cross-border service,
/// which railfreq runs as far as the track goes toward the stop) and leaves out pairs with both
/// beyond. Returns the pairs kept and how many have a stop beyond.
pub fn mark_beyond(pairs: &[u8], cov: &Asking) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(pairs.len());
    let mut beyond = 0;
    let e7 = |p: [f32; 2]| p.map(|v| (v as f64 * 1e7).round() as i32);
    for mut r in Pair::all(pairs) {
        let out_a = !cov.contains(e7(r.a));
        let out_b = !cov.contains(e7(r.b));
        if out_a && out_b {
            continue;
        }
        beyond += (out_a || out_b) as usize;
        r.mode =
            (r.mode & !(BEYOND_A | BEYOND_B)) | if out_a { BEYOND_A } else { 0 } | if out_b { BEYOND_B } else { 0 };
        r.write(&mut out);
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
    (0..n).flat_map(move |j| {
        (0..n).map(move |i| {
            [
                (b[0] as i64 + (2 * i + 1) * w / (2 * LATTICE)) as i32,
                (b[1] as i64 + (2 * j + 1) * h / (2 * LATTICE)) as i32,
            ]
        })
    })
}

/// A territory's ISO 3166-1 code. An outline keeps its ISO 3166-2 code when it has both (Hong
/// Kong's "CN-HK", Saint-Pierre-et-Miquelon's "FR-PM"), whose second part is then the code.
fn alpha2(iso: &str) -> Option<String> {
    let c = iso.rsplit('-').next()?;
    (c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).then(|| c.to_string())
}

/// The ISO 3166-2 codes of the territories with an ISO 3166-1 code of their own that isn't their
/// code's end (and of two without one, whose ends are other countries' codes): their own code (the
/// catalogue declaration's `own_code`, crate::inputs::feeds::own_code).
pub type OwnCode = BTreeMap<String, Option<String>>;

/// rail-feeds' countries (`countries`) as a question of the coverage asked whole, which its key
/// asks again (buildstore::asked::Record::also): the pass outlines' content name and the own codes.
pub fn countries_question(outlines: &str, own: &OwnCode) -> String {
    format!("countries {outlines} {}", serde_json::to_string(own).expect("codes serialise"))
}

/// `countries_question`'s parts: (the outlines' content name, the own codes); None for another question.
pub fn parse_countries_question(q: &str) -> Option<(String, OwnCode)> {
    let (o, own) = q.strip_prefix("countries ")?.split_once(' ')?;
    Some((o.to_string(), serde_json::from_str(own).ok()?))
}

/// The answer to a question whose outlines the records no longer name (their pass gone).
pub const GONE: &str = "gone";

/// The answer `cov` gives question `q` (`countries_question`) now, from the outlines it names
/// (`outlines`; None: no longer named, `GONE`): the countries, comma-separated. None for another
/// question, or while they can't be worked out now.
pub fn reask_countries(cov: &Coverage, q: &str, outlines: Option<&Outlines>) -> Option<String> {
    let (_, own) = parse_countries_question(q)?;
    match outlines {
        None => Some(GONE.to_string()),
        Some(o) => {
            countries(cov, o, &own).map_err(|e| eprintln!("rail-feeds' countries: {e:#}")).ok().map(|c| c.join(","))
        }
    }
}

/// The ISO 3166-1 codes a territory's outline stands for: its own, and for one with an ISO 3166-2
/// code, its country's too ("CN-HK": HK and CN; "FR-PM": PM and FR; "FI-01": AX and FI, by
/// `own_code`), which the catalogue may file its feeds under (Hong Kong's under CN). The feeds' boxes
/// keep the country's to those near.
fn territory_codes(iso: &str, own_code: &OwnCode) -> Vec<String> {
    let own = match own_code.get(iso) {
        Some(own) => own.clone(),
        None => alpha2(iso),
    };
    let mut codes: Vec<String> = own.into_iter().collect();
    if let Some((country, _)) = iso.split_once('-') {
        if country.len() == 2 && country.bytes().all(|b| b.is_ascii_uppercase()) && !codes.iter().any(|c| c == country)
        {
            codes.push(country.to_string());
        }
    }
    codes
}

/// The countries the coverage is in, as ISO 3166-1 codes (where the rail feeds come from), from the
/// pass's territories (outlines with an ISO 3166-1 code, simplified): each holding at least 5 % of
/// one of its outlines' points (a lattice over the outline, without its buffer, each point given to
/// the smallest territory holding it, so Hong Kong's points are Hong Kong's even if China's outline
/// holds them too, and only points some territory holds count), or half or more of whose own points
/// the coverage holds (a microstate or an island inside a larger outline). A neighbour that only the
/// margin of an outline reaches (Geofabrik's outlines run a few kilometres past borders: 1 % of
/// Ontario's is the US) is neither; a small outline's wide margin may be (Monaco's is 44 % France,
/// Taiwan's 11 % China, around Kinmen and Matsu). A territory with an ISO 3166-2 code brings its
/// country too (`territory_codes`: Hong Kong brings China).
pub fn countries(cov: &Coverage, outlines: &Outlines, own_code: &OwnCode) -> Result<Vec<String>> {
    let mut terr: Vec<(&buildstore::outlines::OutlineRec, Vec<String>)> = outlines
        .recs
        .iter()
        .filter(|r| r.flags & flag::ISO1 != 0 && cov.meets_box(r.bbox))
        .map(|r| (r, territory_codes(outlines.string(r.iso), own_code)))
        .filter(|(_, codes)| !codes.is_empty())
        .collect();
    terr.sort_by(|a, b| a.0.area_km2.total_cmp(&b.0.area_km2).then(a.0.id.cmp(&b.0.id)));
    // Each outline's lattice points inside its rings, until a territory takes them.
    let mut left: Vec<Vec<[i32; 2]>> = cov
        .shapes
        .iter()
        .map(|s| {
            let bare = Shape::new(s.source.clone(), s.rings.to_vec(), 0.0);
            lattice(bare.bbox).filter(|p| bare.contains(*p)).collect()
        })
        .collect();
    let mut held: BTreeMap<(usize, &[String]), usize> = BTreeMap::new();
    let mut found: BTreeSet<String> = BTreeSet::new();
    for (r, codes) in &terr {
        let t = Shape::new(codes[0].clone(), outlines.simple_rings(r)?, 0.0);
        for (i, pts) in left.iter_mut().enumerate() {
            let before = pts.len();
            pts.retain(|p| !t.contains(*p));
            if pts.len() < before {
                *held.entry((i, codes.as_slice())).or_default() += before - pts.len();
            }
        }
        let own: Vec<[i32; 2]> = lattice(t.bbox).filter(|p| t.contains(*p)).collect();
        if !own.is_empty() && own.iter().filter(|p| cov.contains(**p)).count() as f64 >= OF_COUNTRY * own.len() as f64 {
            found.extend(codes.iter().cloned());
        }
    }
    let mut in_some = vec![0usize; cov.shapes.len()];
    for ((i, _), n) in &held {
        in_some[*i] += n;
    }
    for ((i, codes), n) in &held {
        if *n as f64 >= OF_OUTLINE * in_some[*i] as f64 {
            found.extend(codes.iter().cloned());
        }
    }
    Ok(found.into_iter().collect())
}

/// The day a zip's timetable counts from, by the zip's content name (`sources/rail/fetched`).
pub type Fetched = BTreeMap<String, String>;

pub fn read_fetched(out: &Out) -> Result<Fetched> {
    match out.get(FETCHED) {
        Some(c) => Ok(serde_json::from_slice(&out.path(c).read().with_context(|| format!("read {c}"))?)?),
        None => Ok(Fetched::new()),
    }
}

/// The NAS's zips for crate::railfeeds: `{feed id: {path, fetched}}`, every
/// `sources/rail/gtfs/<id>` with its day (else its file's).
pub fn cache_index(out: &Out, fetched: &Fetched) -> Value {
    let mut idx = serde_json::Map::new();
    let prefix = "sources/rail/gtfs/";
    for (l, c) in out.manifest.range(prefix.to_string()..).take_while(|(l, _)| l.starts_with(prefix)) {
        let day =
            fetched.get(c).cloned().or_else(|| out.path(c).metadata().and_then(|m| m.modified()).ok().map(day_of));
        let Some(day) = day else { continue };
        // (A path the counting program opens: tests/nas_access.rs.)
        idx.insert(
            l[prefix.len()..].to_string(),
            serde_json::json!({"path": out.path(c).unchecked_path(), "fetched": day}),
        );
    }
    Value::Object(idx)
}

/// Saves this run's changes, then adds zips' days (`days`) to `sources/rail/fetched` as the manifest
/// on disk names it now (another run may be adding some), and saves again.
fn record_days(out: &mut Out, days: &Fetched) -> Result<()> {
    out.save()?;
    if days.is_empty() {
        return Ok(());
    }
    let mut all = read_fetched(out)?;
    let mut changed = false;
    for (zip, day) in days {
        changed |= all.insert(zip.clone(), day.clone()).as_ref() != Some(day);
    }
    if changed {
        out.put_bytes(FETCHED, "json", &serde_json::to_vec_pretty(&all)?)?;
        out.save()?;
    }
    Ok(())
}

/// A list crate::railfeeds wrote (`found`: its rail feeds, or its ferry feeds) as `logical` (`FEEDS`,
/// `FERRY_FEEDS`): each feed with a zip given its zip's content name
/// (`zip`) and day (`fetched`), `new` ones uploaded first from `gtfs` (its out folder's `gtfs/`;
/// one the same as a zip already there is kept once, counting from its new day). The manifest is
/// saved after each upload, since put_file removes the local copy: a failure later keeps them all,
/// with their days, and a run cut short keeps all but the last, counting from their files' days.
/// Returns how many zips were new.
pub fn put_feeds(out: &mut Out, found: &Path, gtfs: &Path, logical: &str) -> Result<usize> {
    let mut v: Value = serde_json::from_slice(&std::fs::read(found)?)?;
    let mut fetched = read_fetched(out)?;
    let mut days = Fetched::new();
    let named = name_zips(out, v["feeds"].as_array_mut().context("feeds")?, gtfs, &mut fetched, &mut days);
    let recorded = record_days(out, &days);
    let new = named?;
    recorded?;
    out.put_bytes(logical, "json", &serde_json::to_vec_pretty(&serde_json::json!({"fmt": 1, "feeds": v["feeds"]}))?)?;
    Ok(new)
}

/// put_feeds' uploads, and each feed's zip and day; the days set go to `days` too.
fn name_zips(
    out: &mut Out,
    feeds: &mut [Value],
    gtfs: &Path,
    fetched: &mut Fetched,
    days: &mut Fetched,
) -> Result<usize> {
    let mut new = 0;
    for f in feeds {
        let id = f["id"].as_str().context("a feed without its id")?.to_string();
        let zip = match f.get("zip").and_then(Value::as_str) {
            Some("new") => {
                let day = f["fetched"].as_str().context("a new zip without its day")?.to_string();
                let name = out.put_file(&zip_logical(&id), "zip", &gtfs.join(format!("{id}.zip")))?;
                out.save()?;
                fetched.insert(name.clone(), day.clone());
                days.insert(name.clone(), day);
                new += 1;
                name
            }
            Some(_) => out.get(&zip_logical(&id)).with_context(|| format!("{id}: no zip in the sources"))?.to_string(),
            None => continue,
        };
        // (A zip put there another way counts from the day crate::railfeeds was given: its file's.)
        let day = match fetched.get(&zip) {
            Some(d) => d.clone(),
            None => {
                let d = f["fetched"].as_str().with_context(|| format!("{id}: its zip's day"))?.to_string();
                fetched.insert(zip.clone(), d.clone());
                days.insert(zip.clone(), d.clone());
                d
            }
        };
        f["fetched"] = Value::from(day);
        f["zip"] = Value::from(zip);
    }
    Ok(new)
}

/// What crate::railfeeds downloaded before failing (`gtfs`, its out folder's `gtfs/`): kept in the sources, each
/// counting from the day it was downloaded, so the next run has them; saved as put_feeds does.
pub fn keep_downloads(out: &mut Out, gtfs: &Path) -> Result<usize> {
    put_zips(out, gtfs, None)
}

/// The declared feeds' first pins (a declaration's `pin`: a zip fetched before the app fetched it,
/// its path from the project folder, and the day it was fetched), put in the sources by the
/// rail-feeds job as it would a download, for each declared feed that has no zip there yet; `dir`:
/// local space. A pin whose zip can't be read fails the job (it's tried again), rather than fetch
/// the feed anew. Returns how many were pinned.
pub fn pin_seeds(out: &mut Out, decls: &[Value], dir: &Path) -> Result<usize> {
    let mut by_day: BTreeMap<String, Vec<(String, store::nasfs::NasPath)>> = BTreeMap::new();
    for d in decls {
        let (Some(id), Some(zip), Some(day)) = (d["id"].as_str(), d["pin"]["zip"].as_str(), d["pin"]["day"].as_str())
        else {
            continue;
        };
        if out.get(&zip_logical(id)).is_none() {
            by_day.entry(day.to_string()).or_default().push((id.to_string(), out.root().join(zip)));
        }
    }
    let mut n = 0;
    for (day, zips) in by_day {
        let at = dir.join(&day);
        std::fs::remove_dir_all(&at).ok();
        std::fs::create_dir_all(&at)?;
        for (id, src) in zips {
            src.copy_data_to_local(&at.join(format!("{id}.zip")))
                .with_context(|| format!("{id}: its pin, {}", src.display()))?;
        }
        n += put_zips(out, &at, Some(&day))?;
    }
    Ok(n)
}

/// `gtfs`'s zips in the sources, each counting from `day`, else from its file's day.
fn put_zips(out: &mut Out, gtfs: &Path, day: Option<&str>) -> Result<usize> {
    let mut days = Fetched::new();
    let mut n = 0;
    let kept = (|| -> Result<()> {
        let Ok(rd) = std::fs::read_dir(gtfs) else { return Ok(()) };
        let mut files: Vec<PathBuf> =
            rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "zip")).collect();
        files.sort();
        for p in files {
            let id = p.file_stem().context("zip")?.to_string_lossy().into_owned();
            let day = match day {
                Some(d) => d.to_string(),
                None => day_of(std::fs::metadata(&p)?.modified()?),
            };
            let name = out.put_file(&zip_logical(&id), "zip", &p)?;
            out.save()?;
            days.insert(name, day);
            n += 1;
        }
        Ok(())
    })();
    let recorded = record_days(out, &days);
    kept?;
    recorded?;
    Ok(n)
}

/// A feeds list (`FEEDS` for crate::gtfs::railcount, `FERRY_FEEDS` for crate::ferries): each zip's content name as
/// its path on the NAS.
pub fn feeds_for_counting(out: &Out, logical: &str) -> Result<Value> {
    let c = out.get(logical).with_context(|| format!("no {logical} yet (the rail-feeds step)"))?;
    let mut v: Value = serde_json::from_slice(&out.path(c).read().with_context(|| format!("read {c}"))?)?;
    for f in v["feeds"].as_array_mut().context("feeds")? {
        if let Some(z) = f.get("zip").and_then(Value::as_str) {
            // (A path the counting program opens: tests/nas_access.rs.)
            f["path"] = Value::from(out.path(z).unchecked_path().to_string_lossy().into_owned());
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue declaration's `own_code` as it was written when it moved out of the code (#140).
    fn own_code() -> OwnCode {
        let one = |c: &str, own: Option<&str>| (c.to_string(), own.map(str::to_string));
        [
            one("FI-01", Some("AX")),
            one("FR-971", Some("GP")),
            one("FR-972", Some("MQ")),
            one("FR-973", Some("GF")),
            one("FR-974", Some("RE")),
            one("FR-976", Some("YT")),
            one("NL-BQ1", Some("BQ")),
            one("NL-BQ2", Some("BQ")),
            one("NL-BQ3", Some("BQ")),
            one("NO-21", Some("SJ")),
            one("NO-22", Some("SJ")),
            one("RS-KM", Some("XK")),
            one("ES-CE", None),
            one("ES-ML", None),
        ]
        .into()
    }

    #[test]
    fn a_territorys_codes() {
        let own = own_code();
        let c = |iso: &str| territory_codes(iso, &own);
        assert_eq!(c("FR"), vec!["FR"]);
        assert_eq!(c("CN-HK"), vec!["HK", "CN"]);
        assert_eq!(c("FR-PM"), vec!["PM", "FR"]);
        assert_eq!(c("FI-01"), vec!["AX", "FI"]);
        assert_eq!(c("FR-971"), vec!["GP", "FR"]);
        assert_eq!(c("RS-KM"), vec!["XK", "RS"], "not Comoros");
        assert_eq!(c("ES-ML"), vec!["ES"], "not Mali");
    }
    use buildstore::recipes::Recipe;

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
        Coverage::fixture(&[Recipe { id: "s".into(), name: "S".into(), outline: vec!["poly:sq.poly".into()] }], None, d)
            .unwrap()
    }

    #[test]
    fn stops_beyond_the_coverage() {
        let d = tempfile::tempdir().unwrap();
        let cov = square_cov(d.path());
        let pairs = [
            pair((0.5, 0.5), (0.6, 0.6), 2, 10.0),
            pair((0.5, 0.5), (1.5, 0.5), 0x80 | 1, 4.0),
            pair((1.5, 0.5), (0.5, 0.5), 2, 3.0),
            pair((2.0, 2.0), (3.0, 3.0), 2, 7.0),
        ]
        .concat();
        let cov = Asking::new(&cov);
        let (kept, beyond) = mark_beyond(&pairs, &cov);
        assert_eq!((kept.len() / PAIR, beyond), (3, 2), "both beyond: left out");
        let modes: Vec<u8> = kept.chunks_exact(PAIR).map(|r| r[16]).collect();
        assert_eq!(modes, vec![2, 0x80 | 1 | BEYOND_B, 2 | BEYOND_A], "the lower bound kept, beyond marked");
        assert_eq!(&kept[PAIR + 17..2 * PAIR], &4.0f32.to_le_bytes(), "trains as they were");
        // Marked again (a coverage that grew): the marks follow the coverage.
        let (again, _) = mark_beyond(&kept, &cov);
        assert_eq!(again, kept);
    }

    /// Task #852: a record read and written again is the same bytes, a NaN's bits too.
    #[test]
    fn a_pair_record_reads_and_writes_the_same_bytes() {
        let mut r: Vec<u8> = Vec::new();
        for x in [2.35f32, 48.85, -0.1, f32::from_bits(0x7fc0_0001)] {
            r.extend(x.to_le_bytes());
        }
        r.push(0x80 | BEYOND_B | 3);
        r.extend(12.5f32.to_le_bytes());
        let mut again = Vec::new();
        for p in Pair::all(&r) {
            p.write(&mut again);
        }
        assert_eq!(again, r);
    }

    #[test]
    fn freq_by_way_id() {
        let way = |id: i64| roadcore::WayRec { id, ..bytemuck::Zeroable::zeroed() };
        let ways = [way(30), way(10), way(20)];
        let rec = |i: u32, f: f32| i.to_le_bytes().into_iter().chain(f.to_le_bytes()).collect::<Vec<u8>>();
        let freq = [rec(0, 5.0), rec(1, -2.5), rec(2, 1.0), rec(7, 9.0)].concat();
        let out = by_way_id(&ways, &freq);
        let got: Vec<(u32, f32)> = out
            .chunks_exact(8)
            .map(|c| (u32::from_le_bytes(c[..4].try_into().unwrap()), f32::from_le_bytes(c[4..].try_into().unwrap())))
            .collect();
        assert_eq!(got, vec![(10, -2.5), (20, 1.0), (30, 5.0)], "sorted by way id; an index past the ways dropped");
    }

    #[test]
    fn country_codes() {
        assert_eq!(alpha2("FR").as_deref(), Some("FR"));
        assert_eq!(alpha2("CN-HK").as_deref(), Some("HK"));
        assert_eq!(alpha2("FR-PM").as_deref(), Some("PM"));
        assert_eq!(alpha2("NO-21"), None);
        assert_eq!(alpha2(""), None);
        // A territory with an ISO 3166-2 code stands for its country too.
        let own = own_code();
        assert_eq!(territory_codes("CN-HK", &own), vec!["HK", "CN"]);
        assert_eq!(territory_codes("FR-PM", &own), vec!["PM", "FR"]);
        assert_eq!(territory_codes("FR", &own), vec!["FR"]);
        assert_eq!(territory_codes("NO-21", &own), vec!["SJ", "NO"], "Svalbard");
        assert_eq!(territory_codes("NO-21", &OwnCode::new()), vec!["NO"], "no own code declared: its country's alone");
        assert!(territory_codes("", &own).is_empty());
    }

    /// Outlines of five territories: A (10° square), its neighbour B, C inside A (and inside A's
    /// outline too, as Hong Kong's is in China's), D, a small island in A's waters with its own
    /// code, and E inside B with an ISO 3166-2 code under B's ("BB-EE", as Hong Kong's "CN-HK").
    fn outlines(d: &Path) -> Outlines {
        let feat = |id: u64, iso: &str, w: f64, s: f64, e: f64, n: f64| {
            let (iso1, iso2) =
                iso.split_once('-').map_or((iso, String::new()), |(_, c)| (c, format!(",\"ISO3166-2\":\"{iso}\"")));
            format!(
                "\u{1e}{{\"type\":\"Feature\",\"geometry\":{{\"type\":\"Polygon\",\"coordinates\":[[[{w},{s}],[{e},{s}],[{e},{n}],[{w},{n}],[{w},{s}]]]}},\"properties\":{{\"@type\":\"relation\",\"@id\":{id},\"boundary\":\"administrative\",\"admin_level\":\"2\",\"name\":\"{iso1}\",\"ISO3166-1\":\"{iso1}\"{iso2}}}}}\n"
            )
        };
        let geo = d.join("o.geojsonseq");
        std::fs::write(
            &geo,
            [
                feat(1, "AA", 0.0, 0.0, 10.0, 10.0),
                feat(2, "BB", 10.0, 0.0, 20.0, 10.0),
                feat(3, "CC", 4.0, 4.0, 5.0, 5.0),
                feat(4, "DD", 9.0, 9.0, 9.2, 9.2),
                feat(5, "BB-EE", 15.0, 1.0, 16.0, 2.0),
            ]
            .concat(),
        )
        .unwrap();
        buildstore::outlines::assemble_geojsonseq(&geo, &d.join("o.sect")).unwrap();
        Outlines::open(&d.join("o.sect")).unwrap()
    }

    #[test]
    fn countries_the_coverage_is_in() {
        let d = tempfile::tempdir().unwrap();
        let o = outlines(d.path());
        let poly = |name: &str, w: f64, s: f64, e: f64, n: f64| {
            std::fs::write(d.path().join(name), format!("p\n1\n {w} {s}\n {e} {s}\n {e} {n}\n {w} {n}\nEND\nEND\n"))
                .unwrap();
            format!("poly:{name}")
        };
        let cov = |outline: Vec<String>| {
            Coverage::fixture(&[Recipe { id: "r".into(), name: "R".into(), outline }], None, d.path()).unwrap()
        };
        let countries = |c: &Coverage, o: &Outlines| countries(c, o, &own_code());
        // Most of A, running 0.2° into B: A's, and C's and D's (each all inside, though a small part
        // of the outline); not B's (a margin).
        assert_eq!(countries(&cov(vec![poly("a.poly", 0.5, 0.5, 10.2, 9.5)]), &o).unwrap(), vec!["AA", "CC", "DD"]);
        // An outline of C alone: C's, not A's, though A's outline holds it too.
        assert_eq!(countries(&cov(vec![poly("c.poly", 4.1, 4.1, 4.9, 4.9)]), &o).unwrap(), vec!["CC"]);
        // Two outlines, one in each: both.
        assert_eq!(
            countries(&cov(vec![poly("a2.poly", 1.0, 1.0, 2.0, 2.0), poly("b.poly", 15.0, 5.0, 16.0, 6.0)]), &o)
                .unwrap(),
            vec!["AA", "BB"]
        );
        // Out at sea: none.
        assert!(countries(&cov(vec![poly("sea.poly", 30.0, 30.0, 31.0, 31.0)]), &o).unwrap().is_empty());
        // An outline of E alone: E's, and B's, whose code E's has (its feeds may be filed under it).
        assert_eq!(countries(&cov(vec![poly("e.poly", 15.1, 1.1, 15.9, 1.9)]), &o).unwrap(), vec!["BB", "EE"]);
    }
}
