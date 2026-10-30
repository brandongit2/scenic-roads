//! How the agent's parts share (docs/agent-parts.md §3, task #871): snapshots one part writes and
//! others read (`Snap`), and the typed messages they send each other on `std::sync::mpsc`
//! channels (`ControlMsg`, `BeatMsg`). A part's own state is `&mut` inside its thread; anything
//! another part needs it publishes here. No `Mutex` on shared state outside this module (and
//! nasread's own pools): tests/no_shared_mutex.rs holds the agent's files to it, the ones not yet
//! split counted until their step lands.

use std::sync::{Arc, Mutex};

/// State one part writes whole and others read: an `Arc<T>` swapped under a lock held only for the
/// swap, and a generation counted up at each. A reader takes the `Arc` and works on it with no lock
/// held; nothing reads a half-written state.
pub struct Snap<T> {
    cur: Mutex<(Arc<T>, u64)>,
}

impl<T> Snap<T> {
    /// A snapshot holding `v`, generation 0.
    pub fn new(v: T) -> Arc<Snap<T>> {
        Arc::new(Snap { cur: Mutex::new((Arc::new(v), 0)) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (Arc<T>, u64)> {
        self.cur.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The state as last set.
    pub fn get(&self) -> Arc<T> {
        self.lock().0.clone()
    }

    /// The state as last set, and its generation.
    pub fn read(&self) -> (Arc<T>, u64) {
        let g = self.lock();
        (g.0.clone(), g.1)
    }

    /// `v` set in place of the state; its generation.
    pub fn set(&self, v: T) -> u64 {
        let v = Arc::new(v);
        let mut g = self.lock();
        g.0 = v;
        g.1 += 1;
        g.1
    }

    /// The state replaced by what `f` makes of it, if anything (`f` run under the swap's lock: kept
    /// to a comparison); the new state, if set.
    pub fn update(&self, f: impl FnOnce(&T) -> Option<T>) -> Option<Arc<T>> {
        let mut g = self.lock();
        let v = Arc::new(f(&g.0)?);
        g.0 = v.clone();
        g.1 += 1;
        Some(v)
    }
}

/// What **control** tells the loop (until **jobs**, step 5, takes them): each wakes the loop's wait
/// between rounds, so it acts on it at once rather than after its 20 s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlMsg {
    /// This Mac's hold changed (`crate::control::HoldState`, the later `at` winning).
    Hold,
    /// The app asks to quit, or no longer does.
    Quit,
    /// The owner's settings changed (room target, in-use share, build memory).
    Settings,
    /// A cancel ask waits (`crate::control::CANCEL_REQUESTS`), or a hold ask for another member:
    /// the loop's, as they need the jobs or the pool.
    Ask,
}

/// What **beat** is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BeatMsg {
    /// A state it says changed: written at once (debounced `beat::DEBOUNCE`).
    Changed,
    /// The agent stops: its last beat written, and its thread ends.
    Stop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_is_swapped_whole_and_counted() {
        let s = Snap::new(vec![1, 2]);
        let (old, g0) = s.read();
        assert_eq!(g0, 0);
        assert_eq!(s.set(vec![3]), 1);
        // (The reader's Arc is the state it took, whole, whatever was set since.)
        assert_eq!(*old, vec![1, 2]);
        assert!(s.update(|_| None).is_none() && s.read().1 == 1);
        assert_eq!((s.get().as_slice(), s.read().1), (&[3][..], 1));
        // Many writers, many readers: every read is one set's whole state.
        let s = Snap::new((0u64, 0u64));
        std::thread::scope(|t| {
            for w in 0..4u64 {
                let s = &s;
                t.spawn(move || {
                    for i in 0..1000 {
                        s.set((w * 10_000 + i, w * 10_000 + i));
                    }
                });
            }
            for _ in 0..4 {
                let s = &s;
                t.spawn(move || {
                    for _ in 0..1000 {
                        let v = s.get();
                        assert_eq!(v.0, v.1);
                    }
                });
            }
        });
        assert_eq!(s.read().1, 4000);
    }
}
