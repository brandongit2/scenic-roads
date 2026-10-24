//! The pool's driver (docs/pool.md §6, §12): what a member is in the pool and what it does as that,
//! decided one step at a time, the same in the agent and in the simulator (crate::pool's `sim`),
//! which is what checks that it keeps §4's invariants and makes progress whatever the share's
//! faults, the Macs' sleeps and stale reads.
//!
//! # The contract
//!
//! **One process per member.** The agent takes its member's lock (crate::pool::MemberLock: a flock
//! named by the member id, in the folder it passes, the app's: so a second process of the member, a
//! second agent or one started from a copy of the agent's folder, can't take it) and makes its one
//! `Driver` with it (`Driver::new`, from the state it saved last: `saved`). A process without the
//! lock steps no driver (it runs dry, as a second agent on one folder does). Every step checks the
//! lock first: its file removed is taken again if it's free; a check that can't tell (the folder
//! unreadable a moment) holds that step's duties, and is tried again next step; a process another
//! holds it from stops for good (`Out::stop`: the agent saves that step's `Saved`, which keeps the
//! hand-offs handed to it, writes no heartbeat, sends nothing, steps no more, and leaves the pool).
//! Two processes of one member would lead one term twice, and number leases `<term>-<n>` twice.
//!
//! The agent steps it about every 20 s, one step at a time: its loop's step, and while that loop is
//! busy (a long plan, a job stopped) its pool's own thread's (crate::agent::pool's `Ticker`), each
//! under the run's lock. A step:
//!
//! - **does its NAS operations through `Io`** (crate::pool::nas::Nas, and two clocks): a few
//!   reads and stats, the records' save (about 3 MB), a term made now and then, and its jobs'
//!   entries written and entries merged for `BUSY_S` at most (the rest in the next steps); a
//!   listing of the terms only at its first step (when the lead's hint names no term that's
//!   there); never a listing of the journal, never a sleep, no thread of its own. What's slow (a
//!   listing of the journal, 3 to 33 s a folder on the share under load) it asks for in its output
//!   (`Out::list`), for the agent to make off the loop and hand back, with the ask's number, in a
//!   later step (`Heard::listed`): every listing asked for, however long it takes, one that fails
//!   made again. It asks for no other while its last is out; one out `OVERDUE_S` with none handed
//!   back meanwhile (lost, or that slow) it says, and asks again, and whichever comes back counts;
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
//!   it back in `Heard::settled`); publish a catalog only while it may do its duties and its
//!   records reflect the journal (`duties` and `caught_up`: a listing of every day merged and
//!   saved, its take-up's or a daily one, asked for under a day ago, `listed_at` saying when;
//!   nothing told or listed waiting to be read; no re-assertion owed; a re-assertion keeping it),
//!   and sweep (GC) only once its records reflect the journal, on a step that re-asserted (`fresh`:
//!   asked with `Heard::reassert`; it says no later term was made before, not that its records are
//!   whole);
//!   a listing to make; and what happened (`Event`s: terms taken up, stepped down from, handed
//!   over; errors), for the history and the log;
//! - **never fails**: an error stops only the duty that met it (said in an `Event::Failed`), and
//!   the step goes on: a lead whose re-assertion fails keeps leading, its duties held, and tries
//!   again next step; one the app rule refuses stands down.
//!
//! What the driver does itself, through `Io`, is what decides safety: making terms
//! (crate::pool::term), taking them up and saving its term's records (crate::pool::records),
//! writing its jobs' entries and merging others' (crate::pool::journal), the handover's transitions
//! (crate::pool::handover). What it leaves to the agent can't break §4's invariants: granting and
//! the coordinator, the merge's checks (`Check`, passed to every step), the duties, the heartbeat's
//! other fields, the messages' transport, and persisting `Saved` (its state between processes,
//! naming its member: its entries, kept whole until written, then until a horizon passes their day)
//! after every step that changed it, **before acting on that step's `Out`** (a take-up's grants,
//! its messages): a crash between would restart from a state that doesn't know the term it took up.
//! **A job's hand-off is kept until a `Saved` from a step it was handed to is on the agent's
//! disk**: before that, a crash loses it. A `Saved` that's lost, or another member's (this Mac's
//! member file lost, a new id made; or a copy of another Mac's folder), counts for nothing but its
//! jobs' hand-offs not written yet: the driver then re-asserts a term naming it that it finds at
//! its start rather than take it up again (it may have led it, its leases granted), and the state
//! it saves says so, for the processes after it. So does an older state of its own (a backup
//! restored) for a term naming it that it made and the state doesn't record (`Saved::made`), or
//! whose records are there already, a process before this one having taken it up (a handover's
//! too), unless this process is taking it up.
//!
//! The controls (the menu, the pages, `scenic lead`) ask the driver what the step would decide:
//! whether a takeover from this Mac needs the owner's force or downgrade, and why (`takeover`);
//! whether the lead can be handed to a member, and why not (`hand_to`).
//!
//! # Re-asserting, standing down, and taking over a lead that stood down
//!
//! A lead's view can be old without its knowing (§6.6). It re-asserts (makes the next term naming
//! itself, a create-new no stale read can fool) before acting again when, since its last step or
//! within its last one, its wall clock and its awake clock moved more than `GAP_S` apart (it slept,
//! or its wall clock was set, either way);
//! when its last step ran over `STALL_S`, or that long awake passed since it (the NAS stalled, or
//! its process was held up: a share under load slows every operation, so a step's length short of
//! that says nothing); after a restart; and when the agent asks (`Heard::reassert`, before a GC
//! sweep). Time spent listing the journal (off the steps) isn't a gap. A re-assertion keeps what the lead knew of the journal (no other lead came
//! between), so a sweep's step is caught up; after a sleep, its members' messages meanwhile lost,
//! it lists the journal again. A lead whose re-assertion the app rule refuses (it restarted into
//! an older app or a development build) stands down, and says so in its heartbeat
//! (`Beat::stood_down`); once its app is new enough it re-asserts its term ("re-asserted: its app
//! new enough"). A member that can lead (`Heard::able`) and has seen the lead's heartbeat stood
//! down for `STOOD_DOWN_S` takes over by itself, if the app rule lets it: the
//! members that can, newest app first, then lowest member id, try `AUTO_RANK_S` apart, and the
//! next term's create-new decides between them. So is a lead gone silent, its heartbeat `SILENT_S`
//! old and watched unchanged for `SILENT_SEEN_S` (`take_over_silent`); a lead whose own heartbeat
//! hasn't landed for `LEAD_BEAT_S` grants nothing (`Out::duties`); back, it re-asserts before it
//! acts, finds the newer term and steps down, its coordinator granting nothing meanwhile (crate::coord's
//! `told`). A current term its reads find unreadable (its create landed and
//! its bytes never did) for `UNREAD_TAKE_S` awake is taken past by a member that can lead, the same
//! turns deciding, as the owner's force would (`term::forced`): only reads that answer short count
//! (one that fails starts the time again), only while every other member's heartbeat reads whole,
//! in touch and not leading the term, and this Mac's own is fresh (failing closed), and the term
//! and heartbeats read again just before (pool.md §6.5).

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
use std::path::Path;

/// More than this unaccounted for, between steps or within one, and a lead re-asserts before it
/// acts (s).
pub const GAP_S: u64 = 60;
/// A step running longer than this, awake, or this long awake between two steps: the NAS stalled,
/// or the process stepping it was held up, and its lead re-asserts (s).
pub const STALL_S: u64 = 300;
/// The longest a read is taken to stay stale (s). Measured (task #645, tools/check/nas-probe.py, two
/// Macs, 24 Oct, idle share, 570 trials): another Mac's rename over a file was read within 1.1 s,
/// a new file opened within 0.12 s; a stat or listing kept a removed file, and a listing missed a
/// new one, up to 33.5 s (macOS's SMB attribute and folder caches, ~30 s). 600 s is 18 times that
/// tail, with room for a share under load (task #630 saw one name stale for 40 minutes: past any
/// such bound, which `MOVED_ON_S` refuses as damage): term 1 with no snapshot a take-up can read whole for this long awake (its lead's, or
/// its first) has a first snapshot that won't be, and the take-up starts from today's files.
pub const STALE_S: u64 = 600;
/// The time a step spends writing its jobs' entries, and reading entries to merge, before it leaves
/// the rest to the next steps (on a share under load each takes seconds) (s).
pub const BUSY_S: u64 = 60;
/// How often a lead lists the journal's last days for entries no member told it of (their member
/// gone, or told a lead that was no longer current, and gone since) (s).
pub const SWEEP_S: u64 = 600;
/// The days back from today a sweep lists (a take-up lists every day not forgotten).
pub const SWEEP_DAYS: u64 = 2;
/// A lead lists every day of the journal not forgotten at least this often (its take-up's listing,
/// then again `AHEAD_S` before this one's a day old), and its records reflect the journal
/// (`Out::caught_up`) only by such a listing begun this recently (s): an entry written late into an
/// old day (kept unwritten while its Mac was away), its member gone before telling of it, waits no
/// longer than this.
pub const RELIST_S: u64 = 86_400;
/// How long before its listing of every day is a day old a lead asks for the next (s): time to make
/// it, on a share under load.
pub const AHEAD_S: u64 = 3600;
/// A lead's last listing asked for, out this long with none handed back meanwhile (the agent lost
/// it, or it's that slow), is overdue: said, and the next asked for, the first kept (s).
pub const OVERDUE_S: u64 = 7200;
/// How long a member sees the lead's heartbeat stood down before it takes over by itself (s).
pub const STOOD_DOWN_S: u64 = 120;
/// The wait between the members that can take over a lead that stood down, by rank (s).
pub const AUTO_RANK_S: u64 = 30;
/// A lead whose heartbeat is this old (s) is silent, and a member that can lead takes over by
/// itself (§6.5): the longer of out of touch (crate::pool::beat::OUT_OF_TOUCH_S) and a lease's
/// life (crate::coord::TTL), so the old lead's leases have lapsed by then, unrenewed; and its
/// coordinator, unstepped past `STALL_S` (crate::coord::Coordinator::set_moving), grants nothing.
pub const SILENT_S: u64 = {
    let ttl = crate::coord::TTL.as_secs();
    if ttl > super::beat::OUT_OF_TOUCH_S {
        ttl
    } else {
        super::beat::OUT_OF_TOUCH_S
    }
};
/// A lead whose own heartbeat, read back from the server, is older than this by its clock grants
/// nothing (s; `Driver::beat_stale`): one heartbeat's interval (`beat::BEAT_EVERY_S`) and three
/// steps, so a beat that lands as it should never trips it.
pub const LEAD_BEAT_S: u64 = super::beat::BEAT_EVERY_S + 3 * crate::agent::pool::TICK_S;
/// How long, awake, a member must have watched the lead's heartbeat unchanged before it takes over
/// (s), whatever the beat's age by its clock (a lead's clock behind makes a fresh beat look old).
/// The lead's grants outlast its last landed beat by at most `LEAD_BEAT_S` (its steps' check) plus
/// `STALL_S` (its coordinator granting on after its last step told it so, the next step hung)
/// plus a step; this is that, plus one heartbeat's interval as margin: by then the lead grants
/// nothing. Past two heartbeats too, so a healthy lead's beat always changes within it.
pub const SILENT_SEEN_S: u64 = LEAD_BEAT_S + STALL_S + crate::agent::pool::TICK_S + super::beat::BEAT_EVERY_S;
const _: () = assert!(SILENT_SEEN_S > LEAD_BEAT_S + STALL_S + crate::agent::pool::TICK_S);
const _: () = assert!(SILENT_SEEN_S > 2 * super::beat::BEAT_EVERY_S + crate::agent::pool::TICK_S);
const _: () = assert!(LEAD_BEAT_S > super::beat::BEAT_EVERY_S + crate::agent::pool::TICK_S);
/// How long, awake, a member finds the current term unreadable (empty or short) before it makes
/// the next past it by itself (s; §6.5): far past any stale read (`STALE_S`) and any create's bytes
/// in flight, so no Mac reads the term whole, and none can lead it but by making a later term.
pub const UNREAD_TAKE_S: u64 = 1800;
/// Taking a term past by itself, this Mac's own heartbeat on the NAS must be this fresh (s): its
/// writes land, so its view of the others' isn't an outage's.
pub const OWN_BEAT_S: u64 = 360;
/// An entry a lead's reads find not whole (short, or not there) over this long awake is refused
/// (s): no stale read lasts so long, and its file won't be whole (cut short on the share, or
/// removed). So is one whose reads fail while the share answers the step's others (a file the
/// share errors on); a read that fails when none answered (the share away) starts the time again.
pub const UNREADABLE_S: u64 = 3600;
/// An entry a member told it of, not read whole over this long awake once an entry that member
/// told it of later is read whole, is refused (s; §7.3): its writer has moved on. The member told
/// of it only once its write returned (renamed into place whole: `Nas::write_whole`), and its later
/// write reached the share and this Mac's reads, so no rename of this one is still to land; what's
/// left is this Mac's SMB client answering from a stale name (task #630: the lead's own entry read
/// as not there for 40 minutes, whole on the share) or the share's damage. Refusing loses only the
/// job's work, done again; the hour held every catalog. Past a stale read's usual `STALE_S`.
pub const MOVED_ON_S: u64 = STALE_S;
/// How long an entry waits to be read whole before its lead says so, once (s).
const WAIT_SAID_S: u64 = 600;

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
    /// A build page's ask to cancel a job (§11), passed on to the lead by the member whose page
    /// asked; sent only to a lead on the sender's own app (an older one wouldn't read it).
    Cancel(crate::control::Cancel),
    /// A build page's ask to try a failed target again now (§11): the lead forgets its failures
    /// (the coordinator's, and its own back-off). Passed on as `Cancel` is.
    Retry(crate::control::Retry),
}

/// A build page's ask the lead acts on (`Out::acts`): from its own page, or a member's (`Msg`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Cancel(crate::control::Cancel),
    Retry(crate::control::Retry),
}

impl Act {
    fn msg(self) -> Msg {
        match self {
            Act::Cancel(c) => Msg::Cancel(c),
            Act::Retry(r) => Msg::Retry(r),
        }
    }
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
    /// A build page's on this Mac (`Act`): the lead acts on it; a member passes it on.
    Act(Act),
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
    /// A listing of the journal it asked for (`Out::list`), done since, with the ask's number. The
    /// agent makes every listing asked for, however long it takes (one that fails made again), and
    /// hands each back once: any of a lead's asks still out counts when it's back.
    pub listed: Option<Listed>,
    /// Settling a handover (`Out::settle`): the coordinator's state as the agent wrote it once it
    /// stopped granting and cancelled its duties in flight; handed over with the records.
    pub settled: Option<serde_json::Value>,
    /// Raw tiles' archives its records hold (`Records::raw`) that the agent has named in the raw
    /// store's index since (crate::rawpack::name_handed, the index saved): taken off the records,
    /// saved with this step's snapshot. (One named and not taken off yet, a restart between, is
    /// named again: passed over unread.)
    pub named_raw: Vec<String>,
    /// Whether this Mac can lead now if offered the lead, or take over a lead that stood down (its
    /// disk, home and power; the app rule is the driver's). The agent sets it.
    pub able: bool,
    /// Re-assert before the agent does what only a fresh lead may (a GC sweep): `Out::fresh` says
    /// it did.
    pub reassert: bool,
    /// The pool's members, by id, as the agent knows them (the lead's status names them): who may
    /// take over a lead that stood down, and in what order.
    pub members: Vec<String>,
    /// The members' heartbeats as the agent last read them (shadow runs' left out): whom they
    /// report they can't reach (crate::pool::reachable), for `hand_to`.
    pub reports: Vec<Beat>,
    /// When the lead's coordinator last answered this Mac (its wall clock, unix s): a lead whose
    /// heartbeat is silent but which answered within `beat::OUT_OF_TOUCH_S` is late, not gone, and
    /// isn't taken over by itself.
    pub answered: Option<u64>,
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
    /// Leading, its records reflect the journal: a listing of every day (its take-up's, or the
    /// daily one) asked for less than `RELIST_S` ago is merged and saved, every entry told or
    /// listed is read (on a share under load a loop leaves some to the next; one never read whole
    /// is refused after `UNREADABLE_S`, or `MOVED_ON_S`), and no re-assertion is owed. A re-assertion keeps it, but
    /// after a sleep. A catalog waits for it (and `duties`); GC too (and `fresh`): an entry not
    /// merged yet may hold uploads the records don't name.
    pub caught_up: bool,
    /// Leading: the entries its check holds (`records::HOLD`: from a member on a newer app), with
    /// why: neither merged nor refused, merged again every step until the check takes or refuses
    /// them. They don't keep it from being `caught_up`.
    pub held: Vec<(String, String)>,
    /// Leading: when it asked for the newest listing of every day its records reflect (this Mac's
    /// clock; within `RELIST_S` while `caught_up`): caught up, every entry the listing found,
    /// written before it as far as a read may be stale, is merged.
    pub listed_at: Option<u64>,
    /// This process isn't the member's only one (another process holds its lock:
    /// `MemberLock::check`), and why. The agent saves this step's `Saved` (the hand-offs handed to
    /// the step are kept in it, for this folder's next process to write), writes no heartbeat (this
    /// step's is empty), sends nothing, steps no more, and leaves the pool. A term it led is the
    /// other process's (the same member) to re-assert.
    pub stop: Option<String>,
    /// A listing of the journal to make, handed back in `Heard::listed`.
    pub list: Option<Listing>,
    /// Leading: the build pages' asks to act on (its own page's, and the members' passed on).
    pub acts: Vec<Act>,
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
    /// (`done`: the applied entries' jobs done, crate::pool::records::Merged::done.)
    Merged {
        applied: Vec<String>,
        overtaken: usize,
        refused: usize,
        listed: bool,
        done: Vec<crate::pool::records::MergedJob>,
    },
    /// Something it couldn't do this step, to try again: what, and why.
    Waits { what: &'static str, why: String },
    /// A duty met an error and stopped there; the step went on.
    Failed { what: &'static str, why: String },
}

/// What a member keeps between processes: the agent saves it in its own folder after every step
/// that changed it, before acting on that step's `Out`, and starts the next process's driver from
/// it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Saved {
    /// The member it's of: another's (this Mac's member file lost and a new id made; or a copy of
    /// another Mac's folder, its id made again there) counts for nothing but its hand-offs not
    /// written yet.
    #[serde(default)]
    pub member: String,
    /// Its journal entries: kept whole until written, then (their keys, with the term whose lead
    /// acknowledged each) until a horizon passes their day.
    pub mine: Mine,
    /// The highest term it knew of (its view never goes back, a stale read at its start
    /// notwithstanding), and the highest it led.
    #[serde(default)]
    pub term: u64,
    pub led: u64,
    /// The highest term it made naming itself (a re-assertion, a takeover, a take-back, term 1):
    /// one naming it that's newer, not a handover's, was made by a process whose state this isn't
    /// (a backup restored), and is re-asserted, not taken up again.
    #[serde(default)]
    pub made: u64,
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
    /// It needs the owner's force, and why (the lead is in touch, or its heartbeat can't be read;
    /// the term can't be read whole; this Mac's own handover waits for its target).
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
    /// The entries members told it of and it hasn't acknowledged: who told it, and when it first
    /// did (awake clock).
    told: BTreeMap<String, (String, u64)>,
    /// By member, the latest first telling (awake clock) of an entry of its read whole: its writer
    /// has moved on past what it told before (`MOVED_ON_S`).
    whole_told: BTreeMap<String, u64>,
    /// Entries told or listed not read whole yet (not readable yet, or past the step's time for
    /// reading): read again from the next step on.
    waiting: BTreeSet<String>,
    /// Of those, the ones its reads found not whole, or failed on while the share answered, since
    /// when (awake clock; a read that failed, the share away, starts it again), and whether they
    /// failed last; and those it said wait long (`WAIT_SAID_S`).
    unreadable: BTreeMap<String, (u64, bool)>,
    said: BTreeSet<String>,
    /// Where the next step's reads of those start: they're read in turn.
    turn: usize,
    /// When it asked for the newest listing of every day it merged (its take-up's, or a later one):
    /// None before one is merged.
    listed_at: Option<u64>,
    /// Changes not saved yet.
    dirty: bool,
    /// Entries refused: their refusals to note once that's saved.
    refused: Vec<(String, String)>,
    /// Entries its check holds (`records::HOLD`), with why: merged again every step.
    held: BTreeMap<String, String>,
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

/// A listing asked for and not handed back yet (by its number, in `Driver::asked`).
#[derive(Clone, Copy, Debug)]
struct Asked {
    /// When (this Mac's clock).
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
    /// The listing due next (a take-up's: every day); those asked for and not back, by number (any
    /// counts when it's back; a day old, forgotten); the asks' number; when it last asked, and when
    /// a listing last came back (this Mac's clock).
    due: Option<(Option<String>, bool)>,
    asked: BTreeMap<u64, Asked>,
    asks: u64,
    swept: u64,
    back: u64,
    /// The member that handed it a term, by term: told when it leads it.
    handed_by: BTreeMap<u64, String>,
    /// It just started (a restart): a term it led and names it is re-asserted.
    restarted: bool,
    /// The current term it learnt from the NAS at its first step (none before), not counting a
    /// term 1 its own first step made.
    first: Option<u64>,
    /// The lead that stood down, as seen: its term, and since when (this Mac's clock).
    stood: Option<(u64, u64)>,
    /// The lead seen silent (its heartbeat older than `SILENT_S`): its term, its heartbeat's
    /// `beat`, and since when (awake clock) this Mac has seen that same beat.
    silent: Option<(u64, u64, u64)>,
    /// The current term found unreadable (no lead anyone knows): its number, and since when (awake
    /// clock).
    unread: Option<(u64, u64)>,
    /// Since when (awake clock) a take-up has found term 1 with no snapshot that reads whole (its
    /// lead's, or its first).
    first_unread: Option<u64>,
    /// Take-ups refused by the journal GC's marker since (this Mac's clock), and why
    /// (journal_gc::Fenced): its heartbeat says so (`Beat::fenced`), for the views' alert.
    fenced: Option<super::beat::Fenced>,
    /// The members' heartbeats its last step heard (`Heard::reports`): whether a member is
    /// reachable (`hand_to`).
    reports: Vec<Beat>,
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
        let saved = if known {
            saved
        } else {
            Saved { member: me.id.clone(), mine: saved.mine.unwritten_only(), ..Default::default() }
        };
        let passing = saved.passing.clone().map(|(own, passed, hand)| Passing { own, passed, hand, records: None });
        Driver {
            me,
            lock: None,
            stopped: None,
            cur: Current::default(),
            saved,
            known,
            lead: None,
            passing,
            taking: None,
            spare: None,
            must: None,
            slept: false,
            clocks: None,
            due: None,
            asked: BTreeMap::new(),
            asks: 0,
            swept: 0,
            back: 0,
            handed_by: BTreeMap::new(),
            restarted: true,
            first: None,
            stood: None,
            silent: None,
            unread: None,
            first_unread: None,
            fenced: None,
            reports: Vec::new(),
        }
    }

    /// What to keep for the next process: saved after every step that changed it, before that
    /// step's `Out` is acted on.
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

    /// The current term as it knows it, and its lead when the term reads whole (its members' mail
    /// goes to that lead).
    pub fn current(&self) -> &Current {
        &self.cur
    }

    /// The records of the term it leads, as it last merged them (saved or not: `Records::seq` says
    /// which snapshot they are), for the agent to read; never to change.
    pub fn records(&self) -> Option<&Records> {
        self.lead.as_ref().map(|l| &l.records)
    }

    /// One step (see the module's doc: the contract).
    pub fn step(&mut self, io: &dyn Io, heard: Heard, check: Check) -> Out {
        let mut out = Out::default();
        self.reports = heard.reports.clone();
        // One process per member: its lock still held, or it stops, writing nothing more (another
        // process holds it). A check that can't tell holds this step's duties, and is tried again.
        let mut unsure = None;
        match self.lock.as_mut().filter(|_| self.stopped.is_none()).map(MemberLock::check) {
            Some(Ok(false)) => {
                let why = format!(
                    "another process holds {}",
                    self.lock.as_ref().map_or(Path::new(""), MemberLock::path).display()
                );
                if let Some(l) = self.lead.take() {
                    out.events.push(Event::SteppedDown {
                        term: l.term.term,
                        why: format!("this process isn't the member's only one: {why}"),
                    });
                }
                self.passing = None;
                self.stopped = Some(why);
            }
            Some(Err(e)) => unsure = Some(format!("{e:#}")),
            _ => {}
        }
        if let Some(why) = &self.stopped {
            // (Its jobs' hand-offs kept, in the state the agent saves: this folder's next process
            // writes them.)
            for e in heard.entries {
                if let Err(err) = self.saved.mine.add(e) {
                    out.events.push(Event::Failed { what: "keep an entry", why: format!("{err:#}") });
                }
            }
            out.stop = Some(why.clone());
            out.term = self.cur.term.max(self.saved.term);
            return out;
        }
        let start = (io.now(), io.awake());
        if self.lead.is_some() {
            if self.clocks.is_some_and(|was| gap(was, start)) {
                self.must = Some("re-asserted after a gap");
                self.slept = true;
            } else if self.clocks.is_some_and(|was| start.1.saturating_sub(was.1) > STALL_S) && self.must.is_none() {
                // (Not stepped for that long awake: its process held up, the share stalled.)
                self.must = Some("re-asserted after a long while unstepped");
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
        let (mut tells, mut hand_asks, mut takeover, mut led_by_target, mut passed) =
            (Vec::new(), Vec::new(), None, false, None);
        let mut acts: Vec<Act> = Vec::new();
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
                Msg::Leads(e) => {
                    led_by_target |=
                        self.passing.as_ref().is_some_and(|p| p.passed.term == e && p.passed.member == from)
                }
                Msg::HandTo(to) => hand_asks.push(to),
                Msg::Cancel(c) => acts.push(Act::Cancel(c)),
                Msg::Retry(r) => acts.push(Act::Retry(r)),
            }
        }
        for a in heard.asks {
            match a {
                Ask::HandTo(to) => hand_asks.push(to),
                Ask::TakeOver { force, downgrade } => takeover = Some((force, downgrade)),
                Ask::Act(a) => acts.push(a),
            }
        }
        let learnt = self.learn(io, passed);
        if let Err(e) = &learnt {
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
            self.lead_step(
                io,
                &mut out,
                tells,
                hand_asks,
                heard.listed,
                heard.settled,
                &heard.named_raw,
                check,
                busy,
                learnt.is_ok(),
            );
        } else if let Some(t) = self.cur.lead.as_ref().filter(|t| t.member != self.me.id) {
            out.send.extend(hand_asks.into_iter().map(|to| (t.member.clone(), Msg::HandTo(to))));
        }
        // The pages' asks: acted on leading, else passed on to a lead on this app (an older app's
        // mail reader would fail on them).
        if self.lead.is_some() {
            out.acts.extend(acts);
        } else if !acts.is_empty() {
            match self.cur.lead.as_ref().filter(|t| t.member != self.me.id) {
                Some(t) if t.app == self.me.app => {
                    out.send.extend(acts.into_iter().map(|a| (t.member.clone(), a.msg())))
                }
                Some(t) => out.events.push(Event::Failed {
                    what: "pass the build page's ask on to the lead",
                    why: format!("the lead runs app {}, this Mac {}: ask on the lead's own page", t.app, self.me.app),
                }),
                None => out.events.push(Event::Failed {
                    what: "pass the build page's ask on to the lead",
                    why: "no lead now".into(),
                }),
            }
        }
        if self.passing.is_some() {
            self.passing_step(io, &mut out, led_by_target);
        }
        if let Some((force, downgrade)) = takeover {
            self.take_over(io, &mut out, force, downgrade);
        }
        let read = self.auto_take_over(io, &mut out, &heard.members, heard.able);
        self.take_over_silent(io, &mut out, &heard.members, heard.able, heard.answered, read);
        self.take_past_unread(io, &mut out, &heard.members, heard.able, learnt.is_ok());
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
        // (Its view as the state it saves keeps it: a `learn` cut short by the share, its view
        // moved on, doesn't leave the state behind it, for a restart to go back to.)
        self.saved.term = self.saved.term.max(self.cur.term);
        out.term = self.saved.term;
        if let Some(l) = &self.lead {
            out.leads = Some(l.term.term);
            out.duties = l.hand.grants() && self.must.is_none() && !self.beat_stale(io, &mut out);
            out.settle = matches!(l.hand, Handover::Settling { .. });
            out.fresh &= self.must.is_none();
            out.caught_up = l.listed_at.is_some_and(|at| end.0.saturating_sub(at) < RELIST_S)
                && !l.dirty
                && l.waiting.is_empty()
                && self.must.is_none();
            out.listed_at = l.listed_at;
            out.held = l.held.iter().map(|(k, w)| (k.clone(), w.clone())).collect();
        } else {
            out.fresh = false;
        }
        // (Its lock not known held this step: no duties, catalog or sweep until it's checked
        // again.)
        if let Some(why) = unsure {
            out.events.push(Event::Failed { what: "check the member's lock", why });
            (out.duties, out.caught_up, out.fresh) = (false, false, false);
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
            let made =
                term::bootstrap(io, &self.me, now, false)?.is_some_and(|t| t.member == self.me.id && t.since == now);
            if made {
                self.saved.made = self.saved.made.max(1);
            }
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
        if let Some(t) =
            passed.filter(|t| t.term > self.cur.term || (t.term == self.cur.term && self.cur.lead.is_none()))
        {
            self.cur = Current { term: t.term, lead: Some(t) };
        }
        self.saved.term = self.saved.term.max(self.cur.term);
        Ok(())
    }

    /// Finishes the term its create made and couldn't fill; passed on, tells its target.
    fn finish(&mut self, io: &dyn Io, out: &mut Out) {
        let Some(t) = self.saved.unfinished.clone() else { return };
        // (A later term made past it meanwhile: it's no one's to lead, and isn't finished.)
        if term::next(io, t.term).unwrap_or(false) {
            self.saved.unfinished = None;
            out.events.push(Event::Waits {
                what: "finish the term it made",
                why: format!("term {} was made past it: left unfinished", t.term + 1),
            });
            return;
        }
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
                    self.saved.made = self.saved.made.max(t.term);
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
                if t.member == self.me.id {
                    self.saved.made = self.saved.made.max(t.term);
                }
                self.saved.unfinished = Some(t);
                true
            }
            Made::Theirs => false,
        }
    }

    /// Re-asserts: makes the next term naming itself and takes it up from its own records;
    /// another's made first, it steps down; the app rule refusing it, it stands down.
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
                let (slept, due, asked) = (std::mem::take(&mut self.slept), self.due.clone(), self.asked.clone());
                self.made(io, out, t, made);
                // (No other lead between: what it knew of the journal holds, its take-up's listing
                // merged too, so a sweep's step is caught up. Not after a sleep: what was written
                // meanwhile, told to it and lost, a listing finds.)
                if let Some(n) = self.lead.as_mut() {
                    (n.told, n.whole_told, n.waiting, n.unreadable, n.said, n.refused, n.held) =
                        (l.told, l.whole_told, l.waiting, l.unreadable, l.said, l.refused, l.held);
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
    /// or another member's) counts as one it led: it may have, its leases granted. So does one its
    /// state is older than (a backup restored): one it made that the state doesn't record, and one
    /// whose records are there already, taken up by a process before this one (a handover's too).
    /// Not on an app older than the term's (restarted into an older app or a development build): it
    /// stands down, and says so, until its app is new enough.
    fn own_term(&mut self, io: &dyn Io, out: &mut Out) {
        // (A term its create made and couldn't fill: finished first, then taken up.)
        if self.saved.unfinished.is_some() {
            return;
        }
        let Some(t) = self.cur.lead.clone().filter(|t| t.member == self.me.id && t.term >= self.saved.led) else {
            return;
        };
        if !term::app_at_least(&self.me.app, &t.app) {
            if self.saved.stood_down != Some(t.term) {
                self.saved.stood_down = Some(t.term);
                out.events.push(Event::SteppedDown {
                    term: t.term,
                    why: format!(
                        "can't re-assert: {} runs app {}, older than term {}'s {}",
                        self.me.host, self.me.app, t.term, t.app
                    ),
                });
            }
            return;
        }
        // (Newer than it led: taken up, but for one it made that its state doesn't record, or one
        // whose records are there and that this process isn't taking up: an older state of its own
        // restored, which may have led it, its leases granted.)
        let unrecorded = t.seq.is_none() && t.term > self.saved.made;
        let mut older = unrecorded;
        if t.term > self.saved.led && !unrecorded && !self.taking.as_ref().is_some_and(|r| r.term == t.term) {
            match io.exists(&records::path(t.term)) {
                Ok(there) => older = there,
                Err(e) => {
                    return out.events.push(Event::Waits {
                        what: "take up",
                        why: format!("term {}: whether its records are there: {e:#}", t.term),
                    })
                }
            }
        }
        if t.term > self.saved.led && !older {
            self.take_up(io, out, &t);
            return;
        }
        let how = match () {
            _ if !self.known => "re-asserted: its saved state lost",
            _ if older => "re-asserted: its saved state older than the term",
            _ if self.restarted => "restarted: re-asserted",
            _ => "re-asserted: its app new enough",
        };
        match Term::after(&self.cur, &self.me, how, io.now()).and_then(|n| Ok((term::make(io, &n)?, n))) {
            Ok((made, n)) => {
                if !self.made(io, out, n, made) {
                    out.events.push(Event::Waits {
                        what: "re-assert",
                        why: format!("term {} made by another first", self.cur.term + 1),
                    });
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
        // (Term 1 with no snapshot that reads whole, its lead's or its first: from today's files
        // only once it has stayed so for `STALE_S` awake, no stale read of them, or of today's
        // files, left.)
        let today = self.first_unread.is_some_and(|at| io.awake().saturating_sub(at) >= STALE_S);
        let mut r = match records::start(io, t, own.as_ref(), today) {
            Ok(r) => {
                self.first_unread = None;
                self.fenced = None;
                r
            }
            Err(e) => {
                if e.is::<records::FirstUnreadable>() {
                    self.first_unread.get_or_insert(io.awake());
                }
                if let Some(f) = e.chain().find_map(|c| c.downcast_ref::<super::journal_gc::Fenced>()) {
                    let since = self.fenced.as_ref().map_or(io.now(), |f| f.since);
                    self.fenced = Some(super::beat::Fenced { since, why: f.0.clone() });
                }
                return out.events.push(Event::Waits { what: "take up", why: format!("term {}: {e:#}", t.term) });
            }
        };
        let handed = records::handed(&mut r, t);
        // (Nothing merged yet: its members tell it of their entries once they know it leads, and a
        // listing of the journal follows.)
        if let Err(e) = r.save(io) {
            self.taking = Some(r);
            return out
                .events
                .push(Event::Failed { what: "save the records", why: format!("taking up term {}: {e:#}", t.term) });
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
        self.lead = Some(Lead {
            term: t.clone(),
            records: r,
            hand: Handover::Leading,
            told: BTreeMap::new(),
            whole_told: BTreeMap::new(),
            waiting: BTreeSet::new(),
            unreadable: BTreeMap::new(),
            said: BTreeSet::new(),
            turn: 0,
            listed_at: None,
            dirty: false,
            refused: Vec::new(),
            held: BTreeMap::new(),
        });
        // A take-up lists the journal: every day not forgotten.
        self.due = Some((Some(horizon).filter(|h| !h.is_empty()), true));
        self.asked.clear();
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
    fn lead_step(
        &mut self,
        io: &dyn Io,
        out: &mut Out,
        tells: Vec<(String, Vec<String>)>,
        asks: Vec<String>,
        listed: Option<Listed>,
        settled: Option<serde_json::Value>,
        named_raw: &[String],
        check: Check,
        busy: u64,
        learnt: bool,
    ) {
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
        let now = io.now();
        let told_at = io.awake();
        let l = self.lead.as_mut().expect("leading");
        let e = l.term.term;
        for (from, keys) in tells.into_iter().chain([(me.clone(), own)]) {
            for k in keys {
                l.told.entry(k).or_insert_with(|| (from.clone(), told_at));
            }
        }
        let mut keys: Vec<String> =
            l.told.keys().chain(&l.waiting).chain(l.held.keys()).filter(|k| !l.records.handles(k)).cloned().collect();
        let from_listing = listed.is_some();
        if let Some(listed) = listed {
            // (A listing of every day, its take-up's or a daily one: once merged and saved, the
            // records reflect the journal as it was when it was asked for; any of its asks still
            // out, the newest's time kept. Keys asked for before its take-up are merged as any.)
            self.back = now;
            if let Some(a) = self.asked.remove(&listed.n) {
                if a.full {
                    l.listed_at = Some(l.listed_at.map_or(a.at, |at| at.max(a.at)));
                    // (The entries the journal's GC removed, forgotten: §7.3.)
                    match super::journal_gc::read(io) {
                        Ok(Some(g)) if g.done && g.horizon > l.records.horizon && g.covers(&l.records) => {
                            l.records.forget(&g.keys, &g.horizon);
                            l.dirty = true;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            out.events.push(Event::Failed { what: "read the journal's GC", why: format!("{e:#}") })
                        }
                    }
                }
            }
            keys.extend(listed.keys.into_iter().filter(|k| !l.records.handles(k)));
        }
        // The raw archives the agent named: off the records, for good.
        if !named_raw.is_empty() {
            let named: BTreeSet<&str> = named_raw.iter().map(String::as_str).collect();
            let was = l.records.raw.len();
            l.records.raw.retain(|(_, p)| !named.contains(p.name.as_str()));
            l.dirty |= l.records.raw.len() != was;
        }
        // Settling: the coordinator's state, as the agent wrote it, saved with the records.
        if let (Some(c), Handover::Settling { .. }) = (settled, &l.hand) {
            l.records.handed = Some(c);
            l.dirty = true;
        }
        // Read while the step's time for it lasts, the rest left to the next step: first those no
        // read has found not whole or failed on (a step's leftovers among them), the oldest lease
        // first; then the others in turn, from where the last step's reads of them stopped, so each
        // is read again every few steps however many wait. (Read first, entries that won't be whole
        // took all the time on a share under load and left the others waiting; read in one order,
        // the last of them waited behind the first until those were refused, an hour on.)
        records::by_lease(&mut keys);
        keys.dedup();
        let (fresh, mut again): (Vec<String>, Vec<String>) =
            keys.into_iter().partition(|k| !l.unreadable.contains_key(k));
        if !again.is_empty() {
            let at = l.turn % again.len();
            again.rotate_left(at);
        }
        let n = again.len();
        let keys: Vec<String> = fresh.into_iter().chain(again).collect();
        let m = records::merge_while(io, &mut l.records, &keys, check, &|| io.awake() < busy);
        // (Each member's entries read whole: how far its writer has moved on.)
        for k in m.applied.iter().chain(&m.overtaken).chain(m.refused.iter().map(|(k, _)| k)) {
            if let Some((from, at)) = l.told.get(k) {
                let w = l.whole_told.entry(from.clone()).or_default();
                *w = (*w).max(*at);
            }
        }
        l.turn = l.turn.wrapping_add(n - m.unread.iter().filter(|k| l.unreadable.contains_key(*k)).count().min(n));
        if !m.applied.is_empty() || !m.refused.is_empty() || !m.overtaken.is_empty() {
            l.dirty = true;
            l.refused.extend(m.refused.iter().cloned());
            out.events.push(Event::Merged {
                applied: m.applied.clone(),
                overtaken: m.overtaken.len(),
                refused: m.refused.len(),
                listed: from_listing,
                done: m.done.clone(),
            });
        }
        // What its check holds is merged again next step (said once as it's first held); what it
        // read this step and didn't hold is no longer held.
        let unread: BTreeSet<&String> = m.unread.iter().collect();
        let was: BTreeSet<String> = l.held.keys().cloned().collect();
        let records = &l.records;
        l.held.retain(|k, _| unread.contains(k) && !records.handles(k));
        for (k, why) in &m.held {
            if l.held.insert(k.clone(), why.clone()).is_none() && !was.contains(k) {
                out.events.push(Event::Waits { what: "merge an entry", why: format!("{k}: {why}") });
            }
        }
        // What isn't read whole yet is read again next step; what's handled or forgotten, done.
        l.waiting.extend(m.waiting.iter().chain(&m.unread).chain(&m.failed).cloned());
        l.waiting.retain(|k| !l.records.handles(k) && !m.forgotten.contains(k));
        // What its reads find not whole over `UNREADABLE_S` never will be: refused, as a damaged
        // entry is (its work done again). So what its reads fail on while the share answers the
        // step's others (its learning of the term, or reads of other entries): a file the share
        // errors on. A read that failed when none answered (the share away) says nothing, and
        // starts the time again; one the step's time left unread keeps it, and isn't refused
        // unread.
        let answered = learnt
            || !(m.applied.is_empty()
                && m.overtaken.is_empty()
                && m.refused.is_empty()
                && m.forgotten.is_empty()
                && m.waiting.is_empty());
        let awake = io.awake();
        for k in &m.failed {
            if answered {
                l.unreadable.entry(k.clone()).or_insert((awake, true)).1 = true;
            } else {
                l.unreadable.remove(k);
            }
        }
        for k in &m.waiting {
            l.unreadable.entry(k.clone()).or_insert((awake, false)).1 = false;
        }
        l.unreadable.retain(|k, _| l.waiting.contains(k));
        let read: Vec<String> = m.waiting.iter().chain(m.failed.iter().filter(|_| answered)).cloned().collect();
        // (One waiting long: said, once, naming it.)
        for k in &read {
            if let Some(&(at, failing)) =
                l.unreadable.get(k).filter(|&&(at, _)| awake.saturating_sub(at) >= WAIT_SAID_S)
            {
                if l.said.insert(k.clone()) {
                    let how =
                        if failing { "its reads failing, the share answering others" } else { "short, or not there" };
                    out.events.push(Event::Waits {
                        what: "read an entry whole",
                        why: format!("{k}: {how}, for {} minutes", awake.saturating_sub(at) / 60),
                    });
                }
            }
        }
        l.said.retain(|k| l.waiting.contains(k));
        // (Refused after an hour; after `MOVED_ON_S` once its writer has moved on: an entry it
        // told of later is read whole.)
        let moved_on =
            |k: &String| l.told.get(k).is_some_and(|(from, at)| l.whole_told.get(from).is_some_and(|w| w > at));
        let never: Vec<(String, String)> = read
            .into_iter()
            .filter_map(|k| {
                let since = awake.saturating_sub(l.unreadable.get(&k)?.0);
                let why = if since >= UNREADABLE_S {
                    format!("not read whole in {} minutes", UNREADABLE_S / 60)
                } else if since >= MOVED_ON_S && moved_on(&k) {
                    format!("not read whole in {} minutes, its writer's later entries read whole", since / 60)
                } else {
                    return None;
                };
                Some((k, why))
            })
            .collect();
        if !never.is_empty() {
            for (k, why) in &never {
                l.records.refuse(k, why);
                l.waiting.remove(k);
                l.unreadable.remove(k);
                l.refused.push((k.clone(), why.clone()));
            }
            l.dirty = true;
            out.events.push(Event::Merged {
                applied: Vec::new(),
                overtaken: 0,
                refused: never.len(),
                listed: false,
                done: Vec::new(),
            });
        }
        if l.dirty {
            match l.records.save(io) {
                Ok(()) => l.dirty = false,
                Err(err) => {
                    out.events.push(Event::Failed { what: "save the records", why: format!("term {e}: {err:#}") })
                }
            }
        }
        // Refusals its saved records name (this step's, or an earlier one's, a re-assertion's
        // take-up saving them): noted for the owner. (A note not made now is only the owner's
        // loss.)
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
            for (k, (from, _)) in &l.told {
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
        let settled = (!l.dirty
            && m.waiting.is_empty()
            && m.unread.is_empty()
            && m.failed.is_empty()
            && l.records.handed.is_some())
        .then_some(l.records.seq);
        let target = match &l.hand {
            Handover::Offered { to, .. } | Handover::Settling { to, .. } => Beat::read(io, to).ok().flatten(),
            _ => None,
        };
        let was = l.hand.clone();
        let step = l.hand.step(
            e,
            &Seen {
                now: io.now(),
                current: self.cur.term,
                ask: ask.as_deref(),
                target: target.as_ref(),
                settled,
                taken_up: false,
            },
        );
        match (&was, &l.hand) {
            (Handover::Leading, Handover::Offered { to, .. }) => {
                out.events.push(Event::Handover { term: e, to: to.clone(), what: "offered" })
            }
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
                let t = match b
                    .map(|b| Term::after(&self.cur, &b.member(), &format!("handed over by {}", self.me.host), io.now()))
                {
                    Some(Ok(t)) => t,
                    other => {
                        let why = other.map_or("its heartbeat isn't readable".to_string(), |r| {
                            r.err().map(|e| format!("{e:#}")).unwrap_or_default()
                        });
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
                        self.passing =
                            Some(Passing { own: l.term, passed: t.clone(), hand: l.hand, records: Some(l.records) });
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
        let seen = Seen {
            now: io.now(),
            current: self.cur.term,
            target: target.as_ref(),
            taken_up: snapshot,
            ..Default::default()
        };
        match p.hand.step(p.own.term, &seen) {
            Do::Done => out.events.push(Event::Handover { term: p.own.term, to, what: "over" }),
            Do::TakeBack { to } if self.cur.term == p.passed.term => {
                match term::back(&p.own, &p.passed, &self.me, &format!("{}: {to} didn't take up", term::BACK), io.now())
                {
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
                }
            }
            _ => self.passing = Some(p),
        }
    }

    /// The owner's "Take it": when the lead is out of touch or stood down, or forced (over this
    /// Mac's own handover still waiting for its target too).
    fn take_over(&mut self, io: &dyn Io, out: &mut Out, force: bool, downgrade: bool) {
        let needs = self.takeover(io);
        let why = needs
            .refused
            .clone()
            .or_else(|| needs.force.clone().filter(|_| !force))
            .or_else(|| needs.downgrade.clone().filter(|_| !downgrade));
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
                        self.saved.made = self.saved.made.max(t.term);
                        self.take_up(io, out, &t);
                    }
                }
                Ok(None) => {}
                Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
            }
            return;
        }
        let t = if force || downgrade {
            term::forced(io, &self.cur, &self.me, &how, now, downgrade)
        } else {
            Term::after(&self.cur, &self.me, &how, now)
        };
        match t.and_then(|t| Ok((term::make(io, &t)?, t))) {
            Ok((made, t)) => {
                if !self.made(io, out, t, made) {
                    out.events.push(Event::Waits {
                        what: "take over",
                        why: format!("term {} made by another first", self.cur.term + 1),
                    });
                }
            }
            Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
        }
    }

    /// A lead that stood down (its heartbeat says so: §6.6), seen so for `STOOD_DOWN_S`: taken over
    /// by this Mac, if the app rule lets it, after the members that can and come first (newest app,
    /// then lowest member id) had their turn, `AUTO_RANK_S` each.
    /// Returns the lead's heartbeat as it read it (None: not read, or the read failed), for
    /// `take_over_silent`: one read a step.
    fn auto_take_over(&mut self, io: &dyn Io, out: &mut Out, members: &[String], able: bool) -> Option<Option<Beat>> {
        let t = match &self.cur.lead {
            Some(t)
                if t.member != self.me.id
                    && self.lead.is_none()
                    && self.passing.is_none()
                    && self.saved.unfinished.is_none() =>
            {
                t.clone()
            }
            _ => {
                self.stood = None;
                return None;
            }
        };
        // (A stand-down is its term's for good: in touch or not by this Mac's clock, which may be
        // minutes off the lead's, its heartbeat saying so is enough.)
        let read = Beat::read(io, &t.member).ok();
        let b = read.clone().flatten();
        let now = io.now();
        if !b.is_some_and(|b| b.stood_down == Some(t.term)) {
            self.stood = None;
            return read;
        }
        let since = match self.stood {
            Some((e, since)) if e == t.term => since,
            _ => {
                self.stood = Some((t.term, now));
                now
            }
        };
        if !able || !term::app_at_least(&self.me.app, &t.app) || now.saturating_sub(since) < STOOD_DOWN_S {
            return None;
        }
        let mut can = vec![self.me.clone()];
        for m in members.iter().filter(|m| **m != self.me.id && **m != t.member) {
            if let Ok(Some(b)) = Beat::read(io, m) {
                if !b.out_of_touch(now)
                    && b.leads.is_none()
                    && b.stood_down.is_none()
                    && term::app_at_least(&b.app, &t.app)
                {
                    can.push(b.member());
                }
            }
        }
        can.sort_by(first_to_try);
        let rank = can.iter().position(|m| m.id == self.me.id).unwrap_or(0) as u64;
        if now.saturating_sub(since) < STOOD_DOWN_S + rank * AUTO_RANK_S {
            return None;
        }
        let how = format!("taken over by {}: {} stood down", self.me.host, t.host);
        match Term::after(&self.cur, &self.me, &how, io.now()).and_then(|n| Ok((term::make(io, &n)?, n))) {
            Ok((made, n)) => {
                if !self.made(io, out, n, made) {
                    out.events.push(Event::Waits {
                        what: "take over",
                        why: format!("term {} made by another first", self.cur.term + 1),
                    });
                }
            }
            Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
        }
        None
    }

    /// Leading: whether its own heartbeat on the NAS (read past this Mac's cache) is older than
    /// `LEAD_BEAT_S` by its clock, or can't be read (failing closed): its writes aren't landing, members may see it silent and
    /// take over (`take_over_silent`), so it grants nothing until one lands. None there yet (a
    /// first step): not stale, as no member can see it silent either.
    fn beat_stale(&self, io: &dyn Io, out: &mut Out) -> bool {
        let now = io.now();
        let why = match Beat::read_fresh(io, &self.me.id) {
            Ok(Some(b)) if now.saturating_sub(b.beat) > LEAD_BEAT_S => {
                format!("its own heartbeat on the NAS is {} s old", now.saturating_sub(b.beat))
            }
            Ok(_) => return false,
            Err(e) => format!("its own heartbeat can't be read: {e:#}"),
        };
        out.events.push(Event::Waits { what: "grant", why });
        true
    }

    /// A lead gone silent (§6.5: its heartbeat older than `SILENT_S` by this Mac's clock, the same
    /// beat watched for `SILENT_SEEN_S` awake, its coordinator not answering this Mac meanwhile):
    /// taken over by this Mac, if it can lead now and the app rule lets it, after the members in
    /// touch that can and come first had their turn, `AUTO_RANK_S` each; the next term's
    /// create-new decides between them. Not while this Mac's own heartbeat isn't landing (within
    /// `OWN_BEAT_S`: a NAS slow for everyone isn't the lead gone), nor when the lead's heartbeat
    /// can't be read. The old lead, back, re-asserts before it acts (its clocks' gap, or `STALL_S`
    /// unstepped), finds the newer term and steps down; its coordinator grants nothing meanwhile.
    fn take_over_silent(
        &mut self,
        io: &dyn Io,
        out: &mut Out,
        members: &[String],
        able: bool,
        answered: Option<u64>,
        read: Option<Option<Beat>>,
    ) {
        let t = match &self.cur.lead {
            Some(t)
                if t.member != self.me.id
                    && self.lead.is_none()
                    && self.passing.is_none()
                    && self.saved.unfinished.is_none() =>
            {
                t.clone()
            }
            _ => {
                self.silent = None;
                return;
            }
        };
        let now = io.now();
        // (The lead's heartbeat as `auto_take_over` read it this step; a read that failed, or none
        // there: not known silent.)
        let Some(Some(b)) = read else {
            self.silent = None;
            return;
        };
        let late = answered.is_some_and(|a| now.saturating_sub(a) < super::beat::OUT_OF_TOUCH_S);
        if b.stood_down == Some(t.term) || late {
            self.silent = None;
            return;
        }
        // (The same beat watched unchanged, by this Mac's awake clock, whatever the lead's clock:
        // from the first read of it, old or not.)
        let awake = io.awake();
        let since = match self.silent {
            Some((e, beat, since)) if e == t.term && beat == b.beat => since,
            _ => {
                self.silent = Some((t.term, b.beat, awake));
                awake
            }
        };
        if now <= b.beat.saturating_add(SILENT_S) {
            return;
        }
        // (This Mac's own heartbeat landing: a NAS slow for every Mac isn't the lead gone.)
        if !matches!(Beat::read_fresh(io, &self.me.id), Ok(Some(o)) if now.saturating_sub(o.beat) <= OWN_BEAT_S) {
            self.silent = None;
            return;
        }
        if !able || !term::app_at_least(&self.me.app, &t.app) {
            return;
        }
        let mut can = vec![self.me.clone()];
        for m in members.iter().filter(|m| **m != self.me.id && **m != t.member) {
            if let Ok(Some(o)) = Beat::read(io, m) {
                if !o.out_of_touch(now)
                    && o.leads.is_none()
                    && o.stood_down.is_none()
                    && term::app_at_least(&o.app, &t.app)
                {
                    can.push(o.member());
                }
            }
        }
        can.sort_by(first_to_try);
        let rank = can.iter().position(|m| m.id == self.me.id).unwrap_or(0) as u64;
        if awake.saturating_sub(since) < SILENT_SEEN_S + rank * AUTO_RANK_S {
            return;
        }
        let how =
            format!("taken over by {}: {} silent for {} min", self.me.host, t.host, now.saturating_sub(b.beat) / 60);
        match Term::after(&self.cur, &self.me, &how, now).and_then(|n| Ok((term::make(io, &n)?, n))) {
            Ok((made, n)) => {
                self.silent = None;
                if !self.made(io, out, n, made) {
                    out.events.push(Event::Waits {
                        what: "take over",
                        why: format!("term {} made by another first", self.cur.term + 1),
                    });
                }
            }
            Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
        }
    }

    /// A current term unreadable (empty or short: its create landed, its bytes never did) for
    /// `UNREAD_TAKE_S` awake, as this Mac's reads find it: the next term made past it by this Mac,
    /// as the owner's force would (`term::forced`, the app rule against the newest term that can be
    /// read), if it can lead now, after the members in touch that can and come first had their
    /// turn, `AUTO_RANK_S` each (§6.5: why it's safe). Not by a Mac that knows it made the term
    /// (`Saved::unfinished`): it finishes it.
    fn take_past_unread(&mut self, io: &dyn Io, out: &mut Out, members: &[String], able: bool, learnt: bool) {
        if !learnt
            || self.cur.term == 0
            || self.cur.lead.is_some()
            || self.lead.is_some()
            || self.passing.is_some()
            || self.saved.unfinished.is_some()
        {
            self.unread = None;
            return;
        }
        // (Only a read that answers, of a file that's there, its bytes too few to parse, counts: a
        // read that fails says nothing of the term, and starts the time again.)
        if !matches!(term::short(io, self.cur.term), Ok(true)) {
            self.unread = None;
            return;
        }
        let now = io.now();
        if !self.clear_to_take_past(io, members, now) {
            self.unread = None;
            return;
        }
        let awake = io.awake();
        let since = match self.unread {
            Some((e, since)) if e == self.cur.term => since,
            _ => {
                self.unread = Some((self.cur.term, awake));
                awake
            }
        };
        if !able || awake.saturating_sub(since) < UNREAD_TAKE_S {
            return;
        }
        // (The newest term that can be read: the app rule's, as the owner's force checks it.)
        let Some(known) = (1..self.cur.term).rev().find_map(|e| term::read(io, e).ok().flatten()) else { return };
        if !term::app_at_least(&self.me.app, &known.app) {
            return;
        }
        let mut can = vec![self.me.clone()];
        for m in members.iter().filter(|m| **m != self.me.id) {
            if let Ok(Some(b)) = Beat::read(io, m) {
                if !b.out_of_touch(now) && b.leads.is_none() && term::app_at_least(&b.app, &known.app) {
                    can.push(b.member());
                }
            }
        }
        can.sort_by(first_to_try);
        let rank = can.iter().position(|m| m.id == self.me.id).unwrap_or(0) as u64;
        if awake.saturating_sub(since) < UNREAD_TAKE_S + rank * AUTO_RANK_S {
            return;
        }
        // (Read again just before, the heartbeats too: a term that reads whole now, a read that
        // fails, or a member not known clear, and it waits.)
        if !matches!(term::short(io, self.cur.term), Ok(true)) || !self.clear_to_take_past(io, members, io.now()) {
            self.unread = None;
            return;
        }
        let how = format!(
            "taken over by {}: term {} unreadable for {} min",
            self.me.host,
            self.cur.term,
            awake.saturating_sub(since) / 60
        );
        match term::forced(io, &self.cur, &self.me, &how, now, false).and_then(|n| Ok((term::make(io, &n)?, n))) {
            Ok((made, n)) => {
                if !self.made(io, out, n, made) {
                    out.events.push(Event::Waits {
                        what: "take over",
                        why: format!("term {} made by another first", self.cur.term + 1),
                    });
                }
            }
            Err(e) => out.events.push(Event::Failed { what: "take over", why: format!("{e:#}") }),
        }
    }

    /// Whether taking the current term past is clear, failing closed (§6.5): this Mac's own
    /// heartbeat on the NAS written within `OWN_BEAT_S` (its writes land), at least one other member
    /// known (a member list that's only itself may be a listing that failed), and every other
    /// member's heartbeat read, in touch, and not leading the term (it would read it whole). A read
    /// that fails, a heartbeat missing or not whole, or one out of touch: not clear.
    fn clear_to_take_past(&self, io: &dyn Io, members: &[String], now: u64) -> bool {
        let fresh = |b: &Beat| !b.out_of_touch(now) && now.saturating_sub(b.beat) <= OWN_BEAT_S;
        if !matches!(Beat::read(io, &self.me.id), Ok(Some(b)) if fresh(&b)) {
            return false;
        }
        let others: Vec<&String> = members.iter().filter(|m| **m != self.me.id).collect();
        !others.is_empty()
            && others.iter().all(
                |m| matches!(Beat::read(io, m), Ok(Some(b)) if !b.out_of_touch(now) && b.leads != Some(self.cur.term)),
            )
    }

    /// Since when (awake clock) its reads have found the current term there but too short to read
    /// whole (`take_past_unread`): None when it reads whole, or reads fail.
    pub fn unread_since(&self) -> Option<u64> {
        self.unread.filter(|(e, _)| *e == self.cur.term).map(|(_, s)| s)
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
            t.force = Some(format!(
                "this Mac's handover of term {} waits for {} to take up: forced, it's dropped",
                p.own.term, p.passed.host
            ));
        }
        let app = |k: &Term| {
            (!term::app_at_least(&self.me.app, &k.app))
                .then(|| format!("this Mac's app {} is older than term {}'s {}", self.me.app, k.term, k.app))
        };
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
                            t.force.get_or_insert_with(|| {
                                format!("{} leads, in touch: its beat {} s old", c.host, now.saturating_sub(b.beat))
                            });
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
        // (§6.8: a member another reports it can't reach would grant nobody else work.)
        if let Some(why) =
            super::reachable::why_unreachable(&self.reports, to, if b.host.is_empty() { to } else { &b.host }, now)
        {
            return Err(why);
        }
        Ok(())
    }

    /// The listings it asks for: of every day, a take-up's, then daily (`RELIST_S`); and of the
    /// last days every `SWEEP_S`. One at a time, none while its last is out (the agent hands every
    /// one back, however long it takes); its last out `OVERDUE_S` with none back meanwhile (lost,
    /// or that slow) is said, and the next asked for all the same. Whichever comes back counts:
    /// each ask is kept until it's a day old, its listing too old by then to count.
    fn listings(&mut self, io: &dyn Io, out: &mut Out) {
        let Some(l) = &self.lead else {
            self.due = None;
            return;
        };
        let now = io.now();
        self.asked.retain(|_, a| now.saturating_sub(a.at) < RELIST_S);
        if let Some(a) = self.asked.get(&self.asks) {
            let quiet = now.saturating_sub(a.at.max(self.back));
            if quiet < OVERDUE_S {
                return;
            }
            out.events.push(Event::Waits {
                what: "have a listing back",
                why: format!(
                    "listing {} of the journal, asked for {} minutes ago, none back for {} minutes: asked for again",
                    self.asks,
                    now.saturating_sub(a.at) / 60,
                    quiet / 60
                ),
            });
        }
        if self.due.is_none() && l.listed_at.is_none_or(|at| now.saturating_sub(at) + AHEAD_S >= RELIST_S) {
            self.due = Some((Some(l.records.horizon.clone()).filter(|h| !h.is_empty()), true));
        }
        if self.due.is_none() && now.abs_diff(self.swept) >= SWEEP_S {
            let since = journal::day(now.saturating_sub(SWEEP_DAYS * 86_400)).filter(|d| *d > l.records.horizon);
            self.due = Some((since.or_else(|| Some(l.records.horizon.clone()).filter(|h| !h.is_empty())), false));
        }
        if let Some((since, full)) = self.due.take() {
            self.asks += 1;
            self.asked.insert(self.asks, Asked { at: now, full });
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
        Beat {
            member: self.me.id.clone(),
            host: self.me.host.clone(),
            app: self.me.app.clone(),
            beat: io.now(),
            leads,
            handing_to,
            ready_for,
            stood_down,
            addresses: Vec::new(),
            reach: Vec::new(),
            journal: super::journal_gc::FENCE,
            fenced: self
                .fenced
                .clone()
                .filter(|_| self.lead.is_none() && self.cur.lead.as_ref().is_some_and(|t| t.member == self.me.id)),
        }
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

    /// One Mac's view of the shared NAS, with its own clocks; while `down`, its reads, stats and
    /// listings of paths holding `fail` (of every path, when None) fail: the share doesn't answer
    /// them; and its whole writes of paths holding `wfail` fail.
    struct Mac<'a> {
        mem: &'a Mem,
        wall: Cell<u64>,
        awake: Cell<u64>,
        down: Cell<bool>,
        fail: std::cell::RefCell<Option<String>>,
        wfail: std::cell::RefCell<Option<String>>,
        /// Its reads of paths holding any of these (`|` between) answer, but empty (a stale or
        /// cut-short read).
        short: std::cell::RefCell<Option<String>>,
        /// Its reads of paths holding this answer that there's no such file (a stale read).
        gone: std::cell::RefCell<Option<String>>,
    }

    impl<'a> Mac<'a> {
        fn new(mem: &'a Mem) -> Mac<'a> {
            Mac {
                mem,
                wall: Cell::new(T0),
                awake: Cell::new(1_000),
                down: Cell::new(false),
                fail: Default::default(),
                wfail: Default::default(),
                short: Default::default(),
                gone: Default::default(),
            }
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
            if self.short.borrow().as_deref().is_some_and(|f| f.split('|').any(|f| path.contains(f))) {
                return Ok(self.mem.read(path)?.map(|_| Vec::new()));
            }
            if self.gone.borrow().as_deref().is_some_and(|f| path.contains(f)) {
                return Ok(None);
            }
            self.mem.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            if self.down.get() && self.fail.borrow().as_deref().is_none_or(|f| path.contains(f)) {
                anyhow::bail!("stat {path}: the share doesn't answer");
            }
            self.mem.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            if self.down.get() && self.fail.borrow().as_deref().is_none_or(|f| dir.contains(f)) {
                anyhow::bail!("list {dir}: the share doesn't answer");
            }
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
        mem.write_whole("state/build/manifest.json", b"{}").unwrap();
    }

    fn entry(member: &str, term: u64, n: u64, t: &str) -> Entry {
        let h = Handoff {
            changes: [(format!("base/{t}"), Some(format!("base/{t}.k{n}.base")))].into(),
            done: Some(("unit".into(), vec![(t.into(), format!("k{n}"))])),
            ..Default::default()
        };
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
    fn a_pages_cancel_or_retry_goes_to_the_lead_on_its_app_alone() {
        // (Task #409 phase 3) a member's page's ask is mailed to the lead, which acts on it, as on
        // its own page's; a lead on another app is told nothing (its mail reader wouldn't know it).
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        let cancel = Act::Cancel(crate::control::Cancel::new("6/1/2", Some("unit"), false, "the build page (m1)"));
        let retry = Act::Retry(crate::control::Retry {
            target: "6/1/3".into(),
            step: Some("unit".into()),
            ..Default::default()
        });
        ib.pass(5);
        let o = step(&mut b, &ib, Heard { asks: vec![Ask::Act(cancel.clone()), Ask::Act(retry.clone())], ..able() });
        let to_a: Vec<&Msg> = o.send.iter().filter(|(to, _)| to == A).map(|(_, m)| m).collect();
        assert!(to_a.iter().any(|m| matches!(m, Msg::Cancel(c) if c.target == "6/1/2")), "{:?}", o.send);
        assert!(to_a.iter().any(|m| matches!(m, Msg::Retry(r) if r.target == "6/1/3")), "{:?}", o.send);
        assert!(o.acts.is_empty(), "a member acts on none");
        // The lead: the member's, and its own page's.
        let msgs: Vec<(String, Msg)> =
            o.send.into_iter().filter(|(to, _)| to == A).map(|(_, m)| (B.to_string(), m)).collect();
        ia.pass(5);
        let o = step(&mut a, &ia, Heard { msgs, asks: vec![Ask::Act(retry.clone())], ..able() });
        assert_eq!(o.acts, vec![cancel.clone(), retry.clone(), retry.clone()]);
        // A member on another app than the lead's: refused, said why.
        let mut c = Driver::unlocked(member(C, "MacBook-Pro", "development"), Saved::default());
        let ic = Mac::new(&mem);
        step(&mut c, &ic, able());
        let o = step(&mut c, &ic, asks(Ask::Act(cancel)));
        assert!(!o.send.iter().any(|(_, m)| matches!(m, Msg::Cancel(_))), "{:?}", o.send);
        assert!(
            o.events.iter().any(|e| matches!(e, Event::Failed { what, .. } if what.contains("page's ask"))),
            "{:?}",
            o.events
        );
    }

    #[test]
    fn the_lead_stays_put_unasked_and_moves_by_the_owners_ask() {
        // (Task #497) A leads, B idle and able: hours on, nothing moves the lead by itself; the
        // owner's ask hands it to B.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "MacBook-Air", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        for _ in 0..240 {
            ia.pass(30);
            ib.pass(30);
            step(&mut b, &ib, able());
            let o = step(&mut a, &ia, able());
            assert!(!o.events.iter().any(|e| matches!(e, Event::Handover { .. })), "{:?}", o.events);
            assert_eq!(o.leads, Some(1));
        }
        hand_over(&mut a, &ia, &mut b, &ib);
        ib.pass(5);
        assert_eq!(step(&mut b, &ib, able()).leads, Some(2));
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
        assert!(step(&mut a, &ia, asks(Ask::TakeOver { force: true, downgrade: false }))
            .events
            .iter()
            .any(|e| matches!(e, Event::Waits { what: "take over", why } if why.contains("older"))));
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
            dropped |=
                step(&mut a, &ia, able()).events.iter().any(|e| matches!(e, Event::Handover { what: "dropped", .. }));
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
        step(&mut b, &ib, able());
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
        let o = step(
            &mut b,
            &ib,
            Heard { listed: Some(Listed { n: list.n, keys: journal::list(&mem, None).unwrap() }), ..able() },
        );
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
        assert!(step(&mut a, &ia, able())
            .events
            .iter()
            .any(|e| matches!(e, Event::Made { term: 3, how } if how.starts_with("restarted"))));
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
    fn raw_archives_named_come_off_the_saved_records() {
        // (Task #607) the records held every raw archive ever handed; named ones now come off, saved,
        // and one not yet named stays.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        let pack = |n: &str| crate::rawpack::Pack { name: format!("6-1-1.{n}.tiles"), bytes: 9 };
        let mut e = entry(A, 1, 1, "6-1-1");
        e.handoff.raw = vec![("6-1-1".into(), pack("0000000000000001")), ("6-1-1".into(), pack("0000000000000002"))];
        ia.pass(5);
        step(&mut a, &ia, Heard { entries: vec![e], ..able() });
        for _ in 0..3 {
            ia.pass(5);
            step(&mut a, &ia, able());
        }
        assert_eq!(a.records().unwrap().raw.len(), 2);
        ia.pass(5);
        step(&mut a, &ia, Heard { named_raw: vec![pack("0000000000000001").name], ..able() });
        let saved = Records::load(&mem, 1).unwrap().unwrap();
        assert_eq!(saved.raw, vec![("6-1-1".to_string(), pack("0000000000000002"))]);
        assert_eq!(a.records().unwrap().raw, saved.raw);
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

    /// An entry its check holds (`records::HOLD`: a member's on a newer app, outside this app's
    /// write-sets) is neither merged nor refused, merged again every step, said once, and keeps
    /// no catalog back; once the check takes it (the lead on that app), it's merged.
    #[test]
    fn an_entry_held_is_merged_again_until_taken() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.expect("its take-up's listing");
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }).caught_up);
        let e = entry(B, 1, 3, "6-1-1");
        let key = journal::write(&mem, &e).unwrap();
        let hold = std::cell::Cell::new(true);
        let check = |_: &Entry, _: &Records| {
            if hold.get() {
                Err(format!("{}its member runs a newer app", records::HOLD))
            } else {
                Ok(())
            }
        };
        let said =
            |o: &Out| o.events.iter().filter(|ev| matches!(ev, Event::Waits { what: "merge an entry", .. })).count();
        ia.pass(20);
        let o = a.step(&ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() }, &check);
        assert_eq!(o.held, [(key.clone(), "its member runs a newer app".to_string())]);
        assert!(o.caught_up && said(&o) == 1 && o.send.is_empty(), "{:?}", o.events);
        let r = Records::load(&mem, 1).unwrap().unwrap();
        assert!(!r.handles(&key) && journal::refusal(&mem, &key).unwrap().is_none(), "neither merged nor refused");
        // Merged again the next step, held still, not said again; nothing acknowledged.
        ia.pass(20);
        let o = a.step(&ia, able(), &check);
        assert!(o.held.len() == 1 && said(&o) == 0 && o.send.is_empty(), "{:?}", o.events);
        // Taken: merged, acknowledged to its member, no longer held.
        hold.set(false);
        ia.pass(20);
        let o = a.step(&ia, able(), &check);
        assert!(
            o.held.is_empty() && Records::load(&mem, 1).unwrap().unwrap().reflected.contains(&key),
            "{:?}",
            o.events
        );
        assert!(
            o.send.iter().any(|(to, m)| to == B && matches!(m, Msg::Ack { keys, .. } if keys.contains(&key))),
            "{:?}",
            o.send
        );
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
        assert!(
            !step(&mut a, &ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() }).caught_up,
            "told of it, not whole"
        );
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
        assert!(
            o.send.iter().any(|(to, m)| to == B && matches!(m, Msg::Ack { keys, .. } if keys.contains(&key))),
            "{:?}",
            o.send
        );
    }

    /// Task #630: B tells the lead of its entry 3, which the lead's reads find short (`short`) or
    /// not there, and of entry 4, whole: a step later (`later`), or with 3. Whether entry 3 is
    /// refused 10 minutes on, and the steps' outcomes 5 and 10 minutes on.
    fn told_not_whole(short: bool, later: bool) -> (bool, Out, Out) {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.expect("its take-up's listing");
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }).caught_up);
        let key = entry(B, 1, 3, "6-1-1").key().unwrap();
        if short {
            mem.write_whole(&journal::path(&key), b"{\"member\":").unwrap();
        }
        let next = journal::write(&mem, &entry(B, 1, 4, "6-1-2")).unwrap();
        let tell = |keys: Vec<String>| Heard { msgs: vec![(B.into(), Msg::Tell(keys))], ..able() };
        ia.pass(20);
        let first = if later { vec![key.clone()] } else { vec![key.clone(), next.clone()] };
        assert!(!step(&mut a, &ia, tell(first)).caught_up, "told of it, not whole");
        ia.pass(20);
        step(&mut a, &ia, tell(vec![key.clone(), next.clone()]));
        assert!(Records::load(&mem, 1).unwrap().unwrap().handles(&next), "the later one merged");
        ia.pass(300);
        let at_5 = step(&mut a, &ia, tell(vec![key.clone()]));
        ia.pass(300);
        let at_10 = step(&mut a, &ia, tell(vec![key.clone()]));
        let refused = Records::load(&mem, 1).unwrap().unwrap().rejected.contains_key(&key);
        (refused, at_5, at_10)
    }

    #[test]
    fn an_entry_not_whole_is_refused_in_minutes_once_its_writer_moved_on() {
        // On 24 Oct the lead's own entry read as not there for 40 minutes (its SMB client's stale
        // name; whole on the share), and no catalog went out: one waits for every entry read.
        for short in [true, false] {
            let (refused, at_5, at_10) = told_not_whole(short, true);
            assert!(!at_5.caught_up, "short {short}: 5 minutes: read again");
            assert!(refused && at_10.caught_up, "short {short}: {:?}", at_10.events);
        }
    }

    #[test]
    fn an_entry_told_with_the_one_read_whole_waits_its_hour() {
        // Entry 4 told with entry 3, not after: B's write of 3 may not have returned before 4's.
        for short in [true, false] {
            let (refused, _, at_10) = told_not_whole(short, false);
            assert!(!refused && !at_10.caught_up, "short {short}: {:?}", at_10.events);
        }
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
    fn a_lock_check_that_fails_holds_the_steps_duties_and_checks_again() {
        // The lock's folder unreadable a moment (a stat that fails), no other process holding the
        // lock: this step does no duties, and the next, the folder readable again, does. (Re-review
        // 2, M1: it stopped for good, its term left to the owner.)
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a =
            Driver::new(member(A, "Mac-mini", V1), Saved::default(), MemberLock::take(dir.path(), A).unwrap().unwrap());
        let list = step(&mut a, &ia, able()).list.unwrap();
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: vec![] }), ..able() }).duties);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(o.stop.is_none() && o.leads == Some(1) && !o.duties && !o.caught_up, "{o:?}");
        assert!(
            o.events.iter().any(|e| matches!(e, Event::Failed { what: "check the member's lock", .. })),
            "{:?}",
            o.events
        );
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert!(o.stop.is_none() && o.leads == Some(1) && o.duties && o.caught_up, "{o:?}");
        assert!(MemberLock::take(dir.path(), A).unwrap().is_none(), "still this process's");
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
        let mut a =
            Driver::new(member(A, "Mac-mini", V1), Saved::default(), MemberLock::take(dir.path(), A).unwrap().unwrap());
        let o = step(&mut a, &ia, able());
        assert_eq!((o.leads, o.stop), (Some(1), None));
        std::fs::remove_file(&path).unwrap();
        ia.pass(20);
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1), "taken again: no other process");
        std::fs::remove_file(&path).unwrap();
        let second = MemberLock::take(dir.path(), A).unwrap().expect("a second process");
        ia.pass(20);
        let before = mem.0.borrow().clone();
        let e = entry(A, 1, 9, "6-1-1");
        let o = a.step(&ia, Heard { entries: vec![e.clone()], ..able() }, &any);
        assert!(o.stop.is_some() && o.leads.is_none() && o.send.is_empty(), "{o:?}");
        assert!(o.events.iter().any(|e| matches!(e, Event::SteppedDown { term: 1, .. })), "{:?}", o.events);
        assert!(
            a.saved().mine.unwritten().any(|k| *k == e.key().unwrap()),
            "the hand-off handed to it kept, for the agent to save"
        );
        ia.pass(20);
        assert!(a.step(&ia, able(), &any).stop.is_some(), "for good");
        assert_eq!(*mem.0.borrow(), before, "nothing written");
        drop(second);
    }

    #[test]
    fn a_lead_gone_silent_is_taken_over_within_the_bound_and_steps_down_when_back() {
        // A leads, then is gone (its Mac at a login screen after a restart, 19 Oct): B and C step
        // every 20 s. One of them takes over by itself once A's beat is `SILENT_S` old and seen so
        // for `SILENT_SEEN_S`, within that plus the ranks' turns; B first (lowest id).
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib, ic) = (Mac::new(&mem), Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let mut c = Driver::unlocked(member(C, "iMac", V1), Saved::default());
        let out = step(&mut a, &ia, able());
        assert_eq!(out.leads, Some(1));
        let gone = ia.now();
        let mut took = None;
        for i in 0..90u64 {
            for (d, io) in [(&mut b, &ib), (&mut c, &ic)] {
                io.pass(20);
                if step(d, io, able()).leads.is_some() {
                    took.get_or_insert((i, io.now()));
                }
            }
        }
        let (_, at) = took.expect("taken over by itself");
        let bound = SILENT_S + 20 + SILENT_SEEN_S + 2 * AUTO_RANK_S + 40;
        assert!(at - gone <= bound, "taken over {} s after the lead went silent, past {bound}", at - gone);
        assert!(at - gone > SILENT_S, "not before its heartbeat was silent");
        assert_eq!(b.leads(), Some(2), "B (the lowest id) first");
        assert_eq!(c.leads(), None, "one lead");
        let t = term::read(&mem, 2).unwrap().unwrap();
        assert!(t.how.contains("silent"), "{}", t.how);
        // A comes back (its clocks moved together: a process held up, not asleep): it re-asserts,
        // finds term 2, and steps down without a step that grants.
        ia.pass(at - gone + 60);
        let back = step(&mut a, &ia, able());
        assert!(!back.duties, "back, it grants nothing");
        assert_eq!(back.leads, None);
        assert_eq!(back.term, 2);
        assert!(term::read(&mem, 3).unwrap().is_none(), "its re-assertion lost to B's term");
        assert!(back.events.iter().any(|e| matches!(e, Event::SteppedDown { term: 1, .. })), "{:?}", back.events);
        ib.pass(20);
        assert_eq!(step(&mut b, &ib, able()).leads, Some(2), "B keeps the lead");
    }

    #[test]
    fn a_lead_that_lost_the_nas_while_online_is_taken_over() {
        // (Task #497) A loses the NAS (away, its share unmounted) but stays online: its heartbeat
        // stops. While its coordinator's answers still say its term, B holds off ("late, not
        // gone"); once they say none (crate::coord's `set_member(None)`, so `answered` is None), B
        // takes over within the silent bound.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        for _ in 0..90 {
            ib.pass(20);
            let answered = Some(ib.now());
            assert_eq!(step(&mut b, &ib, Heard { answered, ..able() }).leads, None, "answering: late, not gone");
        }
        let lost = ib.now();
        let mut took = None;
        for _ in 0..90 {
            ib.pass(20);
            if step(&mut b, &ib, able()).leads.is_some() {
                took.get_or_insert(ib.now());
            }
        }
        let at = took.expect("taken over");
        assert!(at - lost <= SILENT_SEEN_S + 2 * AUTO_RANK_S + 40, "{} s", at - lost);
        assert!(term::read(&mem, 2).unwrap().unwrap().how.contains("silent"));
    }

    #[test]
    fn a_lead_whose_beats_dont_land_grants_nothing_before_a_member_takes_over() {
        // A keeps stepping, every other NAS operation landing, but its heartbeat's writes fail
        // (and B can't reach its coordinator): A stops granting once its own beat is `LEAD_BEAT_S`
        // old, before B takes over; then steps down at its next step.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let two = || Heard { members: vec![A.into(), B.into()], ..able() };
        assert_eq!(step(&mut a, &ia, two()).leads, Some(1));
        let mut held_at = None;
        for _ in 0..90 {
            ia.pass(20);
            let o = a.step(&ia, two(), &any);
            if o.leads.is_some() && !o.duties {
                held_at.get_or_insert(ia.now());
            }
            if o.leads.is_some() && o.duties {
                assert!(b.leads().is_none(), "A grants while B leads");
            }
            ib.pass(20);
            step(&mut b, &ib, two());
        }
        assert_eq!(b.leads(), Some(2), "B took over");
        assert_eq!(a.leads(), None, "A stepped down");
        let held = held_at.expect("A held its grants");
        let took = term::read(&mem, 2).unwrap().unwrap().since;
        assert!(held < took, "A stopped granting at {held}, before B's term at {took}");
    }

    #[test]
    fn a_lead_whose_beats_dont_land_then_hangs_stops_granting_before_the_takeover() {
        // The worst case: A's beat writes fail, it steps (granting) until its own-beat check
        // trips, and its last granting step is followed by a hang: its coordinator, told it may
        // grant at that step, grants on for `STALL_S` (crate::coord's `told`) and a step. B's
        // takeover comes after all of that.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let two = || Heard { members: vec![A.into(), B.into()], ..able() };
        assert_eq!(step(&mut a, &ia, two()).leads, Some(1));
        let mut last_grant = ia.now();
        let mut hung = false;
        let mut took = None;
        for _ in 0..120 {
            if !hung {
                ia.pass(20);
                let o = a.step(&ia, two(), &any);
                if o.duties {
                    last_grant = ia.now();
                } else {
                    hung = true;
                }
            }
            ib.pass(20);
            if step(&mut b, &ib, two()).leads.is_some() {
                took.get_or_insert(ib.now());
            }
        }
        let took = took.expect("B took over");
        let grants_until = last_grant + STALL_S + crate::agent::pool::TICK_S;
        assert!(took > grants_until, "B's term at {took}, A's coordinator granting until {grants_until}");
    }

    #[test]
    fn a_healthy_lead_whose_clock_lags_keeps_the_lead() {
        // A's clock 9 min behind B's: its fresh beat looks 9 min old to B, but changes every
        // couple of minutes, so B never watches it unchanged for `SILENT_SEEN_S`.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        ia.wall.set(ia.wall.get() - 540);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let two = || Heard { members: vec![A.into(), B.into()], ..able() };
        assert_eq!(step(&mut a, &ia, two()).leads, Some(1));
        for i in 0..180u64 {
            ia.pass(20);
            // (Its heartbeat written every `BEAT_EVERY_S`, as the agent does, not every step.)
            if i % (super::super::beat::BEAT_EVERY_S / 20) == 0 {
                step(&mut a, &ia, two());
            } else {
                a.step(&ia, two(), &any);
            }
            ib.pass(20);
            assert_eq!(step(&mut b, &ib, two()).leads, None);
        }
        assert_eq!(a.leads(), Some(1));
    }

    #[test]
    fn a_slow_lead_or_one_answering_keeps_the_lead() {
        // A steps every 8 minutes (a NAS under load: it re-asserts each time, by itself): its beat
        // is never `SILENT_S` old, and no member takes over.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let two = || Heard { members: vec![A.into(), B.into()], ..able() };
        assert_eq!(step(&mut a, &ia, two()).leads, Some(1));
        for _ in 0..6 {
            for _ in 0..24 {
                ib.pass(20);
                assert_eq!(step(&mut b, &ib, two()).leads, None);
            }
            ia.pass(480);
            step(&mut a, &ia, two());
        }
        assert!(a.leads().is_some() && b.leads().is_none(), "a slow lead keeps it");
        let e = a.leads().unwrap();
        // Silent past the bound, but its coordinator answered B lately: late, not gone.
        for _ in 0..60 {
            ib.pass(20);
            let answered = Some(ib.now() - 30);
            assert_eq!(step(&mut b, &ib, Heard { answered, ..two() }).leads, None);
        }
        assert!(term::read(&mem, e + 1).unwrap().is_none(), "a lead answering isn't gone");
        // Silent, and B's own heartbeat not landing either (the NAS slow for every Mac): nothing.
        let mut own = Beat::read(&mem, B).unwrap().unwrap();
        own.beat = ib.now() - OWN_BEAT_S - 1;
        own.write(&ib).unwrap();
        for _ in 0..60 {
            ib.pass(20);
            assert_eq!(b.step(&ib, two(), &any).leads, None);
        }
        assert!(term::read(&mem, e + 1).unwrap().is_none(), "B's own beat stale: no takeover");
        // B's writes land again: it takes over.
        for _ in 0..40 {
            ib.pass(20);
            step(&mut b, &ib, two());
        }
        assert_eq!(b.leads(), Some(e + 1));
    }

    #[test]
    fn a_silent_lead_isnt_taken_over_by_a_member_that_cant_lead() {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        let two = |able| Heard { members: vec![A.into(), B.into()], able, ..Default::default() };
        assert_eq!(step(&mut a, &ia, two(true)).leads, Some(1));
        for _ in 0..90 {
            ib.pass(20);
            assert_eq!(step(&mut b, &ib, two(false)).leads, None, "paused, on battery or short of room");
        }
        for _ in 0..90 {
            ib.pass(20);
            assert_eq!(step(&mut b, &ib, two(true)).leads, None, "the app rule: older than the lead's");
        }
        assert!(term::read(&mem, 2).unwrap().is_none());
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
        assert!(
            (STOOD_DOWN_S + AUTO_RANK_S..STOOD_DOWN_S + AUTO_RANK_S + 20).contains(&took),
            "{took} s after it saw the stand-down"
        );
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
    fn entries_found_not_whole_are_read_in_turn() {
        // Forty entries B told the lead of, cut short, on a share taking five seconds an operation:
        // a step reads a few of them. Once the last by lease is whole, a step reads it within a
        // few: those found not whole are read in turn. (Read each step from the first, it waited
        // until the others were refused, an hour on: seed 1003691 of the four-hour runs.)
        let mem = Mem::default();
        setup(&mem);
        let io = Slow(Mac::new(&mem), Cell::new(0));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = a.step(&io, able(), &any).list.unwrap();
        io.0.pass(20);
        a.step(&io, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }, &any);
        let entries: Vec<Entry> = (1..=40).map(|n| entry(B, 1, n, "6-1-1")).collect();
        let keys: Vec<String> = entries.iter().map(|e| e.key().unwrap()).collect();
        for k in &keys {
            mem.write_whole(&journal::path(k), b"{\"member\":").unwrap();
        }
        io.1.set(5);
        io.0.pass(20);
        a.step(&io, Heard { msgs: vec![(B.into(), Msg::Tell(keys.clone()))], ..able() }, &any);
        for _ in 0..10 {
            io.0.pass(20);
            a.step(&io, able(), &any);
        }
        // Each read and found not whole by now; the last made whole.
        journal::write(&mem, &entries[39]).unwrap();
        let mut steps = 0;
        while !Records::load(&mem, 1).unwrap().unwrap().handles(&keys[39]) {
            steps += 1;
            assert!(steps <= 8, "not merged in eight steps");
            io.0.pass(20);
            a.step(&io, able(), &any);
        }
        let r = Records::load(&mem, 1).unwrap().unwrap();
        assert!(keys[..39].iter().all(|k| !r.handles(k)), "the others still waiting");
    }

    #[test]
    fn rr2_a_read_that_always_fails_for_one_entry_is_refused_after_an_hour() {
        // One entry's reads fail every time (a file the share errors on: EIO, EACCES), the share
        // answering every other read. (Re-review 2, L1: each failed read started its hour again,
        // so it was never refused: never caught up, no handover settled, and nothing said why.)
        // Said at ten minutes, refused at sixty, caught up after.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: vec![] }), ..able() }).caught_up);
        let key = journal::write(&mem, &entry(C, 1, 3, "6-1-1")).unwrap();
        *ia.fail.borrow_mut() = Some(key.clone());
        ia.down.set(true);
        ia.pass(20);
        step(&mut a, &ia, Heard { msgs: vec![(C.into(), Msg::Tell(vec![key.clone()]))], ..able() });
        let (mut said, mut caught) = (None, None);
        for i in 1..=200 {
            ia.pass(20);
            let o = step(&mut a, &ia, able());
            if o.events
                .iter()
                .any(|e| matches!(e, Event::Waits { what: "read an entry whole", why } if why.contains(&key)))
            {
                said.get_or_insert(i);
            }
            if o.caught_up {
                caught.get_or_insert(i);
            }
        }
        assert!(said.is_some_and(|i| (30..=31).contains(&i)), "said at ten minutes: step {said:?}");
        assert!(caught.is_some_and(|i| (180..=181).contains(&i)), "caught up at sixty: step {caught:?}");
        let why = Records::load(&mem, 1).unwrap().unwrap().rejected.get(&key).cloned();
        assert!(why.is_some_and(|w| w.contains("not read whole")), "refused");
    }

    #[test]
    fn rr_the_hour_rule_and_a_share_that_doesnt_answer() {
        // B tells A of an entry; the share answers none of A's reads for two hours (away): not
        // refused; then read whole, applied. (The re-review's.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: vec![] }), ..able() }).caught_up);
        let key = journal::write(&mem, &entry(B, 1, 3, "6-1-1")).unwrap();
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
        // The entry reads not whole once (not there yet as A reads it), then the share answers none
        // of A's reads for 70 minutes (away). (Re-review F1: refused at minute 60, on time alone,
        // though it was whole on the share all along: an hour's reads that answer count, and a read
        // that fails, none answering, starts the hour again.)
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
        ia.down.set(true);
        for _ in 0..70 {
            ia.pass(60);
            step(&mut a, &ia, able());
            assert!(
                !Records::load(&mem, 1).unwrap().unwrap().rejected.contains_key(&key),
                "refused while every read of it failed"
            );
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
        assert!(
            step(
                &mut a,
                &ia,
                Heard { listed: Some(Listed { n: list.n, keys: journal::list(&mem, None).unwrap() }), ..able() }
            )
            .caught_up
        );
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
                step(
                    &mut a,
                    &ia,
                    Heard {
                        listed: Some(Listed { n: l.n, keys: journal::list(&mem, l.since.as_deref()).unwrap() }),
                        ..able()
                    },
                );
            }
            if merged.is_none() && Records::newest(&mem, a.leads().unwrap(), false).unwrap().unwrap().handles(&key) {
                merged = Some(i);
            }
        }
        assert!(sweeps >= 10, "{sweeps}");
        assert!(merged.is_some_and(|i| i < 6 * 24), "merged within a day: {merged:?}");
    }

    #[test]
    fn a_lead_lists_every_day_and_is_caught_up_only_by_a_listing_less_than_a_day_old() {
        // Its take-up's listing merged, its sweeps' handed back; the daily one asked for an hour
        // ahead and kept out: not caught up once its listing is a day old, nothing asked for
        // meanwhile; caught up once it's handed back. (Re-review F3; re-review 2, M2.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let list = step(&mut a, &ia, able()).list.unwrap();
        let at = ia.now();
        ia.pass(20);
        assert_eq!(
            step(&mut a, &ia, Heard { listed: Some(Listed { n: list.n, keys: Vec::new() }), ..able() }).listed_at,
            Some(at)
        );
        let (mut daily, mut heard) = (None, able());
        while ia.now() < at + RELIST_S {
            ia.pass(300);
            let o = step(&mut a, &ia, std::mem::replace(&mut heard, able()));
            assert_eq!(o.caught_up, ia.now() < at + RELIST_S, "{} s after its listing", ia.now() - at);
            match o.list {
                Some(_) if daily.is_some() => panic!("asked for a listing while one is out"),
                Some(l) if l.since.is_none() => daily = Some((ia.now() - at, l)),
                Some(l) => heard = Heard { listed: Some(Listed { n: l.n, keys: Vec::new() }), ..able() },
                None => {}
            }
        }
        let (when, l) = daily.expect("asked for");
        assert!(when + AHEAD_S >= RELIST_S && when < RELIST_S, "an hour ahead: {when}");
        for _ in 0..6 {
            ia.pass(300);
            assert!(step(&mut a, &ia, able()).list.is_none(), "nothing asked for while it's out");
        }
        ia.pass(20);
        assert!(step(&mut a, &ia, Heard { listed: Some(Listed { n: l.n, keys: Vec::new() }), ..able() }).caught_up);
    }

    #[test]
    fn rr2_a_listing_of_every_day_slower_than_listing_s_counts() {
        // The agent makes one listing at a time, each of every day taking 20 minutes (a journal of
        // 36 day folders at 33 s a folder, no GC yet), a sweep's two. (Re-review 2, M2: asked again
        // after 15 minutes, each listing came back under an older ask's number, merged as any: the
        // lead was never caught up.) None is asked for while one is out, and each counts.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut queue: std::collections::VecDeque<(Listing, u64)> = Default::default();
        let mut running: Option<(Listing, u64)> = None;
        let (mut first, mut last, mut most, mut heard) = (None, false, 0, able());
        for i in 0..(3 * 60 * 24 * 2) {
            let o = step(&mut a, &ia, std::mem::replace(&mut heard, able()));
            if o.caught_up {
                first.get_or_insert(i);
            }
            last = o.caught_up;
            if let Some(l) = o.list {
                let d = if l.since.is_none() { 1200 } else { 120 };
                queue.push_back((l, d));
            }
            most = most.max(queue.len() + usize::from(running.is_some()));
            ia.pass(20);
            if running.is_none() {
                running = queue.pop_front().map(|(l, d)| (l, ia.now() + d));
            }
            if let Some((l, done)) = running.take() {
                if ia.now() >= done {
                    heard = Heard {
                        listed: Some(Listed { n: l.n, keys: journal::list(&mem, l.since.as_deref()).unwrap() }),
                        ..able()
                    };
                } else {
                    running = Some((l, done));
                }
            }
        }
        assert!(first.is_some_and(|i| i <= 62), "caught up once its take-up's listing is back: step {first:?}");
        assert!(last, "two days on, its daily listings back");
        assert_eq!(most, 1, "one listing out at a time");
    }

    #[test]
    fn a_listing_lost_is_asked_for_again_once_overdue() {
        // A lead that never sleeps (the M4, on power) whose agent loses its take-up's
        // listing, never handing it back. (Re-review 3: it was never caught up again, so no
        // catalog and no GC, and nothing said why.) Nothing else is asked for while it's out; two
        // hours on with none back, it's said and asked for again; once that one's back it's caught
        // up, and the first, back late after all, counts for no more.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let lost = step(&mut a, &ia, able()).list.unwrap();
        let (mut again, mut said) = (None, 0);
        while again.is_none() {
            assert!(ia.now() < T0 + OVERDUE_S + 60, "not asked for again");
            ia.pass(20);
            let o = step(&mut a, &ia, able());
            assert!(!o.caught_up);
            said += o.events.iter().filter(|e| matches!(e, Event::Waits { what: "have a listing back", .. })).count();
            again = o.list;
        }
        let again = again.unwrap();
        assert!(
            ia.now() >= T0 + OVERDUE_S && again.since.is_none() && again.n > lost.n,
            "every day, two hours on: {again:?}"
        );
        assert_eq!(said, 1, "said once");
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { listed: Some(Listed { n: again.n, keys: Vec::new() }), ..able() });
        assert!(o.caught_up, "{:?}", o.events);
        let at = o.listed_at;
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { listed: Some(Listed { n: lost.n, keys: Vec::new() }), ..able() });
        assert!(o.caught_up && o.listed_at == at, "{:?}", o.listed_at);
    }

    #[test]
    fn rr2_term_ones_fallback_to_todays_files_pairs_versions_under_stale_reads() {
        // Seed 3090226 of 100,000 four-hour schedules (re-review 2, H1): term 1's first snapshot's
        // create cut short; term 1's lead saves twice, writing today's three files after each; the
        // owner has B take over, its reads stale: term 1's lead's snapshot not there yet, its
        // first as it was cut (empty), `jobs.json` as it was before the last save, `manifest.json`
        // as it is now. Falling back to today's files at once paired a manifest of one version with
        // keys of another (invariant 4). Term 1's maker now writes its first snapshot whole before
        // term 1 is made, and a take-up that can't read it whole tries again, until its reads
        // aren't stale.
        struct Stale<'a>(&'a Mem, Vec<(String, Option<Vec<u8>>)>);
        impl Nas for Stale<'_> {
            fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
                self.0.create_new(path, bytes)
            }
            fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
                self.0.write_whole(path, bytes)
            }
            fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match self.1.iter().find(|(p, _)| p == path) {
                    Some((_, b)) => b.clone(),
                    None => self.0.read(path)?,
                })
            }
            fn exists(&self, path: &str) -> Result<bool> {
                self.0.exists(path)
            }
            fn list(&self, dir: &str) -> Result<Vec<String>> {
                self.0.list(dir)
            }
            fn remove(&self, path: &str) -> Result<()> {
                self.0.remove(path)
            }
        }
        /// Its create of term 1's first snapshot cut short: made, its bytes not landed.
        struct Cut<'a>(&'a Mem);
        impl Nas for Cut<'_> {
            fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
                if path != records::FIRST {
                    return self.0.create_new(path, bytes);
                }
                self.0.create_new(path, b"")?;
                Ok(Created::Unwritten(anyhow::anyhow!("write {path} (made, and left short)")))
            }
            fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
                self.0.write_whole(path, bytes)
            }
            fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
                self.0.read(path)
            }
            fn exists(&self, path: &str) -> Result<bool> {
                self.0.exists(path)
            }
            fn list(&self, dir: &str) -> Result<Vec<String>> {
                self.0.list(dir)
            }
            fn remove(&self, path: &str) -> Result<()> {
                self.0.remove(path)
            }
        }
        let mem = Mem::default();
        setup(&mem);
        let t1 = term::bootstrap(&Cut(&mem), &member(A, "Mac-mini", V1), T0, false).unwrap().unwrap();
        assert!(Records::newest(&mem, 1, false).unwrap().is_some(), "written whole before term 1 is made");
        let mut r = records::start(&mem, &t1, None, false).unwrap();
        r.save(&mem).unwrap();
        let old_jobs = mem.read("state/build/jobs.json").unwrap().unwrap();
        let e = entry(A, 1, 1, "6-1-4");
        r.apply(&e.key().unwrap(), &e);
        r.save(&mem).unwrap();
        let t2 = term::forced(
            &mem,
            &term::current(&mem).unwrap(),
            &member(B, "MacBook-Air", V1),
            "taken over by MacBook-Air",
            T0 + 60,
            false,
        )
        .unwrap();
        let stale = Stale(
            &mem,
            vec![
                (records::path(1), None),
                (records::FIRST.into(), Some(Vec::new())),
                ("state/build/jobs.json".into(), Some(old_jobs)),
            ],
        );
        let err = records::start(&stale, &t2, None, false).unwrap_err();
        assert!(err.is::<records::FirstUnreadable>(), "tried again: {err:#}");
        let r2 = records::start(&mem, &t2, None, false).unwrap();
        let six = (r2.manifest.get("base/6-1-4").map(String::as_str), r2.keys.unit.get("6-1-4").map(String::as_str));
        assert_eq!(six, (Some("base/6-1-4.k1.base"), Some("k1")), "one version, its last save's");
    }

    #[test]
    fn term_ones_first_snapshot_never_whole_is_taken_from_todays_files_after_ten_minutes() {
        // Term 1 made by a Mac that left before its first snapshot's bytes landed: A, named by term
        // 1, can't read it whole, tries again, and takes term 1 up from today's files once it has
        // stayed so `STALE_S` awake. (Re-review 2, H1.)
        let mem = Mem::default();
        setup(&mem);
        mem.create_new(records::FIRST, b"").unwrap();
        let t1 = Term {
            term: 1,
            member: A.into(),
            host: "Mac-mini".into(),
            app: V1.into(),
            since: T0,
            how: "the first Mac in the pool".into(),
            from: 0,
            seq: None,
        };
        mem.create_new(&term::path(1), &serde_json::to_vec_pretty(&t1).unwrap()).unwrap();
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved { member: A.into(), ..Default::default() });
        let led = (0..40).find(|_| {
            let o = step(&mut a, &ia, able());
            ia.pass(20);
            o.leads.is_some()
        });
        assert_eq!(led, Some(30), "after ten minutes, not before");
        assert_eq!(Records::load(&mem, 1).unwrap().map(|r| r.seq), Some(2), "from today's files, saved whole");
    }

    #[test]
    fn a_view_the_share_cut_short_is_kept_in_the_saved_state() {
        // A learns of term 2, then the share doesn't answer its look at term 3: its step says term
        // 2, and the state it saves keeps it. (Re-review 2's failing reads, in the simulator: the
        // state kept the term before, and a restart went back to it.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        for e in [2, 3] {
            let t = Term {
                term: e,
                member: B.into(),
                host: "MacBook-Air".into(),
                app: V1.into(),
                since: T0 + e,
                how: "taken over by MacBook-Air".into(),
                from: e - 1,
                seq: None,
            };
            mem.create_new(&term::path(e), &serde_json::to_vec_pretty(&t).unwrap()).unwrap();
        }
        *ia.fail.borrow_mut() = Some("terms/3".into());
        ia.down.set(true);
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert_eq!((o.term, a.saved().term), (2, 2), "{:?}", o.events);
    }

    #[test]
    fn a_saved_state_restored_from_a_backup_re_asserts_a_term_it_made() {
        // A leads term 1, then re-asserts term 2 and leads it (granting leases 2-1, 2-2, …). Its
        // agent's folder is then restored from a backup taken while it led term 1 (Time Machine):
        // the saved state is A's own, older than the process that made term 2. (Re-review 2, M3:
        // the new process took term 2 up again, as new, and could grant 2-1 twice.) It re-asserts.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        let backup = a.saved();
        assert_eq!((backup.led, backup.made), (1, 1));
        ia.pass(20);
        assert_eq!(step(&mut a, &ia, Heard { reassert: true, ..able() }).leads, Some(2));
        ia.pass(20);
        let mut restored = Driver::unlocked(member(A, "Mac-mini", V1), backup);
        let o = step(&mut restored, &ia, able());
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
        assert!(
            o.events.iter().any(|e| matches!(e, Event::Made { term: 3, how } if how.contains("older than the term"))),
            "{:?}",
            o.events
        );
        assert!(!o.events.iter().any(|e| matches!(e, Event::TookUp { term: 2, .. })), "{:?}", o.events);
    }

    #[test]
    fn a_saved_state_restored_from_a_backup_re_asserts_a_handover_term_it_took_up() {
        // A hands term 1 over to B, which takes term 2 up and leads it (granting leases 2-1, 2-2,
        // …). B's agent's folder is then restored from a backup taken before (Time Machine): the
        // state doesn't know it took term 2 up, and term 2 isn't one it made. (Re-review 3: it took
        // term 2 up again, from the snapshot the handover names, and could grant 2-1 twice.) Term
        // 2's records are there: it re-asserts.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        let backup = b.saved();
        hand_over(&mut a, &ia, &mut b, &ib);
        ib.pass(20);
        assert_eq!(step(&mut b, &ib, able()).leads, Some(2));
        ib.pass(20);
        let mut restored = Driver::unlocked(member(B, "MacBook-Air", V1), backup);
        let o = step(&mut restored, &ib, able());
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
        assert!(
            o.events.iter().any(|e| matches!(e, Event::Made { term: 3, how } if how.contains("older than the term"))),
            "{:?}",
            o.events
        );
        assert!(!o.events.iter().any(|e| matches!(e, Event::TookUp { term: 2, .. })), "{:?}", o.events);
    }

    #[test]
    fn a_take_up_whose_save_landed_unanswered_is_tried_again() {
        // B takes up the term A handed it, and its save of the records lands with its answer lost:
        // the records there are this process's own try's, taken up again next step, numbered on,
        // the coordinator's state with them. (Not re-asserted, as a term whose records are there
        // is when its state is older: a backup restored.)
        struct Lost<'a>(&'a Mac<'a>, Cell<bool>);
        impl Nas for Lost<'_> {
            fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
                self.0.create_new(path, bytes)
            }
            fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
                self.0.write_whole(path, bytes)?;
                if path == records::path(2) && self.1.replace(false) {
                    anyhow::bail!("write {path}: done, its answer lost");
                }
                Ok(())
            }
            fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
                self.0.read(path)
            }
            fn exists(&self, path: &str) -> Result<bool> {
                self.0.exists(path)
            }
            fn list(&self, dir: &str) -> Result<Vec<String>> {
                self.0.list(dir)
            }
            fn remove(&self, path: &str) -> Result<()> {
                self.0.remove(path)
            }
        }
        impl Io for Lost<'_> {
            fn now(&self) -> u64 {
                self.0.now()
            }
            fn awake(&self) -> u64 {
                self.0.awake()
            }
        }
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        hand_over(&mut a, &ia, &mut b, &ib);
        ib.pass(20);
        let o = b.step(&Lost(&ib, Cell::new(true)), able(), &any);
        assert!(o.leads.is_none() && Records::load(&mem, 2).unwrap().is_some_and(|r| r.seq == 1), "{:?}", o.events);
        ib.pass(20);
        let o = step(&mut b, &ib, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::TookUp { term: 2, handed: Some(_), .. })), "{:?}", o.events);
        assert_eq!(Records::load(&mem, 2).unwrap().map(|r| r.seq), Some(2), "numbered on");
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
    fn a_term_left_empty_is_taken_past_by_itself_after_half_an_hour() {
        // 12 Oct: an SMB drop lost the answer to the M4's create of term 2, which landed 0 bytes.
        // No Mac could read it, so none led, until the owner's force three days on. Now the
        // members that can lead take it past by themselves once it's been unreadable half an hour
        // awake: the first by rank, the create deciding.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        step(&mut b, &ib, able());
        mem.create_new(&term::path(2), b"").unwrap();
        // (The members both know: C was never one, and an unknown heartbeat would hold it.)
        let ab = || Heard { members: vec![A.into(), B.into()], ..able() };
        let mut took = None;
        for i in 0..10 {
            for (d, io) in [(&mut a, &ia), (&mut b, &ib)] {
                io.pass(300);
                let o = step(d, io, ab());
                if let Some(e) = o.leads {
                    took.get_or_insert((d.member().id.clone(), e, i));
                }
            }
        }
        let (who, e, i) = took.expect("taken past");
        assert_eq!((who.as_str(), e), (A, 3), "the lowest member id first");
        // (Seen first at round 0, so half an hour on is round 6.)
        assert_eq!(i, 6, "after half an hour, not before");
        let t = term::read(&mem, 3).unwrap().unwrap();
        assert_eq!(
            (t.from, t.how.as_str()),
            (2, "taken over by Mac-mini: term 2 unreadable for 30 min (forced past: term 2 unreadable)")
        );
        assert!(!mem.exists(&term::path(4)).unwrap(), "B follows term 3; it makes none of its own");
        assert_eq!(b.current().lead.as_ref().map(|t| t.member.as_str()), Some(A));
    }

    #[test]
    fn a_term_left_empty_isnt_taken_past_by_a_mac_that_cant_lead_nor_once_it_reads_whole() {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        // Term 2 made, its bytes landing twenty minutes on (its maker, C, finishing it): it's
        // led, and no one takes it past.
        mem.create_new(&term::path(2), b"").unwrap();
        let t2 = Term {
            term: 2,
            member: C.into(),
            host: "iMac".into(),
            app: V1.into(),
            since: T0,
            how: "re-asserted after a gap".into(),
            from: 1,
            seq: None,
        };
        for i in 0..12 {
            if i == 4 {
                mem.write_whole(&term::path(2), &serde_json::to_vec(&t2).unwrap()).unwrap();
            }
            for (d, io) in [(&mut a, &ia), (&mut b, &ib)] {
                io.pass(300);
                step(d, io, Heard { members: vec![A.into(), B.into()], ..able() });
            }
        }
        assert!(!mem.exists(&term::path(3)).unwrap(), "read whole: never taken past");
        assert_eq!(a.current().lead.as_ref().map(|t| t.member.as_str()), Some(C));
        // Term 3 left empty, neither able to lead now (away, on battery): an hour on, nothing.
        mem.create_new(&term::path(3), b"").unwrap();
        let unable = || Heard { able: false, members: vec![A.into(), B.into()], ..able() };
        for _ in 0..12 {
            for (d, io) in [(&mut a, &ia), (&mut b, &ib)] {
                io.pass(300);
                assert_eq!(step(d, io, unable()).leads, None);
            }
        }
        assert!(!mem.exists(&term::path(4)).unwrap());
    }

    #[test]
    fn a_live_lead_whose_term_reads_short_to_one_member_isnt_taken_past() {
        // (Review of #195, H1: one member's reads of the term short while its lead leads it.)
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        *ib.short.borrow_mut() = Some("terms/1.json".into());
        for _ in 0..12 {
            for (d, io) in [(&mut a, &ia), (&mut b, &ib)] {
                io.pass(300);
                step(d, io, able());
            }
        }
        assert!(b.current().lead.is_none(), "B reads it short throughout");
        assert!(!mem.exists(&term::path(2)).unwrap(), "A's heartbeat says it leads term 1: never taken past");
        assert_eq!(a.leads(), Some(1));
    }

    #[test]
    fn a_term_read_short_isnt_taken_past_while_a_members_heartbeat_cant_be_read_fresh() {
        // (Re-review of #195: the veto fails closed.) B reads term 1 short, and A's heartbeat
        // empty, then stale: an hour on, no term 2.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        *ib.short.borrow_mut() = Some("terms/1.json".into());
        let only = || Heard { members: vec![A.into(), B.into()], ..able() };
        // Empty reads of A's heartbeat for half an hour (A goes on leading, beating).
        *ib.short.borrow_mut() = Some(format!("terms/1.json|members/{A}"));
        for _ in 0..6 {
            for (d, io) in [(&mut a, &ia), (&mut b, &ib)] {
                io.pass(300);
                step(d, io, only());
            }
        }
        // Then A's heartbeat stale (A gone silent) for an hour more, B's reads of term 1 short.
        *ib.short.borrow_mut() = Some("terms/1.json".into());
        for _ in 0..12 {
            ib.pass(300);
            step(&mut b, &ib, only());
        }
        assert!(b.current().lead.is_none() && !mem.exists(&term::path(2)).unwrap(), "left to the owner");
        // And with no other member known (a listing that failed), not either.
        let mem = Mem::default();
        setup(&mem);
        let ib = Mac::new(&mem);
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut b, &ib, able());
        mem.create_new(&term::path(2), b"").unwrap();
        for _ in 0..12 {
            ib.pass(300);
            step(&mut b, &ib, Heard { members: vec![B.into()], ..able() });
        }
        assert!(!mem.exists(&term::path(3)).unwrap());
    }

    #[test]
    fn a_term_whose_reads_fail_isnt_taken_past() {
        // (Review of #195, H2: reads that fail say nothing of the term, and start the time again.)
        let mem = Mem::default();
        setup(&mem);
        let ib = Mac::new(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        mem.create_new(&term::path(2), b"").unwrap();
        ib.down.set(true);
        *ib.fail.borrow_mut() = Some("terms/2.json".into());
        // (A beats on, unable to lead: only B's reads count.)
        let (abl, una) = (
            || Heard { members: vec![A.into(), B.into()], ..able() },
            || Heard { able: false, members: vec![A.into(), B.into()], ..able() },
        );
        for _ in 0..12 {
            ia.pass(300);
            step(&mut a, &ia, una());
            ib.pass(300);
            assert_eq!(step(&mut b, &ib, abl()).leads, None);
        }
        assert!(!mem.exists(&term::path(3)).unwrap());
        // Short reads again from now: half an hour from now, not at once.
        ib.down.set(false);
        ia.pass(300);
        step(&mut a, &ia, una());
        ib.pass(300);
        assert_eq!(step(&mut b, &ib, abl()).leads, None, "the time starts again");
        assert!(b.unread_since().is_some());
        // (Half an hour, and A's turn first: its heartbeat, in touch, ranks it before B.)
        for _ in 0..7 {
            ia.pass(300);
            step(&mut a, &ia, una());
            ib.pass(300);
            step(&mut b, &ib, abl());
        }
        assert_eq!(b.leads(), Some(3));
    }

    #[test]
    fn a_term_made_past_its_unfinished_one_isnt_finished_late() {
        // (Review of #195, M4.)
        let mem = Mem::default();
        setup(&mem);
        let ib = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &Mac::new(&mem), able());
        mem.create_new(&term::path(2), b"").unwrap();
        let t2 = Term {
            term: 2,
            member: B.into(),
            host: "MacBook-Air".into(),
            app: V1.into(),
            since: T0,
            how: "re-asserted after a gap".into(),
            from: 1,
            seq: None,
        };
        mem.create_new(&term::path(3), b"{}").unwrap();
        assert!(term::finish(&mem, &t2).is_err(), "term 3 past it");
        let mut b = Driver::unlocked(
            member(B, "MacBook-Air", V1),
            Saved { member: B.into(), unfinished: Some(t2), term: 2, ..Default::default() },
        );
        let o = step(&mut b, &ib, able());
        assert!(o.events.iter().any(|e| matches!(e, Event::Waits { what: "finish the term it made", why } if why.contains("left unfinished"))), "{:?}", o.events);
        assert!(b.saved().unfinished.is_none());
        assert_eq!(mem.read(&term::path(2)).unwrap(), Some(Vec::new()), "left as it was");
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
        // (§6.8) C reports it can't reach B's broker: B isn't handed the lead, saying why; its
        // report gone, it is again.
        let mut bc = Beat::read(&mem, C).unwrap().unwrap();
        let clean = bc.clone();
        bc.reach = vec![super::super::reachable::Reach {
            member: B.into(),
            what: "broker".into(),
            ok: false,
            at: bc.beat,
            since: bc.beat - 300,
            cause: Some(super::super::reachable::Cause::TimedOut),
            why: "timeout: connect".into(),
        }];
        (a.reports, b.reports) = (vec![bc.clone()], vec![bc]);
        let why = a.hand_to(&ia, B).unwrap_err();
        assert!(why.starts_with("unreachable from iMac: connection timed out (MacBook-Air asleep, off the network, or its firewall blocking Scenic"), "{why}");
        assert_eq!(b.hand_to(&ib, B).map_err(|w| w.starts_with("unreachable")), Err(true));
        a.reports = vec![clean];
        assert_eq!(a.hand_to(&ia, B), Ok(()));
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
        let stall =
            Slow(Mac { wall: Cell::new(io.0.now()), awake: Cell::new(io.0.awake()), ..Mac::new(&mem) }, Cell::new(360));
        a.step(&stall, able(), &any);
        stall.1.set(0);
        stall.0.pass(20);
        assert!(a
            .step(&stall, able(), &any)
            .events
            .iter()
            .any(|e| matches!(e, Event::Made { how, .. } if how == "re-asserted after a long step")));
    }

    /// Steps `d` with `heard`, then on, answering the listings it asks for, until it's caught up.
    fn caught_up(d: &mut Driver, io: &Mac, heard: Heard) -> Out {
        let mut heard = heard;
        let mut seen = Vec::new();
        for _ in 0..30 {
            io.pass(20);
            let o = step(d, io, std::mem::replace(&mut heard, able()));
            seen.push(format!("{:?} {:?} {} {:?}", o.leads, o.list, o.caught_up, o.events));
            if let Some(l) = &o.list {
                heard.listed = Some(Listed { n: l.n, keys: journal::list(io, l.since.as_deref()).unwrap() });
            } else if o.caught_up {
                return o;
            }
        }
        panic!("never caught up: {seen:#?}");
    }

    /// A leads (term 1) and merges its jobs' entries `ns` (of T0's day); eight days on, the
    /// journal's GC marks them, and `WAIT_S` later removes them; A's next listing of every day
    /// forgets them. Their keys.
    fn collected(a: &mut Driver, ia: &Mac, ns: &[u64]) -> Vec<String> {
        use super::super::journal_gc;
        let es: Vec<Entry> = ns.iter().map(|&n| entry(A, 1, n, &format!("6-1-{n}"))).collect();
        let keys: Vec<String> = es.iter().map(|e| e.key().unwrap()).collect();
        caught_up(a, ia, Heard { entries: es, ..able() });
        caught_up(a, ia, able());
        let e = a.leads().expect("A leads");
        assert!(keys.iter().all(|k| Records::load(ia, e).unwrap().unwrap().handles(k)));
        ia.pass(8 * 86400);
        caught_up(a, ia, able());
        let r = journal_gc::run(ia, ia.now(), &|| {}, &|_| {}).unwrap();
        assert_eq!((r.marked, r.removed), (keys.len(), 0), "{r:?}");
        ia.pass(journal_gc::WAIT_S);
        caught_up(a, ia, able());
        let r = journal_gc::run(ia, ia.now(), &|| {}, &|_| {}).unwrap();
        assert_eq!((r.removed, r.done), (keys.len(), true), "{r:?}");
        assert!(keys.iter().all(|k| !ia.exists(&journal::path(k)).unwrap()), "removed");
        ia.pass(RELIST_S);
        caught_up(a, ia, able());
        let r = Records::load(ia, a.leads().unwrap()).unwrap().unwrap();
        assert!(keys.iter().all(|k| r.forgot(k) && !r.reflected.contains(k)), "forgotten: {}", r.horizon);
        keys
    }

    #[test]
    fn gc_a_takeover_right_after_it_starts_from_records_that_name_what_it_removed() {
        // (Task #644) B takes over the moment GC is done. A stale read that finds no records for
        // A's terms would start it from term 1's first, which names none of the removed entries:
        // the fence holds the take-up until a read finds A's.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        caught_up(&mut a, &ia, able());
        step(&mut b, &ib, able());
        let keys = collected(&mut a, &ia, &[1, 2, 3]);
        let was = Records::load(&mem, a.leads().unwrap()).unwrap().unwrap();
        ib.wall.set(ia.now());
        *ib.gone.borrow_mut() = Some("/records.json".into());
        mem.write_whole(
            records::FIRST,
            &serde_json::to_vec(&Records { term: 1, seq: 1, ..Default::default() }).unwrap(),
        )
        .unwrap();
        step(&mut b, &ib, able());
        let o = step(&mut b, &ib, asks(Ask::TakeOver { force: true, downgrade: false }));
        assert_eq!(o.leads, None, "held by the fence: {:?}", o.events);
        assert!(
            o.events
                .iter()
                .any(|e| matches!(e, Event::Waits { what: "take up", why } if why.contains("being removed"))),
            "{:?}",
            o.events
        );
        // (Task #651) Its heartbeat says so, for every view's alert.
        let f = Beat::read(&mem, B).unwrap().unwrap().fenced.expect("fenced in the heartbeat");
        assert!(f.since == ib.now() && f.why.contains("being removed"), "{f:?}");
        *ib.gone.borrow_mut() = None;
        let o = caught_up(&mut b, &ib, able());
        assert!(o.leads.is_some(), "{:?}", o.events);
        assert_eq!(Beat::read(&mem, B).unwrap().unwrap().fenced, None, "taken up: no more");
        let r = Records::load(&mem, o.leads.unwrap()).unwrap().unwrap();
        assert_eq!((&r.manifest, &r.keys, &r.horizon), (&was.manifest, &was.keys, &was.horizon), "nothing lost");
        // A told of a removed entry again (its job's member back): answered as merged.
        ib.pass(20);
        let o = step(&mut b, &ib, Heard { msgs: vec![(C.into(), Msg::Tell(vec![keys[0].clone()]))], ..able() });
        assert!(
            o.send.iter().any(|(to, m)| to == C && matches!(m, Msg::Ack { keys: k, .. } if *k == [keys[0].clone()])),
            "{:?}",
            o.send
        );
    }

    #[test]
    fn gc_a_corrupt_marker_fences_every_take_up_until_scenic_pool_journal_gc_repairs_it() {
        // (Task #651) The marker turns unreadable after GC removed entries: every take-up is
        // refused, its heartbeat saying why. The check proposes a marker made again from the
        // newest records; written only once the kill switch says off; then B takes up, nothing lost.
        use super::super::journal_gc;
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V1), Saved::default());
        caught_up(&mut a, &ia, able());
        step(&mut b, &ib, able());
        collected(&mut a, &ia, &[1, 2, 3]);
        let was = Records::load(&mem, a.leads().unwrap()).unwrap().unwrap();
        mem.write_whole(journal_gc::PATH, b"{\"horizon\": \"2026-").unwrap();
        ib.wall.set(ia.now());
        step(&mut b, &ib, able());
        for _ in 0..3 {
            ib.pass(30);
            let o = step(&mut b, &ib, asks(Ask::TakeOver { force: true, downgrade: false }));
            assert_eq!(o.leads, None, "{:?}", o.events);
        }
        let f = Beat::read(&mem, B).unwrap().unwrap().fenced.expect("fenced");
        assert!(f.why.contains("can't be parsed") && f.since < ib.now(), "{f:?}");
        let said = journal_gc::check(&mem, ib.now(), false).unwrap();
        assert!(said.contains("made again") && said.contains("--repair"), "{said}");
        let e = journal_gc::check(&mem, ib.now(), true).unwrap_err();
        assert!(format!("{e:#}").contains("off"), "the switch first: {e:#}");
        mem.write_whole(journal_gc::SWITCH, b"off").unwrap();
        assert!(journal_gc::check(&mem, ib.now(), true).unwrap().contains("Written"));
        let m = journal_gc::read(&mem).unwrap().unwrap();
        assert!(m.covers(&was) && m.floor == was.horizon, "{m:?}");
        let o = caught_up(&mut b, &ib, able());
        assert!(o.leads.is_some(), "{:?}", o.events);
        let r = Records::load(&mem, o.leads.unwrap()).unwrap().unwrap();
        assert_eq!((&r.manifest, &r.horizon), (&was.manifest, &was.horizon), "nothing lost");
        assert_eq!(Beat::read(&mem, B).unwrap().unwrap().fenced, None);
    }

    #[test]
    fn gc_a_member_asleep_for_days_loses_nothing() {
        // (Task #644) C ran a job on T0's day and slept for two weeks before writing its entry: it
        // writes it late, into a day GC has emptied. It isn't among what GC marked: kept, merged.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        caught_up(&mut a, &ia, able());
        let keys = collected(&mut a, &ia, &[1, 2]);
        let late = entry(C, 1, 9, "6-1-9");
        let k = journal::write(&mem, &late).unwrap();
        assert!(k.as_str() < Records::load(&mem, a.leads().unwrap()).unwrap().unwrap().horizon.as_str());
        assert_eq!(k.split('/').next(), keys[0].split('/').next(), "a removed day");
        ia.pass(20);
        caught_up(&mut a, &ia, Heard { msgs: vec![(C.into(), Msg::Tell(vec![k.clone(), keys[1].clone()]))], ..able() });
        let r = Records::load(&mem, a.leads().unwrap()).unwrap().unwrap();
        assert!(r.reflected.contains(&k) && r.keys.unit.contains_key("6-1-9"), "the late entry merged");
        // The next GC leaves it until its own run marks it, named.
        let g = super::super::journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap();
        assert!(g.marked == 1 && mem.exists(&journal::path(&k)).unwrap(), "{g:?}");
    }

    #[test]
    fn gc_marks_only_what_a_snapshot_on_the_share_names() {
        // (Task #644) A merges two more entries but its saves fail (the share refusing its
        // records' writes): GC marks only what the snapshot on the share names, and removes
        // nothing a later term's records don't name.
        use super::super::journal_gc;
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        caught_up(&mut a, &ia, able());
        let saved = entry(A, 1, 1, "6-1-1");
        caught_up(&mut a, &ia, Heard { entries: vec![saved.clone()], ..able() });
        *ia.wfail.borrow_mut() = Some("state/build/term/".into());
        let unsaved = entry(A, 1, 2, "6-1-2");
        ia.pass(20);
        step(&mut a, &ia, Heard { entries: vec![unsaved.clone()], ..able() });
        ia.pass(20);
        step(&mut a, &ia, able());
        assert!(!Records::load(&mem, 1).unwrap().unwrap().handles(&unsaved.key().unwrap()), "not on the share");
        ia.pass(8 * 86400);
        let g = journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap();
        assert_eq!(g.marked, 0, "a day holding an entry not named is kept, and the days after: {g:?}");
        // Its saves land again: all named, marked. Then a stale lead's records, a term on, don't
        // name them (taken up from term 1's first, say): nothing removed.
        *ia.wfail.borrow_mut() = None;
        caught_up(&mut a, &ia, able());
        let g = journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap();
        assert_eq!(g.marked, 2, "{g:?}");
        let e = a.leads().unwrap();
        let stale = Records { term: e + 1, seq: 1, ..Default::default() };
        mem.write_whole(&term::path(e + 1), &mem.read(&term::path(e)).unwrap().unwrap()).unwrap();
        let mut t: Term = serde_json::from_slice(&mem.read(&term::path(e)).unwrap().unwrap()).unwrap();
        t.term = e + 1;
        mem.write_whole(&term::path(e + 1), &serde_json::to_vec(&t).unwrap()).unwrap();
        mem.write_whole(&records::path(e + 1), &serde_json::to_vec(&stale).unwrap()).unwrap();
        ia.pass(journal_gc::WAIT_S);
        let g = journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap();
        assert_eq!(g.removed, 0, "{g:?}");
        assert!(g.waits.iter().any(|w| w.contains("don't name")), "{g:?}");
        assert!(mem.exists(&journal::path(&saved.key().unwrap())).unwrap());
    }

    #[test]
    fn gc_waits_for_every_member_on_an_app_that_keeps_the_fence() {
        // (Task #644) B's heartbeat is an older app's (no `journal`): nothing is removed while it
        // could take up from whatever records it reads. Once B's app keeps the fence, removed; an
        // older app's take-up then, from the newest records as it reads them, loses nothing: what
        // it's told of the removed days is of days they forgot.
        use super::super::journal_gc;
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        caught_up(&mut a, &ia, able());
        let es: Vec<Entry> = (1..=2).map(|n| entry(A, 1, n, &format!("6-1-{n}"))).collect();
        caught_up(&mut a, &ia, Heard { entries: es.clone(), ..able() });
        let old =
            Beat { member: B.into(), host: "MacBook-Air".into(), app: V1.into(), beat: ia.now(), ..Default::default() };
        let mut b: serde_json::Value = serde_json::to_value(&old).unwrap();
        b.as_object_mut().unwrap().remove("journal");
        mem.write_whole(&super::super::beat::path(B), &serde_json::to_vec(&b).unwrap()).unwrap();
        ia.pass(8 * 86400);
        caught_up(&mut a, &ia, able());
        assert_eq!(journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap().marked, 2);
        ia.pass(journal_gc::WAIT_S);
        caught_up(&mut a, &ia, able());
        let g = journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap();
        assert_eq!(g.removed, 0);
        assert!(g.waits.iter().any(|w| w.contains("from before")), "{g:?}");
        Beat { journal: journal_gc::FENCE, ..old }.write(&mem).unwrap();
        assert_eq!(journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap().removed, 2);
        // An older app's take-up: the newest records, the journal replayed, no fence.
        let e = a.leads().unwrap();
        let mut r = Records::newest(&mem, e, false).unwrap().unwrap();
        let keys: Vec<String> = es.iter().map(|e| e.key().unwrap()).collect();
        let m = records::merge(&mem, &mut r, &[keys.clone(), journal::list(&mem, None).unwrap()].concat(), &any);
        assert!(m.applied.is_empty() && m.waiting.is_empty(), "{m:?}");
        assert_eq!(r.keys.unit.len(), 2, "both jobs' records kept");
    }

    #[test]
    fn gc_resumes_after_a_crash_and_keeps_a_folder_a_late_entry_reached() {
        // (Task #644 review) The remove phase stops midway (its process gone after one day): the
        // next run removes the rest, never a file twice, and writes the marker done only then. A
        // late entry written into a marked day keeps that day's folder; the switch is on unless it
        // says off.
        use super::super::journal_gc;
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        assert!(journal_gc::enabled(&ia), "on by default");
        mem.write_whole(journal_gc::SWITCH, b"off\n").unwrap();
        assert!(!journal_gc::enabled(&ia));
        mem.remove(journal_gc::SWITCH).unwrap();
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        caught_up(&mut a, &ia, able());
        let first = entry(A, 1, 1, "6-1-1");
        caught_up(&mut a, &ia, Heard { entries: vec![first.clone()], ..able() });
        ia.pass(86400);
        let second = Entry { at: T0 + 86400, ..entry(A, 1, 2, "6-1-2") };
        caught_up(&mut a, &ia, Heard { entries: vec![second.clone()], ..able() });
        ia.pass(8 * 86400);
        caught_up(&mut a, &ia, able());
        assert_eq!(journal_gc::run(&ia, ia.now(), &|| {}, &|_| {}).unwrap().marked, 2);
        let late = entry(C, 1, 9, "6-1-9");
        let lk = journal::write(&mem, &late).unwrap();
        assert_eq!(lk.split('/').next(), first.key().unwrap().split('/').next());
        ia.pass(journal_gc::WAIT_S);
        // A crash after the first day: `between` panics on the second.
        let calls = std::cell::Cell::new(0);
        let dirs = std::cell::RefCell::new(Vec::new());
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            journal_gc::run(
                &ia,
                ia.now(),
                &|| {
                    calls.set(calls.get() + 1);
                    assert!(calls.get() < 2, "crash");
                },
                &|d| dirs.borrow_mut().push(d.to_string()),
            )
        }));
        assert!(crash.is_err(), "{crash:?} {:?} {:?}", first.key(), second.key());
        assert!(!mem.exists(&journal::path(&first.key().unwrap())).unwrap());
        assert!(mem.exists(&journal::path(&second.key().unwrap())).unwrap());
        assert!(dirs.borrow().is_empty(), "the day with a late entry keeps its folder");
        assert!(!journal_gc::read(&ia).unwrap().unwrap().done);
        let g = journal_gc::run(&ia, ia.now(), &|| {}, &|d| dirs.borrow_mut().push(d.to_string())).unwrap();
        assert_eq!((g.removed, g.done, g.days), (1, true, 1), "{g:?}");
        assert!(!mem.exists(&journal::path(&second.key().unwrap())).unwrap());
        assert!(mem.exists(&journal::path(&lk)).unwrap(), "the late entry kept");
        assert_eq!(dirs.borrow().len(), 1);
    }
}
