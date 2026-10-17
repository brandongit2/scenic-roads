//! Publishing timed end to end (task #250; docs/pipeline.md, Publishing's times): from a build
//! unit done to its map tiles on every Mac's map, by the events each stage leaves.
//!
//! - **In the build's history** (crate::coord::history, the lead's): a unit's job ended ("end",
//!   "done"); its hand-off merged ("merged": the journal entries' keys, `<day>/<lease>`); the map
//!   tiles (pack) planned ("planned": the stale z6 tiles a map update first plans, with the units
//!   whose ways reach each, `reaching`), leased ("lease") and done ("end", "done"); and catalog n
//!   written ("catalog": its `n`, its time, and `CatalogStats`: delta or whole, the files it
//!   stat'ed, its phases).
//! - **The Macs' map servers' own log on the NAS** (`SERVERS`, a file per Mac appended to: a few
//!   lines a catalog, never a file per event): each server's adoption of catalog n ("adopt") and its
//!   mirror's sync of it, begun ("mirror-start") and through everything wanted ("mirror-end").
//!
//! `chains` joins them, per catalog and per unit; `scenic timings publish` prints them.

use crate::coord::history::Event;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

/// The map servers' log: `<host>.jsonl` in it, one writer each.
pub const SERVERS: &str = "state/publish";

/// The steps whose runs are counted per catalog (besides the map tiles').
pub const COUNTED: [&str; 5] = ["ferries", "stations", "roadunits", "marks", "names-todo"];

/// A map server's event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerEvent {
    pub t: u64,
    pub host: String,
    /// "adopt", "mirror-start" or "mirror-end".
    pub kind: String,
    /// The catalog.
    pub n: u64,
    /// The mirror's sync, from its start for this catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secs: Option<f64>,
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Appends `e` (now, unless it says when) to its Mac's log under the project folder `root`.
pub fn append_server(root: &Path, mut e: ServerEvent) -> std::io::Result<()> {
    if e.t == 0 {
        e.t = now_s();
    }
    let dir = root.join(SERVERS);
    std::fs::create_dir_all(&dir)?;
    let name: String = e.host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let line = serde_json::to_string(&e).map_err(std::io::Error::other)?;
    std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.jsonl")))?.write_all(format!("{line}\n").as_bytes())
}

/// Every Mac's map server's events since `since` (unix seconds), in time order.
pub fn read_servers(root: &Path, since: u64) -> Vec<ServerEvent> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root.join(SERVERS)) else { return out };
    for f in rd.flatten() {
        if f.path().extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f.path()) else { continue };
        out.extend(text.lines().filter_map(|l| serde_json::from_str::<ServerEvent>(l).ok()).filter(|e| e.t >= since));
    }
    out.sort_by_key(|e| e.t);
    out
}

/// What a map server's mirror said of its syncs, per process: each catalog's start once, its end
/// once (a sync through everything wanted, nothing waiting or failed).
#[derive(Debug, Default)]
pub struct MirrorMarks {
    started: Option<(u64, u64)>,
    ended: Option<u64>,
}

impl MirrorMarks {
    pub const fn new() -> MirrorMarks {
        MirrorMarks { started: None, ended: None }
    }

    /// A sync of catalog `n` begins at `t`: whether it's the first of it (to say).
    pub fn start(&mut self, n: u64, t: u64) -> bool {
        if self.started.is_some_and(|(m, _)| m == n) {
            return false;
        }
        self.started = Some((n, t));
        true
    }

    /// A sync of catalog `n` ended at `t`, `complete` or not: its time from the first start of it,
    /// once, when complete.
    pub fn end(&mut self, n: u64, t: u64, complete: bool) -> Option<f64> {
        let (m, began) = self.started?;
        if m != n || !complete || self.ended == Some(n) {
            return None;
        }
        self.ended = Some(n);
        Some(t.saturating_sub(began) as f64)
    }
}

/// What a catalog's job said of itself (a line of its log, `CATALOG_LINE`, which the agent reads
/// into the history's "catalog" event).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogStats {
    pub n: u64,
    /// Written as a delta over the last (else whole).
    pub delta: bool,
    /// The files it listed, and those it looked at on the NAS (stat'ed).
    pub files: u64,
    pub stated: u64,
    /// Its phases' wall times, in order.
    #[serde(default)]
    pub phases: Vec<(String, f64)>,
}

/// The start of a catalog's line in its log.
pub const CATALOG_LINE: &str = "catalog-stats: ";

impl CatalogStats {
    pub fn line(&self) -> String {
        format!("{CATALOG_LINE}{}", serde_json::to_string(self).unwrap_or_default())
    }

    /// The last such line in a log's tail.
    pub fn from_log(tail: &str) -> Option<CatalogStats> {
        tail.lines().rev().find_map(|l| l.find(CATALOG_LINE).and_then(|i| serde_json::from_str(&l[i + CATALOG_LINE.len()..]).ok()))
    }
}

/// The units whose ways reach each z6 tile ("6/x/y"; the units' base packs in the manifest `m`,
/// "6/x/y" too): a unit's owned extent (crate::reach::Reach::owned_extent), without a reach its
/// tile + `reach::LONG_KM`, meeting the tile. (What a "planned" event says, for the units' restale
/// counts.)
pub fn reaching(m: &BTreeMap<String, String>, reach: Option<&crate::reach::Reaches>, tiles: &[String]) -> BTreeMap<String, Vec<String>> {
    use crate::legacy::Unit;
    let units: Vec<(Unit, [i32; 4])> = m
        .keys()
        .filter_map(|l| l.strip_prefix("base/").and_then(Unit::parse))
        .map(|u| (u, reach.and_then(|r| r.get(u)).map(|r| r.owned_extent(u)).unwrap_or_else(|| crate::reach::near_box(u))))
        .collect();
    tiles
        .iter()
        .filter_map(|t| {
            let p: Vec<u32> = t.split('/').filter_map(|s| s.parse().ok()).collect();
            let [z, x, y] = p[..] else { return None };
            let b = crate::hipack::tile_bounds(z as u8, x, y);
            let near = units.iter().filter(|(_, e)| e[0] <= b[2] && e[2] >= b[0] && e[1] <= b[3] && e[3] >= b[1]).map(|(u, _)| u.slash()).collect();
            Some((t.clone(), near))
        })
        .collect()
}

/// A "planned" event: the map tiles `tiles` first planned in a map update, with the units reaching each.
pub fn planned_event(tiles: Vec<String>, reach: BTreeMap<String, Vec<String>>) -> Event {
    Event { step: Some("pack".into()), targets: tiles, data: Some(serde_json::json!({ "reach": reach })), ..Event::new("planned") }
}

/// A unit's way to the map: when each stage was reached (unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UnitChain {
    pub unit: String,
    pub worker: Option<String>,
    /// Its job ended.
    pub done: u64,
    /// Its hand-off merged (a member's job; the lead's own have none).
    pub merged: Option<u64>,
    /// Its map tiles: first planned, first leased, last done.
    pub planned: Option<u64>,
    pub leased: Option<u64>,
    pub packed: Option<u64>,
    /// The catalog it's in.
    pub catalog: Option<u64>,
    pub catalog_t: Option<u64>,
    /// Each Mac's server adopting it (that catalog or a later one), and its mirror through it.
    pub adopted: BTreeMap<String, u64>,
    pub mirrored: BTreeMap<String, u64>,
    /// The map tiles it made stale (planned since the catalog before, reached by its ways).
    pub restaled: usize,
}

/// A catalog's publishing.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CatalogChain {
    pub n: Option<u64>,
    pub t: u64,
    /// Its job's time.
    pub secs: Option<f64>,
    pub stats: Option<CatalogStats>,
    /// Each Mac's server: seconds from the catalog to its adoption; its mirror's sync (from the
    /// catalog to its start, and its length).
    pub adopted: BTreeMap<String, f64>,
    pub mirrored: BTreeMap<String, (f64, Option<f64>)>,
    /// The units in it, and the map tiles planned for it.
    pub units: usize,
    pub tiles: usize,
    /// Of `COUNTED`, the steps that ran for it (names-todo: after it).
    pub ran: Vec<String>,
}

fn ok_job(e: &Event, step: &str) -> bool {
    matches!(e.kind.as_str(), "end" | "done") && e.ok == Some(true) && e.step.as_deref() == Some(step)
}

/// The chains of the catalogs in `events` (any order) and their units, joined with the servers'
/// events; oldest first.
pub fn chains(events: &[Event], servers: &[ServerEvent]) -> (Vec<CatalogChain>, Vec<UnitChain>) {
    let mut ev: Vec<&Event> = events.iter().collect();
    ev.sort_by_key(|e| (e.t, e.seq));
    let cats: Vec<&Event> = ev.iter().copied().filter(|e| e.kind == "catalog").collect();
    // (A lease's merges: an entry's key is `<day>/<term>-<n>`, the coordinator's lease its `n`;
    // the first after the job's end is its.)
    let mut merged: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for e in ev.iter().filter(|e| e.kind == "merged") {
        for k in &e.targets {
            if let Some(l) = k.rsplit(['/', '-']).next().and_then(|l| l.parse().ok()) {
                merged.entry(l).or_default().push(e.t);
            }
        }
    }
    // Each server's events by host.
    let mut by_host: BTreeMap<&str, Vec<&ServerEvent>> = BTreeMap::new();
    for s in servers {
        by_host.entry(s.host.as_str()).or_default().push(s);
    }
    let first_at = |host: &[&ServerEvent], kind: &str, n: u64, after: u64| host.iter().filter(|s| s.kind == kind && s.n >= n && s.t >= after).map(|s| s.t).min();
    let (mut out_c, mut out_u) = (Vec::new(), Vec::new());
    for (i, c) in cats.iter().enumerate() {
        let from = if i == 0 { 0 } else { cats[i - 1].t };
        let next = cats.get(i + 1).map_or(u64::MAX, |n| n.t);
        let window = |e: &Event| e.t > from && e.t <= c.t;
        let stats: Option<CatalogStats> = c.data.as_ref().and_then(|d| serde_json::from_value(d.clone()).ok());
        let n = c.n.or(stats.as_ref().map(|s| s.n));
        // The map tiles planned for it, with the units reaching each; leased and done.
        let mut reach: BTreeMap<String, (u64, Vec<String>)> = BTreeMap::new();
        let (mut leased, mut packed): (BTreeMap<String, u64>, BTreeMap<String, u64>) = Default::default();
        for e in ev.iter().filter(|e| window(e)) {
            if e.kind == "planned" && e.step.as_deref() == Some("pack") {
                let r: BTreeMap<String, Vec<String>> = e.data.as_ref().and_then(|d| serde_json::from_value(d["reach"].clone()).ok()).unwrap_or_default();
                for t in &e.targets {
                    reach.entry(t.clone()).or_insert((e.t, r.get(t).cloned().unwrap_or_default()));
                }
            } else if e.step.as_deref() == Some("pack") && matches!(e.kind.as_str(), "lease" | "start") {
                for t in &e.targets {
                    leased.entry(t.clone()).or_insert(e.t);
                }
            } else if ok_job(e, "pack") {
                for t in &e.targets {
                    packed.insert(t.clone(), e.t);
                }
            }
        }
        let mut cc = CatalogChain { n, t: c.t, secs: c.secs, stats, tiles: reach.len(), ..Default::default() };
        for (host, hs) in &by_host {
            if let Some(n) = n {
                if let Some(a) = first_at(hs, "adopt", n, c.t.saturating_sub(120)) {
                    cc.adopted.insert(host.to_string(), a.saturating_sub(c.t) as f64);
                }
                if let Some(s) = first_at(hs, "mirror-start", n, c.t.saturating_sub(120)) {
                    let len = hs.iter().find(|e| e.kind == "mirror-end" && e.n >= n && e.t >= s).and_then(|e| e.secs.or(Some(e.t.saturating_sub(s) as f64)));
                    cc.mirrored.insert(host.to_string(), (s.saturating_sub(c.t) as f64, len));
                }
            }
        }
        let ran: BTreeSet<&str> = ev
            .iter()
            .filter(|e| COUNTED.iter().any(|s| ok_job(e, s)))
            .filter(|e| if e.step.as_deref() == Some("names-todo") { e.t > c.t && e.t <= next } else { window(e) })
            .filter_map(|e| e.step.as_deref())
            .collect();
        cc.ran = COUNTED.iter().filter(|s| ran.contains(*s)).map(|s| s.to_string()).collect();
        // Its units: those whose job ended since the catalog before.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for e in ev.iter().filter(|e| window(e)).filter(|e| ok_job(e, "unit")) {
            for u in &e.targets {
                if !seen.insert(u.clone()) {
                    continue;
                }
                let mine: Vec<&String> = reach.iter().filter(|(_, (_, us))| us.contains(u)).map(|(t, _)| t).collect();
                let mut uc = UnitChain {
                    unit: u.clone(),
                    worker: e.worker.clone(),
                    done: e.t,
                    merged: e.lease.and_then(|l| merged.get(&l)?.iter().copied().find(|t| *t >= e.t)),
                    planned: mine.iter().map(|t| reach[*t].0).min(),
                    leased: mine.iter().filter_map(|t| leased.get(*t).copied()).min(),
                    packed: if mine.is_empty() { None } else { mine.iter().map(|t| packed.get(*t).copied()).collect::<Option<Vec<u64>>>().and_then(|v| v.into_iter().max()) },
                    catalog: n,
                    catalog_t: Some(c.t),
                    restaled: mine.len(),
                    ..Default::default()
                };
                for (host, hs) in &by_host {
                    if let Some(n) = n {
                        if let Some(a) = first_at(hs, "adopt", n, c.t.saturating_sub(120)) {
                            uc.adopted.insert(host.to_string(), a);
                        }
                        if let Some(m) = first_at(hs, "mirror-end", n, c.t.saturating_sub(120)) {
                            uc.mirrored.insert(host.to_string(), m);
                        }
                    }
                }
                out_u.push(uc);
            }
        }
        cc.units = seen.len();
        out_c.push(cc);
    }
    (out_c, out_u)
}

/// The middle of `v` (the mean of the two middles); None when empty.
pub fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 { v[m] } else { (v[m - 1] + v[m]) / 2.0 })
}

fn gap(a: Option<u64>, b: Option<u64>) -> Option<f64> {
    Some(b?.checked_sub(a?)? as f64)
}

fn s(x: Option<f64>) -> String {
    x.map_or_else(|| "-".into(), crate::timings::secs)
}

/// The stages a unit goes through, as (name, from, to).
fn unit_stages(u: &UnitChain) -> Vec<(&'static str, Option<f64>)> {
    let last = |m: &BTreeMap<String, u64>| m.values().max().copied();
    let hand = u.merged.or(Some(u.done));
    vec![
        ("hand-off", gap(Some(u.done), u.merged)),
        ("to planned", gap(hand, u.planned)),
        ("to leased", gap(u.planned, u.leased)),
        ("packed", gap(u.leased.or(u.planned), u.packed)),
        ("to catalog", gap(u.packed.or(hand), u.catalog_t)),
        ("servers", gap(u.catalog_t, last(&u.adopted))),
        ("mirrors", gap(u.catalog_t, last(&u.mirrored))),
        ("on every map", gap(Some(u.done), last(&u.mirrored).max(last(&u.adopted)))),
    ]
}

/// The chains in words: the last `last` catalogs, each with its units, then the medians.
pub fn text(cats: &[CatalogChain], units: &[UnitChain], last: usize) -> String {
    let mut o = String::new();
    let cats = &cats[cats.len().saturating_sub(last)..];
    let from = cats.first().map_or(u64::MAX, |c| c.t);
    let units: Vec<&UnitChain> = units.iter().filter(|u| u.catalog_t.is_some_and(|t| t >= from)).collect();
    for c in cats {
        let kind = c.stats.as_ref().map_or("?", |s| if s.delta { "delta" } else { "whole" });
        let n = c.n.map_or_else(|| "?".into(), |n| n.to_string());
        o += &format!("catalog {n} ({kind}): {} in its job", s(c.secs));
        if let Some(st) = &c.stats {
            o += &format!(", {} of {} files stat'ed", st.stated, st.files);
        }
        o += &format!("; {} unit(s), {} map tile(s) planned; ran: {}\n", c.units, c.tiles, if c.ran.is_empty() { "none of them".into() } else { c.ran.join(", ") });
        for (h, a) in &c.adopted {
            let m = c.mirrored.get(h);
            o += &format!("  {h}: adopted after {}, mirror began after {}, took {}\n", s(Some(*a)), s(m.map(|m| m.0)), s(m.and_then(|m| m.1)));
        }
        for (h, m) in c.mirrored.iter().filter(|(h, _)| !c.adopted.contains_key(*h)) {
            o += &format!("  {h}: not adopted; mirror began after {}, took {}\n", s(Some(m.0)), s(m.1));
        }
        for u in units.iter().filter(|u| u.catalog_t == Some(c.t)) {
            let st: Vec<String> = unit_stages(u).into_iter().map(|(k, v)| format!("{k} {}", s(v))).collect();
            o += &format!("  unit {} ({}): {}; restaled {}\n", u.unit, u.worker.as_deref().unwrap_or("?"), st.join(", "), u.restaled);
        }
    }
    o += &format!("\nMedians over {} unit(s) and {} catalog(s):\n", units.len(), cats.len());
    let names: Vec<&str> = units.first().map(|u| unit_stages(u).into_iter().map(|(k, _)| k).collect()).unwrap_or_default();
    for (i, k) in names.iter().enumerate() {
        let v: Vec<f64> = units.iter().filter_map(|u| unit_stages(u)[i].1).collect();
        o += &format!("  {k}: {} ({} measured)\n", s(median(v.clone())), v.len());
    }
    o += &format!("  restaled per unit: {}\n", median(units.iter().map(|u| u.restaled as f64).collect()).map_or("-".into(), |m| format!("{m:.1}")));
    for delta in [true, false] {
        let of: Vec<&CatalogChain> = cats.iter().filter(|c| c.stats.as_ref().is_some_and(|s| s.delta == delta)).collect();
        if of.is_empty() {
            continue;
        }
        o += &format!("  {} catalogs ({}): {} in the job, {} files stat'ed\n", if delta { "delta" } else { "whole" }, of.len(), s(median(of.iter().filter_map(|c| c.secs).collect())), median(of.iter().filter_map(|c| c.stats.as_ref().map(|s| s.stated as f64)).collect()).map_or("-".into(), |m| format!("{m:.0}")));
        let mut phases: Vec<String> = Vec::new();
        for c in &of {
            for (p, _) in &c.stats.as_ref().unwrap().phases {
                if !phases.contains(p) {
                    phases.push(p.clone());
                }
            }
        }
        for p in phases {
            let v: Vec<f64> = of.iter().filter_map(|c| c.stats.as_ref().unwrap().phases.iter().find(|(q, _)| *q == p).map(|x| x.1)).collect();
            o += &format!("    {p}: {}\n", s(median(v)));
        }
    }
    let hosts: BTreeSet<&String> = cats.iter().flat_map(|c| c.adopted.keys().chain(c.mirrored.keys())).collect();
    for h in hosts {
        o += &format!(
            "  {h}: adopts after {}, mirror through after {}\n",
            s(median(cats.iter().filter_map(|c| c.adopted.get(h).copied()).collect())),
            s(median(cats.iter().filter_map(|c| c.mirrored.get(h).and_then(|m| Some(m.0 + m.1?))).collect()))
        );
    }
    for st in COUNTED {
        let k = cats.iter().filter(|c| c.ran.iter().any(|r| r == st)).count();
        o += &format!("  {st}: ran for {k} of {} catalog(s)\n", cats.len());
    }
    o
}

/// The build's history: the NAS's (`state/coord/history/<day>/*.jsonl`, the last `days` days',
/// every writer's), else the lead's own (`history`, its coordinator's file); duplicates once.
pub fn load_history(root: Option<&Path>, local: Option<&Path>, days: u64) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::new();
    let since = now_s().saturating_sub(days * 86400);
    let read = |p: &Path| -> Vec<Event> { std::fs::read_to_string(p).map(|t| t.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| e.t >= since).collect()).unwrap_or_default() };
    if let Some(dir) = root.map(|r| r.join("state/coord/history")).filter(|d| d.is_dir()) {
        for d in (0..=days).filter_map(|k| crate::pool::journal::day(since + k * 86400)) {
            if let Ok(rd) = std::fs::read_dir(dir.join(d)) {
                for f in rd.flatten() {
                    out.extend(read(&f.path()));
                }
            }
        }
    }
    if out.is_empty() {
        if let Some(l) = local {
            out.extend(read(l));
        }
    }
    let mut seen = BTreeSet::new();
    out.retain(|e| seen.insert(serde_json::to_string(&Event { seq: 0, ..e.clone() }).unwrap_or_default()));
    out.sort_by_key(|e| (e.t, e.seq));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, t: u64) -> Event {
        Event { t, ..Event::new(kind) }
    }

    fn job(kind: &str, t: u64, step: &str, targets: &[&str], lease: Option<u64>) -> Event {
        Event { step: Some(step.into()), targets: targets.iter().map(|s| s.to_string()).collect(), ok: Some(true), lease, worker: Some("m1".into()), secs: Some(10.0), ..ev(kind, t) }
    }

    fn catalog(t: u64, n: u64, delta: bool) -> Event {
        let st = CatalogStats { n, delta, files: 1000, stated: if delta { 12 } else { 1000 }, phases: vec![("every file checked on the NAS".into(), if delta { 1.0 } else { 9.0 }), ("the catalog written".into(), 2.0)] };
        Event { n: Some(n), secs: Some(if delta { 20.0 } else { 60.0 }), data: serde_json::to_value(&st).ok(), ..ev("catalog", t) }
    }

    /// A unit from its job to every Mac's map: each stage's time, its restale count, the catalog's
    /// kind, its files stat'ed, the counted steps, and the servers' adoptions and mirrors.
    #[test]
    fn a_units_chain_from_its_job_to_every_macs_map() {
        let reach: BTreeMap<String, Vec<String>> = [("6/1/1".to_string(), vec!["6/1/1".to_string()]), ("6/1/2".into(), vec!["6/1/1".into(), "6/9/9".into()]), ("6/5/5".into(), vec!["6/5/5".into()])].into();
        let events = vec![
            catalog(1000, 7, false),
            job("done", 1100, "unit", &["6/1/1"], Some(42)),
            job("end", 1110, "unit", &["6/5/5"], None),
            Event { targets: vec!["2026-10-17/3-42".into()], ..ev("merged", 1130) },
            planned_event(vec!["6/1/1".into(), "6/1/2".into(), "6/5/5".into()], reach),
            job("lease", 1200, "pack", &["6/1/1", "6/1/2"], Some(43)),
            job("start", 1205, "pack", &["6/5/5"], None),
            job("done", 1300, "pack", &["6/1/1", "6/1/2"], Some(43)),
            job("end", 1250, "pack", &["6/5/5"], None),
            job("end", 1310, "ferries", &["ferries"], None),
            job("end", 1320, "stations", &["stations"], None),
            Event { ok: Some(false), ..job("end", 1320, "marks", &["marks"], None) },
            catalog(1400, 8, true),
            job("end", 1500, "names-todo", &["names-todo"], None),
        ];
        let mut events = events;
        events[4].t = 1150;
        let servers = vec![
            ServerEvent { t: 1430, host: "m1".into(), kind: "adopt".into(), n: 8, secs: None },
            ServerEvent { t: 1440, host: "m1".into(), kind: "mirror-start".into(), n: 8, secs: None },
            ServerEvent { t: 1540, host: "m1".into(), kind: "mirror-end".into(), n: 8, secs: Some(100.0) },
            // (The M4 skipped catalog 8: a later one counts.)
            ServerEvent { t: 1600, host: "m4".into(), kind: "adopt".into(), n: 9, secs: None },
            // (Catalog 7's, before 8: not its.)
            ServerEvent { t: 1010, host: "m4".into(), kind: "adopt".into(), n: 7, secs: None },
        ];
        let (cats, units) = chains(&events, &servers);
        assert_eq!(cats.len(), 2);
        let c = &cats[1];
        assert_eq!((c.n, c.units, c.tiles), (Some(8), 2, 3));
        assert!(c.stats.as_ref().unwrap().delta);
        assert_eq!(c.stats.as_ref().unwrap().stated, 12);
        assert_eq!(c.ran, ["ferries", "stations", "names-todo"]);
        assert_eq!(c.adopted, [("m1".to_string(), 30.0), ("m4".into(), 200.0)].into());
        assert_eq!(c.mirrored["m1"], (40.0, Some(100.0)));
        assert_eq!(cats[0].ran, Vec::<String>::new());
        let a = units.iter().find(|u| u.unit == "6/1/1").unwrap();
        assert_eq!((a.done, a.merged, a.planned, a.leased, a.packed, a.catalog, a.restaled), (1100, Some(1130), Some(1150), Some(1200), Some(1300), Some(8), 2));
        assert_eq!(a.adopted, [("m1".to_string(), 1430), ("m4".into(), 1600)].into());
        assert_eq!(a.mirrored, [("m1".to_string(), 1540)].into());
        let b = units.iter().find(|u| u.unit == "6/5/5").unwrap();
        assert_eq!((b.merged, b.leased, b.packed, b.restaled), (None, Some(1205), Some(1250), 1));
        let stages: BTreeMap<&str, Option<f64>> = unit_stages(a).into_iter().collect();
        assert_eq!(stages["hand-off"], Some(30.0));
        assert_eq!(stages["to planned"], Some(20.0));
        assert_eq!(stages["packed"], Some(100.0));
        assert_eq!(stages["to catalog"], Some(100.0));
        assert_eq!(stages["servers"], Some(200.0));
        assert_eq!(stages["on every map"], Some(500.0));
        let t = text(&cats, &units, 10);
        assert!(t.contains("catalog 8 (delta)"), "{t}");
        assert!(t.contains("12 of 1000 files stat'ed"), "{t}");
        assert!(t.contains("unit 6/1/1 (m1)") && t.contains("restaled 2"), "{t}");
        assert!(t.contains("delta catalogs (1)") && t.contains("whole catalogs (1)"), "{t}");
        assert!(t.contains("ferries: ran for 1 of 2"), "{t}");
        assert!(t.contains("m4: adopts after"), "{t}");
        // (--last 1: the last catalog alone.)
        assert!(!text(&cats, &units, 1).contains("catalog 7"));
    }

    #[test]
    fn the_medians_and_a_catalogs_line() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
        let st = CatalogStats { n: 5, delta: true, files: 10, stated: 2, phases: vec![("x".into(), 1.5)] };
        let log = format!("catalog: something\n{}\npublished /x/5.json\n", st.line());
        assert_eq!(CatalogStats::from_log(&log), Some(st));
        assert_eq!(CatalogStats::from_log("nothing"), None);
    }

    #[test]
    fn the_mirror_says_each_catalogs_start_and_end_once() {
        let mut m = MirrorMarks::default();
        assert_eq!(m.end(3, 10, true), None);
        assert!(m.start(3, 100));
        assert!(!m.start(3, 160));
        assert_eq!(m.end(3, 170, false), None);
        assert_eq!(m.end(3, 220, true), Some(120.0));
        assert_eq!(m.end(3, 280, true), None);
        assert!(m.start(4, 300));
        assert_eq!(m.end(3, 310, true), None);
    }

    #[test]
    fn the_servers_log_is_appended_per_mac_and_read_in_order() {
        let d = tempfile::tempdir().unwrap();
        append_server(d.path(), ServerEvent { t: 20, host: "Brandon's M4".into(), kind: "adopt".into(), n: 2, secs: None }).unwrap();
        append_server(d.path(), ServerEvent { t: 10, host: "m1".into(), kind: "adopt".into(), n: 1, secs: None }).unwrap();
        append_server(d.path(), ServerEvent { t: 30, host: "m1".into(), kind: "mirror-end".into(), n: 2, secs: Some(5.0) }).unwrap();
        let files: Vec<String> = std::fs::read_dir(d.path().join(SERVERS)).unwrap().map(|f| f.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(files.len(), 2);
        let all = read_servers(d.path(), 0);
        assert_eq!(all.iter().map(|e| e.t).collect::<Vec<_>>(), [10, 20, 30]);
        assert_eq!(read_servers(d.path(), 15).len(), 2);
    }

    #[test]
    fn the_units_reaching_a_map_tile() {
        let m: BTreeMap<String, String> = [("base/6-28-16".to_string(), "c".to_string()), ("base/6-40-20".into(), "d".into())].into();
        let r = reaching(&m, None, &["6/28/16".into(), "6/40/20".into(), "6/0/0".into()]);
        assert_eq!(r["6/28/16"], ["6/28/16"]);
        assert_eq!(r["6/40/20"], ["6/40/20"]);
        assert!(r["6/0/0"].is_empty());
    }

    #[test]
    fn the_history_from_the_nas_every_writers_once() {
        let d = tempfile::tempdir().unwrap();
        let t = now_s();
        let day = crate::pool::journal::day(t).unwrap();
        let dir = d.path().join("state/coord/history").join(&day);
        std::fs::create_dir_all(&dir).unwrap();
        let line = |e: &Event| serde_json::to_string(e).unwrap() + "\n";
        let a = Event { seq: 1, ..ev("catalog", t) };
        let b = Event { seq: 2, ..ev("merged", t + 1) };
        std::fs::write(dir.join("m-a.jsonl"), line(&a) + &line(&b)).unwrap();
        std::fs::write(dir.join("m-b.jsonl"), line(&Event { seq: 9, ..a.clone() })).unwrap();
        let h = load_history(Some(d.path()), None, 2);
        assert_eq!(h.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), ["catalog", "merged"]);
        let local = d.path().join("h.jsonl");
        std::fs::write(&local, line(&b)).unwrap();
        assert_eq!(load_history(None, Some(&local), 2).len(), 1);
    }
}
