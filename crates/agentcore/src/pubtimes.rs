//! Publishing timed end to end (task #250; docs/building.md §2, The map updates' times): from a build
//! z6 tile done to its map tiles on every Mac's map, by the events each stage leaves.
//!
//! - **In the build's history** (crate::coord::history, the lead's): a z6 tile's job ended ("end",
//!   "done"); its hand-off merged ("merged": `merged_event`, each entry's key, member, step and
//!   targets); the map tiles (pack) planned ("planned": `planned_event`, the z6 tiles the lead
//!   first plans at their keys, and the z6 tiles whose change made each stale, `PlanHeld::restaled_by`), leased
//!   ("lease", "start") and done ("end", "done"); and catalogue n written ("catalog": its `n`, its
//!   job's time, and `CatalogueStats`: the files it stat'ed, its phases).
//!   And each Mac's map server's adoption of the map at `seq` n ("adopt": `ADOPT`, its `worker` the
//!   Mac), the lead's note of its cursor in the map's log moving there (docs/pool.md §10), and its
//!   mirror's round copying map n begun ("mirror-start": `MIRROR_START`) and through everything
//!   wanted ("mirror-end": `MIRROR_END`), the lead's notes of what the mirror's own cursor said
//!   (docs/macs.md, Mirror), each once per `seq` heard.
//!
//! `chains` joins them, per catalogue and per z6 tile; `scenic timings publish` prints them.

use crate::coord::history::Event;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The history's event of a map server's adoption of the map at its `n` (at its `t`; its Mac the
/// `worker`).
pub const ADOPT: &str = "adopt";
/// The history's events of a map server's mirror's round copying the map at its `n` begun, and
/// through everything wanted (at their `t`; its Mac the `worker`).
pub const MIRROR_START: &str = "mirror-start";
pub const MIRROR_END: &str = "mirror-end";

/// The steps whose runs are counted per catalogue (besides the map tiles').
pub const COUNTED: [&str; 5] = ["ferries", "stations", "roadunits", "marks", "names-todo"];

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What a catalogue's job said of itself (a line of its log, `CATALOGUE_LINE`, which the agent reads
/// into the history's "catalog" event).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogueStats {
    pub n: u64,
    /// The files it listed, and those it looked at on the NAS (stat'ed). (A line in the history
    /// from when catalogues were files says too whether it wrote a delta: passed over.)
    pub files: u64,
    pub stated: u64,
    /// Its phases' wall times, in order (a name from before task #844 read as its new one:
    /// jobrt::phase_names).
    #[serde(default, deserialize_with = "current_names")]
    pub phases: Vec<(String, f64)>,
}

fn current_names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<(String, f64)>, D::Error> {
    let v = Vec::<(String, f64)>::deserialize(d)?;
    Ok(v.into_iter().map(|(n, s)| (jobrt::phase_names::current(&n).into_owned(), s)).collect())
}

/// The start of a catalogue's line in its log: stored in the job logs on the NAS, so it keeps its old
/// spelling (task #547).
pub const CATALOGUE_LINE: &str = "catalog-stats: ";

impl CatalogueStats {
    /// Catalogue `n`'s, with the job's phases as timed so far (jobrt::timings; none untimed).
    pub fn now(n: u64, files: u64, stated: u64) -> CatalogueStats {
        let phases = jobrt::timings::snapshot(true)
            .map(|r| r.phases.into_iter().map(|p| (p.name, p.wall_s)).collect())
            .unwrap_or_default();
        CatalogueStats { n, files, stated, phases }
    }

    pub fn line(&self) -> String {
        format!("{CATALOGUE_LINE}{}", serde_json::to_string(self).unwrap_or_default())
    }

    /// The last such line in a log's tail.
    pub fn from_log(tail: &str) -> Option<CatalogueStats> {
        tail.lines().rev().find_map(|l| {
            l.find(CATALOGUE_LINE).and_then(|i| serde_json::from_str(&l[i + CATALOGUE_LINE.len()..]).ok())
        })
    }
}

/// The served catalogue's base packs' and road values' content names, by logical name (what
/// `PlanHeld::restaled_by` compares with).
pub fn served_units(c: &store::catalogue::Catalogue) -> BTreeMap<String, String> {
    c.files
        .iter()
        .filter(|(l, _)| l.starts_with("base/") || l.starts_with("global/roads/"))
        .map(|(l, f)| (l.clone(), f.file.clone()))
        .collect()
}

/// How much of a planned tile's key a "planned" event keeps: enough to tell its keys apart.
const KEY_KEPT: usize = 12;

fn cut(k: &str) -> String {
    k.chars().take(KEY_KEPT).collect()
}

/// A "planned" event: map tiles first planned at their keys (`tiles`: (tile, key)), and the z6 tiles
/// whose change made them stale (`PlanHeld::restaled_by`). Small: the keys cut short, each z6 tile its own tiles.
pub fn planned_event(tiles: &[(String, String)], by_unit: BTreeMap<String, Vec<String>>) -> Event {
    let keys: Vec<String> = tiles.iter().map(|(_, k)| cut(k)).collect();
    Event {
        step: Some("pack".into()),
        targets: tiles.iter().map(|t| t.0.clone()).collect(),
        // (`units`: the stored key of the z6 tiles by their changed map tiles, in the history's events: kept.)
        data: Some(serde_json::json!({ "k": keys, "units": by_unit })),
        ..Event::new("planned")
    }
}

/// The (tile, key) pairs `events` say were planned since `since` (keys as `planned_event` keeps
/// them): what a lead, restarted or new, noted already since the last catalogue.
pub fn planned_since<'a>(events: impl IntoIterator<Item = &'a Event>, since: u64) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for e in events.into_iter().filter(|e| e.kind == "planned" && e.t >= since) {
        let keys: Vec<String> =
            e.data.as_ref().and_then(|d| serde_json::from_value(d["k"].clone()).ok()).unwrap_or_default();
        out.extend(e.targets.iter().cloned().zip(keys));
    }
    out
}

/// Of `tiles` (tile, key), those `noted` doesn't hold at that key, once each: a tile stale again at
/// a new key is noted again.
pub fn not_noted(tiles: &[(String, String)], noted: &BTreeSet<(String, String)>) -> Vec<(String, String)> {
    let mut seen = BTreeSet::new();
    tiles.iter().filter(|(t, k)| !noted.contains(&(t.clone(), cut(k))) && seen.insert(t.clone())).cloned().collect()
}

/// Notes `new` in `noted` (keys cut as `planned_event` keeps them).
pub fn note_planned(noted: &mut BTreeSet<(String, String)>, new: &[(String, String)]) {
    noted.extend(new.iter().map(|(t, k)| (t.clone(), cut(k))));
}

/// Notes begun this long ago (seconds) or less (`PlannedNotes::begin`) have nothing noted by another
/// lead to read.
pub const JUST_BEGUN_S: u64 = 120;

/// What the lead noted planned since the last catalogue (`planned_event`), by tile and key, and
/// the served catalogue's z6 tiles as last read (`served_units`, once a catalogue). What it noted before
/// a restart comes back from its coordinator's history (`begin`); before a change of lead, from
/// the NAS's archive of the history (its finished days: the lead before's day it moved on is on
/// that lead's disk alone), read on a thread of its own (`seed_from_nas`) so a slow share never
/// holds the plan.
#[derive(Debug, Default)]
pub struct PlannedNotes {
    began: u64,
    noted: BTreeSet<(String, String)>,
    served: Option<(u64, BTreeMap<String, String>)>,
    /// The served catalogues read (for the tests).
    pub served_reads: u32,
    seed: std::sync::Arc<std::sync::Mutex<Option<(u64, BTreeSet<(String, String)>)>>>,
}

impl PlannedNotes {
    /// When its notes begin (the last catalogue's time).
    pub fn began(&self) -> u64 {
        self.began
    }

    /// Its notes begin at `b` (the last catalogue's time); `own`: this coordinator's history. On a new
    /// beginning, what `own` says was noted since; true when the NAS's must be read too
    /// (`seed_from_nas`): `own` doesn't reach back to `b`, and it didn't just begin (at `now`).
    pub fn begin(&mut self, b: u64, own: &[Event], now: u64) -> bool {
        if self.began == b {
            return false;
        }
        self.began = b;
        self.noted = planned_since(own, b);
        let covered = own.first().is_some_and(|e| e.t <= b);
        !covered && now.saturating_sub(b) > JUST_BEGUN_S
    }

    /// The NAS's archive of the history since its notes began (`root`), read on a thread; taken up
    /// by the next `new_tiles`.
    pub fn seed_from_nas(&self, root: store::nasfs::NasRoot) {
        let (b, seed) = (self.began, self.seed.clone());
        let spawned = std::thread::Builder::new().name("planned-seed".into()).spawn(move || {
            let s = Self::nas_noted(&root, b);
            *seed.lock().unwrap_or_else(|e| e.into_inner()) = Some((b, s));
        });
        if let Err(e) = spawned {
            eprintln!("agent: reading what was planned from the NAS: {e}");
        }
    }

    /// What the NAS's archive of the history (`root`) says was noted planned since `b`.
    pub fn nas_noted(root: &store::nasfs::NasRoot, b: u64) -> BTreeSet<(String, String)> {
        let days = now_s().saturating_sub(b) / 86400 + 1;
        planned_since(&archived(root, now_s().saturating_sub(days * 86400), days), b)
    }

    /// Of `tiles` (tile, key), those not noted yet (the NAS's seed taken up first, if it came).
    pub fn new_tiles(&mut self, tiles: &[(String, String)]) -> Vec<(String, String)> {
        if let Some((b, s)) = self.seed.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if b == self.began {
                self.noted.extend(s);
            }
        }
        not_noted(tiles, &self.noted)
    }

    /// `new` noted.
    pub fn note(&mut self, new: &[(String, String)]) {
        note_planned(&mut self.noted, new);
    }

    /// The served catalogue's z6 tiles (`cat`: the map as the plan reads it, crate::agent::plan's
    /// `map_now`), worked out again only for a new `seq`.
    pub fn served(&mut self, cat: Option<&store::catalogue::Catalogue>) -> BTreeMap<String, String> {
        if let Some(c) = cat.filter(|c| self.served.as_ref().map(|s| s.0) != Some(c.n)) {
            self.served_reads += 1;
            self.served = Some((c.n, served_units(c)));
        }
        self.served.as_ref().map(|s| s.1.clone()).unwrap_or_default()
    }
}

/// A "merged" event: the hand-offs merged's jobs (crate::pool::records::Merged::done).
pub fn merged_event(done: &[crate::pool::records::MergedJob]) -> Event {
    Event {
        targets: done.iter().map(|d| d.0.clone()).collect(),
        data: Some(serde_json::json!({ "done": done })),
        ..Event::new("merged")
    }
}

/// A z6 tile's way to the map: when each stage was reached (unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UnitChain {
    pub unit: String,
    pub worker: Option<String>,
    /// Its job ended.
    pub done: u64,
    /// Its hand-off merged (a member's job; the lead's own have none).
    pub merged: Option<u64>,
    /// The map tiles its change made stale: first planned, first leased, last done.
    pub planned: Option<u64>,
    pub leased: Option<u64>,
    pub packed: Option<u64>,
    /// The catalogue it's in: the first begun once it was merged and packed.
    // The JSON keys keep their old spelling (task #547): the build page reads them.
    #[serde(rename = "catalog")]
    pub catalogue: Option<u64>,
    #[serde(rename = "catalog_t")]
    pub catalogue_t: Option<u64>,
    /// Each Mac's server adopting it (that catalogue or a later one), and its mirror through it.
    pub adopted: BTreeMap<String, u64>,
    pub mirrored: BTreeMap<String, u64>,
    /// The map tiles its change made stale (`PlanHeld::restaled_by`), as planned after its hand-off.
    pub restaled: usize,
}

/// A catalogue's publishing.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CatalogueChain {
    pub n: Option<u64>,
    pub t: u64,
    /// Its job's time.
    pub secs: Option<f64>,
    pub stats: Option<CatalogueStats>,
    /// Each Mac's server: seconds from the catalogue to its adoption; its mirror's sync (from the
    /// catalogue to its start, and its length).
    pub adopted: BTreeMap<String, f64>,
    pub mirrored: BTreeMap<String, (f64, Option<f64>)>,
    /// The z6 tiles in it, and the map tiles planned between the catalogue before's beginning and its own.
    pub units: usize,
    pub tiles: usize,
    /// Of `COUNTED`, the steps that ran for it (between the same beginnings; names-todo after it).
    pub ran: Vec<String>,
}

fn ok_job(e: &Event, step: &str) -> bool {
    matches!(e.kind.as_str(), "end" | "done") && e.ok == Some(true) && e.step.as_deref() == Some(step)
}

/// When a catalogue's job began (it read the manifest then): its end less its time.
fn began(c: &Event) -> u64 {
    c.t.saturating_sub(c.secs.unwrap_or(0.0).ceil() as u64)
}

/// Whether a merged hand-off's job (`merged`, its key's id) is the run a "done" event names
/// (`done`): the same id, or, for an event from before job ids (read as epoch 0,
/// jobrt::jobid), the same lease number.
fn same_run(merged: jobrt::jobid::JobId, done: jobrt::jobid::JobId) -> bool {
    merged == done || (done.epoch == 0 && merged.n == done.n)
}

/// The chains of the catalogues in `events` (any order) and their z6 tiles, with the servers' and
/// their mirrors' marks; oldest first. A z6 tile is in the first catalogue begun once its hand-off was merged (a
/// member's job; the lead's own, at its end) and the map tiles its change made stale were packed;
/// a catalogue's map tiles and steps are those between the catalogue before's beginning and its own.
pub fn chains(events: &[Event]) -> (Vec<CatalogueChain>, Vec<UnitChain>) {
    let mut ev: Vec<&Event> = events.iter().collect();
    ev.sort_by_key(|e| (e.t, e.seq));
    let cats: Vec<&Event> = ev.iter().copied().filter(|e| e.kind == store::catalogue::NAME).collect();
    // The merges' jobs: (time, job id, step, targets).
    let mut merges: Vec<(u64, Option<jobrt::jobid::JobId>, String, Vec<String>)> = Vec::new();
    for e in ev.iter().filter(|e| e.kind == "merged") {
        let done: Vec<crate::pool::records::MergedJob> =
            e.data.as_ref().and_then(|d| serde_json::from_value(d["done"].clone()).ok()).unwrap_or_default();
        for (key, _, step, targets) in done {
            // (`<day>/<epoch>-<n>`: its job's id.)
            let n = key.rsplit('/').next().and_then(|l| l.parse().ok());
            merges.push((e.t, n, step, targets));
        }
    }
    // (The servers' adoptions and their mirrors' rounds, by Mac.)
    let mut by_host: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for e in ev.iter().filter(|e| [ADOPT, MIRROR_START, MIRROR_END].contains(&e.kind.as_str()) && e.n.is_some()) {
        if let Some(host) = e.worker.as_deref() {
            by_host.entry(host).or_default().push(e);
        }
    }
    let first_at = |host: &[&Event], kind: &str, n: u64, after: u64| {
        host.iter().filter(|e| e.kind == kind && e.n >= Some(n) && e.t >= after).map(|e| e.t).min()
    };
    let stats_of =
        |c: &Event| -> Option<CatalogueStats> { c.data.as_ref().and_then(|d| serde_json::from_value(d.clone()).ok()) };
    let n_of = |c: &Event| c.n.or(stats_of(c).map(|s| s.n));
    // Each planned tile: when, which, and the z6 tiles whose change made it stale.
    let planned: Vec<(u64, String, Vec<String>)> = ev
        .iter()
        .filter(|e| e.kind == "planned")
        .flat_map(|e| {
            let units: BTreeMap<String, Vec<String>> =
                e.data.as_ref().and_then(|d| serde_json::from_value(d["units"].clone()).ok()).unwrap_or_default();
            e.targets
                .iter()
                .map(move |t| {
                    (e.t, t.clone(), units.iter().filter(|(_, ts)| ts.contains(t)).map(|(u, _)| u.clone()).collect())
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let pack_at = |tile: &str, after: u64, kinds: &[&str]| {
        ev.iter()
            .filter(|e| {
                e.t >= after
                    && e.step.as_deref() == Some("pack")
                    && kinds.contains(&e.kind.as_str())
                    && e.ok != Some(false)
                    && e.targets.iter().any(|t| t == tile)
            })
            .map(|e| e.t)
            .min()
    };
    let mut units: Vec<UnitChain> = Vec::new();
    let unit_ends: Vec<&Event> = ev.iter().copied().filter(|e| ok_job(e, "unit")).collect();
    for (i, e) in unit_ends.iter().enumerate() {
        for u in &e.targets {
            // (Its next build's are that one's.)
            let until = unit_ends[i + 1..].iter().find(|f| f.targets.contains(u)).map_or(u64::MAX, |f| f.t);
            let merged = merges
                .iter()
                .find(|(t, n, step, targets)| {
                    *t >= e.t
                        && *t < until
                        && step == "unit"
                        && targets.contains(u)
                        && e.job.is_none_or(|j| n.is_some_and(|n| same_run(n, j)))
                })
                .map(|m| m.0);
            let hand = merged.unwrap_or(e.t);
            let mut mine: BTreeMap<&str, u64> = BTreeMap::new();
            for (t, tile, _) in planned.iter().filter(|(t, _, us)| *t >= hand && *t < until && us.contains(u)) {
                mine.entry(tile.as_str()).or_insert(*t);
            }
            let leased = mine.iter().filter_map(|(tile, t)| pack_at(tile, *t, &["lease", "start"])).min();
            let packs: Vec<Option<u64>> = mine.iter().map(|(tile, t)| pack_at(tile, *t, &["end", "done"])).collect();
            let all_packed: Option<Vec<u64>> = packs.iter().copied().collect();
            let packed = all_packed.as_ref().and_then(|v| v.iter().max().copied());
            // (Its catalogue: the first begun once it was merged and packed; a tile not packed yet, none.)
            let cat =
                all_packed.map(|_| packed.unwrap_or(hand).max(hand)).and_then(|r| cats.iter().find(|c| began(c) >= r));
            let mut uc = UnitChain {
                unit: u.clone(),
                worker: e.worker.clone(),
                done: e.t,
                merged,
                planned: mine.values().min().copied(),
                leased,
                packed,
                catalogue: cat.and_then(|c| n_of(c)),
                catalogue_t: cat.map(|c| c.t),
                restaled: mine.len(),
                ..Default::default()
            };
            if let Some(n) = uc.catalogue {
                let after = uc.catalogue_t.unwrap_or(0).saturating_sub(120);
                for (host, hs) in &by_host {
                    if let Some(a) = first_at(hs, ADOPT, n, after) {
                        uc.adopted.insert(host.to_string(), a);
                    }
                    if let Some(m) = first_at(hs, MIRROR_END, n, after) {
                        uc.mirrored.insert(host.to_string(), m);
                    }
                }
            }
            units.push(uc);
        }
    }
    let mut out = Vec::new();
    for (i, c) in cats.iter().enumerate() {
        let (from, to) = (if i == 0 { 0 } else { began(cats[i - 1]) }, began(c));
        let next = cats.get(i + 1).map_or(u64::MAX, |n| n.t);
        let n = n_of(c);
        let tiles: BTreeSet<&str> =
            planned.iter().filter(|(t, _, _)| *t > from && *t <= to).map(|p| p.1.as_str()).collect();
        let mut cc = CatalogueChain {
            n,
            t: c.t,
            secs: c.secs,
            stats: stats_of(c),
            tiles: tiles.len(),
            units: units.iter().filter(|u| u.catalogue_t == Some(c.t)).count(),
            ..Default::default()
        };
        if let Some(n) = n {
            for (host, hs) in &by_host {
                if let Some(a) = first_at(hs, ADOPT, n, c.t.saturating_sub(120)) {
                    cc.adopted.insert(host.to_string(), a.saturating_sub(c.t) as f64);
                }
                if let Some(s) = first_at(hs, MIRROR_START, n, c.t.saturating_sub(120)) {
                    let len = hs
                        .iter()
                        .find(|e| e.kind == MIRROR_END && e.n >= Some(n) && e.t >= s)
                        .and_then(|e| e.secs.or(Some(e.t.saturating_sub(s) as f64)));
                    cc.mirrored.insert(host.to_string(), (s.saturating_sub(c.t) as f64, len));
                }
            }
        }
        let ran: BTreeSet<&str> = ev
            .iter()
            .filter(|e| COUNTED.iter().any(|s| ok_job(e, s)))
            .filter(|e| {
                if e.step.as_deref() == Some("names-todo") {
                    e.t > c.t && e.t <= next
                } else {
                    e.t > from && e.t <= to
                }
            })
            .filter_map(|e| e.step.as_deref())
            .collect();
        cc.ran = COUNTED.iter().filter(|s| ran.contains(*s)).map(|s| s.to_string()).collect();
        out.push(cc);
    }
    (out, units)
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
    x.map_or_else(|| "-".into(), jobrt::timings::secs)
}

/// The stages a z6 tile goes through, each's time.
fn unit_stages(u: &UnitChain) -> Vec<(&'static str, Option<f64>)> {
    let last = |m: &BTreeMap<String, u64>| m.values().max().copied();
    let hand = u.merged.or(Some(u.done));
    vec![
        ("hand-off", gap(Some(u.done), u.merged)),
        ("to planned", gap(hand, u.planned)),
        ("to leased", gap(u.planned, u.leased)),
        ("packed", gap(u.leased.or(u.planned), u.packed)),
        ("to map update", gap(u.packed.or(hand), u.catalogue_t)),
        ("servers", gap(u.catalogue_t, last(&u.adopted))),
        ("mirrors", gap(u.catalogue_t, last(&u.mirrored))),
        ("on every map", gap(Some(u.done), last(&u.mirrored).max(last(&u.adopted)))),
    ]
}

/// The chains in words: the last `last` catalogues, each with its z6 tiles, then the medians.
pub fn text(cats: &[CatalogueChain], units: &[UnitChain], last: usize) -> String {
    let mut o = String::new();
    let cats = &cats[cats.len().saturating_sub(last)..];
    let from = cats.first().map_or(u64::MAX, |c| c.t);
    let units: Vec<&UnitChain> = units.iter().filter(|u| u.catalogue_t.is_some_and(|t| t >= from)).collect();
    for c in cats {
        let n = c.n.map_or_else(|| "?".into(), |n| n.to_string());
        o += &format!("map update {n}: {} in its job", s(c.secs));
        if let Some(st) = &c.stats {
            o += &format!(", {} of {} files stat'ed", st.stated, st.files);
        }
        o += &format!(
            "; {} unit(s), {} map tile(s) planned; ran: {}\n",
            c.units,
            c.tiles,
            if c.ran.is_empty() { "none of them".into() } else { c.ran.join(", ") }
        );
        for (h, a) in &c.adopted {
            let m = c.mirrored.get(h);
            o += &format!(
                "  {h}: adopted after {}, mirror began after {}, took {}\n",
                s(Some(*a)),
                s(m.map(|m| m.0)),
                s(m.and_then(|m| m.1))
            );
        }
        for (h, m) in c.mirrored.iter().filter(|(h, _)| !c.adopted.contains_key(*h)) {
            o += &format!("  {h}: not adopted; mirror began after {}, took {}\n", s(Some(m.0)), s(m.1));
        }
        for u in units.iter().filter(|u| u.catalogue_t == Some(c.t)) {
            let st: Vec<String> = unit_stages(u).into_iter().map(|(k, v)| format!("{k} {}", s(v))).collect();
            o += &format!(
                "  unit {} ({}): {}; restaled {}\n",
                u.unit,
                u.worker.as_deref().unwrap_or("?"),
                st.join(", "),
                u.restaled
            );
        }
    }
    o += &format!("\nMedians over {} unit(s) and {} map update(s):\n", units.len(), cats.len());
    let names: Vec<&str> =
        units.first().map(|u| unit_stages(u).into_iter().map(|(k, _)| k).collect()).unwrap_or_default();
    for (i, k) in names.iter().enumerate() {
        let v: Vec<f64> = units.iter().filter_map(|u| unit_stages(u)[i].1).collect();
        o += &format!("  {k}: {} ({} measured)\n", s(median(v.clone())), v.len());
    }
    o += &format!(
        "  restaled per unit: {}\n",
        median(units.iter().map(|u| u.restaled as f64).collect()).map_or("-".into(), |m| format!("{m:.1}"))
    );
    let of: Vec<&CatalogueStats> = cats.iter().filter_map(|c| c.stats.as_ref()).collect();
    if !of.is_empty() {
        let secs: Vec<f64> = cats.iter().filter(|c| c.stats.is_some()).filter_map(|c| c.secs).collect();
        o += &format!(
            "  map updates ({}): {} in the job, {} files stat'ed\n",
            of.len(),
            s(median(secs)),
            median(of.iter().map(|s| s.stated as f64).collect()).map_or("-".into(), |m| format!("{m:.0}"))
        );
        let mut phases: Vec<&String> = Vec::new();
        for st in &of {
            for (p, _) in &st.phases {
                if !phases.contains(&p) {
                    phases.push(p);
                }
            }
        }
        for p in phases {
            let v: Vec<f64> = of.iter().filter_map(|st| st.phases.iter().find(|(q, _)| q == p).map(|x| x.1)).collect();
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
        o += &format!("  {st}: ran for {k} of {} map update(s)\n", cats.len());
    }
    o
}

/// The build's history of the last `days` days: the NAS's archive (`state/coord/history-archive/
/// <day>.tar`, a finished day packed by GC: agent::histarchive), with the lead's own: the file
/// `local` (this Mac's coordinator's, or one named) and, with the root given, the lead's coordinator's
/// asked (`/api/history`: the last week's); duplicates once; the runs of jobs across a lead's
/// restart mended (crate::coord::history::mend_runs).
pub fn load_history(root: Option<&store::nasfs::NasRoot>, local: Option<&Path>, days: u64) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::new();
    let since = now_s().saturating_sub(days * 86400);
    let lines = |t: &str| -> Vec<Event> {
        t.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| e.t >= since).collect()
    };
    if let Some(r) = root {
        out.extend(archived(r, since, days));
    }
    if let Some(t) = local.and_then(|l| std::fs::read_to_string(l).ok()) {
        out.extend(lines(&t));
    }
    // (Asked of the lead too whenever the pool's root is given: a file here is the lead's only while
    // this Mac leads, and a Mac that led before keeps its old one. Its lines twice are kept once.)
    if root.is_some() {
        out.extend(asked_of_the_lead(root).into_iter().filter(|e| e.t >= since));
    }
    let mut seen = BTreeSet::new();
    out.retain(|e| seen.insert(serde_json::to_string(&Event { seq: 0, ..e.clone() }).unwrap_or_default()));
    out.sort_by_key(|e| (e.t, e.seq));
    crate::coord::history::mend_runs(&mut out);
    out
}

/// The NAS's archive of the history (agent::histarchive), its days from `since` (unix s), `days` of
/// them, events from `since` on.
fn archived(root: &store::nasfs::NasRoot, since: u64, days: u64) -> Vec<Event> {
    let arch = root.join(crate::agent::histarchive::ARCHIVE);
    let mut out = Vec::new();
    for d in (0..=days).filter_map(|k| crate::pool::journal::day(since + k * 86400)) {
        let tar = arch.join(format!("{d}.tar"));
        if let Ok(members) = tar.read().map_err(anyhow::Error::from).and_then(|t| codecs::tar::list_tar(&t)) {
            for (_, b) in members {
                let t = String::from_utf8_lossy(&b);
                out.extend(t.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| e.t >= since));
            }
        }
    }
    out
}

/// The lead's history as its coordinator answers it (`/api/history`, a page of events at a time):
/// none when there's no lead to ask, or it doesn't answer.
fn asked_of_the_lead(root: Option<&store::nasfs::NasRoot>) -> Vec<Event> {
    let home = coordwire::lead::this_macs_agent();
    let Ok(Some(c)) = crate::coord::client::Client::to_lead(home.as_deref(), root, &store::sys::host_name()) else {
        return Vec::new();
    };
    let mut out: Vec<Event> = Vec::new();
    let mut after = 0;
    // (At most 200 pages of 5,000: a week's history, `history::KEEP_MAX`, ten times over.)
    for _ in 0..200 {
        let Ok((200, v)) = c.post_json("/api/history", &serde_json::json!({ "since": after, "max": 5000 })) else {
            break;
        };
        let page: Vec<Event> = serde_json::from_value(v["events"].clone()).unwrap_or_default();
        let Some(last) = page.last().map(|e| e.seq) else { break };
        let full = page.len() >= 5000;
        out.extend(page);
        if !full || last <= after {
            break;
        }
        after = last;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, t: u64) -> Event {
        Event { t, ..Event::new(kind) }
    }

    fn job(kind: &str, t: u64, step: &str, targets: &[&str], lease: Option<u64>) -> Event {
        Event {
            step: Some(step.into()),
            targets: targets.iter().map(|s| s.to_string()).collect(),
            ok: Some(true),
            job: lease.map(|n| jobrt::jobid::JobId { epoch: 0, n }),
            worker: Some("m1".into()),
            secs: Some(10.0),
            ..ev(kind, t)
        }
    }

    /// Catalogue `n` written at `t`, its job `secs` long, its files' sizes mostly known (`few`
    /// stat'ed) or each looked at.
    fn catalogue(t: u64, secs: f64, n: u64, few: bool) -> Event {
        let st = CatalogueStats {
            n,
            files: 1000,
            stated: if few { 12 } else { 1000 },
            phases: vec![
                ("checking every file on the NAS".into(), if few { 1.0 } else { 9.0 }),
                ("writing the catalog".into(), 2.0),
            ],
        };
        Event { n: Some(n), secs: Some(secs), data: serde_json::to_value(&st).ok(), ..ev("catalog", t) }
    }

    fn merged(t: u64, key: &str, step: &str, targets: &[&str]) -> Event {
        Event {
            t,
            ..merged_event(&[(key.into(), "m-b".into(), step.into(), targets.iter().map(|s| s.to_string()).collect())])
        }
    }

    fn planned(t: u64, tiles: &[&str], units: &[(&str, &[&str])]) -> Event {
        let tiles: Vec<(String, String)> = tiles.iter().map(|t| (t.to_string(), format!("key-of-{t}"))).collect();
        let by: BTreeMap<String, Vec<String>> =
            units.iter().map(|(u, ts)| (u.to_string(), ts.iter().map(|s| s.to_string()).collect())).collect();
        Event { t, ..planned_event(&tiles, by) }
    }

    /// One of publishing's marks the lead notes from a cursor (crate::agent's publish_marks).
    fn mark(kind: &str, t: u64, host: &str, n: u64) -> Event {
        Event { t, worker: Some(host.into()), n: Some(n), ..Event::new(kind) }
    }

    /// A mirror's times are the lead's marks in the history, each round's first: a round begun
    /// again (the server restarted) or an older seq's end counts for nothing newer.
    #[test]
    fn mirror_times_come_from_the_history() {
        let events = vec![
            catalogue(1000, 10.0, 5, false),
            mark(MIRROR_END, 1005, "m1", 4),
            mark(MIRROR_START, 1030, "m1", 5),
            mark(MIRROR_START, 1060, "m1", 5),
            mark(MIRROR_END, 1090, "m1", 5),
        ];
        let (cats, _) = chains(&events);
        assert_eq!(cats[0].mirrored, [("m1".to_string(), (30.0, Some(60.0)))].into());
    }

    /// A z6 tile from its job to every Mac's map: each stage's time, its restale count, the catalogue's
    /// kind, its files stat'ed, the counted steps, and the servers' adoptions and mirrors.
    #[test]
    fn a_units_chain_from_its_job_to_every_macs_map() {
        let events = vec![
            catalogue(1000, 60.0, 7, false),
            job("done", 1100, "unit", &["6/1/1"], Some(42)),
            job("end", 1110, "unit", &["6/5/5"], None),
            merged(1130, "2026-10-17/3-42", "unit", &["6/1/1"]),
            // (6/1/2 owned by 6/1/1 and 6/9/9, but only 6/1/1 changed.)
            planned(1150, &["6/1/1", "6/1/2", "6/5/5"], &[("6/1/1", &["6/1/1", "6/1/2"]), ("6/5/5", &["6/5/5"])]),
            job("lease", 1200, "pack", &["6/1/1", "6/1/2"], Some(43)),
            job("start", 1205, "pack", &["6/5/5"], None),
            job("end", 1250, "pack", &["6/5/5"], None),
            job("done", 1300, "pack", &["6/1/1", "6/1/2"], Some(43)),
            job("end", 1310, "ferries", &["ferries"], None),
            job("end", 1320, "stations", &["stations"], None),
            Event { ok: Some(false), ..job("end", 1320, "marks", &["marks"], None) },
            catalogue(1400, 20.0, 8, true),
            job("end", 1500, "names-todo", &["names-todo"], None),
        ];
        // (The servers' adoptions: their cursors in the map's log, as the lead noted them.)
        let adopt = |t: u64, host: &str, n: u64| mark(ADOPT, t, host, n);
        let events: Vec<Event> = events
            .into_iter()
            .chain([
                adopt(1430, "m1", 8),
                // (The M4 skipped catalogue 8: a later one counts.)
                adopt(1600, "m4", 9),
                // (Catalogue 7's, before 8: not its.)
                adopt(1010, "m4", 7),
                // (The M1's mirror's round copying 8, through 100 s later.)
                mark(MIRROR_START, 1440, "m1", 8),
                mark(MIRROR_END, 1540, "m1", 8),
            ])
            .collect();
        let (cats, units) = chains(&events);
        assert_eq!(cats.len(), 2);
        let c = &cats[1];
        assert_eq!((c.n, c.units, c.tiles), (Some(8), 2, 3));
        assert_eq!(c.stats.as_ref().unwrap().stated, 12);
        assert_eq!(c.ran, ["ferries", "stations", "names-todo"]);
        assert_eq!(c.adopted, [("m1".to_string(), 30.0), ("m4".into(), 200.0)].into());
        assert_eq!(c.mirrored["m1"], (40.0, Some(100.0)));
        assert_eq!(cats[0].ran, Vec::<String>::new());
        let a = units.iter().find(|u| u.unit == "6/1/1").unwrap();
        assert_eq!(
            (a.done, a.merged, a.planned, a.leased, a.packed, a.catalogue, a.restaled),
            (1100, Some(1130), Some(1150), Some(1200), Some(1300), Some(8), 2)
        );
        assert_eq!(a.adopted, [("m1".to_string(), 1430), ("m4".into(), 1600)].into());
        assert_eq!(a.mirrored, [("m1".to_string(), 1540)].into());
        let b = units.iter().find(|u| u.unit == "6/5/5").unwrap();
        assert_eq!((b.merged, b.leased, b.packed, b.restaled), (None, Some(1205), Some(1250), 1));
        let stages: BTreeMap<&str, Option<f64>> = unit_stages(a).into_iter().collect();
        assert_eq!(stages["hand-off"], Some(30.0));
        assert_eq!(stages["to planned"], Some(20.0));
        assert_eq!(stages["packed"], Some(100.0));
        assert_eq!(stages["to map update"], Some(100.0));
        assert_eq!(stages["servers"], Some(200.0));
        assert_eq!(stages["on every map"], Some(500.0));
        let t = text(&cats, &units, 10);
        assert!(t.contains("map update 8: "), "{t}");
        assert!(t.contains("12 of 1000 files stat'ed"), "{t}");
        assert!(t.contains("unit 6/1/1 (m1)") && t.contains("restaled 2"), "{t}");
        assert!(t.contains("map updates (2)"), "{t}");
        assert!(t.contains("ferries: ran for 1 of 2"), "{t}");
        assert!(t.contains("m4: adopts after"), "{t}");
        assert!(!text(&cats, &units, 1).contains("map update 7"));
    }

    /// A catalogue reads the manifest as it begins: a z6 tile merged, or its tiles packed, while one
    /// runs is in the next; the map tiles and steps are counted by the catalogues' beginnings.
    #[test]
    fn a_unit_done_while_a_catalogue_runs_is_in_the_next() {
        let events = vec![
            catalogue(1000, 10.0, 7, true),
            // Catalogue 8 runs from 1500 to 1600.
            job("done", 1100, "unit", &["6/1/1"], Some(1)),
            merged(1550, "2026-10-17/1-1", "unit", &["6/1/1"]),
            job("end", 1120, "unit", &["6/2/2"], None),
            planned(1130, &["6/2/2"], &[("6/2/2", &["6/2/2"])]),
            job("end", 1560, "pack", &["6/2/2"], None),
            // (A z6 tile with no map tile stale: in the first begun after its end.)
            job("end", 1200, "unit", &["6/3/3"], None),
            job("end", 1540, "ferries", &["ferries"], None),
            catalogue(1600, 100.0, 8, true),
            catalogue(1800, 50.0, 9, true),
        ];
        let (cats, units) = chains(&events);
        let cat_of = |u: &str| units.iter().find(|x| x.unit == u).unwrap().catalogue;
        assert_eq!(cat_of("6/1/1"), Some(9));
        assert_eq!(cat_of("6/2/2"), Some(9));
        assert_eq!(cat_of("6/3/3"), Some(8));
        assert_eq!((cats[1].units, cats[2].units), (1, 2));
        // (Planned at 1130: before 8 began. Ferries at 1540: after it began, so 9's.)
        assert_eq!((cats[1].tiles, cats[2].tiles), (1, 0));
        assert_eq!(cats[1].ran, Vec::<String>::new());
        assert_eq!(cats[2].ran, ["ferries"]);
        // (A tile never packed: no catalogue yet.)
        let (_, units) = chains(&[
            job("end", 10, "unit", &["6/4/4"], None),
            planned(20, &["6/4/4"], &[("6/4/4", &["6/4/4"])]),
            catalogue(100, 5.0, 3, true),
        ]);
        assert_eq!(units[0].catalogue, None);
    }

    /// Lease numbers may repeat across leads: a hand-off's merge is the one of its z6 tile, after
    /// its job ended, at its lease's number (events from before job ids, `same_run`); one naming
    /// its run's whole id, at that id.
    #[test]
    fn a_hand_off_is_matched_by_its_unit_and_lease_across_terms() {
        let events = vec![
            job("done", 100, "unit", &["6/1/1"], Some(5)),
            merged(110, "2026-10-17/2-5", "unit", &["6/1/1"]),
            // (A new lead's lease 5: another z6 tile.)
            job("done", 200, "unit", &["6/2/2"], Some(5)),
            merged(205, "2026-10-17/3-5", "unit", &["6/9/9"]),
            merged(230, "2026-10-17/3-5", "unit", &["6/2/2"]),
            // (The same z6 tile again, lease 7: its own merge, not the first's.)
            job("done", 300, "unit", &["6/1/1"], Some(7)),
            merged(320, "2026-10-17/3-6", "unit", &["6/1/1"]),
            merged(340, "2026-10-17/3-7", "unit", &["6/1/1"]),
            catalogue(500, 10.0, 4, true),
        ];
        let (_, units) = chains(&events);
        let m: Vec<(String, u64, Option<u64>)> = units.iter().map(|u| (u.unit.clone(), u.done, u.merged)).collect();
        assert_eq!(
            m,
            [("6/1/1".to_string(), 100, Some(110)), ("6/2/2".into(), 200, Some(230)), ("6/1/1".into(), 300, Some(340))]
        );
        // (A "done" naming its whole job id: another epoch's lease 5 isn't its merge.)
        let run = |epoch| Some(jobrt::jobid::JobId { epoch, n: 5 });
        let events = vec![
            Event { job: run(3), ..job("done", 100, "unit", &["6/1/1"], None) },
            merged(110, "2026-10-17/2-5", "unit", &["6/1/1"]),
            merged(120, "2026-10-17/3-5", "unit", &["6/1/1"]),
        ];
        let (_, units) = chains(&events);
        assert_eq!(units.iter().map(|u| u.merged).collect::<Vec<_>>(), [Some(120)]);
    }

    /// The tiles noted planned are known again from the history (a lead restarted or new), by tile
    /// and key: one stale again at a new key is noted again, once.
    #[test]
    fn planned_tiles_are_noted_once_per_key_across_restarts() {
        let tiles = |ks: &[(&str, &str)]| -> Vec<(String, String)> {
            ks.iter().map(|(t, k)| (t.to_string(), k.to_string())).collect()
        };
        let first = tiles(&[("6/1/1", "aaaaaaaaaaaaaaaa1"), ("6/1/2", "bbbbbbbbbbbbbbbb")]);
        let e = Event { t: 500, ..planned_event(&first, BTreeMap::new()) };
        // (Before the notes began: not theirs.)
        let old = Event { t: 100, ..planned_event(&tiles(&[("6/9/9", "cccccccccccccccc")]), BTreeMap::new()) };
        let mut noted = planned_since([&e, &old], 400);
        assert_eq!(noted.len(), 2);
        let now = tiles(&[
            ("6/1/1", "aaaaaaaaaaaaaaaa1"),
            ("6/1/2", "dddddddddddddddd"),
            ("6/9/9", "cccccccccccccccc"),
            ("6/9/9", "cccccccccccccccc"),
        ]);
        let new = not_noted(&now, &noted);
        assert_eq!(new, tiles(&[("6/1/2", "dddddddddddddddd"), ("6/9/9", "cccccccccccccccc")]));
        note_planned(&mut noted, &new);
        assert!(not_noted(&now, &noted).is_empty());
        // (The event stays small: keys cut short.)
        assert_eq!(e.data.as_ref().unwrap()["k"][0], "aaaaaaaaaaaa");
    }

    /// The agent's notes: a restart's from its coordinator's history (no NAS read), a new lead's
    /// from the NAS's (none for notes just begun), and the served catalogue read once a catalogue.
    #[test]
    fn the_planned_notes_across_a_restart_a_new_lead_and_catalogues() {
        let pair = |t: &str, k: &str| (t.to_string(), k.to_string());
        let (t0, now) = (1_791_500_000, 1_791_500_600);
        let noted = Event { t: t0 + 10, ..planned_event(&[pair("6/1/1", "aaaaaaaaaaaaaaaa")], BTreeMap::new()) };
        // Restarted: its own history reaches back to the beginning.
        let own = vec![ev("agent", t0 - 5), noted.clone()];
        let mut n = PlannedNotes::default();
        assert!(!n.begin(t0, &own, now));
        assert!(!n.begin(t0, &own, now));
        assert!(n.new_tiles(&[pair("6/1/1", "aaaaaaaaaaaaaaaa")]).is_empty());
        let new = n.new_tiles(&[pair("6/1/2", "bbbbbbbbbbbbbbbb")]);
        assert_eq!(new.len(), 1);
        n.note(&new);
        assert!(n.new_tiles(&[pair("6/1/2", "bbbbbbbbbbbbbbbb")]).is_empty());
        // A new lead: its history begins after; the NAS's archive says what was noted.
        let d = tempfile::tempdir().unwrap();
        let t = now_s() - 300;
        let day = crate::pool::journal::day(t + 10).unwrap();
        let arch = d.path().join(crate::agent::histarchive::ARCHIVE);
        std::fs::create_dir_all(&arch).unwrap();
        let line = serde_json::to_string(&Event { t: t + 10, ..noted.clone() }).unwrap() + "\n";
        let tar = codecs::tar::build(&[(format!("{day}/m-a.jsonl"), line.into_bytes())]).unwrap();
        std::fs::write(arch.join(format!("{day}.tar")), tar).unwrap();
        let mut m = PlannedNotes::default();
        assert!(m.begin(t, &[ev("agent", t + 100)], t + 300));
        m.seed_from_nas(store::nasfs::NasRoot::local_for_test(d.path().to_path_buf()));
        for _ in 0..200 {
            if m.seed.lock().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(m.new_tiles(&[pair("6/1/1", "aaaaaaaaaaaaaaaa")]).is_empty());
        // Just begun: no NAS read.
        assert!(!PlannedNotes::default().begin(t, &[], t + 30));
        // The served catalogue: read once, and again for a newer one.
        let cat = |n: u64, base: &str| {
            let mut c = store::catalogue::Catalogue { fmt: 1, n, ..Default::default() };
            c.files.insert(
                "base/6-1-1".into(),
                serde_json::from_value(serde_json::json!({"file": base, "size": 1})).unwrap(),
            );
            c.files.insert(
                "layers/roads/hi/6-1-1".into(),
                serde_json::from_value(serde_json::json!({"file": "x", "size": 1})).unwrap(),
            );
            c
        };
        assert!(m.served(None).is_empty());
        assert_eq!(m.served(Some(&cat(1, "b1"))), [("base/6-1-1".to_string(), "b1".to_string())].into());
        m.served(Some(&cat(1, "b1")));
        assert_eq!(m.served_reads, 1);
        assert_eq!(m.served(Some(&cat(2, "b2")))["base/6-1-1"], "b2");
        assert_eq!(m.served_reads, 2);
    }

    #[test]
    fn the_medians_and_a_catalogues_line() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
        let st = CatalogueStats { n: 5, files: 10, stated: 2, phases: vec![("x".into(), 1.5)] };
        let log = format!("catalogue: something\n{}\npublished /x/5.json\n", st.line());
        assert_eq!(CatalogueStats::from_log(&log), Some(st));
        assert_eq!(CatalogueStats::from_log("nothing"), None);
    }

    /// Task #1119: the history is the NAS's archive (each finished day's tar) and the lead's own
    /// file (its coordinator's, the week's), each event once.
    #[test]
    fn the_history_is_the_archive_and_the_leads_own_each_once() {
        let d = tempfile::tempdir().unwrap();
        let t = now_s() - 86400;
        let day = crate::pool::journal::day(t).unwrap();
        let line = |e: &Event| serde_json::to_string(e).unwrap() + "\n";
        let a = Event { seq: 1, ..ev("catalog", t) };
        let b = Event { seq: 2, ..ev("merged", t + 1) };
        let c = Event { seq: 3, ..ev("pause", now_s()) };
        let arch = d.path().join(crate::agent::histarchive::ARCHIVE);
        std::fs::create_dir_all(&arch).unwrap();
        let members = vec![(format!("{day}/m-a.jsonl"), line(&a).into_bytes())];
        std::fs::write(arch.join(format!("{day}.tar")), codecs::tar::build(&members).unwrap()).unwrap();
        let local = d.path().join("history.jsonl");
        std::fs::write(&local, line(&a) + &line(&b) + &line(&c)).unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path());
        let h = load_history(Some(&root), Some(&local), 3);
        assert_eq!(h.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), ["catalog", "merged", "pause"]);
        let only = d.path().join("h.jsonl");
        std::fs::write(&only, line(&b)).unwrap();
        assert_eq!(load_history(None, Some(&only), 2).len(), 1, "the lead's own alone");
    }
}
