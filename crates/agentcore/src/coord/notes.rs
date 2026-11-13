//! A member's notes to the lead (task #1160; docs/coordination.md §10, decision 4): the events of
//! the build's history that only a member sees (`KINDS`: its agent started, its conditions
//! changing, its build caches cleared or freed, the tasks its broker's jobs offered done or given
//! back), carried by its beats into the lead's history. What the lead learns itself (a member's
//! leases granted, its jobs' results and failures as their ends said them, its coming into touch
//! and going out of it) is never noted twice.
//!
//! - **On a member** its agent queues each (`Queue::push`, in its beats' registry,
//!   crate::coord::beats::Beats::note); each beat carries the oldest `BEAT_MAX` not yet taken, to a
//!   lead that says it takes them (`crate::coord::beats::NOTES`); the answer says the last the lead
//!   has (`Noted`), and those up to it go. A beat lost, or its answer, carries them again. In
//!   memory: a note made in the seconds before its agent stops is lost with it; one older than the
//!   history keeps (history::KEEP_S) goes unsent.
//! - **On the lead** a beat's notes past the last it took of that member (`take`), by its agent's
//!   process and the note's number there, go into the history in order; that last is kept in its
//!   store (`noted/<member>`), so a note sent again, to a restarted lead too, is taken once.
//! - **Mixed apps:** a member on an app before it sends none (as before: its events stay on it);
//!   a member sends none to a lead before it (whose answer doesn't say `NOTES`), keeping them.

use super::history::{Event, History, KEEP_S};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// The kinds of history a member notes to the lead.
pub const KINDS: [&str; 6] = ["agent", "conditions", "caches", "task", "task-fail", "task-back"];

/// The most notes a beat carries (a few a day is usual: the rest go with the next beats).
pub const BEAT_MAX: usize = 50;

/// One note: its number in its agent's process, and its event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub n: u64,
    pub event: Event,
}

/// A beat's notes (`held::Beat::notes`): its agent's process (held::process) and the notes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Notes {
    pub process: u64,
    pub notes: Vec<Note>,
}

/// The last note the lead took of a member (`held::Answer::noted`, its store's `noted/<member>`):
/// later processes' and later numbers' are greater.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Noted {
    pub process: u64,
    pub n: u64,
}

/// A member's notes not yet taken by the lead.
#[derive(Debug, Default)]
pub struct Queue {
    next: u64,
    pending: VecDeque<Note>,
}

impl Queue {
    /// `e` queued at `now` (unix s; its time, unless it says one).
    pub fn push(&mut self, mut e: Event, now: u64) {
        if e.t == 0 {
            e.t = now;
        }
        self.next += 1;
        self.pending.push_back(Note { n: self.next, event: e });
    }

    /// What the next beat carries at `now`: the oldest `BEAT_MAX` (those older than the history
    /// keeps let go); None when there are none.
    pub fn out(&mut self, now: u64) -> Option<Notes> {
        let from = now.saturating_sub(KEEP_S);
        self.pending.retain(|x| x.event.t >= from);
        (!self.pending.is_empty()).then(|| Notes {
            process: super::held::process(),
            notes: self.pending.iter().take(BEAT_MAX).cloned().collect(),
        })
    }

    /// The lead has this process's notes up to `n`'s: those go.
    pub fn noted(&mut self, n: Noted) {
        if n.process == super::held::process() {
            self.pending.retain(|x| x.n > n.n);
        }
    }

    /// How many wait.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// The store's key for what the lead took of `member`.
pub fn key(member: &str) -> String {
    format!("noted/{member}")
}

/// On the lead: `notes`' past `last` into `history`, in order; the last taken since (`last` when
/// none was new).
pub fn take(history: &mut History, last: Option<Noted>, notes: &Notes) -> Option<Noted> {
    let mut to = last;
    for x in &notes.notes {
        let at = Noted { process: notes.process, n: x.n };
        if to.is_some_and(|t| at <= t) {
            continue;
        }
        if KINDS.contains(&x.event.kind.as_str()) {
            history.add(Event { seq: 0, ..x.event.clone() });
        }
        to = Some(at);
    }
    to
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, note: &str) -> Event {
        Event { worker: Some("m1".into()), note: note.into(), ..Event::new(kind) }
    }

    /// Taken once by their numbers: again (a lost answer) nothing; a later process's all; an
    /// earlier process's (a beat held on its way) none; a kind not a note's passed over.
    #[test]
    fn notes_are_taken_once_in_order() {
        let mut h = History::load(crate::agent::seam::for_tests(), None);
        let mut q2 = Queue::default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        q2.push(ev("conditions", "on battery"), now);
        q2.push(ev("caches", "freed 3 GB"), now);
        q2.push(ev("lease", "not a note"), now);
        let notes = q2.out(now).unwrap();
        assert_eq!(notes.notes.len(), 3);
        let to = take(&mut h, None, &notes);
        assert_eq!(to, Some(Noted { process: notes.process, n: 3 }));
        let kinds = |h: &History| h.since(0, 99).into_iter().map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&h), ["conditions", "caches"]);
        assert_eq!(take(&mut h, to, &notes), to, "sent again: taken once");
        assert_eq!(kinds(&h).len(), 2);
        let older = Notes { process: notes.process - 1, notes: vec![Note { n: 9, event: ev("agent", "old") }] };
        assert_eq!(take(&mut h, to, &older), to, "an earlier process's: none");
        let later = Notes { process: notes.process + 1, notes: vec![Note { n: 1, event: ev("agent", "app 2") }] };
        assert_eq!(take(&mut h, to, &later), Some(Noted { process: notes.process + 1, n: 1 }));
        assert_eq!(kinds(&h), ["conditions", "caches", "agent"]);
        // (The member drops what the lead has.)
        q2.noted(Noted { process: notes.process, n: 2 });
        assert_eq!(q2.len(), 1);
        q2.noted(Noted { process: notes.process + 1, n: 99 });
        assert_eq!(q2.len(), 1, "another process's word drops none of this one's");
    }

    /// A beat carries `BEAT_MAX` at most, the oldest first; notes older than the history keeps go
    /// unsent.
    #[test]
    fn a_beat_carries_the_oldest_notes_bounded() {
        let mut q = Queue::default();
        let now = 10 * KEEP_S;
        q.push(Event { t: now - KEEP_S - 1, ..ev("agent", "too old") }, now);
        for i in 0..(BEAT_MAX + 5) {
            q.push(ev("caches", &i.to_string()), now);
        }
        let out = q.out(now).unwrap();
        assert_eq!(out.notes.len(), BEAT_MAX);
        assert_eq!(out.notes[0].event.note, "0");
        assert_eq!(q.len(), BEAT_MAX + 5);
        q.noted(Noted { process: out.process, n: out.notes.last().unwrap().n });
        assert_eq!(q.out(now).unwrap().notes.len(), 5);
        q.noted(Noted { process: out.process, n: u64::MAX });
        assert!(q.out(now).is_none());
    }
}
