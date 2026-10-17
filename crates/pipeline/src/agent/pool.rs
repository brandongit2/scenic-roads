//! The agent's part in the pool (docs/pool.md §12, phase 1's integration): what runs the pool's
//! driver (crate::pool::driver) and does what it can't do itself. The driver's contract (its
//! module's doc) says what that is; this module keeps it:
//!
//! - **Stepped every 20 s or so, whatever the agent is doing** (`Run`, shared under a lock:
//!   `Shared`): by the agent's loop, its step and what follows it in one hold of the run
//!   (`Run::in_step`), and by the pool's own thread (`Ticker`) whenever that loop hasn't stepped it
//!   for `QUIET_S` (a plan, a job being stopped, a share slow to answer): `Run::tick`, every `TICK_S`
//!   while the loop stays busy. A tick does what's the pool's alone: the owner's asks, the driver's
//!   step (its heartbeat, mail and listings with it), a handover's offer, answer, settling (when no
//!   duty runs or starts here: `Cx`) and pass, the history, the view (`VIEW`), whether the
//!   coordinator grants. What's the agent's (its jobs, its coordinator's start and stop, the state a
//!   term taken up loads: `Run::pending_took_up`) waits for its loop's next step. One step at a
//!   time, each file of the pool's written by the run alone.
//! - **One process per member** (`Side::open`): the member's id (crate::pool::member_id, in the
//!   agent's folder), its lock in the app's folder (crate::pool::MemberLock), and the driver made
//!   from the state saved last. A process the lock is held from runs no driver.
//! - **The saved state** (`Saved`) written whole to the agent's folder after every step that changed
//!   it, before anything that step said is acted on; a job's hand-off handed to a step kept (its
//!   folder) until a saved state holding it is on disk.
//! - **The listings** the driver asks for, made on a thread of their own one at a time, every one
//!   handed back once, a failed one made again a minute later; and a listing of the members'
//!   heartbeats the same way, every fifth of the time a heartbeat takes to go out of touch (two
//!   minutes: nothing lists a folder in the loop).
//! - **The messages** between members, by mailbox on the NAS (`MAIL`: `<to>/<from>.json`, written
//!   whole by its sender alone, the last `KEPT` it sent there, each numbered; read by member id,
//!   never by listing): best effort, as the driver's are (a message lost is told again or made up
//!   for). The pool's API (§9) is planned.
//! - **The heartbeat**: the driver's fields, the members it knows, written each step they change
//!   and at least every two minutes, stamped as it's written, to `state/pool/members/<id>.json`.
//! - **The merge's checks** (`check`): what phase 1 checks of an entry.
//!
//! The pool is always on (§12 phase 4's thirteenth batch): every agent with the NAS leads or works
//! as a member as the terms say. (`state/pool/enabled`, the switch of before, is no longer read; an
//! older app reads it, so it stays on the NAS until none runs.)

use crate::handoff::Handoff;
use crate::pool::driver::{self, Driver, Event, Heard, Io, Listed, Listing, Msg, Out, Saved};
use crate::pool::journal::{self, Entry};
use crate::pool::nas::{Created, Nas, Share};
use crate::pool::records::Records;
use crate::pool::{Beat, Member, MemberLock};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The switch that has a member's part change in its process (docs/pool.md §12, phase 4's fourth
/// batch): its coordinator started as it takes the lead, stopped as it loses it, its jobs going on
/// through the lead wherever it is (`on` in it; missing, as `SLOTS_DEFAULT` says). Off, a process
/// whose part changes restarts into it once its first job's slot is free.
pub const SLOTS: &str = "state/pool/slots";
/// Whether a part changes in its process while the switch is missing.
pub const SLOTS_DEFAULT: bool = false;

/// Whether a member's part changes in its process, as the switch at `root` says (`SLOTS`); None
/// when it can't be read now.
pub fn slots_on(root: &Path) -> Option<bool> {
    match std::fs::read_to_string(root.join(SLOTS)) {
        Ok(s) => Some(match s.trim() {
            "off" => false,
            _ => true,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(SLOTS_DEFAULT),
        Err(_) => None,
    }
}

/// The members' mailboxes: `<to>/<from>.json`.
pub const MAIL: &str = "state/pool/mail";
/// The members' heartbeats (crate::pool::beat::path).
pub const MEMBERS: &str = "state/pool/members";
/// The messages a sender keeps in a mailbox: its last.
const KEPT: usize = 64;
/// How often the members' heartbeats are listed (off the loop), for the members it knows (a lead
/// reads their mail; every member's view says which are out of touch): a fifth of the time a
/// heartbeat takes to go out of touch (crate::pool::beat::OUT_OF_TOUCH_S), so no view of it lags
/// that by more.
const MEMBERS_EVERY: Duration = Duration::from_secs(crate::pool::beat::OUT_OF_TOUCH_S / 5);
/// A member whose heartbeat file a listing doesn't find is forgotten once listings have missed it
/// this long (twice the time a heartbeat takes to go out of touch): never on a failed listing, nor
/// on one that misses this member's own heartbeat (the folder not there, the NAS away).
pub const FORGET_AFTER: Duration = Duration::from_secs(2 * crate::pool::beat::OUT_OF_TOUCH_S);
/// The members forgotten (`Side::forgotten`), in the agent's pool folder: kept across restarts.
const FORGOTTEN: &str = "forgotten.json";
/// A failed listing is made again after this.
const AGAIN: Duration = Duration::from_secs(60);
/// The heartbeat is written at least this often.
const BEAT_EVERY: Duration = Duration::from_secs(120);
/// A Tell the same as the last to that member is sent again only after this (the lead hears it
/// each loop otherwise; it keeps what it was told).
const TELL_AGAIN: Duration = Duration::from_secs(600);
/// The pool's own thread (`Ticker`) steps the driver when no step has run for this long (s, awake)…
pub const TICK_S: u64 = 20;
/// …and the agent's loop hasn't stepped it for this long (s, awake): its loop steps it about every
/// 20 s while its steps are short, and the thread steps it meanwhile when one is long (a plan, a
/// job stopped, a share slow to answer). No more than this goes between two steps of a member's
/// driver while its process runs, whatever the agent's loop is doing.
pub const QUIET_S: u64 = 25;
/// The pool as this member's process last stepped it (`Run::write_view`): in the agent's pool
/// folder, written by the run alone (under its lock), at every step.
pub const VIEW: &str = "view.json";

/// The pool as a member's process last stepped it (`VIEW`): when (its clock, unix s), and the view.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolFile {
    pub beat: u64,
    pub pool: super::PoolView,
}

/// The pool's view this Mac's agent last wrote (`VIEW`, in its folder `home`'s pool folder).
pub fn read_view(home: &Path) -> Option<PoolFile> {
    std::fs::read(home.join("pool").join(VIEW)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// A NAS the driver's steps and the listings' thread share.
pub type SharedNas = Arc<dyn Nas + Send + Sync>;

/// The driver's world: a NAS and this process's clocks (the awake one, `Instant`, stops while the
/// Mac sleeps).
struct Clocked<'a> {
    nas: &'a dyn Nas,
    clock: &'a dyn store::clock::Clock,
    start: Instant,
    /// Both clocks moved on this far (s): the tests' way of letting time pass.
    ahead: u64,
}

impl Nas for Clocked<'_> {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        self.nas.create_new(path, bytes)
    }
    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.nas.write_whole(path, bytes)
    }
    fn write_aside(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.nas.write_aside(path, bytes)
    }
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.nas.read(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.nas.exists(path)
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.nas.list(dir)
    }
    fn remove(&self, path: &str) -> Result<()> {
        self.nas.remove(path)
    }
}

impl Io for Clocked<'_> {
    fn now(&self) -> u64 {
        self.clock.wall().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) + self.ahead
    }
    fn awake(&self) -> u64 {
        self.clock.now().saturating_duration_since(self.start).as_secs() + self.ahead
    }
}

/// A mailbox as its sender writes it: the last `KEPT` messages it sent its recipient, each
/// numbered (higher is later; numbers never repeat across the sender's processes).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Mailbox {
    msgs: Vec<(u64, Msg)>,
}

/// The path of the mailbox `from` sends `to`.
pub fn mail_path(to: &str, from: &str) -> String {
    format!("{MAIL}/{to}/{from}.json")
}

/// What a member keeps of its mail, in the agent's folder (`mail.json`): the number of the last
/// message it took from each sender, so a restart doesn't take one again; what it sent each member
/// lately (rewritten whole as it sends more).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Mail {
    #[serde(default)]
    read: BTreeMap<String, u64>,
    #[serde(default)]
    sent: BTreeMap<String, Vec<(u64, Msg)>>,
    #[serde(default)]
    n: u64,
}

/// The heartbeat a member writes (`state/pool/members/<id>.json`): the driver's fields, and the
/// members it knows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    #[serde(flatten)]
    pub pool: Beat,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// Whether this was a shadow run's (an older app's, §12: acting on nothing; left out since).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shadow: bool,
    /// Its Mac's conditions, for the controls (§11: whether it's home on power, on battery, away;
    /// whether it can lead now). Left out by apps before phase 3, and ignored by them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conds: Option<Conds>,
    /// Its broker (docs/pool.md §8, crate::agent::brokers: `state/pool/brokers` on): where a page
    /// reaches it, and how many of its jobs' tasks wait, of which kinds, how large. Left out with
    /// the switch off, and by apps before it (ignored by them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker: Option<crate::coord::brokers::Broker>,
}

/// A member's conditions, as its heartbeat says them (§10, §11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conds {
    /// At home: the NAS answers on the LAN.
    pub home: bool,
    /// On mains power.
    pub ac: bool,
    /// The battery's charge (%), when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery: Option<u8>,
    /// It can lead now (its disk, home and power: `Heard::able`).
    pub able: bool,
}

impl Conds {
    /// Why a member with these conditions can't lead now (`able` false), as the agent decides it
    /// (crate::agent's `pool_step`: home, on power or charged past `BATTERY_MIN`, the reserve free).
    pub fn why_unable(&self) -> String {
        if !self.home {
            "away from home: it can't lead until it's back".into()
        } else if !self.ac && self.battery.is_some_and(|b| b < crate::agent::cond::BATTERY_MIN) {
            format!("on battery at {}%: it can't lead below {}%", self.battery.unwrap_or(0), crate::agent::cond::BATTERY_MIN)
        } else {
            format!("short of room on its disk: it can't lead with less than {} GB free", crate::agent::room::RESERVE >> 30)
        }
    }
}

/// A listing made off the loop: its kind, and the thread making it.
struct Making {
    what: Kind,
    thread: std::thread::JoinHandle<Result<Vec<String>>>,
}

enum Kind {
    Journal(Listing),
    Members,
}

/// What the agent hands a step.
#[derive(Default)]
pub struct Give {
    /// Its jobs' hand-offs that ended, each with the folder (or file) holding it, removed once a
    /// saved state holding the entry is on disk.
    pub entries: Vec<(Entry, Option<PathBuf>)>,
    /// Settling a handover: the coordinator's state, as written.
    pub settled: Option<serde_json::Value>,
    /// Whether this Mac can lead now (its disk, home, power).
    pub able: bool,
    /// Re-assert first (before a GC sweep).
    pub reassert: bool,
    /// Its owner's asks.
    pub asks: Vec<driver::Ask>,
    /// Its Mac's conditions, for its heartbeat (None: left out).
    pub conds: Option<Conds>,
}

/// One member's part in the pool, in this process.
pub struct Side {
    me: Member,
    /// (None only while a restart lets its lock go.)
    driver: Option<Driver>,
    nas: SharedNas,
    start: Instant,
    /// The agent's pool folder (its saved state, mail marks, log).
    dir: PathBuf,
    /// The saved state as last written, and the folders of hand-offs it doesn't hold yet.
    written: Vec<u8>,
    folders: Vec<PathBuf>,
    mail: Mail,
    mail_written: Vec<u8>,
    /// The mailboxes posted to and not written since.
    mail_dirty: BTreeSet<String>,
    /// The last Tell sent each member, and when.
    told: BTreeMap<String, (Vec<String>, Instant)>,
    /// The members it knows (itself among them), as listed last and heard from since.
    members: BTreeSet<String>,
    members_listed: Option<Instant>,
    members_queued: bool,
    /// The members the listings missed, since when (`FORGET_AFTER`).
    missed: BTreeMap<String, Instant>,
    /// The members forgotten: not known again from a message or a term until a listing finds
    /// their heartbeat.
    forgotten: BTreeSet<String>,
    /// The listings asked for and not made yet, the one being made, those made and not handed back,
    /// and when one last failed.
    asked: VecDeque<Listing>,
    making: Option<Making>,
    made: VecDeque<Listed>,
    failed_at: Option<Instant>,
    /// The heartbeat as last written (beat left out), and when (and `ahead` then).
    beat: Option<(Heartbeat, Instant, u64)>,
    /// It stopped for good (another process holds its lock): why.
    pub stopped: Option<String>,
    /// Its Mac's conditions as last given, for its heartbeat.
    conds: Option<Conds>,
    /// Its broker, for its heartbeat (crate::agent::brokers; None: the switch off).
    pub broker: Option<crate::coord::brokers::Broker>,
    /// Whom it reached and whom it couldn't, for its heartbeat (crate::pool::reachable).
    pub reach: Vec<crate::pool::reachable::Reach>,
    /// The members' heartbeats (their pool's fields, shadow runs' left out) as `Run::tend_reach`
    /// last read them: handed to each step (`Heard::reports`), so the driver's `hand_to` reads
    /// none from the NAS.
    pub beats: Vec<Beat>,
    /// When (unix s) `beats` were last read whole: a round with a read failing keeps the last.
    pub beats_at: Option<u64>,
    /// Both its clocks moved on this far (s): tests let time pass so.
    pub ahead: u64,
    /// Its clocks (a test's virtual one: `set_clock`).
    clock: Arc<dyn store::clock::Clock>,
}

impl Side {
    /// This Mac's member in the pool, in folder `dir` (the agent's pool folder: its saved state)
    /// with its member id in `home` (the agent's folder), its lock in `locks` (the app's folder),
    /// over `nas`. None when another process holds the member's lock (this one runs no driver).
    pub fn open(home: &Path, dir: &Path, locks: &Path, app: &str, nas: SharedNas) -> Result<Option<Side>> {
        let id = crate::pool::member_id(home)?;
        let Some(lock) = MemberLock::take(locks, &id)? else { return Ok(None) };
        std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
        let me = Member { id: id.clone(), host: crate::agent::cond::host_name(), app: app.to_string() };
        let (mut saved, written) = load_saved(&dir.join("saved.json"));
        // (A state from an earlier time the pool was on, its terms since moved aside: it names terms
        // no longer there, and would hold this member's view above them. Counted as lost: its jobs'
        // hand-offs not written yet alone kept, as the driver keeps another member's.)
        if saved.term > 0 && nas.list(crate::pool::term::DIR).is_ok_and(|t| t.is_empty()) {
            eprintln!("pool: the NAS has no terms: this member's saved state, of term {}, is from an earlier time the pool was on; set aside", saved.term);
            std::fs::rename(dir.join("saved.json"), dir.join("saved.earlier.json")).ok();
            saved = Saved { mine: saved.mine.unwritten_only(), ..Default::default() };
        }
        let mail: Mail = std::fs::read(dir.join("mail.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let known: Vec<String> = std::fs::read(dir.join("members.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let forgotten: BTreeSet<String> = std::fs::read(dir.join(FORGOTTEN)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut members: BTreeSet<String> = known.into_iter().filter(|m| crate::pool::is_member_id(m) && !forgotten.contains(m)).collect();
        members.insert(id);
        let driver = Driver::new(me.clone(), saved, lock);
        Ok(Some(Side { me, driver: Some(driver), nas, start: Instant::now(), dir: dir.to_path_buf(), written, folders: Vec::new(), mail, mail_written: Vec::new(), mail_dirty: BTreeSet::new(), told: BTreeMap::new(), members, members_listed: None, members_queued: false, missed: BTreeMap::new(), forgotten, asked: VecDeque::new(), making: None, made: VecDeque::new(), failed_at: None, beat: None, stopped: None, conds: None, broker: None, reach: Vec::new(), beats: Vec::new(), beats_at: None, ahead: 0, clock: store::clock::real() }))
    }

    /// Its member.
    pub fn member(&self) -> &Member {
        &self.me
    }

    /// The driver (the controls' queries: `takeover`, `hand_to`; the current term; the records).
    pub fn driver(&self) -> &Driver {
        self.driver.as_ref().expect("a driver")
    }

    /// The NAS its steps use.
    pub fn nas(&self) -> &dyn Nas {
        &*self.nas
    }

    /// The members it knows.
    pub fn members(&self) -> &BTreeSet<String> {
        &self.members
    }

    /// Knows member `m` (its mail read from now on), before a listing of the heartbeats finds it.
    pub fn know(&mut self, m: &str) {
        if crate::pool::is_member_id(m) {
            self.forgotten.remove(m);
            self.members.insert(m.to_string());
        }
    }

    /// Knows member `m` from a message or a term, unless it was forgotten (its heartbeat gone).
    fn heard_of(&mut self, m: &str) {
        if !self.forgotten.contains(m) {
            self.members.insert(m.to_string());
        }
    }

    /// The members heard from lately (their heartbeats as last read whole, not out of touch then),
    /// this one among them: what the budgets divide by.
    pub fn heard_lately(&self) -> usize {
        let at = self.beats_at.unwrap_or(0);
        let mut ids: BTreeSet<&str> = self.beats.iter().filter(|b| b.beat > 0 && !b.out_of_touch(at)).map(|b| b.member.as_str()).collect();
        ids.insert(&self.me.id);
        ids.len()
    }

    /// A listing of the heartbeats taken in at `now` (`names` the files found): the members found
    /// known, those missed `FORGET_AFTER` forgotten; members.json written.
    fn take_members(&mut self, names: &[String], now: Instant) {
        let found: BTreeSet<String> = names.iter().filter_map(|n| n.strip_suffix(".json")).filter(|m| crate::pool::is_member_id(m)).map(str::to_string).collect();
        let gone = forget(&mut self.members, &mut self.missed, &found, &self.me.id, self.beat.is_some(), now, FORGET_AFTER);
        let before = self.forgotten.clone();
        self.forgotten.retain(|m| !found.contains(m));
        for m in gone {
            eprintln!("pool: member {m} forgotten: its heartbeat not on the NAS for {} min", FORGET_AFTER.as_secs() / 60);
            // (Its mail not sent: nobody reads it.)
            self.mail.sent.remove(&m);
            self.mail_dirty.remove(&m);
            self.told.remove(&m);
            self.forgotten.insert(m);
        }
        if self.forgotten != before {
            if let Ok(b) = serde_json::to_vec(&self.forgotten) {
                crate::whole::write(&self.dir.join(FORGOTTEN), &b).ok();
            }
            self.save_mail();
        }
        if let Ok(b) = serde_json::to_vec(&self.members) {
            crate::whole::write(&self.dir.join("members.json"), &b).ok();
        }
    }

    /// Whether the lead can be handed to member `to` now, and why not (`Driver::hand_to`, §11), and
    /// whether it can lead now by its heartbeat's conditions (`Conds::able`): a member that can't
    /// wouldn't answer the offer (its driver's `ready_for` takes `Give::able`), so the views and the
    /// asks refuse it up front, saying why, rather than offer and give up a minute on.
    pub fn hand_to(&self, to: &str) -> std::result::Result<(), String> {
        self.hand_to_with(to, &self.regions_app())
    }

    /// `hand_to`, the readers' switches' app (`regions_app`) read already: a view reads it once.
    pub fn hand_to_with(&self, to: &str, regions: &std::result::Result<Option<String>, String>) -> std::result::Result<(), String> {
        self.driver().hand_to(&self.io(), to)?;
        let app = self.heartbeat(to).map(|h| h.pool.app).unwrap_or_default();
        crate::inputs::regions::read::may_lead(&app, regions)?;
        match self.heartbeat(to).and_then(|h| h.conds) {
            Some(c) if !c.able => Err(c.why_unable()),
            _ => Ok(()),
        }
    }

    /// The app the newer readers' switch (the regions', the elevation datasets') was recorded on, when the records have it
    /// (crate::inputs::regions::read::switch_app): no member on an older one leads.
    pub fn regions_app(&self) -> std::result::Result<Option<String>, String> {
        let read = |n: &str| -> anyhow::Result<Vec<u8>> { self.nas.read(n)?.with_context(|| format!("{n} isn't there")) };
        let m: BTreeMap<String, String> = serde_json::from_slice(&read("state/build/manifest.json").map_err(|e| format!("{e:#}"))?).map_err(|e| format!("the records: {e}"))?;
        crate::inputs::regions::read::switch_app(&m, &read).map_err(|e| format!("{e:#}"))
    }

    /// What a takeover from this Mac needs now, and why (`Driver::takeover`, §11).
    pub fn takeover(&self) -> driver::Takeover {
        self.driver().takeover(&self.io())
    }

    /// Member `m`'s heartbeat as it wrote it (None: none, or one that can't be read now).
    pub fn heartbeat(&self, m: &str) -> Option<Heartbeat> {
        self.read_heartbeat(m).ok().flatten()
    }

    /// Member `m`'s heartbeat: none when there's none (or it doesn't parse), an error when the NAS
    /// couldn't be read.
    pub fn read_heartbeat(&self, m: &str) -> Result<Option<Heartbeat>> {
        Ok(self.nas.read(&crate::pool::beat::path(m))?.and_then(|b| serde_json::from_slice(&b).ok()))
    }

    /// This Mac's wall clock, as its steps read it.
    pub fn now(&self) -> u64 {
        self.io().now()
    }

    /// Its clocks from now on `clock` (a test's virtual one: time then passes only as the test
    /// lets it, however slowly a busy Mac steps it).
    pub fn set_clock(&mut self, clock: Arc<dyn store::clock::Clock>) {
        self.start = clock.now();
        self.clock = clock;
    }

    fn io(&self) -> Clocked<'_> {
        Clocked { nas: &*self.nas, clock: &*self.clock, start: self.start, ahead: self.ahead }
    }

    /// The listings asked for and not handed back yet (asked, being made, made).
    pub fn listings_out(&self) -> usize {
        self.asked.len() + self.making.is_some() as usize + self.made.len()
    }

    /// A new driver for this member from its saved state, as a new process would make one: the
    /// lock let go and taken again.
    pub fn restart(&mut self) -> Result<()> {
        let (saved, written) = load_saved(&self.dir.join("saved.json"));
        // (The old driver, its lock with it, goes first.)
        self.driver = None;
        let lock = self.lock_again();
        // (Its lock not taken again, another process has it: this one stops.)
        let lock = match lock {
            Ok(l) => l,
            Err(e) => {
                self.stopped = Some(format!("{e:#}"));
                return Err(e);
            }
        };
        self.driver = Some(Driver::new(self.me.clone(), saved, lock));
        self.written = written;
        self.asked.clear();
        self.made.clear();
        Ok(())
    }

    fn lock_again(&self) -> Result<MemberLock> {
        let locks = self.locks_dir();
        MemberLock::take(&locks, &self.me.id)?.context("another process holds the member's lock")
    }

    /// The folder its lock is in: the app's (the agent's folder's parent).
    fn locks_dir(&self) -> PathBuf {
        self.dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| self.dir.clone())
    }

    /// One step: what it heard (its mail, the listings made), what the agent gives, the driver's
    /// step with `check`, its saved state written if it changed; then, only once that's on disk, the
    /// step's messages sent, its heartbeat written, the listings it asked for begun. A step whose
    /// state couldn't be written does none of that, and says the lead may do nothing now (its
    /// duties, catalogs and sweeps held) until one is.
    pub fn step(&mut self, give: Give, check: crate::pool::records::Check) -> Out {
        if let Some(why) = &self.stopped {
            return Out { stop: Some(why.clone()), ..Default::default() };
        }
        self.poll_listings();
        let msgs = self.take_mail();
        let (entries, folders): (Vec<Entry>, Vec<Option<PathBuf>>) = give.entries.into_iter().unzip();
        self.folders.extend(folders.into_iter().flatten());
        let heard = Heard { msgs, asks: give.asks, entries, listed: self.made.pop_front(), settled: give.settled, able: give.able, reassert: give.reassert, members: self.members.iter().cloned().collect(), reports: self.beats.clone() };
        self.conds = give.conds;
        let io = Clocked { nas: &*self.nas, clock: &*self.clock, start: self.start, ahead: self.ahead };
        let Some(driver) = self.driver.as_mut() else { return Out { stop: self.stopped.clone(), ..Default::default() } };
        let mut out = driver.step(&io, heard, check);
        let kept = self.keep();
        if let Some(why) = &out.stop {
            self.stopped = Some(why.clone());
            if let Err(e) = &kept {
                eprintln!("pool: its state not saved as it stops ({e:#}): its jobs' hand-offs stay in their folders");
            }
            return out;
        }
        if let Err(e) = kept {
            out.events.push(Event::Failed { what: "save the member's state", why: format!("{e:#}") });
            (out.duties, out.settle, out.caught_up, out.fresh) = (false, false, false, false);
            out.send.clear();
            // (Its listings are made all the same: the driver counts them asked.)
            self.asked.extend(out.list.clone());
            return out;
        }
        if let Some(t) = self.driver().current().lead.clone() {
            self.heard_of(&t.member);
        }
        for (to, m) in std::mem::take(&mut out.send) {
            self.post(&to, m.clone());
            out.send.push((to, m));
        }
        self.send_mail();
        self.write_beat(&out);
        self.asked.extend(out.list.clone());
        self.list_members();
        self.next_listing();
        out
    }

    /// Writes the saved state when it changed; the folders of the hand-offs it now holds removed.
    fn keep(&mut self) -> Result<()> {
        let b = serde_json::to_vec(&self.driver().saved())?;
        if b != self.written {
            crate::whole::write(&self.dir.join("saved.json"), &b)?;
            self.written = b;
        }
        for f in std::mem::take(&mut self.folders) {
            let gone = if f.is_dir() { std::fs::remove_dir_all(&f) } else { std::fs::remove_file(&f) };
            if let Err(e) = gone {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("pool: removing {} (its hand-off is in the saved state): {e}", f.display());
                }
            }
        }
        Ok(())
    }

    /// The messages its members sent it since it last read, by member id (none listed: read by the
    /// ids it knows).
    fn take_mail(&mut self) -> Vec<(String, Msg)> {
        let mut msgs = Vec::new();
        let others: Vec<String> = self.members.iter().filter(|m| **m != self.me.id).cloned().collect();
        for from in others {
            let b = match self.nas.read(&mail_path(&self.me.id, &from)) {
                Ok(Some(b)) => b,
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("pool: reading {from}'s mail: {e:#}");
                    continue;
                }
            };
            let Ok(mb) = serde_json::from_slice::<Mailbox>(&b) else { continue };
            let last = self.mail.read.get(&from).copied().unwrap_or(0);
            let mut top = last;
            for (n, m) in mb.msgs {
                if n > last {
                    top = top.max(n);
                    msgs.push((from.clone(), m));
                }
            }
            if top > last {
                self.mail.read.insert(from, top);
            }
        }
        self.save_mail();
        msgs
    }

    /// Queues message `m` to member `to` (a Tell the same as the last, sent lately, is passed over).
    fn post(&mut self, to: &str, m: Msg) {
        if self.forgotten.contains(to) {
            return;
        }
        self.heard_of(to);
        if let Msg::Tell(keys) = &m {
            let now = self.clock.now();
            if self.told.get(to).is_some_and(|(k, at)| k == keys && now.saturating_duration_since(*at) < TELL_AGAIN) {
                return;
            }
            self.told.insert(to.to_string(), (keys.clone(), now));
        }
        // (Numbered from the clock in milliseconds, and on from the last: never again across this
        // member's processes.)
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.mail.n = (self.mail.n + 1).max(ms);
        let q = self.mail.sent.entry(to.to_string()).or_default();
        q.push((self.mail.n, m));
        if q.len() > KEPT {
            q.drain(..q.len() - KEPT);
        }
        self.mail_dirty.insert(to.to_string());
    }

    /// Writes the mailboxes it posted to since.
    fn send_mail(&mut self) {
        for to in std::mem::take(&mut self.mail_dirty) {
            let mb = Mailbox { msgs: self.mail.sent.get(&to).cloned().unwrap_or_default() };
            match serde_json::to_vec(&mb).map_err(anyhow::Error::from).and_then(|b| self.nas.write_whole(&mail_path(&to, &self.me.id), &b)) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("pool: mail to {to}: {e:#} (sent with the next)");
                    self.told.remove(&to);
                    self.mail_dirty.insert(to);
                    break;
                }
            }
        }
        self.save_mail();
    }

    fn save_mail(&mut self) {
        let Ok(b) = serde_json::to_vec(&self.mail) else { return };
        if b != self.mail_written && crate::whole::write(&self.dir.join("mail.json"), &b).is_ok() {
            self.mail_written = b;
        }
    }

    /// Its heartbeat, when it changed or every two minutes: stamped as it's written.
    fn write_beat(&mut self, out: &Out) {
        let mut hb = Heartbeat { pool: Beat { beat: 0, ..out.beat.clone() }, members: self.members.iter().cloned().collect(), shadow: false, conds: self.conds, broker: self.broker.clone() };
        if hb.pool.member.is_empty() {
            return;
        }
        // (The reports' times alone changed: written with the next beat due, not at once.)
        let reach = self.reach.clone();
        hb.pool.reach = reach.iter().map(|r| crate::pool::reachable::Reach { at: 0, ..r.clone() }).collect();
        let now = self.clock.now();
        if self.beat.as_ref().is_some_and(|(b, at, ahead)| *b == hb && now.saturating_duration_since(*at) + Duration::from_secs(self.ahead - ahead) < BEAT_EVERY) {
            return;
        }
        let mut stamped = hb.clone();
        stamped.pool.beat = self.now();
        stamped.pool.reach = reach;
        match serde_json::to_vec(&stamped).map_err(anyhow::Error::from).and_then(|b| self.nas.write_aside(&crate::pool::beat::path(&self.me.id), &b)) {
            Ok(()) => self.beat = Some((hb, now, self.ahead)),
            Err(e) => eprintln!("pool: heartbeat: {e:#}"),
        }
    }

    /// The members' heartbeats listed every `MEMBERS_EVERY`, off the loop.
    fn list_members(&mut self) {
        let every = MEMBERS_EVERY;
        let now = self.clock.now();
        if self.members_listed.is_none_or(|t| now.saturating_duration_since(t) >= every) && !self.members_queued {
            self.members_queued = true;
        }
    }

    /// A listing made: handed back, or (the members') taken in; a failed one made again a minute on.
    fn poll_listings(&mut self) {
        let Some(m) = self.making.take_if(|m| m.thread.is_finished()) else { return };
        let r = m.thread.join().unwrap_or_else(|_| Err(anyhow::anyhow!("the listing's thread panicked")));
        match (m.what, r) {
            (Kind::Journal(l), Ok(keys)) => self.made.push_back(Listed { n: l.n, keys }),
            (Kind::Journal(l), Err(e)) => {
                eprintln!("pool: listing the journal: {e:#}; made again in a minute");
                self.asked.push_front(l);
                self.failed_at = Some(self.clock.now());
            }
            (Kind::Members, Ok(names)) => {
                self.members_listed = Some(self.clock.now());
                self.members_queued = false;
                let now = self.clock.now();
                self.take_members(&names, now);
            }
            (Kind::Members, Err(e)) => {
                eprintln!("pool: listing the members: {e:#}");
                self.members_queued = false;
                self.members_listed = Some(self.clock.now());
            }
        }
    }

    /// The next listing begun, when none is being made: the journal's asked first.
    fn next_listing(&mut self) {
        let now = self.clock.now();
        if self.making.is_some() || self.failed_at.is_some_and(|t| now.saturating_duration_since(t) < AGAIN) {
            return;
        }
        let nas = self.nas.clone();
        if let Some(l) = self.asked.pop_front() {
            let since = l.since.clone();
            let thread = std::thread::spawn(move || journal::list(&*nas, since.as_deref()));
            self.making = Some(Making { what: Kind::Journal(l), thread });
        } else if self.members_queued {
            let thread = std::thread::spawn(move || nas.list(MEMBERS));
            self.making = Some(Making { what: Kind::Members, thread });
        }
    }
}

/// A listing of the heartbeats that found the members `found` at `now`, taken into `members`: those
/// found known (and no longer missed); those not found missed from now, and forgotten (taken out,
/// and returned) once missed `after`. A listing without member `me`'s own heartbeat, once it has
/// written one (`written`), forgets nothing and starts no miss: the folder isn't all there.
fn forget(members: &mut BTreeSet<String>, missed: &mut BTreeMap<String, Instant>, found: &BTreeSet<String>, me: &str, written: bool, now: Instant, after: Duration) -> Vec<String> {
    members.extend(found.iter().cloned());
    missed.retain(|m, _| !found.contains(m) && members.contains(m));
    if !written || !found.contains(me) {
        return Vec::new();
    }
    let mut gone = Vec::new();
    for m in members.iter().filter(|m| *m != me && !found.contains(*m)) {
        let since = *missed.entry(m.clone()).or_insert(now);
        if now.saturating_duration_since(since) >= after {
            gone.push(m.clone());
        }
    }
    for m in &gone {
        members.remove(m);
        missed.remove(m);
    }
    gone
}

/// The state saved at `p`, and its bytes as read; an empty one when there's none or it can't be
/// read (set aside as `.bad`, for the owner: the driver counts a lost state for nothing but the
/// hand-offs it held, which their folders still hold).
fn load_saved(p: &Path) -> (Saved, Vec<u8>) {
    match std::fs::read(p) {
        Ok(b) => match serde_json::from_slice::<Saved>(&b) {
            Ok(s) => (s, b),
            Err(e) => {
                eprintln!("pool: {} can't be read ({e}); set aside, the member starts as if it had none", p.display());
                std::fs::rename(p, p.with_extension("bad")).ok();
                (Saved::default(), Vec::new())
            }
        },
        Err(_) => (Saved::default(), Vec::new()),
    }
}

/// The merge's checks of a journal entry (docs/pool.md §7.3): its done record is its own step's
/// and names targets; every change is to a logical name, as a content name of it, and within its
/// step's outputs (crate::agent::steps::outside: the function its job's `Out` wrote by, so an entry
/// it refuses is one an older app's job, or a damaged one, made); each raw tiles' archive it names
/// is one of its area. A shared
/// step's (what a helper's coordinator checked of its hand-off: crate::coord, `check_handoff`):
/// every change is to a file its step saves for one of the targets it did, every upload it says
/// is pending one of its saves, every one it says it checked one of those. (Not that its targets
/// are its lease's: a lead after the one that granted it may not know the lease.)
pub fn check(e: &Entry, _r: &Records) -> std::result::Result<(), String> {
    let h = &e.handoff;
    if let Some((s, ts)) = &h.done {
        if *s != e.step {
            return Err(format!("its done record is of {s}, its lease of {}", e.step));
        }
        if ts.is_empty() {
            return Err("its done record names no target".into());
        }
    }
    for (l, v) in &h.changes {
        if !store::naming::valid_logical(l) {
            return Err(format!("{l:?} isn't a logical name"));
        }
        if let Some(c) = v {
            if store::naming::parse_content_name(c).is_none_or(|n| n.logical != l) {
                return Err(format!("{c} isn't a content name of {l}"));
            }
        }
    }
    if let Some(why) = crate::agent::steps::outside(e) {
        return Err(why);
    }
    for (area, p) in &h.raw {
        if !crate::rawpack::is_area(area) || !crate::rawpack::named_for(&p.name, area) {
            return Err(format!("{} isn't an archive of {area}", p.name));
        }
    }
    if crate::agent::steps::SHARED.contains(&e.step.as_str()) {
        let did: &[(String, String)] = h.done.as_ref().map_or(&[], |d| &d.1);
        for l in h.changes.keys() {
            if !did.iter().any(|(t, _)| crate::coord::saves(&e.step, t, l)) {
                return Err(format!("{l} isn't one of the files of the targets it did"));
            }
        }
        let saved: BTreeSet<&String> = h.changes.values().flatten().collect();
        if let Some(c) = h.pending.keys().find(|c| !saved.contains(c)) {
            return Err(format!("{c} isn't one of its saves"));
        }
        if let Some(c) = h.checked.iter().find(|c| !h.pending.contains_key(*c)) {
            return Err(format!("{c} isn't one of its uploads"));
        }
    }
    Ok(())
}

/// `check`, by a lead on app `lead_app`, members' apps as `app_of` reads them (their heartbeats;
/// None: not read): an entry with a change outside its step's write-set from a member on a newer
/// app than the lead's is held (`records::HOLD`), neither merged nor refused, until the lead runs
/// an app as new (its step may write what this app's write-sets don't know of: refused, its work
/// would be done again, and refused again, until the lead updates). As the coordinator ends such a
/// member's lease rather than hold its failure against its targets (crate::coord). An entry whose
/// member's app can't be read now is held too (that says nothing of it); one from a member on the
/// lead's app, or an older one, is refused.
pub fn check_from(e: &Entry, r: &Records, lead_app: &str, app_of: &dyn Fn(&str) -> Option<String>) -> std::result::Result<(), String> {
    match check(e, r) {
        Err(why) if crate::agent::steps::outside(e).as_deref() == Some(why.as_str()) => match app_of(&e.member) {
            Some(app) if app == lead_app || crate::pool::term::app_at_least(lead_app, &app) => Err(why),
            Some(app) => Err(format!("{}{why}; its member runs app {app}, newer than this lead's ({lead_app}): held until the lead runs it", crate::pool::records::HOLD)),
            None => Err(format!("{}{why}; its member's app can't be read now", crate::pool::records::HOLD)),
        },
        other => other,
    }
}

/// An event in words, for the log.
pub fn said(e: &Event) -> String {
    match e {
        Event::Made { term, how } => format!("made term {term} ({how})"),
        Event::TookUp { term, how, handed } => format!("took up term {term} ({how}){}", if handed.is_some() { ", the coordinator's state handed over" } else { "" }),
        Event::SteppedDown { term, why } => format!("no longer leads term {term}: {why}"),
        Event::Handover { term, to, what } => format!("its handover of term {term} to {to}: {what}"),
        Event::Merged { applied, overtaken, refused, listed, .. } => format!("merged {} entr{} ({overtaken} passed over, {refused} refused){}", applied.len(), if applied.len() == 1 { "y" } else { "ies" }, if *listed { ", a listing's" } else { "" }),
        Event::Waits { what, why } => format!("waits to {what}: {why}"),
        Event::Failed { what, why } => format!("couldn't {what}: {why}"),
    }
}

/// What a process of the agent is in the pool while it's on: decided as it starts, by the terms
/// (`Run::open`), and kept for its life. A process whose member takes up a term, or stops leading
/// one, restarts into its new part once its first job's slot is free (phase 1: the lead's jobs run
/// in its own process; phase 2 moves them into its slots).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// It leads: it plans, grants jobs through its coordinator, does the lead's duties.
    Lead,
    /// A member: it asks the lead for work.
    Member,
}

/// What the driver says the agent may do now, as of its last step (`Out`'s gates).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gates {
    pub term: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leads: Option<u64>,
    /// Leading: grant jobs and plan.
    pub duties: bool,
    /// Leading: settle a handover.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub settle: bool,
    /// Leading: its records reflect the journal (a catalog may go out).
    pub caught_up: bool,
    /// Leading: it re-asserted this step (a sweep may run, caught up).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fresh: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listed_at: Option<u64>,
    /// Leading: the entries it holds, with why (`check_from`: from a member on a newer app,
    /// outside this app's write-sets), neither merged nor refused until it runs that app.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<(String, String)>,
}

impl Gates {
    fn of(o: &Out) -> Gates {
        Gates { term: o.term, leads: o.leads, duties: o.duties, settle: o.settle, caught_up: o.caught_up, fresh: o.fresh, listed_at: o.listed_at, held: o.held.clone() }
    }

    /// A catalog may be published now.
    pub fn publish(&self) -> bool {
        self.duties && self.caught_up
    }

    /// A sweep (GC) may run now: on a step that re-asserted, caught up.
    pub fn sweep(&self) -> bool {
        self.duties && self.caught_up && self.fresh
    }
}

/// The steps that are sweeps (GC: the lead's, once fresh and caught up).
pub const SWEEPS: [&str; 1] = ["gc"];
/// The steps that publish (a catalog: the lead's, once caught up).
pub const PUBLISHES: [&str; 2] = ["catalog", "catalog-held"];
/// Lease numbers at and above this are hand-offs from before the pool, drained into the journal by
/// the member that had them (term 0: before any term; below it, a helper's outbox keeps the lease
/// the lead's coordinator gave it, numbered by the clock in ms, far below).
pub const DRAINED: u64 = 1 << 62;

/// The agent's part in the pool while it's on, in this process.
pub struct Run {
    pub side: Side,
    pub role: Role,
    pub gates: Gates,
    /// Its jobs' hand-offs for the next step, with the folders or files holding them.
    pub entries: Vec<(Entry, Option<PathBuf>)>,
    /// Settling: the coordinator's state, written, for the next step.
    pub settled: Option<serde_json::Value>,
    /// Re-assert at the next step (a sweep waits for it).
    pub reassert: bool,
    /// Why it restarts once its first slot is free (its part changed, or it stopped).
    pub restart: Option<String>,
    /// What it drained already in this process (handed, not yet removed), and the drained hand-offs'
    /// last number.
    drained: BTreeSet<PathBuf>,
    drain_n: u64,
    /// When the NAS's folder of hand-offs was last drained, and whether it was found empty.
    left_taken: bool,
    /// The records (term, seq) last written to today's files; the raw tiles' archives named.
    pub today: Option<(u64, u64)>,
    pub named_raw: BTreeSet<String>,
    /// The outbox drained (once a process), and the hand-off folders' last files drained, to mark
    /// merged (handoff.rs's `<folder>.merged`) once a saved state holds them.
    outbox_drained: bool,
    marks: Vec<(PathBuf, String)>,
    /// The coordinator's state per term as last written, and the history's last event written.
    pub state_written: Option<(u64, Vec<u8>)>,
    pub history_seq: u64,
    /// The terms this process led (a take-up of the next one after them keeps its coordinator's
    /// state; one after another lead's loads that lead's).
    pub led: BTreeSet<u64>,
    /// The owner's asks for the next step (crate::agent::lead), and this Mac's conditions for its
    /// heartbeat.
    pub asks: Vec<driver::Ask>,
    pub conds: Option<Conds>,
    /// The owner's controls: their asks, where each stands, the view (crate::agent::lead).
    pub controls: super::lead::Controls,
    /// Its part changes in this process (`SLOTS`): a step that takes up a term or loses one doesn't
    /// ask for a restart; the agent changes its part (crate::agent's `change_part`).
    pub in_process: bool,
    /// What a tick needs of the agent (`Cx`), as its loop left it.
    pub cx: Cx,
    /// The agent's loop is stepping it (its step and what follows it, `in_step` until it's done):
    /// no tick meanwhile.
    pub in_step: bool,
    /// The awake time (its driver's clock, s) of its last step, and of the agent's loop's last.
    last_step: Option<u64>,
    last_full: Option<u64>,
    /// Terms a tick took up whose coordinator's part is the agent's (`took_up`: the state handed
    /// over loaded, its own host's leases from before dropped), at its loop's next step.
    pub pending_took_up: Vec<Event>,
    /// Tasks brokered by every member: when the members' brokers were last read from their
    /// heartbeats, and this Mac's last given to its own (`tend_brokers`).
    pub brokers_read: Option<Instant>,
    pub broker_told: Option<Instant>,
    /// Whether a tick holds the run, and since when: read without the lock (`Ticking`).
    pub ticking: Ticking,
    /// The members' coordinators tried (crate::pool::reachable).
    pub prober: Prober,
    /// The current term, and when this Mac first saw it (its own clock): a request that failed
    /// before then was another lead's.
    term_seen: Option<(u64, u64)>,
}

/// Since when a tick has held the run (the awake clock, `awake`), 0 while none does: set and read
/// without the run's lock, so the agent's loop tells a tick (maybe hung on the share) from the
/// pool's thread's moment's check of whether one is due.
/// Ends a `Ticking` when dropped (`Ticking::begin`).
pub struct TickingEnds(Ticking);

impl Drop for TickingEnds {
    fn drop(&mut self) {
        self.0.end();
    }
}

#[derive(Clone, Default)]
pub struct Ticking(Arc<std::sync::atomic::AtomicU64>);

static AWAKE_FROM: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

impl Ticking {
    /// This process's awake clock (s): a monotonic clock that stops while the Mac sleeps (on macOS,
    /// `Instant`), from a day on, so a time before it is never 0.
    pub fn awake() -> u64 {
        86_400 + AWAKE_FROM.get_or_init(Instant::now).elapsed().as_secs()
    }
    pub fn start(&self) {
        self.start_at(Self::awake());
    }
    pub fn start_at(&self, awake: u64) {
        self.0.store(awake.max(1), std::sync::atomic::Ordering::SeqCst);
    }
    /// `start`, ended when the guard it gives is dropped.
    pub fn begin(&self) -> TickingEnds {
        self.start();
        TickingEnds(self.clone())
    }
    pub fn end(&self) {
        self.0.store(0, std::sync::atomic::Ordering::SeqCst);
    }
    /// Since when a tick has held the run; None: none does.
    pub fn since(&self) -> Option<u64> {
        Some(self.0.load(std::sync::atomic::Ordering::SeqCst)).filter(|&t| t > 0)
    }
}

/// What the pool's own thread needs of the agent to step the driver while the agent's loop is busy
/// (`Run::tick`): set by the agent's loop at each of its steps, under the run's lock. The tick
/// changes nothing of the agent's own (its jobs, its coordinator's life, its part): those stay its
/// loop's.
#[derive(Default)]
pub struct Cx {
    /// The NAS's project folder (None: unreachable, no tick).
    pub root: Option<PathBuf>,
    /// The agent's folder.
    pub home: PathBuf,
    /// This process's coordinator (the lead's), for what the lead's step asks of it: the build
    /// page's asks, settling's state, whether it grants (`set_moving`), its term.
    pub coord: Option<Arc<crate::coord::Coordinator>>,
    /// Whether this Mac can lead now (its disk, home and power).
    pub able: bool,
    /// A duty (a catalog, a sweep) runs here, or passed its gate to start: a tick doesn't settle a
    /// handover (the agent's step stops the duty first).
    pub duty: bool,
    /// The agent's loop is starting a job (its lease from this coordinator): a tick doesn't settle
    /// until it's done.
    pub starting: bool,
    /// When this Mac last had an answer from the lead's coordinator (its clock, unix s): a member's
    /// client's (crate::coord::client::Client::answered).
    pub lead_answered: Option<u64>,
    /// This Mac's broker (docs/pool.md §8, crate::agent::brokers): the lead's coordinator, or a
    /// member's broker; None with tasks brokered by every member off. Its address over HTTPS, when
    /// `tailscale serve` proxies it.
    pub broker: Option<Arc<crate::coord::Coordinator>>,
    pub broker_https: Option<String>,
    /// Where this Mac's client reaches the lead's coordinator (its contact on the NAS), and when a
    /// request of its last reached none of them, and why: what it tries, and a failure it counts
    /// (crate::pool::reachable).
    pub lead_urls: Vec<String>,
    pub lead_failed: Option<(u64, String)>,
}

/// How a member tries another's coordinator: at its addresses, reached or not and why
/// (crate::coord::client::probe; a test's own).
pub type ProbeFn = Arc<dyn Fn(&[String]) -> std::result::Result<(), (crate::pool::reachable::Cause, String)> + Send + Sync>;

/// A round of tries' outcomes: (member, what, reached or not and why).
type Tries = Vec<(String, String, std::result::Result<(), (crate::pool::reachable::Cause, String)>)>;

/// The members' coordinators tried (`Run::tend_reach`): on a thread of its own, one round at a
/// time, every `PROBE_S`; or in the step itself (`inline`: the tests', with a probe that answers at
/// once).
pub struct Prober {
    pub probe: ProbeFn,
    pub inline: bool,
    /// The last round's start (this Mac's clock), and the round under way.
    last: Option<u64>,
    going: Option<(u64, std::thread::JoinHandle<Tries>)>,
    /// The outcomes so far.
    pub tracker: crate::pool::reachable::Tracker,
}

impl Default for Prober {
    fn default() -> Prober {
        let probe: ProbeFn = Arc::new(|u: &[String]| crate::coord::client::probe(u));
        Prober { probe, inline: false, last: None, going: None, tracker: Default::default() }
    }
}

/// A run shared by the agent's loop and the pool's own thread (`Ticker`).
pub type Shared = Arc<std::sync::Mutex<Run>>;

/// The run, locked (a thread that panicked holding it took its process down with it: the lock is
/// taken all the same).
pub fn lock(r: &Shared) -> std::sync::MutexGuard<'_, Run> {
    r.lock().unwrap_or_else(|e| e.into_inner())
}

/// The run, locked, unless another holds it now (a tick, perhaps on a share that hangs).
pub fn try_lock(r: &Shared) -> Option<std::sync::MutexGuard<'_, Run>> {
    match r.try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Clears `Run::in_step` when dropped: the agent's loop's pool step over, however it ended.
pub struct InStep(pub Shared);

impl Drop for InStep {
    fn drop(&mut self) {
        lock(&self.0).in_step = false;
    }
}

/// The pool's own thread: it steps the driver (`Run::tick`) whenever the agent's loop hasn't for a
/// while (`TICK_S`, `QUIET_S`), so the heartbeat, the mail, the owner's asks, a handover's offers,
/// answers and settling, and the view go on while the agent plans or tends its jobs. Stopped (and
/// waited for) when dropped.
pub struct Ticker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Ticker {
    pub fn start(run: Shared) -> Ticker {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let halt = stop.clone();
        let thread = std::thread::Builder::new()
            .name("pool".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let mut r = lock(&run);
                    if halt.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    if r.tick_due() {
                        // (Said outside the lock: the loop tells a tick holding the run from this
                        // check's moment, and how long a tick has held it: `Ticking`.)
                        // (Ended however the tick ends, a panic too.)
                        let _said = r.ticking.begin();
                        r.tick();
                    }
                }
            })
            .map_err(|e| eprintln!("pool: its thread didn't start ({e}): the agent's loop steps it alone"))
            .ok();
        Ticker { stop, thread }
    }

    /// Stops it: its tick under way finished, waited for `wait` at most (then left to end by itself:
    /// it steps nothing more).
    pub fn stop(mut self, wait: Duration) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let Some(t) = self.thread.take() else { return };
        let since = Instant::now();
        while !t.is_finished() && since.elapsed() < wait {
            std::thread::sleep(Duration::from_millis(100));
        }
        if t.is_finished() {
            t.join().ok();
        } else {
            eprintln!("pool: its thread's tick hasn't ended in {} s; left to end by itself", wait.as_secs());
        }
    }
}

impl Drop for Ticker {
    /// Stopped, not waited for (a tick on a share that hangs may not return): `stop` waits.
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// When the current term's lead's coordinator last answered this Mac (`answered`, its client's): only
/// an answer saying it's that term's, from that member (crate::coord::POOL_TERM, POOL_MEMBER); one
/// from another coordinator (an earlier lead's not yet stopped, an older app's) says nothing of it.
pub fn lead_answered(answered: Option<&crate::coord::client::Answered>, cur: Option<&crate::pool::term::Term>) -> Option<u64> {
    let (a, t) = (answered?, cur?);
    (a.term == Some(t.term) && a.member.as_deref() == Some(t.member.as_str())).then_some(a.at)
}

/// What the coordinator answers asks for jobs with, by the gates (None: it grants): settling a
/// handover, its term being taken up (`pending`: its state not loaded yet), its view not fresh, or
/// no longer leading.
pub fn moving(g: &Gates, pending: bool) -> Option<String> {
    match () {
        _ if g.leads.is_some() && pending => Some("taking up its term".into()),
        _ if g.leads.is_some() && g.duties => None,
        _ if g.settle => Some("settling a handover".into()),
        _ if g.leads.is_some() => Some("re-asserting its term".into()),
        _ => Some("no longer leads".into()),
    }
}

impl Run {
    pub fn new(side: Side, role: Role, gates: Gates) -> Run {
        let controls = super::lead::Controls::open(&side.dir);
        Run { side, role, gates, entries: Vec::new(), settled: None, reassert: false, restart: None, drained: BTreeSet::new(), drain_n: 0, left_taken: false, today: None, named_raw: BTreeSet::new(), outbox_drained: false, marks: Vec::new(), state_written: None, history_seq: 0, led: BTreeSet::new(), asks: Vec::new(), conds: None, controls, in_process: false, cx: Cx::default(), in_step: false, last_step: None, last_full: None, pending_took_up: Vec::new(), ticking: Ticking::default(), brokers_read: None, broker_told: None, prober: Prober::default(), term_seen: None }
    }

    /// Whether the pool's own thread steps it now: the agent's loop isn't, no step has run for
    /// `TICK_S`, the loop's last was `QUIET_S` ago or more, and the NAS was reachable then.
    pub fn tick_due(&self) -> bool {
        let now = self.side.io().awake();
        let since = |t: Option<u64>, s: u64| t.is_none_or(|t| now.saturating_sub(t) >= s);
        !self.in_step && self.side.stopped.is_none() && self.cx.root.is_some() && since(self.last_step, TICK_S) && since(self.last_full, QUIET_S)
    }

    /// The agent's loop's step is done (its after-effects too): the view written, ticks again.
    pub fn stepped_full(&mut self) {
        self.last_full = Some(self.side.io().awake());
        self.in_step = false;
        self.write_view();
    }

    /// A step of the pool's own thread (`Ticker`), the agent's loop busy: what the loop's step does
    /// that's the pool's alone. The owner's asks taken (this Mac's and the build page's); a
    /// handover settled when nothing the agent would stop runs or starts here (`Cx::duty`,
    /// `Cx::starting`: else the loop's next step stops it and settles); the driver's step, its
    /// heartbeat and mail with it; the history and the view; the coordinator told whether it grants.
    /// A term it takes up that isn't its own re-asserted is left to the loop's next step for the
    /// coordinator's part (`pending_took_up`: the state handed over, its leases), the coordinator
    /// granting nothing until then; a change of this process's part is the loop's too (its next
    /// step's `change_part`). None when there was nothing to step (no NAS).
    pub fn tick(&mut self) -> Option<Out> {
        let root = self.cx.root.clone()?;
        let home = self.cx.home.clone();
        let coord = self.cx.coord.clone();
        if let Some(co) = &coord {
            if self.gates.settle && self.settled.is_none() && !self.cx.duty && !self.cx.starting {
                co.set_moving(Some("settling a handover".into()));
                self.settled = serde_json::to_value(co.pool_state()).ok();
            }
            self.controls.page.extend(co.take_lead_asks());
        }
        super::lead::take(self, &home);
        self.tend_brokers();
        self.tend_reach();
        let out = self.step(self.cx.able);
        for e in &out.events {
            let Event::TookUp { term, handed, .. } = e else { continue };
            match &coord {
                Some(c) if self.led.contains(&(term - 1)) && handed.is_none() => c.set_term(*term),
                _ => self.pending_took_up.push(e.clone()),
            }
        }
        if let (Some(c), Some(e), true) = (&coord, out.leads, self.pending_took_up.is_empty()) {
            c.set_term(e);
        }
        super::lead::after(self, &out, &root, &home, coord.as_deref());
        self.behind();
        if let Some(co) = &coord {
            co.set_moving(moving(&self.gates, !self.pending_took_up.is_empty()));
        }
        for e in &out.events {
            if let Event::Failed { what, why } | Event::Waits { what, why } = e {
                eprintln!("pool: couldn't {what} yet: {why}");
            }
        }
        self.write_view();
        Some(out)
    }

    /// Tasks brokered by every member, in each of its steps (the loop's and the ticks'), before its
    /// heartbeat is written: this Mac's broker (`Cx::broker`) given to the heartbeat as
    /// crate::agent::brokers::due says, and every half minute the members' heartbeats read for
    /// theirs and the broker told. No broker (the switch off): none in the heartbeat.
    pub fn tend_brokers(&mut self) {
        use crate::agent::brokers;
        let Some(c) = self.cx.broker.clone() else {
            self.side.broker = None;
            (self.brokers_read, self.broker_told) = (None, None);
            return;
        };
        let now = Instant::now();
        let (waiting, out) = c.brokered();
        let own = crate::coord::brokers::Broker { urls: c.contact.urls.clone(), https: self.cx.broker_https.clone(), waiting, out };
        if brokers::due(self.side.broker.as_ref(), &own, self.broker_told.map(|t| now.saturating_duration_since(t))) {
            self.side.broker = Some(own.clone());
            self.broker_told = Some(now);
        }
        if brokers::stale(self.brokers_read, now, brokers::EVERY) {
            let side = &self.side;
            let me = side.member().clone();
            let lead = side.driver().current().lead.as_ref().map(|t| t.member.clone());
            let mut ids: Vec<String> = side.members().iter().cloned().collect();
            if let Some(l) = lead.as_ref().filter(|l| !ids.contains(l)) {
                ids.push(l.clone());
            }
            let beats: Vec<(String, Heartbeat)> = ids.into_iter().filter(|id| *id != me.id).filter_map(|id| side.heartbeat(&id).map(|h| (id, h))).collect();
            let peers = brokers::peers(&beats, lead.as_deref(), side.now(), (&me.id, &me.host, &own));
            c.set_brokers(Some(crate::coord::brokers::Brokers::new(peers, &own)));
            self.brokers_read = Some(now);
        }
    }

    /// Whom this Mac reaches (docs/pool.md §6.8, crate::pool::reachable), in each of its steps,
    /// before its heartbeat is written: the outcome of its client's requests to the lead's
    /// coordinator counted (an answer from it, as the current term's: `Cx::lead_answered`; a
    /// request that reached none of its addresses: `Cx::lead_failed`), and every `PROBE_S` a round
    /// of tries, off the loop, of the current lead's coordinator (its broker's addresses in its
    /// heartbeat, else the contact's) and the other members' brokers in touch; the round before's
    /// outcomes taken in. The heartbeat says what the tracker reports.
    pub fn tend_reach(&mut self) {
        use crate::pool::reachable::PROBE_S;
        let now = self.side.now();
        let me = self.side.member().id.clone();
        let cur = self.side.driver().current().lead.clone().filter(|t| t.member != me);
        let lead = cur.as_ref().map(|t| t.member.clone());
        let term = self.side.driver().current().term;
        if self.term_seen.is_none_or(|(t, _)| t != term) {
            self.term_seen = Some((term, now));
        }
        let seen = self.term_seen.map_or(now, |(_, at)| at);
        if let Some(l) = &lead {
            if let Some(a) = self.cx.lead_answered {
                self.prober.tracker.seen(l, "coordinator", Ok(()), a);
            }
            // (A failure since this Mac first saw its term: one before was another lead's.)
            if let Some((at, why)) = self.cx.lead_failed.as_ref().filter(|(at, _)| *at >= seen) {
                let cause = crate::pool::reachable::cause_of(why);
                self.prober.tracker.seen(l, "coordinator", Err((cause, why.clone())), *at);
            }
        }
        if let Some((at, h)) = self.prober.going.take_if(|(_, h)| h.is_finished()) {
            for (m, what, r) in h.join().unwrap_or_default() {
                self.prober.tracker.seen(&m, &what, r, at);
            }
        }
        if self.prober.going.is_none() && self.prober.last.is_none_or(|l| now.abs_diff(l) >= PROBE_S) {
            let mut targets: Vec<(String, String, Vec<String>)> = Vec::new();
            // (The members' heartbeats, read once a round: the driver's `hand_to` goes by them.)
            let mut ids: Vec<String> = self.side.members().iter().cloned().collect();
            if let Some(l) = lead.as_ref().filter(|l| !ids.contains(l)) {
                ids.push(l.clone());
            }
            // (A read failing, the NAS away: the last round's kept whole, for the budgets.)
            let read: Result<Vec<Option<Heartbeat>>> = ids.iter().map(|id| self.side.read_heartbeat(id)).collect();
            if let Ok(read) = read {
                self.side.beats = read.into_iter().flatten().filter(|h| !h.shadow).map(|h| h.pool).collect();
                self.side.beats_at = Some(now);
            }
            if let Some(l) = &lead {
                let urls = self.side.heartbeat(l).and_then(|h| h.broker).map(|b| b.urls).filter(|u| !u.is_empty()).unwrap_or_else(|| self.cx.lead_urls.clone());
                if !urls.is_empty() {
                    targets.push((l.clone(), "coordinator".into(), urls));
                }
            }
            for id in self.side.members().iter().filter(|m| **m != me && Some(*m) != lead.as_ref()) {
                let Some(h) = self.side.heartbeat(id) else { continue };
                if h.pool.beat == 0 || h.pool.out_of_touch(now) {
                    continue;
                }
                if let Some(b) = h.broker.filter(|b| !b.urls.is_empty()) {
                    targets.push((id.clone(), "broker".into(), b.urls));
                }
            }
            let keep: BTreeSet<String> = targets.iter().map(|t| t.0.clone()).collect();
            self.prober.tracker.retain(|m| keep.contains(m));
            self.prober.last = Some(now);
            let probe = self.prober.probe.clone();
            let run = move || -> Tries { targets.into_iter().map(|(m, what, urls)| { let r = probe(&urls); (m, what, r) }).collect() };
            if self.prober.inline {
                for (m, what, r) in run() {
                    self.prober.tracker.seen(&m, &what, r, now);
                }
            } else {
                match std::thread::Builder::new().name("pool-reach".into()).spawn(run) {
                    Ok(h) => self.prober.going = Some((now, h)),
                    Err(e) => eprintln!("pool: trying the members' coordinators: {e}"),
                }
            }
        }
        self.side.reach = self.prober.tracker.report(now);
        // (Its own report as it stands now, not as last read.)
        let own = self.side.reach.clone();
        if let Some(b) = self.side.beats.iter_mut().find(|b| b.member == me) {
            b.reach = own;
        }
    }

    /// Leading term 2 or later, today's records files not yet written from its records: not caught
    /// up (a catalog and GC read them). True when so.
    pub fn behind(&mut self) -> bool {
        if self.gates.leads.is_none() {
            return false;
        }
        let behind = self.side.driver().records().is_some_and(|rec| rec.term >= 2 && self.today != Some((rec.term, rec.seq)));
        if behind {
            (self.gates.caught_up, self.gates.fresh) = (false, false);
        }
        behind
    }

    /// This member as the status shows it (crate::agent::PoolView).
    pub fn pool_view(&self) -> super::PoolView {
        super::PoolView { member: self.side.member().id.clone(), role: self.role, gates: self.gates.clone(), members: self.side.members().iter().cloned().collect(), heard: self.side.heard_lately(), unacked: self.side.driver().mine().to_tell(self.gates.term).len(), restart: self.restart.clone(), lead: self.controls.view.clone() }
    }

    /// The view (`pool_view`) with when it was written (this Mac's clock), to the agent's pool
    /// folder (`VIEW`): what the controls read while the agent's status waits for its loop.
    pub fn write_view(&self) {
        let v = PoolFile { beat: self.side.now(), pool: self.pool_view() };
        let r = serde_json::to_vec(&v).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&self.side.dir.join(VIEW), &b));
        if let Err(e) = r {
            eprintln!("pool: writing its view: {e:#}");
        }
    }

    /// A process's first step, which says its part (`Role`). (The jobs an earlier process left are
    /// handed over later, `take_left`, once the agent has stopped any still running.)
    pub fn start(side: Side, _home: &Path) -> (Run, Out) {
        let mut r = Run::new(side, Role::Member, Gates::default());
        let out = r.step(false);
        r.role = if out.leads.is_some() { Role::Lead } else { Role::Member };
        r.restart = out.stop.as_ref().map(|why| format!("this process left the pool: {why}"));
        (r, out)
    }

    /// One step: what the agent gathered handed over, the gates kept; the role's change noted (it
    /// restarts into its new part).
    pub fn step(&mut self, able: bool) -> Out {
        let give = Give { entries: std::mem::take(&mut self.entries), settled: self.settled.take(), able, reassert: self.reassert, asks: std::mem::take(&mut self.asks), conds: self.conds };
        let marks = std::mem::take(&mut self.marks);
        // (The merge's checks, an entry outside its step's write-set from a member on a newer app
        // held: `check_from`. Its member's app read from its heartbeat, only for such an entry.)
        let (nas, lead_app, me) = (self.side.nas.clone(), self.side.member().app.clone(), self.side.member().id.clone());
        let app_of = |m: &str| -> Option<String> { if m == me { return Some(lead_app.clone()) } nas.read(&crate::pool::beat::path(m)).ok().flatten().and_then(|b| serde_json::from_slice::<Heartbeat>(&b).ok()).map(|h| h.pool.app) };
        let checked = |e: &Entry, r: &Records| check_from(e, r, &lead_app, &app_of);
        let out = self.side.step(give, &checked);
        self.last_step = Some(self.side.io().awake());
        // (Drained hand-offs held by the state just saved: their folders marked merged, so one whose
        // removal failed isn't drained again by a later process.)
        let kept = out.stop.is_none() && !out.events.iter().any(|e| matches!(e, Event::Failed { what: "save the member's state", .. }));
        if kept {
            for (dir, name) in marks {
                let marker = dir.with_extension("merged");
                let was = std::fs::read_to_string(&marker).unwrap_or_default();
                if name.as_str() > was.trim() {
                    if let Err(e) = crate::whole::write(&marker, name.as_bytes()) {
                        eprintln!("pool: marking {} drained: {e:#}", dir.display());
                    }
                }
            }
        } else {
            self.marks = marks;
        }
        if let Some(why) = &out.stop {
            self.restart.get_or_insert_with(|| format!("this process left the pool: {why}"));
        }
        self.gates = Gates::of(&out);
        if out.fresh {
            self.reassert = false;
        }
        if let Some(e) = out.leads {
            self.led.insert(e);
        }
        match (self.role, out.leads, self.in_process) {
            (_, _, true) => {}
            (Role::Lead, None, _) if out.stop.is_none() => {
                self.restart.get_or_insert_with(|| "it no longer leads".into());
            }
            (Role::Member, Some(e), _) => {
                self.restart.get_or_insert_with(|| format!("it leads term {e}"));
            }
            _ => {}
        }
        for e in &out.events {
            eprintln!("pool: {}", said(e));
            // (The hand-offs merged, by their entries' keys, in the build's history: publishing's
            // times, crate::pubtimes.)
            if let (Event::Merged { done, .. }, Some(co)) = (e, &self.cx.coord) {
                if !done.is_empty() {
                    co.note(crate::pubtimes::merged_event(done));
                }
            }
        }
        out
    }

    /// The jobs an earlier process left in the agent's folder `home` (`left_jobs`), handed to the
    /// next step: once a process, after the agent stopped those still running.
    pub fn take_left(&mut self, home: &Path) {
        if std::mem::replace(&mut self.left_taken, true) {
            return;
        }
        let me = self.side.member().id.clone();
        self.entries.extend(left_jobs(home, &me).into_iter().map(|(e, d)| (e, Some(d))));
    }

    /// The next number for a hand-off drained from before the pool (`DRAINED` on, by the clock).
    fn next_drained(&mut self) -> journal::LeaseId {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.drain_n = (self.drain_n + 1).max(ms);
        journal::LeaseId { term: 0, n: DRAINED + self.drain_n }
    }

    /// The hand-offs from before the pool this member holds, drained into its journal entries
    /// (§12, seeding and draining): a lead's coordinator's journal on its disk (`journal`: the
    /// helpers' hand-offs it took and didn't merge), and any member's outbox (`outbox`: a helper's
    /// jobs not handed back). Each is removed once a saved state holding its entry is on disk.
    pub fn drain(&mut self, journal: Option<&Path>, outbox: &Path) {
        let me = self.side.member().id.clone();
        let mut found: Vec<(PathBuf, Handoff)> = Vec::new();
        if let Some(j) = journal {
            match crate::handoff::waiting_in(j) {
                Ok(hs) => found.extend(hs),
                Err(e) => eprintln!("pool: draining {}: {e:#}", j.display()),
            }
        }
        for (p, h) in found {
            if self.drained.contains(&p) {
                continue;
            }
            let lease = self.next_drained();
            let step = h.done.as_ref().map_or_else(|| "handoff".to_string(), |d| d.0.clone());
            self.drained.insert(p.clone());
            if let (Some(d), Some(n)) = (p.parent(), p.file_name().and_then(|n| n.to_str())) {
                self.marks.push((d.to_path_buf(), n.to_string()));
            }
            self.entries.push((Entry { member: me.clone(), lease, step, handoff: h, at: crate::agent::jobs::now_s() }, Some(p)));
        }
        // (A helper's outbox from before the pool: once a process. Its tasks' folders stay with the
        // agent's sending.)
        if !std::mem::replace(&mut self.outbox_drained, true) {
            for (e, dir) in outbox_entries(outbox, &me) {
                if self.drained.insert(dir.clone()) {
                    self.entries.push((e, Some(dir)));
                }
            }
        }
    }
}

/// A term taken up (`Event::TookUp`), as the lead's coordinator takes it (§6.2, §7.5): the leases it
/// grants from now in that term; the state handed over with it (a handover's), else the newest a
/// term before has (a takeover's, or its own before a restart), loaded; its own host's leases from
/// before dropped (its jobs ended with the process that ran them), but those of the jobs this
/// process runs (`keep`: a member that took the lead in its process, its job going on).
///
/// `events`: a step's, with those a tick left it (`Run::pending_took_up`); `leads`: the term the
/// last step says this member leads.
pub fn took_up(run: &mut Run, coord: Option<&crate::coord::Coordinator>, events: &[Event], leads: Option<u64>, host: &str, keep: &[u64]) {
    let Some(c) = coord else { return };
    for e in events {
        let Event::TookUp { term, handed, .. } = e else { continue };
        c.set_term(*term);
        // (A take-up after a term this process led keeps what it has: it's newer than the files.)
        if run.led.contains(&(term - 1)) && handed.is_none() {
            continue;
        }
        // (The newest a term before has: one whose lead had no coordinator, a member that took it
        // over before restarting into its lead, wrote none.)
        let state: Option<crate::coord::PoolState> = match handed {
            Some(v) => serde_json::from_value(v.clone()).ok(),
            None => (1..*term).rev().find_map(|e| run.side.nas().read(&state_path(e)).ok().flatten().and_then(|b| serde_json::from_slice(&b).ok())),
        };
        if let Some(st) = state {
            match c.load_pool_state(&st) {
                Ok(n) => eprintln!("pool: term {term}'s coordinator took up {n} lease{} {}", if n == 1 { "" } else { "s" }, if handed.is_some() { "handed over" } else { "from the term before" }),
                Err(e) => eprintln!("pool: the coordinator's state for term {term}: {e:#}"),
            }
            for l in c.drop_workers(&[host.to_string(), crate::agent::second_worker(host)], keep) {
                eprintln!("pool: {}'s lease ended with the process before this one", l.what());
            }
        }
    }
    if let Some(e) = leads {
        c.set_term(e);
    }
}

/// Where a job of lease `lease` keeps its hand-off while it runs (`SCENIC_HANDOFF`), its work and
/// done marks: `<home>/pool/jobs/<term>-<n>/`.
pub fn job_dir(home: &Path, lease: journal::LeaseId) -> PathBuf {
    home.join("pool/jobs").join(lease.to_string())
}

/// The journal entry of the job whose folder is `dir` (`job_dir`): its saves, in the order written,
/// and the targets it finished (`done`; none: it finished none). A shared step's changes only to
/// the files of the targets it finished (one it was on when it stopped may have saved part of its
/// own); another step's (the OSM pass's stages) all of them. None when it hands off nothing; an
/// error when one of its saves is damaged.
pub fn entry_of(dir: &Path, member: &str, lease: journal::LeaseId, step: &str, done: &[(String, String)], at: u64) -> Result<Option<Entry>> {
    let saves = crate::handoff::written_in(dir)?.context("one of its saves is damaged")?;
    let mut h = Handoff::default();
    for x in saves {
        h.absorb(x);
    }
    if crate::agent::steps::SHARED.contains(&step) {
        h.changes.retain(|l, _| done.iter().any(|(t, _)| crate::coord::saves(step, t, l)));
        let kept: BTreeSet<String> = h.changes.values().flatten().cloned().collect();
        h.pending.retain(|c, _| kept.contains(c));
        let pending = h.pending.clone();
        h.checked.retain(|c| pending.contains_key(c));
    }
    h.done = (!done.is_empty()).then(|| (step.to_string(), done.to_vec()));
    if h.done.is_none() && h.changes.is_empty() && h.pending.is_empty() && h.checked.is_empty() && h.raw.is_empty() {
        return Ok(None);
    }
    Ok(Some(Entry { member: member.to_string(), lease, step: step.to_string(), handoff: h, at }))
}

/// A job's folder as it's kept (`work.json`): its step, targets and lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobKept {
    pub step: String,
    pub targets: Vec<(String, String)>,
    pub lease: journal::LeaseId,
}

/// The jobs an earlier process of this agent left in `<home>/pool/jobs/` (it stopped or crashed
/// while they ran): each one's hand-off, what it noted done (`done.txt`) as finished, with its
/// folder; a folder of no job, or whose saves are damaged, removed.
pub fn left_jobs(home: &Path, member: &str) -> Vec<(Entry, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(home.join("pool/jobs")) else { return Vec::new() };
    let mut out = Vec::new();
    for d in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let kept: Option<JobKept> = std::fs::read(d.join("work.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let Some(k) = kept else {
            std::fs::remove_dir_all(&d).ok();
            continue;
        };
        let names = crate::control::read_done(&d.join("done.txt"), &k.step);
        let done: Vec<(String, String)> = k.targets.iter().filter(|(t, _)| names.contains(t)).cloned().collect();
        match entry_of(&d, member, k.lease, &k.step, &done, crate::agent::jobs::now_s()) {
            Ok(Some(e)) => out.push((e, d)),
            Ok(None) => {
                std::fs::remove_dir_all(&d).ok();
            }
            Err(e) => {
                eprintln!("pool: {}'s hand-off can't be read ({e:#}); its work is done again", d.display());
                std::fs::remove_dir_all(&d).ok();
            }
        }
    }
    out
}

/// A helper's outbox from before the pool (`outbox/<lease>/`): each leased job's hand-off as the
/// helper would have sent it (its saves and done record, or what it noted done when it stopped),
/// as an entry of this member under its lease from the lead's coordinator (term 0: before any
/// term). A task's folder (its outputs went to its broker) and a job that handed off nothing are
/// left to the outbox's own sending, or nothing.
fn outbox_entries(outbox: &Path, member: &str) -> Vec<(Entry, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(outbox) else { return Vec::new() };
    let mut out = Vec::new();
    for d in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Some(lease) = d.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok()) else { continue };
        let Some(w) = std::fs::read(d.join("work.json")).ok().and_then(|b| serde_json::from_slice::<crate::agent::build::Work>(&b).ok()) else { continue };
        let result: Option<serde_json::Value> = std::fs::read(d.join("result.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        // (Still running here: not yet. A job the agent of before the pool ran ends with it.)
        let done: Vec<(String, String)> = match result.as_ref().filter(|r| r["ok"].as_bool() == Some(true)) {
            Some(r) => serde_json::from_value::<Option<(String, Vec<(String, String)>)>>(r["done"].clone()).ok().flatten().map(|d| d.1).unwrap_or_default(),
            None => {
                let names = crate::control::read_done(&d.join("done.txt"), &w.step);
                w.targets.iter().filter(|(t, _)| names.contains(t)).cloned().collect()
            }
        };
        match entry_of(&d, member, journal::LeaseId { term: 0, n: lease }, &w.step, &done, crate::agent::jobs::now_s()) {
            Ok(Some(e)) => out.push((e, d)),
            // (Nothing to hand off, or its saves damaged: its work is done again; the folder goes,
            // or this member would never ask for work.)
            Ok(None) | Err(_) => {
                std::fs::remove_dir_all(&d).ok();
            }
        }
    }
    out
}

/// The coordinator's state of term `term` on the NAS (§6.2, §7.5).
pub fn state_path(term: u64) -> String {
    format!("state/coord/term/{term}/state.json")
}

/// The pool's copy of the workers' token (§8: the same on every member).
pub const TOKEN: &str = "state/coord/token";

/// The workers' token made the pool's (§12, seeding): this Mac's (the lead's, as the pool is
/// switched on) copied to the NAS when it has none (create-new: the first lead's stays), else the
/// NAS's copied here before its coordinator starts, so the Macs' agents keep working with this
/// one.
pub fn seed(nas: &dyn Nas, coord: &Path) -> Result<()> {
    std::fs::create_dir_all(coord)?;
    for (theirs, ours) in [(TOKEN, "workers-token")] {
        let here = coord.join(ours);
        match nas.read(theirs)? {
            Some(b) if !b.is_empty() => {
                if std::fs::read(&here).ok().as_deref() != Some(&b[..]) {
                    crate::whole::write(&here, &b)?;
                }
            }
            _ => {
                if let Ok(b) = std::fs::read(&here) {
                    if let Created::Unwritten(e) = nas.create_new(theirs, &b)? {
                        nas.write_whole(theirs, &b).context(e)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Today's three files written from `r` (the records of a term after the first: term 1's own saves
/// write them), for the readers that read them: the jobs, the map's server, the catalogs, and the
/// lead's own planning (§6.2's readers of the snapshot itself: planned).
pub fn write_today(nas: &dyn Nas, r: &Records) -> Result<()> {
    nas.write_whole("state/build/manifest.json", &serde_json::to_vec_pretty(&r.manifest)?)?;
    nas.write_whole("state/build/pending.json", &serde_json::to_vec_pretty(&r.pending)?)?;
    nas.write_whole("state/build/jobs.json", &serde_json::to_vec_pretty(&r.keys)?)
}

/// The history's events appended to this member's file of their day,
/// `state/coord/history/<day>/<member>.jsonl` (§7.5: one file per writer; merged for reading by
/// time, writer and number). Through the share's path (an append: its one writer).
pub fn append_history(root: &Path, member: &str, events: &[crate::coord::history::Event]) -> Result<()> {
    let mut by_day: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for e in events {
        let Some(day) = journal::day(e.t) else { continue };
        let b = by_day.entry(day).or_default();
        b.extend(serde_json::to_vec(e)?);
        b.push(b'\n');
    }
    for (day, b) in by_day {
        let p = root.join("state/coord/history").join(day).join(format!("{member}.jsonl"));
        std::fs::create_dir_all(p.parent().unwrap())?;
        use std::io::Write;
        std::fs::OpenOptions::new().create(true).append(true).open(&p).and_then(|mut f| f.write_all(&b)).with_context(|| format!("append to {}", p.display()))?;
    }
    Ok(())
}

/// The agents' statuses on the NAS: the lead's (`state/status.json`) and the members'
/// (`state/helpers/<host>.json`), by file name.
fn statuses(root: &Path) -> Vec<(String, super::Status)> {
    let mut out = Vec::new();
    let read = |p: &Path| std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<super::Status>(&b).ok());
    if let Some(s) = read(&root.join("state/status.json")) {
        out.push(("state/status.json".to_string(), s));
    }
    if let Ok(rd) = std::fs::read_dir(root.join("state/helpers")) {
        let mut ps: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        ps.sort();
        for p in ps {
            if let Some(s) = read(&p) {
                out.push((format!("state/helpers/{}", p.file_name().unwrap().to_string_lossy()), s));
            }
        }
    }
    out
}

/// `scenic pool status`: the terms, the agents as their statuses say.
pub fn status(root: &Path) -> String {
    let mut out = Vec::new();
    let nas = Share::new(root);
    match crate::pool::term::current(&nas) {
        Ok(c) if c.term > 0 => out.push(match &c.lead {
            Some(t) => format!("term {}: {} ({}), app {}, since {} ({})", t.term, t.host, t.member, t.app, t.since, t.how),
            None => format!("term {}: can't be read whole", c.term),
        }),
        Ok(_) => out.push("no terms".to_string()),
        Err(e) => out.push(format!("the terms: {e:#}")),
    }
    let now = crate::agent::jobs::now_s();
    for (f, st) in statuses(root) {
        let p = st.pool.as_ref().map_or("not in the pool".to_string(), |p| format!("{} {}, term {}, leads {:?}, duties {}, caught up {}, unacked {}{}{}", serde_json::to_value(p.role).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(), p.member, p.gates.term, p.gates.leads, p.gates.duties, p.gates.caught_up, p.unacked, p.gates.held.first().map(|(k, why)| format!(", {} entr{} held until it runs a newer app (the first: {k}: {why})", p.gates.held.len(), if p.gates.held.len() == 1 { "y" } else { "ies" })).unwrap_or_default(), p.restart.as_ref().map(|r| format!(", restarting: {r}")).unwrap_or_default()));
        out.push(format!("{} ({f}, app {}, beat {} s ago, job {}): {p}", st.host, st.app, now.saturating_sub(st.beat), st.job.as_ref().map_or("none", |j| j.id.as_str())));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::driver::Ask;
    use crate::pool::journal::LeaseId;

    /// A NAS folder as the pool begins: today's records, and this Mac named their writer.
    fn root(d: &Path) -> PathBuf {
        let r = d.join("nas");
        std::fs::create_dir_all(r.join("state/build")).unwrap();
        std::fs::create_dir_all(r.join("catalog")).unwrap();
        std::fs::write(r.join("state/build/manifest.json"), r#"{"base/6-1-1": "base/6-1-1.1111111111111111.base"}"#).unwrap();
        std::fs::write(r.join("state/build/jobs.json"), r#"{"unit": {"6/1/1": "k1"}}"#).unwrap();
        std::fs::write(r.join("state/build/pending.json"), "{}").unwrap();
        r
    }

    fn side(root: &Path, home: &Path) -> Side {
        let nas: SharedNas = Arc::new(Share::new(root));
        // (Its clocks virtual: time passes as the test says, `ahead`, however slowly a busy Mac
        // steps it.)
        let mut s = Side::open(home, &home.join("pool"), home.parent().unwrap(), "development", nas).unwrap().expect("its lock");
        s.set_clock(store::clock::Virtual::new());
        s
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Steps `s` until `done` says so (each step's listings made off its loop meanwhile, as long
    /// as a busy Mac takes: five minutes is a watchdog, not a measure).
    fn until(s: &mut Side, give: impl Fn() -> Give, done: impl Fn(&Out) -> bool) -> Out {
        let end = Instant::now() + WATCHDOG;
        loop {
            let out = s.step(give(), &any);
            if done(&out) {
                return out;
            }
            assert!(Instant::now() < end, "not done in {WATCHDOG:?} of steps");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    const WATCHDOG: Duration = Duration::from_secs(300);

    const ME: &str = "m-0000000000000001";
    const B: &str = "m-0000000000000002";
    const PH: &str = "m-00000000000000ff";

    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_member_missed_by_the_listings_is_forgotten_after_the_grace() {
        let t0 = Instant::now();
        let after = FORGET_AFTER;
        let (mut members, mut missed) = (set(&[ME, B, PH]), BTreeMap::new());
        // (Its file gone: missed from the first listing, kept until the grace has passed.)
        assert!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, true, t0, after).is_empty());
        assert!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, true, t0 + after - Duration::from_secs(1), after).is_empty());
        assert!(members.contains(PH));
        assert_eq!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, true, t0 + after, after), vec![PH.to_string()]);
        assert_eq!(members, set(&[ME, B]));
        assert!(missed.is_empty());
    }

    #[test]
    fn a_member_found_again_is_no_longer_missed() {
        let t0 = Instant::now();
        let after = FORGET_AFTER;
        let (mut members, mut missed) = (set(&[ME, B]), BTreeMap::new());
        forget(&mut members, &mut missed, &set(&[ME]), ME, true, t0, after);
        // (Its file back, as a member restarting rewrites it: the miss starts over.)
        forget(&mut members, &mut missed, &set(&[ME, B]), ME, true, t0 + after / 2, after);
        assert!(forget(&mut members, &mut missed, &set(&[ME]), ME, true, t0 + after + Duration::from_secs(1), after).is_empty());
        assert!(members.contains(B));
        // (A Mac asleep for hours, its file still there: never missed.)
        let (mut members, mut missed) = (set(&[ME, B]), BTreeMap::new());
        for h in 0..24 {
            assert!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, true, t0 + Duration::from_secs(3600 * h), after).is_empty());
        }
        assert!(missed.is_empty());
    }

    #[test]
    fn a_listing_without_its_own_heartbeat_forgets_nobody() {
        let t0 = Instant::now();
        let after = FORGET_AFTER;
        let (mut members, mut missed) = (set(&[ME, B]), BTreeMap::new());
        // (The folder not there, the NAS away: an empty listing, hours of it.)
        for h in 0..10 {
            assert!(forget(&mut members, &mut missed, &set(&[]), ME, true, t0 + Duration::from_secs(3600 * h), after).is_empty());
        }
        // (Before it has written its own heartbeat, likewise.)
        assert!(forget(&mut members, &mut missed, &set(&[ME]), ME, false, t0 + Duration::from_secs(36000), after).is_empty());
        assert_eq!(members, set(&[ME, B]));
        assert!(missed.is_empty());
    }

    #[test]
    fn a_forgotten_member_is_known_again_only_from_its_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("home");
        let mut s = side(&r, &home);
        let me = s.member().id.clone();
        s.know(PH);
        s.post(PH, Msg::Tell(vec!["k".into()]));
        assert!(s.mail.sent.contains_key(PH));
        s.beat = Some((Heartbeat::default(), s.clock.now(), 0));
        let t0 = s.clock.now();
        let files = |ids: &[&str]| ids.iter().map(|i| format!("{i}.json")).collect::<Vec<_>>();
        s.take_members(&files(&[&me]), t0);
        s.take_members(&files(&[&me]), t0 + FORGET_AFTER);
        assert!(!s.members().contains(PH));
        // (Another Mac's message or term naming it, an older app re-advertising it: not again.)
        s.heard_of(PH);
        assert!(!s.members().contains(PH));
        let saved: BTreeSet<String> = serde_json::from_slice(&std::fs::read(home.join("pool/members.json")).unwrap()).unwrap();
        assert!(!saved.contains(PH));
        // (Its queued mail dropped, nothing posted to it, and so after a restart.)
        assert!(!s.mail.sent.contains_key(PH));
        s.post(PH, Msg::Tell(vec!["k".into()]));
        assert!(!s.mail.sent.contains_key(PH));
        drop(s);
        let mut s = side(&r, &home);
        assert!(!s.members().contains(PH));
        s.heard_of(PH);
        assert!(!s.members().contains(PH));
        // (Its heartbeat found again: known.)
        s.beat = Some((Heartbeat::default(), s.clock.now(), 0));
        s.take_members(&files(&[&me, PH]), t0 + FORGET_AFTER * 2);
        assert!(s.members().contains(PH));
        let gone: BTreeSet<String> = serde_json::from_slice(&std::fs::read(home.join("pool/forgotten.json")).unwrap()).unwrap();
        assert!(gone.is_empty());
    }

    #[test]
    fn the_budgets_count_the_members_heard_lately() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let mut s = side(&r, &d.path().join("home"));
        for m in [PH, B] {
            s.know(m);
        }
        assert_eq!(s.heard_lately(), 1);
        let now = 1_000_000;
        let beat = |m: &str, at: u64| Beat { member: m.into(), beat: at, ..Default::default() };
        s.beats = vec![beat(B, now - 30), beat(PH, now - crate::pool::beat::OUT_OF_TOUCH_S - 1)];
        s.beats_at = Some(now);
        assert_eq!(s.heard_lately(), 2);
        // (The run busy: the view's count, not its members.)
        let v = super::super::PoolView { member: s.member().id.clone(), role: Role::Member, gates: Gates::default(), members: vec![s.member().id.clone(), B.into(), PH.into()], heard: s.heard_lately(), unacked: 0, restart: None, lead: None };
        assert_eq!(super::super::PoolView::heard_of(Some(&v)), 2);
        assert_eq!(super::super::PoolView::heard_of(None), 1);
    }

    fn unit_entry(member: &str, lease: LeaseId, target: &str, key: &str) -> Entry {
        let l = format!("base/{}", target.replace('/', "-"));
        let h = Handoff { changes: [(l.clone(), Some(format!("{l}.2222222222222222.base")))].into(), done: Some(("unit".into(), vec![(target.into(), key.into())])), ..Default::default() };
        Entry { member: member.into(), lease, step: "unit".into(), handoff: h, at: crate::agent::jobs::now_s() }
    }

    #[test]
    fn the_members_are_listed_by_the_out_of_touch_threshold() {
        // (Derived, not a figure of its own: no view of a heartbeat lags its going out of touch by
        // more than a fifth of it, on a lead or a member.)
        assert_eq!(MEMBERS_EVERY.as_secs() * 5, crate::pool::beat::OUT_OF_TOUCH_S);
    }

    #[test]
    fn heartbeats_of_apps_before_and_after_the_controls_read_each_other() {
        // The heartbeat as the apps before phase 3 have it (1fa03d8), and as they write it.
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Before {
            #[serde(flatten)]
            pool: Beat,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            members: Vec<String>,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            shadow: bool,
        }
        let pool = Beat { member: "m-000000000000000a".into(), host: "Mac-mini".into(), app: "20261008-1500-1fa03d8".into(), beat: 1000, leads: Some(3), ..Default::default() };
        let new = Heartbeat { pool: pool.clone(), members: vec!["m-000000000000000b".into()], shadow: false, conds: Some(Conds { home: true, ac: false, battery: Some(54), able: false }), broker: None };
        let b = serde_json::to_vec(&new).unwrap();
        // An older app reads the newer's: its fields, the conditions passed over.
        let old: Before = serde_json::from_slice(&b).unwrap();
        assert_eq!((old.pool.clone(), old.members.clone()), (pool.clone(), new.members.clone()));
        assert_eq!(Beat::read(&{ let m = crate::pool::nas::Mem::default(); m.write_whole(&crate::pool::beat::path(&pool.member), &b).unwrap(); m }, &pool.member).unwrap(), Some(pool.clone()));
        // A newer app reads the older's: no conditions ("conditions unknown" in the views).
        let back: Heartbeat = serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(back, Heartbeat { conds: None, ..new.clone() });
        assert!(crate::agent::lead::state_of(&back, 1000, None).starts_with("conditions unknown"));
        // And its own, back as written.
        assert_eq!(serde_json::from_slice::<Heartbeat>(&b).unwrap(), new);
    }

    #[test]
    fn a_member_steps_saves_its_state_beats_and_lists_off_its_loop() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut a = side(&r, &home);
        let id = a.member().id.clone();
        // Its first step: term 1, its own (this Mac is the writer), taken up from today's records.
        let out = a.step(Give { able: true, ..Default::default() }, &any);
        assert_eq!((out.term, out.leads, out.duties, out.caught_up), (1, Some(1), true, false));
        assert!(out.list.as_ref().is_some_and(|l| l.since.is_none()), "a take-up lists every day");
        // Its saved state on disk before anything else: naming it, the term it led.
        let saved: Saved = serde_json::from_slice(&std::fs::read(home.join("pool/saved.json")).unwrap()).unwrap();
        assert_eq!((saved.member.as_str(), saved.led), (id.as_str(), 1));
        // Its heartbeat: the driver's fields and the members it knows.
        let hb: Heartbeat = serde_json::from_slice(&std::fs::read(r.join(crate::pool::beat::path(&id))).unwrap()).unwrap();
        assert_eq!((hb.pool.leads, hb.members.clone()), (Some(1), vec![id.clone()]));
        assert!(hb.pool.beat > 0 && !hb.shadow);
        // The listing is made on a thread of its own and handed back: caught up.
        let out = until(&mut a, || Give { able: true, ..Default::default() }, |o| o.caught_up);
        assert!(out.listed_at.is_some() && a.listings_out() <= 1);
    }

    #[test]
    fn a_member_whose_lock_another_process_holds_runs_no_driver_or_stops() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut a = side(&r, &home);
        let nas: SharedNas = Arc::new(Share::new(&r));
        assert!(Side::open(&home, &home.join("pool"), home.parent().unwrap(), "development", nas).unwrap().is_none(), "a second process of the member runs none");
        a.step(Give { able: true, ..Default::default() }, &any);
        // Its lock file removed and taken by another process: it stops, its state saved, its
        // hand-offs kept in it, no heartbeat written.
        let lock = home.parent().unwrap().join(format!("pool-{}.lock", a.member().id));
        std::fs::remove_file(&lock).unwrap();
        let _other = MemberLock::take(home.parent().unwrap(), &a.member().id).unwrap().unwrap();
        let beat = r.join(crate::pool::beat::path(&a.member().id));
        std::fs::remove_file(&beat).unwrap();
        let e = unit_entry(&a.member().id, LeaseId { term: 1, n: 7 }, "6/1/2", "k2");
        let out = a.step(Give { entries: vec![(e, None)], able: true, ..Default::default() }, &any);
        assert!(out.stop.is_some() && a.stopped.is_some());
        let saved: Saved = serde_json::from_slice(&std::fs::read(home.join("pool/saved.json")).unwrap()).unwrap();
        assert_eq!(saved.mine.unwritten().count(), 1, "its hand-off kept for the next process");
        assert!(!beat.exists());
        assert!(a.step(Give::default(), &any).stop.is_some(), "it steps no more");
    }

    #[test]
    fn a_saved_state_of_an_earlier_time_the_pool_was_on_counts_for_nothing() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut a = side(&r, &home);
        until(&mut a, || Give { able: true, ..Default::default() }, |o| o.leads == Some(1));
        drop(a);
        // Switched off, its terms moved aside: the state of term 1 would hold this member above them.
        std::fs::rename(r.join("state/build"), d.path().join("aside")).unwrap();
        std::fs::create_dir_all(r.join("state/build")).unwrap();
        for f in ["manifest.json", "jobs.json", "pending.json"] {
            std::fs::copy(d.path().join("aside").join(f), r.join("state/build").join(f)).unwrap();
        }
        let mut a = side(&r, &home);
        assert_eq!(a.driver().saved().term, 0);
        assert!(home.join("pool/saved.earlier.json").exists());
        let out = a.step(Give { able: true, ..Default::default() }, &any);
        assert_eq!((out.term, out.leads), (1, Some(1)), "term 1 afresh");
    }

    #[test]
    fn a_failed_listing_is_made_again_and_handed_back() {
        // (A NAS whose first listing of the journal fails.)
        struct Flaky(Share, std::sync::atomic::AtomicU32);
        impl Nas for Flaky {
            fn create_new(&self, p: &str, b: &[u8]) -> Result<Created> {
                self.0.create_new(p, b)
            }
            fn write_whole(&self, p: &str, b: &[u8]) -> Result<()> {
                self.0.write_whole(p, b)
            }
            fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
                self.0.read(p)
            }
            fn exists(&self, p: &str) -> Result<bool> {
                self.0.exists(p)
            }
            fn list(&self, dir: &str) -> Result<Vec<String>> {
                if dir == journal::DIR && self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    anyhow::bail!("the share went away");
                }
                self.0.list(dir)
            }
            fn remove(&self, p: &str) -> Result<()> {
                self.0.remove(p)
            }
        }
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let nas = Arc::new(Flaky(Share::new(&r), Default::default()));
        let mut a = Side::open(&home, &home.join("pool"), home.parent().unwrap(), "development", nas.clone()).unwrap().unwrap();
        a.set_clock(store::clock::Virtual::new());
        a.step(Give { able: true, ..Default::default() }, &any);
        // (Stepped until the failed listing, made off the loop, is back.)
        let end = Instant::now() + WATCHDOG;
        let out = loop {
            let out = a.step(Give { able: true, ..Default::default() }, &any);
            if a.failed_at.is_some() {
                break out;
            }
            assert!(Instant::now() < end, "the failing listing never came back");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!out.caught_up, "the failed one isn't handed back");
        a.failed_at = a.failed_at.map(|t| t - AGAIN);
        let out = until(&mut a, || Give { able: true, ..Default::default() }, |o| o.caught_up);
        assert!(out.listed_at.is_some());
        assert!(nas.1.load(std::sync::atomic::Ordering::SeqCst) >= 2, "made again");
    }

    #[test]
    fn two_members_mail_entries_acknowledge_and_hand_the_lead_over() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let (mut a, mut b) = (side(&r, &d.path().join("a/agent")), side(&r, &d.path().join("b/agent")));
        let (ia, ib) = (a.member().id.clone(), b.member().id.clone());
        let lead = until(&mut a, || Give { able: true, ..Default::default() }, |o| o.caught_up);
        assert_eq!(lead.leads, Some(1));
        // B's job's hand-off, in its folder: written to the journal, told to A by mail; the folder
        // removed once B's saved state holds it.
        let folder = d.path().join("b/agent/pool/jobs/1-5");
        std::fs::create_dir_all(&folder).unwrap();
        let e = unit_entry(&ib, LeaseId { term: 1, n: 5 }, "6/1/2", "k2");
        let key = e.key().unwrap();
        let out = b.step(Give { entries: vec![(e, Some(folder.clone()))], able: true, ..Default::default() }, &any);
        assert_eq!(out.leads, None);
        assert!(out.send.iter().any(|(to, m)| *to == ia && matches!(m, Msg::Tell(k) if k.contains(&key))), "{:?}", out.send);
        assert!(!folder.exists() && r.join(journal::path(&key)).exists());
        assert!(r.join(mail_path(&ia, &ib)).exists());
        // A knows B (its heartbeats listed, or here: told), reads its mail, merges, acknowledges.
        a.know(&ib);
        let out = until(&mut a, || Give { able: true, ..Default::default() }, |o| o.events.iter().any(|e| matches!(e, Event::Merged { applied, .. } if applied.contains(&key))));
        assert!(out.send.iter().any(|(to, m)| *to == ib && matches!(m, Msg::Ack { keys, .. } if keys.contains(&key))));
        assert_eq!(a.driver().records().unwrap().keys.recorded("unit", "6/1/2"), Some("k2"));
        b.step(Give { able: true, ..Default::default() }, &any);
        assert!(b.driver().mine().to_tell(1).is_empty(), "acknowledged");
        // The owner hands the lead to B: offered; B ready; A settles, the coordinator's state
        // handed back; passed; B takes term 2 up with that state.
        let out = a.step(Give { asks: vec![Ask::HandTo(ib.clone())], able: true, ..Default::default() }, &any);
        assert!(out.events.iter().any(|e| matches!(e, Event::Handover { what: "offered", .. })), "{:?}", out.events);
        b.step(Give { able: true, ..Default::default() }, &any);
        let out = until(&mut a, || Give { able: true, ..Default::default() }, |o| o.settle);
        assert!(!out.duties, "settling, it grants nothing");
        let state = serde_json::json!({ "leases": { "next": 9, "leases": [] }, "costs": {}, "failed": [], "pause": null, "pause_at": 0 });
        let out = until(&mut a, || Give { settled: Some(state.clone()), able: true, ..Default::default() }, |o| o.events.iter().any(|e| matches!(e, Event::Handover { what: "passed", .. })));
        assert!(out.leads.is_none() && out.send.iter().any(|(to, m)| *to == ib && matches!(m, Msg::Passed(t) if t.term == 2)));
        let out = until(&mut b, || Give { able: true, ..Default::default() }, |o| o.leads == Some(2));
        let handed = out.events.iter().find_map(|e| match e {
            Event::TookUp { term: 2, handed, .. } => Some(handed.clone()),
            _ => None,
        });
        assert_eq!(handed, Some(Some(state)), "{:?}", out.events);
        let out = until(&mut a, || Give { able: true, ..Default::default() }, |o| o.events.iter().any(|e| matches!(e, Event::Handover { what: "over", .. })));
        assert_eq!(out.term, 2);
    }

    #[test]
    fn a_jobs_entry_is_its_saves_and_the_targets_it_finished() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("1-3");
        let save = |l: &str, c: &str| Handoff { changes: [(l.to_string(), Some(c.to_string()))].into(), pending: [(c.to_string(), "aa".to_string())].into(), ..Default::default() };
        crate::handoff::write(&dir, &save("base/6-1-1", "base/6-1-1.1111111111111111.base")).unwrap();
        crate::handoff::write(&dir, &save("base/6-1-2", "base/6-1-2.2222222222222222.base")).unwrap();
        let lease = LeaseId { term: 1, n: 3 };
        // A shared step's: only the files of the targets it finished.
        let e = entry_of(&dir, "m-000000000000000a", lease, "unit", &[("6/1/1".into(), "k")].map(|(a, b): (String, &str)| (a, b.to_string())), 100_000).unwrap().unwrap();
        assert_eq!(e.handoff.changes.keys().collect::<Vec<_>>(), ["base/6-1-1"]);
        assert_eq!(e.handoff.pending.len(), 1);
        assert_eq!(e.handoff.done, Some(("unit".into(), vec![("6/1/1".into(), "k".into())])));
        // Another step's (the OSM pass's stages): all of them, done or not.
        let e = entry_of(&dir, "m-000000000000000a", lease, "osm-pass", &[], 100_000).unwrap().unwrap();
        assert_eq!((e.handoff.changes.len(), e.handoff.done.clone()), (2, None));
        // Nothing done of a shared step: nothing to hand off.
        assert!(entry_of(&dir, "m-000000000000000a", lease, "unit", &[], 100_000).unwrap().is_none());
        // A damaged save: an error (its work is done again).
        std::fs::write(dir.join("00000000000000000009-1.json"), b"{").unwrap();
        assert!(entry_of(&dir, "m-000000000000000a", lease, "osm-pass", &[], 100_000).is_err());
    }

    #[test]
    fn the_hand_offs_from_before_the_pool_are_drained_into_the_journal() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut run = Run::new(side(&r, &home), Role::Lead, Gates::default());
        let done = |u: &str, k: &str| Handoff { done: Some(("unit".into(), vec![(u.into(), k.into())])), ..Default::default() };
        // The coordinator's journal here, a helper's outbox.
        let journal = home.join("coord/journal");
        crate::handoff::write(&journal.join("m1"), &done("6/2/1", "k21")).unwrap();
        crate::handoff::write(&journal.join("m1"), &done("6/2/2", "k22")).unwrap();
        let ob = home.join("outbox/1791328399374");
        std::fs::create_dir_all(&ob).unwrap();
        std::fs::write(ob.join("work.json"), serde_json::to_vec(&crate::agent::build::Work { step: "unit".into(), targets: vec![("6/2/3".into(), "k23".into())] }).unwrap()).unwrap();
        crate::handoff::write(&ob, &Handoff { changes: [("base/6-2-3".to_string(), Some("base/6-2-3.3333333333333333.base".to_string()))].into(), ..Default::default() }).unwrap();
        std::fs::write(ob.join("result.json"), serde_json::json!({ "ok": true, "done": ["unit", [["6/2/3", "k23"]]] }).to_string()).unwrap();
        // (A task's folder: left to its broker. A failed job's with nothing to hand off: gone, or
        // the member would never ask for work again.)
        std::fs::create_dir_all(home.join("outbox/1791328399375")).unwrap();
        let failed = home.join("outbox/1791328399376");
        std::fs::create_dir_all(&failed).unwrap();
        std::fs::write(failed.join("work.json"), serde_json::to_vec(&crate::agent::build::Work { step: "unit".into(), targets: vec![("6/2/4".into(), "k24".into())] }).unwrap()).unwrap();
        std::fs::write(failed.join("result.json"), r#"{"ok": false, "done": null, "error": "it failed"}"#).unwrap();
        run.drain(Some(&journal), &home.join("outbox"));
        let leases: Vec<LeaseId> = run.entries.iter().map(|(e, _)| e.lease).collect();
        assert_eq!(leases.len(), 3, "{leases:?}");
        assert!(leases.iter().all(|l| l.term == 0));
        assert!(leases.contains(&LeaseId { term: 0, n: 1791328399374 }), "a helper's keeps its lease");
        assert_eq!(leases.iter().filter(|l| l.n >= DRAINED).count(), 2);
        // Drained once in this process, whatever the next look finds.
        run.drain(Some(&journal), &home.join("outbox"));
        assert_eq!(run.entries.len(), 3);
        // Merged by the lead's step; their sources removed once its saved state holds them.
        let out = run.step(true);
        assert_eq!(out.leads, Some(1));
        let rec = run.side.driver().records().unwrap();
        assert_eq!((rec.keys.recorded("unit", "6/2/1"), rec.keys.recorded("unit", "6/2/2"), rec.keys.recorded("unit", "6/2/3")), (Some("k21"), Some("k22"), Some("k23")));
        assert_eq!(rec.manifest.get("base/6-2-3").map(String::as_str), Some("base/6-2-3.3333333333333333.base"));
        assert!(crate::handoff::waiting_in(&journal).unwrap().is_empty() && !ob.exists());
        // (Marked merged: a file whose removal had failed isn't drained again.)
        assert!(journal.join("m1").with_extension("merged").exists());
        assert!(home.join("outbox/1791328399375").exists() && !failed.exists());
        // Term 1's records written to today's files too, for their readers.
        assert_eq!(crate::agent::build::Keys::load(&r).recorded("unit", "6/2/2"), Some("k22"));
    }

    #[test]
    fn the_merge_refuses_what_out_would() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut run = Run::new(side(&r, &home), Role::Lead, Gates::default());
        let me = run.side.member().id.clone();
        let pack = |n: u64, t: &str, names: &[&str]| {
            let dash = t.replace('/', "-");
            let changes = names.iter().map(|l| (format!("{l}/{dash}"), Some(format!("{l}/{dash}.{n:016x}.pack")))).collect();
            Entry { member: me.clone(), lease: LeaseId { term: 1, n }, step: "pack".into(), handoff: Handoff { changes, done: Some(("pack".into(), vec![(t.into(), format!("k{n}"))])), ..Default::default() }, at: crate::agent::jobs::now_s() }
        };
        // A map tile's own files, and one that also writes a terrain pack (another step's).
        let within = pack(1, "6/1/2", &["hidata", "layers/roads/hi"]);
        let outside = pack(2, "6/1/3", &["hidata", "layers/terrain/hi"]);
        let key = outside.key().unwrap();
        // (The same names a pack job's `Out` takes and refuses.)
        let mut o = crate::out::Out::open(&d.path().join("o"), &d.path().join("o-scratch"), "pack").unwrap();
        for (l, c) in &within.handoff.changes {
            o.set(l, c.clone());
        }
        o.save().unwrap();
        o.set("layers/terrain/hi/6-1-3", Some("layers/terrain/hi/6-1-3.0000000000000002.pack".into()));
        assert!(o.save().is_err());
        run.entries.extend([(within, None), (outside, None)]);
        let out = run.step(true);
        assert_eq!(out.leads, Some(1));
        // The one within merged, the one outside refused, with why, and nothing of it taken.
        let rec = run.side.driver().records().unwrap();
        assert_eq!((rec.keys.recorded("pack", "6/1/2"), rec.keys.recorded("pack", "6/1/3")), (Some("k1"), None));
        assert!(!rec.manifest.contains_key("layers/terrain/hi/6-1-3") && !rec.manifest.contains_key("hidata/6-1-3"));
        let why = rec.rejected.get(&key).expect("refused");
        assert!(why.contains("layers/terrain/hi/6-1-3"), "{why}");
    }

    #[test]
    fn jobs_an_earlier_process_left_are_handed_off() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let lease = LeaseId { term: 3, n: 11 };
        let dir = job_dir(&home, lease);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("work.json"), serde_json::to_vec(&JobKept { step: "unit".into(), targets: vec![("6/1/1".into(), "k1".into()), ("6/1/2".into(), "k2".into())], lease }).unwrap()).unwrap();
        crate::handoff::write(&dir, &Handoff { changes: [("base/6-1-1".to_string(), Some("base/6-1-1.1111111111111111.base".to_string())), ("base/6-1-2".to_string(), Some("base/6-1-2.2222222222222222.base".to_string()))].into(), ..Default::default() }).unwrap();
        std::fs::write(dir.join("done.txt"), "unit 6/1/1\n").unwrap();
        // (A folder of no job: gone.)
        std::fs::create_dir_all(home.join("pool/jobs/junk")).unwrap();
        let left = left_jobs(&home, "m-000000000000000a");
        assert_eq!(left.len(), 1);
        let (e, d0) = &left[0];
        assert_eq!((e.lease, d0), (lease, &dir));
        assert_eq!(e.handoff.done, Some(("unit".into(), vec![("6/1/1".into(), "k1".into())])), "what it noted done");
        assert_eq!(e.handoff.changes.keys().collect::<Vec<_>>(), ["base/6-1-1"]);
        assert!(!home.join("pool/jobs/junk").exists());
    }

    #[test]
    fn the_token_is_the_pools() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path().join("nas");
        let nas = Share::new(&r);
        let (m4, m1) = (d.path().join("m4/coord"), d.path().join("m1/coord"));
        std::fs::create_dir_all(&m4).unwrap();
        std::fs::write(m4.join("workers-token"), "t0k3n").unwrap();
        // The lead's, copied to the NAS as the pool begins.
        seed(&nas, &m4).unwrap();
        assert_eq!(nas.read(TOKEN).unwrap().as_deref(), Some(&b"t0k3n"[..]));
        // Another lead's coordinator starts with them.
        std::fs::create_dir_all(&m1).unwrap();
        std::fs::write(m1.join("workers-token"), "other").unwrap();
        seed(&nas, &m1).unwrap();
        assert_eq!(std::fs::read_to_string(m1.join("workers-token")).unwrap(), "t0k3n");
        // The first's stay.
        seed(&nas, &m1).unwrap();
        assert_eq!(nas.read(TOKEN).unwrap().as_deref(), Some(&b"t0k3n"[..]));
    }

    #[test]
    fn the_merges_checks() {
        let e = unit_entry("m-000000000000000a", LeaseId { term: 1, n: 1 }, "6/1/2", "k2");
        let r = Records::default();
        assert!(check(&e, &r).is_ok());
        let mut other = e.clone();
        other.step = "pois".into();
        assert!(check(&other, &r).unwrap_err().contains("done record is of unit"));
        let mut bad = e.clone();
        bad.handoff.changes.insert("base/6-1-3".into(), Some("base/6-1-4.3333333333333333.base".into()));
        assert!(check(&bad, &r).unwrap_err().contains("isn't a content name of base/6-1-3"));
        let mut raw = e.clone();
        raw.handoff.raw.push(("3-1-2".into(), crate::rawpack::Pack { name: "3-1-3.0123456789abcdef.tiles".into(), bytes: 1 }));
        assert!(check(&raw, &r).is_err());
        let mut none = e.clone();
        none.handoff.done = Some(("unit".into(), Vec::new()));
        assert!(check(&none, &r).is_err());
        // A shared step's: only its targets' files, its own uploads.
        let mut other = e.clone();
        other.handoff.changes.insert("base/6-1-9".into(), Some("base/6-1-9.3333333333333333.base".into()));
        assert!(check(&other, &r).unwrap_err().contains("isn't one of the files of the targets it did"));
        let mut up = e.clone();
        up.handoff.pending.insert("base/6-1-9.3333333333333333.base".into(), "aa".into());
        assert!(check(&up, &r).unwrap_err().contains("isn't one of its saves"));
        // Another step's (the OSM pass's stages): any name of its outputs, and none outside them
        // (the unit's base pack, another step's).
        let mut pass = e;
        pass.step = "osm-pass".into();
        pass.handoff.done = None;
        pass.handoff.changes.insert("sources/osm/2026-10-01/pass".into(), Some("sources/osm/2026-10-01/pass.4444444444444444.json".into()));
        assert!(check(&pass, &r).unwrap_err().contains("writes base/6-1-2, outside osm-pass's write-set"));
        pass.handoff.changes.remove("base/6-1-2");
        pass.handoff.pending.clear();
        assert!(check(&pass, &r).is_ok());
    }

    /// An entry outside its step's write-set from a member on a newer app than the lead's is held,
    /// not refused (its app's write-sets may know the name); from the lead's app or an older one,
    /// refused; its member's app not read now, held; any other refusal stands whatever the app.
    #[test]
    fn an_entry_outside_from_a_newer_app_is_held() {
        let r = Records::default();
        let lead = "20261009-0000-bbbbbbb";
        let mut e = unit_entry("m-000000000000000b", LeaseId { term: 1, n: 1 }, "6/1/2", "k2");
        e.handoff.changes.insert("work/new-output".into(), Some("work/new-output.0123456789abcdef.json".into()));
        let on = |app: Option<&'static str>| move |_: &str| app.map(str::to_string);
        let held = check_from(&e, &r, lead, &on(Some("20261010-0000-ccccccc"))).unwrap_err();
        assert!(held.starts_with(crate::pool::records::HOLD) && held.contains("work/new-output") && held.contains("20261010-0000-ccccccc"), "{held}");
        assert!(check_from(&e, &r, lead, &on(None)).unwrap_err().starts_with(crate::pool::records::HOLD));
        for app in [lead, "20261008-0000-aaaaaaa"] {
            let why = check_from(&e, &r, lead, &on(Some(app))).unwrap_err();
            assert!(!why.starts_with(crate::pool::records::HOLD) && why.contains("outside unit's write-set"), "{app}: {why}");
        }
        // Another check's refusal (a content name not of its logical name): refused, newer or not.
        let mut bad = unit_entry("m-000000000000000b", LeaseId { term: 1, n: 2 }, "6/1/2", "k2");
        bad.handoff.changes.insert("base/6-1-2".into(), Some("base/6-1-3.3333333333333333.base".into()));
        assert!(!check_from(&bad, &r, lead, &on(Some("20261010-0000-ccccccc"))).unwrap_err().starts_with(crate::pool::records::HOLD));
        assert_eq!(check_from(&unit_entry("m-000000000000000b", LeaseId { term: 1, n: 3 }, "6/1/2", "k2"), &r, lead, &on(None)), Ok(()));
    }

    #[test]
    fn todays_files_and_the_history_per_writer() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let mut rec = Records { term: 2, seq: 4, ..Default::default() };
        rec.manifest.insert("base/6-1-1".into(), "base/6-1-1.1111111111111111.base".into());
        rec.keys.record("unit", &[("6/1/1".into(), "k1".into())]);
        write_today(&Share::new(r), &rec).unwrap();
        assert_eq!(crate::agent::build::Keys::load(r).recorded("unit", "6/1/1"), Some("k1"));
        let m: BTreeMap<String, String> = crate::out::read_record(&r.join("state/build/manifest.json")).unwrap();
        assert_eq!(m, rec.manifest);
        let ev = |seq: u64, t: u64| crate::coord::history::Event { seq, t, ..crate::coord::history::Event::new("start") };
        append_history(r, "m-000000000000000a", &[ev(1, 1_791_400_000), ev(2, 1_791_400_001)]).unwrap();
        append_history(r, "m-000000000000000a", &[ev(3, 1_791_500_000)]).unwrap();
        let day = |t| journal::day(t).unwrap();
        let f = |t| std::fs::read_to_string(r.join("state/coord/history").join(day(t)).join("m-000000000000000a.jsonl")).unwrap();
        assert_eq!(f(1_791_400_000).lines().count(), 2);
        assert_eq!(f(1_791_500_000).lines().count(), 1);
    }

    /// Moves a run's clocks on `s` seconds (its driver's wall and awake clocks: `Side::ahead`).
    fn pass(r: &mut Run, s: u64) {
        r.side.ahead += s;
    }

    /// Ticks `r` as its pool's own thread would (`TICK_S` apart: its agent's loop busy) until
    /// `done`; the other runs ticked in turn, their clocks moved alike (their listings made off
    /// their steps meanwhile: five minutes is a watchdog, not a measure).
    fn tick_until(runs: &mut [&mut Run], done: impl Fn(&[&mut Run]) -> bool) {
        let end = Instant::now() + WATCHDOG;
        loop {
            for r in runs.iter_mut() {
                pass(r, TICK_S);
                assert!(r.tick_due(), "due {TICK_S} s on, its loop silent");
                r.tick();
            }
            if done(runs) {
                return;
            }
            assert!(Instant::now() < end, "not done in {WATCHDOG:?} of ticks");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    #[test]
    fn only_the_current_leads_coordinator_answering_counts() {
        // Its answers say its term and member, while it has them; a member counts an answer only
        // from the current term's lead.
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "a", "development");
        let client = crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "b");
        let ask = crate::coord::Ask { kind: "native".into(), can: vec!["x".into()], mem_mb: 1024, ..Default::default() };
        client.ask(&ask).ok();
        let first = client.answered().expect("answered");
        assert_eq!((first.term, first.member.as_deref()), (None, None), "not a pool's lead yet: it says nothing");
        c.set_member("m-000000000000000a");
        c.set_term(3);
        client.ask(&ask).ok();
        let a = client.answered().unwrap();
        assert_eq!((a.term, a.member.as_deref()), (Some(3), Some("m-000000000000000a")));
        let term = |t: u64, m: &str| crate::pool::term::Term { term: t, member: m.into(), host: "A".into(), app: "development".into(), since: 1, how: String::new(), from: t - 1, seq: None };
        assert_eq!(lead_answered(Some(&a), Some(&term(3, "m-000000000000000a"))), Some(a.at));
        assert_eq!(lead_answered(Some(&a), Some(&term(4, "m-000000000000000a"))), None, "an earlier term's lead");
        assert_eq!(lead_answered(Some(&a), Some(&term(3, "m-000000000000000c"))), None, "another member's");
        assert_eq!(lead_answered(Some(&first), Some(&term(3, "m-000000000000000a"))), None, "one that says nothing");
        assert_eq!(lead_answered(None, Some(&term(3, "m-000000000000000a"))), None);
    }

    #[test]
    fn a_pool_step_held_by_a_tick_is_passed_over_and_a_panicking_one_clears_in_step() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let shared: Shared = Arc::new(std::sync::Mutex::new(Run::new(side(&r, &home), Role::Member, Gates::default())));
        {
            let _held = lock(&shared);
            assert!(try_lock(&shared).is_none(), "a tick holds it: not waited on");
        }
        let s2 = shared.clone();
        let t = std::thread::spawn(move || {
            lock(&s2).in_step = true;
            let _clear = InStep(s2.clone());
            panic!("the loop's pool step panicked");
        });
        assert!(t.join().is_err());
        let run = try_lock(&shared).expect("free again");
        assert!(!run.in_step, "cleared as it unwound");
    }

    #[test]
    fn a_tick_that_panics_isnt_said_to_hold_the_run() {
        let t = Ticking::default();
        let t2 = t.clone();
        assert!(std::thread::spawn(move || {
            let _said = t2.begin();
            assert!(t2.since().is_some());
            panic!("the tick panicked");
        })
        .join()
        .is_err());
        assert_eq!(t.since(), None, "ended as it unwound");
    }

    #[test]
    fn a_handovers_times_fit_the_pools_steps() {
        use crate::pool::handover::{OFFER_S, SETTLE_S, TAKE_UP_S};
        // (Derived, not figures of their own: each Mac's pool is stepped at least every `QUIET_S`
        // whatever its agent does, a step itself taking up to as long again on a share under load.)
        // An offer: the target's step reads it and answers, the lead's step reads the answer.
        assert!(OFFER_S >= 2 * 2 * QUIET_S, "{OFFER_S}");
        // Settling: the lead's next step settles, the one after passes.
        assert!(SETTLE_S >= 2 * QUIET_S, "{SETTLE_S}");
        // Taking up: the target's next step reads the term (or the message) and takes it up.
        assert!(TAKE_UP_S >= 2 * 2 * QUIET_S, "{TAKE_UP_S}");
        assert!(TICK_S <= QUIET_S && QUIET_S < crate::pool::driver::STALL_S);
    }

    #[test]
    fn a_tick_is_due_while_the_agents_loop_is_quiet_and_never_during_its_step() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut run = Run::new(side(&r, &home), Role::Member, Gates::default());
        assert!(!run.tick_due(), "no NAS known yet: nothing to tick");
        run.cx = Cx { root: Some(r.clone()), home: home.clone(), able: true, ..Default::default() };
        assert!(run.tick_due(), "never stepped");
        // The loop's step: none while it runs, nor for `QUIET_S` after it.
        run.in_step = true;
        assert!(!run.tick_due());
        run.step(true);
        run.stepped_full();
        assert!(!run.in_step && !run.tick_due());
        pass(&mut run, TICK_S);
        assert!(!run.tick_due(), "the loop stepped it {TICK_S} s ago: its next step is about due");
        pass(&mut run, QUIET_S - TICK_S);
        assert!(run.tick_due(), "the loop quiet {QUIET_S} s: its step is held up");
        run.tick();
        assert!(!run.tick_due());
        pass(&mut run, TICK_S - 1);
        assert!(!run.tick_due());
        pass(&mut run, 1);
        assert!(run.tick_due(), "every {TICK_S} s while the loop stays busy");
        // Its view, written at every step, for the controls (`read_view`).
        let v = read_view(&home).expect("its view");
        assert_eq!((v.pool.member, v.beat), (run.side.member().id.clone(), run.side.now() - TICK_S));
    }

    #[test]
    fn a_handover_goes_through_both_pools_threads_while_both_agents_plan() {
        // (#191: the target answered an offer only once per agent step, minutes apart while it
        // planned, and the lead gave up after a minute.) Neither agent's loop steps here: their
        // pools' ticks alone offer, answer, settle, pass and take up, `TICK_S` apart.
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let (ha, hb) = (d.path().join("a/agent"), d.path().join("b/agent"));
        let mut a = Run::new(side(&r, &ha), Role::Member, Gates::default());
        let mut b = Run::new(side(&r, &hb), Role::Member, Gates::default());
        let (ia, ib) = (a.side.member().id.clone(), b.side.member().id.clone());
        let (coord, _) = crate::coord::start_for_test(&d.path().join("a/coord"), "a", "development");
        let coord = Arc::new(coord);
        // (A's first steps its loop's, as the agent's start makes them: it takes term 1 up.)
        let end = Instant::now() + WATCHDOG;
        while !a.step(true).caught_up {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(a.gates.leads, Some(1));
        a.cx = Cx { root: Some(r.clone()), home: ha.clone(), coord: Some(coord.clone()), able: true, ..Default::default() };
        b.cx = Cx { root: Some(r.clone()), home: hb.clone(), able: true, ..Default::default() };
        tick_until(&mut [&mut a], |r| r[0].gates.caught_up);
        assert_eq!(coord.shared.lock().unwrap().moving, None, "it grants");
        a.side.know(&ib);
        b.side.know(&ia);
        tick_until(&mut [&mut b], |r| r[0].side.driver().current().term == 1);
        // A duty runs on A (a catalog): offered and answered, but not settled while it runs.
        crate::control::request_lead(&ha, crate::control::LeadAsk::Give { to: ib.clone() }, "a test").unwrap();
        a.cx.duty = true;
        tick_until(&mut [&mut a, &mut b], |r| r[0].gates.settle);
        assert!(a.settled.is_none() && coord.shared.lock().unwrap().moving.as_deref() == Some("settling a handover"));
        a.tick();
        assert!(a.settled.is_none(), "a duty runs: the agent's loop stops it first");
        // Stopped (the loop's step): settled, passed, taken up, within the offer's and the settle's
        // minutes on A's clock.
        a.cx.duty = false;
        tick_until(&mut [&mut a, &mut b], |r| r[1].side.driver().leads() == Some(2));
        assert_eq!(a.side.driver().leads(), None);
        assert_eq!(coord.shared.lock().unwrap().moving.as_deref(), Some("no longer leads"), "its coordinator grants nothing; the loop stops it");
        // B's coordinator's part (the state handed over) is its loop's next step's.
        let handed = b.pending_took_up.iter().find_map(|e| match e {
            Event::TookUp { term: 2, handed, .. } => Some(handed.clone()),
            _ => None,
        });
        assert!(handed.is_some_and(|h| h.is_some_and(|h| h["leases"].is_object())), "{:?}", b.pending_took_up);
        tick_until(&mut [&mut a, &mut b], |r| r[0].controls.kept.asked.as_ref().is_some_and(|k| k.state == super::super::lead::State::Done));
        // The views, fresh on both Macs without their loops.
        assert_eq!(read_view(&hb).and_then(|v| v.pool.lead).map(|v| (v.term, v.leading)), Some((2, true)));
        assert_eq!(read_view(&ha).and_then(|v| v.pool.gates.leads), None);
    }
}
