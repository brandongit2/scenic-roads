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
//! Two processes of one member would lead at once, each saving the records over the other's.
//!
//! The agent steps it about every 20 s, one step at a time: its loop's step, and while that loop is
//! busy (a long plan, a job stopped) its pool's own thread's (crate::agent::pool's `Ticker`), each
//! under the run's lock. A step:
//!
//! - **does its NAS operations through `Io`** (crate::pool::nas::Nas, two clocks, its own disk's
//!   copy of the records and its store's epoch): the lead's declaration read, the records' save
//!   (about 14 MB), and while a member writes the journal its jobs' entries written and entries
//!   merged for `BUSY_S` at most (the rest in the next steps); a take-up's read of the one path;
//!   never a listing of the journal, never a sleep, no thread of its own. What's slow (a listing of
//!   the journal, 3 to 33 s a folder on the share under load) it asks for in its output
//!   (`Out::list`), for the agent to make off the loop and hand back, with the ask's number, in a
//!   later step (`Heard::listed`): every listing asked for, however long it takes, one that fails
//!   made again. It asks for no other while its last is out; one out `OVERDUE_S` with none handed
//!   back meanwhile (lost, or that slow) it says, and asks again, and whichever comes back counts;
//! - **reads the clocks itself**, where a decision needs the time: the agent passes no time in;
//! - **takes what was heard since the last step** (`Heard`): the members' messages to it, the
//!   build pages' asks, the hand-offs of its jobs that ended, those its members posted it, the
//!   listing it asked for, its lead's word;
//! - **gives what to do now** (`Out`): the messages to send, by member id, over the pool's API
//!   (best effort: a message lost is told again or made up for); the pool's fields of this Mac's
//!   heartbeat, to write with the rest of it, stamped as it's written; the epoch it leads, if it
//!   leads, and what it may do as that now: grant jobs and plan (`duties`); publish a catalogue
//!   only while its records reflect what's handed off (`caught_up`), and sweep (GC) only on a step
//!   whose own read of the declaration found it the lead (`vouched`); a listing to make; and what
//!   happened (`Event`s: epochs taken up and stepped down from; errors), for the history and the
//!   log;
//! - **never fails**: an error stops only the duty that met it (said in an `Event::Failed`), and
//!   the step goes on.
//!
//! What the driver does itself, through `Io`, is what decides safety: taking up the records and
//! saving them (crate::pool::records), merging what's handed off. What it leaves to the agent can't
//! break §4's invariants: granting and the coordinator, the merge's checks (`Check`, passed to every
//! step), the duties, the heartbeat's other fields, the messages' transport, and persisting `Saved`
//! (its state between processes, naming its member: its jobs' hand-offs, kept until a lead's saved
//! records name them; the records it last saved) after every step that changed it, **before acting
//! on that step's `Out`** (a take-up's grants, its messages, its acknowledgements). **A job's
//! hand-off is kept until a `Saved` from a step it was handed to is on the agent's disk**: before
//! that, a crash loses it. A `Saved` that's lost, or another member's (this Mac's member file lost,
//! a new id made; or a copy of another Mac's folder), counts for nothing but its jobs' hand-offs not
//! written yet.
//!
//! # The fixed lead (§6, task #881; no terms, docs/coordination.md step 8c)
//!
//! One member leads: the one `state/build/lead-member.json` declares (buildstore::records::lead,
//! the Mac mini). There is no election, handover or takeover, and no term: the lead leads the epoch
//! of its coordinator's store (bumped as the store opens, so once a process: `Io::epoch`), which
//! numbers its records' snapshots and its leases. A member never leads; with the lead gone, members
//! finish the jobs they hold, keep their hand-offs, and wait for it. No lead declared yet, no one
//! leads until the owner writes it; in a new pool (no records saved at the one path), the first Mac
//! in it declares itself, once, with create-new.
//!
//! The lead takes up the records once a process (`take_up`): from its own, kept on its disk (step
//! 8b), unless the one path holds newer ones (another lead saved them: the owner moved the lead
//! here, or away and back), or it has none (its disk's copy lost); then from the one path's, read
//! once `STALE_S` has passed since this process found itself declared, so that the last save the
//! lead before it acknowledged is in them (that lead acknowledges only while a read of the
//! declaration after its save still names it: `lead_step`). Its epoch is raised past the records'
//! it starts from, so (epoch, seq) orders every snapshot, and lease ids order, across the owner's
//! moves (and past the pool's last term, at the move from terms). It leads until the declaration
//! names another member, read each step.
//!
//! A member learns its lead from its lead's beat answer while one came lately (`Heard::lead_says`,
//! docs/coordination.md step 8a), with no NAS call; without one (a lead not leading, or none
//! answering: the owner may have moved the lead here) it reads the declaration, as the lead does
//! every step.

use super::beat::Beat;
use super::journal::{self, Entry, Mine};
use super::nas::Nas;
use super::records::{self, Check, Records};
use super::{Member, MemberLock};
use buildstore::records::lead;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// More than this unaccounted for, between steps or within one (its wall clock and its awake clock
/// apart: the Mac slept, or its wall clock was set), and a lead whose members write the journal
/// lists it again before it's caught up: what they told it meanwhile was lost (s).
pub const GAP_S: u64 = 60;
/// The longest a read is taken to stay stale (s). Measured (task #645, tools/check/nas-probe.py, two
/// Macs, 24 Oct, idle share, 570 trials): another Mac's rename over a file was read within 1.1 s,
/// a new file opened within 0.12 s; a stat or listing kept a removed file, and a listing missed a
/// new one, up to 33.5 s (macOS's SMB attribute and folder caches, ~30 s). 600 s is 18 times that
/// tail, with room for a share under load (task #630 saw one name stale for 40 minutes: past any
/// such bound, which `MOVED_ON_S` refuses as damage). A take-up that reads the one path waits this
/// long awake after its process found itself declared, so the last save the lead before it
/// acknowledged is in what it reads.
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
/// A lead saves its records again, changed or not, once this long awake has passed since its last
/// save reached the NAS (s), so that an older save landing late over the one path (records::ONE)
/// stays there this long at most (docs/coordination.md step 8b). Within one process a write past
/// its deadline holds back the next to its path until it ends (crate::agent::nasread's
/// `InFlight`); one an earlier process left running as it stopped, or a lead's before the owner
/// moved it, may land after any: readers read it meanwhile, and a take-up passes over it (its
/// lead's own copy is newer).
pub const RESAVE_S: u64 = STALE_S;

/// What the driver needs of the world: the NAS's operations, this Mac's clocks, its own copy of
/// the records it leads, and its store's epoch.
pub trait Io: Nas {
    /// This Mac's wall clock: unix seconds (§6.7: no NAS clock; each Mac's own).
    fn now(&self) -> u64;
    /// Seconds this Mac has been awake since some start: a clock that stops while it sleeps (on
    /// macOS, Rust's `Instant`), so the wall clock's lead over it is the time it slept.
    fn awake(&self) -> u64;
    /// The records this Mac last saved leading, as kept on its own disk (`keep`); None when it has
    /// none (it never led, or its disk lost them).
    fn kept(&self) -> anyhow::Result<Option<Vec<u8>>>;
    /// Keeps the records it saves leading on its own disk, whole and flushed, before they go to the
    /// NAS (docs/coordination.md step 8b): what its next take-up starts from, a late NAS write
    /// notwithstanding.
    fn keep(&self, records: &[u8]) -> anyhow::Result<()>;
    /// The epoch of this Mac's store (its coordinator's: docs/coordination.md §4.3, bumped as it
    /// opens), raised past `past` first, durably, when it isn't: what a take-up leads, its records
    /// and its leases numbered by it. An error when there's no store open (not the lead's process
    /// yet) or it can't be raised (its disk failing).
    fn epoch(&self, past: u64) -> anyhow::Result<u64>;
}

/// A message between members, over the pool's API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// From a member to the lead it knows: its journal entries that lead hasn't acknowledged. (A
    /// member writes to the journal, and tells, only while its lead doesn't take hand-offs by HTTP:
    /// the rollout gate of docs/coordination.md step 6, deleted with it.)
    Tell(Vec<String>),
    /// From the lead, in `epoch` (under the key `term` older apps read): those entries are in a
    /// snapshot of its records it saved (applied, passed over or refused). (`horizon` always empty:
    /// read by apps before step 6, which forgot what they had acknowledged before it; deleted with
    /// the gate.)
    Ack {
        #[serde(rename = "term")]
        epoch: u64,
        keys: Vec<String>,
        #[serde(default)]
        horizon: String,
    },
    /// A build page's ask to cancel a job (§11), passed on to the lead by the member whose page
    /// asked; sent only to a lead on the sender's own app (an older one wouldn't read it).
    Cancel(jobrt::control::Cancel),
    /// A build page's ask to try a failed target again now (§11): the lead forgets its failures
    /// (the coordinator's, and its own back-off). Passed on as `Cancel` is.
    Retry(jobrt::control::Retry),
}

/// A build page's ask the lead acts on (`Out::acts`): from its own page, or a member's (`Msg`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Cancel(jobrt::control::Cancel),
    Retry(jobrt::control::Retry),
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
    /// store's index since (buildstore::rawpack::name_handed, the index saved): taken off the records,
    /// saved with this step's snapshot. (One named and not taken off yet, a restart between, is
    /// named again: passed over unread.)
    pub named_raw: Vec<String>,
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
    /// A member: the lead its lead's beat answered it is (crate::coord::held::Answer::leading),
    /// that answer fresh (within crate::coord::beats::PAGE_AWAY_S: crate::coord::beats::Beats::
    /// lead_says). Its lead is learnt from it, and the NAS's declaration isn't read; None (a lead
    /// not leading, none answering lately: the owner may have moved the lead) reads it
    /// (docs/coordination.md step 8a).
    pub lead_says: Option<Member>,
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
    /// The epoch it leads in.
    pub leads: Option<u64>,
    /// Leading, this step's own read of the declaration found it the lead. Only then do its
    /// coordinator's beat answers say it leads (crate::agent::lead::after,
    /// `held::Answer::leading`), which its members take in place of the NAS (docs/coordination.md
    /// step 8a): a lead whose share fails that read, keeping its last view, says nothing, so its
    /// members read the declaration and find an owner's move. And only then does its agent sweep
    /// (GC): no other lead declared.
    pub vouched: bool,
    /// Leading, whether it may grant jobs and plan now (its member's lock known held).
    pub duties: bool,
    /// Leading, its records reflect what's handed off: every hand-off posted, told or listed is
    /// merged and saved (on a share under load a loop leaves some to the next; a journal entry never
    /// read whole is refused after `UNREADABLE_S`, or `MOVED_ON_S`), and while a member writes the
    /// journal (`Heard::http_only` not said) a listing of every day (its take-up's, or the daily
    /// one) asked for less than `RELIST_S` ago is merged too. A catalogue waits for it (and
    /// `duties`); GC too (and `vouched`): an entry not merged yet may hold uploads the records don't
    /// name.
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
    /// step's is empty), sends nothing, steps no more, and leaves the pool.
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
    /// Took up the records in epoch `epoch`, from where `how` says: its coordinator grants in it.
    TookUp { epoch: u64, how: String },
    /// No longer leads (in epoch `epoch`): why.
    SteppedDown { epoch: u64, why: String },
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
/// it. (An app before docs/coordination.md step 8c kept its terms beside: read past.)
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
    /// The records it last saved leading (epoch, seq; a term's, before step 8c): a take-up never
    /// starts from older ones, whose saves acknowledged hand-offs no member keeps now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<(u64, u64)>,
}

/// The records this Mac leads.
#[derive(Clone, Debug)]
struct Lead {
    /// The epoch it leads in, and when it took the records up (this Mac's clock).
    epoch: u64,
    since: u64,
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
    /// Changes not saved to the NAS yet (on its own disk or not), and when its last save reached
    /// the NAS (awake clock: `RESAVE_S`).
    dirty: bool,
    saved_at: u64,
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
    saved: Saved,
    /// Its saved state is its own: what it saved leading before is known.
    known: bool,
    lead: Option<Lead>,
    /// A take-up tried and not kept on its disk: its records as tried (numbered on, in its epoch).
    taking: Option<Records>,
    /// Its records as it last led, for its next take-up to start from (it stepped down: the
    /// declaration named another member a while).
    spare: Option<Records>,
    /// Declared and not leading: since when (awake clock) this process has found itself declared,
    /// and the moment (awake clock) before which its take-up waits (`STALE_S`).
    declared_at: Option<u64>,
    wait_until: Option<u64>,
    /// The listing due next (a take-up's: every day); those asked for and not back, by number (any
    /// counts when it's back; a day old, forgotten); the asks' number; when it last asked, and when
    /// a listing last came back (this Mac's clock).
    due: Option<(Option<String>, bool)>,
    asked: BTreeMap<u64, Asked>,
    asks: u64,
    swept: u64,
    back: u64,
    /// Leading: a member writes the journal (`Heard::http_only` not said), as its last step heard.
    journal: bool,
    /// Its clocks (wall, awake) at its last step's end: a gap since (`GAP_S`) asks for a listing.
    clocks: Option<(u64, u64)>,
    /// The lead's declaration as last read (lead::declared), or as its lead's answer said it: the
    /// member that leads; and that member as last known (its app known only from its answer, or
    /// when it's this Mac).
    declared: Option<String>,
    lead_member: Option<Member>,
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
            saved,
            known,
            lead: None,
            taking: None,
            spare: None,
            declared_at: None,
            wait_until: None,
            due: None,
            asked: BTreeMap::new(),
            asks: 0,
            swept: 0,
            back: 0,
            journal: true,
            clocks: None,
            declared: None,
            lead_member: None,
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

    /// The epoch it leads in.
    pub fn leads(&self) -> Option<u64> {
        self.lead.as_ref().map(|l| l.epoch)
    }

    /// When it took up the records it leads (this Mac's clock).
    pub fn leads_since(&self) -> Option<u64> {
        self.lead.as_ref().map(|l| l.since)
    }

    /// The member the lead's declaration names, as last read or as its lead's answer said (None:
    /// none declared yet, or not read).
    pub fn declared(&self) -> Option<&str> {
        self.declared.as_deref()
    }

    /// The lead as it last knew it (its app empty when known only from the declaration: no answer
    /// from it lately): its members' mail goes to it.
    pub fn lead(&self) -> Option<&Member> {
        self.lead_member.as_ref()
    }

    /// The records it leads, as it last merged them (saved or not: `Records::seq` says which
    /// snapshot they are), for the agent to read; never to change.
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
                        epoch: l.epoch,
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
            return out;
        }
        let start = (io.now(), io.awake());
        let busy = start.1 + BUSY_S;
        if self.clocks.is_some_and(|was| gap(was, start)) {
            self.slept();
        }
        // Its jobs' entries, kept whole.
        for e in heard.entries {
            if let Err(err) = self.saved.mine.add(e) {
                out.events.push(Event::Failed { what: "keep an entry", why: format!("{err:#}") });
            }
        }
        self.saved.mine.acked_all(&heard.merged);
        self.journal = !heard.http_only;
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
        // Who leads: its lead's word (of another member) in place of the NAS's declaration.
        let (mine, read) = match heard.lead_says.filter(|m| m.id != self.me.id) {
            Some(m) => {
                self.declared = Some(m.id.clone());
                self.lead_member = Some(m);
                (false, false)
            }
            None => self.declare(io, &mut out),
        };
        // A member's entries to the journal, for a lead that doesn't take them by HTTP (the gate);
        // the declared lead merges its own from memory.
        if !mine && heard.route == Route::Journal {
            for (k, err) in self.saved.mine.write_while(io, &|| io.awake() < busy) {
                out.events.push(Event::Failed { what: "write an entry", why: format!("{k}: {err:#}") });
            }
        }
        // A lead leads while it's declared; the declared lead, not leading, takes the records up.
        if self.lead.is_some() && !mine {
            let why =
                if self.declared.is_some() { "another member is the declared lead" } else { "no lead is declared" };
            self.step_down(&mut out, why);
        }
        if mine {
            if self.lead.is_none() {
                self.take_up(io, &mut out);
            }
        } else {
            (self.declared_at, self.wait_until) = (None, None);
        }
        if self.lead.is_some() {
            let given = Given { tells, listed: heard.listed, handed: heard.handed, named_raw: &heard.named_raw };
            self.lead_step(io, &mut out, given, check, busy, read);
        }
        // The pages' asks: acted on leading, else passed on to a lead on this app (an older app's
        // mail reader would fail on them).
        if self.lead.is_some() {
            out.acts.extend(acts);
        } else if !acts.is_empty() {
            match self.lead_member.as_ref().filter(|m| m.id != self.me.id) {
                Some(m) if m.app == self.me.app => out.send.extend(acts.into_iter().map(|a| (m.id.clone(), a.msg()))),
                Some(m) if m.app.is_empty() => out.events.push(Event::Failed {
                    what: "pass the build page's ask on to the lead",
                    why: format!("no word from {} lately: its app isn't known", m.host),
                }),
                Some(m) => out.events.push(Event::Failed {
                    what: "pass the build page's ask on to the lead",
                    why: format!("the lead runs app {}, this Mac {}: ask on the lead's own page", m.app, self.me.app),
                }),
                None => out.events.push(Event::Failed {
                    what: "pass the build page's ask on to the lead",
                    why: "no lead now".into(),
                }),
            }
        }
        // Telling the lead it knows of its entries in the journal.
        if let Some(to) = self.declared.as_ref().filter(|d| **d != self.me.id) {
            let keys = self.saved.mine.to_tell();
            if !keys.is_empty() {
                out.send.push((to.clone(), Msg::Tell(keys)));
            }
        }
        // (A step that slept midway: what members told it meanwhile was lost too.)
        let end = (io.now(), io.awake());
        if gap(start, end) {
            self.slept();
        }
        self.clocks = Some(end);
        self.listings(io, &mut out);
        out.beat = self.beat(io);
        if let Some(l) = &self.lead {
            out.leads = Some(l.epoch);
            out.vouched = read;
            out.duties = true;
            out.caught_up = (!self.journal || l.listed_at.is_some_and(|at| io.now().saturating_sub(at) < RELIST_S))
                && !l.dirty
                && l.waiting.is_empty();
            out.listed_at = l.listed_at;
            out.held = l.held.iter().map(|(k, w)| (k.clone(), w.clone())).collect();
        }
        // (Its lock not known held this step: no duties, catalogue or sweep until it's checked
        // again.)
        if let Some(why) = unsure {
            out.events.push(Event::Failed { what: "check the member's lock", why });
            (out.duties, out.caught_up, out.vouched) = (false, false, false);
        }
        out
    }

    /// It slept (or its wall clock was set): leading while a member writes the journal, what its
    /// members told it meanwhile was lost, so it's caught up only by a listing asked for from now
    /// on, those out forgotten (their keys merged as any when back).
    fn slept(&mut self) {
        if let Some(l) = self.lead.as_mut().filter(|_| self.journal) {
            l.listed_at = None;
            self.due = Some((None, true));
            self.asked.clear();
        }
    }

    fn step_down(&mut self, out: &mut Out, why: &str) {
        if let Some(l) = self.lead.take() {
            out.events.push(Event::SteppedDown { epoch: l.epoch, why: why.to_string() });
            self.spare = Some(l.records);
        }
    }

    /// Whether this Mac is the declared lead (lead::declared), read each step (the last read kept
    /// when one fails), with whether it was read (`Out::vouched`). None declared: in a new pool (no
    /// records saved at the one path, nor its brief) the first Mac declares itself, once, with
    /// create-new; past that no Mac declares itself (the owner writes the declaration, and until
    /// then no one leads).
    fn declare(&mut self, io: &dyn Io, out: &mut Out) -> (bool, bool) {
        let read = match lead::declared(io) {
            Ok(Some(d)) => {
                self.know(Some(Member { id: d.member, host: d.host, app: String::new() }));
                true
            }
            Ok(None) => match io.exists(records::ONE).and_then(|one| Ok(one || io.exists(records::ONE_BRIEF)?)) {
                Ok(false) => match lead::declare(io, &self.me, io.now()) {
                    Ok(d) => {
                        self.know(d.map(|d| Member { id: d.member, host: d.host, app: String::new() }));
                        true
                    }
                    Err(e) => {
                        out.events.push(Event::Failed { what: "declare the lead", why: format!("{e:#}") });
                        false
                    }
                },
                Ok(true) => {
                    self.know(None);
                    true
                }
                Err(e) => {
                    out.events.push(Event::Waits { what: "read the lead's declaration", why: format!("{e:#}") });
                    false
                }
            },
            Err(e) => {
                out.events.push(Event::Waits { what: "read the lead's declaration", why: format!("{e:#}") });
                false
            }
        };
        (self.declared.as_deref() == Some(self.me.id.as_str()), read)
    }

    /// The declaration as read (`m`: its member, its app unknown): kept, its app too while it names
    /// the member last known (from its answer), or this Mac's own.
    fn know(&mut self, m: Option<Member>) {
        self.declared = m.as_ref().map(|m| m.id.clone());
        self.lead_member = m.map(|m| match &self.lead_member {
            _ if m.id == self.me.id => self.me.clone(),
            Some(was) if was.id == m.id && !was.app.is_empty() => Member { host: m.host, ..was.clone() },
            _ => m,
        });
    }

    /// The records it kept on its own disk leading (`Io::kept`), when its saved state is its own
    /// (another member's folder's, or one set aside, says nothing of what it led); one that can't be
    /// read is said, and counts as none.
    fn kept(&self, io: &dyn Io, out: &mut Out) -> Option<Records> {
        if !self.known {
            return None;
        }
        match io.kept().and_then(|b| b.map(|b| serde_json::from_slice::<Records>(&b)).transpose().map_err(Into::into)) {
            Ok(r) => r,
            Err(e) => {
                out.events.push(Event::Failed { what: "read its own records", why: format!("{e:#}") });
                None
            }
        }
    }

    /// Takes up the records (§6.2), this Mac the declared lead and not leading. It starts from its
    /// own (an earlier try's, its records as it last led, or kept on its disk: docs/coordination.md
    /// step 8b), none older than it saved last (`Saved::records`), unless the one path's are newer
    /// (another lead saved them: the owner moved the lead) or it has none (its disk's copy lost):
    /// then from the one path's, read again once `STALE_S` has passed since this process found
    /// itself declared, so the last save the lead before it acknowledged is in them; with none
    /// there nor a brief, and none saved by this Mac before (a new pool), today's files. It leads its store's epoch, raised
    /// past theirs (`Io::epoch`), its first snapshot saved. Not readable yet, or not kept on its
    /// disk: tried again next step, numbered on. Kept and not written to the NAS: it leads all the
    /// same, its NAS files written at its next step, nothing acknowledged until then.
    fn take_up(&mut self, io: &dyn Io, out: &mut Out) {
        let awake = io.awake();
        let since = *self.declared_at.get_or_insert(awake);
        if self.wait_until.is_some_and(|w| awake < w) {
            return;
        }
        // (No store to lead an epoch of yet: its agent's coordinator starts after its first step.)
        if let Err(e) = io.epoch(0) {
            return out.events.push(Event::Waits { what: "take up the records", why: format!("its epoch: {e:#}") });
        }
        let waited = awake.saturating_sub(since) >= STALE_S;
        let wait = |me: &mut Driver, out: &mut Out, why: &str| {
            me.wait_until = Some(since + STALE_S);
            out.events.push(Event::Waits {
                what: "take up the records",
                why: format!(
                    "{why}: read in {} s, once the last save of a lead before it can't read stale",
                    since + STALE_S - awake
                ),
            });
        };
        // (Nor from records older than those it saved last: a stale read, or its disk's copy
        // restored from a backup.)
        let saved = self.saved.records.filter(|_| self.known);
        let retry = self.taking.is_some();
        let own = match &self.taking {
            Some(r) => Some(r.clone()),
            None => self.spare.clone().or_else(|| self.kept(io, out)),
        }
        .filter(|r| saved.is_none_or(|s| (r.epoch, r.seq) >= s));
        let one = match records::load_one(io) {
            Ok(o) => o,
            Err(e) => return out.events.push(Event::Waits { what: "take up the records", why: format!("{e:#}") }),
        };
        let (base, how) = match (own, one) {
            (Some(r), Some(o)) if (o.epoch, o.seq) > (r.epoch, r.seq) => {
                if !waited {
                    return wait(self, out, "the one path's records are newer than its own: another lead saved them");
                }
                (o, "the one path's records, newer than its own")
            }
            (Some(r), _) => (r, "its own records"),
            (None, Some(_)) if !waited => {
                return wait(self, out, "no records of its own: the one path's");
            }
            (None, Some(o)) => (o, "the one path's records"),
            // (A new pool: no records saved, nor by this Mac before.)
            (None, None) if saved.is_some() => {
                return out.events.push(Event::Waits {
                    what: "take up the records",
                    why: format!("{} can't be read, and this Mac saved records before: tried again", records::ONE),
                })
            }
            (None, None) => match io.exists(records::ONE_BRIEF) {
                Ok(false) => match Records::today(io) {
                    Ok(r) => (r, "today's files: the pool's first"),
                    Err(e) => {
                        return out
                            .events
                            .push(Event::Waits { what: "take up the records", why: format!("today's files: {e:#}") })
                    }
                },
                Ok(true) => {
                    return out.events.push(Event::Waits {
                        what: "take up the records",
                        why: format!("{} can't be read, its brief there: tried again", records::ONE),
                    })
                }
                Err(e) => return out.events.push(Event::Waits { what: "take up the records", why: format!("{e:#}") }),
            },
        };
        let mut r = base;
        // (Never from records older than those its lead last saved: a stale read's. What their
        // saves acknowledged is on no member's disk any more: docs/coordination.md step 6.)
        if let Some((e, seq)) = saved.filter(|s| (r.epoch, r.seq) < *s) {
            let why = records::Older(format!(
                "the newest records read are epoch {}'s at {}, older than epoch {e}'s at {seq}, which it saved \
                 (a stale read: tried again)",
                r.epoch, r.seq
            ));
            return out.events.push(Event::Waits { what: "take up the records", why: why.to_string() });
        }
        // Its epoch: past theirs, and past what it saved (an earlier try's is its own already).
        let epoch = if retry { Ok(r.epoch) } else { io.epoch(r.epoch.max(saved.map_or(0, |s| s.0))) };
        let epoch = match epoch {
            Ok(e) => e,
            Err(e) => {
                return out.events.push(Event::Waits { what: "take up the records", why: format!("its epoch: {e:#}") })
            }
        };
        if r.epoch != epoch {
            (r.epoch, r.seq) = (epoch, 0);
        }
        // (Nothing merged yet: its members post it their hand-offs once they know it leads, and a
        // listing of the journal follows while one writes it.)
        let dirty = match r.save(io, &|b| io.keep(b)) {
            Ok(()) => {
                self.saved.records = Some((r.epoch, r.seq));
                false
            }
            Err(records::Unsaved::Kept(e)) => {
                self.taking = Some(r);
                return out
                    .events
                    .push(Event::Failed { what: "keep the records", why: format!("taking up epoch {epoch}: {e:#}") });
            }
            Err(e) => {
                out.events
                    .push(Event::Failed { what: "save the records", why: format!("taking up epoch {epoch}: {e:#}") });
                true
            }
        };
        self.taking = None;
        self.spare = None;
        (self.declared_at, self.wait_until) = (None, None);
        out.events.push(Event::TookUp { epoch, how: how.to_string() });
        self.lead = Some(Lead {
            epoch,
            since: io.now(),
            records: r,
            told: BTreeMap::new(),
            whole_told: BTreeMap::new(),
            waiting: BTreeSet::new(),
            unreadable: BTreeMap::new(),
            said: BTreeSet::new(),
            turn: 0,
            listed_at: None,
            dirty,
            saved_at: io.awake(),
            refused: Vec::new(),
            held: BTreeMap::new(),
        });
        // A take-up lists the journal, every day, while a member writes it.
        self.due = self.journal.then_some((None, true));
        self.asked.clear();
    }

    /// The lead's step: merge what members posted it and told it, its own jobs' hand-offs and what
    /// a listing found (and what's waiting to be read whole), save, acknowledge, note the refusals.
    fn lead_step(&mut self, io: &dyn Io, out: &mut Out, given: Given, check: Check, busy: u64, vouched: bool) {
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
        let e = l.epoch;
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
        // step's others (its read of the declaration, or reads of other entries): a file the share
        // errors on. A read that failed when none answered (the share away) says nothing, and
        // starts the time again; one the step's time left unread keeps it, and isn't refused
        // unread.
        let answered = vouched
            || !(m.applied.is_empty() && m.overtaken.is_empty() && m.refused.is_empty() && m.waiting.is_empty());
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
        // (Saved again unchanged `RESAVE_S` on: an older save landed late over the one path stays
        // that long at most. That save failing leaves nothing unsaved: tried again next step.)
        if l.dirty || io.awake().saturating_sub(l.saved_at) >= RESAVE_S {
            match l.records.save(io, &|b| io.keep(b)) {
                Ok(()) => {
                    (l.dirty, l.saved_at) = (false, io.awake());
                    self.saved.records = Some((l.records.epoch, l.records.seq));
                }
                Err(err) => {
                    let what =
                        if matches!(err, records::Unsaved::Kept(_)) { "keep the records" } else { "save the records" };
                    out.events.push(Event::Failed { what, why: format!("epoch {e}: {err:#}") })
                }
            }
        }
        // Refusals its saved records name (this step's, or an earlier one's): noted for the owner. (A note not made now is only the owner's
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
            // (Only while the declaration, read after the save, still names it: the owner moved the
            // lead, the next lead may have read its records before this save, and what's
            // acknowledged is on no member's disk then. Its take-up reads them `STALE_S` after
            // finding itself declared, so a save this read follows is in what it reads (`take_up`).
            // Not acknowledged now: next step, while it still leads; else its members keep them for
            // the next lead.)
            if !acks.is_empty() || !named.is_empty() {
                match lead::declared(io) {
                    Ok(Some(d)) if d.member == me => {}
                    Ok(d) => {
                        let who =
                            d.map_or("no member".to_string(), |d| if d.host.is_empty() { d.member } else { d.host });
                        return out.events.push(Event::Waits {
                            what: "acknowledge hand-offs",
                            why: format!("the declaration names {who} after its save: its next lead has them"),
                        });
                    }
                    Err(err) => {
                        return out.events.push(Event::Failed {
                            what: "acknowledge hand-offs",
                            why: format!("whether it's still the declared lead after its save: {err:#}"),
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
                    out.send.push((to, Msg::Ack { epoch: e, keys, horizon: String::new() }));
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

    /// The pool's fields of its heartbeat: the epoch it leads in.
    fn beat(&self, io: &dyn Io) -> Beat {
        let leads = self.lead.as_ref().map(|l| l.epoch);
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
    use anyhow::Result;
    use buildstore::handoff::Handoff;
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
        /// The records its lead kept on its own disk (`Io::keep`); while `local_fails`, keeping
        /// them fails.
        local: std::cell::RefCell<Option<Vec<u8>>>,
        local_fails: Cell<bool>,
        /// Its store's epoch (`Io::epoch`: 0, none open yet; raised past a take-up's records).
        store: Cell<u64>,
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
                local: Default::default(),
                local_fails: Cell::new(false),
                store: Cell::new(0),
            }
        }

        /// Its agent restarted: its store's epoch one more, as the store opens.
        fn restart(&self) {
            self.store.set(self.store.get() + 1);
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
        fn kept(&self) -> Result<Option<Vec<u8>>> {
            Ok(self.local.borrow().clone())
        }
        fn keep(&self, records: &[u8]) -> Result<()> {
            if self.local_fails.get() {
                anyhow::bail!("its own disk: full");
            }
            *self.local.borrow_mut() = Some(records.to_vec());
            Ok(())
        }
        fn epoch(&self, past: u64) -> Result<u64> {
            if self.store.get() <= past {
                self.store.set(past + 1);
            }
            Ok(self.store.get())
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
        fn kept(&self) -> Result<Option<Vec<u8>>> {
            self.0.kept()
        }
        fn keep(&self, records: &[u8]) -> Result<()> {
            self.0.keep(records)
        }
        fn epoch(&self, past: u64) -> Result<u64> {
            self.0.epoch(past)
        }
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    fn member(id: &str, host: &str, app: &str) -> Member {
        Member { id: id.into(), host: host.into(), app: app.into() }
    }

    /// A step.
    fn step(d: &mut Driver, io: &Mac, heard: Heard) -> Out {
        d.step(io, heard, &any)
    }

    fn able() -> Heard {
        Heard::default()
    }

    fn setup(mem: &Mem) {
        mem.write_whole("state/build/manifest.json", b"{}").unwrap();
    }

    /// The one path's records, as read now.
    fn one(mem: &Mem) -> Records {
        records::load_one(mem).unwrap().unwrap()
    }

    fn entry(member: &str, epoch: u64, n: u64, t: &str) -> Entry {
        let h = Handoff {
            changes: [(format!("base/{t}"), Some(format!("base/{t}.k{n}.base")))].into(),
            done: Some(("unit".into(), vec![(t.into(), format!("k{n}"))])),
            ..Default::default()
        };
        Entry { member: member.into(), lease: LeaseId { epoch, n }, step: "unit".into(), handoff: h, at: T0 + n }
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
        let says = |app: &str| Heard { lead_says: Some(member(A, "Mac-mini", app)), ..able() };
        step(&mut b, &ib, says(V2));
        let cancel = Act::Cancel(jobrt::control::Cancel::new("6/1/2", Some("unit"), false, "the build page (m1)"));
        let retry = Act::Retry(jobrt::control::Retry {
            target: "6/1/3".into(),
            step: Some("unit".into()),
            ..Default::default()
        });
        ib.pass(5);
        let o = step(&mut b, &ib, Heard { asks: vec![Ask::Act(cancel.clone()), Ask::Act(retry.clone())], ..says(V2) });
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
        // A member on another app than the lead's, or with no word from the lead lately (its app
        // not known from the declaration): refused, said why.
        let mut c = Driver::unlocked(member(C, "MacBook-Pro", "development"), Saved::default());
        let ic = Mac::new(&mem);
        for heard in [says(V2), able()] {
            ic.pass(5);
            let o = step(&mut c, &ic, Heard { asks: vec![Ask::Act(cancel.clone())], ..heard });
            assert_eq!(c.declared(), Some(A));
            assert!(!o.send.iter().any(|(_, m)| matches!(m, Msg::Cancel(_))), "{:?}", o.send);
            assert!(
                o.events.iter().any(|e| matches!(e, Event::Failed { what, .. } if what.contains("page's ask"))),
                "{:?}",
                o.events
            );
        }
    }

    /// Moves the declaration of the lead to member `to` (the owner's edit, by hand).
    fn declare(mem: &Mem, to: &str) {
        mem.write_whole(lead::LEAD, format!("{{\"member\": \"{to}\"}}").as_bytes()).unwrap();
    }

    #[test]
    fn a_member_never_leads() {
        // (Task #881) A, the first Mac in the pool, declares itself the lead. B never leads: not
        // with A leading for hours, not with A gone for a day.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        assert_eq!(lead::declared(&mem).unwrap().map(|d| d.member), Some(A.to_string()));
        assert_eq!((step(&mut b, &ib, able()).leads, b.declared()), (None, Some(A)));
        for _ in 0..240 {
            ia.pass(30);
            ib.pass(30);
            assert_eq!(step(&mut b, &ib, able()).leads, None);
            assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        }
        for _ in 0..(86_400 / 600) {
            ib.pass(600);
            let o = step(&mut b, &ib, able());
            assert_eq!((o.leads, b.declared()), (None, Some(A)), "{:?}", o.events);
        }
        // A pool past its start with no declaration (the records saved): no Mac declares itself.
        mem.remove(lead::LEAD).unwrap();
        let mut c = Driver::unlocked(member(C, "MacBook-Pro", V2), Saved::default());
        let ic = Mac::new(&mem);
        for _ in 0..3 {
            ic.pass(20);
            assert_eq!((step(&mut c, &ic, able()).leads, c.declared()), (None, None));
        }
        assert_eq!(lead::declared(&mem).unwrap(), None);
    }

    #[test]
    fn with_the_lead_gone_a_member_writes_its_jobs_hand_offs_and_waits() {
        // (Task #881) A silent; B's jobs end: their hand-offs go to the journal (A takes none by
        // HTTP: the gate), B tells A of them (its mail waits for A), and B leads nothing; A back,
        // it merges them.
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
        // A back (its sleep no matter: no other lead could have saved), told: merged, same epoch.
        ia.wall.set(ib.now());
        let o = caught_up(&mut a, &ia, Heard { msgs: vec![(B.into(), Msg::Tell(vec![key.clone()]))], ..able() });
        assert_eq!(o.leads, Some(1), "{:?}", o.events);
        assert!(a.records().unwrap().handles(&key), "{:?}", o.events);
    }

    /// docs/coordination.md step 8c: the owner moves the lead (another Mac in the mini's place): A
    /// steps down at its next step, its read of the declaration naming B; B, finding itself
    /// declared, takes up A's records from the one path once A's last save can't read stale
    /// (`STALE_S`), its epoch past theirs; and A, declared again later (moved back while it was
    /// stopped), takes up B's, newer than its own disk's copy.
    #[test]
    fn the_owners_edit_of_the_declaration_moves_the_lead() {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let http = || Heard { http_only: true, ..able() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        ia.store.set(40);
        assert_eq!(step(&mut a, &ia, http()).leads, Some(40));
        ia.pass(20);
        step(&mut a, &ia, Heard { handed: vec![entry(B, 40, 1, "6-1-1")], ..http() });
        step(&mut b, &ib, http());
        declare(&mem, B);
        ia.pass(20);
        let o = step(&mut a, &ia, http());
        assert!(
            o.leads.is_none()
                && o.events
                    .iter()
                    .any(|e| matches!(e, Event::SteppedDown { epoch: 40, why } if why.contains("declared"))),
            "{:?}",
            o.events
        );
        ib.pass(20);
        let o = step(&mut b, &ib, http());
        assert_eq!(o.leads, None, "waits `STALE_S` after finding itself declared: {:?}", o.events);
        ib.pass(STALE_S - 20);
        assert_eq!(step(&mut b, &ib, http()).leads, None);
        ib.pass(20);
        let o = step(&mut b, &ib, http());
        assert_eq!(o.leads, Some(41), "past A's epoch: {:?}", o.events);
        assert_eq!(b.records().unwrap().keys.recorded("unit", "6-1-1"), Some("k1"), "from A's records");
        ib.pass(20);
        step(&mut b, &ib, Heard { handed: vec![entry(A, 41, 2, "6-1-2")], ..http() });
        assert_eq!((one(&mem).epoch, one(&mem).keys.recorded("unit", "6-1-2")), (41, Some("k2")));
        // Moved back while A's agent was stopped: its own disk's copy is older than the one path's.
        declare(&mem, A);
        ia.restart();
        ia.wall.set(ib.now());
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        ia.pass(20);
        let o = step(&mut a, &ia, http());
        assert!(
            o.leads.is_none()
                && o.events.iter().any(|e| matches!(e, Event::Waits { why, .. } if why.contains("newer than its own"))),
            "{:?}",
            o.events
        );
        ia.pass(STALE_S);
        let o = step(&mut a, &ia, http());
        assert_eq!(o.leads, Some(42), "{:?}", o.events);
        assert_eq!(a.records().unwrap().keys.recorded("unit", "6-1-2"), Some("k2"), "B's records, not its own");
    }

    /// docs/coordination.md step 8a, measured: a member's step learns its lead from its lead's beat
    /// answer (`Heard::lead_says`) with no NAS call, where it reads the declaration each step; with
    /// no word from its lead (lately, or ever: an older lead), it reads the declaration, so it
    /// learns of the owner's move of the lead to it. No member ever reads a term (step 8c).
    #[test]
    fn a_member_learns_its_lead_from_its_answer_with_no_nas_call() {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        let says = || Heard { lead_says: Some(member(A, "Mac-mini", V2)), ..able() };
        // Without its lead's word: the declaration, one read a step.
        let nb = Logged(&ib, Default::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        b.step(&nb, able(), &any);
        assert_eq!(b.declared(), Some(A));
        ib.pass(20);
        b.step(&nb, able(), &any);
        assert_eq!(nb.take(), [format!("read {}", lead::LEAD), format!("read {}", lead::LEAD)]);
        // From its lead's word: no NAS call.
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        for _ in 0..30 {
            ib.pass(20);
            let o = b.step(&nb, says(), &any);
            assert_eq!((o.leads, b.declared(), b.lead().map(|l| l.app.as_str())), (None, Some(A), Some(V2)));
        }
        assert!(nb.take().is_empty(), "no NAS call in 30 steps");
        // The owner moves the lead to B. While A's answers say it leads, B leads nothing; once B
        // hears no word (A stepped down, or away), it reads the declaration, and takes the lead up
        // `STALE_S` on.
        declare(&mem, B);
        ib.pass(20);
        assert_eq!(b.step(&nb, says(), &any).leads, None);
        assert_eq!(b.declared(), Some(A));
        ib.pass(20);
        b.step(&nb, able(), &any);
        assert_eq!(b.declared(), Some(B));
        ib.pass(STALE_S);
        let o = b.step(&nb, able(), &any);
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        let calls = nb.take();
        assert!(
            calls.iter().all(|c| !c.contains("terms") && !c.contains("/term/") && !c.contains("lead.json")),
            "{calls:#?}"
        );
    }

    /// docs/coordination.md step 8c, a lead restart: its agent's store opens in a new epoch; the
    /// lead takes up from the records it kept on its own disk, reading the one path once (its
    /// records not newer: an earlier process's save that landed late over it is passed over) and
    /// no term; its take-up's save puts the newest back, in the new epoch. Its disk's copy lost, it
    /// takes up the one path's once `STALE_S` has passed, nothing it saved lost.
    #[test]
    fn a_restarted_lead_takes_up_from_its_own_disk_in_a_new_epoch() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let http = || Heard { http_only: true, ..able() };
        // (The move from terms: the lead declared, its records numbered by the pool's last term, its
        // store's epoch below it.)
        declare(&mem, A);
        let last = Records { epoch: 840, seq: 7, ..Records::today(&mem).unwrap() };
        *ia.local.borrow_mut() = Some(serde_json::to_vec(&last).unwrap());
        mem.write_whole(records::ONE, &serde_json::to_vec(&last).unwrap()).unwrap();
        let saved = Saved { member: A.into(), records: Some((840, 7)), ..Default::default() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), saved);
        ia.store.set(121);
        let o = step(&mut a, &ia, http());
        assert_eq!(o.leads, Some(841), "past the pool's last term: {:?}", o.events);
        let e = entry(A, 841, 1, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e], ..http() });
        assert_eq!(o.merged, std::slice::from_ref(&key), "{:?}", o.events);
        // An earlier save lands late over the one path.
        let late = serde_json::to_vec(&Records { epoch: 841, seq: 1, ..Default::default() }).unwrap();
        mem.write_whole(records::ONE, &late).unwrap();
        // Restarted: from its own disk's copy, the one path read once.
        ia.restart();
        let io = Logged(&ia, Default::default());
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        ia.pass(20);
        let o = a.step(&io, http(), &any);
        assert_eq!(o.leads, Some(842), "{:?}", o.events);
        assert!(a.records().unwrap().handles(&key), "its own disk's records");
        assert!(o.events.iter().any(|e| matches!(e, Event::TookUp { epoch: 842, how } if how == "its own records")));
        let calls = io.take();
        eprintln!("a restart's step, its records on its disk: {calls:#?}");
        let read = |c: &&String| c.starts_with("read ") || c.starts_with("stat ") || c.starts_with("list ");
        assert_eq!(calls.iter().filter(read).filter(|c| c.contains("records.json")).count(), 1, "{calls:#?}");
        assert!(!calls.iter().any(|c| c.contains("terms") || c.contains("/term/")), "{calls:#?}");
        let now: Records = buildstore::records::now(&mem).unwrap();
        assert_eq!((now.epoch, now.handles(&key)), (842, true), "the take-up's save over the late one");
        // Its disk's copy lost: the one path's, once `STALE_S` has passed.
        let ib = Mac::new(&mem);
        ib.wall.set(ia.now());
        ib.awake.set(ia.awake());
        ib.store.set(ia.store.get() + 1);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        ib.pass(20);
        assert_eq!(step(&mut a, &ib, http()).leads, None);
        ib.pass(STALE_S);
        let o = step(&mut a, &ib, http());
        assert_eq!(o.leads, Some(843), "{:?}", o.events);
        assert!(a.records().unwrap().handles(&key));
    }

    #[test]
    fn a_take_up_never_starts_from_records_older_than_it_saved() {
        // (docs/coordination.md step 6) What a save acknowledged is on no member's disk now: a
        // restart whose disk's copy is lost and whose read of the one path is stale waits for them
        // rather than lose it.
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
        assert_eq!(saved.records, Some((1, one(&mem).seq)));
        // (The one path read stale: an older save's; or not there at all.)
        let newest = mem.read(records::ONE).unwrap().unwrap();
        mem.write_whole(
            records::ONE,
            &serde_json::to_vec(&Records { epoch: 1, seq: 1, ..Default::default() }).unwrap(),
        )
        .unwrap();
        let ib = Mac::new(&mem);
        ib.wall.set(ia.now() + 20);
        ib.awake.set(ia.awake() + 20);
        ib.store.set(2);
        let mut b = Driver::unlocked(member(A, "Mac-mini", V1), saved);
        step(&mut b, &ib, able());
        ib.pass(STALE_S);
        let o = step(&mut b, &ib, able());
        assert_eq!(o.leads, None, "{:?}", o.events);
        assert!(o
            .events
            .iter()
            .any(|e| matches!(e, Event::Waits { what: "take up the records", why } if why.contains("older"))));
        mem.remove(records::ONE).unwrap();
        ib.pass(20);
        let o = step(&mut b, &ib, able());
        assert!(
            o.leads.is_none()
                && o.events
                    .iter()
                    .any(|e| matches!(e, Event::Waits { why, .. } if why.contains("saved records before"))),
            "never today's files: {:?}",
            o.events
        );
        mem.write_whole(records::ONE, &newest).unwrap();
        ib.pass(20);
        let o = step(&mut b, &ib, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(b.records().unwrap().handles(&key));
    }

    #[test]
    fn a_lead_moved_mid_step_acknowledges_nothing() {
        // (Task #878 step 6 review) The owner moves the lead to B while A's step merges a hand-off:
        // the declaration names B before A's save lands. A, reading the declaration after its save,
        // acknowledges nothing (its member keeps it, for B); B takes up only `STALE_S` after finding
        // itself declared, from A's last save.
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
                records::ONE.to_string(),
                Box::new(|| {
                    declare(&mem, B);
                    let o = step(&mut b.borrow_mut(), &ib, http());
                    assert_eq!(o.leads, None, "{:?}", o.events);
                }),
            ))),
        };
        let o = a.step(&race, Heard { handed: vec![e], ..http() }, &any);
        assert!(one(&mem).handles(&key));
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

    /// docs/coordination.md step 8c: B is declared; A's read of the declaration is stale, so it
    /// merges, saves and acknowledges a hand-off (its member drops it). B, taking up `STALE_S` after
    /// finding itself declared, reads that save: nothing lost.
    #[test]
    fn a_moved_lead_s_last_acknowledgement_is_in_the_next_lead_s_records() {
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let http = || Heard { http_only: true, ..able() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, http()).leads, Some(1));
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        step(&mut b, &ib, http());
        let was = mem.read(lead::LEAD).unwrap();
        declare(&mem, B);
        ib.pass(20);
        assert_eq!(step(&mut b, &ib, http()).leads, None, "waits");
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.wall.set(ib.now());
        let stale = Racing { mac: &ia, stale: [(lead::LEAD.to_string(), was)].into(), before: Default::default() };
        let o = a.step(&stale, Heard { handed: vec![e], ..http() }, &any);
        assert_eq!(o.merged, std::slice::from_ref(&key), "acknowledged, stale: {:?}", o.events);
        ib.pass(STALE_S);
        let o = step(&mut b, &ib, http());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(b.records().unwrap().handles(&key), "in B's records");
    }

    /// docs/coordination.md step 8b, as on 9 Nov at 20:40Z: a lead whose take-up's NAS write fails
    /// (past its deadline, or refused while an earlier one still runs) leads on from the records
    /// kept on its disk, acknowledging nothing and not caught up (no catalogue, no GC) until a save
    /// reaches the NAS. Not kept on its disk, it doesn't lead yet, and takes the records up at a
    /// later step, numbered on.
    #[test]
    fn a_lead_whose_take_up_s_nas_save_fails_leads_on() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let http = || Heard { http_only: true, ..able() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, http()).leads, Some(1));
        *ia.wfail.borrow_mut() = Some("records.json".into());
        ia.pass(20);
        ia.restart();
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        let o = step(&mut a, &ia, http());
        assert_eq!((o.leads, o.vouched, o.caught_up), (Some(2), true, false), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Failed { what: "save the records", .. })));
        let e = entry(A, 2, 1, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e.clone()], ..http() });
        assert!(o.merged.is_empty() && !o.caught_up && o.duties, "{:?}", o.events);
        *ia.wfail.borrow_mut() = None;
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e], ..http() });
        assert_eq!((o.leads, o.caught_up), (Some(2), true), "{:?}", o.events);
        assert_eq!(o.merged, [key.clone()]);
        let now: Records = buildstore::records::now(&mem).unwrap();
        assert_eq!((now.epoch, now.handles(&key)), (2, true));
        // Its own disk refusing: no lead until it keeps them.
        ia.local_fails.set(true);
        ia.pass(20);
        ia.restart();
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        let o = step(&mut a, &ia, http());
        assert_eq!(o.leads, None, "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Failed { what: "keep the records", .. })));
        ia.local_fails.set(false);
        ia.pass(20);
        let o = step(&mut a, &ia, http());
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
        assert_eq!(a.records().unwrap().seq, 2, "numbered on from the try kept");
        assert!(a.records().unwrap().handles(&key));
    }

    /// The first take-up waits for its agent's store (none before its coordinator starts), and
    /// in a new pool takes today's files up at once, declaring itself; a lead whose saved state is
    /// lost takes the one path's up once `STALE_S` has passed.
    #[test]
    fn a_take_up_waits_for_its_store_and_a_lost_state_for_the_one_path() {
        let mem = Mem::default();
        mem.write_whole("state/build/manifest.json", br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        let ia = Mac::new(&mem);
        struct NoStore<'a>(&'a Mac<'a>);
        impl Nas for NoStore<'_> {
            fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
                self.0.create_new(path, bytes)
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
        impl Io for NoStore<'_> {
            fn now(&self) -> u64 {
                self.0.now()
            }
            fn awake(&self) -> u64 {
                self.0.awake()
            }
            fn kept(&self) -> Result<Option<Vec<u8>>> {
                self.0.kept()
            }
            fn keep(&self, records: &[u8]) -> Result<()> {
                self.0.keep(records)
            }
            fn epoch(&self, _: u64) -> Result<u64> {
                anyhow::bail!("no coordinator's store in this process")
            }
        }
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let o = a.step(&NoStore(&ia), able(), &any);
        assert!(o.leads.is_none() && a.declared() == Some(A), "{:?}", o.events);
        assert!(!mem.exists(records::ONE).unwrap(), "nothing read or saved");
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(1), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::TookUp { how, .. } if how.contains("today's files"))));
        assert_eq!(one(&mem).manifest.len(), 1);
        // Its saved state lost (a new member file: another id) on the same Mac.
        ia.pass(20);
        ia.restart();
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, None, "waits");
        ia.pass(STALE_S);
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
    }

    #[test]
    fn a_new_lead_is_caught_up_once_its_listing_is_merged() {
        // After a restart the lead's records lack an entry only a listing finds (its member gone,
        // never told of it). (Review N2: its duties and its own word were all the agent had, and
        // both were true before the listing came back.)
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
        assert!(o.duties && o.vouched && !o.caught_up, "vouched for, not whole");
        let list = o.list.expect("a listing asked for");
        assert_eq!(list.since, None, "every day");
        assert!(!one(&mem).handles(&x));
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
        assert!(one(&mem).handles(&x));
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
        assert!(one(&mem).handles(&key), "in the snapshot saved");
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
        fn kept(&self) -> Result<Option<Vec<u8>>> {
            self.mac.kept()
        }
        fn keep(&self, records: &[u8]) -> Result<()> {
            self.mac.keep(records)
        }
        fn epoch(&self, past: u64) -> Result<u64> {
            self.mac.epoch(past)
        }
    }

    /// A Mac's NAS calls noted in order ("read <path>", "stat …", "write …", "create …", "list …").
    struct Logged<'a>(&'a Mac<'a>, std::cell::RefCell<Vec<String>>);

    impl Logged<'_> {
        fn note(&self, op: &str, path: &str) {
            self.1.borrow_mut().push(format!("{op} {path}"));
        }
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.1.borrow_mut())
        }
    }

    impl Nas for Logged<'_> {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            self.note("create", path);
            self.0.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.note("write", path);
            self.0.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.note("read", path);
            self.0.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.note("stat", path);
            self.0.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.note("list", dir);
            self.0.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.note("remove", path);
            self.0.remove(path)
        }
    }

    impl Io for Logged<'_> {
        fn now(&self) -> u64 {
            self.0.now()
        }
        fn awake(&self) -> u64 {
            self.0.awake()
        }
        fn kept(&self) -> Result<Option<Vec<u8>>> {
            self.0.kept()
        }
        fn keep(&self, records: &[u8]) -> Result<()> {
            self.0.keep(records)
        }
        fn epoch(&self, past: u64) -> Result<u64> {
            self.0.epoch(past)
        }
    }

    /// docs/coordination.md steps 8b, 8c: a save of an earlier process's, landing late over the one
    /// path, is read until the lead saves again, `RESAVE_S` at most with nothing changed.
    #[test]
    fn a_late_save_over_the_one_path_is_saved_over() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let http = || Heard { http_only: true, ..able() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, http()).leads, Some(1));
        let e = entry(A, 1, 1, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { handed: vec![e], ..http() });
        assert!(one(&mem).handles(&key));
        mem.write_whole(
            records::ONE,
            &serde_json::to_vec(&Records { epoch: 1, seq: 1, ..Default::default() }).unwrap(),
        )
        .unwrap();
        let read = || buildstore::records::now::<Records>(&mem).unwrap().handles(&key);
        let mut steps = 0;
        while !read() {
            ia.pass(20);
            step(&mut a, &ia, http());
            steps += 1;
            assert!(steps * 20 <= RESAVE_S + 20, "still the late save after {steps} steps");
        }
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
        let pack = |n: &str| buildstore::rawpack::Pack { name: format!("6-1-1.{n}.tiles"), bytes: 9 };
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
        let saved = one(&mem);
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
        assert!(one(&mem).handles(&key) && o.caught_up, "{:?}", o.events);
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
        let r = one(&mem);
        assert!(!r.handles(&key) && journal::refusal(&mem, &key).unwrap().is_none(), "neither merged nor refused");
        // Merged again the next step, held still, not said again; nothing acknowledged.
        ia.pass(20);
        let o = a.step(&ia, able(), &check);
        assert!(o.held.len() == 1 && said(&o) == 0 && o.send.is_empty(), "{:?}", o.events);
        // Taken: merged, acknowledged to its member, no longer held.
        hold.set(false);
        ia.pass(20);
        let o = a.step(&ia, able(), &check);
        assert!(o.held.is_empty() && one(&mem).reflected.contains(&key), "{:?}", o.events);
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
        let why = one(&mem).rejected.get(&key).cloned();
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
        assert!(one(&mem).handles(&next), "the later one merged");
        ia.pass(300);
        let at_5 = step(&mut a, &ia, tell(vec![key.clone()]));
        ia.pass(300);
        let at_10 = step(&mut a, &ia, tell(vec![key.clone()]));
        let refused = one(&mem).rejected.contains_key(&key);
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
        let mut saved = Saved { member: A.into(), records: Some((3, 4)), ..Default::default() };
        saved.mine.add(e).unwrap();
        let d = "m-000000000000000d";
        let mut dr = Driver::unlocked(member(d, "Mac-mini", V1), saved);
        let s = dr.saved();
        assert_eq!((s.member.as_str(), s.records, s.mine.unwritten().count()), (d, None, 1));
        assert_eq!(step(&mut dr, &ia, able()).leads, Some(1));
        assert!(matches!(journal::read(&mem, &key).unwrap(), journal::Read::Missing), "not written");
        assert!(one(&mem).handles(&key), "merged");
    }

    #[test]
    fn a_lock_check_that_fails_holds_the_steps_duties_and_checks_again() {
        // The lock's folder unreadable a moment (a stat that fails), no other process holding the
        // lock: this step does no duties, and the next, the folder readable again, does. (Re-review
        // 2, M1: it stopped for good, its lead left to the owner.)
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
        assert!(o.events.iter().any(|e| matches!(e, Event::SteppedDown { epoch: 1, .. })), "{:?}", o.events);
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
        let merged = |m: &Mem| keys.iter().filter(|k| one(m).handles(k)).count();
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
        while !one(&mem).handles(&keys[39]) {
            steps += 1;
            assert!(steps <= 8, "not merged in eight steps");
            io.0.pass(20);
            a.step(&io, able(), &any);
        }
        let r = one(&mem);
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
        let why = one(&mem).rejected.get(&key).cloned();
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
        let r = one(&mem);
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
            assert!(!one(&mem).rejected.contains_key(&key), "refused while every read of it failed");
        }
        ia.down.set(false);
        ia.pass(60);
        assert!(step(&mut a, &ia, able()).caught_up);
        assert!(one(&mem).reflected.contains(&key), "applied once the share answers");
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
    fn a_note_of_a_stale_leads_refusal_holds_no_lead() {
        // (Review 2's last scenario: the current lead's own check decides.)
        let mem = Mem::default();
        setup(&mem);
        let x = journal::write(&mem, &entry(C, 1, 7, "6-1-1")).unwrap();
        journal::note_refusal(&mem, &x, "a stale lead's why").unwrap();
        let mut r = Records { epoch: 2, ..Default::default() };
        assert_eq!(records::merge(&mem, &mut r, std::slice::from_ref(&x), &any).applied, std::slice::from_ref(&x));
        assert!(journal::list(&mem, None).unwrap().contains(&x));
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
