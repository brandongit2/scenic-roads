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

/// The reads a lane has in flight now (tests).
pub fn in_flight(lane: Lane) -> usize {
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
    fn a_failed_read_or_the_computes_end_stops_the_reading_ahead() {
        let n = AtomicUsize::new(0);
        ahead(
            1000,
            1000,
            |_| Some(Lane::Nas),
            |i, _| {
                n.fetch_add(1, Ordering::Relaxed);
                if i == 2 {
                    anyhow::bail!("gone")
                }
                std::thread::sleep(Duration::from_millis(1));
                Ok(())
            },
            |_| std::thread::sleep(Duration::from_millis(50)),
        );
        assert!(n.load(Ordering::Relaxed) < 100, "{} read after a failure", n.load(Ordering::Relaxed));
        // (The compute ending at once: a read waiting sees `stop` and the threads end.)
        let saw = AtomicBool::new(false);
        ahead(
            4,
            4,
            |_| Some(Lane::Net),
            |_, stop| {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                saw.store(true, Ordering::Relaxed);
                Ok(())
            },
            |_| (),
        );
        assert!(saw.load(Ordering::Relaxed));
    }
}
