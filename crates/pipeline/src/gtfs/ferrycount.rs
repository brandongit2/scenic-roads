//! The ferry counter (docs/inputs.md §6.7.0): ferry sailings per OSM ferry line from timetable
//! feeds (GTFS), for the `ferries` step (crate::ferries), which runs it on the declared feeds the
//! `rail-feeds` step fetched (docs/inputs.md §6.7.1), each zip with the day it was fetched.
//!
//! For every ferry line (its two ends), counts the timetabled ferry trips that call at both of its
//! ports, i.e. stop within 400 m (at most 30 % of the distance between them) of each end of the
//! line. Per day of service: departures each way (both directions averaged). Reported per line:
//!   per_day      median over the days the line runs; lines not sailing both ways most days: the
//!                weekly count ÷ 7 (a weekly crossing reads "1 a week"),
//!   per_day_low  median in the quietest month (feeds covering a whole year only),
//!   headway      median gap between departures from the first port, 07:00–19:00, on a typical day,
//!   days         weekdays with service (e.g. "daily", "Mo–Fr"),
//!   months       months with service over the 12 months from the feed's day, when the line's
//!                timetable breaks for a month or more (seasonal); a timetable that just ends says
//!                nothing about the season.
//! The reference day is the day the feed's zip was fetched, so a zip gives the same counts whenever
//! it's read. Only service dates from a year before it to 400 days after count (a feed with none is
//! stale). Only ferry routes are counted (route_type 4, 1000–1099, 1200–1299, or the route ids the
//! declaration lists in `ferry_routes` when the feed labels its ferries as something else).
//!
//! A static file compiled by hand (docs/inputs.md §6.7.2, `static_`) is counted the same way over
//! one reference year, its calendar read as months and days (crate::gtfs::statics), its days from 1
//! January of that year; its records are looked-up timetables' (`kind` "timetable"), each with the
//! source its first matched route names (`route_long_name`, `route_url`, `checked`). A route pinned
//! to an OSM line (`osm`) counts for that line alone, its ports the calls nearest the line's two
//! ends, however far.
//!
//! Distances are the Python's: `math.hypot` as CPython computes it (`hypot`), `cos` this Mac's.

use super::{statics, Cell, Date, OwnedRow, PyErr, Zip, R};
use crate::interest::py_round;
use crate::pyjson::Py;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

const R_KM: f64 = 0.4;
const DAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `math.hypot(x, y)` as CPython 3.12 on computes it (`vector_norm`: exact squares and sums, one
/// correction), so distances are the Python's to the bit.
pub fn hypot(x: f64, y: f64) -> f64 {
    let (x, y) = (x.abs(), y.abs());
    let max = x.max(y);
    if max.is_infinite() {
        return max;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if max == 0.0 {
        return max;
    }
    norm2(x, y, max)
}

fn norm2(x: f64, y: f64, max: f64) -> f64 {
    let (_, max_e) = frexp(max);
    if max_e < -1023 {
        return f64::MIN_POSITIVE * norm2(x / f64::MIN_POSITIVE, y / f64::MIN_POSITIVE, max / f64::MIN_POSITIVE);
    }
    let scale = ldexp(1.0, -max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for v in [x, y] {
        let v = v * scale;
        let (hi, lo) = dl_mul(v, v);
        let (s_hi, s_lo) = dl_fast_sum(csum, hi);
        csum = s_hi;
        frac1 += lo;
        frac2 += s_lo;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let (hi, lo) = dl_mul(-h, h);
    let (s_hi, s_lo) = dl_fast_sum(csum, hi);
    csum = s_hi;
    frac1 += lo;
    frac2 += s_lo;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let z = x - a;
    (x, b - z)
}

/// C's `frexp`: (m, e) with x = m × 2^e, 0.5 ≤ |m| < 1.
fn frexp(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        let (m, e) = frexp(x * 2f64.powi(54));
        return (m, e - 54);
    }
    let m = f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52));
    (m, exp - 1022)
}

fn ldexp(x: f64, e: i32) -> f64 {
    x * 2f64.powi(e)
}

/// Kilometres between two points, as the Python's equirectangular formula.
pub fn km(a: [f64; 2], b: [f64; 2]) -> f64 {
    let k = ((a[1] + b[1]) / 2.0).to_radians().cos();
    hypot((a[0] - b[0]) * k, a[1] - b[1]) * 111.195
}

/// A ferry route by its type.
pub fn is_ferry(route_type: &str) -> bool {
    super::py_int(route_type).is_some_and(|t| t == 4 || (1000..=1099).contains(&t) || (1200..=1299).contains(&t))
}

/// `h:m:s` in seconds (a ValueError for anything else; an empty time is midnight).
fn secs(t: &str) -> R<i64> {
    let t = crate::pytext::strip(t);
    let t = if t.is_empty() { "0:0:0" } else { t };
    let p: Vec<&str> = t.split(':').collect();
    let bad = || PyErr::value(format!("not a time: {}", super::py_repr(t)));
    if p.len() != 3 {
        return Err(bad());
    }
    let n = |s: &str| super::py_int(s).ok_or_else(bad);
    Ok(n(p[0])? * 3600 + n(p[1])? * 60 + n(p[2])?)
}

fn days_text(active: &BTreeSet<i64>) -> String {
    if active.len() == 7 {
        return "daily".into();
    }
    let mut runs: Vec<Vec<i64>> = Vec::new();
    let mut cur = Vec::new();
    for d in 0..7 {
        if active.contains(&d) {
            cur.push(d);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs.iter()
        .map(|r| {
            if r.len() == 1 {
                DAYS[r[0] as usize].to_string()
            } else {
                format!("{}–{}", DAYS[r[0] as usize], DAYS[r[r.len() - 1] as usize])
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn months_text(ms: &BTreeSet<i64>) -> String {
    // Contiguous runs, wrapping over the new year (e.g. Nov–Mar).
    if ms.len() == 12 {
        return "all year".into();
    }
    let Some(start) = (0..12).find(|m| ms.contains(m) && !ms.contains(&((m + 11) % 12))) else {
        return "all year".into();
    };
    let (mut runs, mut m, mut cur): (Vec<Vec<i64>>, i64, Vec<i64>) = (Vec::new(), start, Vec::new());
    for _ in 0..12 {
        if ms.contains(&m) {
            cur.push(m);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
        m = (m + 1) % 12;
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs.iter()
        .map(|r| {
            if r.len() == 1 {
                MONTHS[r[0] as usize].to_string()
            } else {
                format!("{}–{}", MONTHS[r[0] as usize], MONTHS[r[r.len() - 1] as usize])
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `statistics.median` of numbers.
fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// A feed as the counter reads it: its id, name, URL, the page about it and its `ferry_routes`.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub id: String,
    pub name: Option<String>,
    pub url: String,
    pub source: Option<String>,
    pub ferry_routes: Vec<String>,
}

impl Feed {
    /// A feed of a list (serde JSON: the rail-feeds step's ferry list, a declaration's facts).
    pub fn of(v: &serde_json::Value) -> Feed {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
        Feed {
            id: s("id").unwrap_or_default(),
            name: s("name"),
            url: v.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            source: s("source"),
            ferry_routes: v
                .get("ferry_routes")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        }
    }
}

/// `d.get(k, "")`: None for a missing cell (Python's None), "" for no such column.
fn get_default(c: Cell<'_>) -> Option<&str> {
    match c {
        Cell::Absent => Some(""),
        Cell::None => None,
        Cell::Str(s) => Some(s),
    }
}

fn py_opt(s: Option<&str>) -> Py {
    s.map_or(Py::Null, |s| Py::Str(s.to_string()))
}

/// The feed's ferry routes, by id (in their order): by their type, or listed in its `ferry_routes`.
fn ferry_routes(z: &Zip, feed: &Feed) -> R<Vec<(String, OwnedRow)>> {
    let extra: HashSet<&str> = feed.ferry_routes.iter().map(String::as_str).collect();
    let mut out: Vec<(String, OwnedRow)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut rows = z.dict_rows("routes.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let r = rows.row(cells);
        let t = get_default(r.cell("route_type")).ok_or_else(PyErr::none)?;
        if !(is_ferry(t) || extra.contains(r.cell("route_id").req("route_id")?)) {
            continue;
        }
        let id = r.cell("route_id").req("route_id")?.to_string();
        let row = r.to_map();
        match at.get(&id) {
            Some(&i) => out[i].1 = row,
            None => {
                at.insert(id.clone(), out.len());
                out.push((id, row));
            }
        }
    }
    Ok(out)
}

/// A trip: its route and service (None: a missing cell).
struct Trip {
    route: String,
    service: Option<String>,
}

fn ferry_trips(z: &Zip, routes: &HashMap<&str, &OwnedRow>) -> R<HashMap<String, Trip>> {
    let mut trips = HashMap::new();
    let mut rows = z.dict_rows("trips.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let t = rows.row(cells);
        let route = match t.cell("route_id") {
            Cell::Absent => return Err(PyErr::key("route_id")),
            Cell::None => continue,
            Cell::Str(r) => r,
        };
        if !routes.contains_key(route) {
            continue;
        }
        let tid = t.cell("trip_id").req("trip_id")?.to_string();
        let service = match t.cell("service_id") {
            Cell::Str(s) => Some(s.to_string()),
            Cell::None => None,
            // (`t["service_id"]` is asked of every trip once its dates are.)
            Cell::Absent => return Err(PyErr::key("service_id")),
        };
        trips.insert(tid, Trip { route: route.to_string(), service });
    }
    Ok(trips)
}

/// Each service's dates (calendar.txt, then calendar_dates.txt), from a year before `day` to 400
/// days after.
fn service_dates(z: &Zip, day: Date) -> R<HashMap<String, BTreeSet<Date>>> {
    let (lo, hi) = (day.plus(-365), day.plus(400));
    let mut dates: HashMap<String, BTreeSet<Date>> = HashMap::new();
    const WD: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    let mut rows = z.dict_rows("calendar.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let c = rows.row(cells);
        let (d, end) =
            (super::ymd(c.cell("start_date").req("start_date")?)?, super::ymd(c.cell("end_date").req("end_date")?)?);
        let mut wd = [false; 7];
        for (k, n) in WD.iter().enumerate() {
            wd[k] = match c.cell(n) {
                Cell::Absent => return Err(PyErr::key(n)),
                Cell::None => false,
                Cell::Str(v) => v == "1",
            };
        }
        // (Only the window's dates are kept: the rest aren't walked, but a service id missing where
        // a day would be added fails the feed as before.)
        let any_day = (0..7).any(|k| d.0 + k <= end.0 && wd[d.plus(k as i64).weekday() as usize]);
        let sid = match c.cell("service_id") {
            Cell::Str(s) => s.to_string(),
            _ if any_day => return Err(PyErr::key("service_id")),
            _ => continue,
        };
        let (mut a, b) = (d.max(lo), end.min(hi));
        while a <= b {
            if wd[a.weekday() as usize] {
                dates.entry(sid.clone()).or_default().insert(a);
            }
            a = a.plus(1);
        }
        if any_day {
            dates.entry(sid).or_default();
        }
    }
    let mut rows = z.dict_rows("calendar_dates.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let c = rows.row(cells);
        let d = super::ymd(c.cell("date").req("date")?)?;
        let add = match c.cell("exception_type") {
            Cell::Absent => return Err(PyErr::key("exception_type")),
            Cell::None => false,
            Cell::Str(v) => v == "1",
        };
        let set = dates.entry(c.cell("service_id").req("service_id")?.to_string()).or_default();
        if !(lo <= d && d <= hi) {
            continue;
        }
        if add {
            set.insert(d);
        } else {
            set.remove(&d);
        }
    }
    Ok(dates)
}

/// Whether a feed's zip has ferry service around `day` (rail-feeds' check of a zip it has).
pub fn has_service(zpath: &Path, feed: &Feed, day: Date) -> R<bool> {
    let z = Zip::open(zpath)?;
    let routes = ferry_routes(&z, feed)?;
    let by_id: HashMap<&str, &OwnedRow> = routes.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let trips = ferry_trips(&z, &by_id)?;
    let dates = service_dates(&z, day)?;
    Ok(trips.values().any(|t| t.service.as_ref().and_then(|s| dates.get(s)).is_some_and(|d| !d.is_empty())))
}

/// A timetable route that matches nothing on the map (docs/inputs.md §6.7.3).
pub fn unmatched_route(source: &str, static_: bool, r: &OwnedRow, agency: Py, pin: &str, why: &str) -> Py {
    let s = |x: &str| Py::Str(x.to_string());
    let name = [r.cell("route_short_name").get(), r.cell("route_long_name").get()]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .unwrap_or("");
    Py::Dict(vec![
        ("mode".into(), s("ferry")),
        ("source".into(), s(source)),
        ("static".into(), Py::Bool(static_)),
        ("route".into(), py_opt(get_default(r.cell("route_id")))),
        ("name".into(), s(name)),
        ("agency".into(), agency),
        ("osm".into(), s(pin)),
        ("why".into(), s(why)),
    ])
}

/// The records of the lines of `lines` (id, its two ends) a feed's ferries sail, counted from
/// `day`, the day its zip was fetched (a static file, `day` None: see the module's docs).
/// `unmatched`, when given, gets the routes with service in the window that match no line
/// (`unmatched_route`).
pub fn process(
    feed: &Feed,
    zpath: &Path,
    lines: &[(String, [[f64; 2]; 2])],
    day: Option<Date>,
    unmatched: Option<&mut Vec<Py>>,
) -> R<Vec<Py>> {
    let static_ = day.is_none();
    let z = Zip::open(zpath)?;
    let today = day.unwrap_or_else(|| Date::new(statics::REF_YEAR, 1, 1).unwrap());
    let routes_v = ferry_routes(&z, feed)?;
    if routes_v.is_empty() {
        eprintln!("  {}: no ferry routes", feed.id);
        return Ok(Vec::new());
    }
    let routes: HashMap<&str, &OwnedRow> = routes_v.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let trips = ferry_trips(&z, &routes)?;
    let mut agencies: HashMap<Option<String>, Option<String>> = HashMap::new();
    let mut rows = z.dict_rows("agency.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let a = rows.row(cells);
        agencies.insert(
            get_default(a.cell("agency_id")).map(str::to_string),
            get_default(a.cell("agency_name")).map(str::to_string),
        );
    }
    let agency_of = |r: &OwnedRow| -> Py {
        let k = get_default(r.cell("agency_id")).map(str::to_string);
        match agencies.get(&k) {
            Some(n) => py_opt(n.as_deref()),
            None => Py::Str(String::new()),
        }
    };
    let mut stops: HashMap<String, [f64; 2]> = HashMap::new();
    let mut rows = z.dict_rows("stops.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let s = rows.row(cells);
        let ll = (|| -> R<Option<[f64; 2]>> {
            let (Cell::Str(x), Cell::Str(y)) = (s.cell("stop_lon"), s.cell("stop_lat")) else {
                if matches!(s.cell("stop_lon"), Cell::None) || matches!(s.cell("stop_lat"), Cell::None) {
                    return Err(PyErr::none());
                }
                return Ok(None);
            };
            Ok(super::py_float(x).zip(super::py_float(y)).map(|(x, y)| [x, y]))
        })()?;
        // (A stop without its id is passed over: the Python's KeyError fell in its try.)
        if let (Some(ll), Cell::Str(id)) = (ll, s.cell("stop_id")) {
            stops.insert(id.to_string(), ll);
        }
    }
    // trip → (sequence, stop, departure s), in the order trips first appear.
    let mut calls: super::railcount::Ordered<String, (String, Vec<(i64, String, i64)>)> = Default::default();
    let mut rows = z.dict_rows("stop_times.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let st = rows.row(cells);
        let tid = match st.cell("trip_id") {
            Cell::Absent => return Err(PyErr::key("trip_id")),
            Cell::None => continue,
            Cell::Str(t) => t,
        };
        if !trips.contains_key(tid) {
            continue;
        }
        let t = [st.cell("departure_time").get(), st.cell("arrival_time").get()]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .unwrap_or("");
        let seq_text = st.cell("stop_sequence").req("stop_sequence")?;
        let seq = super::py_int(seq_text).ok_or_else(|| {
            PyErr::value(format!("invalid literal for int() with base 10: {}", super::py_repr(seq_text)))
        })?;
        let stop = st.cell("stop_id").req("stop_id")?.to_string();
        let secs = if t.is_empty() { -1 } else { secs(t)? };
        calls.entry(tid.to_string(), || (tid.to_string(), Vec::new())).1.push((seq, stop, secs));
    }
    for (_, c) in calls.items.iter_mut() {
        c.sort();
    }
    // Service dates: the current timetable only, from a year before the feed's day to a year and a
    // bit after. A feed with nothing in that window is stale (its lines are left to the looked-up
    // timetables).
    let lo = today.plus(-365);
    let dates: HashMap<String, BTreeSet<Date>> = if static_ {
        statics::service_dates(&z)?.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect()
    } else {
        service_dates(&z, today)?
    };
    let no_dates = BTreeSet::new();
    let dates_of = |t: &Trip| t.service.as_ref().and_then(|s| dates.get(s)).unwrap_or(&no_dates);
    let pins: HashMap<&str, String> = routes_v
        .iter()
        .filter(|_| static_)
        .filter_map(|(rid, r)| {
            let p = crate::pytext::strip(r.cell("osm").or_empty());
            (!p.is_empty()).then(|| (rid.as_str(), p.to_string()))
        })
        .collect();
    // Headway-based trips: departures per trip run.
    let mut runs: HashMap<String, Vec<i64>> = HashMap::new();
    let mut rows = z.dict_rows("frequencies.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let fr = rows.row(cells);
        let tid = match fr.cell("trip_id") {
            Cell::Absent => return Err(PyErr::key("trip_id")),
            Cell::None => continue,
            Cell::Str(t) => t,
        };
        if !trips.contains_key(tid) {
            continue;
        }
        let (a, b) = (secs(fr.cell("start_time").req("start_time")?)?, secs(fr.cell("end_time").req("end_time")?)?);
        let hw_text = fr.cell("headway_secs").req("headway_secs")?;
        let hw = super::py_int(hw_text).ok_or_else(|| {
            PyErr::value(format!("invalid literal for int() with base 10: {}", super::py_repr(hw_text)))
        })?;
        let step = hw.max(60);
        let v = runs.entry(tid.to_string()).or_default();
        let mut x = a;
        while x < b {
            v.push(x);
            x += step;
        }
    }
    let all_dates: BTreeSet<Date> = trips.values().flat_map(|t| dates_of(t).iter().copied()).collect();
    let (Some(&first_date), Some(&last_date)) = (all_dates.first(), all_dates.last()) else {
        if static_ {
            eprintln!("  {}: no timed sailings", feed.id);
        } else {
            eprintln!("  {}: stale (no service since {})", feed.id, lo.iso());
        }
        return Ok(Vec::new());
    };
    let feed_name = feed.name.clone().unwrap_or_else(|| feed.id.clone());

    // Only stops ferries call at, and lines with both ports near one.
    let fstops: Vec<(&str, [f64; 2])> = {
        let mut seen = HashSet::new();
        let mut v = Vec::new();
        for (_, c) in &calls.items {
            for (_, s, _) in c {
                if let Some(&p) = stops.get(s) {
                    if seen.insert(s.as_str()) {
                        v.push((s.as_str(), p));
                    }
                }
            }
        }
        v
    };
    if fstops.is_empty() {
        return Ok(Vec::new());
    }
    let fold = |f: fn(f64, f64) -> f64, i: usize| fstops.iter().map(|(_, p)| p[i]).reduce(f).unwrap();
    let bx = [fold(f64::min, 0) - 0.02, fold(f64::min, 1) - 0.02, fold(f64::max, 0) + 0.02, fold(f64::max, 1) + 0.02];
    let inside = |p: [f64; 2]| bx[0] <= p[0] && p[0] <= bx[2] && bx[1] <= p[1] && p[1] <= bx[3];

    let mut out = Vec::new();
    let pinned_lines: HashSet<&str> = pins.values().map(String::as_str).collect();
    let mut all_matched: BTreeSet<String> = BTreeSet::new();
    for (lid, [a, b]) in lines {
        let (a, b) = (*a, *b);
        let sep = km(a, b);
        if sep < 0.05 {
            continue; // round trips: no two ports to match
        }
        let (near_a, near_b): (HashSet<&str>, HashSet<&str>) = if pinned_lines.contains(lid.as_str()) {
            (HashSet::new(), HashSet::new())
        } else {
            if !(inside(a) && inside(b)) {
                continue;
            }
            let r = R_KM.min(sep * 0.3);
            let na: HashSet<&str> = fstops.iter().filter(|(_, p)| km(*p, a) <= r).map(|(s, _)| *s).collect();
            let nb: HashSet<&str> = fstops.iter().filter(|(_, p)| km(*p, b) <= r).map(|(s, _)| *s).collect();
            if na.is_empty() || nb.is_empty() {
                continue;
            }
            (na, nb)
        };
        let mut per_date: [BTreeMap<Date, i64>; 2] = [BTreeMap::new(), BTreeMap::new()];
        let mut deps_a: HashMap<Date, Vec<i64>> = HashMap::new();
        let mut matched: BTreeSet<&str> = BTreeSet::new();
        for (tid, c) in &calls.items {
            let trip = &trips[tid.as_str()];
            let (ia, ib) = match pins.get(trip.route.as_str()) {
                Some(pin) => {
                    // Pinned: this line's alone, its ports the calls nearest its ends.
                    let at: Vec<usize> = (0..c.len()).filter(|&i| stops.contains_key(&c[i].1)).collect();
                    if pin != lid || at.len() < 2 {
                        continue;
                    }
                    let nearest = |e: [f64; 2]| {
                        let mut best = at[0];
                        let mut bd = km(stops[&c[best].1], e);
                        for &i in &at[1..] {
                            let d = km(stops[&c[i].1], e);
                            if d < bd {
                                (best, bd) = (i, d);
                            }
                        }
                        best
                    };
                    (Some(nearest(a)), Some(nearest(b)))
                }
                None => (
                    c.iter().position(|(_, s, _)| near_a.contains(s.as_str())),
                    c.iter().position(|(_, s, _)| near_b.contains(s.as_str())),
                ),
            };
            let (Some(ia), Some(ib)) = (ia, ib) else { continue };
            if ia == ib {
                continue;
            }
            let d = if ia < ib { 0 } else { 1 };
            let run = runs.get(tid.as_str()).filter(|r| !r.is_empty());
            let n_starts = run.map_or(1, Vec::len) as i64;
            let t0 = c[ia].2;
            matched.insert(trip.route.as_str());
            all_matched.insert(trip.route.clone());
            for &day in dates_of(trip) {
                *per_date[d].entry(day).or_default() += n_starts;
                if d == 0 && t0 >= 0 {
                    let v = deps_a.entry(day).or_default();
                    match run {
                        Some(r) => v.extend(r.iter().map(|s| t0 + s)),
                        None => v.push(t0),
                    }
                }
            }
        }
        let both = !per_date[0].is_empty() && !per_date[1].is_empty();
        let days: BTreeSet<Date> = per_date[0].keys().chain(per_date[1].keys()).copied().collect();
        if days.is_empty() {
            continue;
        }
        let per_day: BTreeMap<Date, f64> = days
            .iter()
            .map(|d| {
                let n = per_date[0].get(d).copied().unwrap_or(0) + per_date[1].get(d).copied().unwrap_or(0);
                (*d, n as f64 / if both { 2.0 } else { 1.0 })
            })
            .collect();
        // Sailings a day on the days it runs; for lines running fewer than 4 days a week, the
        // weekly count spread over the week (a weekly crossing is "1 a week", not "1 a day").
        let mut weeks: super::railcount::Ordered<(i64, i64), Vec<Date>> = Default::default();
        for d in &days {
            weeks.entry(d.iso_week(), Vec::new).push(*d);
        }
        let days_per_week = median(weeks.items.iter().map(|v| v.len() as f64).collect());
        let all_per_day: Vec<f64> = per_day.values().copied().collect();
        let daily = days_per_week >= 4.0 && median(all_per_day.clone()) >= 1.0;
        let typical = if daily {
            median(all_per_day)
        } else {
            median(
                weeks
                    .items
                    .iter()
                    .map(|v| super::py_sum(v.iter().map(|d| super::Num::Float(per_day[d]))).f())
                    .collect(),
            ) / 7.0
        };
        let names: BTreeSet<String> = matched
            .iter()
            .filter_map(|x| match agency_of(routes[x]) {
                Py::Str(s) if !s.is_empty() => Some(s),
                _ => None,
            })
            .collect();
        let names: Vec<String> = names.into_iter().collect();
        let who = if !names.is_empty() && agencies.len() > 1 {
            names[..names.len().min(2)].join(", ")
        } else {
            feed_name.clone()
        };
        let rname = |x: &str| -> String {
            let r = routes[x];
            [r.cell("route_short_name").get(), r.cell("route_long_name").get()]
                .into_iter()
                .flatten()
                .find(|s| !s.is_empty())
                .unwrap_or(x)
                .to_string()
        };
        let mut gtfs_routes: Vec<String> = matched.iter().map(|x| rname(x)).collect();
        gtfs_routes.sort();
        let per_day_r = py_round(typical, if typical < 1.0 { 2 } else { 1 });
        let s = |x: &str| Py::Str(x.to_string());
        let mut rec = vec![
            ("line".to_string(), s(lid)),
            ("kind".into(), s("gtfs")),
            ("per_day".into(), Py::Float(per_day_r)),
            ("source".into(), Py::Str(format!("{who} timetable (GTFS)"))),
            ("url".into(), s(feed.source.as_deref().unwrap_or(&feed.url))),
            ("checked".into(), Py::Str(format!("{} to {}", first_date.iso(), last_date.iso()))),
            ("gtfs_routes".into(), Py::List(gtfs_routes.into_iter().map(Py::Str).collect())),
        ];
        if static_ {
            let first = routes[*matched.first().unwrap()];
            let set = |rec: &mut Vec<(String, Py)>, k: &str, v: Py| rec.iter_mut().find(|(n, _)| n == k).unwrap().1 = v;
            set(&mut rec, "kind", s("timetable"));
            let src = [first.cell("route_long_name").get(), first.cell("route_short_name").get()]
                .into_iter()
                .flatten()
                .find(|s| !s.is_empty())
                .unwrap_or(&feed_name)
                .to_string();
            set(&mut rec, "source", Py::Str(src));
            set(&mut rec, "url", py_opt(get_default(first.cell("route_url"))));
            set(&mut rec, "checked", py_opt(get_default(first.cell("checked"))));
        }
        // Weekdays with service in at least half the weeks the line runs.
        let wd: BTreeSet<i64> = (0..7)
            .filter(|&w| {
                weeks.items.iter().filter(|v| v.iter().any(|d| d.weekday() == w)).count() as f64
                    >= 0.5 * weeks.len() as f64
            })
            .collect();
        rec.push(("days".into(), Py::Str(days_text(&wd))));
        // Season, over the next 12 months: a break of a month or more in the line's own
        // timetable means it is seasonal; a timetable running the whole year means year-round; one
        // that just ends (the next period not yet published) says nothing.
        let fwd: Vec<Date> = days.iter().copied().filter(|d| today <= *d && *d < today.plus(365)).collect();
        if let (Some(&f0), Some(&fl)) = (fwd.first(), fwd.last()) {
            let mut gap = fwd.windows(2).any(|w| w[1].0 - w[0].0 >= 30);
            // (A static file's year comes round again: its break may span the new year, and it
            // never just ends.)
            if static_ {
                gap = gap || (f0.0 - fl.0) + 365 >= 30;
            }
            let mut by_month: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
            let mut month_order: Vec<i64> = Vec::new();
            for d in &fwd {
                let m = d.month() - 1;
                if !by_month.contains_key(&m) {
                    month_order.push(m);
                }
                by_month.entry(m).or_default().push(per_day[d]);
            }
            if gap {
                rec.push(("months".into(), Py::Str(months_text(&by_month.keys().copied().collect()))));
                rec.push(("season".into(), s("seasonal")));
            } else if static_ || ((fl.0 - today.0) >= 330 && (f0.0 - today.0) <= 30) {
                rec.push(("season".into(), s(if wd.len() == 7 { "year" } else { "some-days" })));
            }
            if daily && by_month.len() >= 6 {
                let low = month_order.iter().map(|m| median(by_month[m].clone())).fold(f64::INFINITY, |a, b| {
                    if b < a {
                        b
                    } else {
                        a
                    }
                });
                if low < per_day_r * 0.8 {
                    rec.push(("per_day_low".into(), Py::Float(py_round(low, 1))));
                }
            }
        }
        // Typical daytime headway on the day nearest the median.
        let key = |d: &Date| ((per_day[d] - typical).abs(), d.weekday() >= 5);
        let mut day = *days.first().unwrap();
        for d in &days {
            let (k, kd) = (key(d), key(&day));
            if k.0 < kd.0 || (k.0 == kd.0 && !k.1 && kd.1) {
                day = *d;
            }
        }
        let mut ts: Vec<i64> = deps_a
            .get(&day)
            .map_or(Vec::new(), |v| v.iter().copied().filter(|t| (7 * 3600..=19 * 3600).contains(t)).collect());
        ts.sort();
        if ts.len() >= 4 {
            let gaps: Vec<f64> = ts.windows(2).filter(|w| w[1] > w[0]).map(|w| (w[1] - w[0]) as f64).collect();
            if !gaps.is_empty() {
                rec.push(("headway".into(), Py::Int(((median(gaps) / 60.0).round_ties_even() as i64).to_string())));
            }
        }
        out.push(Py::Dict(rec));
    }
    if let Some(unmatched) = unmatched {
        // Routes sailing in the window, between two ports, that no line took.
        let mut live: BTreeSet<&str> = BTreeSet::new();
        for (tid, c) in &calls.items {
            let t = &trips[tid.as_str()];
            let ports: HashSet<&str> =
                c.iter().filter(|(_, s, _)| stops.contains_key(s)).map(|(_, s, _)| s.as_str()).collect();
            if !dates_of(t).is_empty() && ports.len() >= 2 {
                live.insert(t.route.as_str());
            }
        }
        for rid in live {
            if all_matched.contains(rid) {
                continue;
            }
            let r = routes[rid];
            let pin = pins.get(rid).map_or("", String::as_str);
            let lines_have = lines.iter().any(|(l, _)| l == pin);
            let why = if !pin.is_empty() && !lines_have {
                "its osm pin names no ferry line of this pass"
            } else {
                "its ports match no OpenStreetMap ferry line"
            };
            unmatched.push(unmatched_route(&feed.id, static_, r, agency_of(r), pin, why));
        }
    }
    Ok(out)
}
