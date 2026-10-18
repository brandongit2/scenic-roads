//! Static GTFS compiled by hand (docs/inputs.md §6.7.2): a declare drop box,
//! `inputs/timetables/gtfs/`, of `<name>.zip` files, each a GTFS feed compiled from timetables found
//! anywhere, with a `<name>.toml` description (§5.1) beside it when its sources need a credit line
//! beyond each route's page. The `rail` and `ferries` jobs count the accepted zips (`zips`) with
//! their counters (dem/railgtfs.py, dem/ferrygtfs.py, dem/staticgtfs.py).
//!
//! The shape beyond GTFS: `frequencies.txt` allowed; `calendar.txt`'s dates read as month and day
//! (no expiry, seasons kept); each route's source page in `routes.txt`'s `route_url`, with the
//! extra columns `checked`, `months` and `osm` (the OSM line it's pinned to); and `counts.txt`, for
//! services known only by counts (no departure time invented).
//!
//! The checks:
//! - **Errors:** a file that isn't `<name>.zip` or `<name>.toml` (a name of letters, digits, `-` and
//!   `_`); a description without its credit and licence; a zip that can't be read; a required file
//!   missing (`agency.txt`, `stops.txt`, `routes.txt`, `trips.txt`, `stop_times.txt`,
//!   `calendar.txt`, `feed_info.txt`) or a table without the columns it needs; references broken (a
//!   trip's route or service, a stop time's trip or stop, a frequency's trip, a count row's route or
//!   stops); a time that doesn't parse; a calendar date that isn't a valid month and day; a
//!   `route_type` GTFS doesn't define; a headway not positive; a route without `route_url`; an `osm`
//!   pin that isn't `w<id>` or `r<id>`; a count row's number that isn't one (or `null`), an
//!   `overnight` or `at_least` not 0 or 1; a rail route's count row without its two stops; a ferry
//!   route with count rows and no `osm` pin; a route with both trips and count rows; a stop's
//!   coordinates out of range.
//! - **Warnings:** a route with no trips and no count rows; two zips with one `route_id` of one
//!   agency.
//!
//! Whether a route matches something on the map isn't the input check's (it depends on OSM, which
//! changes each pass): it's #142's report.

use super::{Checks, FileCheck, Finding, Level, Version};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

/// The unit's name.
pub const UNIT: &str = "timetables/gtfs";

const SHAPE: &str = "tg-shape";
const ZIP: &str = "tg-zip";
const TABLE: &str = "tg-table";
const REF: &str = "tg-ref";
const VALUE: &str = "tg-value";
const ROUTE: &str = "tg-route";
const EMPTY: &str = "tg-empty";
const SAME_ROUTE: &str = "tg-same-route";

/// The tables every zip has, with the columns the counters read.
const REQUIRED: [(&str, &[&str]); 7] = [
    ("agency.txt", &["agency_name"]),
    ("stops.txt", &["stop_id", "stop_lat", "stop_lon"]),
    ("routes.txt", &["route_id", "route_type", "route_url"]),
    ("trips.txt", &["route_id", "service_id", "trip_id"]),
    ("stop_times.txt", &["trip_id", "stop_id", "stop_sequence"]),
    (
        "calendar.txt",
        &[
            "service_id",
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
            "start_date",
            "end_date",
        ],
    ),
    ("feed_info.txt", &["feed_publisher_name"]),
];

/// The largest table read (bytes, uncompressed): a hand-compiled file is far smaller.
const MAX_TABLE: u64 = 256 << 20;

/// The static files' checks.
pub struct Gtfs;

/// A name the drop box takes: letters, digits, `-` and `_`, then `.zip` or `.toml`.
fn stem_of(path: &str) -> Option<(&str, &str)> {
    let (stem, ext) = path.rsplit_once('.')?;
    (!stem.is_empty()
        && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && (ext == "zip" || ext == "toml"))
        .then_some((stem, ext))
}

// ---- Reading a zip -------------------------------------------------------------------------------

/// A zip's files (by their names without folders), read whole: stored or deflated entries, no
/// Zip64 (a hand-compiled file is small).
pub fn unzip(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    let u16_at =
        |i: usize| -> Result<u16> { Ok(u16::from_le_bytes(bytes.get(i..i + 2).context("cut short")?.try_into()?)) };
    let u32_at =
        |i: usize| -> Result<u32> { Ok(u32::from_le_bytes(bytes.get(i..i + 4).context("cut short")?.try_into()?)) };
    // The end of the central directory: its signature within the last 64 KiB and 22 bytes.
    let from = bytes.len().saturating_sub(22 + 0xffff);
    let end = (from..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .context("no zip directory (not a zip)")?;
    let n = u16_at(end + 10)? as usize;
    let mut at = u32_at(end + 16)? as usize;
    anyhow::ensure!(at != 0xffff_ffff, "a Zip64 file (not read here)");
    let mut out = BTreeMap::new();
    for _ in 0..n {
        anyhow::ensure!(u32_at(at)? == 0x0201_4b50, "a broken zip directory");
        let method = u16_at(at + 10)?;
        let (csize, usize_) = (u32_at(at + 20)? as usize, u32_at(at + 24)? as u64);
        let (nl, xl, cl) = (u16_at(at + 28)? as usize, u16_at(at + 30)? as usize, u16_at(at + 32)? as usize);
        let local = u32_at(at + 42)? as usize;
        let name = String::from_utf8_lossy(bytes.get(at + 46..at + 46 + nl).context("cut short")?).into_owned();
        at += 46 + nl + xl + cl;
        if name.ends_with('/') {
            continue;
        }
        anyhow::ensure!(u32_at(local)? == 0x0403_4b50, "{name}: a broken zip entry");
        let data_at = local + 30 + u16_at(local + 26)? as usize + u16_at(local + 28)? as usize;
        let raw = bytes.get(data_at..data_at + csize).with_context(|| format!("{name}: cut short"))?;
        anyhow::ensure!(usize_ <= MAX_TABLE, "{name}: larger than {} MB", MAX_TABLE >> 20);
        let data = match method {
            0 => raw.to_vec(),
            8 => {
                let mut v = Vec::with_capacity(usize_ as usize);
                Read::take(flate2::read::DeflateDecoder::new(raw), MAX_TABLE + 1)
                    .read_to_end(&mut v)
                    .with_context(|| format!("{name}: can't be inflated"))?;
                v
            }
            m => bail!("{name}: compression method {m} (only stored and deflated are read)"),
        };
        anyhow::ensure!(
            data.len() as u64 == usize_,
            "{name}: {} bytes, not the {usize_} its directory says",
            data.len()
        );
        let base = name.rsplit('/').next().unwrap_or(&name).to_string();
        out.insert(base, data);
    }
    Ok(out)
}

/// A CSV table (RFC 4180, a byte-order mark dropped): its header (each name trimmed) and rows.
pub fn csv(bytes: &[u8]) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let text = std::str::from_utf8(bytes).context("not UTF-8")?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let (mut row, mut field) = (Vec::new(), String::new());
    let (mut quoted, mut was_quoted) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !was_quoted => {
                quoted = true;
                was_quoted = true;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                was_quoted = false;
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    anyhow::ensure!(!quoted, "a quoted field isn't closed");
    if !field.is_empty() || was_quoted || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    if rows.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let head = rows.remove(0).into_iter().map(|h| h.trim().to_string()).collect();
    Ok((head, rows))
}

/// A table as rows of named cells.
struct Table {
    rows: Vec<BTreeMap<String, String>>,
}

impl Table {
    fn parse(bytes: &[u8]) -> Result<(Table, Vec<String>)> {
        let (head, rows) = csv(bytes)?;
        let rows = rows
            .into_iter()
            .map(|r| head.iter().cloned().zip(r.into_iter().chain(std::iter::repeat(String::new()))).collect())
            .collect();
        Ok((Table { rows }, head))
    }
}

fn cell<'a>(r: &'a BTreeMap<String, String>, k: &str) -> &'a str {
    r.get(k).map(|s| s.trim()).unwrap_or("")
}

/// A GTFS time, `H:MM:SS` (hours past 24 allowed).
fn valid_time(s: &str) -> bool {
    let p: Vec<&str> = s.split(':').collect();
    p.len() == 3
        && !p[0].is_empty()
        && p[0].len() <= 3
        && p.iter().all(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()))
        && p[1].len() == 2
        && p[2].len() == 2
        && p[1] < "60"
        && p[2] < "60"
}

/// A calendar date read as month and day: `YYYYMMDD`, the month and day a valid pair (29 February
/// included), the year any.
fn valid_month_day(s: &str) -> bool {
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let (m, d): (u32, u32) = (s[4..6].parse().unwrap_or(0), s[6..8].parse().unwrap_or(0));
    let days = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1]
}

/// A route type GTFS defines: the basic ones and the extended ones' ranges.
fn valid_route_type(s: &str) -> bool {
    s.parse::<u32>().is_ok_and(|t| matches!(t, 0..=7 | 11 | 12 | 100..=117 | 200..=209 | 400..=405 | 700..=716 | 800 | 900..=906 | 1000 | 1100 | 1200 | 1300..=1307 | 1400 | 1500..=1507 | 1700..=1702))
}

/// A ferry route by its type (dem/ferrygtfs.py's `is_ferry`).
fn is_ferry(t: &str) -> bool {
    t.parse::<u32>().is_ok_and(|t| t == 4 || (1000..=1099).contains(&t) || (1200..=1299).contains(&t))
}

/// A rail route by its type (dem/railgtfs.py's `mode_of`).
fn is_rail(t: &str) -> bool {
    t.parse::<u32>().is_ok_and(|t| matches!(t, 0 | 1 | 2 | 5 | 7 | 12 | 100..=199 | 400..=499 | 900..=999 | 1400))
}

/// A number as `counts.txt` holds it (or `null`: the source gives none).
fn valid_number(s: &str) -> bool {
    s == "null" || s.parse::<f64>().is_ok_and(f64::is_finite)
}

/// An OSM pin: `w<id>` or `r<id>`.
fn valid_pin(s: &str) -> bool {
    (s.starts_with('w') || s.starts_with('r')) && s.len() > 1 && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// What one zip says: its findings and what's read from it (routes by id with their agency).
struct Zipped {
    findings: Vec<Finding>,
    routes: Vec<(String, String)>,
    facts: serde_json::Map<String, serde_json::Value>,
}

fn check_zip(path: &str, bytes: &[u8]) -> Zipped {
    let mut r = Zipped { findings: Vec::new(), routes: Vec::new(), facts: serde_json::Map::new() };
    let mut err = |check: &str, what: &[&str], msg: String| {
        r.findings.push(Finding::new(
            check,
            Level::Error,
            &[path],
            what,
            vec![path.into()],
            format!("{path}: {msg} (docs/inputs.md §6.7.2)"),
        ))
    };
    let files = match unzip(bytes) {
        Ok(f) => f,
        Err(e) => {
            err(ZIP, &["zip"], format!("can't be read as a zip: {e:#}"));
            return r;
        }
    };
    let mut tables: BTreeMap<&str, Table> = BTreeMap::new();
    let mut missing = false;
    for (name, cols) in REQUIRED.iter().copied().chain([
        ("frequencies.txt", &["trip_id", "start_time", "end_time", "headway_secs"][..]),
        ("counts.txt", &["route_id"][..]),
    ]) {
        let Some(b) = files.get(name) else {
            if !matches!(name, "frequencies.txt" | "counts.txt") {
                err(TABLE, &[name, "missing"], format!("no {name} (a required file)"));
                missing = true;
            }
            continue;
        };
        match Table::parse(b) {
            Ok((t, head)) => {
                let lacking: Vec<&str> = cols.iter().copied().filter(|c| !head.iter().any(|h| h == c)).collect();
                if !lacking.is_empty() {
                    err(TABLE, &[name, &lacking.join(",")], format!("{name} has no column {}", lacking.join(", ")));
                    missing = true;
                }
                tables.insert(name, t);
            }
            Err(e) => {
                err(TABLE, &[name, "csv"], format!("{name} isn't a CSV table: {e:#}"));
                missing = true;
            }
        }
    }
    if missing {
        return r;
    }
    let t = |n: &str| &tables[n].rows;
    // Stops.
    let mut stops = BTreeSet::new();
    for s in t("stops.txt") {
        let id = cell(s, "stop_id");
        stops.insert(id.to_string());
        let (lat, lon) = (cell(s, "stop_lat").parse::<f64>(), cell(s, "stop_lon").parse::<f64>());
        if !matches!((lat, lon), (Ok(a), Ok(o)) if (-90.0..=90.0).contains(&a) && (-180.0..=180.0).contains(&o)) {
            err(VALUE, &["stop", id], format!("stop {id:?}: its coordinates are out of range or not numbers"));
        }
    }
    // Calendar.
    let mut services = BTreeSet::new();
    for c in t("calendar.txt") {
        let id = cell(c, "service_id");
        services.insert(id.to_string());
        for k in ["start_date", "end_date"] {
            if !valid_month_day(cell(c, k)) {
                err(
                    VALUE,
                    &["calendar", id, k],
                    format!(
                        "service {id:?}: {k} {:?} isn't a valid month and day (YYYYMMDD, the year ignored)",
                        cell(c, k)
                    ),
                );
            }
        }
    }
    // Routes.
    let mut routes: BTreeMap<String, (String, String, String)> = BTreeMap::new(); // id → (type, agency, pin)
    for x in t("routes.txt") {
        let id = cell(x, "route_id");
        let ty = cell(x, "route_type");
        if !valid_route_type(ty) {
            err(VALUE, &["route_type", id, ty], format!("route {id:?}: route_type {ty:?} isn't one GTFS defines"));
        }
        if cell(x, "route_url").is_empty() {
            err(
                ROUTE,
                &["route_url", id],
                format!("route {id:?} has no route_url (each route's source page is required)"),
            );
        }
        let pin = cell(x, "osm");
        if !pin.is_empty() && !valid_pin(pin) {
            err(VALUE, &["osm", id, pin], format!("route {id:?}: osm {pin:?} isn't w<id> or r<id>"));
        }
        routes.insert(id.to_string(), (ty.to_string(), cell(x, "agency_id").to_string(), pin.to_string()));
        r.routes.push((id.to_string(), cell(x, "agency_id").to_string()));
    }
    // Trips, stop times, frequencies.
    let mut trips = BTreeSet::new();
    let mut timed: BTreeSet<String> = BTreeSet::new();
    for x in t("trips.txt") {
        let (id, route, svc) = (cell(x, "trip_id"), cell(x, "route_id"), cell(x, "service_id"));
        trips.insert(id.to_string());
        if !routes.contains_key(route) {
            err(REF, &["trip route", id, route], format!("trip {id:?} names route {route:?}, which routes.txt hasn't"));
        } else {
            timed.insert(route.to_string());
        }
        if !services.contains(svc) {
            err(
                REF,
                &["trip service", id, svc],
                format!("trip {id:?} names service {svc:?}, which calendar.txt hasn't"),
            );
        }
    }
    for (i, x) in t("stop_times.txt").iter().enumerate() {
        let (trip, stop) = (cell(x, "trip_id"), cell(x, "stop_id"));
        if !trips.contains(trip) || !stops.contains(stop) {
            err(
                REF,
                &["stop time", trip, stop],
                format!("stop_times.txt row {}: trip {trip:?} or stop {stop:?} isn't there", i + 2),
            );
        }
        for k in ["arrival_time", "departure_time"] {
            let v = cell(x, k);
            if !v.is_empty() && !valid_time(v) {
                err(
                    VALUE,
                    &["time", trip, k, v],
                    format!("stop_times.txt row {}: {k} {v:?} isn't a time (H:MM:SS)", i + 2),
                );
            }
        }
        if cell(x, "stop_sequence").parse::<u32>().is_err() {
            err(
                VALUE,
                &["stop_sequence", trip, cell(x, "stop_sequence")],
                format!("stop_times.txt row {}: stop_sequence isn't a whole number", i + 2),
            );
        }
    }
    for x in tables.get("frequencies.txt").map(|t| &t.rows[..]).unwrap_or_default() {
        let trip = cell(x, "trip_id");
        if !trips.contains(trip) {
            err(REF, &["frequency trip", trip], format!("frequencies.txt names trip {trip:?}, which trips.txt hasn't"));
        }
        for k in ["start_time", "end_time"] {
            if !valid_time(cell(x, k)) {
                err(
                    VALUE,
                    &["frequency time", trip, k],
                    format!("frequencies.txt, trip {trip:?}: {k} {:?} isn't a time", cell(x, k)),
                );
            }
        }
        if !cell(x, "headway_secs").parse::<f64>().is_ok_and(|h| h > 0.0) {
            err(VALUE, &["headway_secs", trip], format!("frequencies.txt, trip {trip:?}: headway_secs isn't positive"));
        }
    }
    // Count rows.
    let mut counted: BTreeSet<String> = BTreeSet::new();
    let counts = tables.get("counts.txt").map(|t| &t.rows[..]).unwrap_or_default();
    for (i, x) in counts.iter().enumerate() {
        let row = i + 2;
        let route = cell(x, "route_id");
        let Some((ty, _, pin)) = routes.get(route) else {
            err(REF, &["count route", route], format!("counts.txt row {row}: route {route:?} isn't in routes.txt"));
            continue;
        };
        counted.insert(route.to_string());
        for k in ["per_day", "per_week", "per_day_low"] {
            let v = cell(x, k);
            if !v.is_empty() && !valid_number(v) {
                err(VALUE, &["count number", route, k, v], format!("counts.txt row {row}: {k} {v:?} isn't a number"));
            }
        }
        let hw = cell(x, "headway");
        if hw.parse::<f64>().is_ok_and(|h| h.is_nan() || h <= 0.0) {
            err(VALUE, &["count headway", route, hw], format!("counts.txt row {row}: headway {hw} isn't positive"));
        }
        for k in ["overnight", "at_least"] {
            let v = cell(x, k);
            if !matches!(v, "" | "0" | "1") {
                err(VALUE, &["count flag", route, k, v], format!("counts.txt row {row}: {k} {v:?} isn't 0 or 1"));
            }
        }
        if is_rail(ty) {
            let (a, b) = (cell(x, "from_stop_id"), cell(x, "to_stop_id"));
            if !stops.contains(a) || !stops.contains(b) {
                err(REF, &["count stops", route, a, b], format!("counts.txt row {row}: a rail route's row needs its two stops, from_stop_id {a:?} and to_stop_id {b:?}, in stops.txt"));
            }
        }
        if is_ferry(ty) && pin.is_empty() {
            err(
                ROUTE,
                &["count pin", route],
                format!(
                    "ferry route {route:?} is counted in counts.txt but has no osm pin (it has no stops to match by)"
                ),
            );
        }
    }
    let both: Vec<&String> = counted.intersection(&timed).collect();
    for route in &both {
        err(
            ROUTE,
            &["trips and counts", route],
            format!("route {route:?} has both trips and count rows (a service is counted one way)"),
        );
    }
    let idle: Vec<&String> = routes.keys().filter(|k| !counted.contains(*k) && !timed.contains(*k)).collect();
    if !idle.is_empty() {
        let refs: Vec<&str> = idle.iter().map(|s| s.as_str()).collect();
        r.findings.push(Finding::new(
            EMPTY,
            Level::Warning,
            &[path],
            &refs,
            vec![path.into()],
            format!("{path}: routes with no trips and no count rows: {} (docs/inputs.md §6.7.2)", refs.join(", ")),
        ));
    }
    r.facts.insert("routes".into(), routes.len().into());
    r.facts.insert("trips".into(), trips.len().into());
    r.facts.insert("count_rows".into(), counts.len().into());
    r
}

impl Checks for Gtfs {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        match stem_of(path) {
            None => c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not a static file"], vec![path.into()], format!("{path} isn't the drop box's shape: timetables/gtfs takes <name>.zip static GTFS files and their <name>.toml descriptions (a name of letters, digits, - and _: docs/inputs.md §6.7.2); notes go in how/"))),
            Some((_, "toml")) => {
                let (d, mut fs, _) = super::credits::parse_description(path, bytes);
                c.findings.append(&mut fs);
                if let Some(d) = d {
                    c.facts.insert("description".into(), serde_json::to_value(d).unwrap_or_default());
                }
            }
            Some(_) => {
                let r = check_zip(path, bytes);
                c.findings = r.findings;
                c.facts = r.facts;
            }
        }
        c
    }

    fn partners(&self, path: &str, all: &BTreeSet<String>) -> Vec<String> {
        let Some((stem, ext)) = stem_of(path) else { return Vec::new() };
        let other = format!("{stem}.{}", if ext == "zip" { "toml" } else { "zip" });
        all.contains(&other).then_some(other).into_iter().collect()
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut by_route: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for p in v.paths.iter().filter(|p| p.ends_with(".zip")) {
            for key in check_zip(p, &(v.read)(p)?).routes {
                by_route.entry(key).or_default().push(p.clone());
            }
        }
        let mut out = Vec::new();
        for ((route, agency), mut ps) in by_route {
            ps.dedup();
            if ps.len() > 1 {
                let refs: Vec<&str> = ps.iter().map(String::as_str).collect();
                out.push(Finding::new(
                    SAME_ROUTE,
                    Level::Warning,
                    &refs,
                    &[&route, &agency],
                    ps.clone(),
                    format!(
                        "{} each have route {route:?} of agency {agency:?}: one service in two files counts twice",
                        ps.join(", ")
                    ),
                ));
            }
        }
        Ok(out)
    }
}

/// The accepted zips, in their drop-box paths' order: (path, checked copy) — what the counters read.
pub fn zips(a: &super::Accepted) -> Vec<(String, std::path::PathBuf)> {
    a.files().filter(|(p, _)| p.ends_with(".zip")).map(|(p, c)| (p.clone(), c)).collect()
}

/// The counters' list of the accepted zips (`{"zips": [{name, path}]}`, dem/railgtfs.py's and
/// dem/ferries.py's `--static`).
pub fn zips_json(a: &super::Accepted) -> serde_json::Value {
    serde_json::json!({"zips": zips(a).into_iter().map(|(p, c)| serde_json::json!({"name": p, "path": c})).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests;
