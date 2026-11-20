//! A key reads only through its context (docs/incremental-planning.md §4.1, Reads beside the
//! arguments): while a thread derives a key (`deriving`, pipeline's agent::steps::key_lines), a
//! NAS call (crate::waits::nas, which every one of crate::nasfs's makes) or the pool's wall clock
//! (jobrt::jobs::now_s) panics in a build with debug assertions (the tests'), naming what it read;
//! `recorded` lets one through: a reader's read by a content name its key read in the manifest
//! first (pipeline's TerrainTiles::target_asked and outlines). A release build checks and counts
//! nothing.
//! Per thread: work a derivation hands to other threads isn't watched.

use std::cell::Cell;

thread_local! {
    /// The key derivations under way on this thread, and the recorded reads within them.
    static DEPTH: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
}

/// A key derivation under way on this thread, while it lives.
pub struct Deriving(());

/// A recorded reader's read under way on this thread, while it lives (`recorded`).
pub struct Recorded(());

/// A key derivation under way on this thread until the guard drops (nested: until the outermost).
pub fn deriving() -> Deriving {
    if cfg!(debug_assertions) {
        DEPTH.with(|d| d.set((d.get().0 + 1, d.get().1)));
    }
    Deriving(())
}

/// A read by a content name the key read first, let through while the guard lives.
pub fn recorded() -> Recorded {
    if cfg!(debug_assertions) {
        DEPTH.with(|d| d.set((d.get().0, d.get().1 + 1)));
    }
    Recorded(())
}

impl Drop for Deriving {
    fn drop(&mut self) {
        if cfg!(debug_assertions) {
            DEPTH.with(|d| d.set((d.get().0 - 1, d.get().1)));
        }
    }
}

impl Drop for Recorded {
    fn drop(&mut self) {
        if cfg!(debug_assertions) {
            DEPTH.with(|d| d.set((d.get().0, d.get().1 - 1)));
        }
    }
}

/// Whether this thread derives a key now, outside a recorded read.
fn unrecorded() -> bool {
    DEPTH.with(|d| matches!(d.get(), (n, 0) if n > 0))
}

/// Panics (with debug assertions) when this thread reads `what` while it derives a key, outside a
/// recorded read.
#[track_caller]
pub fn check(what: &str) {
    if cfg!(debug_assertions) && unrecorded() {
        panic!("a key read {what} beside its context (store::keyread; docs/incremental-planning.md §4.1)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_beside_the_context_panics_while_a_key_is_derived() {
        check("the NAS");
        let d = deriving();
        let inner = deriving();
        drop(inner);
        assert!(unrecorded(), "the outer derivation still under way");
        let r = recorded();
        check("the NAS");
        drop(r);
        let caught = std::panic::catch_unwind(|| check("the NAS"));
        assert!(caught.is_err(), "unrecorded while deriving");
        drop(d);
        check("the NAS");
        assert!(!unrecorded());
    }
}
