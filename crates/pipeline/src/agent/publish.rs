//! Publishing (docs/building.md §2, Publishing): its switch, and the map's log the lead writes.
//!
//! - **Slope and tree cover near** (`state/pool/near-slope`: off while missing or `off` in it, on
//!   while `on` in it or present; acting whatever the pool, publishing being the lead's agent's): a
//!   done region's slope and tree cover the z6 tiles near it (crate::agent::build::Publishing::near),
//!   their assemblies with the low zooms.
//! - **The map's log** (docs/maplog.md): the `catalog` step builds the map from the records and
//!   posts it whole to its agent (`POSTED`, a file in the agent's `maplog/`, `--post`); the agent's
//!   writer (`MapLog`, on a thread of its own) diffs it against the map and appends the batch. The
//!   servers follow the log through the coordinator's `/map/` routes (its `feed`), the agent reads
//!   the map from it (`Here`), and a job or a command reads it from the lead (coordwire::map_now). No
//!   catalogue file is written or read (§8, step 6).

pub const NEAR_SLOPE: &str = "state/pool/near-slope";
/// The switches as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Switches {
    pub near: bool,
}

impl Switches {
    /// The switches at `root`, each one that can't be read now as it was (`was`).
    pub fn read(root: &store::nasfs::NasRoot, was: Switches) -> Switches {
        let near = match super::nasread::read_switch(&root.join(NEAR_SLOPE)) {
            Ok(s) => s.trim() != "off",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => was.near,
        };
        Switches { near }
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

/// The map's log, written by the lead's agent (docs/maplog.md §4, §6.7): each map the `catalog` step
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
/// How often the map is written whole: the lead's snapshot and the NAS's (docs/maplog.md §4.4).
const SNAPSHOT_EVERY_S: u64 = 3600;
/// What a batch's `why` says of a map made with the low zooms (store::map::Batch::says), and
/// the log's watch of it: when they last went out (build::Publishing::since_low).
pub const LOW: &str = "the low zooms";
pub const LOW_WATCH: &str = "why:the low zooms";

/// What the log watches (store::map::log::Log::open_watching): the low zooms' batches, and the
/// names' to-do lists' inputs (crate::namestodo::INPUTS).
fn watches() -> Vec<&'static str> {
    std::iter::once(LOW_WATCH).chain(crate::namestodo::INPUTS).collect()
}

/// The lead's map as its agent reads it (docs/maplog.md §6.5): the log's feed (the map, its batches
/// held, the log's marks), GC's queue (told of each batch here, of each records save by the agent's
/// loop: super::gcqueue), and the head's catalogue: the one posted and checked equal to the map,
/// else made of the map (`catalogue`), kept only while one is held (#930: 367 MB beside the map's
/// own 660 MB on 4 Nov, made again in 0.1 s).
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
}

impl MapLog {
    /// The map as the agent and the coordinator's `/map/` routes read it: the thread started (its log
    /// in `home`'s `maplog/`, opened at once, so a lead started again answers before its next
    /// publish; its snapshots written to `root`).
    pub fn here(&mut self, home: &std::path::Path, root: &store::nasfs::NasRoot) -> std::sync::Arc<Here> {
        self.start(home, root);
        self.here.clone().expect("started")
    }

    /// `here`, once its thread is started (None before: not the lead, a dry run).
    pub fn started(&self) -> Option<std::sync::Arc<Here>> {
        self.tx.as_ref().and(self.here.clone())
    }

    fn start(&mut self, home: &std::path::Path, root: &store::nasfs::NasRoot) -> &std::sync::mpsc::SyncSender<bool> {
        let here = self.here.get_or_insert_with(Default::default).clone();
        self.tx.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
            let (dir, root) = (home.join(MAPLOG), root.clone());
            let started = std::thread::Builder::new().name("maplog".into()).spawn(move || {
                let mut w = Writer::new(dir, here);
                w.open_guarded();
                // (A post taken and not appended, or posted and not told of: the agent stopped.)
                w.take_posted(&root, false);
                for low in rx {
                    w.take_posted(&root, low);
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
    pub fn published(&mut self, home: &std::path::Path, root: &store::nasfs::NasRoot, low: bool) {
        match self.start(home, root).try_send(low) {
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

/// The NAS snapshot's cursor among the clients' (docs/maplog.md §5.4): the oldest kept there.
pub const SNAPSHOT_CLIENT: &str = "snapshot";

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
    dir: std::path::PathBuf,
    log: Option<store::map::log::Log>,
    here: std::sync::Arc<Here>,
}

impl Writer {
    pub(crate) fn new(dir: std::path::PathBuf, here: std::sync::Arc<Here>) -> Writer {
        Writer { dir, log: None, here }
    }

    /// The log opened now, its map, the batches it replayed and its marks handed to the feed (a
    /// log not started yet: nothing to hand; it starts at the first map posted).
    fn open(&mut self) -> anyhow::Result<&mut store::map::log::Log> {
        use anyhow::Context;
        if self.log.is_none() {
            let mut log = store::map::log::Log::open_watching(&self.dir, &watches())
                .with_context(|| format!("open the log in {}", self.dir.display()))?;
            self.here.gc.open(&self.dir, &log.map(), unix(now())).context("open GC's queue")?;
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
    pub(crate) fn take_posted(&mut self, root: &store::nasfs::NasRoot, low: bool) {
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
                Ok(c) => self.publish_guarded(Some(root), c, low && take, now()),
                Err(e) => eprintln!("maplog: MISMATCH: the map posted can't be read: {e:#}; no batch"),
            }
            std::fs::remove_file(&taken).ok();
        }
    }

    /// `publish`, a panic in it logged (the log dropped, opened again from its disk next time).
    fn publish_guarded(
        &mut self,
        root: Option<&store::nasfs::NasRoot>,
        cat: store::catalogue::Catalogue,
        low: bool,
        now: std::time::SystemTime,
    ) {
        let n = cat.n;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.publish(root, cat, low, now)));
        if let Err(p) = r {
            let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
            eprintln!("maplog: MISMATCH at map {n}: the writer panicked: {}", why.unwrap_or_default());
            self.log = None;
        }
    }

    /// `cat`'s batch (made with the low zooms: `low`), the map checked against it, and the snapshots
    /// (to `root`) when an hour has passed since the last (`now`); each failure and mismatch logged
    /// loudly.
    pub(crate) fn publish(
        &mut self,
        root: Option<&store::nasfs::NasRoot>,
        mut cat: store::catalogue::Catalogue,
        low: bool,
        now: std::time::SystemTime,
    ) {
        let n = cat.n;
        match self.append(&mut cat, low) {
            Ok(w) => {
                eprintln!("maplog: map {} posted: {w:?}", cat.n);
                if cat.n != n {
                    eprintln!("maplog: the map posted as {n} is {}: {n} was taken", cat.n);
                }
                self.here.checked(std::sync::Arc::new(cat));
                if let Some(root) = root {
                    self.snapshots(root, now, w == Wrote::Seeded);
                }
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
        if !log.map().is_empty() {
            cat.n = cat.n.max(log.map().seq + 1);
        }
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
            Ok(oldest) => {
                eprintln!("maplog: the map at {} written to the NAS", map.seq);
                // (A reader may start from the oldest kept: it holds its files, docs/maplog.md §5.4.)
                self.here.feed.note(SNAPSHOT_CLIENT, oldest, unix(now));
            }
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
        std::fs::write(d.path().join(NEAR_SLOPE), "off\n").unwrap();
        let s = Switches::read(&store::nasfs::NasRoot::local_for_test(d.path()), Switches { near: true });
        assert_eq!(s, Switches { near: false });
        std::fs::write(d.path().join(NEAR_SLOPE), "on\n").unwrap();
        assert!(Switches::read(&store::nasfs::NasRoot::local_for_test(d.path()), Switches::default()).near);
        // (A switch that's a folder can't be read: as it was.)
        std::fs::remove_file(d.path().join(NEAR_SLOPE)).unwrap();
        std::fs::create_dir_all(d.path().join(NEAR_SLOPE)).unwrap();
        let was = Switches { near: true };
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
        store::testkit::recorded_chain()
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
            w.publish(Some(&root), c.clone(), false, t0);
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

    /// The `catalog` step's post (`POSTED`) is taken and appended whole; one taken and not appended
    /// (the agent stopped between) is appended when the log's thread starts again, before a newer
    /// post; a post read once is let go (never appended again over a newer map).
    #[test]
    fn a_posted_map_is_appended_once() {
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        let chain = recorded();
        let dir = d.path().join(MAPLOG);
        std::fs::create_dir_all(&dir).unwrap();
        let post =
            |c: &store::catalogue::Catalogue, name: &str| std::fs::write(dir.join(name), c.encode().unwrap()).unwrap();
        let here = std::sync::Arc::new(Here::default());
        let mut w = Writer::new(dir.clone(), here.clone());
        post(&chain[0], POSTED);
        w.take_posted(&root, false);
        assert_eq!(here.feed.head(), Some(chain[0].n));
        assert!(!dir.join(POSTED).exists() && !dir.join(TAKEN).exists(), "let go");
        // (Nothing posted: nothing appended.)
        w.take_posted(&root, false);
        assert_eq!(here.feed.head(), Some(chain[0].n));
        // Taken, the agent stopped; a newer one posted since: both, in turn.
        post(&chain[1], TAKEN);
        post(&chain[2], POSTED);
        let here = std::sync::Arc::new(Here::default());
        let mut w = Writer::new(dir.clone(), here.clone());
        w.open_guarded();
        w.take_posted(&root, true);
        assert_eq!(*here.catalogue().unwrap(), chain[2]);
        assert_eq!(here.feed.marks()[LOW_WATCH].seq, chain[2].n, "the newest went out with the low zooms");
        assert!(!dir.join(POSTED).exists() && !dir.join(TAKEN).exists());
    }
}
