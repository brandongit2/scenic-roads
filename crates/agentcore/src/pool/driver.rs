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
//! busy (a long plan, a job stopped) its pool's own thread's (crate::agent::poolside's `Ticker`), each
//! under the run's lock. A step:
//!
//! - **does its NAS operations through `Io`** (crate::pool::nas::Nas, two clocks, its own disk's
//!   copy of the records and its store's epoch): the lead's declaration read, the records' save
//!   (about 14 MB); never a listing, never a sleep, no thread of its own. Hand-offs come and go by HTTP (crate::coord::handoffs): the lead merges them from
//!   memory;
//! - **reads the clocks itself**, where a decision needs the time: the agent passes no time in;
//! - **takes what was heard since the last step** (`Heard`): the members' messages to it, the
//!   build pages' asks, the hand-offs of its jobs that ended, those its members posted it and those
//!   its lead's saved records name, its lead's word;
//! - **gives what to do now** (`Out`): the messages to send, by member id, over the pool's API
//!   (best effort: a message lost is made up for); the pool's fields of this Mac's heartbeat, to
//!   write with the rest of it, stamped as it's written; the epoch it leads, if it leads, and what
//!   it may do as that now: grant jobs and plan (`duties`); publish a catalogue only while its
//!   records reflect what's handed off (`caught_up`); the hand-offs to acknowledge; and what
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
//! a new id made; or a copy of another Mac's folder), counts for nothing but its jobs' hand-offs.
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
//! The lead takes up the records once a process (`take_up`), from its own, kept on its disk (step
//! 8b: `Io::kept`); in a new pool, from today's files. It never takes up the one path's: the lead
//! is moved to another Mac by hand, its agent stopped and its folders copied (docs/pool.md §6.1),
//! and a lead whose disk lost its records doesn't lead. Its records are numbered on in its
//! store's epoch, which is past theirs (a store whose epoch isn't, not the one that saved them,
//! leads nothing). It leads while the declaration names it, read each step: an old lead started
//! again after a hand move leads nothing.
//!
//! A member learns its lead from its lead's beat answer while one came lately (`Heard::lead_says`,
//! docs/coordination.md step 8a), with no NAS call; without one it reads the declaration, as the
//! lead does every step.

use super::beat::Beat;
use super::journal::{Entry, Mine};
use super::nas::Nas;
use super::records::{self, Check, Records};
use super::{Member, MemberLock};
use buildstore::records::lead;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A lead saves its records again, changed or not, once this long awake has passed since its last
/// save reached the NAS (s), so that an older save landing late over the one path (records::ONE)
/// stays there this long at most (docs/coordination.md step 8b). Within one process a write past
/// its deadline holds back the next to its path until it ends (crate::agent::nasread's
/// `InFlight`); one an earlier process left running as it stopped may land after any: readers read
/// it meanwhile.
pub const RESAVE_S: u64 = 600;

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
    /// opens): what a take-up leads, its records and its leases numbered by it. An error when
    /// there's no store open (not the lead's process yet).
    fn epoch(&self) -> anyhow::Result<u64>;
}

/// A message between members, over the pool's API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Msg {
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
    /// The hand-offs of its jobs that ended: kept whole (in `Saved`) until a lead's saved records
    /// name them; the agent may drop one once a `Saved` from this step is on its disk.
    pub entries: Vec<Entry>,
    /// Raw tiles' archives its records hold (`Records::raw`) that the agent has named in the raw
    /// store's index since (buildstore::rawpack::name_handed, the index saved): taken off the records,
    /// saved with this step's snapshot. (One named and not taken off yet, a restart between, is
    /// named again: passed over unread.)
    pub named_raw: Vec<String>,
    /// Leading: the hand-offs members posted it by HTTP (crate::coord::handoffs) and it hasn't
    /// acknowledged: merged from memory, acknowledged once a snapshot naming them is saved
    /// (`Out::merged`).
    pub handed: Vec<Entry>,
    /// A member: its hand-offs that its lead's saved records name (its beats' answers): dropped.
    pub merged: Vec<String>,
    /// A member: the lead its lead's beat answered it is (crate::coord::held::Answer::leading),
    /// that answer fresh (within crate::coord::beats::PAGE_AWAY_S: crate::coord::beats::Beats::
    /// lead_says). Its lead is learnt from it, and the NAS's declaration isn't read; None (a lead
    /// not leading, none answering lately) reads it (docs/coordination.md step 8a).
    pub lead_says: Option<Member>,
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
    /// Leading, whether it may grant jobs and plan now (its member's lock known held).
    pub duties: bool,
    /// Leading, its records reflect what's handed off: every hand-off posted it, and its own jobs',
    /// is merged and saved. A catalogue waits for it (and `duties`); GC too.
    pub caught_up: bool,
    /// Leading: the hand-offs its check holds (`records::HOLD`: from a member on a newer app), with
    /// why: neither merged nor refused, merged again every step until the check takes or refuses
    /// them. They don't keep it from being `caught_up`.
    pub held: Vec<(String, String)>,
    /// This process isn't the member's only one (another process holds its lock:
    /// `MemberLock::check`), and why. The agent saves this step's `Saved` (the hand-offs handed to
    /// the step are kept in it, for this folder's next process to post), writes no heartbeat (this
    /// step's is empty), sends nothing, steps no more, and leaves the pool.
    pub stop: Option<String>,
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
    /// Hand-offs merged: applied, passed over (an older lease's), refused (`done`: the applied ones'
    /// jobs done, crate::pool::records::Merged::done; `again`: the work of those refused or passed
    /// over that the records don't hold, to do again, crate::pool::records::Merged::again).
    Merged {
        applied: Vec<String>,
        overtaken: usize,
        refused: usize,
        done: Vec<crate::pool::records::MergedJob>,
        again: Vec<crate::pool::records::Again>,
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
    /// another Mac's folder, its id made again there) counts for nothing but its jobs' hand-offs.
    #[serde(default)]
    pub member: String,
    /// Its jobs' hand-offs, kept until a lead's saved records name them.
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
    /// Changes not saved to the NAS yet (on its own disk or not), and when its last save reached
    /// the NAS (awake clock: `RESAVE_S`).
    dirty: bool,
    saved_at: u64,
    /// The hand-offs its check holds (`records::HOLD`), with why: merged again every step.
    held: BTreeMap<String, String>,
}

/// A member's part in the pool, one step at a time (see the module's doc: the contract).
#[derive(Debug)]
pub struct Driver {
    me: Member,
    /// Its member's lock (none in the simulator), and why it stopped: another process took it.
    lock: Option<MemberLock>,
    stopped: Option<String>,
    saved: Saved,
    lead: Option<Lead>,
    /// A take-up tried and not kept on its disk: its records as tried (numbered on, in its epoch).
    taking: Option<Records>,
    /// The lead's declaration as last read (lead::declared), or as its lead's answer said it: the
    /// member that leads; and that member as last known (its app known only from its answer, or
    /// when it's this Mac).
    declared: Option<String>,
    lead_member: Option<Member>,
}

impl Driver {
    /// The driver of member `me` (its app the process's), holding its lock, from what it saved
    /// last. Another member's state (a copy of the agent's folder, or this Mac's member file lost
    /// and a new id made), or one naming none (lost, or never saved), counts for nothing but its
    /// jobs' hand-offs: those are posted, the same bytes whoever posts them (an older app's keys of
    /// the NAS's journal read back first, `Mine::read_back`).
    pub fn new(me: Member, saved: Saved, lock: MemberLock) -> Driver {
        Driver { lock: Some(lock), ..Driver::without(me, saved) }
    }

    /// `new` without the lock: the simulator's members and the tests', in one process.
    #[cfg(test)]
    pub fn unlocked(me: Member, saved: Saved) -> Driver {
        Driver::without(me, saved)
    }

    fn without(me: Member, saved: Saved) -> Driver {
        let saved = if saved.member == me.id {
            saved
        } else {
            Saved { member: me.id.clone(), mine: saved.mine, ..Default::default() }
        };
        Driver { me, lock: None, stopped: None, saved, lead: None, taking: None, declared: None, lead_member: None }
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

    /// Its jobs' hand-offs, as `Saved` keeps them.
    pub fn mine(&self) -> &Mine {
        &self.saved.mine
    }

    /// The (epoch, seq) of the records it last saved whole (its disk and the NAS's one path).
    pub fn saved_records(&self) -> Option<(u64, u64)> {
        self.saved.records
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
            // posts them.)
            for e in heard.entries {
                if let Err(err) = self.saved.mine.add(e) {
                    out.events.push(Event::Failed { what: "keep an entry", why: format!("{err:#}") });
                }
            }
            out.stop = Some(why.clone());
            return out;
        }
        // Its jobs' hand-offs, kept whole; those an app before task #1125 wrote to the NAS's journal
        // read back whole.
        for e in heard.entries {
            if let Err(err) = self.saved.mine.add(e) {
                out.events.push(Event::Failed { what: "keep an entry", why: format!("{err:#}") });
            }
        }
        self.saved.mine.acked_all(&heard.merged);
        let (dropped, failed) = self.saved.mine.read_back(io);
        for (k, why) in dropped {
            out.events.push(Event::Failed {
                what: "read back an older app's journal entry",
                why: format!("{k}: {why}: dropped, its job's work done again"),
            });
        }
        for (k, err) in failed {
            out.events
                .push(Event::Waits { what: "read back an older app's journal entry", why: format!("{k}: {err:#}") });
        }
        // What it heard.
        let mut acts: Vec<Act> = Vec::new();
        for (_, m) in heard.msgs {
            match m {
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
        // A lead leads while it's declared (an old lead started again after the owner moved the lead
        // by hand leads nothing); the declared lead, not leading, takes the records up.
        if self.lead.is_some() && !mine {
            let why =
                if self.declared.is_some() { "another member is the declared lead" } else { "no lead is declared" };
            if let Some(l) = self.lead.take() {
                out.events.push(Event::SteppedDown { epoch: l.epoch, why: why.to_string() });
            }
        }
        if mine && read && self.lead.is_none() {
            self.take_up(io, &mut out);
        }
        if self.lead.is_some() {
            self.lead_step(io, &mut out, heard.handed, &heard.named_raw, check);
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
        out.beat = self.beat(io);
        if let Some(l) = &self.lead {
            out.leads = Some(l.epoch);
            out.duties = true;
            out.caught_up = !l.dirty;
            out.held = l.held.iter().map(|(k, w)| (k.clone(), w.clone())).collect();
        }
        // (Its lock not known held this step: no duties, catalogue or sweep until it's checked
        // again.)
        if let Some(why) = unsure {
            out.events.push(Event::Failed { what: "check the member's lock", why });
            (out.duties, out.caught_up) = (false, false);
        }
        out
    }

    /// Whether this Mac is the declared lead (lead::declared), read each step (the last read kept
    /// when one fails), with whether it was read. None declared: in a new pool (no
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
                Ok(false) => match lead::declare(io, &self.me) {
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

    /// The records kept on this Mac's disk leading (`Io::kept`: its own, or copied in with its
    /// folder as the owner moved the lead here by hand); one that can't be read is said, and counts
    /// as none.
    fn kept(&self, io: &dyn Io, out: &mut Out) -> Option<Records> {
        match io.kept().and_then(|b| b.map(|b| serde_json::from_slice::<Records>(&b)).transpose().map_err(Into::into)) {
            Ok(r) => r,
            Err(e) => {
                out.events.push(Event::Failed { what: "read its own records", why: format!("{e:#}") });
                None
            }
        }
    }

    /// Takes up the records (§6.2), this Mac the declared lead and not leading: from its own (an
    /// earlier try's, or kept on its disk: docs/coordination.md step 8b), none older than it saved
    /// last (`Saved::records`); with none, in a new pool (no records at the one path nor its brief,
    /// none saved by this Mac before), today's files. It leads its store's epoch (`Io::epoch`),
    /// past theirs, its first snapshot saved. Not readable yet, or not kept on its disk: tried
    /// again next step, numbered on. Kept and not written to the NAS: it leads all the same, its
    /// NAS files written at its next step, nothing acknowledged until then.
    fn take_up(&mut self, io: &dyn Io, out: &mut Out) {
        let wait = |out: &mut Out, why: String| out.events.push(Event::Waits { what: "take up the records", why });
        // (No store to lead an epoch of yet: its agent's coordinator starts after its first step.)
        let epoch = match io.epoch() {
            Ok(e) => e,
            Err(e) => return wait(out, format!("its epoch: {e:#}")),
        };
        let saved = self.saved.records;
        let own = match &self.taking {
            Some(r) => Some(r.clone()),
            None => self.kept(io, out),
        };
        let (mut r, how) = match own {
            Some(r) => (r, "its own records"),
            None if saved.is_some() => {
                return wait(out, "its records aren't on its disk, and it saved records before: tried again".into())
            }
            None => match io.exists(records::ONE).and_then(|one| Ok(one || io.exists(records::ONE_BRIEF)?)) {
                Ok(false) => match Records::today(io) {
                    Ok(r) => (r, "today's files: the pool's first"),
                    Err(e) => return wait(out, format!("today's files: {e:#}")),
                },
                Ok(true) => {
                    return wait(
                        out,
                        format!(
                            "the pool's records ({}) aren't on this Mac's disk: a lead moved by hand takes the lead's \
                             folders with it (docs/pool.md §6.1)",
                            records::ONE
                        ),
                    )
                }
                Err(e) => return wait(out, format!("{e:#}")),
            },
        };
        // (Never from records older than those it saved last: its disk's copy rolled back. What
        // their saves acknowledged is on no member's disk any more: docs/coordination.md step 6.)
        if let Some((e, seq)) = saved.filter(|s| (r.epoch, r.seq) < *s) {
            return wait(
                out,
                format!(
                    "its records are epoch {}'s at {}, older than epoch {e}'s at {seq}, which it saved",
                    r.epoch, r.seq
                ),
            );
        }
        // (Its store numbered them, or the store before it on this disk: a store whose epoch isn't
        // past theirs is another's, whose job ids would come again.)
        if r.epoch > epoch {
            return wait(
                out,
                format!(
                    "its store's epoch {epoch} isn't past its records' ({}): its coordinator's store isn't the one that \
                     saved them (a lead moved by hand takes the lead's folders with it, docs/pool.md §6.1)",
                    r.epoch
                ),
            );
        }
        if r.epoch != epoch {
            (r.epoch, r.seq) = (epoch, 0);
        }
        // (Nothing merged yet: its members post it their hand-offs once they know it leads.)
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
        out.events.push(Event::TookUp { epoch, how: how.to_string() });
        self.lead =
            Some(Lead { epoch, since: io.now(), records: r, dirty, saved_at: io.awake(), held: BTreeMap::new() });
    }

    /// The lead's step: merge what members posted it (`handed`) and its own jobs' hand-offs, take off
    /// the raw archives the agent named, save, acknowledge.
    fn lead_step(&mut self, io: &dyn Io, out: &mut Out, handed: Vec<Entry>, named_raw: &[String], check: Check) {
        // (Held whole: members' posted by HTTP, and its own jobs', by key.)
        let whole: BTreeMap<String, Entry> =
            handed.into_iter().chain(self.saved.mine.entries().cloned()).filter_map(|e| Some((e.key()?, e))).collect();
        let l = self.lead.as_mut().expect("leading");
        let e = l.epoch;
        // The raw archives the agent named: off the records, for good.
        if !named_raw.is_empty() {
            let named: BTreeSet<&str> = named_raw.iter().map(String::as_str).collect();
            let was = l.records.raw.len();
            l.records.raw.retain(|(_, p)| !named.contains(p.name.as_str()));
            l.dirty |= l.records.raw.len() != was;
        }
        let m = records::merge(&mut l.records, whole.values(), check);
        if !m.applied.is_empty() || !m.refused.is_empty() || !m.overtaken.is_empty() {
            l.dirty = true;
            out.events.push(Event::Merged {
                applied: m.applied.clone(),
                overtaken: m.overtaken.len(),
                refused: m.refused.len(),
                done: m.done.clone(),
                again: m.again.clone(),
            });
        }
        // What its check holds is merged again next step (said once as it's first held).
        let held: BTreeMap<String, String> = m.held.into_iter().collect();
        for (k, why) in held.iter().filter(|(k, _)| !l.held.contains_key(*k)) {
            out.events.push(Event::Waits { what: "merge an entry", why: format!("{k}: {why}") });
        }
        l.held = held;
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
        // Acknowledged: what a saved snapshot names. Posted by HTTP, by the coordinator
        // (`Out::merged`); its own, dropped.
        if l.dirty {
            return;
        }
        let named: Vec<String> = whole.into_keys().filter(|k| l.records.handles(k)).collect();
        if named.is_empty() {
            return;
        }
        self.saved.mine.acked_all(&named);
        out.merged = named;
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
        }
    }
}

#[cfg(test)]
mod tests {
    //! The driver, scripted on the in-memory NAS: Macs with their own clocks, stepping in turn.
    use super::super::nas::{Created, Mem};
    use super::*;
    use anyhow::Result;
    use buildstore::handoff::Handoff;
    use jobrt::jobid::JobId;
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
        /// Its store's epoch (`Io::epoch`: 1 as it first opens, one more each restart).
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
                store: Cell::new(1),
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
        fn epoch(&self) -> Result<u64> {
            Ok(self.store.get())
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
        buildstore::records::load_one(mem).unwrap().unwrap()
    }

    fn entry(member: &str, epoch: u64, n: u64, t: &str) -> Entry {
        let h = Handoff {
            changes: [(format!("base/{t}"), Some(format!("base/{t}.k{n}.base")))].into(),
            done: Some(("unit".into(), vec![(t.into(), format!("k{n}"))])),
            ..Default::default()
        };
        Entry { member: member.into(), job: JobId { epoch, n }, step: "unit".into(), handoff: h, at: T0 + n }
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
        mem.0.borrow_mut().remove(lead::LEAD);
        let mut c = Driver::unlocked(member(C, "MacBook-Pro", V2), Saved::default());
        let ic = Mac::new(&mem);
        for _ in 0..3 {
            ic.pass(20);
            assert_eq!((step(&mut c, &ic, able()).leads, c.declared()), (None, None));
        }
        assert_eq!(lead::declared(&mem).unwrap(), None);
    }

    #[test]
    fn with_the_lead_gone_a_member_keeps_its_jobs_hand_offs_and_waits() {
        // (Task #881) A silent; B's jobs end: their hand-offs are kept whole in its saved state,
        // nothing written to the NAS nor sent, and B leads nothing; A back, posted them (B's beat),
        // it merges them, and B drops them once A's answer says so.
        let mem = Mem::default();
        setup(&mem);
        let (ia, ib) = (Mac::new(&mem), Mac::new(&mem));
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), Saved::default());
        step(&mut a, &ia, able());
        step(&mut b, &ib, able());
        ib.pass(3 * 3600);
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        let before = mem.0.borrow().clone();
        let o = step(&mut b, &ib, Heard { entries: vec![e], ..able() });
        assert!(o.leads.is_none() && o.send.is_empty(), "{o:?}");
        assert_eq!(*mem.0.borrow(), before, "nothing written");
        assert_eq!(b.saved().mine.keys().collect::<Vec<_>>(), [&key], "kept whole");
        for _ in 0..10 {
            ib.pass(600);
            assert_eq!(step(&mut b, &ib, able()).leads, None);
        }
        // A back, posted it: it merges it in its epoch, acknowledged.
        ia.wall.set(ib.now());
        let handed = b.mine().entries().cloned().collect();
        let o = step(&mut a, &ia, Heard { handed, ..able() });
        assert_eq!((o.leads, o.merged.clone()), (Some(1), vec![key.clone()]), "{:?}", o.events);
        assert!(one(&mem).handles(&key), "{:?}", o.events);
        ib.pass(20);
        step(&mut b, &ib, Heard { merged: o.merged, ..able() });
        assert_eq!(b.mine().unacked(), 0, "dropped once its lead's saved records name it");
    }

    /// docs/coordination.md step 8a, measured: a member's step learns its lead from its lead's beat
    /// answer (`Heard::lead_says`) with no NAS call, where it reads the declaration each step; with
    /// no word from its lead (lately, or ever: an older lead), it reads the declaration. Declared
    /// itself with none of the lead's folders on its disk (a hand move half done), it leads
    /// nothing. No member ever reads a term (step 8c).
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
        // The owner declares B, A's folders not copied to it. While A's answers say it leads, B
        // leads nothing; once B hears no word, it reads the declaration, and takes nothing up: the
        // pool's records aren't on its disk.
        declare(&mem, B);
        ib.pass(20);
        assert_eq!(b.step(&nb, says(), &any).leads, None);
        assert_eq!(b.declared(), Some(A));
        ib.pass(20);
        let o = b.step(&nb, able(), &any);
        assert_eq!((o.leads, b.declared()), (None, Some(B)), "{:?}", o.events);
        assert!(
            o.events.iter().any(|e| matches!(e, Event::Waits { why, .. } if why.contains("aren't on this Mac's disk"))),
            "{:?}",
            o.events
        );
        let calls = nb.take();
        assert!(
            calls.iter().all(|c| !c.contains("terms") && !c.contains("/term/") && !c.contains("lead.json")),
            "{calls:#?}"
        );
    }

    /// docs/coordination.md step 8c, a lead restart: its agent's store opens in a new epoch; the
    /// lead takes up from the records it kept on its own disk, reading the one path not at all
    /// (an earlier process's save that landed late over it is passed over) and no term; its
    /// take-up's save puts the newest back, in the new epoch. Its disk's copy lost, it leads
    /// nothing: a lost lead disk isn't taken up from the NAS.
    #[test]
    fn a_restarted_lead_takes_up_from_its_own_disk_in_a_new_epoch() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        declare(&mem, A);
        let last = Records { epoch: 840, seq: 7, ..Records::today(&mem).unwrap() };
        *ia.local.borrow_mut() = Some(serde_json::to_vec(&last).unwrap());
        mem.write_whole(records::ONE, &serde_json::to_vec(&last).unwrap()).unwrap();
        let saved = Saved { member: A.into(), records: Some((840, 7)), ..Default::default() };
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), saved);
        ia.store.set(841);
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(841), "{:?}", o.events);
        let e = entry(A, 841, 1, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e], ..able() });
        assert_eq!(o.merged, std::slice::from_ref(&key), "{:?}", o.events);
        // An earlier save lands late over the one path.
        let late = serde_json::to_vec(&Records { epoch: 841, seq: 1, ..Default::default() }).unwrap();
        mem.write_whole(records::ONE, &late).unwrap();
        // Restarted: from its own disk's copy.
        ia.restart();
        let io = Logged(&ia, Default::default());
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        ia.pass(20);
        let o = a.step(&io, able(), &any);
        assert_eq!(o.leads, Some(842), "{:?}", o.events);
        assert!(a.records().unwrap().handles(&key), "its own disk's records");
        assert!(o.events.iter().any(|e| matches!(e, Event::TookUp { epoch: 842, how } if how == "its own records")));
        let calls = io.take();
        let read = |c: &&String| c.starts_with("read ") || c.starts_with("stat ") || c.starts_with("list ");
        assert!(!calls.iter().filter(read).any(|c| c.contains("records")), "{calls:#?}");
        assert!(!calls.iter().any(|c| c.contains("terms") || c.contains("/term/")), "{calls:#?}");
        let now: Records = buildstore::records::now(&mem).unwrap();
        assert_eq!((now.epoch, now.handles(&key)), (842, true), "the take-up's save over the late one");
        // Its disk's copy lost: it leads nothing, however long.
        ia.restart();
        *ia.local.borrow_mut() = None;
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        for _ in 0..40 {
            ia.pass(600);
            let o = step(&mut a, &ia, able());
            assert_eq!(o.leads, None, "{:?}", o.events);
        }
    }

    /// What a save acknowledged is on no member's disk now (docs/coordination.md step 6): a lead
    /// whose disk's copy is older than the records it saved last (rolled back) waits for them
    /// rather than lose it; and a store whose epoch isn't past its records' (not the store that
    /// saved them) leads nothing.
    #[test]
    fn a_take_up_never_starts_from_records_older_than_it_saved() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ia, able());
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { handed: vec![e], ..able() });
        let saved = a.saved();
        assert_eq!(saved.records, Some((1, one(&mem).seq)));
        let newest = ia.local.borrow().clone();
        *ia.local.borrow_mut() = Some(serde_json::to_vec(&Records { epoch: 1, seq: 1, ..Default::default() }).unwrap());
        ia.restart();
        let mut b = Driver::unlocked(member(A, "Mac-mini", V1), saved.clone());
        ia.pass(20);
        let o = step(&mut b, &ia, able());
        assert_eq!(o.leads, None, "{:?}", o.events);
        assert!(o
            .events
            .iter()
            .any(|e| matches!(e, Event::Waits { what: "take up the records", why } if why.contains("older"))));
        *ia.local.borrow_mut() = None;
        ia.pass(20);
        let o = step(&mut b, &ia, able());
        assert!(
            o.leads.is_none()
                && o.events
                    .iter()
                    .any(|e| matches!(e, Event::Waits { why, .. } if why.contains("saved records before"))),
            "never today's files: {:?}",
            o.events
        );
        *ia.local.borrow_mut() = newest.clone();
        ia.pass(20);
        let o = step(&mut b, &ia, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(b.records().unwrap().handles(&key));
        // Its records with another store (epoch 1, behind them): nothing led.
        let ib = Mac::new(&mem);
        *ib.local.borrow_mut() = ia.local.borrow().clone();
        let mut c = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let o = step(&mut c, &ib, able());
        assert!(
            o.leads.is_none()
                && o.events.iter().any(|e| matches!(e, Event::Waits { why, .. } if why.contains("isn't past"))),
            "{:?}",
            o.events
        );
    }

    #[test]
    fn a_lead_whose_take_up_s_nas_save_fails_leads_on() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        *ia.wfail.borrow_mut() = Some("records.json".into());
        ia.pass(20);
        ia.restart();
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        let o = step(&mut a, &ia, able());
        assert_eq!((o.leads, o.caught_up), (Some(2), false), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Failed { what: "save the records", .. })));
        let e = entry(A, 2, 1, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e.clone()], ..able() });
        assert!(o.merged.is_empty() && !o.caught_up && o.duties, "{:?}", o.events);
        *ia.wfail.borrow_mut() = None;
        ia.pass(20);
        let o = step(&mut a, &ia, Heard { handed: vec![e], ..able() });
        assert_eq!((o.leads, o.caught_up), (Some(2), true), "{:?}", o.events);
        assert_eq!(o.merged, [key.clone()]);
        let now: Records = buildstore::records::now(&mem).unwrap();
        assert_eq!((now.epoch, now.handles(&key)), (2, true));
        // Its own disk refusing: no lead until it keeps them.
        ia.local_fails.set(true);
        ia.pass(20);
        ia.restart();
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), a.saved());
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, None, "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::Failed { what: "keep the records", .. })));
        ia.local_fails.set(false);
        ia.pass(20);
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(3), "{:?}", o.events);
        assert_eq!(a.records().unwrap().seq, 2, "numbered on from the try kept");
        assert!(a.records().unwrap().handles(&key));
    }

    /// The first take-up waits for its agent's store (none before its coordinator starts), and
    /// in a new pool takes today's files up at once, declaring itself; a lead whose saved state is
    /// lost takes up the records on its disk.
    #[test]
    fn a_take_up_waits_for_its_store_and_a_lost_state_takes_up_its_disks_records() {
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
            fn epoch(&self) -> Result<u64> {
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
        // Its saved state lost on the same Mac: its records on its disk taken up at once.
        ia.pass(20);
        ia.restart();
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let o = step(&mut a, &ia, able());
        assert_eq!(o.leads, Some(2), "{:?}", o.events);
        assert!(o.events.iter().any(|e| matches!(e, Event::TookUp { how, .. } if how == "its own records")));
    }

    #[test]
    fn a_lead_merges_what_s_posted_it_with_no_journal_read() {
        // (Task #1125) Hand-offs by HTTP alone: the lead merges what's posted it from memory,
        // saves, then acknowledges (`merged`); it neither lists nor reads the NAS's journal, and is
        // caught up once what's posted it is saved.
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let io = Logged(&ia, Default::default());
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        let o = a.step(&io, able(), &any);
        assert!(o.leads == Some(1) && o.caught_up, "{:?}", o.events);
        let e = entry(B, 1, 5, "6-1-5");
        let key = e.key().unwrap();
        ia.pass(20);
        let o = a.step(&io, Heard { handed: vec![e.clone()], ..able() }, &any);
        assert!(o.caught_up && o.merged == [key.clone()], "{:?}", o.events);
        assert!(one(&mem).handles(&key), "in the snapshot saved");
        // Posted again (its member's beat listing it to a restarted lead): passed over, acknowledged
        // again, merged once.
        ia.pass(20);
        let o = a.step(&io, Heard { handed: vec![e], ..able() }, &any);
        assert!(o.merged == [key.clone()] && !o.events.iter().any(|e| matches!(e, Event::Merged { .. })), "{o:?}");
        for _ in 0..40 {
            ia.pass(600);
            a.step(&io, able(), &any);
        }
        let calls = io.take();
        assert!(!calls.iter().any(|c| c.contains("journal") || c.starts_with("list ")), "{calls:#?}");
    }

    #[test]
    fn an_older_app_s_journal_entries_are_read_back_and_posted() {
        // (Task #1125) B ran an app before #1125 that wrote a hand-off to the NAS's journal for an
        // older lead and kept its key, unacknowledged. Its first step reads it back whole: kept, to
        // post; a key whose file isn't there is dropped, said; a read that fails is tried again.
        let mem = Mem::default();
        setup(&mem);
        declare(&mem, A);
        let ib = Mac::new(&mem);
        let (e, gone) = (entry(B, 1, 5, "6-1-5"), entry(B, 1, 6, "6-1-6"));
        let (key, gone) = (e.key().unwrap(), gone.key().unwrap());
        mem.write_whole(&format!("state/journal/{key}.json"), &serde_json::to_vec(&e).unwrap()).unwrap();
        let saved: Saved =
            serde_json::from_value(serde_json::json!({ "member": B, "mine": { "entries": [key, gone] } })).unwrap();
        let mut b = Driver::unlocked(member(B, "MacBook-Air", V2), saved);
        *ib.fail.borrow_mut() = Some("state/journal".into());
        ib.down.set(true);
        let o = step(&mut b, &ib, able());
        assert!(o.events.iter().any(|e| matches!(e, Event::Waits { what, .. } if what.contains("journal"))), "{o:?}");
        assert_eq!((b.mine().keys().count(), b.mine().unacked()), (0, 2), "tried again");
        ib.down.set(false);
        ib.pass(20);
        let o = step(&mut b, &ib, able());
        assert_eq!(b.mine().keys().collect::<Vec<_>>(), [&key], "read back whole, to post");
        assert!(
            o.events.iter().any(|ev| matches!(ev, Event::Failed { why, .. } if why.contains(&gone))),
            "{:?}",
            o.events
        );
        assert_eq!(b.mine().unacked(), 1);
        // (Review: another member id's saved state, this Mac's member file lost, had its journal keys
        // dropped unread: no lead reads the journal now, so they're read back as its own are.)
        let saved: Saved =
            serde_json::from_value(serde_json::json!({ "member": C, "mine": { "entries": [key] } })).unwrap();
        let mut b2 = Driver::unlocked(member(B, "MacBook-Air", V2), saved);
        step(&mut b2, &ib, able());
        assert_eq!(b2.mine().keys().collect::<Vec<_>>(), [&key], "read back whole under its new id");
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
        assert!(!mem.0.borrow().keys().any(|p| p.contains("journal")), "not written");
        assert_eq!(a.mine().unacked(), 0, "dropped once saved");
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
        fn epoch(&self) -> Result<u64> {
            self.0.epoch()
        }
    }

    /// docs/coordination.md steps 8b, 8c: a save of an earlier process's, landing late over the one
    /// path, is read until the lead saves again, `RESAVE_S` at most with nothing changed.
    #[test]
    fn a_late_save_over_the_one_path_is_saved_over() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V2), Saved::default());
        assert_eq!(step(&mut a, &ia, able()).leads, Some(1));
        let e = entry(A, 1, 1, "6-1-1");
        let key = e.key().unwrap();
        ia.pass(20);
        step(&mut a, &ia, Heard { handed: vec![e], ..able() });
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
            step(&mut a, &ia, able());
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

    /// A hand-off its check holds (`records::HOLD`: a member's on a newer app, outside this app's
    /// write-sets) is neither merged nor refused, merged again every step (posted, it stays in the
    /// coordinator's store until merged), said once, and keeps no catalogue back; once the check
    /// takes it (the lead on that app), it's merged and acknowledged.
    #[test]
    fn an_entry_held_is_merged_again_until_taken() {
        let mem = Mem::default();
        setup(&mem);
        let ia = Mac::new(&mem);
        let mut a = Driver::unlocked(member(A, "Mac-mini", V1), Saved::default());
        step(&mut a, &ia, able());
        let e = entry(B, 1, 3, "6-1-1");
        let key = e.key().unwrap();
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
        let posted = || Heard { handed: vec![e.clone()], ..able() };
        ia.pass(20);
        let o = a.step(&ia, posted(), &check);
        assert_eq!(o.held, [(key.clone(), "its member runs a newer app".to_string())]);
        assert!(o.caught_up && said(&o) == 1 && o.merged.is_empty(), "{:?}", o.events);
        assert!(!one(&mem).handles(&key), "neither merged nor refused");
        // Merged again the next step, held still, not said again; nothing acknowledged.
        ia.pass(20);
        let o = a.step(&ia, posted(), &check);
        assert!(o.held.len() == 1 && said(&o) == 0 && o.merged.is_empty(), "{:?}", o.events);
        // Taken: merged, acknowledged, no longer held.
        hold.set(false);
        ia.pass(20);
        let o = a.step(&ia, posted(), &check);
        assert!(o.held.is_empty() && one(&mem).reflected.contains(&key), "{:?}", o.events);
        assert_eq!(o.merged, [key]);
    }

    #[test]
    fn a_lost_member_files_hand_offs_are_kept() {
        // This Mac's member file lost, a new id made (D), its saved state A's: a hand-off its jobs
        // left is merged, the same bytes; what A led counts for nothing. (Re-review F5.)
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
        assert_eq!((s.member.as_str(), s.records, s.mine.keys().count()), (d, None, 1));
        assert_eq!(step(&mut dr, &ia, able()).leads, Some(1));
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
        step(&mut a, &ia, able());
        ia.pass(20);
        assert!(step(&mut a, &ia, able()).duties);
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
            a.saved().mine.keys().any(|k| *k == e.key().unwrap()),
            "the hand-off handed to it kept, for the agent to save"
        );
        ia.pass(20);
        assert!(a.step(&ia, able(), &any).stop.is_some(), "for good");
        assert_eq!(*mem.0.borrow(), before, "nothing written");
        drop(second);
    }
}
