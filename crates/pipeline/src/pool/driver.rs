//! The pool's driver (docs/pool.md §6, §12): what a member is in the pool and what it does as that,
//! decided one step at a time, the same in the agent and in the simulator (crate::pool's `sim`),
//! which is what checks that it keeps §4's invariants and makes progress whatever the share's
//! faults, the Macs' sleeps and stale reads.
//!
//! # The contract
//!
//! **One process per member.** The agent takes its member's lock (crate::pool::MemberLock: a flock
//! named by the member id in the app's folder, so a second process of the member, a second agent or
//! one started from a copy of the agent's folder, can't take it) and makes its one `Driver` with
//! it (`Driver::new`, from the state it saved last: `saved`). A process without the lock steps no
//! driver (it runs dry, as a second agent on one folder does). Every step checks the lock first
//! (its file removed is taken again if it's free); a process another took it from stops for good
//! (`Out::stop`: it writes no heartbeat, sends nothing, and steps no more). Two processes of one
//! member would lead one term twice, and number leases `<term>-<n>` twice.
//!
//! The agent calls `Driver::step` once per loop of its own, about every 20 s, from one thread. A
//! step:
//!
//! - **does its NAS operations through `Io`** (crate::pool::nas::Nas, and two clocks): a few
//!   reads and stats, the records' save (about 3 MB), a term made now and then, and its jobs'
//!   entries written and entries merged for `BUSY_S` at most (the rest in the next steps); a
//!   listing of the terms only at its first step (when the lead's hint doesn't name the current
//!   term); never a listing of the journal, never a sleep, no thread of its own. What's slow (a listing of the
//!   journal, 3 to 33 s a folder on the share under load) it asks for in its output (`Out::list`),
//!   for the agent to make off the loop and hand back, with the ask's number, in a later step
//!   (`Heard::listed`);
//! - **reads the clocks itself**, where a decision needs the time, after what it compares it with
//!   (a heartbeat read, then the clock): the agent passes no time in. The awake clock tells it
//!   what the wall clock can't: that the Mac slept, rather than worked;
//! - **takes what was heard since the last step** (`Heard`): the members' messages to it, its
//!   owner's asks (its menu, `scenic lead`, the pages, as its member's API takes them), the
//!   hand-offs of its jobs that ended, the listing it asked for, what settling a handover wrote,
//!   whether this Mac can lead now (its disk, home, power: the agent's conditions), and the pool's
//!   members as the agent knows them (the lead's status names them);
//! - **gives what to do now** (`Out`): the messages to send, by member id, over the pool's API
//!   (best effort: a message lost is told again or made up for); the pool's fields of this Mac's
//!   heartbeat, to write with the rest of it, stamped as it's written; the term it leads, if it
//!   leads, and what it may do as that now: grant jobs and plan (`duties`); settle a handover
//!   (`settle`: stop granting, cancel its duties in flight, write the coordinator's state and hand
//!   it back in `Heard::settled`); publish a catalog or sweep (GC) only once its records reflect
//!   the journal (`caught_up`: a listing of every day under a day old merged and saved, its
//!   take-up's or a daily one, `listed_at` saying when it began; nothing it was told of waiting; a
//!   re-assertion keeping it), and sweep only on a step that re-asserted (`fresh`: asked with
//!   `Heard::reassert`; it says no later term was made before, not that its records are whole);
//!   a listing to make; and what happened (`Event`s: terms taken up, stepped down from, handed
//!   over; errors), for the history and the log;
//! - **never fails**: an error stops only the duty that met it (said in an `Event::Failed`), and
//!   the step goes on: a lead that can't re-assert stands down rather than stop the loop.
//!
//! What the driver does itself, through `Io`, is what decides safety: making terms
//! (crate::pool::term), taking them up and saving its term's records (crate::pool::records),
//! writing its jobs' entries and merging others' (crate::pool::journal), the handover's transitions
//! (crate::pool::handover). What it leaves to the agent can't break §4's invariants: granting and
//! the coordinator, the merge's checks (`Check`, passed to every step), the duties, the heartbeat's
//! other fields, the messages' transport, and persisting `Saved` (its state between processes,
//! naming its member: its entries not yet acknowledged, kept whole until written) after every step
//! that changed it. **A job's hand-off is kept until a `Saved` from a step it was handed to is on
//! the agent's disk**: before that, a crash loses it. A `Saved` that's lost, or another member's
//! (a copy of the agent's folder), counts for nothing: the driver then re-asserts a term naming it
//! that it finds at its start rather than take it up again (it may have led it, its leases
//! granted), and the state it saves says so, for the processes after it.
//!
//! The controls (the menu, the pages, `scenic lead`) ask the driver what the step would decide:
//! whether a takeover from this Mac needs the owner's force or downgrade, and why (`takeover`);
//! whether the lead can be handed to a member, and why not (`hand_to`).
//!
//! # Re-asserting, standing down, and taking over a lead that stood down
//!
//! A lead's view can be old without its knowing (§6.6). It re-asserts (makes the next term naming
//! itself, a create-new no stale read can fool) before acting again when, since its last step or
//! within its last one, it slept or its wall clock moved more than `GAP_S` beyond its awake clock;
//! when its last step ran over `STALL_S` (the NAS stalled: a share under load slows every
//! operation, so a step's length short of that says nothing); after a restart; and when the agent
//! asks (`Heard::reassert`, before a GC sweep). Time spent listing the journal, or waiting between
//! loops, isn't a gap. A re-assertion keeps what the lead knew of the journal (no other lead came
//! between), so a sweep's step is caught up; after a sleep, its members' messages meanwhile lost,
//! it lists the journal again. A lead whose re-assertion the app rule refuses (it restarted into an older
//! app or a development build) stands down, and says so in its heartbeat (`Beat::stood_down`); it
//! takes its term up again once its app is new enough. A member that has seen the lead's
//! heartbeat stood down for `STOOD_DOWN_S` takes over by itself, if the app rule lets it: the
//! members that can, newest app first, then lowest member id, try `AUTO_RANK_S` apart, and the
//! next term's create-new decides between them (a lead gone or asleep is taken over only at the
//! owner's ask: it may come back).

use super::beat::Beat;
use super::handover::{self, Do, Handover, Seen};
use super::journal::{self, Entry, Mine};
use super::nas::Nas;
use super::records::{self, Check, Records};
use super::term::{self, Current, Made, Term};
use super::{Member, MemberLock};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// More than this unaccounted for, between steps or within one, and a lead re-asserts before it
/// acts (s).
pub const GAP_S: u64 = 60;
/// A step running longer than this, awake: the NAS stalled, and its lead re-asserts (s).
pub const STALL_S: u64 = 300;
/// The time a step spends writing its jobs' entries, and reading entries to merge, before it leaves
/// the rest to the next steps (on a share under load each takes seconds) (s).
pub const BUSY_S: u64 = 60;
/// How often a lead lists the journal's last days for entries no member told it of (their member
/// gone, or told a lead that was no longer current, and gone since) (s).
pub const SWEEP_S: u64 = 600;
/// The days back from today a sweep lists (a take-up lists every day not forgotten).
pub const SWEEP_DAYS: u64 = 2;
/// How long a listing asked for may take before it's asked for again (s).
pub const LISTING_S: u64 = 900;
/// A lead lists every day of the journal not forgotten at least this often (its take-up's listing,
/// then again an hour before this one's a day old), and its records reflect the journal
/// (`Out::caught_up`) only by such a listing begun this recently (s): an entry written late into an
/// old day (kept unwritten while its Mac was away), its member gone before telling of it, waits no
/// longer than this.
pub const RELIST_S: u64 = 86_400;
/// How long a member sees the lead's heartbeat stood down before it takes over by itself (s).
pub const STOOD_DOWN_S: u64 = 120;
/// The wait between the members that can take over a lead that stood down, by rank (s).
pub const AUTO_RANK_S: u64 = 30;
/// An entry a lead's reads find not whole (short, or not there) over this long awake is refused
/// (s): no stale read lasts so long, and its file won't be whole (cut short on the share, or
/// removed). Only reads that answer count: one that fails, the share not answering, starts the
/// time again.
pub const UNREADABLE_S: u64 = 3600;

/// What the driver needs of the world: the NAS's operations, and this Mac's clocks.
pub trait Io: Nas {
    /// This Mac's wall clock: unix seconds (§6.7: no NAS clock; each Mac's own).
    fn now(&self) -> u64;
    /// Seconds this Mac has been awake since some start: a clock that stops while it sleeps (on
    /// macOS, Rust's `Instant`), so the wall clock's lead over it is the time it slept.
    fn awake(&self) -> u64;
}

/// A message between members, over the pool's API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// From a member to the lead it knows: its journal entries that lead hasn't acknowledged.
    Tell(Vec<String>),
    /// From the lead of `term`: those entries are in a snapshot of its records it saved (applied,
    /// passed over or refused), or of days they forgot; and the day before which they forget
    /// (`Records::horizon`), for the member to forget what it had acknowledged before it.
    Ack { term: u64, keys: Vec<String>, horizon: String },
    /// From a lead handing over: the term it made naming the receiver (§6.4, Passed), so it needn't
    /// wait for a stale stat to show it.
    Passed(Term),
    /// From a member that took up the term handed to it: it leads it.
    Leads(u64),
    /// An ask to hand the lead to a member, passed on to the lead by the member whose owner asked.
    HandTo(String),
}

/// An ask of this Mac's owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Hand the lead to member `to` (§6.3): this Mac's lead hands over, if `to` is a live member on
    /// an app new enough (`Driver::hand_to`); another member passes it on to the lead.
    HandTo(String),
    /// Take the lead over (§6.5): when the lead is out of touch or stood down; `force`, also with
    /// the lead in touch, past a term that can't be read, and over this Mac's own handover still
    /// waiting for its target; `downgrade`, also on an older app than the current term's
    /// (crate::pool::term::forced). What it needs now: `Driver::takeover`.
    TakeOver { force: bool, downgrade: bool },
}

/// What a member heard since its last step.
#[derive(Clone, Debug, Default)]
pub struct Heard {
    /// The members' messages to it, by sender's member id.
    pub msgs: Vec<(String, Msg)>,
    /// Its owner's asks.
    pub asks: Vec<Ask>,
    /// The hand-offs of its jobs that ended: kept whole (in `Saved`) until written to the journal;
    /// the agent may drop one once a `Saved` from this step is on its disk.
    pub entries: Vec<Entry>,
    /// The listing of the journal it asked for (`Out::list`), done since, with the ask's number;
    /// none when the listing failed (asked for again later).
    pub listed: Option<Listed>,
    /// Settling a handover (`Out::settle`): the coordinator's state as the agent wrote it once it
    /// stopped granting and cancelled its duties in flight; handed over with the records.
    pub settled: Option<serde_json::Value>,
    /// Whether this Mac can lead now if offered the lead, or take over a lead that stood down (its
    /// disk, home and power; the app rule is the driver's). The agent sets it.
    pub able: bool,
    /// Re-assert before the agent does what only a fresh lead may (a GC sweep): `Out::fresh` says
    /// it did.
    pub reassert: bool,
    /// The pool's members, by id, as the agent knows them (the lead's status names them): who may
    /// take over a lead that stood down, and in what order.
    pub members: Vec<String>,
}

/// A listing of the journal to make, off the loop (crate::pool::journal::list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// The ask's number, handed back with the keys.
    pub n: u64,
    /// The first day to list (YYYY-MM-DD); every day when None.
    pub since: Option<String>,
}

/// A listing made: the ask's number (`Listing::n`) and the keys it found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub n: u64,
    pub keys: Vec<String>,
}

/// What the step came to.
#[derive(Clone, Debug, Default)]
pub struct Out {
    /// Messages to send, by member id.
    pub send: Vec<(String, Msg)>,
    /// The pool's fields of this Mac's heartbeat, to write with the rest of it (its `beat` set to
    /// the clock as it's written).
    pub beat: Beat,
    /// The current term, as this Mac knows it (never goes back).
    pub term: u64,
    /// The term it leads.
    pub leads: Option<u64>,
    /// Leading, whether it may grant jobs and plan now: not while it settles a handover, nor while
    /// its view may be old (a re-assertion due).
    pub duties: bool,
    /// Leading, settling a handover: grant nothing new, cancel the duties in flight, write the
    /// coordinator's state, and hand it back (`Heard::settled`).
    pub settle: bool,
    /// Leading, it re-asserted (or took its term up) this step: no later term was made before. Not
    /// that its records are whole (`caught_up`).
    pub fresh: bool,
    /// Leading, its records reflect the journal: the listing its take-up asked for is merged and
    /// saved, and every entry it was told of or listed is read (on a share under load a loop
    /// leaves some to the next; one never read whole is refused after `UNREADABLE_S`). A
    /// re-assertion keeps it, but after a sleep. A catalog waits for it (and `duties`); GC too
    /// (and `fresh`): an entry not merged yet may hold uploads the records don't name.
    pub caught_up: bool,
    /// Leading: when it asked for the listing of every day its records reflect (this Mac's clock;
    /// within `RELIST_S` while `caught_up`): caught up, every entry written before it is merged.
    pub listed_at: Option<u64>,
    /// This process isn't the member's only one (another took its lock: `MemberLock::check`), and
    /// why: the agent stops the pool. It writes no heartbeat (this step's is empty), sends nothing,
    /// and steps no more; a term it led is left to a takeover.
    pub stop: Option<String>,
    /// A listing of the journal to make, handed back in `Heard::listed`.
    pub list: Option<Listing>,
    /// What happened.
    pub events: Vec<Event>,
}

/// What happened in a step, for the history and the log.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Made term `term`: how its file says.
    Made { term: u64, how: String },
    /// Took up term `term` (how its file says); the coordinator's state handed over with the
    /// records it started from, to load (else the term before's own files').
    TookUp { term: u64, how: String, handed: Option<serde_json::Value> },
    /// No longer leads term `term`: why.
    SteppedDown { term: u64, why: String },
    /// Its handover of term `term` to `to`: offered, settling, passed, over, taken back, given up,
    /// or dropped (the take-back refused by the app rule, or the owner's takeover in its place).
    Handover { term: u64, to: String, what: &'static str },
    /// Journal entries merged: applied, passed over (an older lease's), refused; and whether they
    /// came from a listing.
    Merged { applied: Vec<String>, overtaken: usize, refused: usize, listed: bool },
    /// Something it couldn't do this step, to try again: what, and why.
    Waits { what: &'static str, why: String },
    /// A duty met an error and stopped there; the step went on.
    Failed { what: &'static str, why: String },
}

/// What a member keeps between processes (the agent saves it after every step, in its own
/// folder, and starts the next process's driver from it).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Saved {
    /// The member it's of: another's (a copy of the agent's folder) counts for nothing.
    #[serde(default)]
    pub member: String,
    /// Its journal entries: kept whole until written, then until each lead acknowledges them.
    pub mine: Mine,
    /// The highest term it knew of (its view never goes back, a stale read at its start
    /// notwithstanding), and the highest it led.
    #[serde(default)]
    pub term: u64,
    pub led: u64,
    /// A term its create made and couldn't fill, to finish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfinished: Option<Term>,
    /// The term naming it it stood down from (its app older than the term's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stood_down: Option<u64>,
    /// A handover it passed on and the target hasn't taken up: its own term, the term it made, and
    /// where the handover stands (to take it back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passing: Option<(Term, Term, Handover)>,
}

/// What a takeover from this Mac needs now (`Driver::takeover`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Takeover {
    /// It can't take over now, and why (this Mac leads, or has a term to finish, or the term names
    /// it and it takes it up itself).
    pub refused: Option<String>,
    /// It needs the owner's force, and why (the lead is in touch; the term can't be read whole;
    /// this Mac's own handover waits for its target).
    pub force: Option<String>,
    /// It needs the owner's downgrade too, and why (this Mac's app is older than the term's).
    pub downgrade: Option<String>,
}

/// A term this Mac leads.
#[derive(Clone, Debug)]
struct Lead {
    term: Term,
    records: Records,
    hand: Handover,
    /// The entries members told it of and it hasn't acknowledged, by who told it.
    told: BTreeMap<String, String>,
    /// Entries told or listed not read whole yet (not readable yet, or past the step's time for
    /// reading): read again from the next step on.
    waiting: BTreeSet<String>,
    /// Of those, the ones its reads found not whole, since when (awake clock; a read that failed
    /// starts it again).
    unreadable: BTreeMap<String, u64>,
    /// When it asked for the listing of every day it merged last (its take-up's, or a later one):
    /// None before its take-up's is merged.
    listed_at: Option<u64>,
    /// Changes not saved yet.
    dirty: bool,
    /// Entries refused: their refusals to note once that's saved.
    refused: Vec<(String, String)>,
}

/// A handover this Mac passed on: waiting for its target to lead.
#[derive(Clone, Debug)]
struct Passing {
    /// The term it led and handed over, and the term it made naming the target.
    own: Term,
    passed: Term,
    hand: Handover,
    /// Its records of `own`, to take the lead back with (none after a restart: read then).
    records: Option<Records>,
}

/// A listing asked for and not handed back yet.
#[derive(Clone, Copy, Debug)]
struct Asked {
    n: u64,
    at: u64,
    /// Of every day not forgotten (a take-up's, or the daily one), not the last days' (a sweep's).
    full: bool,
}

/// A member's part in the pool, one step at a time (see the module's doc: the contract).
#[derive(Debug)]
pub struct Driver {
    me: Member,
    /// Its member's lock (none in the simulator), and why it stopped: another process took it.
    lock: Option<MemberLock>,
    stopped: Option<String>,
    cur: Current,
    saved: Saved,
    /// Its saved state is its own: what it led before is known.
    known: bool,
    lead: Option<Lead>,
    passing: Option<Passing>,
    /// A take-up of a term naming it, tried and not done: its records as tried (numbered on).
    taking: Option<Records>,
    /// Its records of a term it led, for a later term naming it to start from.
    spare: Option<Records>,
    /// It must re-assert before acting as lead, and why; and, leading, it slept since it last did
    /// (its members' messages lost meanwhile).
    must: Option<&'static str>,
    slept: bool,
    /// Its clocks (wall, awake) at its last step's end.
    clocks: Option<(u64, u64)>,
    /// The listing due next (a take-up's: every day), the one asked for and not back, the asks'
    /// number, and when it last asked.
    due: Option<(Option<String>, bool)>,
    asked: Option<Asked>,
    asks: u64,
    swept: u64,
    /// The member that handed it a term, by term: told when it leads it.
    handed_by: BTreeMap<u64, String>,
    /// It just started (a restart): a term it led and names it is re-asserted.
    restarted: bool,
    /// The current term it learnt from the NAS at its first step (none before), not counting a
    /// term 1 its own first step made.
    first: Option<u64>,
    /// The lead that stood down, as seen: its term, and since when (this Mac's clock).
    stood: Option<(u64, u64)>,
}

/// Whether the clocks went `was` → `now` (wall, awake) with more than `GAP_S` unaccounted for: the
/// Mac slept, or its wall clock was set, either way.
fn gap(was: (u64, u64), now: (u64, u64)) -> bool {
    let wall = now.0 as i64 - was.0 as i64;
    let awake = now.1 as i64 - was.1 as i64;
    (wall - awake).unsigned_abs() > GAP_S
}

/// The order in which members try to take over a lead that stood down: the newest app first, then
/// the lowest member id.
fn first_to_try(a: &Member, b: &Member) -> Ordering {
    match (term::app_at_least(&a.app, &b.app), term::app_at_least(&b.app, &a.app)) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.id.cmp(&b.id),
    }
}

impl Driver {
    /// The driver of member `me` (its app the process's), holding its lock, from what it saved
    /// last. Another member's state (a copy of the agent's folder, or this Mac's member file lost
    /// and a new id made), or one naming none (lost, or never saved), counts for nothing but its
    /// jobs' hand-offs not written yet: those are written, the same bytes whoever writes them.
    pub fn new(me: Member, saved: Saved, lock: MemberLock) -> Driver {
        Driver { lock: Some(lock), ..Driver::without(me, saved) }
    }

    /// `new` without the lock: the simulator's members and the tests', in one process.
    #[cfg(test)]
    pub fn unlocked(me: Member, saved: Saved) -> Driver {
        Driver::without(me, saved)
    }

    fn without(me: Member, saved: Saved) -> Driver {
        let known = saved.member == me.id;
        let saved = if known { saved } else { Saved { member: me.id.clone(), mine: saved.mine.unwritten_only(), ..Default::default() } };
        let passing = saved.passing.clone().map(|(own, passed, hand)| Passing { own, passed, hand, records: None });
        Driver { me, lock: None, stopped: None, cur: Current::default(), saved, known, lead: None, passing, taking: None, spare: None, must: None, slept: false, clocks: None, due: None, asked: None, asks: 0, swept: 0, handed_by: BTreeMap::new(), restarted: true, first: None, stood: None }
    }

    /// What to keep for the next process (after every step that changed it).
    pub fn saved(&self) -> Saved {
        let mut s = self.saved.clone();
        s.passing = self.passing.as_ref().map(|p| (p.own.clone(), p.passed.clone(), p.hand.clone()));
        s
    }

    /// Its member.
    pub fn member(&self) -> &Member {
        &self.me
    }

    /// Its journal entries, as `Saved` keeps them.
    pub fn mine(&self) -> &Mine {
        &self.saved.mine
    }

    /// The term it leads.
    pub fn leads(&self) -> Option<u64> {
        self.lead.as_ref().map(|l| l.term.term)
    }

    /// One step (see the module's doc: the contract).
    pub fn step(&mut self, io: &dyn Io, heard: Heard, check: Check) -> Out {
        let mut out = Out::default();
        // One process per member: its lock still held, or it stops, writing nothing more.
        if let Some(Err(e)) = self.lock.as_mut().filter(|_| self.stopped.is_none()).map(MemberLock::check) {
            let why = format!("{e:#}");
            if let Some(l) = self.lead.take() {
                out.events.push(Event::SteppedDown { term: l.term.term, why: format!("this process lost its member's lock: {why}") });
            }
            self.passing = None;
            self.stopped = Some(why);
        }
        if let Some(why) = &self.stopped {
            out.stop = Some(why.clone());
            out.term = self.cur.term;
            return out;
        }
        let start = (io.now(), io.awake());
        if self.lead.is_some() {
            if self.clocks.is_some_and(|was| gap(was, start)) {
                self.must = Some("re-asserted after a gap");
                self.slept = true;
            } else if heard.reassert && self.must.is_none() {
                self.must = Some("re-asserted before a sweep");
            }
        }
        // Its jobs' entries, kept whole before their first try, then written.
        for e in heard.entries {
            if let Err(err) = self.saved.mine.add(e) {
                out.events.push(Event::Failed { what: "keep an entry", why: format!("{err:#}") });
            }
        }
        let busy = start.1 + BUSY_S;
        for (k, err) in self.saved.mine.write_while(io, &|| io.awake() < busy) {
            out.events.push(Event::Failed { what: "write an entry", why: format!("{k}: {err:#}") });
        }
        self.finish(io, &mut out);
        // What it heard.
        let (mut tells, mut hand_asks, mut takeover, mut led_by_target, mut passed) = (Vec::new(), Vec::new(), None, false, None);
        for (from, m) in heard.msgs {
            match m {
                Msg::Tell(keys) => tells.push((from, keys)),
                Msg::Ack { term, keys, horizon } => {
                    keys.iter().for_each(|k| self.saved.mine.acked(k, term));
                    if !horizon.is_empty() {
                        self.saved.mine.forget_before(&horizon);
                    }
                }
                Msg::Passed(t) if t.member == self.me.id => {
                    self.handed_by.insert(t.term, from);
                    passed = Some(t);
                }
                Msg::Passed(_) => {}
                Msg::Leads(e) => led_by_target |= self.passing.as_ref().is_some_and(|p| p.passed.term == e && p.passed.member == from),
                Msg::HandTo(to) => hand_asks.push(to),
            }
        }
        for a in heard.asks {
            match a {
                Ask::HandTo(to) => hand_asks.push(to),
                Ask::TakeOver { force, downgrade } => takeover = Some((force, downgrade)),
            }
        }
        if let Err(e) = self.learn(io, passed) {
            out.events.push(Event::Failed { what: "learn the current term", why: format!("{e:#}") });
        }
        if self.taking.as_ref().is_some_and(|r| r.term != self.cur.term) {
            self.taking = None;
        }
        // A lead: a later term ends it; it re-asserts when it must.
        if let Some(l) = &self.lead {
            if self.cur.term > l.term.term {
                self.step_down(&mut out, "a later term exists");
            }
        }
        if self.lead.is_some() {
            if let Some(how) = self.must {
                self.reassert(io, &mut out, how);
            }
        }
        // A member: takes up a term naming it, or its own again.
        if self.lead.is_none() && self.passing.is_none() {
            self.own_term(io, &mut out);
        }
        self.restarted = false;
        if self.lead.is_some() {
            self.lead_step(io, &mut out, tells, hand_asks, heard.listed, heard.settled, check, busy);
        } else if let Some(t) = self.cur.lead.as_ref().filter(|t| t.member != self.me.id) {
            out.send.extend(hand_asks.into_iter().map(|to| (t.member.clone(), Msg::HandTo(to))));
        }
        if self.passing.is_some() {
            self.passing_step(io, &mut out, led_by_target);
        }
        if let Some((force, downgrade)) = takeover {
            self.take_over(io, &mut out, force, downgrade);
        }
        self.auto_take_over(io, &mut out, &heard.members, heard.able);
        // Telling the lead it knows of its entries.
        if let Some(t) = self.cur.lead.as_ref().filter(|t| t.member != self.me.id) {
            let keys = self.saved.mine.to_tell(self.cur.term);
            if !keys.is_empty() {
                out.send.push((t.member.clone(), Msg::Tell(keys)));
            }
        }
        self.listings(io, &mut out);
        out.beat = self.beat(io, heard.able);
        // A step that slept midway, or ran past a stall: its view may be old.
        let end = (io.now(), io.awake());
        if self.lead.is_some() && self.must.is_none() && (gap(start, end) || end.1.saturating_sub(start.1) > STALL_S) {
            self.must = Some("re-asserted after a long step");
            self.slept |= gap(start, end);
        }
        self.clocks = Some(end);
        out.term = self.cur.term;
        if let Some(l) = &self.lead {
            out.leads = Some(l.term.term);
            out.duties = l.hand.grants() && self.must.is_none();
            out.settle = matches!(l.hand, Handover::Settling { .. });
            out.fresh &= self.must.is_none();
            out.caught_up = l.listed_at.is_some_and(|at| end.0.saturating_sub(at) < RELIST_S) && !l.dirty && l.waiting.is_empty() && self.must.is_none();
            out.listed_at = l.listed_at;
        } else {
            out.fresh = false;
        }
        out
    }

    /// Learns the current term: once from the NAS at its first step (making term 1 if it's this
    /// Mac's to make; the lead's hint, or one listing of the terms), then by checking the next;
    /// and from a lead's message handing it one.
    fn learn(&mut self, io: &dyn Io, passed: Option<Term>) -> anyhow::Result<()> {
        if self.cur.term == 0 {
            // (Term 1 made by this call: its `since` this call's.)
            let now = io.now();
            let made = term::bootstrap(io, &self.me, now, false)?.is_some_and(|t| t.member == self.me.id && t.since == now);
            if self.first.is_none() {
                self.cur = term::current(io)?;
                if self.cur.term < self.saved.term {
                    self.cur = Current { term: self.saved.term, lead: term::read(io, self.saved.term)? };
                }
                let first = if made { 0 } else { self.cur.term };
                self.first = Some(first);
                // (Its past unknown, it may have led any term there is: it takes none of them up
                // again, its saved state saying so from now on, a restart's too.)
                if !self.known {
                    self.saved.led = self.saved.led.max(first);
                }
            }
        }
        if self.cur.term > 0 && self.cur.lead.is_none() {
            self.cur.lead = term::read(io, self.cur.term)?;
        }
        while term::next(io, self.cur.term)? {
            let e = self.cur.term + 1;
            self.cur = Current { term: e, lead: term::read(io, e)? };
        }
        // (Made, and its bytes landed: its maker sends it only then.)
        if let Some(t) = passed.filter(|t| t.term > self.cur.term || (t.term == self.cur.term && self.cur.lead.is_none())) {
            self.cur = Current { term: t.term, lead: Some(t) };
        }
        self.saved.term = self.saved.term.max(self.cur.term);
        Ok(())
    }

    /// Finishes the term its create made and couldn't fill; passed on, tells its target.
    fn finish(&mut self, io: &dyn Io, out: &mut Out) {
        let Some(t) = self.saved.unfinished.clone() else { return };
        match term::finish(io, &t) {
            Ok(()) => {
                self.saved.unfinished = None;
                out.events.push(Event::Made { term: t.term, how: t.how.clone() });
                if t.member != self.me.id {
                    out.send.push((t.member.clone(), Msg::Passed(t)));
                }
            }
            Err(e) => out.events.push(Event::Waits { what: "finish the term it made", why: format!("{e:#}") }),
        }
    }

    fn step_down(&mut self, out: &mut Out, why: &str) {
        if let Some(l) = self.lead.take() {
            out.events.push(Event::SteppedDown { term: l.term.term, why: why.to_string() });
            self.spare = Some(l.records);
        }
        self.must = None;
    }

    /// A term this Mac made: noted, and taken up when it names this Mac (else, a pass, its target
    /// told); unfinished, kept to finish.
    fn made(&mut self, io: &dyn Io, out: &mut Out, t: Term, made: Made) -> bool {
        match made {
            Made::Ours => {
                out.events.push(Event::Made { term: t.term, how: t.how.clone() });
                self.cur = Current { term: t.term, lead: Some(t.clone()) };
                self.saved.term = self.saved.term.max(t.term);
                if t.member == self.me.id {
                    self.take_up(io, out, &t);
                } else {
                    out.send.push((t.member.clone(), Msg::Passed(t)));
                }
                true
            }
            Made::Unfinished(e) => {
                out.events.push(Event::Waits { what: "finish the term it made", why: format!("{e:#}") });
                self.cur = Current { term: t.term, lead: Some(t.clone()) };
                self.saved.term = self.saved.term.max(t.term);
                self.saved.unfinished = Some(t);
                true
            }
            Made::Theirs => false,
        }
    }

    /// Re-asserts: makes the next term naming itself and takes it up from its own records; another's
    /// made first, it steps down; the app rule refusing it, it stands down.
    fn reassert(&mut self, io: &dyn Io, out: &mut Out, how: &'static str) {
        let t = match Term::after(&self.cur, &self.me, how, io.now()) {
            Ok(t) => t,
            Err(err) => {
                let e = self.cur.term;
                self.step_down(out, &format!("can't re-assert: {err:#}"));
                self.saved.stood_down = Some(e);
                return;
            }
        };
        match term::make(io, &t) {
            Ok(Made::Theirs) => self.step_down(out, "re-asserting, it found the next term made"),
            Ok(made) => {
                let l = self.lead.take().expect("leading");
                if let Some(h) = l.hand.handing_to(l.term.term) {
                    out.events.push(Event::Handover { term: l.term.term, to: h.to, what: "given up" });
                }
                self.must = None;
                self.spare = Some(l.records);
                let (slept, due, asked) = (std::mem::take(&mut self.slept), self.due.clone(), self.asked);
                self.made(io, out, t, made);
                // (No other lead between: what it knew of the journal holds, its take-up's listing
                // merged too, so a sweep's step is caught up. Not after a sleep: what was written
                // meanwhile, told to it and lost, a listing finds.)
                if let Some(n) = self.lead.as_mut() {
                    (n.told, n.waiting, n.unreadable, n.refused) = (l.told, l.waiting, l.unreadable, l.refused);
                    if l.listed_at.is_some() && !slept {
                        n.listed_at = l.listed_at;
                        (self.due, self.asked) = (due, asked);
                    }
                }
                out.fresh = self.lead.is_some();
            }
            // (Still leading, its duties held until it can.)
            Err(e) => out.events.push(Event::Failed { what: "re-assert", why: format!("{e:#}") }),
        }
    }

    /// A member whose current term names it: takes it up (handed to it, its own claim, or the
    /// owner's takeover on this Mac); or, a term it led and doesn't lead now (restarted, or stood
    /// down), re-asserts it. One it found at its start with its past unknown (its saved state lost,
    /// or another member's) counts as one it led: it may have, its leases granted. Not on an app
    /// older than the term's (restarted into an older app or a development build): it stands down,
    /// and says so, until its app is new enough.
    fn own_term(&mut self, io: &dyn Io, out: &mut Out) {
        // (A term its create made and couldn't fill: finished first, then taken up.)
        if self.saved.unfinished.is_some() {
            return;
        }
        let Some(t) = self.cur.lead.clone().filter(|t| t.member == self.me.id && t.term >= self.saved.led) else { return };
        if !term::app_at_least(&self.me.app, &t.app) {
            if self.saved.stood_down != Some(t.term) {
                self.saved.stood_down = Some(t.term);
                out.events.push(Event::SteppedDown { term: t.term, why: format!("can't re-assert: {} runs app {}, older than term {}'s {}", self.me.host, self.me.app, t.term, t.app) });
            }
            return;
        }
        if t.term > self.saved.led {
            self.take_up(io, out, &t);
            return;
        }
        let how = match () {
            _ if !self.known => "re-asserted: its saved state lost",
            _ if self.restarted => "restarted: re-asserted",
            _ => "re-asserted: its app new enough",
        };
        match Term::after(&self.cur, &self.me, how, io.now()).and_then(|n| Ok((term::make(io, &n)?, n))) {
            Ok((made, n)) => {
                if !self.made(io, out, n, made) {
                    out.events.push(Event::Waits { what: "re-assert", why: format!("term {} made by another first", self.cur.term + 1) });
                }
            }
            Err(e) => out.events.push(Event::Failed { what: "re-assert", why: format!("{e:#}") }),
        }
    }

    /// Takes up term `t`, which names this Mac (§6.2): from the newest records a read finds (its
    /// own, for a term it led; a handover's, the snapshot it names), its first snapshot saved. Not
    /// readable yet, or not saved: tried again next step, numbered on.
    fn take_up(&mut self, io: &dyn Io, out: &mut Out, t: &Term) {
        let own = match &self.taking {
            Some(r) if r.term == t.term => Some(r.clone()),
            _ => self.spare.clone(),
        };
        let mut r = match records::start(io, t, own.as_ref()) {
            Ok(r) => r,
            Err(e) => return out.events.push(Event::Waits { what: "take up", why: format!("term {}: {e:#}", t.term) }),
        };
        let handed = records::handed(&mut r, t);
        // (Nothing merged yet: its members tell it of their entries once they know it leads, and a
        // listing of the journal follows.)
        if let Err(e) = r.save(io) {
            self.taking = Some(r);
            return out.events.push(Event::Failed { what: "save the records", why: format!("taking up term {}: {e:#}", t.term) });
        }
        r.handed = None;
        // The lead's hint (§6.1: for old apps, and to save a listing; never the truth).
        if let Err(e) = term::write_hint(io, t) {
            out.events.push(Event::Failed { what: "write the lead's hint", why: format!("{e:#}") });
        }
        self.taking = None;
        self.spare = None;
        self.slept = false;
        self.saved.led = t.term;
        self.saved.stood_down = None;
        self.must = None;
        out.fresh = true;
        out.events.push(Event::TookUp { term: t.term, how: t.how.clone(), handed });
        let horizon = r.horizon.clone();
        self.lead = Some(Lead { term: t.clone(), records: r, hand: Handover::Leading, told: BTreeMap::new(), waiting: BTreeSet::new(), unreadable: BTreeMap::new(), listed_at: None, dirty: false, refused: Vec::new() });
        // A take-up lists the journal: every day not forgotten.
        self.due = Some((Some(horizon).filter(|h| !h.is_empty()), true));
        self.asked = None;
        // The Mac that handed it over: told it leads.
        if t.seq.is_some() {
            let by = self.handed_by.remove(&t.term).or_else(|| term::read(io, t.from).ok().flatten().map(|f| f.member));
            if let Some(by) = by.filter(|by| *by != self.me.id) {
                out.send.push((by, Msg::Leads(t.term)));
            }
        }
        self.handed_by.retain(|e, _| *e > t.term);
    }

    /// The lead's step: merge what members told it and what a listing found (and what's waiting to
    /// be read whole), save, acknowledge, note the refusals; hand over.
    #[allow(clippy::too_many_arguments)]
    fn lead_step(&mut self, io: &dyn Io, out: &mut Out, tells: Vec<(String, Vec<String>)>, asks: Vec<String>, listed: Option<Listed>, settled: Option<serde_json::Value>, check: Check, busy: u64) {
        let me = self.me.id.clone();
        // An ask, checked (§6.3): as `hand_to` answers the controls.
        let ask = asks.last().cloned().and_then(|to| match self.hand_to(io, &to) {
            Ok(()) => Some(to),
            Err(why) => {
                out.events.push(Event::Waits { what: "hand the lead over", why: format!("to {to}: {why}") });
                None
            }
        });
        let own: Vec<String> = self.saved.mine.to_tell(self.cur.term);
        let asked = self.asked;
        let l = self.lead.as_mut().expect("leading");
        let e = l.term.term;
        for (from, keys) in tells {
            for k in keys {
                l.told.insert(k, from.clone());
            }
        }
        for k in own {
            l.told.insert(k, me.clone());
        }
        let mut keys: Vec<String> = l.told.keys().chain(&l.waiting).filter(|k| !l.records.handles(k)).cloned().collect();
        let from_listing = listed.is_some();
        if let Some(listed) = listed {
            // (A listing of every day, its take-up's or the daily one: once merged and saved, the
            // records reflect the journal as it was when it was asked for. An older ask's keys, a
            // take-up past, are merged as any.)
            if let Some(a) = asked.filter(|a| a.n == listed.n) {
                self.asked = None;
                if a.full {
                    l.listed_at = Some(a.at);
                }
            }
            keys.extend(listed.keys.into_iter().filter(|k| !l.records.handles(k)));
        }
        // Settling: the coordinator's state, as the agent wrote it, saved with the records.
        if let (Some(c), Handover::Settling { .. }) = (settled, &l.hand) {
            l.records.handed = Some(c);
            l.dirty = true;
        }
        // (Oldest lease first, while the step's time for it lasts: the rest wait for the next.)
        records::by_lease(&mut keys);
        keys.dedup();
        let m = records::merge_while(io, &mut l.records, &keys, check, &|| io.awake() < busy);
        if !m.applied.is_empty() || !m.refused.is_empty() || !m.overtaken.is_empty() {
            l.dirty = true;
            l.refused.extend(m.refused.iter().cloned());
            out.events.push(Event::Merged { applied: m.applied.clone(), overtaken: m.overtaken.len(), refused: m.refused.len(), listed: from_listing });
        }
        // What isn't read whole yet is read again next step; what's handled or forgotten, done.
        l.waiting.extend(m.waiting.iter().chain(&m.unread).chain(&m.failed).cloned());
        l.waiting.retain(|k| !l.records.handles(k) && !m.forgotten.contains(k));
        // What its reads find not whole over `UNREADABLE_S` never will be: refused, as a damaged
        // entry is (its work done again). A read that failed says nothing, and starts the time
        // again; one the step's time left unread keeps it, and isn't refused unread.
        let awake = io.awake();
        for k in &m.failed {
            l.unreadable.remove(k);
        }
        for k in &m.waiting {
            l.unreadable.entry(k.clone()).or_insert(awake);
        }
        l.unreadable.retain(|k, _| l.waiting.contains(k));
        let never: Vec<String> = m.waiting.iter().filter(|k| l.unreadable.get(*k).is_some_and(|&at| awake.saturating_sub(at) >= UNREADABLE_S)).cloned().collect();
        if !never.is_empty() {
            let why = format!("not read whole in {} minutes", UNREADABLE_S / 60);
            for k in &never {
                l.records.refuse(k, &why);
                l.waiting.remove(k);
                l.unreadable.remove(k);
                l.refused.push((k.clone(), why.clone()));
            }
            l.dirty = true;
            out.events.push(Event::Merged { applied: Vec::new(), overtaken: 0, refused: never.len(), listed: false });
        }
        if l.dirty {
            match l.records.save(io) {
                Ok(()) => l.dirty = false,
                Err(err) => out.events.push(Event::Failed { what: "save the records", why: format!("term {e}: {err:#}") }),
            }
        }
        // Refusals its saved records name (this step's, or an earlier one's, a re-assertion's take-up
        // saving them): noted for the owner. (A note not made now is only the owner's loss.)
        if !l.dirty {
            for (k, why) in std::mem::take(&mut l.refused) {
                if let Err(err) = journal::note_refusal(io, &k, &why) {
                    out.events.push(Event::Failed { what: "note a refusal", why: format!("{k}: {err:#}") });
                }
            }
        }
        // Acknowledged: what a saved snapshot names, and what's of days forgotten.
        if !l.dirty {
            let mut acks: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (k, from) in &l.told {
                if l.records.handles(k) || m.forgotten.contains(k) {
                    acks.entry(from.clone()).or_default().push(k.clone());
                }
            }
            for (to, keys) in acks {
                for k in &keys {
                    l.told.remove(k);
                }
                if to == me {
                    keys.iter().for_each(|k| self.saved.mine.acked(k, e));
                    if !l.records.horizon.is_empty() {
                        self.saved.mine.forget_before(&l.records.horizon);
                    }
                } else {
                    out.send.push((to, Msg::Ack { term: e, keys, horizon: l.records.horizon.clone() }));
                }
            }
        }
        let settled = (!l.dirty && m.waiting.is_empty() && m.unread.is_empty() && m.failed.is_empty() && l.records.handed.is_some()).then_some(l.records.seq);
        let target = match &l.hand {
            Handover::Offered { to, .. } | Handover::Settling { to, .. } => Beat::read(io, to).ok().flatten(),
            _ => None,
        };
        let was = l.hand.clone();
        let step = l.hand.step(e, &Seen { now: io.now(), current: self.cur.term, ask: ask.as_deref(), target: target.as_ref(), settled, taken_up: false });
        match (&was, &l.hand) {
            (Handover::Leading, Handover::Offered { to, .. }) => out.events.push(Event::Handover { term: e, to: to.clone(), what: "offered" }),
            (Handover::Offered { .. }, Handover::Settling { to, .. }) => {
                // (A fresh settle: the coordinator's state written for it, not an earlier one's.)
                l.records.handed = None;
                out.events.push(Event::Handover { term: e, to: to.clone(), what: "settling" });
            }
            (Handover::Offered { to, .. } | Handover::Settling { to, .. }, Handover::Leading) => {
                l.records.handed = None;
                out.events.push(Event::Handover { term: e, to: to.clone(), what: "given up" });
            }
            _ => {}
        }
        match step {
            Do::StepDown => self.step_down(out, "a later term exists"),
            Do::Pass { to, seq } => {
                let b = target.filter(|b| b.member == to);
                let t = match b.map(|b| Term::after(&self.cur, &b.member(), &format!("handed over by {}", self.me.host), io.now())) {
                    Some(Ok(t)) => t,
                    other => {
                        let why = other.map_or("its heartbeat isn't readable".to_string(), |r| r.err().map(|e| format!("{e:#}")).unwrap_or_default());
                        out.events.push(Event::Waits { what: "hand the lead over", why: format!("to {to}: {why}") });
                        let l = self.lead.as_mut().expect("leading");
                        l.hand.abandon();
                        l.records.handed = None;
                        return;
                    }
                };
                let t = Term { seq: Some(seq), ..t };
                match term::make(io, &t) {
                    Ok(Made::Theirs) => self.step_down(out, "another made the next term first"),
                    Ok(made) => {
                        let now = io.now();
                        let mut l = self.lead.take().expect("leading");
                        l.hand.passed(now);
                        out.events.push(Event::Handover { term: e, to: to.clone(), what: "passed" });
                        out.events.push(Event::SteppedDown { term: e, why: format!("handed over to {to}") });
                        self.passing = Some(Passing { own: l.term, passed: t.clone(), hand: l.hand, records: Some(l.records) });
                        self.made(io, out, t, made);
                    }
                    Err(err) => {
                        out.events.push(Event::Failed { what: "hand the lead over", why: format!("{err:#}") });
                        let l = self.lead.as_mut().expect("leading");
                        l.hand.abandon();
                        l.records.handed = None;
                    }
                }
            }
            _ => {}
        }
    }

    /// A handover passed on: over once the target is known to lead, taken back after two minutes;
    /// dropped when the app rule refuses the take-back (this Mac restarted into an older app), the
    /// term left to its target, or to a takeover.
    fn passing_step(&mut self, io: &dyn Io, out: &mut Out, told: bool) {
        let Some(mut p) = self.passing.take() else { return };
        let Handover::Passed { to, .. } = p.hand.clone() else {
            self.passing = Some(p);
            return;
        };
        // (Not before its term can be read: its maker finishes it first.)
        if self.saved.unfinished.as_ref().is_some_and(|u| u.term == p.passed.term) {
            self.passing = Some(p);
            return;
        }
        let target = Beat::read(io, &to).ok().flatten();
        let snapshot = told || io.exists(&records::path(p.passed.term)).unwrap_or(false);
        let seen = Seen { now: io.now(), current: self.cur.term, target: target.as_ref(), taken_up: snapshot, ..Default::default() };
        match p.hand.step(p.own.term, &seen) {
            Do::Done => out.events.push(Event::Handover { term: p.own.term, to, what: "over" }),
            Do::TakeBack { to } if self.cur.term == p.passed.term => match term::back(&p.own, &p.passed, &self.me, &format!("{}: {to} didn't take up", term::BACK), io.now()) {
                Ok(t) => match term::make(io, &t) {
                    Ok(made) => {
                        self.spare = p.records.take();
                        if self.made(io, out, t, made) {
                            out.events.push(Event::Handover { term: p.own.term, to, what: "taken back" });
                        }
                    }
                    Err(e) => {
                        out.events.push(Event::Failed { what: "take the lead back", why: format!("{e:#}") });
                        self.passing = Some(p);
                    }
                },
                Err(e) => {
                    out.events.push(Event::Handover { term: p.own.term, to, what: "dropped" });
                    out.events.push(Event::Failed { what: "take the lead back", why: format!("{e:#}") });
                    self.spare = p.records.take();
                }
            },
            _ => self.passing = Some(p),
        }
    }

    /// The owner's "Take it": when the lead is out of touch or stood down, or forced (over this
    /// Mac's own handover still waiting for its target too).
    fn take_over(&mut self, io: &dyn Io, out: &mut Out, force: bool, downgrade: bool) {
        let needs = self.takeover(io);
        let why = needs.refused.clone().or_else(|| needs.force.clone().filter(|_| !force)).or_else(|| needs.downgrade.clone().filter(|_| !downgrade));
        if let Some(why) = why {
            return out.events.push(Event::Waits { what: "take over", why });
        }
        if let Some(p) = self.passing.take() {
            out.events.push(Event::Handover { term: p.own.term, to: p.passed.member.clone(), what: "dropped" });
            self.spare = p.records;
        }
        let how = format!("taken over by {}", self.me.host);
        let now = io.now();
        if self.cur.term == 0 {
            match term::bootstrap(io, &self.me, now, true) {
                Ok(Some(t)) => {
                    // (Taken up when this call made it: one there already is the steps' to learn.)
                    self.cur = Current { term: t.term, lead: Some(t.clone()) };
                    if t.member == self.me.id && t.since == now && t.term > self.saved.led {
                        self.take_up(io, out, &t);
                    }
                }
                Ok(None) => {}
                Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
            }
            return;
        }
        let t = if force || downgrade { term::forced(io, &self.cur, &self.me, &how, now, downgrade) } else { Term::after(&self.cur, &self.me, &how, now) };
        match t.and_then(|t| Ok((term::make(io, &t)?, t))) {
            Ok((made, t)) => {
                if !self.made(io, out, t, made) {
                    out.events.push(Event::Waits { what: "take over", why: format!("term {} made by another first", self.cur.term + 1) });
                }
            }
            Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
        }
    }

    /// A lead that stood down (its heartbeat says so: §6.6), seen so for `STOOD_DOWN_S`: taken over
    /// by this Mac, if the app rule lets it, after the members that can and come first (newest app,
    /// then lowest member id) had their turn, `AUTO_RANK_S` each.
    fn auto_take_over(&mut self, io: &dyn Io, out: &mut Out, members: &[String], able: bool) {
        let t = match &self.cur.lead {
            Some(t) if t.member != self.me.id && self.lead.is_none() && self.passing.is_none() && self.saved.unfinished.is_none() => t.clone(),
            _ => {
                self.stood = None;
                return;
            }
        };
        // (A stand-down is its term's for good: in touch or not by this Mac's clock, which may be
        // minutes off the lead's, its heartbeat saying so is enough.)
        let b = Beat::read(io, &t.member).ok().flatten();
        let now = io.now();
        if !b.is_some_and(|b| b.stood_down == Some(t.term)) {
            self.stood = None;
            return;
        }
        let since = match self.stood {
            Some((e, since)) if e == t.term => since,
            _ => {
                self.stood = Some((t.term, now));
                now
            }
        };
        if !able || !term::app_at_least(&self.me.app, &t.app) || now.saturating_sub(since) < STOOD_DOWN_S {
            return;
        }
        let mut can = vec![self.me.clone()];
        for m in members.iter().filter(|m| **m != self.me.id && **m != t.member) {
            if let Ok(Some(b)) = Beat::read(io, m) {
                if !b.out_of_touch(now) && b.leads.is_none() && b.stood_down.is_none() && term::app_at_least(&b.app, &t.app) {
                    can.push(b.member());
                }
            }
        }
        can.sort_by(first_to_try);
        let rank = can.iter().position(|m| m.id == self.me.id).unwrap_or(0) as u64;
        if now.saturating_sub(since) < STOOD_DOWN_S + rank * AUTO_RANK_S {
            return;
        }
        let how = format!("taken over by {}: {} stood down", self.me.host, t.host);
        match Term::after(&self.cur, &self.me, &how, io.now()).and_then(|n| Ok((term::make(io, &n)?, n))) {
            Ok((made, n)) => {
                if !self.made(io, out, n, made) {
                    out.events.push(Event::Waits { what: "take over", why: format!("term {} made by another first", self.cur.term + 1) });
                }
            }
            Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
        }
    }

    /// What a takeover from this Mac needs now, and why (the controls: "Take Over the Build…",
    /// "Take it", `scenic lead take`; §6.5): as a step would decide `Ask::TakeOver`.
    pub fn takeover(&self, io: &dyn Io) -> Takeover {
        let mut t = Takeover::default();
        if self.lead.is_some() {
            t.refused = Some(format!("this Mac leads term {}", self.cur.term));
            return t;
        }
        if let Some(u) = &self.saved.unfinished {
            t.refused = Some(format!("term {}, which this Mac made, isn't written whole yet", u.term));
            return t;
        }
        if let Some(p) = &self.passing {
            t.force = Some(format!("this Mac's handover of term {} waits for {} to take up: forced, it's dropped", p.own.term, p.passed.host));
        }
        let app = |k: &Term| (!term::app_at_least(&self.me.app, &k.app)).then(|| format!("this Mac's app {} is older than term {}'s {}", self.me.app, k.term, k.app));
        match &self.cur.lead {
            // (No term yet: the owner's say-so makes term 1 here, its records first.)
            _ if self.cur.term == 0 => {}
            None => {
                t.force.get_or_insert_with(|| format!("term {} can't be read whole", self.cur.term));
                let known = (1..self.cur.term).rev().find_map(|e| term::read(io, e).ok().flatten());
                t.downgrade = known.as_ref().and_then(app);
            }
            Some(c) if c.member == self.me.id => {
                if self.saved.stood_down == Some(c.term) {
                    t.downgrade = app(c);
                } else {
                    t.refused = Some(format!("term {} names this Mac: it takes it up itself", c.term));
                }
            }
            Some(c) => {
                match Beat::read(io, &c.member) {
                    Ok(Some(b)) => {
                        let now = io.now();
                        if !b.out_of_touch(now) && b.stood_down != Some(c.term) {
                            t.force.get_or_insert_with(|| format!("{} leads, in touch: its beat {} s old", c.host, now.saturating_sub(b.beat)));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        t.force.get_or_insert_with(|| format!("{}'s heartbeat can't be read: {e:#}", c.host));
                    }
                }
                t.downgrade = app(c);
            }
        }
        t
    }

    /// Whether the lead can be handed to member `to` now, and why not (§6.3: a live member, on an
    /// app the next term may have, not the lead; no handover under way): the controls list the
    /// members with it, and the lead hands over only so.
    pub fn hand_to(&self, io: &dyn Io, to: &str) -> Result<(), String> {
        let Some(t) = &self.cur.lead else { return Err(format!("term {} can't be read whole", self.cur.term)) };
        if to == t.member {
            return Err(format!("it leads term {}", t.term));
        }
        if self.lead.as_ref().is_some_and(|l| l.hand != Handover::Leading) {
            return Err("a handover is under way".into());
        }
        let b = Beat::read(io, to).map_err(|e| format!("its heartbeat can't be read: {e:#}"))?.ok_or("no heartbeat")?;
        let now = io.now();
        if b.out_of_touch(now) {
            return Err(format!("out of touch: its beat {} s old", now.saturating_sub(b.beat)));
        }
        if !term::app_at_least(&b.app, &t.app) {
            return Err(format!("its app {} is older than term {}'s {}", b.app, t.term, t.app));
        }
        Ok(())
    }

    /// The listings it asks for, one at a time: of every day, a take-up's (asked again until one
    /// is back), then daily (`RELIST_S`); and of the last days every `SWEEP_S`.
    fn listings(&mut self, io: &dyn Io, out: &mut Out) {
        let Some(l) = &self.lead else {
            self.due = None;
            return;
        };
        let now = io.now();
        if self.asked.is_some_and(|a| now.abs_diff(a.at) < LISTING_S) {
            return;
        }
        // (An hour ahead of the day: four of a listing's tries.)
        if self.due.is_none() && l.listed_at.is_none_or(|at| now.saturating_sub(at) + 4 * LISTING_S >= RELIST_S) {
            self.due = Some((Some(l.records.horizon.clone()).filter(|h| !h.is_empty()), true));
        }
        if self.due.is_none() && now.abs_diff(self.swept) >= SWEEP_S {
            let since = journal::day(now.saturating_sub(SWEEP_DAYS * 86_400)).filter(|d| *d > l.records.horizon);
            self.due = Some((since.or_else(|| Some(l.records.horizon.clone()).filter(|h| !h.is_empty())), false));
        }
        if let Some((since, full)) = self.due.take() {
            self.asks += 1;
            self.asked = Some(Asked { n: self.asks, at: now, full });
            self.swept = now;
            out.list = Some(Listing { n: self.asks, since });
        }
    }

    /// The pool's fields of its heartbeat: the term it leads, a handover's state, its answer to
    /// an offer.
    fn beat(&self, io: &dyn Io, able: bool) -> Beat {
        let (leads, handing_to) = match (&self.lead, &self.passing) {
            (Some(l), _) => (Some(l.term.term), l.hand.handing_to(l.term.term)),
            (None, Some(p)) => (None, p.hand.handing_to(p.own.term)),
            _ => (None, None),
        };
        let mut ready_for = None;
        if let (None, None, Some(t)) = (&self.lead, &self.passing, &self.cur.lead) {
            if t.member != self.me.id {
                if let Ok(Some(b)) = Beat::read(io, &t.member) {
                    ready_for = handover::ready_for(&b, &self.me.id, able && term::app_at_least(&self.me.app, &t.app));
                }
            }
        }
        let stood_down = self.saved.stood_down.filter(|&e| e == self.cur.term && self.lead.is_none());
        Beat { member: self.me.id.clone(), host: self.me.host.clone(), app: self.me.app.clone(), beat: io.now(), leads, handing_to, ready_for, stood_down, addresses: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    //! The driver, scripted on the in-memory NAS: Macs with their own clocks, stepping in turn.
    use super::super::journal::LeaseId;
    use super::super::nas::{Created, Mem};
    use super::*;
    use crate::handoff::Handoff;
    use anyhow::Result;
    use std::cell::Cell;

    const T0: u64 = 1_791_300_000;
    const V1: &str = "20261005-2202-61eb22c";
    const V2: &str = "20261006-0900-1a2b3c4";
    const A: &str = "m-000000000000000a";
    const B: &str = "m-000000000000000b";
    const C: &str = "m-000000000000000c";

    /// One Mac's view of the shared NAS, with its own clocks; while `down`, its reads of paths
    /// holding `fail` (of every path, when None) fail: the share doesn't answer them; and its whole
    /// writes of paths holding `wfail` fail.
    struct Mac<'a> {
        mem: &'a Mem,
        wall: Cell<u64>,
        awake: Cell<u64>,
        down: Cell<bool>,
        fail: std::cell::RefCell<Option<String>>,
        wfail: std::cell::RefCell<Option<String>>,
    }

    impl<'a> Mac<'a> {
        fn new(mem: &'a Mem) -> Mac<'a> {
            Mac { mem, wall: Cell::new(T0), awake: Cell::new(1_000), down: Cell::new(false), fail: Default::default(), wfail: Default::default() }
        }

        /// Time passes, awake.
        fn pass(&self, s: u64) {
            self.wall.set(self.wall.get() + s);
            self.awake.set(self.awake.get() + s);
        }
    }

    impl Nas for Mac<'_> {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            self.mem.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            if self.wfail.borrow().as_deref().is_some_and(|f| path.contains(f)) {
                anyhow::bail!("write {path}: the share doesn't answer");
            }
            self.mem.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            if self.down.get() && self.fail.borrow().as_deref().is_none_or(|f| path.contains(f)) {
                anyhow::bail!("read {path}: the share doesn't answer");
            }
            self.mem.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.mem.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.mem.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.mem.remove(path)
        }
    }

    impl Io for Mac<'_> {
        fn now(&self) -> u64 {
            self.wall.get()
        }
        fn awake(&self) -> u64 {
            self.awake.get()
        }
    }

    /// A Mac on a share under load: each create, read, stat and whole write takes `.1` s, awake.
    struct Slow<'a>(Mac<'a>, Cell<u64>);

    impl Nas for Slow<'_> {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            self.0.pass(self.1.get());
            self.0.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.0.pass(self.1.get());
            self.0.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.0.pass(self.1.get());
            self.0.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.0.pass(self.1.get());
            self.0.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.0.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.0.remove(path)
        }
    }

    impl Io for Slow<'_> {
        fn now(&self) -> u64 {
            self.0.now()
        }
        fn awake(&self) -> u64 {
            self.0.awake()
        }
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    fn member(id: &str, host: &str, app: &str) -> Member {
        Member { id: id.into(), host: host.into(), app: app.into() }
    }

    /// A step, its heartbeat written as the agent will.
    fn step(d: &mut Driver, io: &Mac, heard: Heard) -> Out {
        let out = d.step(io, heard, &any);
        let mut b = out.beat.clone();
        b.beat = io.now();
        b.write(io).unwrap();
        out
    }

    fn able() -> Heard {
        Heard { able: true, members: vec![A.into(), B.into(), C.into()], ..Default::default() }
    }

    fn asks(a: Ask) -> Heard {
        Heard { asks: vec![a], ..able() }
    }

    fn setup(mem: &Mem) {
        mem.write_whole("state/build/writer", b"Mac-mini\n").unwrap();
        mem.write_whole("state/build/manifest.json", b"{}").unwrap();
    }

    fn entry(member: &str, term: u64, n: u64, t: &str) -> Entry {
        let h = Handoff { changes: [(format!("base/{t}"), Some(format!("base/{t}.k{n}.base")))].into(), done: Some(("unit".into(), vec![(t.into(), format!("k{n}"))])), ..Default::default() };
        Entry { member: member.into(), lease: LeaseId { term, n }, step: "unit".into(), handoff: h, at: T0 + n }
    }

    /// A hands over to B; B answers, A settles and passes: term 2 names B.
    fn hand_over(a: &mut Driver, ia: &Mac, b: &mut Driver, ib: &Mac) {
        ia.pass(5);
        let o = step(a, ia, asks(Ask::HandTo(B.into())));
        assert!(o.events.iter().any(|e| matches!(e, Event::Handover { what: "offered", .. })), "{:?}", o.events);
        ib.pass(5);
        step(b, ib, able());
        ia.pass(5);
        assert!(step(a, ia, able()).settle);
        ia.pass(5);
        let o = step(a, ia, Heard { settled: Some(serde_json::json!({"leases": []})), ..able() });
        assert!(o.events.iter().any(|e| matches!(e, Event::Handover { what: "passed", .. })), "{:?}", o.events);
        assert_eq!(o.leads, None);
    }

    #[test]
    fn a_lead_restarted_into_a_development_build_stands_down() {
        // Another member takes over unforced; the owner's downgrade gives the development build the
        // lead again. (Review 2's first scenario.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        ia.pass(20);
        ib.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        let o = step(&mut a, &ia, able());
        assert_eq!((o.leads, o.beat.stood_down), (None, Some(1)), "{:?}", o.events);
        assert!(a.takeover(&ia).downgrade.is_some_and(|w| w.contains("older than term 1's")));
        ib.pass(20);
        assert!(b.takeover(&ib) == Takeover::default(), "the lead stood down: no force");
        assert_eq!(step(&mut b, &ib, asks(Ask::TakeOver { force: false, downgrade: false })).leads, Some(2));
        ia.pass(20);
        assert!(step(&mut a, &ia, asks(Ask::TakeOver { force: true, downgrade: false })).events.iter().any(|e| matches!(e, Event::Waits { what: "take over", why } if why.contains("older"))));
        let o = step(&mut a, &ia, asks(Ask::TakeOver { force: true, downgrade: true }));
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
    }

    /// A hands over to B, on a newer app; B never steps again: A takes back, restarted or not.
    fn handover_to_newer(restart: bool) -> Out {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        hand_over(&mut a, &ia, &mut b, &ib);
        let mut a = if restart { Driver::unlocked(member(A, "Mac-mini", V1), a.saved()) } else { a };
        let mut last = Out::default();
        for _ in 0..10 {
            ia.pass(20);
            last = step(&mut a, &ia, able());
            if last.leads.is_some() {
                break;
            }
        }
        last
    }

    #[test]
    fn a_handover_to_a_newer_app_not_taken_up_is_taken_back() {
        // (Review 2's second and third scenarios.)
        assert_eq!(handover_to_newer(false).leads, Some(3));
        assert_eq!(handover_to_newer(true).leads, Some(3));
    }

    #[test]
    fn the_owners_forced_downgrade_replaces_a_handover_waiting_for_its_target() {
        // A hands over to B, B's lid closes, and A restarts into a development build: the app rule
        // refuses its take-back. (Review N1: it was refused every step, and the owner's forced
        // downgrade with it, "this Mac leads, or hands over"; no lead until B came back.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        hand_over(&mut a, &ia, &mut b, &ib);
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        // Asked at once (between its steps, as the controls ask): the owner's force drops the
        // handover waiting.
        ia.pass(5);
        step(&mut a, &ia, able());
        let needs = a.takeover(&ia);
        assert!(needs.force.as_ref().is_some_and(|w| w.contains("handover")) && needs.downgrade.is_some(), "{needs:?}");
        let mut forced = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        let o = step(&mut forced, &ia, asks(Ask::TakeOver { force: true, downgrade: true }));
        assert!(o.events.iter().any(|e| matches!(e, Event::Handover { what: "dropped", .. })), "{:?}", o.events);
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
        // Or left to wait: the take-back, refused, drops it, and the owner's downgrade follows.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        hand_over(&mut a, &ia, &mut b, &ib);
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        let mut dropped = false;
        for _ in 0..8 {
            ia.pass(30);
            dropped |= step(&mut a, &ia, able()).events.iter().any(|e| matches!(e, Event::Handover { what: "dropped", .. }));
        }
        assert!(dropped);
        ia.pass(30);
        assert_eq!(step(&mut a, &ia, asks(Ask::TakeOver { force: true, downgrade: true })).leads, Some(3));
    }

    #[test]
    fn a_new_lead_is_caught_up_once_its_listing_is_merged() {
        // After a takeover the lead's records lack an entry only a listing finds (its member gone,
        // never told of it). (Review N2: its duties and `fresh` were all the agent had, and both
        // were true before the listing came back.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        let x = journal::write(&mem, &entry(C, 1, 7, "6-1-1")).unwrap();
        ib.pass(700);
        let o = step(&mut b, &ib, asks(Ask::TakeOver { force: false, downgrade: false }));
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.duties && o.fresh && !o.caught_up, "fresh, not whole");
        let list = o.list.expect("a listing asked for");
        assert_eq!(list.since, None, "every day");
        assert!(!Records::load(&mem, 2).unwrap().unwrap().handles(&x));
        ib.pass(20);
        assert!(!step(&mut b, &ib, able()).caught_up, "not before it's back");
        // An older ask's listing doesn't count; this one's does.
        ib.pass(20);
        let o = step(&mut b, &ib, Heard { listed: Some(Listed { n: list.n + 7, keys: Vec::new() }), ..able() });
        assert!(!o.caught_up);
        ib.pass(20);
        let o = step(&mut b, &ib, Heard { listed: Some(Listed { n: list.n, keys: journal::list(&mem, None).unwrap() }), ..able() });
        assert!(o.caught_up, "{:?}", o.events);
        assert!(Records::load(&mem, 2).unwrap().unwrap().handles(&x));
    }

    #[test]
    fn a_lead_whose_saved_state_is_lost_re_asserts() {
        // (Review N4: it took its own term up again, and a lease `<term>-<n>` it had granted could
        // be granted twice.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Made { how, .. } if how.contains("saved state lost"))));
        // With its saved state: a restart re-asserts too, saying so.
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), a.saved());
        assert!(step(&mut a, &ia, able()).events.iter().any(|e| matches!(e, Event::Made { term: 3, how } if how.starts_with("restarted"))));
        // Lost again, onto a development build: it stands down; restarted onto its app, with the
        // state that process saved, it still doesn't take term 3 up again.
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).beat.stood_down, Some(3));
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), a.saved());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(4));
    }

    #[test]
    fn another_members_saved_state_counts_for_nothing() {
        // A's folder copied to C's Mac, A's saved state with it. (Review N5: C took A's entries as
        // its own, and told the lead of them.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ic) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ia, Heard { entries: vec![entry(A, 1, 1, "6-1-1")], ..able() });
        assert_eq!(a.saved().member, A);
        let mut c = Driver::unlocked(member(C, "Mac-mini-2", V1), a.saved());
        let o = step(&mut c, &ic, able());
        assert!(c.mine().to_tell(1).is_empty() && o.send.is_empty(), "{:?}", o.send);
        assert_eq!(c.saved().member, C);
    }

    #[test]
    fn a_listed_entry_not_readable_yet_is_kept_and_read_again() {
        // A listing names an entry its next read doesn't see yet (a stale read): it lands a moment
        // later, and no member will tell of it. (Review N6: dropped, until the next listing.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.expect("its take-up's listing");
        let e = entry(C, 1, 9, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: vec![key.clone()] }), ..able() });
        assert!(!o.caught_up, "an entry listed waits");
        journal::write(&mem, &e).unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert!(Records::load(&mem, 1).unwrap().unwrap().handles(&key) && o.caught_up, "{:?}", o.events);
    }

    #[test]
    fn an_entry_never_read_whole_is_refused_after_an_hour() {
        // B tells the lead of an entry whose file the share cut short for good (a NAS that lost a
        // renamed file's last bytes). Read again every step, it kept the lead from being caught up
        // for ever: no catalog, no sweep.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.expect("its take-up's listing");
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }).caught_up);
        let key = entry(B, 1, 3, "6-1-1").key().unwrap();
        mem.write_whole(&journal::path(&key), b"{\"member\":").unwrap();
        ia.pass(20);
        assert!(!step(&mut a, &ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() }).caught_up, "told of it, not whole");
        for _ in 0..11 {
            ia.pass(300);
            assert!(!step(&mut a, &ia, able()).caught_up, "55 minutes: read again");
        }
        ia.pass(300);
        let o = step(&mut a, &ia, able());
        assert!(o.caught_up, "{:?}", o.events);
        let why = Records::load(&mem, 1).unwrap().unwrap().rejected.get(&key).cloned();
        assert!(why.as_deref().is_some_and(|w| w.contains("not read whole")), "{why:?}");
        assert_eq!(journal::refusal(&mem, &key).unwrap(), why, "noted for the owner");
        assert!(o.send.iter().any(|(to, m)| to == B && matches!(m, Msg::Ack { keys, .. } if keys.contains(&key))), "{:?}", o.send);
    }

    #[test]
    fn a_lost_member_files_unwritten_hand_offs_are_kept() {
        // This Mac's member file lost, a new id made (D), its saved state A's: a hand-off its
        // jobs left unwritten is written and merged, the same bytes; what A led counts for nothing.
        // (Re-review F5.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let e = entry(A, 1, 4, "6-1-1");
        let key = e.key().unwrap();
        let mut saved = Saved { member: A.into(), led: 3, stood_down: Some(3), ..Default::default() };
        saved.mine.add(e).unwrap();
        let d = "m-000000000000000d";
        let mut dr = Driver::unlocked(member(d, "Mac-mini", V1), saved);
        let s = dr.saved();
        assert_eq!((s.member.as_str(), s.led, s.stood_down, s.mine.unwritten().count()), (d, 0, None, 1));
        assert_eq!(step(&mut dr, &ia, able()).leads, Some(1));
        assert!(matches!(journal::read(&mem, &key).unwrap(), journal::Read::Entry(_)), "written");
        assert!(Records::load(&mem, 1).unwrap().unwrap().handles(&key), "merged");
    }

    #[test]
    fn a_re_assertion_keeps_the_refusals_it_has_yet_to_note() {
        // A refuses an entry B told it of, and its save of the records naming the refusal fails;
        // it re-asserts before a sweep, its take-up saving them: the refusal is noted for the
        // owner. (Re-review F6: dropped with what the re-assertion didn't keep.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: vec![] }), ..able() });
        let key = entry(B, 1, 3, "6-1-1").key().unwrap();
        mem.write_whole(&journal::path(&key), b"{\"not\": \"an entry\"}").unwrap();
        *ia.wfail.borrow_mut() = Some("records".into());
        ia.pass(20);
        step(&mut a, &ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() });
        assert_eq!(journal::refusal(&mem, &key).unwrap(), None, "its records not saved");
        *ia.wfail.borrow_mut() = None;
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { reassert: true, ..able() });
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(journal::refusal(&mem, &key).unwrap().is_some_and(|w| w.contains("isn't an entry")), "noted");
    }

    #[test]
    fn a_driver_another_process_took_the_lock_from_stops() {
        // Its lock's file removed: taken again while no other process holds it; once a second
        // process of the member has it, this one stops, leading nothing and writing nothing more.
        // (Re-review F2.)
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("pool-{A}.lock"));
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default(), MemberLock::take(dir.path(), A).unwrap().unwrap());
        let o = step(&mut a, &ia, able());
        assert_eq!((o.leads, o.stop), (Some(1), None));
        std::fs::remove_file(&path).unwrap();
        ia.pass(20);
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1), "taken again: no other process");
        std::fs::remove_file(&path).unwrap();
        let second = MemberLock::take(dir.path(), A).unwrap().expect("a second process");
        ia.pass(20);
        let before = mem.0.borrow().clone();
        let o = a.step(&ia, able(), &any);
        assert!(o.stop.is_some() && o.leads.is_none() && o.send.is_empty(), "{o:?}");
        assert!(o.events.iter().any(|e| matches!(e, Event::SteppedDown { term: 1, .. })), "{:?}", o.events);
        ia.pass(20);
        assert!(a.step(&ia, able(), &any).stop.is_some(), "for good");
        assert_eq!(*mem.0.borrow(), before, "nothing written");
        drop(second);
    }

    #[test]
    fn a_lead_asleep_or_gone_is_taken_over_only_at_the_owners_ask() {
        // A leads, then sleeps (or is gone): B and C see its heartbeat out of touch for half an
        // hour and take nothing over by themselves (pool.md §14: it may come back); the owner's
        // ask does. (Re-review M21.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib, ic) = (Mac::new(&mem), Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let mut c = Driver::unlocked(member(C, "iMac", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        for _ in 0..90 {
            for (d, io) in [(&mut b, &ib), (&mut c, &ic)] {
                io.pass(20);
                assert_eq!(step(d, io, able()).leads, None);
            }
        }
        assert!(term::read(&mem, 2).unwrap().is_none(), "nothing taken over by itself");
        ib.pass(20);
        assert_eq!(step(&mut b, &ib, asks(Ask::TakeOver { force: false, downgrade: false })).leads, Some(2));
    }

    #[test]
    fn a_stand_down_from_an_earlier_term_isnt_this_ones() {
        // A stood down from term 1, then led term 2 by the owner's downgrade and slept before its
        // heartbeat said so: C sees it say it stood down from term 1, not term 2, and takes nothing
        // over by itself. (Re-review M17.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ic) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut c = Driver::unlocked(member(C, "iMac", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut c, &ic, able());
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        ia.pass(20);
        assert_eq!(step(&mut a, &ia, able()).beat.stood_down, Some(1));
        ia.pass(20);
        let o = a.step(&ia, asks(Ask::TakeOver { force: true, downgrade: true }), &any);
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        for _ in 0..20 {
            ic.pass(20);
            assert_eq!(step(&mut c, &ic, able()).leads, None);
        }
        assert!(term::read(&mem, 3).unwrap().is_none(), "nothing taken over by itself");
    }

    #[test]
    fn a_member_lets_those_ranked_before_it_try_first() {
        // A stands down; B and C can take over, B first (the same app, the lower member id), and B
        // can't lead now: C waits its turn, `AUTO_RANK_S` after the two minutes. (Re-review M22.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib, ic) = (Mac::new(&mem), Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let mut c = Driver::unlocked(member(C, "iMac", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        step(&mut c, &ic, able());
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        ia.pass(10);
        assert_eq!(step(&mut a, &ia, able()).beat.stood_down, Some(1));
        let (mut seen, mut took) = (None, None);
        for _ in 0..30 {
            ib.pass(10);
            step(&mut b, &ib, Heard { able: false, ..able() });
            ic.pass(10);
            let o = step(&mut c, &ic, able());
            seen.get_or_insert(ic.now());
            if o.leads.is_some() {
                took = Some(ic.now() - seen.unwrap());
                break;
            }
            ia.pass(20);
            step(&mut a, &ia, able());
        }
        let took = took.expect("taken over");
        assert!((STOOD_DOWN_S + AUTO_RANK_S..STOOD_DOWN_S + AUTO_RANK_S + 20).contains(&took), "{took} s after it saw the stand-down");
    }

    #[test]
    fn a_step_reads_entries_to_merge_for_a_minute_at_most() {
        // B tells the lead of twenty entries on a share taking five seconds an operation: a step
        // reads them for a minute at most, leaving the rest to the next ones. (Re-review M20.)
        let mem = Mem::default();
        setup(&mem);
        let io = Slow(Mac::new(&mem), Cell::new(0));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = a.step(&io, able(), &any).list.unwrap();
        io.0.pass(20);
        a.step(&io, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }, &any);
        let keys: Vec<String> = (1..=20).map(|n| journal::write(&mem, &entry(B, 1, n, "6-1-1")).unwrap()).collect();
        io.1.set(5);
        io.0.pass(20);
        let start = io.0.awake();
        let o = a.step(&io, Heard { msgs: vec![(B.into(), Msg::Tell(keys.clone()))], ..able() }, &any);
        let merged = |m: &Mem| keys.iter().filter(|k| Records::load(m, 1).unwrap().unwrap().handles(k)).count();
        let first = merged(&mem);
        assert!(first > 0 && first < 20 && !o.caught_up, "{first} merged in one step");
        assert!(io.0.awake() - start < BUSY_S + 60, "{} s", io.0.awake() - start);
        for _ in 0..10 {
            io.0.pass(20);
            a.step(&io, able(), &any);
        }
        assert_eq!(merged(&mem), 20);
    }

    #[test]
    fn rr_the_hour_rule_and_a_share_that_doesnt_answer() {
        // B tells A of an entry; A's reads of it fail for two hours (the share doesn't answer
        // them): not refused; then read whole, applied. (The re-review's.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: vec![] }), ..able() }).caught_up);
        let key = journal::write(&mem, &entry(B, 1, 3, "6-1-1")).unwrap();
        *ia.fail.borrow_mut() = Some(key.clone());
        ia.down.set(true);
        ia.pass(20);
        step(&mut a, &ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() });
        for _ in 0..24 {
            ia.pass(300);
            assert!(!step(&mut a, &ia, able()).caught_up);
        }
        ia.down.set(false);
        ia.pass(20);
        assert!(step(&mut a, &ia, able()).caught_up);
        let r = Records::load(&mem, 1).unwrap().unwrap();
        assert!(r.reflected.contains(&key), "applied, not refused: {:?}", r.rejected);
    }

    #[test]
    fn rr_the_hour_rule_counts_an_outage_after_a_first_read_not_whole() {
        // The entry reads not whole once (not there yet as A reads it), then the share doesn't
        // answer A's reads of it for 70 minutes. (Re-review F1: refused at minute 60, on time
        // alone, though it was whole on the share all along: an hour's reads that answer count,
        // and a read that fails starts the hour again.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: vec![] }), ..able() });
        let e = entry(B, 1, 3, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() });
        journal::write(&mem, &e).unwrap();
        *ia.fail.borrow_mut() = Some(key.clone());
        ia.down.set(true);
        for _ in 0..70 {
            ia.pass(60);
            step(&mut a, &ia, able());
            assert!(!Records::load(&mem, 1).unwrap().unwrap().rejected.contains_key(&key), "refused while every read of it failed");
        }
        ia.down.set(false);
        ia.pass(60);
        assert!(step(&mut a, &ia, able()).caught_up);
        assert!(Records::load(&mem, 1).unwrap().unwrap().reflected.contains(&key), "applied once the share answers");
    }

    #[test]
    fn rr_caught_up_across_re_assertions_misses_an_old_days_entry_written_after_its_listing() {
        // A's take-up listing is merged; then C (gone since, never telling) writes an entry of a
        // day three days back (kept unwritten while its Mac was away, its key fixed by its first
        // try). A's sweeps list the last two days, and its re-assertions before a sweep keep what
        // it knew. (Re-review F3: fresh and caught up for days, the entry not merged.) A lists
        // every day again daily: the entry is merged within the day, and A is caught up only by a
        // listing less than a day old.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: journal::list(&mem, None).unwrap() }), ..able() }).caught_up);
        let mut old = entry(C, 1, 9, "6-1-2");
        old.at = T0 - 3 * 86_400;
        let key = journal::write(&mem, &old).unwrap();
        let (mut sweeps, mut merged) = (0, None);
        for i in 0..(3 * 24 * 6) {
            ia.pass(600);
            let o = step(&mut a, &ia, Heard { reassert: i % 36 == 35, ..able() });
            assert!(!o.caught_up || o.listed_at.is_some_and(|at| ia.now() - at < RELIST_S), "{:?}", o.listed_at);
            if o.fresh && o.caught_up {
                sweeps += 1;
            }
            if let Some(l) = &o.list {
                ia.pass(20);
                step(&mut a, &ia, Heard { listed: Some(Listed { n: l.n, keys: journal::list(&mem, l.since.as_deref()).unwrap() }), ..able() });
            }
            if merged.is_none() && Records::newest(&mem, a.leads().unwrap()).unwrap().unwrap().handles(&key) {
                merged = Some(i);
            }
        }
        assert!(sweeps >= 10, "{sweeps}");
        assert!(merged.is_some_and(|i| i < 6 * 24), "merged within a day: {merged:?}");
    }

    #[test]
    fn a_lead_lists_every_day_and_is_caught_up_only_by_a_listing_less_than_a_day_old() {
        // Its take-up's listing merged; the daily one asked for an hour ahead and never handed
        // back: not caught up once its listing is a day old; asked again, caught up once that's
        // merged. (Re-review F3.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        let at = ia.now();
        ia.pass(20);
        assert_eq!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }).listed_at, Some(at));
        let mut daily = None;
        while ia.now() < at + RELIST_S {
            ia.pass(300);
            let o = step(&mut a, &ia, able());
            assert_eq!(o.caught_up, ia.now() < at + RELIST_S, "{} s after its listing", ia.now() - at);
            if o.list.as_ref().is_some_and(|l| l.since.is_none()) {
                daily.get_or_insert(ia.now() - at);
            }
        }
        let when = daily.expect("asked for");
        assert!(when + 3600 >= RELIST_S && when < RELIST_S, "an hour ahead: {when}");
        let mut again = None;
        for _ in 0..6 {
            ia.pass(300);
            if let Some(l) = step(&mut a, &ia, able()).list.filter(|l| l.since.is_none()) {
                again = Some(l);
            }
        }
        let l = again.expect("asked again");
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: l.n, keys: Vec::new() }), ..able() }).caught_up);
    }

    #[test]
    fn a_re_assertion_keeps_what_its_lead_knew_of_the_journal() {
        // Before a sweep the agent asks the lead to re-assert: a sweep needs both `fresh` and
        // `caught_up`, and a re-assertion's take-up started its knowledge again, so the two never
        // met. After a sleep, what members told it meanwhile was lost: it lists the journal again.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.expect("its take-up's listing");
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }).caught_up);
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { reassert: true, ..able() });
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.fresh && o.caught_up, "a sweep's step: {:?}", o.events);
        assert!(o.list.as_ref().is_none_or(|l| l.since.is_some()), "no listing of every day: {:?}", o.list);
        // Asleep an hour: re-asserted, not caught up until its new listing is merged.
        ia.wall.set(ia.wall.get() + 3600);
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
        assert!(!o.caught_up);
        let list = o.list.expect("a listing of every day");
        assert_eq!(list.since, None);
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }).caught_up);
    }

    #[test]
    fn a_note_of_a_stale_leads_refusal_holds_no_lead() {
        // (Review 2's last scenario: the current lead's own check decides.)
        let mem = Mem::default();
        setup(&mem);
        let x = journal::write(&mem, &entry(C, 1, 7, "6-1-1")).unwrap();
        journal::note_refusal(&mem, &x, "a stale lead's why").unwrap();
        let mut r = Records { term: 2, ..Default::default() };
        assert_eq!(records::merge(&mem, &mut r, std::slice::from_ref(&x), &any).applied, std::slice::from_ref(&x));
        assert!(journal::list(&mem, None).unwrap().contains(&x));
    }

    #[test]
    fn a_lead_that_stood_down_is_taken_over_by_a_member_the_app_rule_lets() {
        // A, on V2, restarts into a development build and stands down. B's app is older than term
        // 1's: it can't lead it. C's is new enough: after seeing the stand-down for two minutes it
        // takes over by itself (pool.md §14: a lead that stood down isn't coming back on its own).
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib, ic) = (Mac::new(&mem), Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let mut c = Driver::unlocked(member(C, "iMac", V2), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        step(&mut c, &ic, able());
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        ia.pass(20);
        assert_eq!(step(&mut a, &ia, able()).beat.stood_down, Some(1));
        let mut took = None;
        for i in 0..12 {
            for (d, io) in [(&mut b, &ib), (&mut c, &ic)] {
                io.pass(20);
                if let Some(e) = step(d, io, able()).leads {
                    took.get_or_insert((d.member().id.clone(), e, i));
                }
            }
            ia.pass(20);
            step(&mut a, &ia, able());
        }
        let (who, e, i) = took.expect("taken over");
        assert_eq!((who.as_str(), e), (C, 2));
        assert!((6..=8).contains(&i), "after two minutes, not before: round {i}");
        let t = term::read(&mem, 2).unwrap().unwrap();
        assert_eq!(t.how, "taken over by iMac: Mac-mini stood down");
        // Two that can: the newest app first, then the lowest member id; the create decides.
        let one = member(B, "MacBook-Air", V2);
        let two = member(C, "iMac", V2);
        let newer = member(A, "Mac-mini", "20261007-0000-0000000");
        let mut v = [two.clone(), newer.clone(), one.clone()];
        v.sort_by(first_to_try);
        assert_eq!(v.map(|m| m.id), [newer.id, one.id, two.id]);
    }

    #[test]
    fn the_controls_ask_what_a_takeover_needs_and_whom_the_lead_can_be_handed_to() {
        // (Review N7: the menu, the pages and `scenic lead` would each check it again.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib, ic) = (Mac::new(&mem), Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        let mut c = Driver::unlocked(member(C, "iMac", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        step(&mut c, &ic, able());
        assert_eq!(a.hand_to(&ia, B), Ok(()));
        assert!(a.hand_to(&ia, C).unwrap_err().contains("older than term 1's"));
        assert!(a.hand_to(&ia, A).unwrap_err().contains("leads term 1"));
        assert_eq!(a.hand_to(&ia, "m-000000000000000d"), Err("no heartbeat".to_string()));
        assert_eq!(b.hand_to(&ib, B), Ok(()), "asked of another member: the same answer");
        assert!(a.takeover(&ia).refused.is_some());
        let t = b.takeover(&ib);
        assert!(t.force.is_some_and(|w| w.contains("in touch")) && t.downgrade.is_none() && t.refused.is_none());
        let t = c.takeover(&ic);
        assert!(t.force.is_some() && t.downgrade.is_some_and(|w| w.contains("older than term 1's")));
        // A out of touch: nothing needed from B.
        ib.pass(700);
        assert_eq!(b.takeover(&ib), Takeover::default());
        // A handover under way: no other.
        ia.pass(5);
        step(&mut a, &ia, asks(Ask::HandTo(B.into())));
        assert_eq!(a.hand_to(&ia, B), Err("a handover is under way".to_string()));
    }

    #[test]
    fn a_long_step_is_a_sleep_or_a_stall_not_slow_operations() {
        // A step slowed by the share (every operation seconds long) doesn't make its lead
        // re-assert; one it slept through, or that stalled five minutes, does. (Review N3.)
        let mem = Mem::default();
        setup(&mem);
        let io = Slow(Mac::new(&mem), Cell::new(0));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let leads = |d: &mut Driver, h: Heard| {
            let o = d.step(&io, h, &any);
            (o.leads, o.events)
        };
        assert_eq!(leads(&mut a, able()).0, Some(1));
        io.1.set(15);
        for _ in 0..3 {
            io.0.pass(20);
            let (l, ev) = leads(&mut a, Heard { entries: vec![entry(A, 1, io.0.now() % 1000, "6-1-1")], ..able() });
            assert_eq!(l, Some(1), "{ev:?}");
        }
        // A stall: one operation of six minutes.
        io.1.set(0);
        io.0.pass(20);
        let stall = Slow(Mac { wall: Cell::new(io.0.now()), awake: Cell::new(io.0.awake()), ..Mac::new(&mem) }, Cell::new(360));
        a.step(&stall, able(), &any);
        stall.1.set(0);
        stall.0.pass(20);
        assert!(a.step(&stall, able(), &any).events.iter().any(|e| matches!(e, Event::Made { how, .. } if how == "re-asserted after a long step")));
    }
}
