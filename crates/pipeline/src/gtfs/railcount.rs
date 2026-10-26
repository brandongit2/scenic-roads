//! The rail counter: passenger trains between consecutive stops on a typical weekday, from the
//! published timetables (GTFS) the `rail` step reads (docs/steps.md §10), for matching onto the OSM
//! track network (`railfreq`).
//!
//! Each feed (the rail-feeds step's list, in its order, each with its zip's path and `fetched`, the
//! day its timetable counts from) is read:
//!   - rail routes only (route types: tram, metro, rail, funicular, monorail and the extended ones);
//!   - the typical day: the Tuesday, Wednesday or Thursday from 30 days before the feed's day to
//!     90 days after with the median number of rail trips, so holidays don't count; feeds with none
//!     are stale. The window follows the feed, not the day it's read, so a zip gives the same
//!     counts whenever it's read;
//!   - every trip that runs that day (headway-based trips expanded), as its stop sequence;
//!   - trains that don't run every day (the Canadian twice a week): where a stop pair has no train
//!     on the typical day, the trains of that day's Monday–Sunday week ÷ 7 (so a weekly train reads
//!     as 0.14 a day rather than none).
//!
//! The same train published in two feeds (a national feed and a regional one, an aggregate and its
//! parts) is counted once: trips are keyed by first and last stop (to ~1 km) and times (to the
//! minute), across all feeds. Where that doesn't catch them (an aggregate republishing an
//! operator's feed with its own times), the operator's feed `replaces` the other, which is left out.
//!
//! The pairs (`Counted::pairs`): per (stop A, stop B, mode) the number of trains that day from A to
//! B; little-endian f32 lon_a, lat_a, lon_b, lat_b, u8 mode (0 tram, 1 metro, 2 rail, 3 funicular),
//! f32 trains (crate::rail::PAIR). Every pair, wherever its stops are: the step then marks the stops
//! beyond the coverage. And `used`: per feed its day, trips and duplicates, or why it was left out.
//!
//! The static files (the accepted zips of inputs/timetables/gtfs/, docs/inputs.md §6.7.2, in their
//! inputs-folder paths' order), read after the feeds:
//!   - their trips as a feed's, the days from `calendar.txt` read as months and days over one
//!     reference year (crate::gtfs::statics), the typical day the median-busy Tuesday to Thursday of
//!     that year; their trains deduplicated with the feeds' by times;
//!   - their count rows (`counts.txt`) of rail routes, as given, to `counts` in their order: per
//!     row the trains each way between its two stops (per_day, else per_week ÷ 7), A to B then B to
//!     A, at the stops' positions as f32, the mode by the route's type, +0x80 for a lower bound
//!     (`at_least`). Never merged with timed trains (they have no times).
//!
//! `routes`: per rail route (its feed or static file, id, name, agency, a static route's `osm` pin),
//! the stop pairs its trains or count rows run between, as the f32 positions the pairs hold them:
//! `{"pairs": [[lon_a, lat_a, lon_b, lat_b]], "routes": [{..., "pairs": [index]}]}`, for the report
//! of the routes that match nothing on the map (docs/inputs.md §6.7.3).

use super::{statics, Date, Kind, PyErr, Zip, R};
use crate::interest::py_round;
use crate::pyjson::Py;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

pub fn mode_of(t: i64) -> Option<u8> {
    if t == 0 || t == 5 || (900..=999).contains(&t) {
        return Some(0);
    }
    if t == 1 || t == 12 || (400..=499).contains(&t) {
        return Some(1);
    }
    if t == 2 || (100..=199).contains(&t) {
        return Some(if t == 116 { 3 } else { 2 });
    }
    if t == 7 || t == 1400 {
        return Some(3);
    }
    None
}

/// `h:m:s` in seconds; -1 when it isn't one.
pub fn secs(t: &str) -> i64 {
    let p: Vec<&str> = crate::pytext::strip(t).split(':').collect();
    if p.len() != 3 {
        return -1;
    }
    match (super::py_int(p[0]), super::py_int(p[1]), super::py_int(p[2])) {
        (Some(h), Some(m), Some(s)) => h * 3600 + m * 60 + s,
        _ => -1,
    }
}

/// The feed's rail routes: route id → (mode, short name).
pub fn rail_routes(z: &Zip) -> R<HashMap<String, (u8, String)>> {
    let mut routes = HashMap::new();
    let mut rows = z.stripped_rows("routes.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let r = rows.row(cells);
        let m = super::py_int(r.cell("route_type").or_empty()).and_then(mode_of);
        if let Some(m) = m {
            let name = [r.cell("route_short_name").or_empty(), r.cell("route_long_name").or_empty()]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or("")
                .to_string();
            routes.insert(r.cell("route_id").req("route_id")?.to_string(), (m, name));
        }
    }
    Ok(routes)
}

/// Each service's days within 400 days of `anchor` (calendar.txt, then calendar_dates.txt).
pub fn service_days(z: &Zip, anchor: Date) -> R<HashMap<String, HashSet<Date>>> {
    let mut days: HashMap<String, HashSet<Date>> = HashMap::new();
    const WD: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    let mut rows = z.stripped_rows("calendar.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let c = rows.row(cells);
        let span = (|| -> R<(Date, Date)> {
            Ok((super::ymd(c.cell("start_date").req("start_date")?)?, super::ymd(c.cell("end_date").req("end_date")?)?))
        })();
        let (mut d, end) = match span {
            Ok(x) => x,
            Err(e) if matches!(e.kind, Kind::Key | Kind::Value) => continue,
            Err(e) => return Err(e),
        };
        let wd: Vec<bool> = WD.iter().map(|k| c.cell(k).get() == Some("1")).collect();
        let end = end.min(anchor.plus(400));
        d = d.max(anchor.plus(-400));
        while d <= end {
            if wd[d.weekday() as usize] {
                days.entry(c.cell("service_id").req("service_id")?.to_string()).or_default().insert(d);
            }
            d = d.plus(1);
        }
    }
    let mut rows = z.stripped_rows("calendar_dates.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let c = rows.row(cells);
        let d = match c.cell("date").req("date").and_then(super::ymd) {
            Ok(d) => d,
            Err(e) if matches!(e.kind, Kind::Key | Kind::Value) => continue,
            Err(e) => return Err(e),
        };
        let set = days.entry(c.cell("service_id").req("service_id")?.to_string()).or_default();
        if c.cell("exception_type").get() == Some("1") {
            set.insert(d);
        } else {
            set.remove(&d);
        }
    }
    Ok(days)
}

/// A service's days, whichever way they were read.
pub trait Days {
    fn has(&self, sid: &str, d: Date) -> bool;
}

impl Days for HashMap<String, HashSet<Date>> {
    fn has(&self, sid: &str, d: Date) -> bool {
        self.get(sid).is_some_and(|s| s.contains(&d))
    }
}

impl Days for HashMap<String, Vec<Date>> {
    fn has(&self, sid: &str, d: Date) -> bool {
        self.get(sid).is_some_and(|s| s.binary_search(&d).is_ok())
    }
}

/// The median-busy Tuesday–Thursday from 30 days before `anchor` to 90 after (None: of the static
/// files' reference year), by rail trips; None when there's no weekday service then (a stale feed).
pub fn typical_day(trips: &HashMap<String, (String, String)>, days: &dyn Days, anchor: Option<Date>) -> Option<Date> {
    let cand: Vec<Date> = match anchor {
        None => {
            let d0 = Date::new(statics::REF_YEAR, 1, 1).unwrap();
            (0..365).map(|k| d0.plus(k)).collect()
        }
        Some(a) => (-30..91).map(|k| a.plus(k)).collect(),
    };
    let mut per_service: HashMap<&str, u64> = HashMap::new();
    for (_, sid) in trips.values() {
        *per_service.entry(sid).or_default() += 1;
    }
    let mut busy: Vec<(u64, Date)> = cand
        .into_iter()
        .filter(|d| (1..=3).contains(&d.weekday()))
        .map(|d| (per_service.iter().filter(|(sid, _)| days.has(sid, d)).map(|(_, n)| n).sum(), d))
        .filter(|(n, _)| *n > 0)
        .collect();
    busy.sort();
    busy.get(busy.len() / 2).map(|x| x.1)
}

/// The rail trips: trip id → (route id, service id).
fn rail_trips(z: &Zip, routes: &HashMap<String, (u8, String)>) -> R<HashMap<String, (String, String)>> {
    let mut trips = HashMap::new();
    let mut rows = z.stripped_rows("trips.txt")?;
    while let Some(cells) = rows.next_cells()? {
        let t = rows.row(cells);
        if !t.cell("route_id").get().is_some_and(|r| routes.contains_key(r)) {
            continue;
        }
        let tid = t.cell("trip_id").req("trip_id")?.to_string();
        trips.insert(
            tid,
            (t.cell("route_id").req("route_id")?.to_string(), t.cell("service_id").req("service_id")?.to_string()),
        );
    }
    Ok(trips)
}

/// Whether a feed's zip has rail service in its window (rail-feeds' check of a zip it has).
pub fn has_service(zpath: &Path, anchor: Date) -> R<bool> {
    let z = Zip::open(zpath)?;
    let routes = rail_routes(&z)?;
    let trips = rail_trips(&z, &routes)?;
    Ok(!routes.is_empty() && typical_day(&trips, &service_days(&z, anchor)?, Some(anchor)).is_some())
}

/// `v` as the f32 the pairs hold it.
fn f32_of(v: f64) -> f64 {
    v as f32 as f64
}

/// A float as a key: Python's equality (-0.0 is 0.0).
fn fk(v: f64) -> u64 {
    if v == 0.0 {
        0
    } else {
        v.to_bits()
    }
}

/// An insertion-ordered map, as Python's dict.
pub struct Ordered<K, V> {
    at: HashMap<K, usize>,
    pub items: Vec<V>,
}

impl<K: std::hash::Hash + Eq, V> Default for Ordered<K, V> {
    fn default() -> Self {
        Ordered { at: HashMap::new(), items: Vec::new() }
    }
}

impl<K: std::hash::Hash + Eq, V> Ordered<K, V> {
    /// The item at `k`, made by `new` when there's none.
    pub fn entry(&mut self, k: K, new: impl FnOnce() -> V) -> &mut V {
        let n = self.items.len();
        let i = *self.at.entry(k).or_insert(n);
        if i == n {
            self.items.push(new());
        }
        &mut self.items[i]
    }
    pub fn get(&self, k: &K) -> Option<&V> {
        self.at.get(k).map(|&i| &self.items[i])
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Each route's stop pairs (`routes`).
#[derive(Default)]
pub struct Routes {
    index: Ordered<[u64; 4], [f64; 4]>,
    routes: Ordered<(String, String), (Vec<(String, Py)>, BTreeSet<usize>)>,
}

impl Routes {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        source: &str,
        static_: bool,
        rid: &str,
        r: Option<&super::OwnedRow>,
        agency: &str,
        a: [f64; 2],
        b: [f64; 2],
    ) {
        let k = [f32_of(a[0]), f32_of(a[1]), f32_of(b[0]), f32_of(b[1])];
        self.index.entry(k.map(fk), || k);
        let i = self.index.at[&k.map(fk)];
        let e = self.routes.entry((source.to_string(), rid.to_string()), || {
            let cell = |k: &str| r.map_or("", |r| r.cell(k).or_empty());
            let name =
                [cell("route_short_name"), cell("route_long_name")].into_iter().find(|s| !s.is_empty()).unwrap_or("");
            let osm = if static_ { crate::pytext::strip(cell("osm")) } else { "" };
            let s = |x: &str| Py::Str(x.to_string());
            (
                vec![
                    ("mode".into(), s("rail")),
                    ("source".into(), s(source)),
                    ("static".into(), Py::Bool(static_)),
                    ("route".into(), s(rid)),
                    ("name".into(), s(name)),
                    ("agency".into(), s(agency)),
                    ("osm".into(), s(osm)),
                ],
                BTreeSet::new(),
            )
        });
        e.1.insert(i);
    }

    /// `{"pairs": […], "routes": […]}`.
    pub fn json(&self) -> Py {
        let pairs = self.index.items.iter().map(|k| Py::List(k.iter().map(|&v| Py::Float(v)).collect())).collect();
        let routes = self
            .routes
            .items
            .iter()
            .map(|(e, ps)| {
                let mut d = e.clone();
                d.push(("pairs".into(), Py::List(ps.iter().map(|i| Py::Int(i.to_string())).collect())));
                Py::Dict(d)
            })
            .collect();
        Py::Dict(vec![("pairs".into(), Py::List(pairs)), ("routes".into(), Py::List(routes))])
    }
}

/// routes.txt by id, and agencies' names by id.
fn route_facts(z: &Zip) -> R<(HashMap<String, super::OwnedRow>, HashMap<String, String>)> {
    let mut facts = HashMap::new();
    for r in z.stripped_rows("routes.txt")?.all()? {
        facts.insert(r.cell("route_id").req("route_id")?.to_string(), r);
    }
    let mut agencies = HashMap::new();
    for a in z.stripped_rows("agency.txt")?.all()? {
        agencies.insert(a.cell("agency_id").or_empty().to_string(), a.cell("agency_name").or_empty().to_string());
    }
    Ok((facts, agencies))
}

/// A stop's place and its rounded copies (to 5 decimals: the pairs'; to 2: the trains' keys).
#[derive(Clone, Copy)]
struct Stop {
    ll: [f64; 2],
    r5: [f64; 2],
    r2: [f64; 2],
}

/// What all the zips' trains add up to, across zips.
#[derive(Default)]
pub struct Counted {
    seen: HashSet<[u64; 7]>,
    seen_week: HashSet<[u64; 7]>,
    pairs: Ordered<[u64; 5], ([f64; 4], u8, f64)>,
    weekly: Ordered<[u64; 5], ([f64; 4], u8, f64)>,
    pub routes: Routes,
}

fn trip_key(a: &Stop, t0: i64, b: &Stop, t1: i64, mode: u8) -> [u64; 7] {
    [
        fk(a.r2[0]),
        fk(a.r2[1]),
        t0.div_euclid(60) as u64,
        fk(b.r2[0]),
        fk(b.r2[1]),
        t1.div_euclid(60) as u64,
        mode as u64,
    ]
}

fn pair_key(a: &Stop, b: &Stop, mode: u8) -> ([u64; 5], [f64; 4]) {
    let v = [a.r5[0], a.r5[1], b.r5[0], b.r5[1]];
    ([fk(v[0]), fk(v[1]), fk(v[2]), fk(v[3]), mode as u64], v)
}

impl Counted {
    /// A zip's trains into the pairs (and the weekly ones): a feed's from `anchor`, a static file's
    /// (None) by its months and days; each route's stop pairs into the routes, named by `source`.
    /// Its status, day, trips and duplicates.
    pub fn process(&mut self, zpath: &Path, anchor: Option<Date>, source: &str) -> R<Vec<(String, Py)>> {
        let p = crate::timings::phase("the feeds' tables read", crate::timings::Class::Mixed);
        let z = Zip::open(zpath)?;
        let routes = rail_routes(&z)?;
        let st = |s: &str| vec![("status".to_string(), Py::Str(s.to_string()))];
        if routes.is_empty() {
            return Ok(st("no rail routes"));
        }
        let trips = rail_trips(&z, &routes)?;
        let (feed_days, static_days);
        let days: &dyn Days = match anchor {
            Some(a) => {
                feed_days = service_days(&z, a)?;
                &feed_days
            }
            None => {
                static_days = statics::service_dates(&z)?;
                &static_days
            }
        };
        let Some(day) = typical_day(&trips, days, anchor) else {
            return Ok(st(if anchor.is_some() {
                "stale (no weekday service in the window)"
            } else {
                "no timed trains"
            }));
        };
        let active: HashSet<&str> =
            trips.iter().filter(|(_, (_, sid))| days.has(sid, day)).map(|(t, _)| t.as_str()).collect();
        // Its week, for trains that don't run that day: trip → days it runs that week.
        let week: Vec<Date> = (0..7).map(|k| day.plus(k - day.weekday())).collect();
        let in_week: HashMap<&str, u32> = trips
            .iter()
            .filter(|(t, _)| !active.contains(t.as_str()))
            .map(|(t, (_, sid))| (t.as_str(), week.iter().filter(|d| days.has(sid, **d)).count() as u32))
            .filter(|(_, n)| *n > 0)
            .collect();
        let mut runs: HashMap<String, Vec<i64>> = HashMap::new();
        let mut rows = z.stripped_rows("frequencies.txt")?;
        while let Some(cells) = rows.next_cells()? {
            let fr = rows.row(cells);
            let Some(tid) = fr.cell("trip_id").get().filter(|t| active.contains(t)) else { continue };
            let (a, b) = (secs(fr.cell("start_time").req("start_time")?), secs(fr.cell("end_time").req("end_time")?));
            let hw_text = fr.cell("headway_secs").or_empty();
            let hw = if hw_text.is_empty() {
                0
            } else {
                let f = super::py_float(hw_text).ok_or_else(|| {
                    PyErr::value(format!("could not convert string to float: {}", super::py_repr(hw_text)))
                })?;
                py_trunc(f)?
            };
            if a >= 0 && b > a && hw > 0 {
                let v = runs.entry(tid.to_string()).or_default();
                let mut x = a;
                while x < b {
                    v.push(x);
                    x += hw;
                }
            }
        }
        let mut stops: HashMap<String, Stop> = HashMap::new();
        let mut rows = z.stripped_rows("stops.txt")?;
        while let Some(cells) = rows.next_cells()? {
            let s = rows.row(cells);
            let (Ok(lon), Ok(lat)) = (s.cell("stop_lon").req("stop_lon"), s.cell("stop_lat").req("stop_lat")) else {
                continue;
            };
            let (Some(x), Some(y)) = (super::py_float(lon), super::py_float(lat)) else { continue };
            if x.abs() > 0.01 || y.abs() > 0.01 {
                let r = |v: f64, n| py_round(v, n);
                stops.insert(
                    s.cell("stop_id").req("stop_id")?.to_string(),
                    Stop { ll: [x, y], r5: [r(x, 5), r(y, 5)], r2: [r(x, 2), r(y, 2)] },
                );
            }
        }
        // Each trip's calls, in the order trips first appear: (sequence, stop, departure).
        let mut names: Vec<String> = Vec::new();
        let mut name_of: HashMap<String, u32> = HashMap::new();
        let mut calls: Ordered<String, (String, Vec<(i64, u32, i64)>)> = Ordered::default();
        let mut rows = z.stripped_rows("stop_times.txt")?;
        let col = |k: &str| rows.head.col(k);
        let (c_trip, c_seq, c_stop, c_dep, c_arr) =
            (col("trip_id"), col("stop_sequence"), col("stop_id"), col("departure_time"), col("arrival_time"));
        while let Some(cells) = rows.next_cells()? {
            let r = rows.row(cells);
            let Some(tid) = r.at(c_trip).get().filter(|t| active.contains(t) || in_week.contains_key(t)) else {
                continue;
            };
            // (A sequence missing or not a number passes the call over.)
            let Some(seq) = r.at(c_seq).get().and_then(super::py_int) else { continue };
            let stop = r.at(c_stop).req("stop_id")?;
            let sid = match name_of.get(stop) {
                Some(&i) => i,
                None => {
                    name_of.insert(stop.to_string(), names.len() as u32);
                    names.push(stop.to_string());
                    names.len() as u32 - 1
                }
            };
            let dep =
                [r.at(c_dep).or_empty(), r.at(c_arr).or_empty()].into_iter().find(|s| !s.is_empty()).unwrap_or("");
            let t = secs(dep);
            match calls.at.get(tid) {
                Some(&i) => calls.items[i].1.push((seq, sid, t)),
                None => calls.entry(tid.to_string(), || (tid.to_string(), Vec::new())).1.push((seq, sid, t)),
            }
        }
        drop(p);
        let _p = crate::timings::phase("the trains counted", crate::timings::Class::Compute);
        let (mut n_trips, mut n_dup) = (0u64, 0u64);
        let (facts, agencies) = route_facts(&z)?;
        for (tid, c) in calls.items.iter_mut() {
            c.sort_by(|a, b| {
                a.0.cmp(&b.0).then_with(|| names[a.1 as usize].cmp(&names[b.1 as usize])).then(a.2.cmp(&b.2))
            });
            let c: Vec<(i64, &Stop, i64)> =
                c.iter().filter_map(|&(q, s, t)| stops.get(&names[s as usize]).map(|st| (q, st, t))).collect();
            if c.len() < 2 {
                continue;
            }
            let rid = trips[tid.as_str()].0.as_str();
            let mode = routes[rid].0;
            {
                let r = facts.get(rid);
                let agency = r.map_or("", |r| agencies.get(r.cell("agency_id").or_empty()).map_or("", String::as_str));
                for w in c.windows(2) {
                    let (a, b) = (w[0].1, w[1].1);
                    if a.ll != b.ll {
                        self.routes.add(source, anchor.is_none(), rid, r, agency, a.r5, b.r5);
                    }
                }
            }
            let (t0, t1) = (c[0].2, c[c.len() - 1].2);
            let (first, last) = (c[0].1, c[c.len() - 1].1);
            if let Some(&n) = in_week.get(tid.as_str()) {
                // Not on the typical day: its runs that week, a seventh each.
                if !self.seen_week.insert(trip_key(first, t0, last, t1, mode)) {
                    continue;
                }
                for w in c.windows(2) {
                    let (a, b) = (w[0].1, w[1].1);
                    if a.ll != b.ll {
                        let (k, v) = pair_key(a, b, mode);
                        self.weekly.entry(k, || (v, mode, 0.0)).2 += n as f64 / 7.0;
                    }
                }
                continue;
            }
            let starts: Vec<Option<i64>> = match runs.get(tid.as_str()) {
                Some(r) if !r.is_empty() => r.iter().map(|&x| Some(x)).collect(),
                _ => vec![None],
            };
            for off in starts {
                let dt = off.map_or(0, |o| o - if t0 >= 0 { t0 } else { 0 });
                if !self.seen.insert(trip_key(first, t0 + dt, last, t1 + dt, mode)) {
                    n_dup += 1;
                    continue;
                }
                n_trips += 1;
                for w in c.windows(2) {
                    let (a, b) = (w[0].1, w[1].1);
                    if a.ll != b.ll {
                        let (k, v) = pair_key(a, b, mode);
                        self.pairs.entry(k, || (v, mode, 0.0)).2 += 1.0;
                    }
                }
            }
        }
        let (_, m, d) = day.ymd();
        let day_text = if anchor.is_some() { day.iso() } else { format!("{m:02}-{d:02}") };
        let int = |n: u64| Py::Int(n.to_string());
        Ok(vec![
            ("status".into(), Py::Str("ok".into())),
            ("day".into(), Py::Str(day_text)),
            ("trips".into(), int(n_trips)),
            ("duplicates".into(), int(n_dup)),
            ("rail_routes".into(), int(routes.len() as u64)),
        ])
    }

    /// A static file's count rows of rail routes as stop pairs (see the module's docs), and how
    /// many rows; each route's pairs into the routes.
    pub fn counted(&mut self, zpath: &Path, source: &str) -> R<(Vec<u8>, u64)> {
        let mut out = Vec::new();
        let mut n = 0;
        let z = Zip::open(zpath)?;
        let routes = rail_routes(&z)?;
        let (facts, agencies) = route_facts(&z)?;
        let mut stops: HashMap<String, [f64; 2]> = HashMap::new();
        let mut rows = z.stripped_rows("stops.txt")?;
        while let Some(cells) = rows.next_cells()? {
            let s = rows.row(cells);
            let (Ok(lon), Ok(lat)) = (s.cell("stop_lon").req("stop_lon"), s.cell("stop_lat").req("stop_lat")) else {
                continue;
            };
            let (Some(x), Some(y)) = (super::py_float(lon), super::py_float(lat)) else { continue };
            stops.insert(s.cell("stop_id").req("stop_id")?.to_string(), [x, y]);
        }
        for c in statics::count_rows(&z)? {
            let cell = |k: &str| crate::pytext::strip(c.cell(k).or_empty()).to_string();
            let r = routes.get(&cell("route_id"));
            let (a, b) = (stops.get(&cell("from_stop_id")), stops.get(&cell("to_stop_id")));
            let (Some(r), Some(&a), Some(&b)) = (r, a, b) else { continue };
            if a == b {
                continue;
            }
            let v = statics::count_values(&c);
            let num = |k: &str| match v.iter().find(|(n, _)| n == k).map(|(_, x)| x) {
                Some(Py::Int(s)) => s.parse::<f64>().ok(),
                Some(Py::Float(f)) => Some(*f),
                Some(Py::Bool(b)) => Some(*b as u8 as f64),
                _ => None,
            };
            let per_day = match (num("per_day"), num("per_week")) {
                (Some(d), _) => d,
                (None, Some(w)) => w / 7.0,
                (None, None) => continue,
            };
            let mode = r.0 | if cell("at_least") == "1" { 0x80 } else { 0 };
            let rid = crate::pytext::strip(c.cell("route_id").req("route_id")?).to_string();
            for (p, q) in [(a, b), (b, a)] {
                for x in [p[0], p[1], q[0], q[1]] {
                    out.extend((x as f32).to_le_bytes());
                }
                out.push(mode);
                out.extend((per_day as f32).to_le_bytes());
                let r = facts.get(&rid);
                let agency = r.map_or("", |r| agencies.get(r.cell("agency_id").or_empty()).map_or("", String::as_str));
                self.routes.add(source, true, &rid, r, agency, p, q);
            }
            n += 1;
        }
        Ok((out, n))
    }

    /// The stop pairs: the trains of the typical day, then the pairs served only on other days of
    /// the week at their weekly average; and how many of those.
    pub fn pairs(&self) -> (Vec<u8>, usize) {
        let mut out = Vec::with_capacity((self.pairs.len() + self.weekly.len()) * crate::rail::PAIR);
        let mut put = |v: &[f64; 4], m: u8, n: f64| {
            for x in v {
                out.extend((*x as f32).to_le_bytes());
            }
            out.push(m);
            out.extend((n as f32).to_le_bytes());
        };
        for (v, m, n) in &self.pairs.items {
            put(v, *m, *n);
        }
        let mut n_week = 0;
        for (v, m, n) in &self.weekly.items {
            let k = [fk(v[0]), fk(v[1]), fk(v[2]), fk(v[3]), *m as u64];
            if self.pairs.get(&k).is_none_or(|p| p.2 == 0.0) {
                put(v, *m, *n);
                n_week += 1;
            }
        }
        (out, n_week)
    }
}

/// `int(f)` of a float: toward zero; ValueError for NaN, OverflowError (here a ValueError) for
/// the infinities.
fn py_trunc(f: f64) -> R<i64> {
    if f.is_nan() {
        return Err(PyErr::value("cannot convert float NaN to integer"));
    }
    if f.is_infinite() {
        return Err(PyErr::value("cannot convert float infinity to integer"));
    }
    Ok(f.trunc() as i64)
}

/// The rail counter's run: the feeds (each a serde object with `id`, `path`, `fetched` and its
/// facts), then the static files (name, path). The stop pairs, the static files' count rows (as
/// pairs), `used` (JSON, as the Python wrote it) and the routes (compact JSON).
pub struct Run {
    pub pairs: Vec<u8>,
    pub counts: Vec<u8>,
    pub used: String,
    pub routes: String,
}

/// Whether the rail counter takes `e` for a zip it can't read (its status), not a failure.
fn unreadable(e: &PyErr) -> bool {
    matches!(e.kind, Kind::BadZip | Kind::Key | Kind::Csv)
}

pub fn run(feeds: &[serde_json::Value], statics_: &[(String, std::path::PathBuf)]) -> anyhow::Result<Run> {
    let mut replaced: HashMap<String, String> = HashMap::new();
    for f in feeds {
        if f.get("path").is_some_and(|p| crate::pyjson::from_serde(p).truthy()) {
            for r in f.get("replaces").and_then(|r| r.as_array()).into_iter().flatten() {
                if let Some(r) = r.as_str() {
                    replaced.insert(r.to_string(), f["id"].as_str().unwrap_or("").to_string());
                }
            }
        }
    }
    let mut c = Counted::default();
    let mut used: Vec<Py> = Vec::new();
    for (k, feed) in feeds.iter().enumerate() {
        crate::agent::jobs::report(k as u64, feeds.len() as u64, "feeds");
        let mut facts: Vec<(String, Py)> = ["id", "provider", "url", "licence", "fetched"]
            .iter()
            .filter_map(|&key| feed.get(key).map(|v| (key.to_string(), crate::pyjson::from_serde(v))))
            .collect();
        let id = feed["id"].as_str().unwrap_or("");
        if let Some(by) = replaced.get(id) {
            facts.push(("status".into(), Py::Str(format!("replaced by {by}"))));
            used.push(Py::Dict(facts));
            continue;
        }
        let path = feed.get("path").and_then(|p| p.as_str()).filter(|p| !p.is_empty());
        let Some(path) = path else {
            let st = feed.get("status").and_then(|s| s.as_str()).filter(|s| !s.is_empty()).unwrap_or("no zip");
            facts.push(("status".into(), Py::Str(st.to_string())));
            used.push(Py::Dict(facts));
            continue;
        };
        let fetched = feed["fetched"].as_str().unwrap_or("");
        let anchor = Date::parse_iso(fetched).ok_or_else(|| anyhow::anyhow!("{id}: its day {fetched:?}"))?;
        let res = match c.process(Path::new(path), Some(anchor), id) {
            Ok(r) => r,
            Err(e) if unreadable(&e) => vec![("status".into(), Py::Str(format!("unreadable: {e}")))],
            Err(e) => anyhow::bail!("{id}: {}: {e}", e.class()),
        };
        let show = |k: &str| res.iter().find(|(n, _)| n == k).map_or(String::new(), |(_, v)| v.py_str());
        eprintln!(
            "{id:<28} {} {}  {} trips {} dup {}",
            feed["provider"].as_str().unwrap_or(""),
            show("status"),
            show("day"),
            show("trips"),
            show("duplicates")
        );
        facts.extend(res);
        used.push(Py::Dict(facts));
    }
    crate::agent::jobs::report(feeds.len() as u64, feeds.len() as u64, "feeds");
    let mut counts = Vec::new();
    for (name, path) in statics_ {
        let mut entry = vec![("static".to_string(), Py::Str(name.clone()))];
        let res = c.counted(path, name).and_then(|(b, n)| {
            let mut res = c.process(path, None, name)?;
            counts.extend(b);
            res.push(("count_rows".into(), Py::Int(n.to_string())));
            Ok(res)
        });
        let res = match res {
            Ok(r) => r,
            Err(e) if unreadable(&e) => vec![("status".into(), Py::Str(format!("unreadable: {e}")))],
            Err(e) => anyhow::bail!("{name}: {}: {e}", e.class()),
        };
        entry.extend(res);
        used.push(Py::Dict(entry));
    }
    let _p = crate::timings::phase("the stop pairs written", crate::timings::Class::Compute);
    let (pairs, n_week) = c.pairs();
    eprintln!("{n_week} stop pairs served only on other days of the week (weekly average)");
    let ok: Vec<&Py> = used.iter().filter(|u| u.at("status").as_str() == Some("ok")).collect();
    let sum = |k: &str| ok.iter().filter_map(|u| u.at(k).number()).sum::<f64>();
    eprintln!(
        "{} stop pairs from {} feeds, {} trains, {} duplicates dropped",
        pairs.len() / crate::rail::PAIR,
        ok.len(),
        sum("trips"),
        sum("duplicates")
    );
    Ok(Run {
        pairs,
        counts,
        used: crate::pyjson::dumps_indent(&Py::List(used), 1),
        routes: crate::pyjson::dumps(&c.routes.json()),
    })
}
