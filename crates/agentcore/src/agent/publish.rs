//! Publishing (docs/building.md §2, Updating the map): its switch, and the map's log the lead writes.
//!
//! - **Its switches**, the pool's settings the lead keeps (`Switches`, jobrt::settings: set with
//!   `scenic pool set` and the build page's Settings): publishing held for review (`hold-catalog`:
//!   the `catalog` step keeps the map it would publish on the lead's disk, `catalog-held`), and
//!   slope and tree cover near (`near-slope`: a done region's slope and tree cover the z6 tiles near
//!   it, Publishing::near, their assemblies with the low zooms). Off while not set.
//! - **The map's log** (docs/formats.md, The map and its log): the `catalog` step builds the map from the records and
//!   posts it whole to its agent (`POSTED`, a file in the agent's `maplog/`, `--post`); the agent's
//!   writer (`MapLog`, on a thread of its own) diffs it against the map and appends the batch. The
//!   servers follow the log through the coordinator's `/map/` routes (its `feed`), the agent reads
//!   the map from it (`Here`), and a job or a command reads it from the lead (coordwire::map_now).

use crate::pool::records::Keys;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Publishing's switches, as the pool's settings say (crate::coord::asks; off while not set).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Switches {
    /// Publishing held for review (`hold-catalog`).
    pub held: bool,
    /// Slope and tree cover near (`near-slope`).
    pub near: bool,
}

impl Switches {
    pub fn of(s: &crate::coord::asks::Settings) -> Switches {
        Switches { held: s.on(crate::coord::asks::HOLD_CATALOG), near: s.on(crate::coord::asks::NEAR_SLOPE) }
    }
}

/// Whether hand-offs of the map tiles or the road index waiting to be merged (`unmerged`) hold the
/// catalogue back, a catalogue held back `held_for` seconds so far: until CATALOGUE_CAP_S, then it
/// goes (the regions they touch not recorded built: build::plan's `ready`).
pub fn unmerged_holds(unmerged: bool, held_for: Option<u64>) -> bool {
    unmerged && held_for.is_none_or(|s| s < CATALOGUE_CAP_S)
}

/// When a catalogue due was first held back (LastCatalogue::held_at), now `now`, it last `was`:
/// held back now (`held`), kept (or now if none); else kept while a catalogue is planned or due and
/// waiting out its minute (`due`: seam::Planned::catalogue_due), so the cap reached isn't lost; else
/// none, the count starting again.
pub fn held_at(was: Option<u64>, held: bool, due: bool, now: u64) -> Option<u64> {
    if held {
        was.or(Some(now))
    } else if due {
        was
    } else {
        None
    }
}

/// A catalogue at most this often (seconds; the owner's, 17 Oct): what's drawn meanwhile goes out
/// with the next.
pub const CATALOGUE_EVERY_S: u64 = 60;

/// How long (seconds) a catalogue may be held back by the map tiles or the road index still to draw,
/// or their hand-offs waiting to be merged (z6 tiles landing one after another), counted from when it
/// was first held (`seam::Publish::held_for`): past it, one goes out with what's drawn and merged.
pub const CATALOGUE_CAP_S: u64 = 600;

/// The low zooms' cadence (`build::low_gate`): how long after they were last made they're made again
/// while other work is queued (the owner's "every few hours"; a policy, not a measure). Sooner
/// when much has changed (`build::low_due`), when nothing else is queued, and once everything is built.
pub const LOW_EVERY_S: u64 = 3 * 3600;

/// How publishing goes, as its switches say (`Switches`) and its cadences.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Publishing {
    /// Seconds since a catalogue last went out with the low zooms made: the map log's last batch
    /// saying so (crate::agent::publish::LOW; None: none in the log).
    pub since_low: Option<u64>,
    /// A done region's slope and tree cover by the z6 tiles near it (the pool's `near-slope`): the
    /// slope z6 tiles whose z6 tile is within 20 km of it (what its slope reads), the tree cover
    /// z6 tiles meeting it; not every stale z6 tile of the z3 tiles around it. Their assemblies (z3–8)
    /// are made with the low zooms, and a region is done with its
    /// z6 tiles.
    pub near: bool,
    /// Seconds since the landmark points and the area overlays last ran (`LastCatalogue::since`;
    /// None: not known): their cadence (`MARKS_EVERY_S`, `OVERLAYS_EVERY_S`).
    pub since_marks: Option<u64>,
    pub since_overlays: Option<u64>,
}

/// The lead's record of its catalogues (its folder's `publishing.json`). When the last went out, and
/// the last with the low zooms made, are the map log's (docs/building.md §2, 5: its head batch, and its
/// last batch saying crate::agent::publish::LOW).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LastCatalogue {
    /// When a catalogue due was first held back since the last (by the map tiles or the road index
    /// to draw, `seam::Planned::catalogue_held`, or their hand-offs waiting to be merged), kept across a
    /// restart or a change of lead: `seam::Publish::held_for` counts from it, toward `CATALOGUE_CAP_S`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_at: Option<u64>,
    /// The cadence-gated steps' (`GATED`) last runs as the lead saw them: the key recorded, and
    /// when it was first seen changed (`LastCatalogue::observe`). Only for when they're planned,
    /// never in a key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ran: BTreeMap<String, Ran>,
    /// The last plan had no z6 tile or terrain left to build (`seam::Planned::idle`; not kept).
    #[serde(skip)]
    pub idle: bool,
}

/// A cadence-gated step's last run (`LastCatalogue::ran`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ran {
    /// Its key recorded ("" for none).
    pub key: String,
    /// When that key was first seen; None when it was already there as this record began (when it
    /// ran isn't known: it's due).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

/// The landmark points at most this often (seconds; the owner's, a policy) while z6 tiles build; at
/// once when none's left.
pub const MARKS_EVERY_S: u64 = 3600;
/// The area overlays at most this often while z6 tiles build; and after the last.
pub const OVERLAYS_EVERY_S: u64 = 6 * 3600;
/// The names' to-do lists at most this often while z6 tiles build; after any catalogue when none's left.
pub const NAMES_TODO_EVERY_S: u64 = 24 * 3600;
/// The steps gated by time: the gate decides only WHEN they're planned; their keys stay by content.
pub const GATED: [&str; 3] = ["marks", "overlays", "names-todo"];

impl LastCatalogue {
    /// Notes, at `now`, each gated step whose recorded key in `done` changed since last seen: a run
    /// of it ended (the lead's own, or a member's merged). Whether the record changed.
    pub fn observe(&mut self, done: &Keys, now: u64) -> bool {
        let mut changed = false;
        for step in GATED {
            let key = done.lo.get(step).cloned().unwrap_or_default();
            match self.ran.get_mut(step) {
                Some(r) if r.key == key => {}
                // (Its record dropped: not a run; its time kept.)
                Some(r) if key.is_empty() => {
                    r.key = key;
                    changed = true;
                }
                Some(r) => {
                    *r = Ran { key, at: Some(now) };
                    changed = true;
                }
                None => {
                    self.ran.insert(step.to_string(), Ran { key, at: None });
                    changed = true;
                }
            }
        }
        changed
    }

    /// Seconds since gated step `step` last ran, as seen (None: not known).
    pub fn since(&self, step: &str, now: u64) -> Option<u64> {
        self.ran.get(step).and_then(|r| r.at).map(|t| now.saturating_sub(t))
    }
}

/// Publishing as the plan has it now, for the agent's log and the build status (`build::publishing`;
/// docs/building.md §2, Updating the map).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishLine {
    /// What publishing does now, or waits for, in plain words.
    pub why: String,
    /// The map tiles to draw (pack(T) targets whose owners changed), and the z6 tiles whose change made
    /// them stale (`build::by_unit`).
    #[serde(default)]
    pub tiles: usize,
    /// The z6 tiles drawn (`units`: the status's stored key, read by the menu bar app and the page: kept).
    #[serde(default)]
    pub units: usize,
    /// The regions done whose catalogue hasn't them as they are now, each with what it still waits
    /// for.
    #[serde(default)]
    pub regions: Vec<PublishRegion>,
    /// The low zooms are made now (`build::low_gate`).
    #[serde(default)]
    pub low: bool,
    /// Seconds until the next catalogue may be made (`CATALOGUE_EVERY_S`; 0: it may now).
    #[serde(default)]
    pub wait_s: u64,
    /// Since when a catalogue has been due without a break (unix s; held back, planned, or waiting
    /// out its minute; None: none due): the stale flag's (crate::agent::flags), counted from the
    /// last catalogue's going out where that's later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_since: Option<u64>,
}

/// A region done whose catalogue hasn't it as it is now (`PublishLine::regions`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishRegion {
    pub id: String,
    /// "new region" or "rebuilt" (its outline or z6 tiles changed), and while it isn't recorded built,
    /// why: "waiting to be merged" (a z6 tile's or a map tile's hand-off), "map tiles to draw", or
    /// "slope and tree cover to make".
    pub kinds: Vec<String>,
}

impl PublishLine {
    /// One line for the agent's log.
    pub fn line(&self) -> String {
        let regions = if self.regions.is_empty() {
            String::new()
        } else {
            format!(
                "; done, not on the map as they are: {}",
                self.regions
                    .iter()
                    .map(|r| format!("{} ({})", r.id, r.kinds.join(", ")))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!(
            "map update: {}{regions}{}",
            self.why,
            if self.low { "" } else { "; the low zooms wait for their turn" }
        )
    }

    /// What the log line is logged again on: what publishing does and why, not the seconds counting
    /// down nor the tiles' count as they're drawn, nor an error's text in what it waits for.
    pub fn key(&self) -> String {
        let why = self.why.split(" (").next().unwrap_or("");
        format!("{why} {:?} {}", self.regions, self.low)
    }
}

/// The map's log, written by the lead's agent (docs/formats.md, The map and its log; docs/building.md §2, 5): each map the `catalog` step
/// posts (`POSTED`) becomes a batch of the log in the agent's `maplog/` (store::map::log), `seq` the
/// step's number (the head's next: one taken already, the next); the map then is checked equal to
/// it, and a mismatch is logged loudly. The servers follow the log through the coordinator's `/map/`
/// routes, answered from the `feed` it keeps; the agent reads the map from it (`Here`).
///
/// The work (the posted map read, diffed against the map, checked, the hourly snapshots) runs on a
/// thread of its own, never in the agent's loop, and can't hold the loop up: `published` wakes it
/// over a channel of one place and returns. The step's next post replaces one not taken yet (the
/// next batch spans both). A post the thread took and didn't append (the agent stopped) is
/// appended when the log opens again. A panic there is logged and the thread goes on, its log
/// opened again at the next post.
#[derive(Default)]
pub struct MapLog {
    tx: Option<std::sync::mpsc::SyncSender<bool>>,
    here: Option<std::sync::Arc<Here>>,
}

/// The log's folder in the agent's home.
pub const MAPLOG: &str = "maplog";
/// The map the `catalog` step posts, in the log's folder (`--post`), and the one the writer took.
pub const POSTED: &str = "posted.json.zst";
const TAKEN: &str = "posted.taken.json.zst";
/// The maps the `catalog` step would have published while publishing is held for review (the
/// pool's `hold-catalog`: the `catalog-held` job's `--held`), in the log's folder: `held/<seq>.json.zst`
/// each whole, `seq` the batch's it would be, the newest three (store::map::write_in). Never on
/// the NAS; no server follows them: the lead's coordinator serves the newest for review
/// (`GET /map/held`), the plan reads its regions, and lifting the hold publishes the map then.
pub const HELD: &str = "held";

/// The held maps' folder (`HELD`) in the agent's folder `home`.
pub fn held_dir(home: &std::path::Path) -> std::path::PathBuf {
    home.join(MAPLOG).join(HELD)
}
/// How often the map is written whole: the lead's snapshot (the log compacted, docs/formats.md, The map and its log).
const SNAPSHOT_EVERY_S: u64 = 3600;
/// What a batch's `why` says of a map made with the low zooms (store::map::Batch::says), and
/// the log's watch of it: when they last went out (Publishing::since_low).
pub const LOW: &str = "the low zooms";
pub const LOW_WATCH: &str = "why:the low zooms";

/// What the log watches (store::map::log::Log::open_watching): the low zooms' batches, and the
/// names' to-do lists' inputs (landmarks::namestodo::INPUTS).
fn watches(the_plan: &dyn crate::agent::seam::Plan) -> Vec<&'static str> {
    std::iter::once(LOW_WATCH).chain(the_plan.names_todo_inputs().iter().copied()).collect()
}

/// The lead's map as its agent reads it (docs/building.md §2, 5): the log's feed (the map, its batches
/// held, the log's marks), GC's queue (told of each batch here, of each records save by the agent's
/// loop: super::gcqueue), and the head's catalogue: the one posted and checked equal to the map,
/// else made of the map (`catalogue`), kept only while one is held (15 MB beside the map's own
/// 115 MB on 10 Nov, its regions the map's: store::catalogue::Coverage; made again in 0.1 s).
pub struct Here {
    pub feed: std::sync::Arc<store::map::feed::Feed>,
    pub gc: std::sync::Arc<store::map::gc::GcQueue>,
    head: std::sync::Arc<super::bus::Snap<Option<std::sync::Weak<store::catalogue::Catalogue>>>>,
}

impl Default for Here {
    fn default() -> Here {
        Here { feed: Default::default(), gc: Default::default(), head: super::bus::Snap::new(None) }
    }
}

impl std::fmt::Debug for Here {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Here({:?})", self.feed)
    }
}

impl Here {
    /// The map now as a catalogue (`n` its `seq`, `created` its batch's time); None while the log
    /// holds no map. Made of the map once a `seq` while one made is held, else again.
    pub fn catalogue(&self) -> Option<std::sync::Arc<store::catalogue::Catalogue>> {
        let map = self.feed.map()?;
        if let Some(c) = self.head.get().as_ref().as_ref().and_then(std::sync::Weak::upgrade).filter(|c| c.n == map.seq)
        {
            return Some(c);
        }
        match map.catalogue() {
            Ok(c) => {
                let c = std::sync::Arc::new(c);
                self.head.set(Some(std::sync::Arc::downgrade(&c)));
                Some(c)
            }
            Err(e) => {
                eprintln!("maplog: MISMATCH: the map at {} isn't a catalogue: {e:#}", map.seq);
                None
            }
        }
    }

    /// The head's catalogue now `cat` (the one posted, checked equal to the map).
    fn checked(&self, cat: std::sync::Arc<store::catalogue::Catalogue>) {
        self.head.set(Some(std::sync::Arc::downgrade(&cat)));
    }

    /// The `seq` a log started now goes past: the newest a client asked from (a map server, a
    /// mirror: store::map::feed::Feed::asked_past), so no client holds a map newer than the lead's.
    fn floor(&self) -> u64 {
        self.feed.asked_past()
    }
}

impl MapLog {
    /// The map as the agent and the coordinator's `/map/` routes read it: the thread started (its log
    /// in `home`'s `maplog/`, opened at once, so a lead started again answers before its next
    /// publish).
    pub fn here(
        &mut self,
        the_plan: &'static dyn crate::agent::seam::Plan,
        home: &std::path::Path,
    ) -> std::sync::Arc<Here> {
        self.start(the_plan, home);
        self.here.clone().expect("started")
    }

    /// `here`, once its thread is started (None before: not the lead, a dry run).
    pub fn started(&self) -> Option<std::sync::Arc<Here>> {
        self.tx.as_ref().and(self.here.clone())
    }

    fn start(
        &mut self,
        the_plan: &'static dyn crate::agent::seam::Plan,
        home: &std::path::Path,
    ) -> &std::sync::mpsc::SyncSender<bool> {
        let here = self.here.get_or_insert_with(Default::default).clone();
        self.tx.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
            let dir = home.join(MAPLOG);
            let started = std::thread::Builder::new().name("maplog".into()).spawn(move || {
                let mut w = Writer::new(the_plan, dir, here);
                w.open_guarded();
                // (A post taken and not appended, or posted and not told of: the agent stopped.)
                w.take_posted(false);
                for low in rx {
                    w.take_posted(low);
                }
            });
            if let Err(e) = started {
                eprintln!("maplog: MISMATCH: the log's thread didn't start ({e}); no batches written");
            }
            tx
        })
    }

    /// The `catalog` step's job ended, its map posted (made with the low zooms: `low`): its batch, on
    /// the log's thread (started the first time, its log in `home`'s `maplog/`).
    pub fn published(&mut self, the_plan: &'static dyn crate::agent::seam::Plan, home: &std::path::Path, low: bool) {
        match self.start(the_plan, home).try_send(low) {
            Ok(()) => {}
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                eprintln!("maplog: a post waits already: the next batch takes the newest")
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                eprintln!("maplog: MISMATCH: the log's thread is gone; the map posted isn't appended")
            }
        }
    }
}

fn now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

fn unix(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What a publish did to the log.
#[derive(Debug, PartialEq)]
pub(crate) enum Wrote {
    /// The log started at this map (its snapshot).
    Seeded,
    /// A batch appended, of this many entries set and deleted.
    Batch(usize, usize),
    /// Nothing changed in the map.
    Same,
}

/// The log's writer: the log, opened when its thread starts (or at its first publish), and what
/// the agent and the coordinator read (`Here`), told of each batch.
pub(crate) struct Writer {
    the_plan: &'static dyn crate::agent::seam::Plan,
    dir: std::path::PathBuf,
    log: Option<store::map::log::Log>,
    here: std::sync::Arc<Here>,
}

impl Writer {
    pub(crate) fn new(
        the_plan: &'static dyn crate::agent::seam::Plan,
        dir: std::path::PathBuf,
        here: std::sync::Arc<Here>,
    ) -> Writer {
        Writer { the_plan, dir, log: None, here }
    }

    /// The log opened now, its map, the batches it replayed and its marks handed to the feed (a
    /// log not started yet: nothing to hand; it starts at the first map posted).
    fn open(&mut self) -> anyhow::Result<&mut store::map::log::Log> {
        use anyhow::Context;
        if self.log.is_none() {
            let mut log = store::map::log::Log::open_watching(&self.dir, &watches(self.the_plan))
                .with_context(|| format!("open the log in {}", self.dir.display()))?;
            self.here.gc.open(&self.dir, &log.map()).context("open GC's queue")?;
            if !log.map().is_empty() {
                let (base, held) = log.replayed();
                self.here.feed.set_marks(log.marks().clone());
                self.here.feed.start(log.map(), base, &held, log.cursors().clone());
            }
            self.log = Some(log);
        }
        Ok(self.log.as_mut().expect("opened above"))
    }

    /// `open`, a failure or panic logged.
    fn open_guarded(&mut self) {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.open().map(|_| ()))) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("maplog: MISMATCH: {e:#}"),
            Err(_) => {
                eprintln!("maplog: MISMATCH: opening the log panicked");
                self.log = None;
            }
        }
    }

    /// The maps posted (`POSTED`) appended, in turn: one taken before and not appended (the agent
    /// stopped meanwhile), then the newest posted, taken (renamed `TAKEN`, so a post that comes
    /// meanwhile waits for the next), read and appended, then let go. Each failure logged loudly.
    pub(crate) fn take_posted(&mut self, low: bool) {
        let (posted, taken) = (self.dir.join(POSTED), self.dir.join(TAKEN));
        for take in [false, true] {
            if take {
                match std::fs::rename(&posted, &taken) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                    Err(e) => {
                        eprintln!("maplog: MISMATCH: the map posted can't be taken: {e}");
                        return;
                    }
                }
            }
            let cat = match std::fs::read(&taken) {
                Ok(b) => store::catalogue::Catalogue::decode(&b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => Err(e.into()),
            };
            match cat {
                Ok(c) => self.publish_guarded(c, low && take, now()),
                Err(e) => eprintln!("maplog: MISMATCH: the map posted can't be read: {e:#}; no batch"),
            }
            std::fs::remove_file(&taken).ok();
        }
    }

    /// `publish`, a panic in it logged (the log dropped, opened again from its disk next time).
    fn publish_guarded(&mut self, cat: store::catalogue::Catalogue, low: bool, now: std::time::SystemTime) {
        let n = cat.n;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.publish(cat, low, now)));
        if let Err(p) = r {
            let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
            eprintln!("maplog: MISMATCH at map {n}: the writer panicked: {}", why.unwrap_or_default());
            self.log = None;
        }
    }

    /// `cat`'s batch (made with the low zooms: `low`), the map checked against it, and the lead's
    /// snapshot when an hour has passed since the last (`now`); each failure and mismatch logged
    /// loudly.
    pub(crate) fn publish(&mut self, mut cat: store::catalogue::Catalogue, low: bool, now: std::time::SystemTime) {
        let n = cat.n;
        match self.append(&mut cat, low) {
            Ok(w) => {
                eprintln!("maplog: map {} posted: {w:?}", cat.n);
                if cat.n != n {
                    eprintln!("maplog: the map posted as {n} is {}: {n} was taken", cat.n);
                }
                self.here.checked(std::sync::Arc::new(cat));
                self.snapshot(now, w == Wrote::Seeded);
            }
            Err(e) => eprintln!("maplog: MISMATCH at map {n}: {e:#}"),
        }
    }

    /// `cat` as a batch of the log (`seq` its number, or the head's next when that's taken: `cat`
    /// numbered so; its `why` saying `LOW` when it went out with the low zooms made), and the log's
    /// map then checked to be it. The map held once: the batch is a diff of `cat`'s entries made one
    /// at a time (store::map::each_entry), applied in place, and the check another pass of them.
    pub(crate) fn append(&mut self, cat: &mut store::catalogue::Catalogue, low: bool) -> anyhow::Result<Wrote> {
        use anyhow::Context;
        let here = self.here.clone();
        let log = self.open()?;
        // (Numbered past the head; a log started here past its clients' maps, `Here::floor`.)
        cat.n = cat.n.max(if log.map().is_empty() { here.floor() } else { log.map().seq } + 1);
        let cat = &*cat;
        let wrote = if log.map().is_empty() {
            log.seed(store::map::Map::of(cat)?)?;
            here.gc.start(&log.map());
            here.feed.set_marks(log.marks().clone());
            here.feed.start(log.map(), cat.n, &[], log.cursors().clone());
            Wrote::Seeded
        } else {
            let mut why = format!("map {}", cat.n);
            if low {
                why = format!("{why}{}{LOW}", store::map::WHY_SEP);
            }
            let b = log.map().batch_of(cat, &cat.app, &why)?;
            if b.is_empty() {
                Wrote::Same
            } else {
                let map = log.append(&b)?;
                here.gc.batch(&b, unix(now()));
                here.feed.set_marks(log.marks().clone());
                here.feed.push(map, &b);
                Wrote::Batch(b.set.len(), b.del.len())
            }
        };
        // The check: the map is the catalogue, entry for entry, and its digest kept by the batches
        // is the catalogue's entries' from scratch.
        log.map().check_is(cat).context("the log's map isn't the map posted")?;
        Ok(wrote)
    }

    /// The lead's snapshot (the log compacted, its cursors with it), when the last was written an
    /// hour or more before `now`, or the log just started (`started`). Nothing goes to the NAS: a
    /// lead restarted reads its own (docs/formats.md, The map and its log).
    fn snapshot(&mut self, now: std::time::SystemTime, started: bool) {
        let Some(log) = self.log.as_mut() else { return };
        let due = started
            || log.snapshot_at().is_none_or(|t| now.duration_since(t).is_ok_and(|d| d.as_secs() >= SNAPSHOT_EVERY_S));
        if !due {
            return;
        }
        log.set_cursors(self.here.feed.cursors());
        if let Err(e) = log.compact() {
            eprintln!("maplog: compacting the log: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switches_are_the_pools_settings_off_until_set() {
        use crate::coord::asks::{SettingAsk, Settings, HOLD_CATALOG, NEAR_SLOPE};
        let mut s = Settings::default();
        assert_eq!(Switches::of(&s), Switches::default());
        let ask = |name: &str, on: bool| SettingAsk { name: name.into(), value: on.into(), by: "t".into(), at: 1 };
        s.take(&ask(HOLD_CATALOG, true));
        assert_eq!(Switches::of(&s), Switches { held: true, near: false });
        s.take(&ask(NEAR_SLOPE, true));
        assert_eq!(Switches::of(&s), Switches { held: true, near: true });
    }

    #[test]
    fn unmerged_hand_offs_hold_the_catalogue_up_to_the_cap() {
        let cap = super::CATALOGUE_CAP_S;
        assert!(unmerged_holds(true, None) && unmerged_holds(true, Some(cap - 1)));
        assert!(!unmerged_holds(true, Some(cap)) && !unmerged_holds(false, None));
    }

    #[test]
    fn the_cap_counts_from_the_first_hold_and_outlasts_the_minutes_wait() {
        assert_eq!(held_at(None, true, false, 100), Some(100));
        assert_eq!(held_at(Some(100), true, false, 700), Some(100));
        // Past the cap, a catalogue due waiting out its minute: still counted.
        assert_eq!(held_at(Some(100), false, true, 720), Some(100));
        // None held and none due: the count starts again.
        assert_eq!(held_at(Some(100), false, false, 800), None);
    }

    /// The recorded chain of real catalogues (store's `src/map/testdata`), lowest first.
    fn recorded() -> Vec<store::catalogue::Catalogue> {
        store::testkit::recorded_chain()
    }

    #[test]
    fn the_log_s_map_is_each_catalogue() {
        let d = tempfile::tempdir().unwrap();
        let chain = recorded();
        let here = std::sync::Arc::new(Here::default());
        let mut w = Writer::new(crate::agent::seam::for_tests(), d.path().join(MAPLOG), here.clone());
        let t0 = std::time::SystemTime::now();
        let snapshot = d.path().join(MAPLOG).join(store::map::log::SNAPSHOT);
        for (i, c) in chain.iter().enumerate() {
            w.publish(c.clone(), false, t0);
            let m = w.log.as_ref().unwrap().map();
            assert_eq!((m.seq, m.catalogue().unwrap()), (c.n, c.clone()));
            // (What the agent reads: the catalogue checked, not one made again.)
            assert_eq!(*here.catalogue().unwrap(), *c);
            if i == 0 {
                // (Started: the lead's snapshot at once.)
                assert!(snapshot.exists());
            }
        }
        // The lead's snapshot only hourly: the first's, then one an hour on.
        let seq = || store::map::Snapshot::decode(&std::fs::read(&snapshot).unwrap()).unwrap().0.seq;
        assert_eq!(seq(), chain[0].n);
        let last = chain.last().unwrap();
        w.snapshot(t0 + std::time::Duration::from_secs(SNAPSHOT_EVERY_S + 5), false);
        assert_eq!(seq(), last.n, "written again an hour on");
        // Reopened (an agent restarted), the log goes on from where it was.
        let here = std::sync::Arc::new(Here::default());
        let mut again = Writer::new(crate::agent::seam::for_tests(), d.path().join(MAPLOG), here.clone());
        let mut next = last.clone();
        next.n += 1;
        next.units.push("6/0/0".into());
        next.units.sort();
        assert_eq!(again.append(&mut next.clone(), true).unwrap(), Wrote::Batch(1, 0));
        // (Made of the map: none checked since the open.)
        assert_eq!(*here.catalogue().unwrap(), next);
        // The low zooms' batch marked, and the units' (the names' to-do lists' inputs).
        let marks = here.feed.marks();
        assert_eq!((marks[LOW_WATCH].seq, marks["u/"].seq), (next.n, next.n));
        // A map the log doesn't change writes nothing; one numbered as a batch already appended is
        // the head's next.
        assert_eq!(again.append(&mut next.clone(), false).unwrap(), Wrote::Same);
        next.units.pop();
        let mut taken = next.clone();
        assert_eq!(again.append(&mut taken, false).unwrap(), Wrote::Batch(0, 1));
        assert_eq!((taken.n, again.log.as_ref().unwrap().map().seq), (last.n + 2, last.n + 2));
    }

    /// Task #1119: a log started on a lead with none (no NAS snapshot to start from) is numbered
    /// past the newest `seq` a client asked from, so the map's numbers go on.
    #[test]
    fn a_new_log_is_numbered_past_its_clients() {
        let d = tempfile::tempdir().unwrap();
        let chain = recorded();
        let here = std::sync::Arc::new(Here::default());
        // (The servers ask from their maps while there's none here: 503, the seq noted.)
        here.feed.changes(chain[0].n + 40, Some("server/m1"), 1);
        let mut w = Writer::new(crate::agent::seam::for_tests(), d.path().join(MAPLOG), here.clone());
        w.publish(chain[0].clone(), false, std::time::SystemTime::now());
        assert_eq!(here.feed.head(), Some(chain[0].n + 41), "past the client's map");
        // (Once there's a log, the head goes on from it.)
        w.publish(chain[1].clone(), false, std::time::SystemTime::now());
        assert_eq!(here.feed.head(), Some(chain[0].n + 42));
    }

    /// The `catalog` step's post (`POSTED`) is taken and appended whole; one taken and not appended
    /// (the agent stopped between) is appended when the log's thread starts again, before a newer
    /// post; a post read once is let go (never appended again over a newer map).
    #[test]
    fn a_posted_map_is_appended_once() {
        let d = tempfile::tempdir().unwrap();
        let chain = recorded();
        let dir = d.path().join(MAPLOG);
        std::fs::create_dir_all(&dir).unwrap();
        let post =
            |c: &store::catalogue::Catalogue, name: &str| std::fs::write(dir.join(name), c.encode().unwrap()).unwrap();
        let here = std::sync::Arc::new(Here::default());
        let mut w = Writer::new(crate::agent::seam::for_tests(), dir.clone(), here.clone());
        post(&chain[0], POSTED);
        w.take_posted(false);
        assert_eq!(here.feed.head(), Some(chain[0].n));
        assert!(!dir.join(POSTED).exists() && !dir.join(TAKEN).exists(), "let go");
        // (Nothing posted: nothing appended.)
        w.take_posted(false);
        assert_eq!(here.feed.head(), Some(chain[0].n));
        // Taken, the agent stopped; a newer one posted since: both, in turn.
        post(&chain[1], TAKEN);
        post(&chain[2], POSTED);
        let here = std::sync::Arc::new(Here::default());
        let mut w = Writer::new(crate::agent::seam::for_tests(), dir.clone(), here.clone());
        w.open_guarded();
        w.take_posted(true);
        assert_eq!(*here.catalogue().unwrap(), chain[2]);
        assert_eq!(here.feed.marks()[LOW_WATCH].seq, chain[2].n, "the newest went out with the low zooms");
        assert!(!dir.join(POSTED).exists() && !dir.join(TAKEN).exists());
    }
}
