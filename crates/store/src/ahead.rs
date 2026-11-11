//! Reads on I/O threads of their own, ahead of the compute that uses them (task #1096): one
//! mechanism for every step reading the NAS or the network while it computes (the terrain's
//! sampler, the slope's terrain tiles and deep mids, the packs' spans, the mids an assembly reads).
//!
//! - **`ahead`**: a list of reads in the order the compute will use them, read on threads of their
//!   own while the compute runs, never more than a window past the item the compute is at
//!   (`Cursor::at`): the reads' memory is the window's, however long the list. The compute's
//!   threads never sit in a read the list foresaw: an item read ahead is waiting for it (where it
//!   keeps it is the reader's own: a GeoTIFF's held bytes, a pack's tiles), one still in flight is
//!   waited for there (its own lock), and one never listed is read by the compute itself.
//! - **`each`**: every item read at once on such threads, the results in order (a z3 tile's mids,
//!   a pack's spans), the first failure stopping the rest.
//! - **`Shelf`**: what a thread beside the compute reads by key (`Shelf::beside`), in the order
//!   the compute is expected to want it, a batch at a time (the slope's terrain tiles from its
//!   neighbours' packs, its deep mids), for the compute to take (`Shelf::take`): one being read is
//!   waited for, one not read yet the taker reads itself (it then never is here), so no compute
//!   thread waits on a read it could make sooner, and no key is read twice.
//! - **Lanes** (`Lane`): a process keeps at most `AT_ONCE` reads in flight on each, whatever
//!   number of lists read at once (a terrain job samples a chunk a thread, each with its own
//!   list): the NAS's share stays what SMB's throughput wants (a few calls in flight) and the
//!   job's NAS weight is still its bytes over its time (jobrt::budget::nas: the reads still count,
//!   only sooner); the network's as wide as a host lets a client be. A permit is held around one
//!   read alone, never across another permit's taking, so lists read inside lists can't deadlock.
//!
//! The threads are the list's (scoped): its reads borrow what the compute does, and end with it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Where a read waits: its lane's reads in flight are held to `AT_ONCE`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// The NAS's share (a stored file in place, a pack, a mid).
    Nas = 0,
    /// The internet (a fetched GeoTIFF's blocks, a pyramid's tiles).
    Net = 1,
}

/// The most reads in flight at once in a process, by `Lane`: the NAS's as many as get_many's span
/// readers and an assembly's mids had each (SMB's throughput wants a few calls in flight; task
/// #534 measured 8 streams at 57 MB/s against one at 25), the network's as many as a host lets a
/// client have (buildstore::fetch::HOST_REQUESTS is the fetcher's own, per host, beneath this).
pub const AT_ONCE: [usize; 2] = [8, 64];

struct Gate {
    held: Mutex<usize>,
    cv: Condvar,
}

static GATES: [Gate; 2] =
    [Gate { held: Mutex::new(0), cv: Condvar::new() }, Gate { held: Mutex::new(0), cv: Condvar::new() }];

/// A read in flight on a lane (`permit`), given back when dropped.
pub struct Permit(Lane);

/// A permit to read on `lane`, waited for while the process has `AT_ONCE` reads in flight there.
pub fn permit(lane: Lane) -> Permit {
    let g = &GATES[lane as usize];
    let mut n = g.held.lock().unwrap_or_else(|e| e.into_inner());
    while *n >= AT_ONCE[lane as usize] {
        n = g.cv.wait(n).unwrap_or_else(|e| e.into_inner());
    }
    *n += 1;
    Permit(lane)
}

impl Drop for Permit {
    fn drop(&mut self) {
        let g = &GATES[self.0 as usize];
        *g.held.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        g.cv.notify_one();
    }
}

/// The reads a lane has in flight now.
#[cfg(test)]
fn in_flight(lane: Lane) -> usize {
    *GATES[lane as usize].held.lock().unwrap_or_else(|e| e.into_inner())
}

/// The reading ahead's state: the next item to read, how far reading may go, whether to stop.
struct State {
    next: usize,
    upto: usize,
    stop: bool,
}

/// Where the compute is in an `ahead` list: it tells each item it's about to use (`at`).
pub struct Cursor<'a> {
    st: &'a Mutex<State>,
    cv: &'a Condvar,
    window: usize,
}

impl Cursor<'_> {
    /// The compute is at item `i`: the reads may go up to `i + window` (never back).
    pub fn at(&self, i: usize) {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        let to = i.saturating_add(self.window);
        if to > g.upto {
            g.upto = to;
            self.cv.notify_all();
        }
    }
}

/// Runs `run` while items `0..n` are read in order on threads of their own (`read(i, stop)`, each
/// under its lane's permit: `lane(i)`, None for an item not read ahead, such as a local file's),
/// never past `window` items beyond the one `run`'s cursor is at (at 0 when it starts). A failed
/// read stops the reading ahead (the compute's own read of that item says why); so does `run`'s
/// end, however it ends: `stop` is set then, for a read waiting to retry to give up (a host
/// giving no answer doesn't keep the step waiting out its retries). The threads: as many as the
/// widest lane among the items lets be in flight, no more than the items.
pub fn ahead<R>(
    n: usize,
    window: usize,
    lane: impl Fn(usize) -> Option<Lane> + Sync,
    read: impl Fn(usize, &Arc<AtomicBool>) -> anyhow::Result<()> + Sync,
    run: impl FnOnce(&Cursor) -> R,
) -> R {
    let st = Mutex::new(State { next: 0, upto: window.max(1), stop: false });
    let cv = Condvar::new();
    let cursor = Cursor { st: &st, cv: &cv, window: window.max(1) };
    let threads = (0..n).filter_map(&lane).map(|l| AT_ONCE[l as usize]).max().unwrap_or(0).min(n);
    if threads == 0 {
        return run(&cursor);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = || {
        cancel.store(true, Ordering::Relaxed);
        st.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        cv.notify_all();
    };
    // (Stopped however `run` ends, a panic too, so the threads end.)
    struct Stop<F: Fn()>(F);
    impl<F: Fn()> Drop for Stop<F> {
        fn drop(&mut self) {
            (self.0)()
        }
    }
    std::thread::scope(|s| {
        for _ in 0..threads {
            std::thread::Builder::new()
                .name("read ahead".into())
                .spawn_scoped(s, || loop {
                    let i = {
                        let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
                        while !g.stop && g.next < n && g.next >= g.upto {
                            g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                        if g.stop || g.next >= n {
                            return;
                        }
                        g.next += 1;
                        g.next - 1
                    };
                    let Some(l) = lane(i) else { continue };
                    let failed = {
                        let _p = permit(l);
                        cancel.load(Ordering::Relaxed) || read(i, &cancel).is_err()
                    };
                    if failed {
                        stop();
                        return;
                    }
                })
                .expect("a read-ahead thread");
        }
        let _stop = Stop(&stop);
        run(&cursor)
    })
}

/// `read(i)` for each `i` below `n`, all at once on threads of their own (as many as `lane`'s
/// `AT_ONCE`, no more than `n`), each under its lane's permit; the results in `i`'s order. The first
/// failure fails it, and no more are started.
pub fn each<T: Send>(n: usize, lane: Lane, read: impl Fn(usize) -> anyhow::Result<T> + Sync) -> anyhow::Result<Vec<T>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let got: Vec<anyhow::Result<Vec<(usize, T)>>> = std::thread::scope(|s| {
        let ws: Vec<_> = (0..AT_ONCE[lane as usize].min(n))
            .map(|_| {
                std::thread::Builder::new()
                    .name("read each".into())
                    .spawn_scoped(s, || {
                        let mut got = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            if i >= n || failed.load(Ordering::Relaxed) {
                                return Ok(got);
                            }
                            let r = {
                                let _p = permit(lane);
                                read(i)
                            };
                            match r {
                                Ok(v) => got.push((i, v)),
                                Err(e) => {
                                    failed.store(true, Ordering::Relaxed);
                                    return Err(e);
                                }
                            }
                        }
                    })
                    .expect("a reading thread")
            })
            .collect();
        ws.into_iter().map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });
    let mut all = Vec::with_capacity(n);
    for g in got {
        all.extend(g?);
    }
    all.sort_by_key(|m| m.0);
    Ok(all.into_iter().map(|m| m.1).collect())
}

/// Values read ahead by key (`fill`, on a thread beside the compute) until the compute takes them
/// (`take`). See the module's doc.
pub struct Shelf<K, V> {
    st: Mutex<ShelfState<K, V>>,
    cv: Condvar,
}

struct ShelfState<K, V> {
    /// Read, not taken yet.
    ready: std::collections::HashMap<K, V>,
    /// Being read now (a batch).
    flight: std::collections::HashSet<K>,
    /// To be read, in a later batch.
    queued: std::collections::HashSet<K>,
}

impl<K: std::hash::Hash + Eq + Clone, V> Default for Shelf<K, V> {
    fn default() -> Self {
        let st = ShelfState { ready: Default::default(), flight: Default::default(), queued: Default::default() };
        Shelf { st: Mutex::new(st), cv: Condvar::new() }
    }
}

impl<K: std::hash::Hash + Eq + Clone + Send + Sync, V: Send> Shelf<K, V> {
    /// Runs `run` while `keys` are read onto the shelf in their order on a thread beside it,
    /// `batch` at a time (`read`: a batch's values, in its order), no further while `window` values
    /// wait there untaken (taken ones make room), till all are read, `run` ends (checked between
    /// batches and while it waits) or a batch fails: then the keys not read are left to their
    /// takers. The keys are listed before `run` starts, so no taker comes before them; a key listed
    /// twice, or on the shelf already, is read once.
    pub fn beside<R>(
        &self,
        keys: &[K],
        batch: usize,
        window: usize,
        read: impl Fn(&[K]) -> anyhow::Result<Vec<V>> + Sync,
        run: impl FnOnce() -> R,
    ) -> R {
        {
            let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
            for k in keys {
                if !g.ready.contains_key(k) && !g.flight.contains(k) {
                    g.queued.insert(k.clone());
                }
            }
        }
        let stop = AtomicBool::new(false);
        // (Stopped however `run` ends, a panic too, so the thread ends.)
        struct Stop<'a>(&'a AtomicBool);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        std::thread::scope(|s| {
            std::thread::Builder::new()
                .name("shelf".into())
                .spawn_scoped(s, || self.fill(keys, batch, window, &stop, &read))
                .expect("a shelf's reading thread");
            let _stop = Stop(&stop);
            run()
        })
    }

    fn fill(
        &self,
        keys: &[K],
        batch: usize,
        window: usize,
        stop: &AtomicBool,
        read: &(dyn Fn(&[K]) -> anyhow::Result<Vec<V>> + Sync),
    ) {
        let mut at = 0;
        while at < keys.len() {
            let mine: Vec<K> = {
                let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
                while g.ready.len() >= window.max(1) && !stop.load(Ordering::Relaxed) {
                    // (Woken by a take; the end seen within a tenth of a second.)
                    let tenth = std::time::Duration::from_millis(100);
                    g = self.cv.wait_timeout(g, tenth).unwrap_or_else(|e| e.into_inner()).0;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let mut mine = Vec::new();
                while at < keys.len() && mine.len() < batch.max(1) {
                    if g.queued.remove(&keys[at]) {
                        g.flight.insert(keys[at].clone());
                        mine.push(keys[at].clone());
                    }
                    at += 1;
                }
                mine
            };
            if mine.is_empty() {
                continue;
            }
            let got = read(&mine);
            let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
            for k in &mine {
                g.flight.remove(k);
            }
            let failed = match got {
                Ok(vs) if vs.len() == mine.len() => {
                    for (k, v) in mine.into_iter().zip(vs) {
                        g.ready.insert(k, v);
                    }
                    false
                }
                _ => true,
            };
            self.cv.notify_all();
            if failed {
                break;
            }
        }
        // (What's left unread, its takers read.)
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        for k in &keys[at.min(keys.len())..] {
            g.queued.remove(k);
        }
        self.cv.notify_all();
    }
}

impl<K: std::hash::Hash + Eq + Clone, V> Shelf<K, V> {
    /// Key `k`'s value from the shelf: there, or once its batch is read (waited for); None when
    /// it isn't to be read (any more, or never was): the taker reads it itself, and the shelf
    /// never will. Taken once: a second take is None.
    pub fn take(&self, k: &K) -> Option<V> {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(v) = g.ready.remove(k) {
                self.cv.notify_all();
                return Some(v);
            }
            if !g.flight.contains(k) {
                g.queued.remove(k);
                return None;
            }
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The shelf emptied: what was read and not taken let go, what was to be read left to its
    /// takers.
    pub fn clear(&self) {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        g.ready.clear();
        g.queued.clear();
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[test]
    fn each_reads_in_order_and_stops_at_a_failure() {
        assert_eq!(each(50, Lane::Nas, |i| Ok(i * 2)).unwrap(), (0..50).map(|i| i * 2).collect::<Vec<_>>());
        let tried = AtomicUsize::new(0);
        let r = each(500, Lane::Nas, |i| {
            tried.fetch_add(1, Ordering::Relaxed);
            if i == 3 {
                anyhow::bail!("no")
            }
            std::thread::sleep(Duration::from_millis(1));
            Ok(i)
        });
        assert!(r.is_err());
        assert!(tried.load(Ordering::Relaxed) < 500, "no more started after the failure");
        assert!(each(0, Lane::Net, Ok).unwrap().is_empty());
    }

    #[test]
    fn a_lane_holds_its_reads_in_flight_whatever_the_lists() {
        // (Two lists at once, each wider than the lane: never more than its AT_ONCE in flight.)
        let most = AtomicUsize::new(0);
        let read = |_: usize| {
            most.fetch_max(in_flight(Lane::Nas), Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(2));
            Ok(())
        };
        std::thread::scope(|s| {
            s.spawn(|| each(40, Lane::Nas, read).unwrap());
            s.spawn(|| ahead(40, 40, |_| Some(Lane::Nas), |i, _| read(i), |_| ()));
            s.spawn(|| each(40, Lane::Nas, read).unwrap());
        });
        let m = most.load(Ordering::Relaxed);
        assert!(m >= 1 && m <= AT_ONCE[Lane::Nas as usize], "{m} reads in flight at once");
    }

    #[test]
    fn ahead_reads_no_further_than_its_window_past_the_cursor() {
        let read = Mutex::new(Vec::new());
        let got = ahead(
            100,
            5,
            |i| (i % 10 != 9).then_some(Lane::Net),
            |i, _| {
                read.lock().unwrap().push(i);
                Ok(())
            },
            |c| {
                for i in 0..20 {
                    c.at(i);
                    // (Every item the window holds is read in time, and none past it.)
                    let t0 = std::time::Instant::now();
                    loop {
                        let r = read.lock().unwrap();
                        assert!(r.iter().all(|&j| j < i + 5), "an item past {} read with the cursor at {i}", i + 4);
                        if (0..i + 5).filter(|j| j % 10 != 9).all(|j| r.contains(&j)) {
                            break;
                        }
                        assert!(t0.elapsed() < Duration::from_secs(60), "the window's items never read");
                        drop(r);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                7
            },
        );
        assert_eq!(got, 7);
        let r = read.into_inner().unwrap();
        assert!(!r.contains(&9) && !r.contains(&19), "an item of no lane isn't read");
        assert!(r.iter().all(|&j| j < 24), "nothing read past the window once the compute ended: {r:?}");
    }

    #[test]
    fn a_shelf_hands_each_key_once_read_ahead_waited_for_or_read_by_its_taker() {
        let shelf: Shelf<u32, u32> = Shelf::default();
        let reads = Mutex::new(Vec::new());
        let keys: Vec<u32> = (0..40).chain([3, 7]).collect();
        // (Batches of 4, at most 8 waiting: the taker takes them in a scrambled order.)
        let read = |b: &[u32]| {
            reads.lock().unwrap().extend_from_slice(b);
            std::thread::sleep(Duration::from_millis(2));
            Ok(b.iter().map(|k| k * 10).collect())
        };
        let (got, inline) = shelf.beside(&keys, 4, 8, read, || {
            // (Once the first batch is under way: its keys are waited for, not read here.)
            let t0 = std::time::Instant::now();
            while reads.lock().unwrap().is_empty() {
                assert!(t0.elapsed() < Duration::from_secs(60), "the shelf never read");
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut got = Vec::new();
            let mut inline = Vec::new();
            for k in (0..40u32).map(|i| (i * 7) % 40) {
                match shelf.take(&k) {
                    Some(v) => got.push((k, v)),
                    None => inline.push(k),
                }
            }
            (got, inline)
        });
        assert!(got.iter().all(|&(k, v)| v == k * 10));
        assert_eq!(got.len() + inline.len(), 40);
        assert!(!got.is_empty(), "nothing read ahead");
        let reads = reads.into_inner().unwrap();
        for k in &inline {
            assert!(!reads.contains(k), "{k}, taken as not to be read, was read by the shelf");
        }
        let mut sorted = reads.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), reads.len(), "a key read twice: {reads:?}");
        assert_eq!(shelf.take(&0), None, "taken once");
        // (A failed batch: its keys and the rest left to their takers.)
        let shelf: Shelf<u32, u32> = Shelf::default();
        let failed = AtomicBool::new(false);
        let read = |b: &[u32]| {
            if b[0] == 2 {
                failed.store(true, Ordering::SeqCst);
                anyhow::bail!("no")
            }
            Ok(vec![b[0]])
        };
        let wait = || {
            while !failed.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        shelf.beside(&[1, 2, 3], 1, 8, read, wait);
        assert_eq!((shelf.take(&1), shelf.take(&2), shelf.take(&3)), (Some(1), None, None));
    }

    #[test]
    fn a_failed_read_or_the_computes_end_stops_the_reading_ahead() {
        // (Reads begun once the failure was in: at most one a thread, taken before it was.)
        let (failed, after, all) = (AtomicBool::new(false), AtomicUsize::new(0), AtomicUsize::new(0));
        ahead(
            1000,
            1000,
            |_| Some(Lane::Nas),
            |i, _| {
                all.fetch_add(1, Ordering::Relaxed);
                if failed.load(Ordering::SeqCst) {
                    after.fetch_add(1, Ordering::Relaxed);
                }
                if i == 2 {
                    failed.store(true, Ordering::SeqCst);
                    anyhow::bail!("gone")
                }
                std::thread::sleep(Duration::from_millis(1));
                Ok(())
            },
            |_| std::thread::sleep(Duration::from_millis(50)),
        );
        let (after, all) = (after.into_inner(), all.into_inner());
        assert!(after <= AT_ONCE[Lane::Nas as usize], "{after} reads begun after the failure");
        assert!(all < 1000, "every item read: the failure stopped nothing");
        // (The compute ending while a read waits on a host that never answers: the read sees `stop`
        // and the threads end, the call with them.)
        let (started, saw) = (AtomicBool::new(false), AtomicBool::new(false));
        ahead(
            4,
            4,
            |_| Some(Lane::Net),
            |_, stop| {
                started.store(true, Ordering::Relaxed);
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                saw.store(true, Ordering::Relaxed);
                Ok(())
            },
            |_| {
                while !started.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            },
        );
        assert!(saw.load(Ordering::Relaxed));
    }
}
