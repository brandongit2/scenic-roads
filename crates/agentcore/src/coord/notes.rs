//! A member's notes to the lead (task #1160; docs/coordination.md §10, decision 4): the events of
//! the build's history that only a member sees (`KINDS`: its agent started, its conditions
//! changing, its build caches cleared or freed, the tasks its broker's jobs offered done or given
//! back), carried by its beats into the lead's history. What the lead learns itself (a member's
//! leases granted, its jobs' results and failures as their ends said them, its coming into touch
//! and going out of it) is never noted twice.
//!
//! - **On a member** its agent queues each (`Queue::push`, in its beats' registry,
//!   crate::coord::beats::Beats::note), numbered in its queue (`Queue::process`, its start); each
//!   beat carries the oldest `BEAT_MAX` not yet taken, to a lead that says it takes them
//!   (`crate::coord::beats::NOTES`); the answer says the last the lead has (`Noted`), and those up
//!   to it go. A beat lost, or its answer, carries them again. In memory: a note made in the
//!   seconds before its agent stops is lost with it; one older than the history keeps
//!   (history::KEEP_S) goes unsent, whether or not its lead takes notes.
//! - **On the lead** a beat's notes past the last it took of that member (`take`), by the queue
//!   and the note's number there, go into the history in order, each event saying which note it
//!   was (`history::Event::noted`): its line in the history's file is the lead's word that it took
//!   it, read back as the lead starts (`history::History::noted`), so a note is taken once, by a
//!   lead restarted or crashed between too.
//! - **Mixed apps:** a member on an app before it sends none (as before: its events stay on it);
//!   a member sends none to a lead before it (whose answer doesn't say `NOTES`), keeping them.

use super::history::{Event, History, KEEP_S};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// The kinds of history a member notes to the lead.
pub const KINDS: [&str; 6] = ["agent", "conditions", "caches", "task", "task-fail", "task-back"];

/// The most notes a beat carries (a few a day is usual: the rest go with the next beats).
pub const BEAT_MAX: usize = 50;

/// One note: its number in its queue, and its event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub n: u64,
    pub event: Event,
}

/// A beat's notes (`held::Beat::notes`): its queue's start (`Queue::process`) and the notes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Notes {
    pub process: u64,
    pub notes: Vec<Note>,
}

/// The last note the lead took of a member (`held::Answer::noted`, history::History::noted): later
/// queues' and later numbers' are greater.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Noted {
    pub process: u64,
    pub n: u64,
}

/// The note an event of the lead's history was (history::Event::noted): its member, its queue and
/// its number there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct From {
    pub member: String,
    pub process: u64,
    pub n: u64,
}

/// `f` taken, in each member's last note taken (`last`): the greater kept.
pub fn taken(last: &mut HashMap<String, Noted>, f: &From) {
    let at = Noted { process: f.process, n: f.n };
    let e = last.entry(f.member.clone()).or_insert(at);
    *e = (*e).max(at);
}

/// A member's notes not yet taken by the lead.
#[derive(Debug)]
pub struct Queue {
    /// Its start (`start`), which its notes' numbers go on from: a later queue's is greater, in
    /// this process (a pool side made again) or a later one.
    process: u64,
    next: u64,
    pending: VecDeque<Note>,
}

impl Default for Queue {
    fn default() -> Queue {
        Queue { process: start(), next: 0, pending: VecDeque::new() }
    }
}

/// A new queue's start: unix ms, greater than any queue's before it in this process.
fn start() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let prev = LAST.fetch_max(now, Ordering::Relaxed);
    if prev < now {
        now
    } else {
        LAST.fetch_add(1, Ordering::Relaxed) + 1
    }
}

impl Queue {
    /// `e` queued at `now` (unix s; its time, unless it says one); those older than the history
    /// keeps let go.
    pub fn push(&mut self, mut e: Event, now: u64) {
        if e.t == 0 {
            e.t = now;
        }
        self.next += 1;
        self.pending.push_back(Note { n: self.next, event: e });
        self.let_go(now);
    }

    fn let_go(&mut self, now: u64) {
        let from = now.saturating_sub(KEEP_S);
        self.pending.retain(|x| x.event.t >= from);
    }

    /// What the next beat carries at `now`: the oldest `BEAT_MAX` (those older than the history
    /// keeps let go); None when there are none.
    pub fn out(&mut self, now: u64) -> Option<Notes> {
        self.let_go(now);
        (!self.pending.is_empty())
            .then(|| Notes { process: self.process, notes: self.pending.iter().take(BEAT_MAX).cloned().collect() })
    }

    /// The lead has this queue's notes up to `n`'s: those go.
    pub fn noted(&mut self, n: Noted) {
        if n.process == self.process {
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

/// On the lead: member `member`'s `notes` past the last it took of it into `history`, in order,
/// each event saying which note it was; the last taken (None: none ever).
pub fn take(history: &mut History, member: &str, notes: &Notes) -> Option<Noted> {
    for x in &notes.notes {
        let at = Noted { process: notes.process, n: x.n };
        if history.noted(member).is_some_and(|t| at <= t) {
            continue;
        }
        if KINDS.contains(&x.event.kind.as_str()) {
            let noted = Some(Box::new(From { member: member.to_string(), process: at.process, n: at.n }));
            history.add(Event { seq: 0, noted, ..x.event.clone() });
        } else {
            history.set_noted(member, at);
        }
    }
    history.noted(member)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, note: &str) -> Event {
        Event { worker: Some("m1".into()), note: note.into(), ..Event::new(kind) }
    }

    /// Taken once by their numbers: again (a lost answer) nothing; a later queue's all; an earlier
    /// queue's (a beat held on its way) none; a kind not a note's passed over, its number taken.
    /// Each event says which note it was, so a lead loaded again from its file takes none twice
    /// (task #1160 review: its store's word had been written after the history's line, a crash
    /// between leaving the note to be taken again).
    #[test]
    fn notes_are_taken_once_in_order() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("history.jsonl");
        let mut h = History::load(crate::agent::seam::for_tests(), Some(&p));
        let mut q2 = Queue::default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        q2.push(ev("conditions", "on battery"), now);
        q2.push(ev("caches", "freed 3 GB"), now);
        q2.push(ev("lease", "not a note"), now);
        let notes = q2.out(now).unwrap();
        assert_eq!(notes.notes.len(), 3);
        let to = take(&mut h, "m1", &notes);
        assert_eq!(to, Some(Noted { process: notes.process, n: 3 }));
        let kinds = |h: &History| h.since(0, 99).into_iter().map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&h), ["conditions", "caches"]);
        assert_eq!(
            h.since(0, 99)[1].noted.as_deref(),
            Some(&From { member: "m1".into(), process: notes.process, n: 2 })
        );
        assert_eq!(take(&mut h, "m1", &notes), to, "sent again: taken once");
        assert_eq!(kinds(&h).len(), 2);
        assert_eq!(take(&mut h, "m2", &notes).map(|n| n.n), Some(3), "another member's own");
        assert_eq!(kinds(&h).len(), 4);
        // (Loaded again from its file alone: what it took of each member from its lines; the note
        // of a kind not kept, never written, sent again and passed over again.)
        let mut h = History::load(crate::agent::seam::for_tests(), Some(&p));
        assert_eq!(h.noted("m1"), Some(Noted { process: notes.process, n: 2 }));
        assert_eq!(take(&mut h, "m1", &notes), to);
        assert_eq!(kinds(&h).len(), 4, "none taken twice after a restart");
        let older = Notes { process: notes.process - 1, notes: vec![Note { n: 9, event: ev("agent", "old") }] };
        assert_eq!(take(&mut h, "m1", &older), to, "an earlier queue's: none");
        let later = Notes { process: notes.process + 1, notes: vec![Note { n: 1, event: ev("agent", "app 2") }] };
        assert_eq!(take(&mut h, "m1", &later), Some(Noted { process: notes.process + 1, n: 1 }));
        assert_eq!(kinds(&h)[4..], ["agent"]);
        // (The member drops what the lead has.)
        q2.noted(Noted { process: notes.process, n: 2 });
        assert_eq!(q2.len(), 1);
        q2.noted(Noted { process: notes.process + 1, n: 99 });
        assert_eq!(q2.len(), 1, "another queue's word drops none of this one's");
    }

    /// A queue made again in the same process (a pool side made again) numbers its notes past the
    /// last: a lead that took the first's takes the second's (task #1160 review: both had been the
    /// process's start, the second's notes taken as the first's, never).
    #[test]
    fn a_new_queue_s_notes_are_past_an_older_one_s() {
        let mut h = History::load(crate::agent::seam::for_tests(), None);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let (mut a, mut b) = (Queue::default(), Queue::default());
        a.push(ev("conditions", "a"), now);
        b.push(ev("conditions", "b"), now);
        take(&mut h, "m1", &a.out(now).unwrap());
        take(&mut h, "m1", &b.out(now).unwrap());
        assert_eq!(h.since(0, 9).into_iter().map(|e| e.note).collect::<Vec<_>>(), ["a", "b"]);
    }

    /// A lead change mid-stream: notes lead A took but whose answer was lost go to the next lead B
    /// once (B's history has its own), and back on A only those it hasn't taken; what A took of
    /// this member's queue before never holds back a later queue's.
    #[test]
    fn notes_go_on_to_the_next_lead_once() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let (mut a, mut b) = (
            History::load(crate::agent::seam::for_tests(), None),
            History::load(crate::agent::seam::for_tests(), None),
        );
        let notes = |h: &History| h.since(0, 99).into_iter().map(|e| e.note).collect::<Vec<_>>();
        let mut q = Queue::default();
        q.push(ev("conditions", "1"), now);
        q.push(ev("conditions", "2"), now);
        // (A takes them; its answer is lost.)
        take(&mut a, "m1", &q.out(now).unwrap());
        // (B leads: it takes them, and the member drops them.)
        q.push(ev("conditions", "3"), now);
        let to = take(&mut b, "m1", &q.out(now).unwrap()).unwrap();
        q.noted(to);
        assert_eq!(
            (notes(&a), notes(&b)),
            (vec!["1".to_string(), "2".into()], vec!["1".into(), "2".into(), "3".into()])
        );
        // (A leads again: the notes since, none twice.)
        q.push(ev("conditions", "4"), now);
        let out = q.out(now).unwrap();
        q.noted(take(&mut a, "m1", &out).unwrap());
        assert_eq!(notes(&a), ["1", "2", "4"]);
        assert!(q.is_empty());
        // (The member's agent restarted: a later queue, numbered from 1 again, taken.)
        let mut q = Queue::default();
        q.push(ev("conditions", "5"), now);
        take(&mut a, "m1", &q.out(now).unwrap());
        assert_eq!(notes(&a), ["1", "2", "4", "5"]);
    }

    /// Notes older than the history keeps go whether or not a beat asks for them (a member whose
    /// lead takes none, an app before them, keeps a week's at most).
    #[test]
    fn a_queue_no_lead_takes_keeps_a_week_at_most() {
        let mut q = Queue::default();
        let t0 = 10 * KEEP_S;
        q.push(Event { t: t0, ..ev("caches", "old") }, t0);
        q.push(Event { t: t0 + KEEP_S + 5, ..ev("caches", "new") }, t0 + KEEP_S + 5);
        assert_eq!(q.len(), 1);
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
