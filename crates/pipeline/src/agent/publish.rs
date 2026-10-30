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
//! And the map's log written beside the catalogues, in shadow (docs/maplog.md §8, step 2: `Shadow`).

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
/// mismatch is logged loudly. Nothing reads the log yet, and the catalogues are written as before.
///
/// The work (a catalogue diffed against the map, checked, the hourly snapshots) runs on a thread of
/// its own, never in the agent's loop, and can't hold the loop up: `published` leaves the newest
/// catalogue in a slot of one and returns (a catalogue the thread hadn't taken yet is replaced: the
/// next batch spans both, so a stalled NAS write never piles catalogues up in memory). A panic there
/// is logged and the thread goes on, its log opened again at the next catalogue.
#[derive(Default)]
pub struct Shadow {
    slot: Option<std::sync::Arc<Slot>>,
}

type Published = (store::nasfs::NasRoot, std::sync::Arc<store::catalogue::Catalogue>);

/// The newest catalogue the thread hasn't taken.
#[derive(Default)]
struct Slot {
    next: std::sync::Mutex<Option<Published>>,
    ready: std::sync::Condvar,
}

/// The log's folder in the agent's home.
pub const MAPLOG: &str = "maplog";
/// How often the map is written whole: the lead's snapshot and the NAS's (docs/maplog.md §4.4).
const SNAPSHOT_EVERY_S: u64 = 3600;

impl Shadow {
    /// Catalogue `cat`, just published at `root`: its batch, on the shadow's thread (started the
    /// first time, its log in `home`'s `maplog/`).
    pub fn published(
        &mut self,
        home: &std::path::Path,
        root: &store::nasfs::NasRoot,
        cat: std::sync::Arc<store::catalogue::Catalogue>,
    ) {
        let slot = self.slot.get_or_insert_with(|| {
            let slot = std::sync::Arc::new(Slot::default());
            let (s, dir) = (slot.clone(), home.join(MAPLOG));
            let started = std::thread::Builder::new().name("maplog".into()).spawn(move || {
                let mut w = Writer::new(dir);
                loop {
                    let (root, cat) = {
                        let mut g = s.next.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            match g.take() {
                                Some(p) => break p,
                                None => g = s.ready.wait(g).unwrap_or_else(|e| e.into_inner()),
                            }
                        }
                    };
                    w.publish_guarded(&root, &cat, now());
                }
            });
            if let Err(e) = started {
                eprintln!("maplog: MISMATCH: the shadow's thread didn't start ({e}); no batches written");
            }
            slot
        });
        *slot.next.lock().unwrap_or_else(|e| e.into_inner()) = Some((root.clone(), cat));
        slot.ready.notify_one();
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

/// The shadow's writer: the log, opened at its first publish.
pub(crate) struct Writer {
    dir: std::path::PathBuf,
    log: Option<store::map::log::Log>,
}

impl Writer {
    pub(crate) fn new(dir: std::path::PathBuf) -> Writer {
        Writer { dir, log: None }
    }

    /// `publish`, a panic in it logged (the log dropped, opened again from its disk next time).
    fn publish_guarded(
        &mut self,
        root: &store::nasfs::NasRoot,
        cat: &store::catalogue::Catalogue,
        now: std::time::SystemTime,
    ) {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.publish(root, cat, now)));
        if let Err(p) = r {
            let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
            eprintln!("maplog: MISMATCH at catalogue {}: the shadow panicked: {}", cat.n, why.unwrap_or_default());
            self.log = None;
        }
    }

    /// `cat`'s batch, the map checked against it, and the snapshots when an hour has passed since
    /// the last (`now`); each failure and mismatch logged loudly.
    pub(crate) fn publish(
        &mut self,
        root: &store::nasfs::NasRoot,
        cat: &store::catalogue::Catalogue,
        now: std::time::SystemTime,
    ) {
        match self.append(cat) {
            Ok(w) => {
                eprintln!("maplog: catalogue {} in shadow: {w:?}", cat.n);
                self.snapshots(root, now, w == Wrote::Seeded);
            }
            Err(e) => eprintln!("maplog: MISMATCH at catalogue {}: {e:#}", cat.n),
        }
    }

    /// `cat` as a batch of the log (`seq` its number), and the log's map then checked to be it. The
    /// map held once: the batch is a diff of `cat`'s entries made one at a time (store::map::
    /// each_entry), applied in place, and the check another pass of them.
    pub(crate) fn append(&mut self, cat: &store::catalogue::Catalogue) -> anyhow::Result<Wrote> {
        use anyhow::Context;
        if self.log.is_none() {
            self.log = Some(
                store::map::log::Log::open(&self.dir)
                    .with_context(|| format!("open the log in {}", self.dir.display()))?,
            );
        }
        let log = self.log.as_mut().expect("opened above");
        let wrote = if log.map().is_empty() {
            log.seed(store::map::Map::of(cat)?)?;
            Wrote::Seeded
        } else {
            let seq = log.map().seq;
            anyhow::ensure!(cat.n > seq, "the log is at {seq} already: a catalogue number taken again");
            let b = log.map().batch_of(cat, &cat.app, &format!("catalogue {}", cat.n))?;
            if b.is_empty() {
                Wrote::Same
            } else {
                log.append(&b)?;
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
        let mut w = Writer::new(d.path().join(MAPLOG));
        let t0 = std::time::SystemTime::now();
        for (i, c) in chain.iter().enumerate() {
            w.publish(&root, c, t0);
            let m = w.log.as_ref().unwrap().map();
            assert_eq!((m.seq, m.catalogue().unwrap()), (c.n, c.clone()));
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
        let mut again = Writer::new(d.path().join(MAPLOG));
        let mut next = last.clone();
        next.n += 1;
        next.units.push("6/0/0".into());
        next.units.sort();
        assert_eq!(again.append(&next).unwrap(), Wrote::Batch(1, 0));
        // A catalogue number taken again, or one the map doesn't change, writes nothing.
        assert!(again.append(&next).is_err());
        next.n += 1;
        assert_eq!(again.append(&next).unwrap(), Wrote::Same);
        assert_eq!(again.log.as_ref().unwrap().map().seq, last.n + 1);
    }
}
