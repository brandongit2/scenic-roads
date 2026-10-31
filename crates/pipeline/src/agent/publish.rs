//! Publishing's switches (docs/building.md §2, Publishing): two under `state/pool/`, each off while
//! missing or `off` in it, `on` in it (or present) on. They act whatever the pool (publishing is
//! the lead's agent's).
//!
//! - **Delta catalogues** (`delta-catalog`): the catalogue written as what changed over the last
//!   (store::catalogue::Delta; a whole one once the deltas since the last whole one add up to one),
//!   and the sizes of the files the last one lists taken from it, not looked at again.
//! - **Slope and tree cover near** (`near-slope`): a done region's slope and tree cover the z6 tiles
//!   near it (crate::agent::build::Publishing::near), their assemblies with the low zooms.
//!
//! And the map's log written beside the catalogues, in shadow (docs/maplog.md §8, step 2: `Shadow`),
//! which the servers follow (step 3: its `feed`, the coordinator's `/map/` routes).

pub const DELTA_CATALOGUE: &str = "state/pool/delta-catalog";
pub const NEAR_SLOPE: &str = "state/pool/near-slope";
/// The switches as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Switches {
    pub delta: bool,
    pub near: bool,
}

/// Whether the switch at `root` is on; None when it can't be read now.
fn on(root: &store::nasfs::NasRoot, switch: &str) -> Option<bool> {
    match super::nasread::read_switch(&root.join(switch)) {
        Ok(s) => Some(s.trim() != "off"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

impl Switches {
    /// The switches at `root`, each one that can't be read now as it was (`was`).
    pub fn read(root: &store::nasfs::NasRoot, was: Switches) -> Switches {
        Switches {
            delta: on(root, DELTA_CATALOGUE).unwrap_or(was.delta),
            near: on(root, NEAR_SLOPE).unwrap_or(was.near),
        }
    }
}

/// Whether hand-offs of the map tiles or the road index waiting to be merged (`unmerged`) hold the
/// catalogue back, a catalogue held back `held_for` seconds so far: until build::CATALOGUE_CAP_S, then it
/// goes (the regions they touch not recorded built: build::plan's `ready`).
pub fn unmerged_holds(unmerged: bool, held_for: Option<u64>) -> bool {
    unmerged && held_for.is_none_or(|s| s < super::build::CATALOGUE_CAP_S)
}

/// When a catalogue due was first held back (build::LastCatalogue::held_at), now `now`, it last `was`:
/// held back now (`held`), kept (or now if none); else kept while a catalogue is planned or due and
/// waiting out its minute (`due`: build::Plan::catalogue_due), so the cap reached isn't lost; else
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

/// The map's log written in shadow beside the catalogues (docs/maplog.md §8, step 2): each catalogue
/// the lead's `catalog` step publishes becomes a batch of the log in the agent's `maplog/`
/// (store::map::log), `seq` its number; the map then is checked equal to that catalogue, and a
/// mismatch is logged loudly. The catalogues are written as before. The servers follow the log
/// through the coordinator's `/map/` routes (step 3), answered from the `feed` it keeps; the agent
/// reads the map from it too (step 4: `Here`).
///
/// The work (the catalogue read, diffed against the map, checked, the hourly snapshots) runs on a
/// thread of its own, never in the agent's loop, and can't hold the loop up: `published` hands the
/// catalogue's number over a channel of one place and returns. While one waits there untaken, a
/// newer one isn't queued (the next batch after it spans both): a stalled NAS read never piles
/// catalogues up. A panic there is logged and the thread goes on, its log opened again at the next
/// catalogue.
#[derive(Default)]
pub struct Shadow {
    tx: Option<std::sync::mpsc::SyncSender<Published>>,
    here: Option<std::sync::Arc<Here>>,
}

/// A catalogue published: the share, its number (None: the newest there), and whether it went out
/// with the low zooms made.
type Published = (store::nasfs::NasRoot, Option<u64>, bool);

/// The log's folder in the agent's home.
pub const MAPLOG: &str = "maplog";
/// How often the map is written whole: the lead's snapshot and the NAS's (docs/maplog.md §4.4).
const SNAPSHOT_EVERY_S: u64 = 3600;
/// What a batch's `why` says of a catalogue made with the low zooms (store::map::Batch::says), and
/// the log's watch of it: when they last went out (build::Publishing::since_low).
pub const LOW: &str = "the low zooms";
pub const LOW_WATCH: &str = "why:the low zooms";

/// What the log watches (store::map::log::Log::open_watching): the low zooms' batches, and the
/// names' to-do lists' inputs (crate::namestodo::INPUTS).
fn watches() -> Vec<&'static str> {
    std::iter::once(LOW_WATCH).chain(crate::namestodo::INPUTS).collect()
}

/// The lead's map as its agent reads it (docs/maplog.md §6.5): the shadow's feed (the map, its
/// batches held, the log's marks), and the head's catalogue: the one the shadow last checked equal
/// to the map, else made of the map once (`catalogue`).
#[derive(Default)]
pub struct Here {
    pub feed: std::sync::Arc<store::map::feed::Feed>,
    head: std::sync::Mutex<Option<std::sync::Arc<store::catalogue::Catalogue>>>,
}

impl std::fmt::Debug for Here {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Here({:?})", self.feed)
    }
}

impl Here {
    /// The map now as a catalogue (`n` its `seq`, `created` its batch's time); None while the log
    /// holds no map. Made of the map at most once a `seq`.
    pub fn catalogue(&self) -> Option<std::sync::Arc<store::catalogue::Catalogue>> {
        let map = self.feed.map()?;
        let mut g = self.head.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.as_ref().filter(|c| c.n == map.seq) {
            return Some(c.clone());
        }
        match map.catalogue() {
            Ok(c) => {
                let c = std::sync::Arc::new(c);
                *g = Some(c.clone());
                Some(c)
            }
            Err(e) => {
                eprintln!("maplog: MISMATCH: the map at {} isn't a catalogue: {e:#}", map.seq);
                None
            }
        }
    }

    /// The head's catalogue now `cat` (the shadow's, checked equal to the map).
    fn checked(&self, cat: std::sync::Arc<store::catalogue::Catalogue>) {
        *self.head.lock().unwrap_or_else(|e| e.into_inner()) = Some(cat);
    }

    /// The head's catalogue as held, whatever its number: what a catalogue after it is read from
    /// (its deltas alone, store::catalogue::read_n_from).
    fn held(&self) -> Option<std::sync::Arc<store::catalogue::Catalogue>> {
        self.head.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Shadow {
    /// The map as the agent and the coordinator's `/map/` routes read it: the thread started (its log
    /// in `home`'s `maplog/`, opened at once, so a lead started again answers before its next
    /// catalogue).
    pub fn here(&mut self, home: &std::path::Path) -> std::sync::Arc<Here> {
        self.start(home);
        self.here.clone().expect("started")
    }

    /// `here`, once its thread is started (None before: not the lead, a dry run).
    pub fn started(&self) -> Option<std::sync::Arc<Here>> {
        self.tx.as_ref().and(self.here.clone())
    }

    fn start(&mut self, home: &std::path::Path) -> &std::sync::mpsc::SyncSender<Published> {
        let here = self.here.get_or_insert_with(Default::default).clone();
        self.tx.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Published>(1);
            let dir = home.join(MAPLOG);
            let started = std::thread::Builder::new().name("maplog".into()).spawn(move || {
                let mut w = Writer::new(dir, here);
                w.open_guarded();
                for (root, n, low) in rx {
                    match w.read(&root, n) {
                        Ok(cat) => w.publish_guarded(&root, cat, low, now()),
                        Err(e) => eprintln!("maplog: MISMATCH: catalogue {n:?} can't be read: {e:#}; no batch"),
                    }
                }
            });
            if let Err(e) = started {
                eprintln!("maplog: MISMATCH: the shadow's thread didn't start ({e}); no batches written");
            }
            tx
        })
    }

    /// Catalogue `n` (None: the newest), just published at `root`, with the low zooms made (`low`):
    /// its batch, on the shadow's thread (started the first time, its log in `home`'s `maplog/`).
    pub fn published(&mut self, home: &std::path::Path, root: &store::nasfs::NasRoot, n: Option<u64>, low: bool) {
        match self.start(home).try_send((root.clone(), n, low)) {
            Ok(()) => {}
            Err(std::sync::mpsc::TrySendError::Full(p)) => {
                eprintln!("maplog: catalogue {:?} not queued: the shadow is behind (the next batch spans it)", p.1)
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(p)) => {
                eprintln!("maplog: MISMATCH at catalogue {:?}: the shadow's thread is gone", p.1)
            }
        }
    }
}

fn now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

/// What a publish did to the log.
#[derive(Debug, PartialEq)]
pub(crate) enum Wrote {
    /// The log started at this catalogue (its snapshot).
    Seeded,
    /// A batch appended, of this many entries set and deleted.
    Batch(usize, usize),
    /// Nothing changed in the map.
    Same,
}

/// The shadow's writer: the log, opened when its thread starts (or at its first publish), and what
/// the agent and the coordinator read (`Here`), told of each batch.
pub(crate) struct Writer {
    dir: std::path::PathBuf,
    log: Option<store::map::log::Log>,
    here: std::sync::Arc<Here>,
}

impl Writer {
    pub(crate) fn new(dir: std::path::PathBuf, here: std::sync::Arc<Here>) -> Writer {
        Writer { dir, log: None, here }
    }

    /// The log opened now, its map, the batches it replayed and its marks handed to the feed (a
    /// log not started yet: nothing to hand; it starts at the first catalogue).
    fn open(&mut self) -> anyhow::Result<&mut store::map::log::Log> {
        use anyhow::Context;
        if self.log.is_none() {
            let mut log = store::map::log::Log::open_watching(&self.dir, &watches())
                .with_context(|| format!("open the log in {}", self.dir.display()))?;
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

    /// Catalogue `n` (None: the newest listed) in `root`'s `catalog/`, read from the head's
    /// catalogue held (its deltas after it alone: store::catalogue::read_n_from), else whole. (The
    /// catalogue's one read left in the agent, until the step posts its map: docs/maplog.md §8,
    /// step 6.)
    fn read(
        &self,
        root: &store::nasfs::NasRoot,
        n: Option<u64>,
    ) -> anyhow::Result<std::sync::Arc<store::catalogue::Catalogue>> {
        use anyhow::Context;
        let dir = root.join(store::catalogue::NAME);
        let n = match n {
            Some(n) => n,
            None => *store::catalogue::list(&dir)?.first().context("no catalogue")?,
        };
        let have = self.here.held();
        Ok(std::sync::Arc::new(store::catalogue::read_n_from(&dir, n, have.as_deref())?))
    }

    /// `publish`, a panic in it logged (the log dropped, opened again from its disk next time).
    fn publish_guarded(
        &mut self,
        root: &store::nasfs::NasRoot,
        cat: std::sync::Arc<store::catalogue::Catalogue>,
        low: bool,
        now: std::time::SystemTime,
    ) {
        let n = cat.n;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.publish(root, cat, low, now)));
        if let Err(p) = r {
            let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
            eprintln!("maplog: MISMATCH at catalogue {n}: the shadow panicked: {}", why.unwrap_or_default());
            self.log = None;
        }
    }

    /// `cat`'s batch (made with the low zooms: `low`), the map checked against it, and the snapshots
    /// when an hour has passed since the last (`now`); each failure and mismatch logged loudly.
    pub(crate) fn publish(
        &mut self,
        root: &store::nasfs::NasRoot,
        cat: std::sync::Arc<store::catalogue::Catalogue>,
        low: bool,
        now: std::time::SystemTime,
    ) {
        match self.append(&cat, low) {
            Ok(w) => {
                eprintln!("maplog: catalogue {} in shadow: {w:?}", cat.n);
                self.here.checked(cat);
                self.snapshots(root, now, w == Wrote::Seeded);
            }
            Err(e) => eprintln!("maplog: MISMATCH at catalogue {}: {e:#}", cat.n),
        }
    }

    /// `cat` as a batch of the log (`seq` its number; its `why` saying `LOW` when it went out with the
    /// low zooms made), and the log's map then checked to be it. The map held once: the batch is a
    /// diff of `cat`'s entries made one at a time (store::map::each_entry), applied in place, and the
    /// check another pass of them.
    pub(crate) fn append(&mut self, cat: &store::catalogue::Catalogue, low: bool) -> anyhow::Result<Wrote> {
        use anyhow::Context;
        let here = self.here.clone();
        let log = self.open()?;
        let wrote = if log.map().is_empty() {
            log.seed(store::map::Map::of(cat)?)?;
            here.feed.set_marks(log.marks().clone());
            here.feed.start(log.map(), cat.n, &[], log.cursors().clone());
            Wrote::Seeded
        } else {
            let seq = log.map().seq;
            anyhow::ensure!(cat.n > seq, "the log is at {seq} already: a catalogue number taken again");
            let mut why = format!("catalogue {}", cat.n);
            if low {
                why = format!("{why}{}{LOW}", store::map::WHY_SEP);
            }
            let b = log.map().batch_of(cat, &cat.app, &why)?;
            if b.is_empty() {
                Wrote::Same
            } else {
                let map = log.append(&b)?;
                here.feed.set_marks(log.marks().clone());
                here.feed.push(map, &b);
                Wrote::Batch(b.set.len(), b.del.len())
            }
        };
        // The check: the map is the catalogue, entry for entry, and its digest kept by the batches
        // is the catalogue's entries' from scratch.
        log.map().check_is(cat).context("the log's map isn't the catalogue")?;
        Ok(wrote)
    }

    /// The lead's snapshot (the log compacted) and the NAS's (`map/<seq>.json.zst`), when the last
    /// was written an hour or more before `now`, or the log just started (`started`).
    fn snapshots(&mut self, root: &store::nasfs::NasRoot, now: std::time::SystemTime, started: bool) {
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
        let map = log.map();
        match store::map::write_nas_snapshot(root, &map) {
            Ok(()) => eprintln!("maplog: the map at {} written to the NAS", map.seq),
            Err(e) => eprintln!("maplog: writing the map's snapshot to the NAS: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_switch_is_off_while_missing_or_off_and_kept_while_unreadable() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            Switches::read(&store::nasfs::NasRoot::local_for_test(d.path()), Switches::default()),
            Switches::default()
        );
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        for (f, s) in [(DELTA_CATALOGUE, "on\n"), (NEAR_SLOPE, "off\n")] {
            std::fs::write(d.path().join(f), s).unwrap();
        }
        let s = Switches::read(&store::nasfs::NasRoot::local_for_test(d.path()), Switches::default());
        assert_eq!(s, Switches { delta: true, near: false });
        // (A switch that's a folder can't be read: as it was.)
        std::fs::remove_file(d.path().join(NEAR_SLOPE)).unwrap();
        std::fs::create_dir_all(d.path().join(NEAR_SLOPE)).unwrap();
        let was = Switches { near: true, ..Default::default() };
        assert!(Switches::read(&store::nasfs::NasRoot::local_for_test(d.path()), was).near);
    }

    #[test]
    fn unmerged_hand_offs_hold_the_catalogue_up_to_the_cap() {
        let cap = super::super::build::CATALOGUE_CAP_S;
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
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../store/src/map/testdata");
        let mut ns = store::catalogue::list(dir.as_path()).unwrap();
        ns.reverse();
        ns.into_iter().map(|n| store::catalogue::read_n(dir.as_path(), n).unwrap()).collect()
    }

    #[test]
    fn the_log_s_map_is_each_catalogue() {
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        let chain = recorded();
        let here = std::sync::Arc::new(Here::default());
        let mut w = Writer::new(d.path().join(MAPLOG), here.clone());
        let t0 = std::time::SystemTime::now();
        for (i, c) in chain.iter().enumerate() {
            w.publish(&root, std::sync::Arc::new(c.clone()), false, t0);
            let m = w.log.as_ref().unwrap().map();
            assert_eq!((m.seq, m.catalogue().unwrap()), (c.n, c.clone()));
            // (What the agent reads: the catalogue checked, not one made again.)
            assert_eq!(*here.catalogue().unwrap(), *c);
            if i == 0 {
                // (Started: the NAS's snapshot at once.)
                assert!(d.path().join(format!("nas/map/{}.json.zst", c.n)).exists());
            }
        }
        // The NAS's snapshot only hourly: none since the first, then one an hour on.
        let snaps = || std::fs::read_dir(d.path().join("nas/map")).unwrap().count();
        assert_eq!(snaps(), 1);
        let last = chain.last().unwrap();
        w.snapshots(&root, t0 + std::time::Duration::from_secs(SNAPSHOT_EVERY_S + 5), false);
        assert!(d.path().join(format!("nas/map/{}.json.zst", last.n)).exists());
        // Reopened (an agent restarted), the log goes on from where it was.
        let here = std::sync::Arc::new(Here::default());
        let mut again = Writer::new(d.path().join(MAPLOG), here.clone());
        let mut next = last.clone();
        next.n += 1;
        next.units.push("6/0/0".into());
        next.units.sort();
        assert_eq!(again.append(&next, true).unwrap(), Wrote::Batch(1, 0));
        // (Made of the map: none checked since the open.)
        assert_eq!(*here.catalogue().unwrap(), next);
        // The low zooms' batch marked, and the units' (the names' to-do lists' inputs).
        let marks = here.feed.marks();
        assert_eq!((marks[LOW_WATCH].seq, marks["u/"].seq), (next.n, next.n));
        // A catalogue number taken again, or one the map doesn't change, writes nothing.
        assert!(again.append(&next, false).is_err());
        next.n += 1;
        assert_eq!(again.append(&next, false).unwrap(), Wrote::Same);
        assert_eq!(again.log.as_ref().unwrap().map().seq, last.n + 1);
    }
}
