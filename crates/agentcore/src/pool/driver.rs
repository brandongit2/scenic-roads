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
//! - **takes what was heard since the last step** (`Heard`): the members' messages to it, the
//!   build pages' asks, the hand-offs of its jobs that ended, the listing it asked for;
//! - **gives what to do now** (`Out`): the messages to send, by member id, over the pool's API
//!   (best effort: a message lost is told again or made up for); the pool's fields of this Mac's
//!   heartbeat, to write with the rest of it, stamped as it's written; the term it leads, if it
//!   leads, and what it may do as that now: grant jobs and plan (`duties`); publish a catalogue
//!   only while it may do its duties and its records reflect the journal (`duties` and
//!   `caught_up`: a listing of every day merged and saved, its take-up's or a daily one, asked for
//!   under a day ago, `listed_at` saying when; nothing told or listed waiting to be read; no
//!   re-assertion owed; a re-assertion keeping it), and sweep (GC) only once its records reflect
//!   the journal, on a step that re-asserted (`fresh`: asked with `Heard::reassert`; it says no
//!   later term was made before, not that its records are whole); a listing to make; and what
//!   happened (`Event`s: terms made and taken up, stepped down from; errors), for the history and
//!   the log;
//! - **never fails**: an error stops only the duty that met it (said in an `Event::Failed`), and
//!   the step goes on: a lead whose re-assertion fails keeps leading, its duties held, and tries
//!   again next step.
//!
//! What the driver does itself, through `Io`, is what decides safety: making terms
//! (crate::pool::term), taking them up and saving its term's records (crate::pool::records),
//! writing its jobs' entries and merging others' (crate::pool::journal). What it leaves to the
//! agent can't break §4's invariants: granting and the coordinator, the merge's checks (`Check`,
//! passed to every step), the duties, the heartbeat's other fields, the messages' transport, and
//! persisting `Saved` (its state between processes, naming its member: its jobs' hand-offs, kept
//! until a lead's saved records name them) after every step that changed it,
//! **before acting on that step's `Out`** (a take-up's grants, its messages): a crash between would
//! restart from a state that doesn't know the term it took up. **A job's hand-off is kept until a
//! `Saved` from a step it was handed to is on the agent's disk**: before that, a crash loses it. A
//! `Saved` that's lost, or another member's (this Mac's member file lost, a new id made; or a copy
//! of another Mac's folder), counts for nothing but its jobs' hand-offs not written yet: the
//! driver then re-asserts a term naming it that it finds at its start rather than take it up again
//! (it may have led it, its leases granted), and the state it saves says so, for the processes
//! after it. So does an older state of its own (a backup restored) for a term naming it that it
//! made and the state doesn't record (`Saved::made`), or whose records are there already, a
//! process before this one having taken it up, unless this process is taking it up.
//!
//! # The fixed lead (§6, task #881)
//!
//! One member leads: the one `state/build/lead-member.json` declares (crate::pool::term::declared,
//! the Mac mini). There is no election, handover or takeover. A member never makes a term and
//! never leads one: a term naming it (one of the elected leads' before #881) is left to the
//! declared lead, which makes the next. With the lead gone, members finish the jobs they hold,
//! write their hand-offs to the journal, and wait for it. No lead declared yet (the pool's move to
//! a fixed lead), no one leads until the owner writes it; in a new pool, term 1's lead declares
//! itself, once, with create-new.
//!
//! A member learns its lead and the current term from its lead's beat answer while one came lately
//! (`Heard::lead_says`, docs/coordination.md step 8a), with no NAS call; without one (an older
//! lead, a lead not leading or hung, none answering: the owner may have moved the lead) it reads
//! the declaration and stats the next term, as the lead does every step.
//!
//! The lead's view can be old without its knowing (§6.6). It re-asserts (makes the next term naming
//! itself and takes it up from its own records, its coordinator loading the term before's state
//! with its leases) before acting again when, since its last step or within its last one, its wall
//! clock and its awake clock moved more than `GAP_S` apart (it slept, or its wall clock was set,
//! either way); when its last step ran over `STALL_S`, or that long awake passed since it (the NAS
//! stalled, or its process was held up: a share under load slows every operation, so a step's
//! length short of that says nothing); after a restart (an update's, at a safe point); and when
//! the agent asks (`Heard::reassert`, before a GC sweep). Time spent listing the journal (off the
//! steps) isn't a gap. A re-assertion keeps what the lead knew of the journal, so a sweep's step is
//! caught up; after a sleep, its members' messages meanwhile lost, it lists the journal again.

use super::beat::Beat;
use super::journal::{self, Entry, Mine};
use super::nas::Nas;
use super::records::{self, Check, Records};
use super::term::{self, Current, Made, Term};
use super::{Member, MemberLock};
use serde::{Deserialize, Serialize};
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
/// job's work, done again; the hour held every catalogue. Past a stale read's usual `STALE_S`.
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
    /// From a member to the lead it knows: its journal entries that lead hasn't acknowledged. (A
    /// member writes to the journal, and tells, only while its lead doesn't take hand-offs by HTTP:
    /// the rollout gate of docs/coordination.md step 6, deleted with it.)
    Tell(Vec<String>),
    /// From the lead of `term`: those entries are in a snapshot of its records it saved (applied,
    /// passed over or refused). (`horizon` always empty: read by apps before step 6, which forgot
    /// what they had acknowledged before it; deleted with the gate.)
    Ack {
        term: u64,
        keys: Vec<String>,
        #[serde(default)]
        horizon: String,
    },
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

/// An ask from this Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// A build page's on this Mac (`Act`): the lead acts on it; a member passes it on.
    Act(Act),
}

/// What a member heard since its last step.
#[derive(Clone, Debug, Default)]
pub struct Heard {
    /// The members' messages to it, by sender's member id.
    pub msgs: Vec<(String, Msg)>,
    /// The asks from this Mac (its build page's).
    pub asks: Vec<Ask>,
    /// The hand-offs of its jobs that ended: kept whole (in `Saved`) until written to the journal;
    /// the agent may drop one once a `Saved` from this step is on its disk.
    pub entries: Vec<Entry>,
    /// A listing of the journal it asked for (`Out::list`), done since, with the ask's number. The
    /// agent makes every listing asked for, however long it takes (one that fails made again), and
    /// hands each back once: any of a lead's asks still out counts when it's back.
    pub listed: Option<Listed>,
    /// Raw tiles' archives its records hold (`Records::raw`) that the agent has named in the raw
    /// store's index since (crate::rawpack::name_handed, the index saved): taken off the records,
    /// saved with this step's snapshot. (One named and not taken off yet, a restart between, is
    /// named again: passed over unread.)
    pub named_raw: Vec<String>,
    /// Re-assert before the agent does what only a fresh lead may (a GC sweep): `Out::fresh` says
    /// it did.
    pub reassert: bool,
    /// Leading: the hand-offs members posted it by HTTP (crate::coord::handoffs) and it hasn't
    /// acknowledged: merged from memory, acknowledged once a snapshot naming them is saved
    /// (`Out::merged`).
    pub handed: Vec<Entry>,
    /// A member: its hand-offs kept whole that its lead's saved records name (its beats' answers):
    /// dropped.
    pub merged: Vec<String>,
    /// A member: how its hand-offs go to its lead (`Route`).
    pub route: Route,
    /// Leading: no member writes its hand-offs to the journal now (none heard is on an app before
    /// docs/coordination.md step 6): it lists the journal no more, and is caught up without a
    /// listing. (The rollout gate, deleted with it.)
    pub http_only: bool,
    /// A member: the term its lead's beat answered it leads, that answer fresh (within
    /// crate::coord::beats::PAGE_AWAY_S: crate::coord::beats::Beats::lead_says). Its lead and term
    /// are learnt from it, and the NAS's declaration and terms aren't read; None (an older lead, a
    /// lead not leading, none answering lately: the owner may have moved the lead) reads them
    /// (docs/coordination.md step 8a).
    pub lead_says: Option<Term>,
}

/// How a member's hand-offs go to its lead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Route {
    /// Written to the journal and told by mail: a lead on an app before docs/coordination.md step 6
    /// (its answer says nothing of hand-offs, or it answers the beat 404). (The rollout gate.)
    #[default]
    Journal,
    /// Kept whole, posted by its beat (crate::agent::beat), acknowledged in the answers
    /// (`Heard::merged`): a lead that says it takes them.
    Http,
    /// Kept whole: its lead hasn't answered since this process started.
    Wait,
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
    /// Leading, whether it may grant jobs and plan now: not while its view may be old (a
    /// re-assertion due).
    pub duties: bool,
    /// Leading, it re-asserted (or took its term up) this step: no later term was made before. Not
    /// that its records are whole (`caught_up`).
    pub fresh: bool,
    /// Leading, its records reflect what's handed off: every hand-off posted, told or listed is
    /// merged and saved (on a share under load a loop leaves some to the next; a journal entry never
    /// read whole is refused after `UNREADABLE_S`, or `MOVED_ON_S`), no re-assertion is owed, and
    /// while a member writes the journal (`Heard::journal`) a listing of every day (its take-up's,
    /// or the daily one) asked for less than `RELIST_S` ago is merged too. A re-assertion keeps it,
    /// but after a sleep. A catalogue waits for it (and `duties`); GC too (and `fresh`): an entry
    /// not merged yet may hold uploads the records don't name.
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
    /// Leading: the hand-offs posted it (`Heard::handed`) that a snapshot it saved names, to
    /// acknowledge (crate::coord::handoffs) once this step's `Saved` is on disk.
    pub merged: Vec<String>,
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
    /// Took up term `term` (how its file says): its coordinator loads the term before's state.
    TookUp { term: u64, how: String },
    /// No longer leads term `term`: why.
    SteppedDown { term: u64, why: String },
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
    /// Its jobs' hand-offs, kept until a lead's saved records name them (whole, or by key once
    /// written to the journal for an older lead).
    pub mine: Mine,
    /// The highest term it knew of (its view never goes back, a stale read at its start
    /// notwithstanding), and the highest it led.
    #[serde(default)]
    pub term: u64,
    pub led: u64,
    /// The highest term it made naming itself (a re-assertion, term 1): one naming it that's newer
    /// was made by a process whose state this isn't
    /// (a backup restored), and is re-asserted, not taken up again.
    #[serde(default)]
    pub made: u64,
    /// A term its create made and couldn't fill, to finish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfinished: Option<Term>,
    /// The records it last saved leading (term, seq): a take-up never starts from older ones
    /// (crate::pool::records::start), whose saves acknowledged hand-offs no member keeps now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<(u64, u64)>,
}

/// A term this Mac leads.
#[derive(Clone, Debug)]
struct Lead {
    term: Term,
    records: Records,
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

/// A listing asked for and not handed back yet (by its number, in `Driver::asked`).
#[derive(Clone, Copy, Debug)]
struct Asked {
    /// When (this Mac's clock).
    at: u64,
    /// Of every day not forgotten (a take-up's, or the daily one), not the last days' (a sweep's).
    full: bool,
}

/// What a lead's step merges: what members told it by mail, a listing made, what they posted.
struct Given<'a> {
    tells: Vec<(String, Vec<String>)>,
    listed: Option<Listed>,
    handed: Vec<Entry>,
    named_raw: &'a [String],
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
    /// It just started (a restart): a term it led and names it is re-asserted.
    restarted: bool,
    /// The current term it learnt from the NAS at its first step (none before), not counting a
    /// term 1 its own first step made.
    first: Option<u64>,
    /// Since when (awake clock) a take-up has found term 1 with no snapshot that reads whole (its
    /// lead's, or its first).
    first_unread: Option<u64>,
    /// Leading: a member writes the journal (`Heard::journal`), as its last step heard.
    journal: bool,
    /// The lead's declaration as last read (crate::pool::term::declared): the member that leads.
    declared: Option<String>,
}

/// Whether the clocks went `was` → `now` (wall, awake) with more than `GAP_S` unaccounted for: the
/// Mac slept, or its wall clock was set, either way.
fn gap(was: (u64, u64), now: (u64, u64)) -> bool {
    let wall = now.0 as i64 - was.0 as i64;
    let awake = now.1 as i64 - was.1 as i64;
    (wall - awake).unsigned_abs() > GAP_S
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
        Driver {
            me,
            lock: None,
            stopped: None,
            cur: Current::default(),
            saved,
            known,
            lead: None,
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
            restarted: true,
            first: None,
            first_unread: None,
            journal: true,
            declared: None,
        }
    }

    /// What to keep for the next process: saved after every step that changed it, before that
    /// step's `Out` is acted on.
    pub fn saved(&self) -> Saved {
        self.saved.clone()
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

    /// The member the lead's declaration names, as last read (None: none declared yet, or not read).
    pub fn declared(&self) -> Option<&str> {
        self.declared.as_deref()
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
        // Its jobs' entries, kept whole.
        for e in heard.entries {
            if let Err(err) = self.saved.mine.add(e) {
                out.events.push(Event::Failed { what: "keep an entry", why: format!("{err:#}") });
            }
        }
        self.saved.mine.acked_all(&heard.merged);
        self.journal = !heard.http_only;
        let busy = start.1 + BUSY_S;
        self.finish(io, &mut out);
        // What it heard.
        let mut tells = Vec::new();
        let mut acts: Vec<Act> = Vec::new();
        for (from, m) in heard.msgs {
            match m {
                Msg::Tell(keys) => tells.push((from, keys)),
                Msg::Ack { keys, .. } => self.saved.mine.acked_all(&keys),
                Msg::Cancel(c) => acts.push(Act::Cancel(c)),
                Msg::Retry(r) => acts.push(Act::Retry(r)),
            }
        }
        for a in heard.asks {
            match a {
                Ask::Act(a) => acts.push(a),
            }
        }
        // (A term its lead says, of another member, no older than its own view: taken in place of
        // the NAS's.)
        let said = heard.lead_says.filter(|t| t.member != self.me.id && t.term >= self.cur.term.max(self.saved.term));
        let by_lead = said.is_some();
        let learnt = match said {
            Some(t) => {
                self.heard_of(t);
                Ok(())
            }
            None => self.learn(io),
        };
        if let Err(e) = &learnt {
            out.events.push(Event::Failed { what: "learn the current term", why: format!("{e:#}") });
        }
        if self.taking.as_ref().is_some_and(|r| r.term != self.cur.term) {
            self.taking = None;
        }
        // The declared lead (a member never leads); none declared yet, the current term's lead
        // declares itself.
        let mine = if by_lead {
            // (The lead leads only while declared: it steps down at its next step otherwise, and
            // its answers say no term.)
            self.declared = self.cur.lead.as_ref().map(|t| t.member.clone());
            false
        } else {
            self.declare(io, &mut out)
        };
        // A member's entries to the journal, for a lead that doesn't take them by HTTP (the gate);
        // the declared lead merges its own from memory.
        if !mine && heard.route == Route::Journal {
            for (k, err) in self.saved.mine.write_while(io, &|| io.awake() < busy) {
                out.events.push(Event::Failed { what: "write an entry", why: format!("{k}: {err:#}") });
            }
        }
        // A lead: a later term ends it, and so does another's declaration; it re-asserts when it
        // must.
        if let Some(l) = &self.lead {
            if self.cur.term > l.term.term {
                self.step_down(&mut out, "a later term exists");
            } else if !mine {
                let why =
                    if self.declared.is_some() { "another member is the declared lead" } else { "no lead is declared" };
                self.step_down(&mut out, why);
            }
        }
        if self.lead.is_some() {
            if let Some(how) = self.must {
                self.reassert(io, &mut out, how);
            }
        }
        // The declared lead, not leading: takes up a term naming it, or makes its own.
        if self.lead.is_none() && mine {
            self.own_term(io, &mut out);
        }
        self.restarted = false;
        if self.lead.is_some() {
            let given = Given { tells, listed: heard.listed, handed: heard.handed, named_raw: &heard.named_raw };
            self.lead_step(io, &mut out, given, check, busy, learnt.is_ok());
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
        // Telling the lead it knows of its entries in the journal.
        if let Some(t) = self.cur.lead.as_ref().filter(|t| t.member != self.me.id) {
            let keys = self.saved.mine.to_tell();
            if !keys.is_empty() {
                out.send.push((t.member.clone(), Msg::Tell(keys)));
            }
        }
        self.listings(io, &mut out);
        out.beat = self.beat(io);
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
            out.duties = self.must.is_none();
            out.fresh &= self.must.is_none();
            out.caught_up = (!self.journal || l.listed_at.is_some_and(|at| end.0.saturating_sub(at) < RELIST_S))
                && !l.dirty
                && l.waiting.is_empty()
                && self.must.is_none();
            out.listed_at = l.listed_at;
            out.held = l.held.iter().map(|(k, w)| (k.clone(), w.clone())).collect();
        } else {
            out.fresh = false;
        }
        // (Its lock not known held this step: no duties, catalogue or sweep until it's checked
        // again.)
        if let Some(why) = unsure {
            out.events.push(Event::Failed { what: "check the member's lock", why });
            (out.duties, out.caught_up, out.fresh) = (false, false, false);
        }
        out
    }

    /// Learns the current term: once from the NAS at its first step (making term 1 if it's this
    /// Mac's to make: the declared lead's, or in a new pool with none declared any Mac's; the
    /// lead's hint, or one listing of the terms), then by checking the next.
    fn learn(&mut self, io: &dyn Io) -> anyhow::Result<()> {
        if self.cur.term == 0 {
            // (Term 1 made by this call: its `since` this call's.)
            let now = io.now();
            let mine = term::declared(io)?.is_none_or(|d| d.member == self.me.id);
            // (Its create cut short: term 1 its own to finish, `finish`, before any Mac reads it.)
            let boot = if mine { term::bootstrap(io, &self.me, now) } else { Ok(None) };
            if let Some(u) = boot.as_ref().err().and_then(|e| e.downcast_ref::<term::Unfinished>()) {
                self.saved.made = self.saved.made.max(1);
                self.saved.unfinished = Some(u.0.clone());
            }
            let made = boot?.is_some_and(|t| t.member == self.me.id && t.since == now);
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
        self.saved.term = self.saved.term.max(self.cur.term);
        Ok(())
    }

    /// Learns the current term from its lead's word (`Heard::lead_says`: a term of another member,
    /// no older than its view), with no NAS call. At its first step it counts as the term learnt
    /// then, as `learn`'s from the NAS does.
    fn heard_of(&mut self, t: Term) {
        if self.first.is_none() {
            self.first = Some(t.term);
            if !self.known {
                self.saved.led = self.saved.led.max(t.term);
            }
        }
        self.cur = Current { term: t.term, lead: Some(t) };
        self.saved.term = self.saved.term.max(self.cur.term);
    }

    /// Finishes the term its create made and couldn't fill.
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

    /// A term this Mac made, naming itself: noted, and taken up; unfinished, kept to finish.
    fn made(&mut self, io: &dyn Io, out: &mut Out, t: Term, made: Made) -> bool {
        match made {
            Made::Ours => {
                out.events.push(Event::Made { term: t.term, how: t.how.clone() });
                self.cur = Current { term: t.term, lead: Some(t.clone()) };
                self.saved.term = self.saved.term.max(t.term);
                self.saved.made = self.saved.made.max(t.term);
                self.take_up(io, out, &t);
                true
            }
            Made::Unfinished(e) => {
                out.events.push(Event::Waits { what: "finish the term it made", why: format!("{e:#}") });
                self.cur = Current { term: t.term, lead: Some(t.clone()) };
                self.saved.term = self.saved.term.max(t.term);
                self.saved.made = self.saved.made.max(t.term);
                self.saved.unfinished = Some(t);
                true
            }
            Made::Theirs => false,
        }
    }

    /// Re-asserts: makes the next term naming itself and takes it up from its own records; another
    /// made first (a process of an elected lead before #881, late), it steps down, and makes its own
    /// next step.
    fn reassert(&mut self, io: &dyn Io, out: &mut Out, how: &'static str) {
        let t = match Term::after(&self.cur, &self.me, how, io.now()) {
            Ok(t) => t,
            Err(err) => return out.events.push(Event::Failed { what: "re-assert", why: format!("{err:#}") }),
        };
        match term::make(io, &t) {
            Ok(Made::Theirs) => self.step_down(out, "re-asserting, it found the next term made"),
            Ok(made) => {
                let l = self.lead.take().expect("leading");
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

    /// The declared lead, not leading: a current term naming it is taken up (one it made); or, a
    /// term it led and doesn't lead now (restarted), re-asserted; a term naming another member (an
    /// elected lead's, before #881) is followed by its own. One it found at its start with its past
    /// unknown (its saved state lost, or another member's) counts as one it led: it may have, its
    /// leases granted. So does one its state is older than (a backup restored): one it made that the
    /// state doesn't record, and one whose records are there already, taken up by a process before
    /// this one.
    fn own_term(&mut self, io: &dyn Io, out: &mut Out) {
        // (A term its create made and couldn't fill: finished first, then taken up.)
        if self.saved.unfinished.is_some() || self.cur.term == 0 {
            return;
        }
        let t = match self.cur.lead.clone() {
            Some(t) if t.member == self.me.id && t.term >= self.saved.led => t,
            // (Unreadable: its maker finishes it, and the next step reads it.)
            None => return,
            Some(t) if t.member == self.me.id => return self.make_own(io, out, "re-asserted"),
            Some(t) => {
                let how = format!("the declared lead, after {}", t.host);
                return self.make_own(io, out, &how);
            }
        };
        // (Newer than it led: taken up, but for one it made that its state doesn't record, or one
        // whose records are there and that this process isn't taking up: an older state of its own
        // restored, which may have led it, its leases granted.)
        let unrecorded = t.term > self.saved.made;
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
            _ => "re-asserted",
        };
        self.make_own(io, out, how);
    }

    /// Makes the next term naming itself, and takes it up.
    fn make_own(&mut self, io: &dyn Io, out: &mut Out, how: &str) {
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

    /// Whether this Mac is the declared lead (crate::pool::term::declared), read each step (the last
    /// read kept when one fails). None declared yet: in a new pool, term 1's lead declares itself,
    /// once, and counts as declared while its declaration's create fails (tried again next step);
    /// the term unreadable meanwhile, the last read is kept. Past term 1 no Mac declares itself
    /// (a pool that led before #881: an elected lead, maybe not the mini, would make itself the
    /// fixed one): the owner writes the declaration, and until then no one leads.
    fn declare(&mut self, io: &dyn Io, out: &mut Out) -> bool {
        match term::declared(io) {
            Ok(Some(d)) => self.declared = Some(d.member),
            Ok(None) => match &self.cur.lead {
                Some(t) if t.term == 1 && t.member == self.me.id => match term::declare(io, &self.me, io.now()) {
                    Ok(d) => self.declared = d.map(|d| d.member).or(Some(self.me.id.clone())),
                    Err(e) => {
                        self.declared = Some(self.me.id.clone());
                        out.events.push(Event::Failed { what: "declare the lead", why: format!("{e:#}") });
                    }
                },
                Some(_) => self.declared = None,
                None => {}
            },
            Err(e) => out.events.push(Event::Waits { what: "read the lead's declaration", why: format!("{e:#}") }),
        }
        self.declared.as_deref() == Some(self.me.id.as_str())
    }

    /// Takes up term `t`, which names this Mac (§6.2): from the newest records a read finds (its
    /// own, for a term it led), its first snapshot saved. Not
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
        // (A term made after another member's: the owner moved the lead. That member saves and
        // acknowledges until it finds this term, read after each save (`lead_step`), and its last
        // save may read stale a while: the records are read `STALE_S` after this term was made, so
        // that what it acknowledged, on no member's disk now, is in them. docs/coordination.md
        // step 6.)
        if t.from >= 1 {
            let before = match term::read(io, t.from) {
                Ok(b) => b,
                Err(e) => {
                    return out.events.push(Event::Waits {
                        what: "take up",
                        why: format!("term {}: the term before it: {e:#}", t.term),
                    })
                }
            };
            let now = io.now();
            if before.as_ref().is_none_or(|b| b.member != self.me.id) && now >= t.since && now - t.since < STALE_S {
                let host = before.map_or_else(|| format!("term {}'s lead", t.from), |b| b.host);
                return out.events.push(Event::Waits {
                    what: "take up",
                    why: format!(
                        "term {} follows {host}'s: its records are read once {host}'s last save can't read \
                         stale, in {} s",
                        t.term,
                        STALE_S - (now - t.since)
                    ),
                });
            }
        }
        // (Nor from records older than those it saved last, or than its term led's: a stale read.)
        let saved = self.saved.records.or((self.saved.led > 0).then_some((self.saved.led, 0))).filter(|_| self.known);
        let mut r = match records::start(io, t, own.as_ref(), today, saved) {
            Ok(r) => {
                self.first_unread = None;
                r
            }
            Err(e) => {
                if e.is::<records::FirstUnreadable>() {
                    self.first_unread.get_or_insert(io.awake());
                }
                return out.events.push(Event::Waits { what: "take up", why: format!("term {}: {e:#}", t.term) });
            }
        };
        // (Nothing merged yet: its members tell it of their entries once they know it leads, and a
        // listing of the journal follows.)
        if let Err(e) = r.save(io) {
            self.taking = Some(r);
            return out
                .events
                .push(Event::Failed { what: "save the records", why: format!("taking up term {}: {e:#}", t.term) });
        }
        self.saved.records = Some((r.term, r.seq));
        // The lead's hint (§6.1: for old apps, and to save a listing; never the truth).
        if let Err(e) = term::write_hint(io, t) {
            out.events.push(Event::Failed { what: "write the lead's hint", why: format!("{e:#}") });
        }
        self.taking = None;
        self.spare = None;
        self.slept = false;
        self.saved.led = t.term;
        self.must = None;
        out.fresh = true;
        out.events.push(Event::TookUp { term: t.term, how: t.how.clone() });
        self.lead = Some(Lead {
            term: t.clone(),
            records: r,
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
        // A take-up lists the journal, every day, while a member writes it.
        self.due = self.journal.then_some((None, true));
        self.asked.clear();
    }

    /// The lead's step: merge what members posted it and told it, its own jobs' hand-offs and what
    /// a listing found (and what's waiting to be read whole), save, acknowledge, note the refusals.
    fn lead_step(&mut self, io: &dyn Io, out: &mut Out, given: Given, check: Check, busy: u64, learnt: bool) {
        let Given { tells, listed, handed, named_raw } = given;
        let me = self.me.id.clone();
        let own: Vec<String> = self.saved.mine.to_tell();
        // (Held whole: members' posted by HTTP, and its own jobs', by key.)
        let mut whole: BTreeMap<String, Entry> = BTreeMap::new();
        for e in handed.into_iter().chain(self.saved.mine.unwritten_entries().cloned()) {
            if let Some(k) = e.key() {
                whole.insert(k, e);
            }
        }
        let now = io.now();
        let told_at = io.awake();
        let l = self.lead.as_mut().expect("leading");
        let e = l.term.term;
        for (from, keys) in tells.into_iter().chain([(me.clone(), own)]) {
            for k in keys {
                l.told.entry(k).or_insert_with(|| (from.clone(), told_at));
            }
        }
        let mut keys: Vec<String> = l
            .told
            .keys()
            .chain(&l.waiting)
            .chain(l.held.keys())
            .chain(whole.keys())
            .filter(|k| !l.records.handles(k))
            .cloned()
            .collect();
        let from_listing = listed.is_some();
        if let Some(listed) = listed {
            // (A listing of every day, its take-up's or a daily one: once merged and saved, the
            // records reflect the journal as it was when it was asked for; any of its asks still
            // out, the newest's time kept. Keys asked for before its take-up are merged as any.)
            self.back = now;
            if let Some(a) = self.asked.remove(&listed.n) {
                if a.full {
                    l.listed_at = Some(l.listed_at.map_or(a.at, |at| at.max(a.at)));
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
        let read = |k: &str| match whole.get(k) {
            Some(e) => Ok(super::journal::Read::Entry(e.clone())),
            None => super::journal::read(io, k),
        };
        let m = records::merge_from(&read, &mut l.records, &keys, check, &|| io.awake() < busy);
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
        l.waiting.retain(|k| !l.records.handles(k));
        // What its reads find not whole over `UNREADABLE_S` never will be: refused, as a damaged
        // entry is (its work done again). So what its reads fail on while the share answers the
        // step's others (its learning of the term, or reads of other entries): a file the share
        // errors on. A read that failed when none answered (the share away) says nothing, and
        // starts the time again; one the step's time left unread keeps it, and isn't refused
        // unread.
        let answered =
            learnt || !(m.applied.is_empty() && m.overtaken.is_empty() && m.refused.is_empty() && m.waiting.is_empty());
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
                Ok(()) => {
                    l.dirty = false;
                    self.saved.records = Some((l.records.term, l.records.seq));
                }
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
        // Acknowledged: what a saved snapshot names. Told by mail, an answer by mail; posted by
        // HTTP, by the coordinator (`Out::merged`); its own, dropped.
        if !l.dirty {
            let mut acks: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (k, (from, _)) in &l.told {
                if l.records.handles(k) {
                    acks.entry(from.clone()).or_default().push(k.clone());
                }
            }
            let named: Vec<String> = whole.into_keys().filter(|k| l.records.handles(k)).collect();
            // (Only while no later term exists, as read after the save: the owner moved the lead,
            // the next lead may have read its records before this save, and what's acknowledged is
            // on no member's disk then. Its take-up waits `STALE_S` after making its term, so a save
            // this read follows is in what it reads (`take_up`). Not acknowledged now: next step,
            // while it still leads; else its members keep them for the next lead.)
            if !acks.is_empty() || !named.is_empty() {
                match term::next(io, e) {
                    Ok(false) => {}
                    Ok(true) => {
                        return out.events.push(Event::Waits {
                            what: "acknowledge hand-offs",
                            why: format!("term {} was made after its save: its lead has them", e + 1),
                        })
                    }
                    Err(err) => {
                        return out.events.push(Event::Failed {
                            what: "acknowledge hand-offs",
                            why: format!("whether term {} was made after its save: {err:#}", e + 1),
                        })
                    }
                }
            }
            for (to, keys) in acks {
                for k in &keys {
                    l.told.remove(k);
                }
                if to == me {
                    self.saved.mine.acked_all(&keys);
                } else {
                    out.send.push((to, Msg::Ack { term: e, keys, horizon: String::new() }));
                }
            }
            self.saved.mine.acked_all(&named);
            out.merged = named;
        }
    }

    /// The listings it asks for: of every day, a take-up's, then daily (`RELIST_S`); and of the
    /// last days every `SWEEP_S`. One at a time, none while its last is out (the agent hands every
    /// one back, however long it takes); its last out `OVERDUE_S` with none back meanwhile (lost,
    /// or that slow) is said, and the next asked for all the same. Whichever comes back counts:
    /// each ask is kept until it's a day old, its listing too old by then to count.
    fn listings(&mut self, io: &dyn Io, out: &mut Out) {
        let Some(l) = self.lead.as_ref().filter(|_| self.journal) else {
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
            self.due = Some((None, true));
        }
        if self.due.is_none() && now.abs_diff(self.swept) >= SWEEP_S {
            self.due = Some((journal::day(now.saturating_sub(SWEEP_DAYS * 86_400)), false));
        }
        if let Some((since, full)) = self.due.take() {
            self.asks += 1;
            self.asked.insert(self.asks, Asked { at: now, full });
            self.swept = now;
            out.list = Some(Listing { n: self.asks, since });
        }
    }

    /// The pool's fields of its heartbeat: the term it leads.
    fn beat(&self, io: &dyn Io) -> Beat {
        let leads = self.lead.as_ref().map(|l| l.term.term);
        Beat {
            member: self.me.id.clone(),
            host: self.me.host.clone(),
            app: self.me.app.clone(),
            beat: io.now(),
            leads,
            addresses: Vec::new(),
            reach: Vec::new(),
            checks_map: true,
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

    /// A Mac's NAS calls counted (creates, whole writes, reads, stats, listings, removes).
    struct Counted<'a>(&'a Mac<'a>, Cell<u64>);

    impl Counted<'_> {
        /// The calls since the last ask.
        fn take(&self) -> u64 {
            self.1.replace(0)
        }
        fn one(&self) {
            self.1.set(self.1.get() + 1);
        }
    }

    impl Nas for Counted<'_> {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            self.one();
            self.0.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.one();
            self.0.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.one();
            self.0.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.one();
            self.0.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.one();
            self.0.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.one();
            self.0.remove(path)
        }
    }

    impl Io for Counted<'_> {
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
        Heard::default()
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

    /// Moves the declaration of the lead to member `to` (the owner's edit, by hand).
    fn declare(mem: &Mem, to: &str) {
        mem.write_whole(term::LEAD, format!("{{\"member\": \"{to}\"}}").as_bytes()).unwrap();
    }

    #[test]
    fn a_member_never_claims_the_lead() {
        // (Task #881) A, term 1's maker, declares itself the lead. B, able, never makes a term:
        // not with A leading for hours, not with A gone for a day, not with a term naming B (an
        // elected lead's, before #881), which A follows with its own.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        assert_eq!(term::declared(&mem).unwrap().map(|d| d.member), Some(A.to_string()));
        assert_eq!((step(&mut b, &ib, able()).leads, b.declared()), (None, Some(A)));
        for _ in 0..240 {
            ia.pass(30);
            ib.pass(30);
            assert_eq!(step(&mut b, &ib, able()).leads, None);
            assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        }
        // A gone a day: B waits.
        for _ in 0..(86_400 / 600) {
            ib.pass(600);
            let o = step(&mut b, &ib, able());
            assert_eq!((o.leads, o.term), (None, 1), "{:?}", o.events);
        }
        assert!(!mem.exists(&term::path(2)).unwrap(), "no term made");
        // A term naming B: B doesn't lead it; A, back, makes the next.
        let t2 = Term {
            term: 2,
            member: B.into(),
            host: "MacBook-Air".into(),
            app: V2.into(),
            since: ib.now(),
            how: "taken over by MacBook-Air".into(),
            from: 1,
        };
        mem.create_new(&term::path(2), &serde_json::to_vec_pretty(&t2).unwrap()).unwrap();
        ib.pass(20);
        assert_eq!(step(&mut b, &ib, able()).leads, None);
        ia.wall.set(ib.now());
        let o = step(&mut a, &ia, able());
        assert!(o
            .events
            .iter()
            .any(|e| matches!(e, Event::Made { term: 3, how } if how == "the declared lead, after MacBook-Air")));
        // (Taken up once B's last save can't read stale.)
        ia.pass(STALE_S);
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
    }

    #[test]
    fn with_the_lead_gone_a_member_writes_its_jobs_hand_offs_and_waits() {
        // (Task #881) A silent; B's jobs end: their hand-offs go to the journal, B tells A of them
        // (its mail waits for A), and B takes nothing over; A back, it merges them.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        caught_up(&mut a, &ia, able());
        step(&mut b, &ib, able());
        ib.pass(3 * 3600);
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        let o = step(&mut b, &ib, Heard { entries: vec![e], ..able() });
        assert_eq!(o.leads, None);
        assert!(matches!(journal::read(&mem, &key).unwrap(), journal::Read::Entry(_)), "written");
        assert!(o.send.iter().any(|(to, m)| to == A && matches!(m, Msg::Tell(k) if k.contains(&key))), "{:?}", o.send);
        for _ in 0..10 {
            ib.pass(600);
            assert_eq!(step(&mut b, &ib, able()).leads, None);
        }
        assert!(!mem.exists(&term::path(2)).unwrap());
        // A back (its sleep a gap: it re-asserts), told: merged.
        ia.wall.set(ib.now());
        let o = caught_up(&mut a, &ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() });
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(a.records().unwrap().handles(&key), "{:?}", o.events);
    }

    #[test]
    fn the_owners_edit_of_the_declaration_moves_the_lead() {
        // The lead moved by hand (another Mac in the mini's place): A steps down at its next step,
        // B makes the next term from A's records.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        caught_up(&mut a, &ia, Heard { entries: vec![entry(A, 1, 1, "6-1-1")], ..able() });
        step(&mut b, &ib, able());
        declare(&mem, B);
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert!(
            o.leads.is_none()
                && o.events
                    .iter()
                    .any(|e| matches!(e, Event::SteppedDown { term: 1, why } if why.contains("declared"))),
            "{:?}",
            o.events
        );
        ib.pass(20);
        let o = step(&mut b, &ib, able());
        assert_eq!(o.leads, None, "waits `STALE_S` after its term is made: {:?}", o.events);
        ib.pass(STALE_S);
        let o = step(&mut b, &ib, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert_eq!(b.records().unwrap().keys.recorded("unit", "6-1-1"), Some("k1"), "from A's records");
    }

    /// docs/coordination.md step 8a, measured: a member's step learns its lead and term from its
    /// lead's beat answer (`Heard::lead_says`) with no NAS call, where it read the declaration and
    /// stat'd the next term each step (and at its first, the lead's hint, the term's file, and
    /// more); its lead's re-assertion learnt the same way. With no word from its lead (lately, or
    /// ever: an older lead), it reads the NAS, so it learns of the owner's move of the lead to it.
    #[test]
    fn a_member_learns_its_lead_s_term_from_its_answer_with_no_nas_call() {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        let says = |a: &Driver| Heard { lead_says: a.current().lead.clone(), ..able() };
        // Before: from the NAS.
        let nb = Counted(&ib, Cell::new(0));
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!((b.step(&nb, able(), &any).term, b.declared()), (1, Some(A)));
        let first = nb.take();
        ib.pass(20);
        b.step(&nb, able(), &any);
        let each = nb.take();
        // (Its first: the declaration, the hint, the hinted term stat'd, the next, the term read, the
        // next again, the declaration; then each step the next stat'd and the declaration.)
        assert_eq!((first, each), (7, 2));
        // After: from its lead's word.
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        for _ in 0..30 {
            ib.pass(20);
            let o = b.step(&nb, says(&a), &any);
            assert_eq!((o.term, o.leads, b.declared()), (1, None, Some(A)), "{:?}", o.events);
        }
        assert_eq!(nb.take(), 0, "no NAS call in 30 steps");
        // A re-asserts (term 2): B learns it from A's word.
        ia.pass(STALL_S + 1);
        assert_eq!(step(&mut a, &ia, able()).leads, Some(2));
        ib.pass(20);
        let o = b.step(&nb, says(&a), &any);
        assert_eq!((o.term, b.current().lead.as_ref().map(|t| t.member.as_str())), (2, Some(A)));
        assert_eq!(nb.take(), 0);
        // A word older than its view (a stale answer) is passed over for the NAS.
        let old = Term { term: 1, ..a.current().lead.clone().unwrap() };
        ib.pass(20);
        assert_eq!(b.step(&nb, Heard { lead_says: Some(old), ..able() }, &any).term, 2);
        assert!(nb.take() > 0);
        // The owner moves the lead to B. While A's answers say it leads, B leads nothing; once B
        // hears no word (A stepped down, or away), it reads the declaration, and takes the lead.
        declare(&mem, B);
        ib.pass(20);
        assert_eq!(b.step(&nb, says(&a), &any).leads, None);
        assert_eq!(b.declared(), Some(A));
        ib.pass(20);
        let o = b.step(&nb, able(), &any);
        assert_eq!(b.declared(), Some(B));
        assert!(
            o.events
                .iter()
                .any(|e| matches!(e, Event::Made { term: 3, how } if how == "the declared lead, after Mac-mini")),
            "{:?}",
            o.events
        );
    }

    #[test]
    fn the_live_pools_move_waits_for_the_owners_declaration() {
        // (Task #881's move, the pool building) A leads term 2, made by an app before #881: no
        // declaration on the NAS. B, on the new app first, declares nothing and leads nothing; A,
        // restarted into it, declares nothing either (a lead elected before #881 may not be the
        // mini) and leads nothing; the owner's declaration written, A re-asserts.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), a.saved());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(2));
        mem.remove(term::LEAD).unwrap();
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        for _ in 0..3 {
            ib.pass(20);
            assert_eq!((step(&mut b, &ib, able()).leads, b.declared()), (None, None));
        }
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        for _ in 0..3 {
            ia.pass(20);
            let o = step(&mut a, &ia, able());
            assert_eq!((o.leads, a.declared()), (None, None), "{:?}", o.events);
        }
        assert_eq!(term::declared(&mem).unwrap(), None, "no Mac declares itself past term 1");
        assert!(!mem.exists(&term::path(3)).unwrap());
        declare(&mem, A);
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert_eq!((o.leads, a.declared()), (Some(3), Some(A)), "{:?}", o.events);
        ib.pass(20);
        assert_eq!((step(&mut b, &ib, able()).leads, b.declared()), (None, Some(A)));
    }

    #[test]
    fn a_lead_restarted_into_a_development_build_re_asserts() {
        // (No app rule nor stand-down since #881: the lead is fixed, whatever app it runs.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), a.saved());
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Made { term: 2, how } if how == "restarted: re-asserted")));
    }

    #[test]
    fn a_new_lead_is_caught_up_once_its_listing_is_merged() {
        // After a restart the lead's records lack an entry only a listing finds (its member gone,
        // never told of it). (Review N2: its duties and `fresh` were all the agent had, and both
        // were true before the listing came back.)
        let mem = Mem::default();
        setup(&mem);
        let ib = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ib, able());
        let x = journal::write(&mem, &entry(C, 1, 7, "6-1-1")).unwrap();
        ib.pass(700);
        let mut b = Driver::unlocked(member(A, "Mac-mini", V1), a.saved());
        let o = step(&mut b, &ib, able());
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
    fn a_lead_lists_the_journal_only_while_a_member_writes_it() {
        // (docs/coordination.md step 6, the rollout gate) No member writes the journal: the lead
        // asks for no listing, and is caught up once what's posted it is merged and saved.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let http = || Heard { http_only: true, ..able() };
        let o = step(&mut a, &ia, http());
        assert_eq!(o.leads, Some(1), "{:?}", o.events);
        assert!(o.list.is_none() && o.caught_up, "no listing: {:?}", o.list);
        // A hand-off posted: merged from memory, saved, then acknowledged (`merged`).
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e.clone()], ..http() });
        assert_eq!(o.merged, std::slice::from_ref(&key));
        assert!(Records::load(&mem, 1).unwrap().unwrap().handles(&key), "in the snapshot saved");
        assert!(!mem.exists(&journal::path(&key)).unwrap(), "no journal");
        // Posted again: passed over, acknowledged again, merged once.
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e], ..http() });
        assert!(o.merged == [key.clone()] && !o.events.iter().any(|e| matches!(e, Event::Merged { .. })), "{o:?}");
        // A member writing the journal heard: a listing of every day asked for, caught up only once
        // it's merged.
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert!(!o.caught_up && o.list.as_ref().is_some_and(|l| l.since.is_none()), "{:?}", o.list);
    }

    #[test]
    fn the_lead_merges_its_own_jobs_hand_offs_from_memory() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ia, able());
        let e = entry(A, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { entries: vec![e], ..able() });
        assert!(o.events.iter().any(|e| matches!(e, Event::Merged { applied, .. } if applied.contains(&key))));
        assert!(!mem.exists(&journal::path(&key)).unwrap(), "not written");
        assert_eq!(a.mine().unacked(), 0, "dropped once saved");
    }

    #[test]
    fn a_take_up_never_starts_from_records_older_than_it_saved() {
        // (docs/coordination.md step 6) What a save acknowledged is on no member's disk now: a
        // restart whose read of its newest records is stale waits for them rather than lose it.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ia, able());
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { handed: vec![e], http_only: true, ..able() });
        let saved = a.saved();
        assert_eq!(saved.records, Some((1, Records::load(&mem, 1).unwrap().unwrap().seq)));
        // (Its newest snapshot read stale: gone a while.)
        let newest = mem.read(&records::path(1)).unwrap().unwrap();
        mem.remove(&records::path(1)).unwrap();
        ia.pass(20);
        let mut b = Driver::unlocked(member(A, "Mac-mini", V1), saved);
        let o = step(&mut b, &ia, able());
        assert_eq!(o.leads, None, "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Waits { what: "take up", why } if why.contains("older"))));
        mem.write_whole(&records::path(1), &newest).unwrap();
        ia.pass(20);
        let o = step(&mut b, &ia, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(b.records().unwrap().handles(&key));
    }

    /// A Mac whose reads of `stale` paths answer as they were (their bytes; None: not there), and
    /// whose first whole write of a path holding `before.0` runs `before.1` first.
    struct Racing<'a> {
        mac: &'a Mac<'a>,
        stale: BTreeMap<String, Option<Vec<u8>>>,
        before: std::cell::RefCell<Option<(String, Box<dyn FnOnce() + 'a>)>>,
    }

    impl Nas for Racing<'_> {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            self.mac.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            let hit = self.before.borrow().as_ref().is_some_and(|(p, _)| path.contains(p.as_str()));
            if hit {
                let (_, f) = self.before.borrow_mut().take().unwrap();
                f();
            }
            self.mac.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            match self.stale.get(path) {
                Some(b) => Ok(b.clone()),
                None => self.mac.read(path),
            }
        }
        fn exists(&self, path: &str) -> Result<bool> {
            match self.stale.get(path) {
                Some(b) => Ok(b.is_some()),
                None => self.mac.exists(path),
            }
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.mac.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.mac.remove(path)
        }
    }

    impl Io for Racing<'_> {
        fn now(&self) -> u64 {
            self.mac.now()
        }
        fn awake(&self) -> u64 {
            self.mac.awake()
        }
    }

    #[test]
    fn a_lead_moved_mid_step_acknowledges_nothing() {
        // (Task #878 step 6 review) The owner moves the lead to B while A's step merges a hand-off:
        // B makes term 2 before A's save lands. A, finding term 2 after its save, acknowledges
        // nothing (its member keeps it, for B); B takes up only `STALE_S` after making its term,
        // from A's last save.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let http = || Heard { http_only: true, ..able() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, http()).leads, Some(1));
        let b = std::cell::RefCell::new(Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default()));
        step(&mut b.borrow_mut(), &ib, http());
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.pass(20);
        ib.wall.set(ia.now());
        let race = Racing {
            mac: &ia,
            stale: BTreeMap::new(),
            before: std::cell::RefCell::new(Some((
                records::path(1),
                Box::new(|| {
                    declare(&mem, B);
                    let o = step(&mut b.borrow_mut(), &ib, http());
                    assert_eq!(o.leads, None, "{:?}", o.events);
                    assert!(o
                        .events
                        .iter()
                        .any(|e| matches!(e, Event::Waits { what: "take up", why } if why.contains("follows"))));
                }),
            ))),
        };
        let o = a.step(&race, Heard { handed: vec![e], ..http() }, &any);
        assert!(mem.exists(&term::path(2)).unwrap() && Records::load(&mem, 1).unwrap().unwrap().handles(&key));
        assert!(o.merged.is_empty(), "nothing acknowledged: {:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Waits { what: "acknowledge hand-offs", .. })));
        drop(race);
        ia.pass(20);
        assert_eq!(step(&mut a, &ia, http()).leads, None, "A steps down");
        let mut b = b.into_inner();
        ib.pass(STALE_S - 30);
        assert_eq!(step(&mut b, &ib, http()).leads, None);
        ib.pass(30);
        let o = step(&mut b, &ib, http());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(b.records().unwrap().handles(&key), "A's last save");
    }

    #[test]
    fn a_moved_lead_s_last_acknowledgement_is_in_the_next_lead_s_records() {
        // (Task #878 step 6 review) B has made term 2; A's reads of the declaration and of term 2
        // are stale, so it merges, saves and acknowledges a hand-off (its member drops it). B, taking
        // up `STALE_S` after its term was made, reads that save: nothing lost.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let http = || Heard { http_only: true, ..able() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, http()).leads, Some(1));
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        step(&mut b, &ib, http());
        let was = mem.read(term::LEAD).unwrap();
        declare(&mem, B);
        ib.pass(20);
        assert_eq!(step(&mut b, &ib, http()).leads, None, "waits");
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.wall.set(ib.now());
        let stale = Racing {
            mac: &ia,
            stale: [(term::LEAD.to_string(), was), (term::path(2), None)].into(),
            before: Default::default(),
        };
        let o = a.step(&stale, Heard { handed: vec![e], ..http() }, &any);
        assert_eq!(o.merged, std::slice::from_ref(&key), "acknowledged, stale: {:?}", o.events);
        ib.pass(STALE_S);
        let o = step(&mut b, &ib, http());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(b.records().unwrap().handles(&key), "in B's records");
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
        // Lost again, onto a development build: it re-asserts all the same.
        ia.pass(20);
        let mut a = Driver::unlocked(member(A, "Mac-mini", "development"), Saved::default());
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
        assert!(c.mine().unacked() == 0 && o.send.is_empty(), "{:?}", o.send);
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
    /// no catalogue back; once the check takes it (the lead on that app), it's merged.
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
        // for ever: no catalogue, no sweep.
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
        // name; whole on the share), and no catalogue went out: one waits for every entry read.
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
        // jobs left unwritten is merged, the same bytes (the lead's own from memory, never the
        // journal's); what A led counts for nothing.
        // (Re-review F5.)
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let e = entry(A, 1, 4, "6-1-1");
        let key = e.key().unwrap();
        let mut saved = Saved { member: A.into(), led: 3, ..Default::default() };
        saved.mine.add(e).unwrap();
        let d = "m-000000000000000d";
        let mut dr = Driver::unlocked(member(d, "Mac-mini", V1), saved);
        let s = dr.saved();
        assert_eq!((s.member.as_str(), s.led, s.mine.unwritten().count()), (d, 0, 1));
        assert_eq!(step(&mut dr, &ia, able()).leads, Some(1));
        assert!(matches!(journal::read(&mem, &key).unwrap(), journal::Read::Missing), "not written");
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
        // catalogue and no GC, and nothing said why.) Nothing else is asked for while it's out; two
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
        // create cut short; term 1's lead saves twice, writing today's three files after each; it
        // re-asserts, its reads stale: term 1's lead's snapshot not there yet, its
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
        let t1 = term::bootstrap(&Cut(&mem), &member(A, "Mac-mini", V1), T0).unwrap().unwrap();
        assert!(Records::newest(&mem, 1, false).unwrap().is_some(), "written whole before term 1 is made");
        let mut r = records::start(&mem, &t1, None, false, None).unwrap();
        r.save(&mem).unwrap();
        let old_jobs = mem.read("state/build/jobs.json").unwrap().unwrap();
        let e = entry(A, 1, 1, "6-1-4");
        r.apply(&e.key().unwrap(), &e);
        r.save(&mem).unwrap();
        let t2 =
            Term::after(&term::current(&mem).unwrap(), &member(A, "Mac-mini", V1), "re-asserted after a gap", T0 + 60)
                .unwrap();
        let stale = Stale(
            &mem,
            vec![
                (records::path(1), None),
                (records::FIRST.into(), Some(Vec::new())),
                ("state/build/jobs.json".into(), Some(old_jobs)),
            ],
        );
        let err = records::start(&stale, &t2, None, false, None).unwrap_err();
        assert!(err.is::<records::FirstUnreadable>(), "tried again: {err:#}");
        let r2 = records::start(&mem, &t2, None, false, None).unwrap();
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
    fn a_take_up_whose_save_landed_unanswered_is_tried_again() {
        // A, restarted, takes up the term it made, and its save of the records lands with its answer
        // lost: the records there are this process's own try's, taken up again next step, numbered
        // on. (Not re-asserted, as a term whose records are there is when its state is older: a
        // backup restored.)
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
        let ib = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        assert_eq!(step(&mut a, &ib, able()).leads, Some(1));
        ib.pass(20);
        let mut b = Driver::unlocked(member(A, "Mac-mini", V1), a.saved());
        let o = b.step(&Lost(&ib, Cell::new(true)), able(), &any);
        assert!(o.leads.is_none() && Records::load(&mem, 2).unwrap().is_some_and(|r| r.seq == 1), "{:?}", o.events);
        ib.pass(20);
        let o = step(&mut b, &ib, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::TookUp { term: 2, .. })), "{:?}", o.events);
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
}
