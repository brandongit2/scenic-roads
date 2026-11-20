//! How the agent's parts share (docs/agent-parts.md §3, task #871): snapshots one part writes and
//! others read (`Snap`), a one-slot mailbox for work handed on, the latest only (`mailbox`), and
//! the typed messages they send each other on `std::sync::mpsc` channels (`ControlMsg`,
//! `BeatMsg`). A part's own state is `&mut` inside its thread; anything
//! another part needs it publishes here. No `Mutex` on shared state outside this module (and
//! nasread's own: its streaks and phases): tests/no_shared_mutex.rs holds the agent's files to it, the ones not yet
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
}

/// What **control** and **jobs**' thread tell the loop (and control tells **jobs**): each wakes the
/// loop's wait between rounds, so it acts on it at once rather than after its 20 s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlMsg {
    /// This Mac's hold changed (`jobrt::control::HoldState`, the later `at` winning).
    Hold,
    /// The app asks to quit, or no longer does.
    Quit,
    /// The owner's settings changed (room target, in-use share, memory floor).
    Settings,
    /// A cancel ask waits (`jobrt::control::CANCEL_REQUESTS`), or a hold ask for another member:
    /// told to **jobs**' thread (crate::agent::slots), which takes them at once; the loop is woken
    /// for a cancel it took (to record it), or for both when that thread doesn't run.
    Ask,
    /// A running job ended (**jobs**' reaper): the loop records it and fills the slot at once.
    Ended,
}

/// A one-slot mailbox (`mailbox`): what one part hands another to do, the latest only. A `put`
/// replaces what still waits (the reader counts what it never took); the reader's `take` waits
/// for one, and while a `hold` is out (the sender busy with what the reader shouldn't run beside),
/// and gives None once the sender is gone. Holding at most one, it never piles up behind a slow
/// reader.
struct Slot<T> {
    state: Mutex<SlotState<T>>,
    changed: std::sync::Condvar,
}

struct SlotState<T> {
    item: Option<T>,
    holds: usize,
    gone: bool,
}

/// A mailbox's sending side (`mailbox`); dropped, the reader's `take` gives None.
pub struct Post<T>(Arc<Slot<T>>);

/// A mailbox's reading side (`mailbox`).
pub struct Inbox<T>(Arc<Slot<T>>);

/// While it lives, the mailbox's reader takes nothing (`Post::hold`).
pub struct Hold<T>(Arc<Slot<T>>);

/// A one-slot mailbox: its sender and its reader.
pub fn mailbox<T>() -> (Post<T>, Inbox<T>) {
    let s = Arc::new(Slot {
        state: Mutex::new(SlotState { item: None, holds: 0, gone: false }),
        changed: std::sync::Condvar::new(),
    });
    (Post(s.clone()), Inbox(s))
}

impl<T> Slot<T> {
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState<T>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn change(&self, f: impl FnOnce(&mut SlotState<T>)) {
        f(&mut self.lock());
        self.changed.notify_all();
    }
}

impl<T> Post<T> {
    /// `v` left for the reader in place of what still waited (dropped).
    pub fn put(&self, v: T) {
        self.0.change(|s| s.item = Some(v));
    }

    /// The reader takes nothing until it's dropped (a panic dropping it too).
    pub fn hold(&self) -> Hold<T> {
        self.0.change(|s| s.holds += 1);
        Hold(self.0.clone())
    }
}

impl<T> Drop for Post<T> {
    fn drop(&mut self) {
        self.0.change(|s| s.gone = true);
    }
}

impl<T> Drop for Hold<T> {
    fn drop(&mut self) {
        self.0.change(|s| s.holds -= 1);
    }
}

impl<T> Inbox<T> {
    /// What waits, once no hold is out; None once the sender is gone (what still waited dropped).
    pub fn take(&self) -> Option<T> {
        let mut s = self.0.lock();
        loop {
            if s.gone {
                return None;
            }
            if s.holds == 0 {
                if let Some(v) = s.item.take() {
                    return Some(v);
                }
            }
            s = self.0.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
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

    /// A mailbox holds the latest put alone, keeps it from the
    /// reader while a hold is out (a hold dropped by a panic too), and its reader's take gives None
    /// once the sender is gone, even while held.
    #[test]
    fn a_mailbox_holds_the_latest_alone() {
        let (post, inbox) = mailbox::<u32>();
        post.put(1);
        post.put(2);
        assert_eq!(inbox.take(), Some(2));
        let hold = post.hold();
        post.put(3);
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            while let Some(v) = inbox.take() {
                tx.send(v).unwrap();
            }
        });
        assert!(rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(), "nothing taken while held");
        // (A hold dropped by a panic lets the reader go on.)
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _hold = hold;
            panic!("the sender's work panicked");
        }));
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(3));
        let _held = post.hold();
        post.put(4);
        drop(post);
        reader.join().unwrap();
        assert!(rx.try_recv().is_err(), "the sender gone: nothing more taken");
    }
}
