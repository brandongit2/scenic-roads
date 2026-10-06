//! The pool's driver (docs/pool.md §6, §12): what a member is in the pool and what it does as that,
//! decided one step at a time, the same in the agent and in the simulator (crate::pool's `sim`),
//! which is what checks that it keeps §4's invariants and makes progress whatever the share's
//! faults, the Macs' sleeps and stale reads.
//!
//! # The contract
//!
//! The agent makes one `Driver` per process (`Driver::new`, from the state it saved last: `saved`)
//! and calls `Driver::step` once per loop of its own, about every 20 s, from one thread. A step:
//!
//! - **does its NAS operations through `Io`** (crate::pool::nas::Nas, and two clocks): a few
//!   reads and stats, the records' save (about 3 MB), a term made now and then, and its jobs'
//!   entries written and entries merged for `BUSY_S` at most (the rest in the next steps); a
//!   listing of the terms only at its first step (when the lead's hint doesn't name the current
//!   term); never a listing of the journal, never a sleep, no thread of its own. What's slow (a listing of the
//!   journal, 3 to 33 s a folder on the share under load) it asks for in its output (`Out::list`),
//!   for the agent to make off the loop and hand back in a later step (`Heard::listed`);
//! - **reads the clocks itself**, where a decision needs the time, after what it compares it with
//!   (a heartbeat read, then the clock): the agent passes no time in. The awake clock tells it
//!   what the wall clock can't: that the Mac slept, rather than worked;
//! - **takes what was heard since the last step** (`Heard`): the members' messages to it, its
//!   owner's asks (its menu, `scenic lead`, the pages, as its member's API takes them), the
//!   hand-offs of its jobs that ended, the listing it asked for, what settling a handover wrote,
//!   whether this Mac can lead now (its disk, home, power: the agent's conditions);
//! - **gives what to do now** (`Out`): the messages to send, by member id, over the pool's API
//!   (best effort: a message lost is told again or made up for); the pool's fields of this Mac's
//!   heartbeat, to write with the rest of it, stamped as it's written; the term it leads, if it
//!   leads, and whether it may grant jobs and do the lead's duties now (`duties`: plan, publish,
//!   keep the build's state), settle a handover (`settle`: stop granting, cancel its duties in
//!   flight, write the coordinator's state and hand it back in `Heard::settled`), or sweep
//!   (`fresh`: re-asserted this step, as GC needs: ask with `Heard::reassert`); a listing to
//!   make; and what happened (`Event`s: terms taken up, stepped down from, handed
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
//! that changed it. A `Saved` that's lost, or another member's (a copy of the agent's folder),
//! counts for nothing: the driver then re-asserts a term naming it that it finds at its start
//! rather than take it up again (it may have led it, its leases granted), and the state it saves
//! says so, for the processes after it.
//!
//! # Re-asserting, and standing down
//!
//! A lead's view can be old without its knowing (§6.6). It re-asserts (makes the next term naming
//! itself, a create-new no stale read can fool) before acting again when, since its last step or
//! within its last one, it slept or its wall clock moved more than `GAP_S` beyond its awake clock;
//! when its last step ran over `STALL_S` (the NAS stalled: a share under load slows every
//! operation, so a step's length short of that says nothing); after a restart; and when the agent
//! asks (`Heard::reassert`, before a GC sweep). Time spent listing the journal, or waiting between
//! loops, isn't a gap. A lead whose re-assertion the app rule refuses (it restarted into an older
//! app or a development build) stands down, and says so in its heartbeat (`Beat::stood_down`), so
//! another member can take over without forcing it; it takes its term up again once its app is new
//! enough.

use super::beat::Beat;
use super::handover::{self, Do, Handover, Seen};
use super::journal::{self, Entry, Mine};
use super::nas::Nas;
use super::records::{self, Check, Records};
use super::term::{self, Current, Made, Term};
use super::Member;
use serde::{Deserialize, Serialize};
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
    /// an app new enough; another member passes it on to the lead.
    HandTo(String),
    /// Take the lead over (§6.5): when the lead is out of touch or stood down; `force`, also with
    /// the lead in touch, past a term that can't be read, and over this Mac's own handover still
    /// waiting for its target; `downgrade`, also on an older app than the current term's
    /// (crate::pool::term::forced).
    TakeOver { force: bool, downgrade: bool },
}

/// What a member heard since its last step.
#[derive(Clone, Debug, Default)]
pub struct Heard {
    /// The members' messages to it, by sender's member id.
    pub msgs: Vec<(String, Msg)>,
    /// Its owner's asks.
    pub asks: Vec<Ask>,
    /// The hand-offs of its jobs that ended: kept whole (in `Saved`) until written to the journal.
    pub entries: Vec<Entry>,
    /// The listing of the journal it asked for (`Out::list`), its keys, done since; none when the
    /// listing failed (asked for again later).
    pub listed: Option<Vec<String>>,
    /// Settling a handover (`Out::settle`): the coordinator's state as the agent wrote it once it
    /// stopped granting and cancelled its duties in flight; handed over with the records.
    pub settled: Option<serde_json::Value>,
    /// Whether this Mac can lead now if offered the lead (its disk, home and power; the app rule is
    /// the driver's). The agent sets it.
    pub able: bool,
    /// Re-assert before the agent does what only a fresh lead may (a GC sweep): `Out::fresh` says
    /// it did.
    pub reassert: bool,
}

/// A listing of the journal to make, off the loop (crate::pool::journal::list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// The first day to list (YYYY-MM-DD); every day when None.
    pub since: Option<String>,
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
    /// Leading, whether it may grant jobs and do the lead's duties now: not while it settles a
    /// handover, nor while its view may be old (a re-assertion due).
    pub duties: bool,
    /// Leading, settling a handover: grant nothing new, cancel the duties in flight, write the
    /// coordinator's state, and hand it back (`Heard::settled`).
    pub settle: bool,
    /// Leading, it re-asserted (or took its term up) this step: no later term was made before.
    pub fresh: bool,
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

/// A member's part in the pool, one step at a time (see the module's doc: the contract).
#[derive(Debug)]
pub struct Driver {
    me: Member,
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
    /// It must re-assert before acting as lead, and why.
    must: Option<&'static str>,
    /// Its clocks (wall, awake) at its last step's end.
    clocks: Option<(u64, u64)>,
    /// The listing due next, and when the last asked for was (none outstanding when 0).
    due: Option<Listing>,
    asked: Option<u64>,
    swept: u64,
    /// The member that handed it a term, by term: told when it leads it.
    handed_by: BTreeMap<u64, String>,
    /// It just started (a restart): a term it led and names it is re-asserted.
    restarted: bool,
    /// The current term it learnt from the NAS at its first step (none before), not counting a
    /// term 1 its own first step made.
    first: Option<u64>,
}

/// Whether the clocks went `was` → `now` (wall, awake) with more than `GAP_S` unaccounted for: the
/// Mac slept, or its wall clock was set, either way.
fn gap(was: (u64, u64), now: (u64, u64)) -> bool {
    let wall = now.0 as i64 - was.0 as i64;
    let awake = now.1 as i64 - was.1 as i64;
    (wall - awake).unsigned_abs() > GAP_S
}

impl Driver {
    /// The driver of member `me` (its app the process's), from what it saved last: nothing of it
    /// when it's another member's, or names none (lost, or never saved).
    pub fn new(me: Member, saved: Saved) -> Driver {
        let known = saved.member == me.id;
        let saved = if known { saved } else { Saved { member: me.id.clone(), ..Default::default() } };
        let passing = saved.passing.clone().map(|(own, passed, hand)| Passing { own, passed, hand, records: None });
        Driver { me, cur: Current::default(), saved, known, lead: None, passing, taking: None, spare: None, must: None, clocks: None, due: None, asked: None, swept: 0, handed_by: BTreeMap::new(), restarted: true, first: None }
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
        let start = (io.now(), io.awake());
        if self.lead.is_some() {
            if self.clocks.is_some_and(|was| gap(was, start)) {
                self.must = Some("re-asserted after a gap");
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
        }
        self.clocks = Some(end);
        out.term = self.cur.term;
        if let Some(l) = &self.lead {
            out.leads = Some(l.term.term);
            out.duties = l.hand.grants() && self.must.is_none();
            out.settle = matches!(l.hand, Handover::Settling { .. });
            out.fresh &= self.must.is_none();
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
                self.made(io, out, t, made);
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
        self.saved.led = t.term;
        self.saved.stood_down = None;
        self.must = None;
        out.fresh = true;
        out.events.push(Event::TookUp { term: t.term, how: t.how.clone(), handed });
        let horizon = r.horizon.clone();
        self.lead = Some(Lead { term: t.clone(), records: r, hand: Handover::Leading, told: BTreeMap::new(), waiting: BTreeSet::new(), dirty: false, refused: Vec::new() });
        // A take-up lists the journal: every day not forgotten.
        self.due = Some(Listing { since: Some(horizon).filter(|h| !h.is_empty()) });
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
    fn lead_step(&mut self, io: &dyn Io, out: &mut Out, tells: Vec<(String, Vec<String>)>, asks: Vec<String>, listed: Option<Vec<String>>, settled: Option<serde_json::Value>, check: Check, busy: u64) {
        let me = self.me.id.clone();
        let own: Vec<String> = self.saved.mine.to_tell(self.cur.term);
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
            self.asked = None;
            keys.extend(listed.into_iter().filter(|k| !l.records.handles(k)));
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
        l.waiting.extend(m.waiting.iter().chain(&m.unread).cloned());
        l.waiting.retain(|k| !l.records.handles(k) && !m.forgotten.contains(k));
        if l.dirty {
            match l.records.save(io) {
                Ok(()) => {
                    l.dirty = false;
                    for (k, why) in std::mem::take(&mut l.refused) {
                        // (Named refused in the records saved: a note not made now is only the
                        // owner's loss.)
                        if let Err(err) = journal::note_refusal(io, &k, &why) {
                            out.events.push(Event::Failed { what: "note a refusal", why: format!("{k}: {err:#}") });
                        }
                    }
                }
                Err(err) => out.events.push(Event::Failed { what: "save the records", why: format!("term {e}: {err:#}") }),
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
        let settled = (!l.dirty && m.waiting.is_empty() && m.unread.is_empty() && l.records.handed.is_some()).then_some(l.records.seq);
        // An ask, checked (§6.3): a live member, on an app the next term may have, not this Mac.
        let ask = match asks.last() {
            Some(to) if *to != me && l.hand == Handover::Leading => match Beat::read(io, to) {
                Ok(Some(b)) if !b.out_of_touch(io.now()) && term::app_at_least(&b.app, &l.term.app) => Some(to.clone()),
                Ok(b) => {
                    let why = b.map_or("no heartbeat".to_string(), |b| if b.out_of_touch(io.now()) { "out of touch".to_string() } else { format!("its app {} is older than term {e}'s", b.app) });
                    out.events.push(Event::Waits { what: "hand the lead over", why: format!("to {to}: {why}") });
                    None
                }
                Err(err) => {
                    out.events.push(Event::Failed { what: "hand the lead over", why: format!("{err:#}") });
                    None
                }
            },
            _ => None,
        };
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
        if self.lead.is_some() || self.saved.unfinished.is_some() {
            return out.events.push(Event::Waits { what: "take over", why: "this Mac leads, or has a term to finish".into() });
        }
        if self.passing.is_some() && !force {
            return out.events.push(Event::Waits { what: "take over", why: "this Mac hands over: only forced".into() });
        }
        // (A term that can't be read whole yet has a lead not known to be gone: by force only. Its
        // own term, which it stood down from: only the owner's downgrade makes it lead it again.)
        let gone = match &self.cur.lead {
            Some(t) if t.member == self.me.id => self.saved.stood_down == Some(t.term),
            Some(t) => match Beat::read(io, &t.member) {
                Ok(b) => b.is_none_or(|b| b.out_of_touch(io.now()) || b.stood_down == Some(t.term)),
                Err(_) => false,
            },
            None => self.cur.term == 0,
        };
        if !gone && !force {
            return out.events.push(Event::Waits { what: "take over", why: "the lead is in touch: only forced".into() });
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

    /// The listings it asks for: a take-up's, then a sweep of the last days every `SWEEP_S`, one
    /// at a time.
    fn listings(&mut self, io: &dyn Io, out: &mut Out) {
        let Some(l) = &self.lead else {
            self.due = None;
            return;
        };
        let now = io.now();
        if self.asked.is_some_and(|at| now.abs_diff(at) < LISTING_S) {
            return;
        }
        if self.due.is_none() && now.abs_diff(self.swept) >= SWEEP_S {
            let since = journal::day(now.saturating_sub(SWEEP_DAYS * 86_400)).filter(|d| *d > l.records.horizon);
            self.due = Some(Listing { since: since.or_else(|| Some(l.records.horizon.clone()).filter(|h| !h.is_empty())) });
        }
        if let Some(listing) = self.due.take() {
            self.asked = Some(now);
            self.swept = now;
            out.list = Some(listing);
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

    /// One Mac's view of the shared NAS, with its own clocks.
    struct Mac<'a> {
        mem: &'a Mem,
        wall: Cell<u64>,
        awake: Cell<u64>,
    }

    impl<'a> Mac<'a> {
        fn new(mem: &'a Mem) -> Mac<'a> {
            Mac { mem, wall: Cell::new(T0), awake: Cell::new(1_000) }
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
            self.mem.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
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
        Heard { able: true, ..Default::default() }
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
        let mut a = Driver::new(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::new(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        ia.pass(20);
        ib.pass(20);
        let mut a = Driver::new(member(A, "Mac-mini", "development"), a.saved());
        let o = step(&mut a, &ia, able());
        assert_eq!((o.leads, o.beat.stood_down), (None, Some(1)), "{:?}", o.events);
        ib.pass(20);
        assert_eq!(step(&mut b, &ib, asks(Ask::TakeOver { force: false, downgrade: false })).leads, Some(2));
        ia.pass(20);
        let o = step(&mut a, &ia, asks(Ask::TakeOver { force: true, downgrade: true }));
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
    }

    /// A hands over to B, on a newer app; B never steps again: A takes back, restarted or not.
    fn handover_to_newer(restart: bool) -> Out {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::new(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        hand_over(&mut a, &ia, &mut b, &ib);
        let mut a = if restart { Driver::new(member(A, "Mac-mini", V1), a.saved()) } else { a };
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
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::new(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        hand_over(&mut a, &ia, &mut b, &ib);
        let mut a = Driver::new(member(A, "Mac-mini", "development"), a.saved());
        // Asked at once: the owner's force drops the handover waiting.
        ia.pass(5);
        step(&mut a, &ia, able());
        let mut forced = Driver::new(member(A, "Mac-mini", "development"), a.saved());
        let o = step(&mut forced, &ia, asks(Ask::TakeOver { force: true, downgrade: true }));
        assert!(o.events.iter().any(|e| matches!(e, Event::Handover { what: "dropped", .. })), "{:?}", o.events);
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
        // Or left to wait: the take-back, refused, drops it, and the owner's downgrade follows.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::new(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        hand_over(&mut a, &ia, &mut b, &ib);
        let mut a = Driver::new(member(A, "Mac-mini", "development"), a.saved());
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
    fn a_lead_whose_saved_state_is_lost_re_asserts() {
        // (Review N4: it took its own term up again, and a lease `<term>-<n>` it had granted could
        // be granted twice.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        ia.pass(20);
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Made { how, .. } if how.contains("saved state lost"))));
        // With its saved state: a restart re-asserts too, saying so.
        ia.pass(20);
        let mut a = Driver::new(member(A, "Mac-mini", V1), a.saved());
        assert!(step(&mut a, &ia, able()).events.iter().any(|e| matches!(e, Event::Made { term: 3, how } if how.starts_with("restarted"))));
        // Lost again, onto a development build: it stands down; restarted onto its app, with the
        // state that process saved, it still doesn't take term 3 up again.
        ia.pass(20);
        let mut a = Driver::new(member(A, "Mac-mini", "development"), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).beat.stood_down, Some(3));
        ia.pass(20);
        let mut a = Driver::new(member(A, "Mac-mini", V1), a.saved());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(4));
    }

    #[test]
    fn another_members_saved_state_counts_for_nothing() {
        // A's folder copied to C's Mac, A's saved state with it. (Review N5: C took A's entries as
        // its own, and told the lead of them.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ic) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ia, Heard { entries: vec![entry(A, 1, 1, "6-1-1")], ..able() });
        assert_eq!(a.saved().member, A);
        let mut c = Driver::new(member(C, "Mac-mini-2", V1), a.saved());
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
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ia, able()).list.expect("its take-up's listing");
        let e = entry(C, 1, 9, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { listed: Some(vec![key.clone()]), ..able() });
        journal::write(&mem, &e).unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert!(Records::load(&mem, 1).unwrap().unwrap().handles(&key), "{:?}", o.events);
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
    fn a_long_step_is_a_sleep_or_a_stall_not_slow_operations() {
        // A step slowed by the share (every operation seconds long) doesn't make its lead
        // re-assert; one it slept through, or that stalled five minutes, does. (Review N3.)
        let mem = Mem::default();
        setup(&mem);
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
        let io = Slow(Mac::new(&mem), Cell::new(0));
        let mut a = Driver::new(member(A, "Mac-mini", V1), Saved::default());
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
        let stall = Slow(Mac { mem: &mem, wall: Cell::new(io.0.now()), awake: Cell::new(io.0.awake()) }, Cell::new(360));
        a.step(&stall, able(), &any);
        stall.1.set(0);
        stall.0.pass(20);
        assert!(a.step(&stall, able(), &any).events.iter().any(|e| matches!(e, Event::Made { how, .. } if how == "re-asserted after a long step")));
    }
}
