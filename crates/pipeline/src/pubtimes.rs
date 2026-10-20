//! Publishing timed end to end (task #250; docs/building.md §2, Publishing's times): from a build
//! unit done to its map tiles on every Mac's map, by the events each stage leaves.
//!
//! - **In the build's history** (crate::coord::history, the lead's): a unit's job ended ("end",
//!   "done"); its hand-off merged ("merged": `merged_event`, each entry's key, member, step and
//!   targets); the map tiles (pack) planned ("planned": `planned_event`, the z6 tiles the lead
//!   first plans at their keys, and the units whose change made each stale, `restaled_by`), leased
//!   ("lease", "start") and done ("end", "done"); and catalog n written ("catalog": its `n`, its
//!   job's time, and `CatalogStats`: delta or whole, the files it stat'ed, its phases).
//! - **The Macs' map servers' own log on the NAS** (`SERVERS`, a file per Mac appended to: a few
//!   lines a catalog, never a file per event; cut to its newer half past `SERVERS_MAX`): each
//!   server's adoption of catalog n ("adopt") and its mirror's sync of it, begun ("mirror-start")
//!   and through everything wanted ("mirror-end"), each once per catalog and server process.
//!
//! `chains` joins them, per catalog and per unit; `scenic timings publish` prints them.

use crate::coord::history::Event;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

/// The map servers' log: `<host>.jsonl` in it, one writer each.
pub const SERVERS: &str = "state/publish";
/// A Mac's log past this many bytes is cut to its newer half as it's next appended to (a few
/// hundred bytes a catalog: months of them).
pub const SERVERS_MAX: u64 = 512 * 1024;

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

/// Appends `e` (now, unless it says when) to its Mac's log under the project folder `root`; a log
/// past `SERVERS_MAX` is written again with its newer half first.
pub fn append_server(root: &Path, mut e: ServerEvent) -> std::io::Result<()> {
    if e.t == 0 {
        e.t = now_s();
    }
    let dir = root.join(SERVERS);
    std::fs::create_dir_all(&dir)?;
    let name: String =
        e.host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let line = serde_json::to_string(&e).map_err(std::io::Error::other)?;
    let p = dir.join(format!("{name}.jsonl"));
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > SERVERS_MAX) {
        let text = std::fs::read_to_string(&p)?;
        let lines: Vec<&str> = text.lines().collect();
        let kept = lines[lines.len() / 2..].join("\n");
        return crate::whole::write(&p, format!("{kept}\n{line}\n").as_bytes()).map_err(std::io::Error::other);
    }
    std::fs::OpenOptions::new().create(true).append(true).open(&p)?.write_all(format!("{line}\n").as_bytes())
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
/// once (a sync through everything wanted, nothing waiting or failed). A server started again says
/// the catalog's start again (`chains` takes the first).
#[derive(Debug, Default)]
pub struct MirrorMarks {
    started: Option<(u64, u64)>,
    ended: Option<u64>,
}

impl MirrorMarks {
    pub const fn new() -> MirrorMarks {
        MirrorMarks { started: None, ended: None }
    }

    /// A sync of catalog `n` begins at `t`: whether it's this process's first of it (to say).
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
    /// Catalog `n`'s, with the job's phases as timed so far (crate::timings; none untimed).
    pub fn now(n: u64, delta: bool, files: u64, stated: u64) -> CatalogStats {
        let phases = crate::timings::snapshot(true)
            .map(|r| r.phases.into_iter().map(|p| (p.name, p.wall_s)).collect())
            .unwrap_or_default();
        CatalogStats { n, delta, files, stated, phases }
    }

    pub fn line(&self) -> String {
        format!("{CATALOG_LINE}{}", serde_json::to_string(self).unwrap_or_default())
    }

    /// The last such line in a log's tail.
    pub fn from_log(tail: &str) -> Option<CatalogStats> {
        tail.lines()
            .rev()
            .find_map(|l| l.find(CATALOG_LINE).and_then(|i| serde_json::from_str(&l[i + CATALOG_LINE.len()..]).ok()))
    }
}

/// The units whose change made each of `tiles` (z6, "6/x/y") stale: of its owners (their ways meet
/// it: crate::agent::build::{unit_reach, owns}, as its key reads them), those whose base pack or
/// road values in the manifest `m` aren't what `served` (the served catalog's, by logical name)
/// has: by unit ("6/x/y"), its tiles. A unit rebuilt the same (the same content names) made none
/// stale.
pub fn restaled_by(
    m: &BTreeMap<String, String>,
    reach: Option<&crate::reach::Reaches>,
    served: &BTreeMap<String, String>,
    tiles: &[String],
) -> BTreeMap<String, Vec<String>> {
    use crate::agent::build::{owns, unit_reach};
    use crate::legacy::Unit;
    let changed: Vec<(Unit, [i32; 4])> = m
        .keys()
        .filter_map(|l| l.strip_prefix("base/").and_then(Unit::parse))
        .filter(|u| {
            [format!("base/{}", u.dash()), format!("global/roads/{}", u.dash())]
                .iter()
                .any(|l| m.get(l) != served.get(l))
        })
        .map(|u| (u, unit_reach(reach, m, u)))
        .collect();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in tiles {
        let p: Vec<u32> = t.split('/').filter_map(|s| s.parse().ok()).collect();
        let [z, x, y] = p[..] else { continue };
        let b = crate::hipack::tile_bounds(z as u8, x, y);
        for (u, _) in changed.iter().filter(|(_, e)| owns(*e, b)) {
            out.entry(u.slash()).or_default().push(t.clone());
        }
    }
    out
}

/// The served catalog's base packs' and road values' content names, by logical name (what
/// `restaled_by` compares with): the latest in `dir`, with its number.
pub fn served_units(dir: &Path) -> anyhow::Result<Option<(u64, BTreeMap<String, String>)>> {
    Ok(store::catalog::latest(dir)?.map(|c| {
        (
            c.n,
            c.files
                .into_iter()
                .filter(|(l, _)| l.starts_with("base/") || l.starts_with("global/roads/"))
                .map(|(l, f)| (l, f.file))
                .collect(),
        )
    }))
}

/// How much of a planned tile's key a "planned" event keeps: enough to tell its keys apart.
const KEY_KEPT: usize = 12;

fn cut(k: &str) -> String {
    k.chars().take(KEY_KEPT).collect()
}

/// A "planned" event: map tiles first planned at their keys (`tiles`: (tile, key)), and the units
/// whose change made them stale (`restaled_by`). Small: the keys cut short, each unit its own tiles.
pub fn planned_event(tiles: &[(String, String)], by_unit: BTreeMap<String, Vec<String>>) -> Event {
    let keys: Vec<String> = tiles.iter().map(|(_, k)| cut(k)).collect();
    Event {
        step: Some("pack".into()),
        targets: tiles.iter().map(|t| t.0.clone()).collect(),
        data: Some(serde_json::json!({ "k": keys, "units": by_unit })),
        ..Event::new("planned")
    }
}

/// The (tile, key) pairs `events` say were planned since `since` (keys as `planned_event` keeps
/// them): what a lead, restarted or new, noted already since the last catalog.
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

/// What the lead noted planned since the last catalog (`planned_event`), by tile and key, and
/// the served catalog's units as last read (`served_units`, once a catalog). What it noted before
/// a restart comes back from its coordinator's history (`begin`); before a change of lead, from
/// the NAS's, read on a thread of its own (`seed_from_nas`) so a slow share never holds the plan.
#[derive(Debug, Default)]
pub struct PlannedNotes {
    began: u64,
    noted: BTreeSet<(String, String)>,
    served: Option<(u64, BTreeMap<String, String>)>,
    /// The served catalogs read (for the tests).
    pub served_reads: u32,
    seed: std::sync::Arc<std::sync::Mutex<Option<(u64, BTreeSet<(String, String)>)>>>,
}

impl PlannedNotes {
    /// When its notes begin (the last catalog's time).
    pub fn began(&self) -> u64 {
        self.began
    }

    /// Its notes begin at `b` (the last catalog's time); `own`: this coordinator's history. On a new
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

    /// The NAS's history since its notes began (`root`), read on a thread; taken up by the
    /// next `new_tiles`.
    pub fn seed_from_nas(&self, root: std::path::PathBuf) {
        let (b, seed) = (self.began, self.seed.clone());
        let spawned = std::thread::Builder::new().name("planned-seed".into()).spawn(move || {
            let s = Self::nas_noted(&root, b);
            *seed.lock().unwrap_or_else(|e| e.into_inner()) = Some((b, s));
        });
        if let Err(e) = spawned {
            eprintln!("agent: reading what was planned from the NAS: {e}");
        }
    }

    /// What the NAS's history (`root`) says was noted planned since `b`.
    pub fn nas_noted(root: &Path, b: u64) -> BTreeSet<(String, String)> {
        let days = now_s().saturating_sub(b) / 86400 + 1;
        planned_since(&load_history(Some(root), None, days), b)
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

    /// The served catalog's units (in `dir`), read again only when a newer catalog is there.
    pub fn served(&mut self, dir: &Path) -> BTreeMap<String, String> {
        let latest = store::catalog::list(dir).ok().and_then(|ns| ns.into_iter().max());
        if latest.is_some() && self.served.as_ref().map(|s| s.0) != latest {
            self.served_reads += 1;
            match served_units(dir) {
                Ok(s) => self.served = s,
                Err(e) => eprintln!("agent: the served catalog's units, for publishing's times: {e:#}"),
            }
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

/// A unit's way to the map: when each stage was reached (unix seconds).
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
    /// The catalog it's in: the first begun once it was merged and packed.
    pub catalog: Option<u64>,
    pub catalog_t: Option<u64>,
    /// Each Mac's server adopting it (that catalog or a later one), and its mirror through it.
    pub adopted: BTreeMap<String, u64>,
    pub mirrored: BTreeMap<String, u64>,
    /// The map tiles its change made stale (`restaled_by`), as planned after its hand-off.
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
    /// The units in it, and the map tiles planned between the catalog before's beginning and its own.
    pub units: usize,
    pub tiles: usize,
    /// Of `COUNTED`, the steps that ran for it (between the same beginnings; names-todo after it).
    pub ran: Vec<String>,
}

fn ok_job(e: &Event, step: &str) -> bool {
    matches!(e.kind.as_str(), "end" | "done") && e.ok == Some(true) && e.step.as_deref() == Some(step)
}

/// When a catalog's job began (it read the manifest then): its end less its time.
fn began(c: &Event) -> u64 {
    c.t.saturating_sub(c.secs.unwrap_or(0.0).ceil() as u64)
}

/// The chains of the catalogs in `events` (any order) and their units, joined with the servers'
/// events; oldest first. A unit is in the first catalog begun once its hand-off was merged (a
/// member's job; the lead's own, at its end) and the map tiles its change made stale were packed;
/// a catalog's map tiles and steps are those between the catalog before's beginning and its own.
pub fn chains(events: &[Event], servers: &[ServerEvent]) -> (Vec<CatalogChain>, Vec<UnitChain>) {
    let mut ev: Vec<&Event> = events.iter().collect();
    ev.sort_by_key(|e| (e.t, e.seq));
    let cats: Vec<&Event> = ev.iter().copied().filter(|e| e.kind == "catalog").collect();
    // The merges' jobs: (time, lease number, step, targets).
    let mut merges: Vec<(u64, Option<u64>, String, Vec<String>)> = Vec::new();
    for e in ev.iter().filter(|e| e.kind == "merged") {
        let done: Vec<crate::pool::records::MergedJob> =
            e.data.as_ref().and_then(|d| serde_json::from_value(d["done"].clone()).ok()).unwrap_or_default();
        for (key, _, step, targets) in done {
            // (`<day>/<term>-<n>`: the coordinator's lease is its `n`, which repeats across terms.)
            let n = key.rsplit(['/', '-']).next().and_then(|l| l.parse().ok());
            merges.push((e.t, n, step, targets));
        }
    }
    let mut by_host: BTreeMap<&str, Vec<&ServerEvent>> = BTreeMap::new();
    for s in servers {
        by_host.entry(s.host.as_str()).or_default().push(s);
    }
    let first_at = |host: &[&ServerEvent], kind: &str, n: u64, after: u64| {
        host.iter().filter(|s| s.kind == kind && s.n >= n && s.t >= after).map(|s| s.t).min()
    };
    let stats_of =
        |c: &Event| -> Option<CatalogStats> { c.data.as_ref().and_then(|d| serde_json::from_value(d.clone()).ok()) };
    let n_of = |c: &Event| c.n.or(stats_of(c).map(|s| s.n));
    // Each planned tile: when, which, and the units whose change made it stale.
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
                        && (e.lease.is_none() || *n == e.lease)
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
            // (Its catalog: the first begun once it was merged and packed; a tile not packed yet, none.)
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
                catalog: cat.and_then(|c| n_of(c)),
                catalog_t: cat.map(|c| c.t),
                restaled: mine.len(),
                ..Default::default()
            };
            if let Some(n) = uc.catalog {
                let after = uc.catalog_t.unwrap_or(0).saturating_sub(120);
                for (host, hs) in &by_host {
                    if let Some(a) = first_at(hs, "adopt", n, after) {
                        uc.adopted.insert(host.to_string(), a);
                    }
                    if let Some(m) = first_at(hs, "mirror-end", n, after) {
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
        let mut cc = CatalogChain {
            n,
            t: c.t,
            secs: c.secs,
            stats: stats_of(c),
            tiles: tiles.len(),
            units: units.iter().filter(|u| u.catalog_t == Some(c.t)).count(),
            ..Default::default()
        };
        if let Some(n) = n {
            for (host, hs) in &by_host {
                if let Some(a) = first_at(hs, "adopt", n, c.t.saturating_sub(120)) {
                    cc.adopted.insert(host.to_string(), a.saturating_sub(c.t) as f64);
                }
                if let Some(s) = first_at(hs, "mirror-start", n, c.t.saturating_sub(120)) {
                    let len = hs
                        .iter()
                        .find(|e| e.kind == "mirror-end" && e.n >= n && e.t >= s)
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
    x.map_or_else(|| "-".into(), crate::timings::secs)
}

/// The stages a unit goes through, each's time.
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
        for u in units.iter().filter(|u| u.catalog_t == Some(c.t)) {
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
    o += &format!("\nMedians over {} unit(s) and {} catalog(s):\n", units.len(), cats.len());
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
    for delta in [true, false] {
        let of: Vec<&CatalogStats> =
            cats.iter().filter_map(|c| c.stats.as_ref()).filter(|s| s.delta == delta).collect();
        if of.is_empty() {
            continue;
        }
        let secs: Vec<f64> =
            cats.iter().filter(|c| c.stats.as_ref().is_some_and(|s| s.delta == delta)).filter_map(|c| c.secs).collect();
        o += &format!(
            "  {} catalogs ({}): {} in the job, {} files stat'ed\n",
            if delta { "delta" } else { "whole" },
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
        o += &format!("  {st}: ran for {k} of {} catalog(s)\n", cats.len());
    }
    o
}

/// The build's history: the NAS's (`state/coord/history/<day>/*.jsonl`, the last `days` days',
/// every writer's), else the lead's own (`local`, its coordinator's file); duplicates once.
pub fn load_history(root: Option<&Path>, local: Option<&Path>, days: u64) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::new();
    let since = now_s().saturating_sub(days * 86400);
    let read = |p: &Path| -> Vec<Event> {
        std::fs::read_to_string(p)
            .map(|t| t.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| e.t >= since).collect())
            .unwrap_or_default()
    };
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
        Event {
            step: Some(step.into()),
            targets: targets.iter().map(|s| s.to_string()).collect(),
            ok: Some(true),
            lease,
            worker: Some("m1".into()),
            secs: Some(10.0),
            ..ev(kind, t)
        }
    }

    /// Catalog `n` written at `t`, its job `secs` long.
    fn catalog(t: u64, secs: f64, n: u64, delta: bool) -> Event {
        let st = CatalogStats {
            n,
            delta,
            files: 1000,
            stated: if delta { 12 } else { 1000 },
            phases: vec![
                ("every file checked on the NAS".into(), if delta { 1.0 } else { 9.0 }),
                ("the catalog written".into(), 2.0),
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

    /// A unit from its job to every Mac's map: each stage's time, its restale count, the catalog's
    /// kind, its files stat'ed, the counted steps, and the servers' adoptions and mirrors.
    #[test]
    fn a_units_chain_from_its_job_to_every_macs_map() {
        let events = vec![
            catalog(1000, 60.0, 7, false),
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
            catalog(1400, 20.0, 8, true),
            job("end", 1500, "names-todo", &["names-todo"], None),
        ];
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
        assert_eq!(
            (a.done, a.merged, a.planned, a.leased, a.packed, a.catalog, a.restaled),
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
        assert!(!text(&cats, &units, 1).contains("catalog 7"));
    }

    /// A catalog reads the manifest as it begins: a unit merged, or its tiles packed, while one
    /// runs is in the next; the map tiles and steps are counted by the catalogs' beginnings.
    #[test]
    fn a_unit_done_while_a_catalog_runs_is_in_the_next() {
        let events = vec![
            catalog(1000, 10.0, 7, true),
            // Catalog 8 runs from 1500 to 1600.
            job("done", 1100, "unit", &["6/1/1"], Some(1)),
            merged(1550, "2026-10-17/1-1", "unit", &["6/1/1"]),
            job("end", 1120, "unit", &["6/2/2"], None),
            planned(1130, &["6/2/2"], &[("6/2/2", &["6/2/2"])]),
            job("end", 1560, "pack", &["6/2/2"], None),
            // (A unit with no map tile stale: in the first begun after its end.)
            job("end", 1200, "unit", &["6/3/3"], None),
            job("end", 1540, "ferries", &["ferries"], None),
            catalog(1600, 100.0, 8, true),
            catalog(1800, 50.0, 9, true),
        ];
        let (cats, units) = chains(&events, &[]);
        let cat_of = |u: &str| units.iter().find(|x| x.unit == u).unwrap().catalog;
        assert_eq!(cat_of("6/1/1"), Some(9));
        assert_eq!(cat_of("6/2/2"), Some(9));
        assert_eq!(cat_of("6/3/3"), Some(8));
        assert_eq!((cats[1].units, cats[2].units), (1, 2));
        // (Planned at 1130: before 8 began. Ferries at 1540: after it began, so 9's.)
        assert_eq!((cats[1].tiles, cats[2].tiles), (1, 0));
        assert_eq!(cats[1].ran, Vec::<String>::new());
        assert_eq!(cats[2].ran, ["ferries"]);
        // (A tile never packed: no catalog yet.)
        let (_, units) = chains(
            &[
                job("end", 10, "unit", &["6/4/4"], None),
                planned(20, &["6/4/4"], &[("6/4/4", &["6/4/4"])]),
                catalog(100, 5.0, 3, true),
            ],
            &[],
        );
        assert_eq!(units[0].catalog, None);
    }

    /// Lease numbers repeat across leads (terms): a hand-off's merge is the one of its unit, after
    /// its job ended, at its lease's number.
    #[test]
    fn a_hand_off_is_matched_by_its_unit_and_lease_across_terms() {
        let events = vec![
            job("done", 100, "unit", &["6/1/1"], Some(5)),
            merged(110, "2026-10-17/2-5", "unit", &["6/1/1"]),
            // (A new lead's lease 5: another unit.)
            job("done", 200, "unit", &["6/2/2"], Some(5)),
            merged(205, "2026-10-17/3-5", "unit", &["6/9/9"]),
            merged(230, "2026-10-17/3-5", "unit", &["6/2/2"]),
            // (The same unit again, lease 7: its own merge, not the first's.)
            job("done", 300, "unit", &["6/1/1"], Some(7)),
            merged(320, "2026-10-17/3-6", "unit", &["6/1/1"]),
            merged(340, "2026-10-17/3-7", "unit", &["6/1/1"]),
            catalog(500, 10.0, 4, true),
        ];
        let (_, units) = chains(&events, &[]);
        let m: Vec<(String, u64, Option<u64>)> = units.iter().map(|u| (u.unit.clone(), u.done, u.merged)).collect();
        assert_eq!(
            m,
            [("6/1/1".to_string(), 100, Some(110)), ("6/2/2".into(), 200, Some(230)), ("6/1/1".into(), 300, Some(340))]
        );
    }

    /// The units whose change made a tile stale: its owners whose base pack or road values aren't
    /// the served catalog's; one rebuilt the same made none.
    #[test]
    fn a_tile_is_restaled_by_its_owners_that_changed() {
        let m: BTreeMap<String, String> = [
            ("base/6-28-16", "b1"),
            ("global/roads/6-28-16", "r1"),
            ("base/6-40-20", "b2"),
            ("global/roads/6-40-20", "r2"),
            ("base/6-28-17", "b3"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        // (6-40-20 as served; 6-28-16's roads new; 6-28-17 not served yet.)
        let served: BTreeMap<String, String> = [
            ("base/6-28-16", "b1"),
            ("global/roads/6-28-16", "r0"),
            ("base/6-40-20", "b2"),
            ("global/roads/6-40-20", "r2"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let r = restaled_by(&m, None, &served, &["6/28/16".into(), "6/40/20".into(), "6/28/17".into(), "6/0/0".into()]);
        assert_eq!(r.get("6/28/16").unwrap(), &["6/28/16", "6/28/17"]);
        assert_eq!(r.get("6/28/17").unwrap(), &["6/28/16", "6/28/17"]);
        assert!(!r.contains_key("6/40/20"));
        // The same as served: none.
        let same: BTreeMap<String, String> = m.clone();
        assert!(restaled_by(&m, None, &same, &["6/28/16".into()]).is_empty());
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
    /// from the NAS's (none for notes just begun), and the served catalog read once a catalog.
    #[test]
    fn the_planned_notes_across_a_restart_a_new_lead_and_catalogs() {
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
        // A new lead: its history begins after; the NAS's says what was noted.
        let d = tempfile::tempdir().unwrap();
        let t = now_s() - 300;
        let dir = d.path().join("state/coord/history").join(crate::pool::journal::day(t).unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("m-a.jsonl"),
            serde_json::to_string(&Event { t: t + 10, ..noted.clone() }).unwrap() + "\n",
        )
        .unwrap();
        let mut m = PlannedNotes::default();
        assert!(m.begin(t, &[ev("agent", t + 100)], t + 300));
        m.seed_from_nas(d.path().to_path_buf());
        for _ in 0..200 {
            if m.seed.lock().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(m.new_tiles(&[pair("6/1/1", "aaaaaaaaaaaaaaaa")]).is_empty());
        // Just begun: no NAS read.
        assert!(!PlannedNotes::default().begin(t, &[], t + 30));
        // The served catalog: read once, and again for a newer one.
        let cats = d.path().join("catalog");
        std::fs::create_dir_all(&cats).unwrap();
        let cat = |n: u64, base: &str| {
            let mut c = store::catalog::Catalog { fmt: 1, n, ..Default::default() };
            c.files.insert(
                "base/6-1-1".into(),
                serde_json::from_value(serde_json::json!({"file": base, "size": 1})).unwrap(),
            );
            c.files.insert(
                "layers/roads/hi/6-1-1".into(),
                serde_json::from_value(serde_json::json!({"file": "x", "size": 1})).unwrap(),
            );
            store::catalog::write_copy(&cats, &c).unwrap();
        };
        assert!(m.served(&cats).is_empty());
        cat(1, "b1");
        assert_eq!(m.served(&cats), [("base/6-1-1".to_string(), "b1".to_string())].into());
        m.served(&cats);
        assert_eq!(m.served_reads, 1);
        cat(2, "b2");
        assert_eq!(m.served(&cats)["base/6-1-1"], "b2");
        assert_eq!(m.served_reads, 2);
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
    fn the_servers_log_is_appended_per_mac_read_in_order_and_cut() {
        let d = tempfile::tempdir().unwrap();
        append_server(
            d.path(),
            ServerEvent { t: 20, host: "Brandon's M4".into(), kind: "adopt".into(), n: 2, secs: None },
        )
        .unwrap();
        append_server(d.path(), ServerEvent { t: 10, host: "m1".into(), kind: "adopt".into(), n: 1, secs: None })
            .unwrap();
        append_server(
            d.path(),
            ServerEvent { t: 30, host: "m1".into(), kind: "mirror-end".into(), n: 2, secs: Some(5.0) },
        )
        .unwrap();
        let files: Vec<String> = std::fs::read_dir(d.path().join(SERVERS))
            .unwrap()
            .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 2);
        let all = read_servers(d.path(), 0);
        assert_eq!(all.iter().map(|e| e.t).collect::<Vec<_>>(), [10, 20, 30]);
        assert_eq!(read_servers(d.path(), 15).len(), 2);
        // Past SERVERS_MAX: its newer half kept, the new line after it.
        let p = d.path().join(SERVERS).join("m1.jsonl");
        let line =
            serde_json::to_string(&ServerEvent { t: 40, host: "m1".into(), kind: "adopt".into(), n: 3, secs: None })
                .unwrap();
        std::fs::write(&p, format!("{line}\n").repeat((SERVERS_MAX as usize / line.len()) + 10)).unwrap();
        let before = std::fs::read_to_string(&p).unwrap().lines().count();
        append_server(d.path(), ServerEvent { t: 50, host: "m1".into(), kind: "adopt".into(), n: 4, secs: None })
            .unwrap();
        let after: Vec<String> = std::fs::read_to_string(&p).unwrap().lines().map(str::to_string).collect();
        assert_eq!(after.len(), before - before / 2 + 1);
        assert!(after.last().unwrap().contains("\"n\":4"));
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
