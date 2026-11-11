//! The agent's part in the pool (docs/pool.md §12, phase 1's integration): what runs the pool's
//! driver (crate::pool::driver) and does what it can't do itself. The driver's contract (its
//! module's doc) says what that is; this module keeps it:
//!
//! - **Stepped every 20 s or so, whatever the agent is doing** (`Run`, shared under a lock:
//!   `Shared`): by the agent's loop, its step and what follows it in one hold of the run
//!   (`Run::in_step`), and by the pool's own thread (`Ticker`) whenever that loop hasn't stepped it
//!   for `QUIET_S` (a plan, a job being stopped, a share slow to answer): `Run::tick`, every `TICK_S`
//!   while the loop stays busy. A tick does what's the pool's alone: the driver's step (its
//!   heartbeat and mail with it), the history, the view (`VIEW`), whether the
//!   coordinator grants. What's the agent's (its jobs, its coordinator's start and stop, the state a
//!   take-up loads: `Run::pending_took_up`) waits for its loop's next step. One step at a
//!   time, each file of the pool's written by the run alone.
//! - **One process per member** (`Side::open`): the member's id (crate::pool::member_id, in the
//!   agent's folder), its lock in the app's folder (crate::pool::MemberLock), and the driver made
//!   from the state saved last. A process the lock is held from runs no driver.
//! - **The saved state** (`Saved`) written whole to the agent's folder after every step that changed
//!   it, before anything that step said is acted on; a job's hand-off handed to a step kept (its
//!   folder) until a saved state holding it is on disk.
//! - **The hand-offs** its jobs handed it, kept whole in the saved state for its beat to list and
//!   post to the lead (crate::coord::handoffs), dropped once the lead's answer says its saved records
//!   name them; leading, those its members posted it, merged by the driver.
//! - **The members it knows**: those heard by their beats (crate::coord::beats: the lead's own, a
//!   member's from its lead's answers), forgotten once not heard for `FORGET_AFTER`.
//! - **The messages** between a member and the lead, by HTTP (docs/coordination.md step 5), each
//!   numbered, the sender's last `KEPT` kept in its `mail.json`: to the lead by its coordinator's
//!   `/pool/mail` (`post_mail`), into its store before it answers, those it hasn't taken posted
//!   again while it answers the member's beats (none lost while it's down); from the lead in the
//!   answer to the member's next beat (crate::coord::inbox's `to`), after the number the member
//!   said it took. Best effort above that, as the driver's are (a message lost is made up for).
//! - **The heartbeat**: the driver's fields, the members it knows, composed each step for its
//!   agent's beat (crate::agent::beat), stamped as it's sent.
//! - **The merge's checks** (`check`): what phase 1 checks of a hand-off.
//!
//! The pool is always on (§12 phase 4's thirteenth batch): every agent with the NAS leads or works
//! as a member: the declared lead (crate::pool::lead::declared, the Mac mini) leads, every other
//! Mac is a member (task #881). (`state/pool/enabled`, the switch of before, is no longer read; an
//! older app reads it, so it stays on the NAS until none runs.)

use crate::pool::driver::{self, Driver, Event, Heard, Io, Msg, Out, Saved};
use crate::pool::journal::{self, Entry};
use crate::pool::nas::{Created, Nas};
use crate::pool::records::Records;
use crate::pool::{Beat, Member, MemberLock};
use anyhow::{Context, Result};
use buildstore::handoff::Handoff;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The messages a sender keeps for a recipient: its last.
const KEPT: usize = 64;
/// A member not heard by its beat this long is forgotten (twice the time a heartbeat takes to go
/// out of touch).
pub const FORGET_AFTER: Duration = Duration::from_secs(2 * crate::pool::beat::OUT_OF_TOUCH_S);
/// The members forgotten (`Side::forgotten`), in the agent's pool folder: kept across restarts.
const FORGOTTEN: &str = "forgotten.json";
/// The records this Mac last saved leading, in the agent's pool folder (crate::pool::driver::Io::
/// keep, docs/coordination.md step 8b): what its next take-up starts from.
pub const RECORDS: &str = "records.json";
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

/// A NAS the driver's steps and the agent's threads share.
pub type SharedNas = Arc<dyn Nas + Send + Sync>;

/// Its coordinator's store's epoch, raised past a take-up's records first (crate::pool::driver::
/// Io::epoch): the lead's agent's (`Run::step`, through `epochs`).
pub type Epochs = Arc<dyn Fn(u64) -> Result<u64> + Send + Sync>;

/// The driver's world: a NAS, this process's clocks (the awake one, `Instant`, stops while the
/// Mac sleeps), the agent's pool folder `dir`, where it keeps the records it leads (`RECORDS`), and
/// its coordinator's store's epoch (`epochs`; None: no coordinator's store, not the lead's process).
struct Clocked<'a> {
    nas: &'a dyn Nas,
    clock: &'a dyn store::clock::Clock,
    start: Instant,
    /// Both clocks moved on this far (s): the tests' way of letting time pass.
    ahead: u64,
    dir: &'a Path,
    epochs: Option<&'a Epochs>,
}

impl Nas for Clocked<'_> {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        self.nas.create_new(path, bytes)
    }
    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.nas.write_whole(path, bytes)
    }
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.nas.read(path)
    }
    fn read_fresh(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.nas.read_fresh(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.nas.exists(path)
    }
}

impl Io for Clocked<'_> {
    fn now(&self) -> u64 {
        self.clock.wall().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) + self.ahead
    }
    fn awake(&self) -> u64 {
        self.clock.now().saturating_duration_since(self.start).as_secs() + self.ahead
    }
    fn kept(&self) -> Result<Option<Vec<u8>>> {
        let p = self.dir.join(RECORDS);
        match std::fs::read(&p) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
        }
    }
    fn keep(&self, records: &[u8]) -> Result<()> {
        // (Its rename made durable too, the folder flushed, before the NAS has them: a power cut
        // after a save acknowledged on the NAS never leaves an older copy here, which a take-up
        // would start from without reading the NAS.)
        store::seglog::write_durable(&self.dir.join(RECORDS), records)
    }
    fn epoch(&self, past: u64) -> Result<u64> {
        self.epochs.context("no coordinator's store in this process")?(past)
    }
}

/// No coordinator's store in a real process until its coordinator starts (`Run::step` gives its
/// `epochs`); a unit test's side, which runs none, numbers its own, in memory, past a take-up's
/// records as a store's are.
fn own_epochs() -> Option<Epochs> {
    if !cfg!(test) {
        return None;
    }
    let n = Arc::new(std::sync::atomic::AtomicU64::new(0));
    Some(Arc::new(move |past: u64| Ok(n.fetch_max(past + 1, std::sync::atomic::Ordering::SeqCst).max(past + 1))))
}

/// `Epochs` from coordinator `c`, on the NAS `nas`, of the Mac `host`: its store's epoch raised past
/// `past`, and past the store's backup on the NAS by `RESTORE_EPOCHS` when another Mac wrote it (the
/// owner moved the lead here: its leases, which `took_up` loads, were granted in epochs up to the
/// backup's and a restart or more after it, as a restored store's are, docs/coordination.md §4.4).
/// A backup that can't be read is said, and passed over.
pub fn epochs(c: Arc<crate::coord::Coordinator>, nas: SharedNas, host: String) -> Epochs {
    Arc::new(move |past: u64| {
        let past = match crate::pool::store::read_backup(&*nas) {
            Ok(Some(b)) if b.member != host => past.max(b.epoch + crate::pool::store::RESTORE_EPOCHS),
            Ok(_) => past,
            Err(e) => {
                eprintln!("pool: the lead's store's backup, for its epoch: {e:#}");
                past
            }
        };
        c.raise_epoch(past)
    })
}

/// The messages after number `last` (from its posts, crate::coord::inbox, or its lead's answers), in
/// their sender's order, each once, and the number read up to: each message read alone, so one
/// this app doesn't know (a newer app's) is passed over, and read past all the same (never read
/// again, never holding the others back).
fn after(msgs: impl IntoIterator<Item = (u64, serde_json::Value)>, last: u64) -> (Vec<Msg>, u64) {
    let new: BTreeMap<u64, serde_json::Value> = msgs.into_iter().filter(|(n, _)| *n > last).collect();
    let top = new.keys().last().copied().unwrap_or(last);
    (new.into_values().filter_map(|m| serde_json::from_value::<Msg>(m).ok()).collect(), top)
}

/// How a member posts its mail to the lead's coordinator (`/pool/mail`): its addresses, its key,
/// the post; the answer's status and JSON. (A test's own reaches a coordinator on loopback.)
pub type Courier =
    Arc<dyn Fn(&[String], &str, &serde_json::Value) -> Result<crate::coord::client::Reply> + Send + Sync>;

/// The courier over HTTP: each address in turn (crate::coord::client).
pub fn http_courier() -> Courier {
    Arc::new(|urls, token, body| {
        crate::coord::client::Client::at(urls.to_vec(), token.to_string(), "pool").post_json("/pool/mail", body)
    })
}

/// The addresses mail may be posted to: the tailnet's alone (a LAN's name or address is any
/// device's on the network to claim, crate::coord::brokers::origins_of). Loopback only in a unit
/// test (`buildstore::out::testing`), where every Mac's address is (net::urls); a real broker never lists it.
fn mail_urls(urls: &[String]) -> Vec<String> {
    use crate::coord::brokers::{origin, tailnet};
    let ours = |o: &str| tailnet(o) || (buildstore::out::testing() && o.starts_with("http://127.0.0.1:"));
    urls.iter().filter(|u| origin(u).is_some_and(|o| ours(&o))).cloned().collect()
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
    /// The number up to which each lead's coordinator took what it sent by HTTP (`post_mail`): a
    /// post carries what it sent after.
    #[serde(default)]
    posted: BTreeMap<String, u64>,
}

/// A member's heartbeat, in its beat to the lead (crate::coord::beats): the driver's fields, and
/// the members it knows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    #[serde(flatten)]
    pub pool: Beat,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// Its Mac's conditions, for the controls (§11: whether it's home on power, on battery, away;
    /// whether it can lead now). Left out by apps before phase 3, and ignored by them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conds: Option<Conds>,
    /// Its broker (docs/pool.md §8, crate::agent::brokers: `state/pool/brokers` on): where a page
    /// reaches it, and how many of its jobs' tasks wait, of which kinds, how large. Left out with
    /// the switch off, and by apps before it (ignored by them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker: Option<crate::coord::brokers::Broker>,
    /// Always so: said for a lead on the app before task #1036, which reads a heartbeat without them
    /// as an older app's (its mail by mailbox, its heartbeat on the NAS) and falls back to the NAS.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mail_http: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub beat_http: bool,
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
    /// Paused alone (jobrt::control::Hold, task #224): it takes no work until resumed. Left out by
    /// apps before it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

/// What the agent hands a step.
#[derive(Default)]
pub struct Give {
    /// Its jobs' hand-offs that ended, each with the folder (or file) holding it, removed once a
    /// saved state holding the entry is on disk.
    pub entries: Vec<(Entry, Option<PathBuf>)>,
    /// Its build page's asks.
    pub asks: Vec<driver::Ask>,
    /// Its Mac's conditions, for its heartbeat (None: left out).
    pub conds: Option<Conds>,
    /// The raw archives named since its records last dropped them (`Heard::named_raw`).
    pub named_raw: Vec<String>,
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
    /// The members sent to and not yet sent it all (posted, queued or written) since.
    mail_dirty: BTreeSet<String>,
    /// What members posted it by HTTP, and what it sends them in their beats' answers
    /// (crate::coord::inbox, its coordinator's when it leads).
    inbox: Arc<crate::coord::inbox::Inbox>,
    /// The members' hand-offs posted it by HTTP (crate::coord::handoffs): its coordinator's when it
    /// leads.
    handoffs: Arc<crate::coord::handoffs::Handoffs>,
    /// The members' beats heard by HTTP (crate::coord::beats): its coordinator's when it leads, the
    /// lead's answers' else; its own heartbeat for its agent's beat.
    http: Arc<crate::coord::beats::Beats>,
    /// How it posts to the lead, the lead's coordinator's addresses and key as its contact on the
    /// NAS says (`Cx::lead_urls`, `lead_token`).
    pub courier: Courier,
    pub lead_contact: (Vec<String>, String),
    /// The members it knows (itself among them), as heard by their beats and of since.
    members: BTreeSet<String>,
    /// The members not heard by their beats, since when (`FORGET_AFTER`).
    missed: BTreeMap<String, Instant>,
    /// The members forgotten: not known again from a message or as the lead until their beat is heard.
    forgotten: BTreeSet<String>,
    /// It stopped for good (another process holds its lock): why.
    pub stopped: Option<String>,
    /// Its Mac's conditions as last given, for its heartbeat.
    conds: Option<Conds>,
    /// Its broker, for its heartbeat (crate::agent::brokers; None: the switch off).
    pub broker: Option<crate::coord::brokers::Broker>,
    /// Whom it reached and whom it couldn't, for its heartbeat (crate::pool::reachable).
    pub reach: Vec<crate::pool::reachable::Reach>,
    /// The members' heartbeats (their pool's fields) as `Run::tend_reach` last read them.
    pub beats: Vec<Beat>,
    /// When (unix s) `beats` were last read.
    pub beats_at: Option<u64>,
    /// Whether every member's heartbeat, as `beats` were last read, says its app's NAS check looks
    /// at `map/` (`Beat::checks_map`): the NAS's old catalogues may go (crate::agent::gc); a member
    /// with none heard counts as one that doesn't. False until read.
    pub all_check_map: bool,
    /// Both its clocks moved on this far (s): tests let time pass so.
    pub ahead: u64,
    /// Its clocks (a test's virtual one: `set_clock`).
    clock: Arc<dyn store::clock::Clock>,
    /// Its coordinator's store's epoch (`Epochs`): the lead's process's, set each step (`Run::step`).
    pub epochs: Option<Epochs>,
}

impl Side {
    /// This Mac's member in the pool, in folder `dir` (the agent's pool folder: its saved state)
    /// with its member id in `home` (the agent's folder), its lock in `locks` (the app's folder),
    /// over `nas`. None when another process holds the member's lock (this one runs no driver).
    pub fn open(home: &Path, dir: &Path, locks: &Path, app: &str, nas: SharedNas) -> Result<Option<Side>> {
        let id = crate::pool::member_id(home)?;
        let Some(lock) = MemberLock::take(locks, &id)? else { return Ok(None) };
        std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
        let me = Member { id: id.clone(), host: store::sys::host_name(), app: app.to_string() };
        let (saved, written) = load_saved(&dir.join("saved.json"));
        let mail: Mail =
            std::fs::read(dir.join("mail.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let known: Vec<String> = std::fs::read(dir.join("members.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let forgotten: BTreeSet<String> =
            std::fs::read(dir.join(FORGOTTEN)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut members: BTreeSet<String> =
            known.into_iter().filter(|m| crate::pool::is_member_id(m) && !forgotten.contains(m)).collect();
        members.insert(id);
        let driver = Driver::new(me.clone(), saved, lock);
        // (What it sent before sent again: a lead's queue for its members' beats, a member's posts
        // the lead hasn't taken. Its asks of the lead an earlier process left, queued again.)
        let mail_dirty = mail.sent.keys().cloned().collect();
        let http: Arc<crate::coord::beats::Beats> = Arc::default();
        // (Its hand-offs known to its beat before its first step: its first beat lists them, so the
        // lead's GC queue never hears it owes fewer leases than it does.)
        http.set_handoffs(driver.mine().entries().cloned().collect());
        http.keep_asks(&dir.join(crate::coord::asks::QUEUE));
        http.had_settings(jobrt::control::read_pool_settings(home));
        http.keep_lead_view(&home.join(crate::coord::beats::LEAD_VIEW));
        Ok(Some(Side {
            me,
            driver: Some(driver),
            nas,
            start: Instant::now(),
            dir: dir.to_path_buf(),
            written,
            folders: Vec::new(),
            mail,
            mail_written: Vec::new(),
            mail_dirty,
            inbox: Arc::new(crate::coord::inbox::Inbox::open(&dir.join("inbox.json"))),
            handoffs: Arc::default(),
            http,
            courier: http_courier(),
            lead_contact: Default::default(),
            members,
            missed: BTreeMap::new(),
            forgotten,
            stopped: None,
            conds: None,
            broker: None,
            reach: Vec::new(),
            beats: Vec::new(),
            beats_at: None,
            all_check_map: false,
            ahead: 0,
            clock: store::clock::real(),
            epochs: own_epochs(),
        }))
    }

    /// Its member.
    pub fn member(&self) -> &Member {
        &self.me
    }

    /// The driver (the declared lead, the records it leads).
    pub fn driver(&self) -> &Driver {
        self.driver.as_ref().expect("a driver")
    }

    /// The NAS its steps use.
    pub fn nas(&self) -> &dyn Nas {
        &*self.nas
    }

    /// The same, for a thread of its own (the coordinator's store's backup, crate::pool::store::back_up).
    pub fn shared_nas(&self) -> SharedNas {
        self.nas.clone()
    }

    /// What members posted it (its coordinator's while it leads: `Coordinator::set_inbox`).
    pub fn inbox(&self) -> Arc<crate::coord::inbox::Inbox> {
        self.inbox.clone()
    }

    /// The hand-offs members posted it (its coordinator's while it leads: `Coordinator::set_handoffs`).
    pub fn handoffs(&self) -> Arc<crate::coord::handoffs::Handoffs> {
        self.handoffs.clone()
    }

    /// The members' beats it hears by HTTP (its coordinator's while it leads: `Coordinator::set_beats`).
    pub fn http_beats(&self) -> Arc<crate::coord::beats::Beats> {
        self.http.clone()
    }

    /// The members it knows.
    pub fn members(&self) -> &BTreeSet<String> {
        &self.members
    }

    /// Knows member `m` (its mail read from now on).
    pub fn know(&mut self, m: &str) {
        if crate::pool::is_member_id(m) {
            self.forgotten.remove(m);
            self.members.insert(m.to_string());
        }
    }

    /// Knows member `m` from a message or as the lead, unless it was forgotten (its beat not heard).
    fn heard_of(&mut self, m: &str) {
        if !self.forgotten.contains(m) {
            self.members.insert(m.to_string());
        }
    }

    /// The members heard by their beats within `beats::FORGET_S` (and this one) known at `now`,
    /// those missed `FORGET_AFTER` forgotten; members.json written when they change.
    fn take_members(&mut self, now: Instant) {
        let mut found: BTreeSet<String> = self.http.fresh(self.now()).into_iter().map(|h| h.beat.pool.member).collect();
        found.insert(self.me.id.clone());
        let before = (self.members.clone(), self.forgotten.clone());
        let gone = forget(&mut self.members, &mut self.missed, &found, &self.me.id, now, FORGET_AFTER);
        self.forgotten.retain(|m| !found.contains(m));
        for m in gone {
            eprintln!("pool: member {m} forgotten: its beat not heard for {} min", FORGET_AFTER.as_secs() / 60);
            // (Its mail not sent: nobody reads it.)
            self.mail.sent.remove(&m);
            self.mail.posted.remove(&m);
            self.mail_dirty.remove(&m);
            self.forgotten.insert(m);
        }
        if self.forgotten != before.1 {
            if let Ok(b) = serde_json::to_vec(&self.forgotten) {
                store::whole::write(&self.dir.join(FORGOTTEN), &b).ok();
            }
            self.save_mail();
        }
        if self.members != before.0 {
            if let Ok(b) = serde_json::to_vec(&self.members) {
                store::whole::write(&self.dir.join("members.json"), &b).ok();
            }
        }
    }

    /// The app the newer readers' switch (the regions', the elevation datasets') was recorded on, when
    /// the records' manifest `m` has it (inputs::inputs::regions::read::switch_app): a member on an
    /// older one is "app too old".
    pub fn regions_app(&self, m: &BTreeMap<String, String>) -> std::result::Result<Option<String>, String> {
        let read =
            |n: &str| -> anyhow::Result<Vec<u8>> { self.nas.read(n)?.with_context(|| format!("{n} isn't there")) };
        inputs::inputs::regions::read::switch_app(m, &read).map_err(|e| format!("{e:#}"))
    }

    /// Member `m`'s heartbeat as heard by its beat (crate::coord::beats; this one's own as
    /// composed), within `beats::FORGET_S`.
    pub fn heartbeat(&self, m: &str) -> Option<Heartbeat> {
        self.http.get(m, self.now())
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
        Clocked {
            nas: &*self.nas,
            clock: &*self.clock,
            start: self.start,
            ahead: self.ahead,
            dir: &self.dir,
            epochs: self.epochs.as_ref(),
        }
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

    /// One step: what it heard (its mail, the hand-offs posted it, its lead's answers), what the agent
    /// gives, the driver's step with `check`, its saved state written if it changed; then, only once
    /// that's on disk, the step's messages sent, its heartbeat written. A step whose
    /// state couldn't be written does none of that, and says the lead may do nothing now (its
    /// duties, catalogues and sweeps held) until one is.
    pub fn step(&mut self, give: Give, check: crate::pool::records::Check) -> Out {
        if let Some(why) = &self.stopped {
            return Out { stop: Some(why.clone()), ..Default::default() };
        }
        let msgs = self.take_mail();
        let (entries, folders): (Vec<Entry>, Vec<Option<PathBuf>>) = give.entries.into_iter().unzip();
        self.folders.extend(folders.into_iter().flatten());
        let now = self.now();
        let heard = Heard {
            msgs,
            asks: give.asks,
            entries,
            named_raw: give.named_raw,
            handed: self.handoffs.taken(),
            merged: self.http.take_merged(),
            lead_says: self.http.lead_says(now),
        };
        self.conds = give.conds;
        let io = Clocked {
            nas: &*self.nas,
            clock: &*self.clock,
            start: self.start,
            ahead: self.ahead,
            dir: &self.dir,
            epochs: self.epochs.as_ref(),
        };
        let Some(driver) = self.driver.as_mut() else {
            return Out { stop: self.stopped.clone(), ..Default::default() };
        };
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
            (out.duties, out.caught_up, out.vouched) = (false, false, false);
            out.send.clear();
            return out;
        }
        // (Its hand-offs kept whole for its beat to list and post; those posted it, merged and
        // saved, acknowledged: both only now that the state saying so is on disk.)
        self.http.set_handoffs(self.driver().mine().entries().cloned().collect());
        self.handoffs.merged(&out.merged);
        if let Some(l) = self.driver().declared().map(str::to_string) {
            self.heard_of(&l);
        }
        for (to, m) in std::mem::take(&mut out.send) {
            self.post(&to, m.clone());
            out.send.push((to, m));
        }
        self.send_mail();
        self.write_beat(&out);
        self.take_members(self.clock.now());
        out
    }

    /// Writes the saved state when it changed; the folders of the hand-offs it now holds removed,
    /// once the state holding them is durable, its rename too (the folder flushed): its job's
    /// folder is the hand-off's only other copy, and no journal on the NAS holds one since task
    /// #1125.
    fn keep(&mut self) -> Result<()> {
        let b = serde_json::to_vec(&self.driver().saved())?;
        if b != self.written {
            store::whole::write(&self.dir.join("saved.json"), &b)?;
            self.written = b;
        }
        if !self.folders.is_empty() {
            store::seglog::sync_dir(&self.dir).with_context(|| format!("fsync {}", self.dir.display()))?;
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

    /// The lead's member, as its driver has it.
    fn lead(&self) -> Option<String> {
        self.driver.as_ref().and_then(|d| d.declared().map(str::to_string))
    }

    /// The messages its members posted it (its inbox, leading) since it last read, each once by its
    /// number. What it read dropped from the inbox once its marks are on disk.
    fn take_mail(&mut self) -> Vec<(String, Msg)> {
        let mut msgs = Vec::new();
        let mut posted = self.inbox.held();
        posted.retain(|from, _| *from != self.me.id && !self.forgotten.contains(from));
        // (A sender it doesn't know yet: known once its beat is heard; none heard, passed over and
        // dropped, so no post makes up a member.)
        let mut strangers = BTreeSet::new();
        let froms: Vec<String> = posted.keys().filter(|f| !self.members.contains(*f)).cloned().collect();
        for from in froms {
            match self.heartbeat(&from) {
                Some(_) => self.heard_of(&from),
                None => {
                    strangers.insert(from);
                }
            }
        }
        let others: Vec<String> = self.members.iter().filter(|m| **m != self.me.id).cloned().collect();
        for from in others {
            let got = posted.remove(&from).unwrap_or_default();
            let last = self.mail.read.get(&from).copied().unwrap_or(0);
            let (got, top) = after(got, last);
            msgs.extend(got.into_iter().map(|m| (from.clone(), m)));
            if top > last {
                self.mail.read.insert(from, top);
            }
        }
        if self.save_mail() {
            let forgotten = &self.forgotten;
            self.inbox.read_up_to(&self.mail.read, |m| !forgotten.contains(m) && !strangers.contains(m));
        }
        msgs
    }

    /// Queues message `m` to member `to`.
    fn post(&mut self, to: &str, m: Msg) {
        if self.forgotten.contains(to) {
            return;
        }
        self.heard_of(to);
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

    /// Sends what it queued since to the lead by HTTP (`post_mail`), kept while it doesn't take
    /// them (sent with the next, once it answers the member's beats again). Nothing goes to a
    /// member.
    fn send_mail(&mut self) {
        let lead = self.lead().filter(|l| *l != self.me.id);
        let answering = self.http.lead_answering(self.now());
        for to in std::mem::take(&mut self.mail_dirty) {
            if lead.as_ref() == Some(&to) && !(answering && self.post_mail(&to)) {
                self.mail_dirty.insert(to);
            }
        }
        self.save_mail();
    }

    /// Posts lead `to` what it sent it that its coordinator hasn't taken (`/pool/mail`): whether
    /// it took them all. Tried at its tailnet addresses alone (`mail_urls`), its broker's (as its
    /// beat says it) else its contact's.
    fn post_mail(&mut self, to: &str) -> bool {
        let urls = self.heartbeat(to).and_then(|hb| hb.broker).map(|b| b.urls).unwrap_or_default();
        let mut urls = mail_urls(&urls);
        if urls.is_empty() {
            urls = mail_urls(&self.lead_contact.0);
        }
        let token = self.lead_contact.1.clone();
        if urls.is_empty() || token.is_empty() {
            return false;
        }
        let from = self.mail.posted.get(to).copied().unwrap_or(0);
        let msgs: Vec<(u64, serde_json::Value)> = self
            .mail
            .sent
            .get(to)
            .into_iter()
            .flatten()
            .filter(|(n, _)| *n > from)
            .filter_map(|(n, m)| Some((*n, serde_json::to_value(m).ok()?)))
            .collect();
        let Some(top) = msgs.last().map(|(n, _)| *n) else { return true };
        let post = crate::coord::inbox::Post { from: self.me.id.clone(), to: to.to_string(), msgs };
        let Ok(body) = serde_json::to_value(&post) else { return false };
        let why = match (self.courier)(&urls, &token, &body) {
            Ok((200, v)) if v["took"].as_u64().is_some_and(|t| t >= top) => {
                self.mail.posted.insert(to.to_string(), top);
                return true;
            }
            Ok((code, v)) => format!("answered {code}: {}", v["error"].as_str().unwrap_or("")),
            Err(e) => format!("{e:#}"),
        };
        eprintln!("pool: posting mail to {to}: {why} (sent again with the next)");
        false
    }

    /// Writes `mail.json` when it changed: whether what it holds is on disk.
    fn save_mail(&mut self) -> bool {
        let Ok(b) = serde_json::to_vec(&self.mail) else { return false };
        if b == self.mail_written {
            return true;
        }
        match store::whole::write(&self.dir.join("mail.json"), &b) {
            Ok(()) => {
                self.mail_written = b;
                true
            }
            Err(e) => {
                eprintln!("pool: its mail's marks: {e:#}");
                false
            }
        }
    }

    /// Its heartbeat, for its agent's beat to send the lead (crate::agent::beat), stamped now.
    fn write_beat(&mut self, out: &Out) {
        let mut own = Heartbeat {
            pool: out.beat.clone(),
            members: self.members.iter().cloned().collect(),
            conds: self.conds,
            broker: self.broker.clone(),
            mail_http: true,
            beat_http: true,
        };
        if own.pool.member.is_empty() {
            return;
        }
        own.pool.beat = self.now();
        own.pool.reach = self.reach.clone();
        self.http.set_own(own);
    }
}

/// The members heard `found` at `now`, taken into `members`: those found known (and no longer
/// missed); those not found missed from now, and forgotten (taken out, and returned) once missed
/// `after`. Member `me` is never missed.
fn forget(
    members: &mut BTreeSet<String>,
    missed: &mut BTreeMap<String, Instant>,
    found: &BTreeSet<String>,
    me: &str,
    now: Instant,
    after: Duration,
) -> Vec<String> {
    members.extend(found.iter().cloned());
    missed.retain(|m, _| !found.contains(m) && members.contains(m));
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

/// The merge's checks of a hand-off (docs/pool.md §7.3): its done record is its own step's
/// and names targets; every change is to a logical name, as a content name of it, and within its
/// step's outputs (crate::agent::steps::outside: the function its job's `Out` wrote by, so an entry
/// it refuses is one an older app's job, or a damaged one, made); each raw tiles' archive it names
/// is one of its shard. A shared
/// step's (what a helper's coordinator checked of its hand-off: crate::coord, `check_handoff`):
/// every change is to a file its step saves for one of the targets it did, every upload it says
/// is pending one of its saves, every one it says it checked one of those. (Not that its targets
/// are its lease's: a lead after the one that granted it may not know the lease.)
pub fn check(the_plan: &dyn crate::agent::seam::Plan, e: &Entry, _r: &Records) -> std::result::Result<(), String> {
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
    if let Some(why) = the_plan.outside(e) {
        return Err(why);
    }
    for (shard, p) in &h.raw {
        if !buildstore::rawpack::is_shard(shard) || !buildstore::rawpack::named_for(&p.name, shard) {
            return Err(format!("{} isn't an archive of {shard}", p.name));
        }
    }
    if the_plan.shared().contains(&e.step.as_str()) {
        let did: &[(String, String)] = h.done.as_ref().map_or(&[], |d| &d.1);
        for l in h.changes.keys() {
            if !did.iter().any(|(t, _)| the_plan.saves(&e.step, t, l)) {
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

/// A redo's entry (crate::agent::redo: it forgets records) from the lead merging it (`lead`, its
/// member: a redo is the lead's own job) alone: one written by any other member, a forged or copied
/// one, or a lead's before the owner moved the lead (its redo asked again), is refused.
pub fn by_its_lead(e: &Entry, lead: &str) -> std::result::Result<(), String> {
    if e.step != crate::agent::redo::STEP || e.member == lead {
        return Ok(());
    }
    Err(format!("a redo is the lead's: {} isn't the lead ({lead} is)", e.member))
}

/// `check`, by a lead on app `lead_app`, members' apps as `app_of` reads them (their heartbeats;
/// None: not read): an entry with a change outside its step's write-set from a member on a newer
/// app than the lead's is held (`records::HOLD`), neither merged nor refused, until the lead runs
/// an app as new (its step may write what this app's write-sets don't know of: refused, its work
/// would be done again, and refused again, until the lead updates). As the coordinator ends such a
/// member's lease rather than hold its failure against its targets (crate::coord). An entry whose
/// member's app can't be read now is held too (that says nothing of it); one from a member on the
/// lead's app, or an older one, is refused.
pub fn check_from(
    the_plan: &dyn crate::agent::seam::Plan,
    e: &Entry,
    r: &Records,
    lead_app: &str,
    app_of: &dyn Fn(&str) -> Option<String>,
) -> std::result::Result<(), String> {
    match check(the_plan, e, r) {
        Err(why) if the_plan.outside(e).as_deref() == Some(why.as_str()) => match app_of(&e.member) {
            Some(app) if app == lead_app || crate::pool::lead::app_at_least(lead_app, &app) => Err(why),
            Some(app) => Err(format!(
                "{}{why}; its member runs app {app}, newer than this lead's ({lead_app}): held until the lead runs it",
                crate::pool::records::HOLD
            )),
            None => Err(format!("{}{why}; its member's app can't be read now", crate::pool::records::HOLD)),
        },
        other => other,
    }
}

/// An event in words, for the log.
pub fn said(e: &Event) -> String {
    match e {
        Event::TookUp { epoch, how } => format!("took up the records in epoch {epoch}, from {how}"),
        Event::SteppedDown { epoch, why } => format!("no longer leads (epoch {epoch}): {why}"),
        Event::Merged { applied, overtaken, refused, .. } => format!(
            "merged {} entr{} ({overtaken} passed over, {refused} refused)",
            applied.len(),
            if applied.len() == 1 { "y" } else { "ies" }
        ),
        Event::Waits { what, why } => format!("waits to {what}: {why}"),
        Event::Failed { what, why } => format!("couldn't {what}: {why}"),
    }
}

/// What a process of the agent is in the pool while it's on: decided as it starts, by the lead's
/// declaration (`Run::open`), and kept for its life. A process whose member takes the records up,
/// or stops leading, restarts into its new part once its first job's slot is free (phase 1: the lead's jobs run
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
    /// The epoch it leads in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leads: Option<u64>,
    /// Leading: grant jobs and plan.
    pub duties: bool,
    /// Leading: its records reflect what's handed off (a catalogue may go out).
    pub caught_up: bool,
    /// Leading: its step's own read of the declaration found it the lead (a sweep may run, caught
    /// up).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub vouched: bool,
    /// Leading: the entries it holds, with why (`check_from`: from a member on a newer app,
    /// outside this app's write-sets), neither merged nor refused until it runs that app.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<(String, String)>,
}

impl Gates {
    fn of(o: &Out) -> Gates {
        Gates { leads: o.leads, duties: o.duties, caught_up: o.caught_up, vouched: o.vouched, held: o.held.clone() }
    }

    /// A catalogue may be published now.
    pub fn publish(&self) -> bool {
        self.duties && self.caught_up
    }

    /// A sweep (GC) may run now: on a step whose read of the declaration found it the lead, caught
    /// up.
    pub fn sweep(&self) -> bool {
        self.duties && self.caught_up && self.vouched
    }
}

/// The steps that are sweeps (GC: the lead's, once vouched for and caught up).
pub const SWEEPS: [&str; 1] = ["gc"];
/// The steps that publish (a catalogue: the lead's, once caught up).
pub const PUBLISHES: [&str; 2] = ["catalog", "catalog-held"];
/// Lease numbers at and above this are hand-offs a lead's coordinator journaled on its disk, drained
/// into its member's hand-offs (`Run::drain`; epoch 0: no lead's). Below it, a job
/// run out of the pool keeps the lease the lead's coordinator gave it (`Run::drain_outbox`).
pub const DRAINED: u64 = 1 << 62;

/// The agent's part in the pool while it's on, in this process.
pub struct Run {
    pub the_plan: &'static dyn crate::agent::seam::Plan,
    pub side: Side,
    pub role: Role,
    pub gates: Gates,
    /// Its jobs' hand-offs for the next step, with the folders or files holding them.
    pub entries: Vec<(Entry, Option<PathBuf>)>,
    /// Why it restarts once its first slot is free (it left the pool, or its coordinator didn't
    /// start as it took the records up).
    pub restart: Option<String>,
    /// What it drained already in this process (handed, not yet removed), and the drained hand-offs'
    /// last number.
    drained: BTreeSet<PathBuf>,
    drain_n: u64,
    /// Whether the jobs an earlier process left were taken (`take_left`: once a process).
    left_taken: bool,
    /// The raw tiles' archives named in the stores' indexes and not yet off the records
    /// (`Give::named_raw`).
    pub named_raw: BTreeSet<String>,
    /// The outbox's jobs drained (once a process: `Run::drain_outbox`), and the hand-off folders'
    /// last files drained, to mark merged (handoff.rs's `<folder>.merged`)
    /// once a saved state holds them.
    outbox_drained: bool,
    marks: Vec<(PathBuf, String)>,
    /// The page's regions as last handed to its coordinator (`/pool/view`): what they were made of.
    pub regions_handed: Option<super::RegionsKey>,
    /// The history's last event written.
    pub history_seq: u64,
    /// The members in touch at the lead's last read of their heartbeats (its time; their ids and
    /// hosts), for the history's joins and leaves (crate::agent::lead::membership).
    pub in_touch: Option<(u64, BTreeMap<String, String>)>,
    /// This process led before (a take-up after it, its member declared again, keeps its
    /// coordinator's state; its first loads another Mac's: `took_up`).
    pub led: bool,
    /// The build page's asks for the next step, and this Mac's conditions for its heartbeat.
    pub asks: Vec<driver::Ask>,
    pub conds: Option<Conds>,
    /// GC's queue, leading (its map's log open): the merge refuses an entry bringing a name deleted
    /// under its lease (super::gcqueue::refusal, docs/maplog.md §7.4).
    pub gc: Option<std::sync::Arc<store::map::gc::GcQueue>>,
    /// The view of the lead and members (crate::agent::lead).
    pub controls: super::lead::Controls,
    /// What a tick needs of the agent (`Cx`), as its loop left it.
    pub cx: Cx,
    /// The agent's loop is stepping it (its step and what follows it, `in_step` until it's done):
    /// no tick meanwhile.
    pub in_step: bool,
    /// The awake time (its driver's clock, s) of its last step, and of the agent's loop's last.
    last_step: Option<u64>,
    last_full: Option<u64>,
    /// Take-ups a tick made whose coordinator's part is the agent's (`took_up`: another Mac's state
    /// loaded, its own host's leases from before dropped), at its loop's next step.
    pub pending_took_up: Vec<Event>,
    /// Tasks brokered by every member: when the members' brokers were last read from their
    /// heartbeats, and this Mac's last given to its own (`tend_brokers`).
    pub brokers_read: Option<Instant>,
    pub broker_told: Option<Instant>,
    /// Whether a tick holds the run, and since when: read without the lock (`Ticking`).
    pub ticking: Ticking,
    /// The members' coordinators tried (crate::pool::reachable).
    pub prober: Prober,
    /// The lead's member, and when this Mac first saw it lead (its own clock): a request that
    /// failed before then was another lead's.
    lead_seen: Option<(String, u64)>,
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
    pub root: Option<store::nasfs::NasRoot>,
    /// The agent's folder.
    pub home: PathBuf,
    /// This process's coordinator (the lead's), for what the lead's step asks of it: whether it
    /// grants (`set_moving`), its epoch.
    pub coord: Option<Arc<crate::coord::Coordinator>>,
    /// When this Mac last had an answer from the lead's coordinator (its clock, unix s): a member's
    /// client's (crate::coord::client::Client::answered).
    pub lead_answered: Option<u64>,
    /// This Mac's broker (docs/pool.md §8, crate::agent::brokers): the lead's coordinator, or a
    /// member's broker; None with tasks brokered by every member off. Its address over HTTPS, when
    /// `tailscale serve` proxies it. Its LAN addresses (coordwire::net::lan_urls).
    pub broker: Option<crate::coord::Brokering>,
    pub broker_https: Option<String>,
    pub broker_lan: Vec<String>,
    /// Where this Mac's client reaches the lead's coordinator (its contact on the NAS), and when a
    /// request of its last reached none of them, and why: what it tries, and a failure it counts
    /// (crate::pool::reachable).
    pub lead_urls: Vec<String>,
    pub lead_failed: Option<(u64, String)>,
    /// The key its client calls the lead's coordinator with (the contact's): its mail's posts too.
    pub lead_token: String,
    /// The records' brief as the agent last had it from the lead (crate::agent::brief), its jobs' and
    /// its view's alike: one copy a Mac.
    pub briefs: Arc<super::brief::Briefs>,
}

/// How a member tries another's coordinator: at its addresses, reached or not and why
/// (crate::coord::client::probe; a test's own).
pub type ProbeFn =
    Arc<dyn Fn(&[String]) -> std::result::Result<(), (crate::pool::reachable::Cause, String)> + Send + Sync>;

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
/// while (`TICK_S`, `QUIET_S`), so the heartbeat, the mail, the build page's asks and the view go
/// on while the agent plans or tends its jobs. Stopped (and
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

/// When the declared lead's coordinator last answered this Mac (`answered`, its client's): only an
/// answer saying it leads (an epoch, from that member: crate::coord::POOL_EPOCH, POOL_MEMBER); one
/// from another coordinator (a member's broker, an earlier lead's not yet stopped) says nothing of
/// it.
pub fn lead_answered(answered: Option<&crate::coord::client::Answered>, lead: Option<&str>) -> Option<u64> {
    let (a, l) = (answered?, lead?);
    (a.epoch.is_some_and(|e| e > 0) && a.member.as_deref() == Some(l)).then_some(a.at)
}

/// What the coordinator answers asks for jobs with, by the gates (None: it grants): its take-up's
/// state not loaded yet (`pending`), its member's lock or the declaration not read lately (`driver::VOUCH_S`), or
/// no longer leading.
pub fn moving(g: &Gates, pending: bool) -> Option<String> {
    match () {
        _ if g.leads.is_some() && pending => Some("taking the records up".into()),
        _ if g.leads.is_some() && g.duties => None,
        _ if g.leads.is_some() => {
            Some("not sure it leads: its member's lock, or the declaration, not read lately".into())
        }
        _ => Some("no longer leads".into()),
    }
}

impl Run {
    pub fn new(the_plan: &'static dyn crate::agent::seam::Plan, side: Side, role: Role, gates: Gates) -> Run {
        let controls = super::lead::Controls::open(&side.dir);
        Run {
            the_plan,
            side,
            role,
            gates,
            entries: Vec::new(),
            restart: None,
            drained: BTreeSet::new(),
            drain_n: 0,
            left_taken: false,
            named_raw: BTreeSet::new(),
            outbox_drained: false,
            marks: Vec::new(),
            regions_handed: None,
            history_seq: 0,
            in_touch: None,
            led: false,
            asks: Vec::new(),
            gc: None,
            conds: None,
            controls,
            cx: Cx::default(),
            in_step: false,
            last_step: None,
            last_full: None,
            pending_took_up: Vec::new(),
            ticking: Ticking::default(),
            brokers_read: None,
            broker_told: None,
            prober: Prober::default(),
            lead_seen: None,
        }
    }

    /// The records' brief for its view (crate::agent::brief::Briefs::now): its coordinator's,
    /// leading; a member's lead's over HTTP, while it answers the member's beats (else the last one
    /// had); the NAS's when that's the one.
    pub fn brief(&self) -> anyhow::Result<Arc<crate::pool::records::Brief>> {
        use super::brief::Lead;
        let root = self.cx.root.clone().context("no NAS")?;
        if let Some(c) = self.cx.coord.as_deref().filter(|_| self.side.driver().leads().is_some()) {
            return self.cx.briefs.now(Lead::Here(c), &root);
        }
        if self.side.driver().declared().is_none() || self.cx.lead_urls.is_empty() {
            return self.cx.briefs.now(Lead::None, &root);
        }
        if !self.side.http.lead_answering(self.side.now()) {
            if let Some(h) = self.cx.briefs.held() {
                return Ok(h.brief);
            }
        }
        let host = self.side.member().host.clone();
        let c = crate::coord::client::Client::at(self.cx.lead_urls.clone(), self.cx.lead_token.clone(), &host);
        self.cx.briefs.now(Lead::At(&c), &root)
    }

    /// Whether the pool's own thread steps it now: the agent's loop isn't, no step has run for
    /// `TICK_S`, the loop's last was `QUIET_S` ago or more, and the NAS was reachable then.
    pub fn tick_due(&self) -> bool {
        let now = self.side.io().awake();
        let since = |t: Option<u64>, s: u64| t.is_none_or(|t| now.saturating_sub(t) >= s);
        !self.in_step
            && self.side.stopped.is_none()
            && self.cx.root.is_some()
            && since(self.last_step, TICK_S)
            && since(self.last_full, QUIET_S)
    }

    /// The agent's loop's step is done (its after-effects too): the view written, ticks again.
    pub fn stepped_full(&mut self) {
        self.last_full = Some(self.side.io().awake());
        self.in_step = false;
        self.write_view();
    }

    /// A step of the pool's own thread (`Ticker`), the agent's loop busy: what the loop's step does
    /// that's the pool's alone. The build page's asks taken; the driver's step, its heartbeat and
    /// mail with it; the history and the view; the coordinator told whether it grants. Its first
    /// take-up in this process is left to the loop's next step for the coordinator's part
    /// (`pending_took_up`: another Mac's state, its leases), the coordinator granting nothing until
    /// then; a change of this process's part is the loop's too (its next
    /// step's `change_part`). None when there was nothing to step (no NAS).
    pub fn tick(&mut self) -> Option<Out> {
        let root = self.cx.root.clone()?;
        let home = self.cx.home.clone();
        let coord = self.cx.coord.clone();
        self.tend_brokers();
        self.tend_reach();
        let out = self.step();
        // (When its checks ran: the coordinator's grants count from then, not from the tell.)
        let checked = crate::coord::wall_now();
        for e in &out.events {
            let Event::TookUp { epoch, .. } = e else { continue };
            match &coord {
                Some(c) if self.led => c.set_epoch(*epoch),
                _ => self.pending_took_up.push(e.clone()),
            }
        }
        if let (Some(c), Some(e), true) = (&coord, out.leads, self.pending_took_up.is_empty()) {
            c.set_epoch(e);
        }
        super::lead::after(self, &out, &root, &home, coord.as_deref());
        if let Some(co) = &coord {
            co.set_moving_as_of(moving(&self.gates, !self.pending_took_up.is_empty()), checked);
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
        let own = crate::coord::brokers::Broker {
            urls: c.urls().to_vec(),
            https: self.cx.broker_https.clone(),
            lan: self.cx.broker_lan.clone(),
            pages: super::proxy_pages(),
        };
        if brokers::due(self.side.broker.as_ref(), &own, self.broker_told.map(|t| now.saturating_duration_since(t))) {
            self.side.broker = Some(own.clone());
            self.broker_told = Some(now);
        }
        if brokers::stale(self.brokers_read, now, brokers::EVERY) {
            let side = &self.side;
            let me = side.member().clone();
            let lead = side.driver().declared().map(str::to_string);
            let mut ids: Vec<String> = side.members().iter().cloned().collect();
            if let Some(l) = lead.as_ref().filter(|l| !ids.contains(l)) {
                ids.push(l.clone());
            }
            let beats: Vec<(String, Heartbeat)> = ids
                .into_iter()
                .filter(|id| *id != me.id)
                .filter_map(|id| side.heartbeat(&id).map(|h| (id, h)))
                .collect();
            let peers = brokers::peers(&beats, lead.as_deref(), side.now(), (&me.id, &me.host, &own));
            c.set_brokers(Some(crate::coord::brokers::Brokers::new(peers, &own)));
            self.brokers_read = Some(now);
        }
    }

    /// Whom this Mac reaches (docs/pool.md §6.8, crate::pool::reachable), in each of its steps,
    /// before its heartbeat is written: the outcome of its client's requests to the lead's
    /// coordinator counted (an answer from it, as the declared lead's: `Cx::lead_answered`; a
    /// request that reached none of its addresses: `Cx::lead_failed`), and every `PROBE_S` a round
    /// of tries, off the loop, of the current lead's coordinator (its broker's addresses in its
    /// heartbeat, else the contact's) and the other members' brokers in touch; the round before's
    /// outcomes taken in. The heartbeat says what the tracker reports.
    pub fn tend_reach(&mut self) {
        use crate::pool::reachable::PROBE_S;
        let now = self.side.now();
        let me = self.side.member().id.clone();
        let lead = self.side.driver().declared().filter(|l| *l != me).map(str::to_string);
        if self.lead_seen.as_ref().map(|(l, _)| l) != lead.as_ref() {
            self.lead_seen = lead.clone().map(|l| (l, now));
        }
        let seen = self.lead_seen.as_ref().map_or(now, |(_, at)| *at);
        if let Some(l) = &lead {
            if let Some(a) = self.cx.lead_answered {
                self.prober.tracker.seen(l, "coordinator", Ok(()), a);
            }
            // (A failure since this Mac first saw it lead: one before was another lead's.)
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
            let read: Vec<Option<Heartbeat>> = ids.iter().map(|id| self.side.heartbeat(id)).collect();
            self.side.all_check_map = read.iter().all(|h| h.as_ref().is_some_and(|h| h.pool.checks_map));
            self.side.beats = read.into_iter().flatten().map(|h| h.pool).collect();
            self.side.beats_at = Some(now);
            if let Some(l) = &lead {
                let urls = self
                    .side
                    .heartbeat(l)
                    .and_then(|h| h.broker)
                    .map(|b| b.urls)
                    .filter(|u| !u.is_empty())
                    .unwrap_or_else(|| self.cx.lead_urls.clone());
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
            let run = move || -> Tries {
                targets
                    .into_iter()
                    .map(|(m, what, urls)| {
                        let r = probe(&urls);
                        (m, what, r)
                    })
                    .collect()
            };
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

    /// This member as the status shows it (crate::agent::PoolView).
    pub fn pool_view(&self) -> super::PoolView {
        super::PoolView {
            member: self.side.member().id.clone(),
            role: self.role,
            gates: self.gates.clone(),
            members: self.side.members().iter().cloned().collect(),
            unacked: self.side.driver().mine().unacked(),
            restart: self.restart.clone(),
            lead: self.controls.view.clone(),
            all_check_map: self.side.all_check_map,
        }
    }

    /// The view (`pool_view`) with when it was written (this Mac's clock), to the agent's pool
    /// folder (`VIEW`): what the controls read while the agent's status waits for its loop.
    pub fn write_view(&self) {
        let v = PoolFile { beat: self.side.now(), pool: self.pool_view() };
        let r = serde_json::to_vec(&v)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.side.dir.join(VIEW), &b));
        if let Err(e) = r {
            eprintln!("pool: writing its view: {e:#}");
        }
    }

    /// A process's first step, which says its part (`Role`). (The jobs an earlier process left are
    /// handed over later, `take_left`, once the agent has stopped any still running.)
    pub fn start(the_plan: &'static dyn crate::agent::seam::Plan, side: Side, _home: &Path) -> (Run, Out) {
        let mut r = Run::new(the_plan, side, Role::Member, Gates::default());
        let out = r.step();
        r.role = if out.leads.is_some() { Role::Lead } else { Role::Member };
        r.restart = out.stop.as_ref().map(|why| format!("this process left the pool: {why}"));
        (r, out)
    }

    /// One step: what the agent gathered handed over, the gates kept; the role's change noted (it
    /// restarts into its new part).
    pub fn step(&mut self) -> Out {
        // (Its members' posts to its coordinator, when it leads, go to its inbox; its own to the
        // lead's coordinator as its contact says.)
        if let Some(co) = &self.cx.coord {
            co.set_inbox(Some(self.side.inbox()));
            co.set_beats(Some(self.side.http_beats()));
            co.set_handoffs(Some(self.side.handoffs()));
            self.side.epochs = Some(epochs(co.clone(), self.side.shared_nas(), self.side.member().host.clone()));
        }
        // (The members heard by HTTP known at once: their mail read, their messages sent.)
        for h in self.side.http.fresh(self.side.now()) {
            self.side.know(&h.beat.pool.member);
        }
        self.side.lead_contact = (self.cx.lead_urls.clone(), self.cx.lead_token.clone());
        let give = Give {
            entries: std::mem::take(&mut self.entries),
            asks: std::mem::take(&mut self.asks),
            conds: self.conds,
            named_raw: self.named_raw.iter().cloned().collect(),
        };
        let marks = std::mem::take(&mut self.marks);
        // (The merge's checks, an entry outside its step's write-set from a member on a newer app
        // held: `check_from`. Its member's app read from its heartbeat, only for such an entry.)
        let (lead_app, me) = (self.side.member().app.clone(), self.side.member().id.clone());
        let (beats, now) = (self.side.http_beats(), self.side.now());
        let app_of = |m: &str| -> Option<String> {
            if m == me {
                return Some(lead_app.clone());
            }
            beats.get(m, now).map(|h| h.pool.app)
        };
        let gc = self.gc.clone();
        let checked = |e: &Entry, r: &Records| {
            by_its_lead(e, &me)?;
            check_from(self.the_plan, e, r, &lead_app, &app_of)?;
            // (Last: one it lets merge holds its names from GC's chunks, docs/maplog.md §7.4.)
            match gc.as_deref().and_then(|q| super::gcqueue::refusal(q, e, crate::coord::wall_now())) {
                Some(why) => Err(why),
                None => Ok(()),
            }
        };
        let out = self.side.step(give, &checked);
        // (Named and off the records now, or no longer leading: forgotten here.)
        let raw: BTreeSet<&str> = self
            .side
            .driver
            .as_ref()
            .and_then(Driver::records)
            .map(|r| r.raw.iter().map(|(_, p)| p.name.as_str()).collect())
            .unwrap_or_default();
        self.named_raw.retain(|n| raw.contains(n.as_str()));
        self.last_step = Some(self.side.io().awake());
        // (Drained hand-offs held by the state just saved: their folders marked merged, so one whose
        // removal failed isn't drained again by a later process.)
        let kept = out.stop.is_none()
            && !out.events.iter().any(|e| matches!(e, Event::Failed { what: "save the member's state", .. }));
        if kept {
            for (dir, name) in marks {
                let marker = dir.with_extension("merged");
                let was = std::fs::read_to_string(&marker).unwrap_or_default();
                if name.as_str() > was.trim() {
                    if let Err(e) = store::whole::write(&marker, name.as_bytes()) {
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
    /// next step: once a process, after the agent stopped those still running; but those it took
    /// over (`running`: their folders), which hand off as they end.
    pub fn take_left(&mut self, home: &Path, running: &[PathBuf]) {
        if std::mem::replace(&mut self.left_taken, true) {
            return;
        }
        let me = self.side.member().id.clone();
        self.entries.extend(left_jobs(self.the_plan, home, &me, running).into_iter().map(|(e, d)| (e, Some(d))));
    }

    /// The next number for a drained hand-off (`DRAINED` on, by the clock).
    fn next_drained(&mut self) -> journal::LeaseId {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.drain_n = (self.drain_n + 1).max(ms);
        journal::LeaseId { epoch: 0, n: DRAINED + self.drain_n }
    }

    /// The hand-offs a lead's coordinator journaled on its disk (`journal`), drained into this
    /// member's hand-offs: those a member out of the pool told it (`journaled: false`, its
    /// member's lock held by another process), which the coordinator wrote whole before ending the
    /// lease. Each is removed once a saved state holding its entry is on disk.
    pub fn drain(&mut self, journal: &Path) {
        let me = self.side.member().id.clone();
        let found = match buildstore::handoff::waiting_in(journal) {
            Ok(hs) => hs,
            Err(e) => {
                eprintln!("pool: draining {}: {e:#}", journal.display());
                return;
            }
        };
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
            self.entries.push((
                Entry { member: me.clone(), lease, step, handoff: h, at: crate::agent::jobs::now_s() },
                Some(p),
            ));
        }
    }

    /// The jobs a process of this agent ran out of the pool (its member's lock held by another
    /// process, or the NAS not reached as it started) and left in its outbox (`outbox/<lease>/`, a
    /// `jobs::Work` kept with its saves): their hand-offs not told to the lead (the process ended
    /// first, the lead refused it, or it was given back or failed), drained into this member's
    /// hand-offs, once a process. Each folder is removed once a saved state holding its entry
    /// is on disk; until then send_outbox passes over it (it has `work.json`) and the member asks
    /// for no task.
    pub fn drain_outbox(&mut self, outbox: &Path) {
        if std::mem::replace(&mut self.outbox_drained, true) {
            return;
        }
        let me = self.side.member().id.clone();
        for (e, dir) in outbox_entries(self.the_plan, outbox, &me) {
            if self.drained.insert(dir.clone()) {
                self.entries.push((e, Some(dir)));
            }
        }
    }
}

/// A take-up (`Event::TookUp`), as the lead's coordinator takes it (§6.2, §7.5): the leases it
/// grants from now in that epoch; at this process's first, another Mac's state, when the store's
/// backup on the NAS is another Mac's (the owner moved the lead here: its leases, so every job out
/// goes on renewing), loaded, and its own host's leases from before dropped (its jobs ended with
/// the process that ran them), but those of the jobs this process runs (`keep`). A take-up after
/// one in this process (its member declared again) keeps what it has: it's newer than the files.
///
/// `events`: a step's, with those a tick left it (`Run::pending_took_up`); `leads`: the epoch the
/// last step says this member leads in.
pub fn took_up(
    run: &mut Run,
    coord: Option<&crate::coord::Coordinator>,
    events: &[Event],
    leads: Option<u64>,
    host: &str,
    keep: &[u64],
) {
    let Some(c) = coord else { return };
    for e in events {
        let Event::TookUp { epoch, .. } = e else { continue };
        c.set_epoch(*epoch);
        if std::mem::replace(&mut run.led, true) {
            continue;
        }
        // (Its own store holds its state (crate::coord::kept); another Mac's lead before it, the lead
        // moved by the owner, left its store's backup on the NAS: taken up.)
        let backup = match crate::pool::store::read_backup(run.side.nas()) {
            Ok(b) => b.filter(|b| b.member != host),
            Err(e) => {
                eprintln!("pool: the lead's store's backup: {e:#}");
                None
            }
        };
        if let Some(b) = backup {
            let st = crate::coord::kept::Loaded::of(&b.docs).pool_state();
            match c.load_pool_state(&st) {
                Ok(n) => eprintln!(
                    "pool: epoch {epoch}'s coordinator took up {n} lease{} from {}'s store",
                    if n == 1 { "" } else { "s" },
                    b.member
                ),
                Err(e) => eprintln!("pool: the coordinator's state for epoch {epoch}: {e:#}"),
            }
        }
        for l in c.drop_workers(&crate::agent::slot_workers(host), keep) {
            eprintln!("pool: {}'s lease ended with the process before this one", l.what());
        }
    }
    if let Some(e) = leads {
        c.set_epoch(e);
    }
}

/// Where a job of lease `lease` keeps its hand-off while it runs (`SCENIC_HANDOFF`), its work and
/// done marks: `<home>/pool/jobs/<epoch>-<n>/`.
pub fn job_dir(home: &Path, lease: journal::LeaseId) -> PathBuf {
    home.join("pool/jobs").join(lease.to_string())
}

/// The hand-off of the job whose folder is `dir` (`job_dir`): its saves, in the order written,
/// and the targets it finished (`done`; none: it finished none). A shared step's changes only to
/// the files of the targets it finished (one it was on when it stopped may have saved part of its
/// own); another step's (the OSM pass's stages) all of them. None when it hands off nothing; an
/// error when one of its saves is damaged.
pub fn entry_of(
    the_plan: &dyn crate::agent::seam::Plan,
    dir: &Path,
    member: &str,
    lease: journal::LeaseId,
    step: &str,
    done: &[(String, String)],
    at: u64,
) -> Result<Option<Entry>> {
    let saves = buildstore::handoff::written_in(dir)?.context("one of its saves is damaged")?;
    let mut h = Handoff::default();
    for x in saves {
        h.absorb(x);
    }
    if the_plan.shared().contains(&step) {
        h.changes.retain(|l, _| done.iter().any(|(t, _)| the_plan.saves(step, t, l)));
        let kept: BTreeSet<String> = h.changes.values().flatten().cloned().collect();
        h.pending.retain(|c, _| kept.contains(c));
        h.sizes.retain(|c, _| kept.contains(c));
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
/// folder; a folder of no job, or whose saves are damaged, removed. Those in `running` (the jobs
/// this agent took over from the one before, still running: crate::agent::jobs::adopt) are left as
/// they are.
pub fn left_jobs(
    the_plan: &dyn crate::agent::seam::Plan,
    home: &Path,
    member: &str,
    running: &[PathBuf],
) -> Vec<(Entry, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(home.join("pool/jobs")) else { return Vec::new() };
    let mut out = Vec::new();
    for d in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !running.contains(p)) {
        let kept: Option<JobKept> =
            std::fs::read(d.join("work.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let Some(k) = kept else {
            std::fs::remove_dir_all(&d).ok();
            continue;
        };
        let names = jobrt::control::read_done(&d.join("done.txt"), &k.step);
        let done: Vec<(String, String)> = k.targets.iter().filter(|(t, _)| names.contains(t)).cloned().collect();
        match entry_of(the_plan, &d, member, k.lease, &k.step, &done, crate::agent::jobs::now_s()) {
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

/// A job folder in this member's outbox, run out of the pool (`Run::drain_outbox`): its hand-off as
/// its end would have told it (its saves and done record, or what it noted done when it stopped), as
/// an entry of this member under its lease from the lead's coordinator (epoch 0). A task's folder (no
/// `work.json`: its outputs went to its broker) is left to the outbox's own sending; a job that
/// handed off nothing, or whose saves are damaged, is removed (its work is done again), or this
/// member would never ask for a task again.
fn outbox_entries(the_plan: &dyn crate::agent::seam::Plan, outbox: &Path, member: &str) -> Vec<(Entry, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(outbox) else { return Vec::new() };
    let mut out = Vec::new();
    for d in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Some(lease) = d.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok()) else { continue };
        let Some(w) = std::fs::read(d.join("work.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<crate::agent::jobs::Work>(&b).ok())
        else {
            continue;
        };
        let result: Option<serde_json::Value> =
            std::fs::read(d.join("result.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let done: Vec<(String, String)> = match result.as_ref().filter(|r| r["ok"].as_bool() == Some(true)) {
            Some(r) => serde_json::from_value::<Option<(String, Vec<(String, String)>)>>(r["done"].clone())
                .ok()
                .flatten()
                .map(|d| d.1)
                .unwrap_or_default(),
            None => {
                let names = jobrt::control::read_done(&d.join("done.txt"), &w.step);
                w.targets.iter().filter(|(t, _)| names.contains(t)).cloned().collect()
            }
        };
        match entry_of(
            the_plan,
            &d,
            member,
            journal::LeaseId { epoch: 0, n: lease },
            &w.step,
            &done,
            crate::agent::jobs::now_s(),
        ) {
            Ok(Some(e)) => out.push((e, d)),
            Ok(None) | Err(_) => {
                std::fs::remove_dir_all(&d).ok();
            }
        }
    }
    out
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
                    store::whole::write(&here, &b)?;
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

/// The history's events appended to this member's file of their day,
/// `state/coord/history/<day>/<member>.jsonl` (§7.5: one file per writer; merged for reading by
/// time, writer and number). Through the share's path (an append: its one writer).
pub fn append_history(
    root: &store::nasfs::NasRoot,
    member: &str,
    events: &[crate::coord::history::Event],
) -> Result<()> {
    let mut by_day: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for e in events {
        let Some(day) = journal::day(e.t) else { continue };
        let b = by_day.entry(day).or_default();
        b.extend(serde_json::to_vec(e)?);
        b.push(b'\n');
    }
    for (day, b) in by_day {
        let p = root.join("state/coord/history").join(day).join(format!("{member}.jsonl"));
        p.parent().unwrap().create_dir_all()?;
        use std::io::Write;
        p.open_with(std::fs::OpenOptions::new().create(true).append(true))
            .and_then(|mut f| f.write_all(&b))
            .with_context(|| format!("append to {}", p.display()))?;
    }
    Ok(())
}

/// The agents' statuses: the lead's (`state/status.json`), then the members' as the lead has them
/// (its `helpers`: their beats, crate::coord::beats), each with where it's from.
fn statuses(root: &store::nasfs::NasRoot) -> Vec<(String, super::Status)> {
    let read = |p: &store::nasfs::NasPath| p.read().ok().and_then(|b| serde_json::from_slice::<super::Status>(&b).ok());
    let Some(mut lead) = read(&root.join("state/status.json")) else { return Vec::new() };
    let members = std::mem::take(&mut lead.helpers);
    std::iter::once(("state/status.json".to_string(), lead))
        .chain(members.into_iter().map(|h| ("the lead's view".to_string(), h)))
        .collect()
}

/// `scenic pool status`: the declared lead, the agents as their statuses say.
pub fn status(root: &store::nasfs::NasRoot) -> String {
    let mut out = Vec::new();
    let nas = root.clone();
    match crate::pool::lead::declared(&nas) {
        Ok(Some(d)) => out.push(format!("the declared lead: {} ({})", d.host, d.member)),
        Ok(None) => out.push("no lead declared".to_string()),
        Err(e) => out.push(format!("the lead's declaration: {e:#}")),
    }
    let now = crate::agent::jobs::now_s();
    for (f, st) in statuses(root) {
        let p = st.pool.as_ref().map_or("not in the pool".to_string(), |p| {
            format!(
                "{} {}, leads in epoch {:?}, duties {}, caught up {}, unacked {}{}{}",
                serde_json::to_value(p.role).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                p.member,
                p.gates.leads,
                p.gates.duties,
                p.gates.caught_up,
                p.unacked,
                p.gates
                    .held
                    .first()
                    .map(|(k, why)| format!(
                        ", {} entr{} held until it runs a newer app (the first: {k}: {why})",
                        p.gates.held.len(),
                        if p.gates.held.len() == 1 { "y" } else { "ies" }
                    ))
                    .unwrap_or_default(),
                p.restart.as_ref().map(|r| format!(", restarting: {r}")).unwrap_or_default()
            )
        });
        out.push(format!(
            "{} ({f}, app {}, beat {} s ago, job {}): {p}",
            st.host,
            st.app,
            now.saturating_sub(st.beat),
            st.jobs().first().map_or("none".to_string(), |j| j.id.clone())
        ));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use crate::coord::client::LeaseCalls;

    #[test]
    fn a_message_this_app_doesnt_know_is_read_past() {
        // (Task #409 review) a newer app's message among the others: passed over, the rest taken,
        // and read past (its number the top), so it holds nothing back and isn't read again.
        // (So are the messages of the apps before #881, "Leads", "HandTo", and before #1125, "Tell",
        // "Ack".)
        let b = br#"{"msgs": [[4, {"Tell": ["k"]}], [5, {"Leads": 3}], [6, {"FromTheFuture": {"x": 1}}], [7, {"Retry": {"target": "6/1/2"}}]]}"#;
        #[derive(serde::Deserialize)]
        struct Raw {
            msgs: Vec<(u64, serde_json::Value)>,
        }
        let raw = || serde_json::from_slice::<Raw>(b).unwrap().msgs;
        let (msgs, top) = super::after(raw(), 3);
        assert_eq!(
            msgs,
            vec![super::Msg::Retry(jobrt::control::Retry { target: "6/1/2".into(), ..Default::default() })]
        );
        assert_eq!(top, 7);
        let (msgs, top) = super::after(raw(), 7);
        assert!(msgs.is_empty() && top == 7);
    }

    use super::*;

    use crate::pool::journal::LeaseId;

    /// A build page's ask to retry target `t`, as a member passes it on to the lead.
    fn retry(t: &str) -> Msg {
        Msg::Retry(jobrt::control::Retry { target: t.into(), ..Default::default() })
    }

    /// A NAS folder as the pool begins: today's records, and this Mac named their writer.
    fn root(d: &Path) -> PathBuf {
        let r = d.join("nas");
        std::fs::create_dir_all(r.join("state/build")).unwrap();
        std::fs::create_dir_all(r.join(crate::agent::NAS_CHECK)).unwrap();
        std::fs::write(r.join("state/build/manifest.json"), r#"{"base/6-1-1": "base/6-1-1.1111111111111111.base"}"#)
            .unwrap();
        std::fs::write(r.join("state/build/jobs.json"), r#"{"unit": {"6/1/1": "k1"}}"#).unwrap();
        std::fs::write(r.join("state/build/pending.json"), "{}").unwrap();
        r
    }

    fn side(root: &store::nasfs::NasRoot, home: &Path) -> Side {
        side_over(Arc::new(root.clone()), home)
    }

    fn side_over(nas: SharedNas, home: &Path) -> Side {
        // (Its clocks virtual: time passes as the test says, `ahead`, however slowly a busy Mac
        // steps it.)
        let mut s = Side::open(home, &home.join("pool"), home.parent().unwrap(), "development", nas)
            .unwrap()
            .expect("its lock");
        s.set_clock(store::clock::Virtual::new());
        s
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Steps `s` until `done` says so (as long as a busy Mac takes: five minutes is a watchdog, not
    /// a measure).
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

    use store::testkit::WATCHDOG;

    const ME: &str = "m-0000000000000001";
    const B: &str = "m-0000000000000002";
    const PH: &str = "m-00000000000000ff";

    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_member_not_heard_is_forgotten_after_the_grace() {
        let t0 = Instant::now();
        let after = FORGET_AFTER;
        let (mut members, mut missed) = (set(&[ME, B, PH]), BTreeMap::new());
        // (Its beat gone: missed from then, kept until the grace has passed.)
        assert!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, t0, after).is_empty());
        assert!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, t0 + after - Duration::from_secs(1), after)
            .is_empty());
        assert!(members.contains(PH));
        assert_eq!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, t0 + after, after), vec![PH.to_string()]);
        assert_eq!(members, set(&[ME, B]));
        assert!(missed.is_empty());
    }

    #[test]
    fn a_member_found_again_is_no_longer_missed() {
        let t0 = Instant::now();
        let after = FORGET_AFTER;
        let (mut members, mut missed) = (set(&[ME, B]), BTreeMap::new());
        forget(&mut members, &mut missed, &set(&[ME]), ME, t0, after);
        // (Its beat back, as a member restarting beats again: the miss starts over.)
        forget(&mut members, &mut missed, &set(&[ME, B]), ME, t0 + after / 2, after);
        assert!(
            forget(&mut members, &mut missed, &set(&[ME]), ME, t0 + after + Duration::from_secs(1), after).is_empty()
        );
        assert!(members.contains(B));
        // (A Mac heard all along: never missed.)
        let (mut members, mut missed) = (set(&[ME, B]), BTreeMap::new());
        for h in 0..24 {
            assert!(forget(&mut members, &mut missed, &set(&[ME, B]), ME, t0 + Duration::from_secs(3600 * h), after)
                .is_empty());
        }
        assert!(missed.is_empty());
    }

    /// A member heard by its beat is known; one not heard for `FORGET_AFTER` is forgotten, and
    /// known again only once its beat is heard (not from another Mac's message or term naming it).
    #[test]
    fn a_forgotten_member_is_known_again_only_from_its_beat() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("home");
        let mut s = side(&store::nasfs::NasRoot::local_for_test(&r), &home);
        let now = s.now();
        let hb = |m: &str| Heartbeat {
            pool: Beat { member: m.into(), host: format!("{m}-host"), beat: now, ..Default::default() },
            ..Default::default()
        };
        s.http_beats().heard(hb(PH), Vec::new(), None, now);
        let t0 = s.clock.now();
        s.take_members(t0);
        assert!(s.members().contains(PH), "heard: known");
        s.post(PH, retry("6/1/2"));
        assert!(s.mail.sent.contains_key(PH));
        // (Silent for `beats::FORGET_S`: missed from then, forgotten after `FORGET_AFTER`.)
        s.http_beats().heard(hb(PH), Vec::new(), None, now.saturating_sub(crate::coord::beats::FORGET_S));
        s.take_members(t0);
        assert!(s.members().contains(PH));
        s.take_members(t0 + FORGET_AFTER);
        assert!(!s.members().contains(PH));
        // (Another Mac's message or term naming it, an older app re-advertising it: not again.)
        s.heard_of(PH);
        assert!(!s.members().contains(PH));
        let saved: BTreeSet<String> =
            serde_json::from_slice(&std::fs::read(home.join("pool/members.json")).unwrap()).unwrap();
        assert!(!saved.contains(PH));
        // (Its queued mail dropped, nothing posted to it, and so after a restart.)
        assert!(!s.mail.sent.contains_key(PH));
        s.post(PH, retry("6/1/2"));
        assert!(!s.mail.sent.contains_key(PH));
        drop(s);
        let mut s = side(&store::nasfs::NasRoot::local_for_test(&r), &home);
        assert!(!s.members().contains(PH));
        s.heard_of(PH);
        assert!(!s.members().contains(PH));
        // (Its beat heard again: known.)
        let now = s.now();
        s.http_beats().heard(hb(PH), Vec::new(), None, now);
        s.take_members(t0 + FORGET_AFTER * 2);
        assert!(s.members().contains(PH));
        let gone: BTreeSet<String> =
            serde_json::from_slice(&std::fs::read(home.join("pool/forgotten.json")).unwrap()).unwrap();
        assert!(gone.is_empty());
    }

    fn unit_entry(member: &str, lease: LeaseId, target: &str, key: &str) -> Entry {
        let l = format!("base/{}", target.replace('/', "-"));
        let h = Handoff {
            changes: [(l.clone(), Some(format!("{l}.2222222222222222.base")))].into(),
            done: Some(("unit".into(), vec![(target.into(), key.into())])),
            ..Default::default()
        };
        Entry { member: member.into(), lease, step: "unit".into(), handoff: h, at: crate::agent::jobs::now_s() }
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
        let pool = Beat {
            member: "m-000000000000000a".into(),
            host: "Mac-mini".into(),
            app: "20261008-1500-1fa03d8".into(),
            beat: 1000,
            leads: Some(3),
            ..Default::default()
        };
        let new = Heartbeat {
            pool: pool.clone(),
            members: vec!["m-000000000000000b".into()],
            conds: Some(Conds { home: true, ac: false, battery: Some(54), held: false }),
            broker: None,
            mail_http: true,
            beat_http: true,
        };
        let b = serde_json::to_vec(&new).unwrap();
        // An older app reads the newer's: its fields, the conditions passed over.
        let old: Before = serde_json::from_slice(&b).unwrap();
        assert_eq!((old.pool.clone(), old.members.clone()), (pool.clone(), new.members.clone()));
        assert_eq!(serde_json::from_slice::<Beat>(&b).unwrap(), pool);
        // A newer app reads the older's: no conditions ("conditions unknown" in the views).
        let back: Heartbeat = serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(back, Heartbeat { conds: None, mail_http: false, beat_http: false, ..new.clone() });
        assert!(crate::agent::lead::state_of(&back, 1000, None).starts_with("conditions unknown"));
        // And its own, back as written.
        assert_eq!(serde_json::from_slice::<Heartbeat>(&b).unwrap(), new);
    }

    #[test]
    fn a_member_steps_saves_its_state_and_beats() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut a = side(&store::nasfs::NasRoot::local_for_test(&r), &home);
        let id = a.member().id.clone();
        // Its first step: the first Mac in the pool, it declares itself and leads epoch 1, taken up
        // from today's records; caught up at once.
        let out = a.step(Give::default(), &any);
        assert_eq!((out.leads, out.duties, out.caught_up), (Some(1), true, true));
        // Its saved state on disk before anything else: naming it, the records it saved.
        let saved: Saved = serde_json::from_slice(&std::fs::read(home.join("pool/saved.json")).unwrap()).unwrap();
        assert_eq!((saved.member.as_str(), saved.records), (id.as_str(), Some((1, 1))));
        // Its heartbeat, for its agent's beat: the driver's fields and the members it knows; none
        // on the NAS.
        let hb = a.http_beats().own().unwrap();
        assert_eq!((hb.pool.leads, hb.members.clone()), (Some(1), vec![id.clone()]));
        assert!(hb.pool.beat > 0 && !r.join("state/pool/members").exists());
        // A member heard on an app before step 6 (its beat without `handoff`): nothing listed or
        // read for it (task #1125), caught up all the same.
        let older = Heartbeat {
            pool: Beat { member: B.into(), host: "b".into(), beat: a.now(), ..Default::default() },
            ..Default::default()
        };
        a.http_beats().heard(older, vec![crate::coord::held::SPEAKS.into()], None, a.now());
        let out = a.step(Give::default(), &any);
        assert!(out.caught_up && !r.join("state/journal").exists(), "{:?}", out.events);
    }

    #[test]
    fn a_member_whose_lock_another_process_holds_runs_no_driver_or_stops() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut a = side(&store::nasfs::NasRoot::local_for_test(&r), &home);
        let nas: SharedNas = Arc::new(store::nasfs::NasRoot::local_for_test(&r));
        assert!(
            Side::open(&home, &home.join("pool"), home.parent().unwrap(), "development", nas).unwrap().is_none(),
            "a second process of the member runs none"
        );
        a.step(Give::default(), &any);
        // Its lock file removed and taken by another process: it stops, its state saved, its
        // hand-offs kept in it, no heartbeat composed.
        let lock = home.parent().unwrap().join(format!("pool-{}.lock", a.member().id));
        std::fs::remove_file(&lock).unwrap();
        let _other = MemberLock::take(home.parent().unwrap(), &a.member().id).unwrap().unwrap();
        let beat = a.http_beats().own().unwrap().pool.beat;
        let e = unit_entry(&a.member().id, LeaseId { epoch: 1, n: 7 }, "6/1/2", "k2");
        let out = a.step(Give { entries: vec![(e, None)], ..Default::default() }, &any);
        assert!(out.stop.is_some() && a.stopped.is_some());
        let saved: Saved = serde_json::from_slice(&std::fs::read(home.join("pool/saved.json")).unwrap()).unwrap();
        assert_eq!(saved.mine.keys().count(), 1, "its hand-off kept for the next process");
        assert_eq!(a.http_beats().own().unwrap().pool.beat, beat);
        assert!(a.step(Give::default(), &any).stop.is_some(), "it steps no more");
    }

    /// The lead's tailnet address as its contact says it, in the mail tests: posts to it reach
    /// the test's coordinator on loopback (`to_loopback`).
    const LEAD_URL: &str = "http://100.64.0.1:8090";

    /// A courier over HTTP to the coordinator on `port` in place of the lead's tailnet address,
    /// counting its posts.
    fn to_loopback(port: u16, posts: Arc<std::sync::atomic::AtomicUsize>) -> Courier {
        let http = http_courier();
        Arc::new(move |urls, token, body| {
            posts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(urls, [LEAD_URL], "the tailnet's addresses alone");
            http(&[format!("http://127.0.0.1:{port}")], token, body)
        })
    }

    /// Lead A with its coordinator (its member's, its inbox, its beats), and member B posting to it.
    struct Mail {
        d: tempfile::TempDir,
        r: PathBuf,
        a: Side,
        b: Side,
        c: crate::coord::Coordinator,
        port: u16,
        posts: Arc<std::sync::atomic::AtomicUsize>,
    }

    fn mail_pool() -> Mail {
        mail_pool_over(|n| n)
    }

    /// `mail_pool`, A's NAS as `over` makes it of the share's.
    fn mail_pool_over(over: impl FnOnce(SharedNas) -> SharedNas) -> Mail {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let (mut a, mut b) = (
            side_over(over(Arc::new(store::nasfs::NasRoot::local_for_test(&r))), &d.path().join("a/agent")),
            side(&store::nasfs::NasRoot::local_for_test(&r), &d.path().join("b/agent")),
        );
        assert_eq!(until(&mut a, || Give::default(), |o| o.caught_up).leads, Some(1));
        let (c, port) = crate::coord::start_for_test(&d.path().join("a/coord"), "a", "");
        c.set_member(Some(&a.member().id));
        c.set_inbox(Some(a.inbox()));
        c.set_beats(Some(a.http_beats()));
        let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        b.courier = to_loopback(port, posts.clone());
        b.lead_contact = (vec![LEAD_URL.into(), "http://mac-a.local:8090".into()], c.contact.token.clone());
        Mail { d, r, a, b, c, port, posts }
    }

    impl Mail {
        /// B's beat taken by A's coordinator, its answer B's, and the hand-offs it says it lacks
        /// posted (as B's agent's beat does).
        fn beat(&mut self) -> crate::coord::held::Answer {
            use crate::coord::{beats, held};
            let b = held::Beat {
                member: "b-host".into(),
                speaks: [held::SPEAKS, beats::SPEAKS, beats::VIEW, beats::ASKS, beats::HANDOFF]
                    .map(str::to_string)
                    .to_vec(),
                heartbeat: self.b.http_beats().own(),
                handoffs: self.b.http_beats().handoff_keys(),
                ..Default::default()
            };
            let a = self.c.member_beat(&b);
            self.b.http_beats().answered(&a, self.b.now());
            for e in self.b.http_beats().handoffs_of(&a.lacks) {
                self.post(&e);
            }
            a
        }

        /// Hand-off `e` posted to A's coordinator by HTTP: taken.
        fn post(&self, e: &Entry) {
            let c = crate::coord::client::Client::at(
                vec![format!("http://127.0.0.1:{}", self.port)],
                self.c.contact.token.clone(),
                "b-host",
            );
            let (code, v) = c.post_json("/pool/handoff", &serde_json::json!({ "entry": e })).unwrap();
            assert_eq!((code, v["taken"].as_str()), (200, e.key().as_deref()), "{v}");
        }

        /// A's coordinator takes hand-offs (as its agent's step makes it), and B's beat hears so.
        fn by_http(&mut self) {
            self.c.set_handoffs(Some(self.a.handoffs()));
            self.b.step(Give::default(), &any);
            assert!(self.beat().speaks.iter().any(|w| w == crate::coord::beats::HANDOFF));
        }

        /// B's job's hand-off `n`, handed to its step: kept whole, nothing written nor sent.
        fn hand_off(&mut self, n: u64) -> Entry {
            let ib = self.b.member().id.clone();
            let e = unit_entry(&ib, LeaseId { epoch: 1, n }, &format!("6/1/{n}"), &format!("k{n}"));
            let out = self.b.step(Give { entries: vec![(e.clone(), None)], ..Default::default() }, &any);
            let key = e.key().unwrap();
            assert!(out.send.is_empty() && !self.r.join("state/journal").exists(), "{:?}", out.send);
            assert_eq!(self.b.http_beats().handoff_keys(), [key]);
            e
        }

        /// A steps until it acknowledges `key` (in a saved snapshot), then B's beat hears it and B
        /// drops it.
        fn merged(&mut self, key: &str) {
            until(&mut self.a, || Give::default(), |o| o.merged.iter().any(|k| k == key));
            assert!(self.a.driver().records().unwrap().handles(key));
            assert_eq!(self.beat().merged, [key.to_string()]);
            self.b.step(Give::default(), &any);
            assert_eq!(self.b.driver().mine().unacked(), 0, "dropped once merged");
        }

        /// B's page's ask to retry target `t`, passed on to A by mail (its step sends it).
        fn ask(&mut self, t: &str) -> String {
            let ia = self.a.member().id.clone();
            self.b.post(&ia, retry(t));
            self.b.step(Give::default(), &any);
            t.to_string()
        }

        /// A steps until it acts on B's ask to retry target `t`.
        fn acted(a: &mut Side, t: &str) {
            until(
                a,
                || Give::default(),
                |o| o.acts.iter().any(|x| matches!(x, driver::Act::Retry(r) if r.target == t)),
            );
        }

        fn posts(&self) -> usize {
            self.posts.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// A NAS whose lead's declaration doesn't answer while `hung` (a share stuck on it); every other
    /// call the share's.
    struct DeclarationHung(SharedNas, Arc<std::sync::atomic::AtomicBool>);

    impl DeclarationHung {
        fn check(&self, path: &str) -> Result<()> {
            anyhow::ensure!(
                !(path == crate::pool::lead::LEAD && self.1.load(std::sync::atomic::Ordering::SeqCst)),
                "{path}: the share doesn't answer"
            );
            Ok(())
        }
    }

    impl Nas for DeclarationHung {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<crate::pool::nas::Created> {
            self.check(path)?;
            self.0.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.0.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.check(path)?;
            self.0.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.check(path)?;
            self.0.exists(path)
        }
    }

    /// docs/coordination.md steps 8a, 8c: a member's pool side takes its lead from the lead's beat
    /// answer (as the lead's agent tells its coordinator it leads, lead::after), so its steps learn
    /// the lead with the declaration not answering, across the lead's restart; its lead's word
    /// stale (no answer within `PAGE_AWAY_S`), it reads the declaration again.
    #[test]
    fn a_member_learns_its_lead_by_the_beat_with_the_declaration_hung() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let hung = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut a = side(&store::nasfs::NasRoot::local_for_test(&r), &d.path().join("a/agent"));
        assert_eq!(until(&mut a, || Give::default(), |o| o.caught_up).leads, Some(1));
        let nas: SharedNas =
            Arc::new(DeclarationHung(Arc::new(store::nasfs::NasRoot::local_for_test(&r)), hung.clone()));
        let mut b = side_over(nas, &d.path().join("b/agent"));
        let (c, _port) = crate::coord::start_for_test(&d.path().join("a/coord"), "a", "");
        c.set_member(Some(&a.member().id));
        c.set_beats(Some(a.http_beats()));
        let tell = |a: &Side| {
            let e = a.driver().leads().unwrap();
            c.set_epoch(e);
            c.set_led(Some((e, a.member().clone())));
        };
        let beat = |b: &Side| {
            let w = crate::coord::held::Beat {
                member: "b-host".into(),
                speaks: vec![crate::coord::held::SPEAKS.into(), crate::coord::beats::SPEAKS.into()],
                heartbeat: b.http_beats().own(),
                ..Default::default()
            };
            b.http_beats().answered(&c.member_beat(&w), b.now());
        };
        let unlearnt = |o: &Out| {
            o.events.iter().any(
                |e| matches!(e, Event::Failed { what, .. } | Event::Waits { what, .. } if what.contains("declaration")),
            )
        };
        tell(&a);
        beat(&b);
        hung.store(true, std::sync::atomic::Ordering::SeqCst);
        let o = b.step(Give::default(), &any);
        assert_eq!(o.leads, None, "{:?}", o.events);
        assert!(!unlearnt(&o), "{:?}", o.events);
        assert_eq!(b.driver().declared(), Some(a.member().id.as_str()));
        assert_eq!(b.driver().lead().map(|l| l.app.clone()), Some(a.member().app.clone()), "its app, from its word");
        // A restarted (a new epoch): B goes on learning its lead from its next answer.
        a.restart().unwrap();
        until(&mut a, || Give::default(), |o| o.leads == Some(2));
        tell(&a);
        beat(&b);
        let o = b.step(Give::default(), &any);
        assert!(!unlearnt(&o) && b.driver().declared() == Some(a.member().id.as_str()), "{:?}", o.events);
        // No answer lately: the declaration read, which doesn't answer: said, its view kept.
        b.ahead += crate::coord::beats::PAGE_AWAY_S;
        let o = b.step(Give::default(), &any);
        assert!(unlearnt(&o), "{:?}", o.events);
        assert_eq!(b.driver().declared(), Some(a.member().id.as_str()), "its view kept");
    }

    #[test]
    fn a_members_mail_goes_by_http_alone_to_a_lead_that_serves_it() {
        let mut m = mail_pool();
        let ib = m.b.member().id.clone();
        m.b.step(Give::default(), &any);
        m.beat();
        let t = m.ask("6/1/5");
        assert_eq!(m.posts(), 1);
        assert!(!m.r.join("state/pool/mail").exists(), "posted, no mailbox written");
        assert_eq!(m.a.inbox().held()[&ib].len(), 1);
        Mail::acted(&mut m.a, &t);
        assert!(m.a.inbox().held().is_empty(), "read, dropped once its mark is on disk");
        // (Review) a post from an id with no heartbeat makes up no member: passed over, dropped.
        let stranger = "m-00000000000000dd";
        m.a.inbox().put(stranger, vec![(1, serde_json::json!({"Leads": 9}))]).unwrap();
        m.a.step(Give::default(), &any);
        assert!(!m.a.members().contains(stranger));
        assert!(m.a.inbox().held().is_empty());
        drop(m.c);
    }

    /// docs/coordination.md step 5: the lead changes while a member posts (the address now answers as
    /// another member: refused, 409), or its coordinator is down: the mail stays queued, no mailbox
    /// written, and goes once the lead takes it.
    #[test]
    fn a_post_refused_stays_queued_until_the_lead_takes_it() {
        let mut m = mail_pool();
        let ib = m.b.member().id.clone();
        m.b.step(Give::default(), &any);
        m.beat();
        m.c.set_member(Some("m-00000000000000cc"));
        let t = m.ask("6/1/5");
        assert_eq!(m.posts(), 1);
        assert!(m.a.inbox().held().is_empty(), "refused: kept");
        m.c.set_member(Some(&m.a.member().id.clone()));
        m.b.step(Give::default(), &any);
        assert_eq!(m.posts(), 2);
        assert_eq!(m.a.inbox().held()[&ib].len(), 1);
        Mail::acted(&mut m.a, &t);
        drop(m.c);
    }

    /// The lead crashes after its coordinator answered a post, before its agent read it: the post
    /// is in its store before the answer, so its next process has it. Nothing lost.
    #[test]
    fn a_post_answered_survives_the_leads_crash() {
        let mut m = mail_pool();
        let ib = m.b.member().id.clone();
        m.b.step(Give::default(), &any);
        m.beat();
        let t = m.ask("6/1/5");
        // (Crashed: neither stepped again nor its state written past this.)
        let home = m.d.path().join("a/agent");
        m.c.stop();
        drop(m.a);
        let mut a = side(&store::nasfs::NasRoot::local_for_test(&m.r), &home);
        let (c, _) = crate::coord::start_for_test(&m.d.path().join("a/coord"), "a", "");
        c.set_member(Some(&a.member().id));
        c.set_inbox(Some(a.inbox()));
        assert_eq!(a.inbox().held()[&ib].len(), 1, "from its store");
        a.know(&ib);
        Mail::acted(&mut a, &t);
        c.stop();
    }

    /// docs/coordination.md step 6: a hand-off posted twice (an answer lost), and once more after
    /// it's merged, is merged once.
    #[test]
    fn a_hand_off_posted_twice_merges_once() {
        let mut m = mail_pool();
        m.by_http();
        let e = m.hand_off(5);
        let key = e.key().unwrap();
        assert_eq!(m.beat().lacks, [key.clone()], "listed, lacked: posted");
        m.post(&e);
        assert_eq!(m.a.handoffs().taken().len(), 1);
        let applied = std::cell::Cell::new(0);
        until(
            &mut m.a,
            || Give::default(),
            |o| {
                let n =
                    o.events.iter().filter(|e| matches!(e, Event::Merged { applied, .. } if applied.contains(&key)));
                applied.set(applied.get() + n.count());
                !o.merged.is_empty()
            },
        );
        assert_eq!(applied.get(), 1);
        assert!(m.a.handoffs().taken().is_empty(), "out of the store");
        assert_eq!(m.beat().merged, [key.clone()]);
        m.b.step(Give::default(), &any);
        assert_eq!(m.b.driver().mine().unacked(), 0);
        m.beat();
        // Posted again late (a post held up in the network): taken, passed over, acknowledged.
        m.post(&e);
        let out = m.a.step(Give::default(), &any);
        assert_eq!(out.merged, [key]);
        assert!(!out.events.iter().any(|e| matches!(e, Event::Merged { .. })), "{:?}", out.events);
        assert!(m.a.handoffs().taken().is_empty());
        drop(m.c);
    }

    /// docs/coordination.md step 6: the lead is killed after its coordinator took a hand-off and
    /// before its agent merged it: its next process has it from its store. Its store lost too (a
    /// new disk), the member's beat to the new epoch finds the lead lacks what it keeps, and posts
    /// it again. Nothing lost.
    #[test]
    fn a_lead_killed_after_taking_before_saving_loses_nothing() {
        let mut m = mail_pool();
        m.by_http();
        let epoch = m.beat().epoch;
        let e = m.hand_off(5);
        let key = e.key().unwrap();
        m.beat();
        assert_eq!(m.a.handoffs().taken().len(), 1, "taken, on its disk before the answer");
        // (Killed: neither stepped again nor its state written past this.)
        let restart = |m: &mut Mail, lose_store: bool| {
            let home = m.d.path().join("a/agent");
            m.c.stop();
            let nas = m.a.shared_nas();
            // (Its process gone, its member's lock let go: a side elsewhere in its place first.)
            m.a = side_over(nas.clone(), &m.d.path().join("x/agent"));
            if lose_store {
                std::fs::remove_dir_all(m.d.path().join("a/coord")).unwrap();
            }
            m.a = side_over(nas, &home);
            let (c, port) = crate::coord::start_for_test(&m.d.path().join("a/coord"), "a", "");
            c.set_member(Some(&m.a.member().id));
            c.set_beats(Some(m.a.http_beats()));
            c.set_handoffs(Some(m.a.handoffs()));
            (m.c, m.port) = (c, port);
        };
        restart(&mut m, false);
        assert_eq!(m.a.handoffs().taken().len(), 1, "from its store");
        let a = m.beat();
        assert!(a.epoch > epoch && a.lacks.is_empty(), "{a:?}");
        m.merged(&key);
        // Its store lost: posted again on the new epoch.
        let e = m.hand_off(6);
        let key = e.key().unwrap();
        m.beat();
        restart(&mut m, true);
        assert!(m.a.handoffs().taken().is_empty());
        assert_eq!(m.beat().lacks, [key.clone()], "lacked: posted again");
        assert_eq!(m.a.handoffs().taken().len(), 1);
        m.merged(&key);
        m.c.stop();
    }

    /// A NAS whose writes of the records fail while `slow` (under nasread's deadline a hung share
    /// fails rather than waits).
    struct SlowRecords {
        nas: SharedNas,
        slow: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Nas for SlowRecords {
        fn create_new(&self, p: &str, b: &[u8]) -> Result<Created> {
            self.nas.create_new(p, b)
        }
        fn write_whole(&self, p: &str, b: &[u8]) -> Result<()> {
            anyhow::ensure!(
                !(p.starts_with("state/build/") && self.slow.load(std::sync::atomic::Ordering::SeqCst)),
                "timed out: the NAS is slow"
            );
            self.nas.write_whole(p, b)
        }
        fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
            self.nas.read(p)
        }
        fn exists(&self, p: &str) -> Result<bool> {
            self.nas.exists(p)
        }
    }

    /// docs/coordination.md step 6: with the NAS slow the lead takes hand-offs as fast as ever (its
    /// handler never touches the NAS); its records' save waits, and so do the acknowledgements;
    /// the NAS back, they're saved and acknowledged. Nothing lost.
    #[test]
    fn hand_offs_merge_with_the_nas_slow() {
        let slow = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = slow.clone();
        let mut m = mail_pool_over(move |nas| Arc::new(SlowRecords { nas, slow: s2 }));
        m.by_http();
        let e = m.hand_off(5);
        let key = e.key().unwrap();
        slow.store(true, std::sync::atomic::Ordering::SeqCst);
        // (Taken while every write of the records fails: the handler never touches the NAS,
        // coord_handlers_never_touch_the_nas.)
        m.beat();
        assert_eq!(m.a.handoffs().taken().len(), 1);
        for _ in 0..3 {
            let o = m.a.step(Give::default(), &any);
            assert!(o.merged.is_empty() && !o.caught_up, "{:?}", o.events);
            assert!(o.events.iter().any(|e| matches!(e, Event::Failed { what: "save the records", .. })));
            assert!(m.beat().merged.is_empty(), "no ack before the save");
        }
        m.b.step(Give::default(), &any);
        assert_eq!(m.b.driver().mine().unacked(), 1, "kept");
        assert_eq!(m.a.handoffs().taken().len(), 1, "kept");
        slow.store(false, std::sync::atomic::Ordering::SeqCst);
        m.merged(&key);
        drop(m.c);
    }

    /// Mixed versions (task #1125): a member on an app before #1125 writes its hand-offs to the
    /// NAS's journal, which no lead reads now, for a lead whose answer doesn't say `handoff`. This
    /// lead says it in every answer, its pool side's registries attached or not (its agent's first
    /// step after its coordinator starts): such a member keeps them, and posts them once the lead
    /// lacks them.
    #[test]
    fn mixed_versions_an_older_member_never_falls_back_to_the_journal() {
        use crate::coord::{beats, held};
        let mut m = mail_pool();
        let d = tempfile::tempdir().unwrap();
        let (bare, _) = crate::coord::start_for_test(&d.path().join("coord"), "a", "");
        let ib = m.b.member().id.clone();
        m.b.step(Give::default(), &any);
        m.beat();
        let e = m.hand_off(5);
        let key = e.key().unwrap();
        let old = held::Beat {
            member: "b-host".into(),
            speaks: [held::SPEAKS, beats::SPEAKS, beats::ASKS, beats::HANDOFF].map(str::to_string).to_vec(),
            heartbeat: m.b.http_beats().own(),
            handoffs: vec![key.clone()],
            ..Default::default()
        };
        // (A coordinator its agent hasn't stepped yet: no registry of any kind.)
        let a = bare.member_beat(&old);
        assert!(a.speaks.iter().any(|w| w == beats::HANDOFF) && a.lacks.is_empty() && a.merged.is_empty(), "{a:?}");
        // (Its beats' registry attached, its hand-offs' not yet.)
        let a = m.c.member_beat(&old);
        assert!(a.speaks.iter().any(|w| w == beats::HANDOFF) && a.lacks.is_empty(), "{a:?}");
        // Attached: lacked, posted, merged.
        m.c.set_handoffs(Some(m.a.handoffs()));
        assert_eq!(m.c.member_beat(&old).lacks, [key.clone()]);
        m.post(&e);
        m.merged(&key);
        assert!(!m.r.join("state/journal").exists(), "{ib}: nothing by the journal");
        bare.stop();
        drop(m.c);
    }

    #[test]
    fn a_jobs_entry_is_its_saves_and_the_targets_it_finished() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("1-3");
        let save = |l: &str, c: &str| Handoff {
            changes: [(l.to_string(), Some(c.to_string()))].into(),
            pending: [(c.to_string(), "aa".to_string())].into(),
            ..Default::default()
        };
        buildstore::handoff::write(&dir, &save("base/6-1-1", "base/6-1-1.1111111111111111.base")).unwrap();
        buildstore::handoff::write(&dir, &save("base/6-1-2", "base/6-1-2.2222222222222222.base")).unwrap();
        let lease = LeaseId { epoch: 1, n: 3 };
        // A shared step's: only the files of the targets it finished.
        let e = entry_of(
            crate::agent::seam::for_tests(),
            &dir,
            "m-000000000000000a",
            lease,
            "unit",
            &[("6/1/1".into(), "k")].map(|(a, b): (String, &str)| (a, b.to_string())),
            100_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(e.handoff.changes.keys().collect::<Vec<_>>(), ["base/6-1-1"]);
        assert_eq!(e.handoff.pending.len(), 1);
        assert_eq!(e.handoff.done, Some(("unit".into(), vec![("6/1/1".into(), "k".into())])));
        // Another step's (the OSM pass's stages): all of them, done or not.
        let e = entry_of(crate::agent::seam::for_tests(), &dir, "m-000000000000000a", lease, "osm-pass", &[], 100_000)
            .unwrap()
            .unwrap();
        assert_eq!((e.handoff.changes.len(), e.handoff.done.clone()), (2, None));
        // Nothing done of a shared step: nothing to hand off.
        assert!(entry_of(crate::agent::seam::for_tests(), &dir, "m-000000000000000a", lease, "unit", &[], 100_000)
            .unwrap()
            .is_none());
        // A damaged save: an error (its work is done again).
        std::fs::write(dir.join("00000000000000000009-1.json"), b"{").unwrap();
        assert!(entry_of(crate::agent::seam::for_tests(), &dir, "m-000000000000000a", lease, "osm-pass", &[], 100_000)
            .is_err());
    }

    #[test]
    fn the_hand_offs_journaled_and_left_out_of_the_pool_are_drained_into_the_journal() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut run = Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &home),
            Role::Lead,
            Gates::default(),
        );
        let done = |u: &str, k: &str| Handoff {
            done: Some(("unit".into(), vec![(u.into(), k.into())])),
            ..Default::default()
        };
        // The coordinator's journal here (a member out of the pool's hand-offs), and this member's
        // outbox (a job it ran out of the pool, never told).
        let journal = home.join("coord/journal");
        buildstore::handoff::write(&journal.join("m1"), &done("6/2/1", "k21")).unwrap();
        buildstore::handoff::write(&journal.join("m1"), &done("6/2/2", "k22")).unwrap();
        let ob = home.join("outbox/1791328399374");
        std::fs::create_dir_all(&ob).unwrap();
        std::fs::write(
            ob.join("work.json"),
            serde_json::to_vec(&crate::agent::jobs::Work {
                step: "unit".into(),
                targets: vec![("6/2/3".into(), "k23".into())],
            })
            .unwrap(),
        )
        .unwrap();
        buildstore::handoff::write(
            &ob,
            &Handoff {
                changes: [("base/6-2-3".to_string(), Some("base/6-2-3.3333333333333333.base".to_string()))].into(),
                ..Default::default()
            },
        )
        .unwrap();
        std::fs::write(
            ob.join("result.json"),
            serde_json::json!({ "ok": true, "done": ["unit", [["6/2/3", "k23"]]] }).to_string(),
        )
        .unwrap();
        // (A task's folder: left to its broker. A failed job's with nothing to hand off: gone, or
        // the member would never ask for work again.)
        std::fs::create_dir_all(home.join("outbox/1791328399375")).unwrap();
        let failed = home.join("outbox/1791328399376");
        std::fs::create_dir_all(&failed).unwrap();
        std::fs::write(
            failed.join("work.json"),
            serde_json::to_vec(&crate::agent::jobs::Work {
                step: "unit".into(),
                targets: vec![("6/2/4".into(), "k24".into())],
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(failed.join("result.json"), r#"{"ok": false, "done": null, "error": "it failed"}"#).unwrap();
        run.drain(&journal);
        run.drain_outbox(&home.join("outbox"));
        let leases: Vec<LeaseId> = run.entries.iter().map(|(e, _)| e.lease).collect();
        assert_eq!(leases.len(), 3, "{leases:?}");
        assert!(leases.iter().all(|l| l.epoch == 0));
        assert!(leases.contains(&LeaseId { epoch: 0, n: 1791328399374 }), "an outbox job keeps its lease");
        assert_eq!(leases.iter().filter(|l| l.n >= DRAINED).count(), 2);
        // Drained once in this process, whatever the next look finds.
        run.drain(&journal);
        run.drain_outbox(&home.join("outbox"));
        assert_eq!(run.entries.len(), 3);
        // Merged by the lead's step; their sources removed once its saved state holds them.
        let out = run.step();
        assert_eq!(out.leads, Some(1));
        let rec = run.side.driver().records().unwrap();
        assert_eq!(
            (
                rec.keys.recorded("unit", "6/2/1"),
                rec.keys.recorded("unit", "6/2/2"),
                rec.keys.recorded("unit", "6/2/3")
            ),
            (Some("k21"), Some("k22"), Some("k23"))
        );
        assert_eq!(rec.manifest.get("base/6-2-3").map(String::as_str), Some("base/6-2-3.3333333333333333.base"));
        assert!(buildstore::handoff::waiting_in(&journal).unwrap().is_empty() && !ob.exists());
        // (Marked merged: a file whose removal had failed isn't drained again.)
        assert!(journal.join("m1").with_extension("merged").exists());
        assert!(home.join("outbox/1791328399375").exists() && !failed.exists());
        // Term 1's records written to today's files too, for their readers.
        assert_eq!(
            crate::pool::records::Keys::load(&store::nasfs::NasRoot::local_for_test(&r)).recorded("unit", "6/2/2"),
            Some("k22")
        );
    }

    #[test]
    fn the_merge_refuses_what_out_would() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut run = Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &home),
            Role::Lead,
            Gates::default(),
        );
        let me = run.side.member().id.clone();
        let pack = |n: u64, t: &str, names: &[&str]| {
            let dash = t.replace('/', "-");
            let changes =
                names.iter().map(|l| (format!("{l}/{dash}"), Some(format!("{l}/{dash}.{n:016x}.pack")))).collect();
            Entry {
                member: me.clone(),
                lease: LeaseId { epoch: 1, n },
                step: "pack".into(),
                handoff: Handoff {
                    changes,
                    done: Some(("pack".into(), vec![(t.into(), format!("k{n}"))])),
                    ..Default::default()
                },
                at: crate::agent::jobs::now_s(),
            }
        };
        // A map tile's own files, and one that also writes a terrain pack (another step's).
        let within = pack(1, "6/1/2", &["hidata", "layers/roads/hi"]);
        let outside = pack(2, "6/1/3", &["hidata", "layers/terrain/hi"]);
        let key = outside.key().unwrap();
        // (The same names a pack job's `Out` takes and refuses.)
        let mut o = buildstore::out::Out::open(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("o")),
            &d.path().join("o-scratch"),
            "pack",
            crate::agent::seam::for_tests().writes(),
        )
        .unwrap();
        for (l, c) in &within.handoff.changes {
            o.set(l, c.clone());
        }
        o.save().unwrap();
        o.set("layers/terrain/hi/6-1-3", Some("layers/terrain/hi/6-1-3.0000000000000002.pack".into()));
        assert!(o.save().is_err());
        run.entries.extend([(within, None), (outside, None)]);
        let out = run.step();
        assert_eq!(out.leads, Some(1));
        // The one within merged, the one outside refused, with why, and nothing of it taken.
        let rec = run.side.driver().records().unwrap();
        assert_eq!((rec.keys.recorded("pack", "6/1/2"), rec.keys.recorded("pack", "6/1/3")), (Some("k1"), None));
        assert!(!rec.manifest.contains_key("layers/terrain/hi/6-1-3") && !rec.manifest.contains_key("hidata/6-1-3"));
        let why = rec.rejected.get(&key).expect("refused");
        assert!(why.contains("layers/terrain/hi/6-1-3"), "{why}");
    }

    /// Task #701: the clean-up's entry dropping a removed step's names (crate::agent::gc::RETIRED)
    /// is merged into the records, in one save; one also removing another name is refused whole;
    /// told again, it changes nothing.
    #[test]
    fn the_merge_takes_the_clean_ups_drop_of_removed_steps_names() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        std::fs::write(
            r.join("state/build/manifest.json"),
            r#"{"base/6-1-1": "base/6-1-1.1111111111111111.base", "sources/terrain-z8-v1": "sources/terrain-z8-v1.2222222222222222.pack", "sources/terrain-z8-v1-max": "sources/terrain-z8-v1-max.3333333333333333.f32"}"#,
        )
        .unwrap();
        let home = d.path().join("a/agent");
        let mut run = Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &home),
            Role::Lead,
            Gates::default(),
        );
        let me = run.side.member().id.clone();
        let gc = |n: u64, names: &[&str]| Entry {
            member: me.clone(),
            lease: LeaseId { epoch: 1, n },
            step: "gc".into(),
            handoff: Handoff { changes: names.iter().map(|l| (l.to_string(), None)).collect(), ..Default::default() },
            at: crate::agent::jobs::now_s(),
        };
        let too_far = gc(1, &["sources/terrain-z8-v1", "base/6-1-1"]);
        let bad = too_far.key().unwrap();
        let drop = gc(2, &["sources/terrain-z8-v1", "sources/terrain-z8-v1-max"]);
        run.entries.extend([(too_far, None), (drop.clone(), None)]);
        assert_eq!(run.step().leads, Some(1));
        let rec = run.side.driver().records().unwrap();
        assert_eq!(rec.manifest.keys().collect::<Vec<_>>(), ["base/6-1-1"]);
        assert!(rec.rejected.get(&bad).expect("refused").contains("base/6-1-1"));
        let seq = rec.seq;
        run.entries.push((drop, None));
        run.step();
        let rec = run.side.driver().records().unwrap();
        assert_eq!((rec.manifest.len(), rec.seq), (1, seq));
    }

    /// Task #679: the clean-up's entry dropping depth packs no target names is merged; a depth
    /// job running for one of those tiles (an older lease, its entry merged after) puts its pack
    /// back, its work not lost (the next clean-up judges it again); an entry from a member on a
    /// newer app dropping a name this lead's write-sets don't let gc remove is held, not refused,
    /// so a mixed-version pool loses neither (check_from).
    #[test]
    fn the_merge_takes_the_clean_ups_drop_of_depth_packs() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        std::fs::write(
            r.join("state/build/manifest.json"),
            r#"{"layers/depth/hi/6-2-33": "layers/depth/hi/6-2-33.1111111111111111.pack", "layers/depth/lo/3-5-5": "layers/depth/lo/3-5-5.2222222222222222.pack", "layers/terrain/hi/6-2-33": "layers/terrain/hi/6-2-33.3333333333333333.pack"}"#,
        )
        .unwrap();
        let home = d.path().join("a/agent");
        let mut run = Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &home),
            Role::Lead,
            Gates::default(),
        );
        let me = run.side.member().id.clone();
        let entry = |n: u64, step: &str, changes: &[(&str, Option<&str>)]| Entry {
            member: me.clone(),
            lease: LeaseId { epoch: 1, n },
            step: step.into(),
            handoff: Handoff {
                changes: changes.iter().map(|(l, c)| (l.to_string(), c.map(str::to_string))).collect(),
                ..Default::default()
            },
            at: crate::agent::jobs::now_s(),
        };
        let drop = entry(3, "gc", &[("layers/depth/hi/6-2-33", None), ("layers/depth/lo/3-5-5", None)]);
        let terrain = entry(4, "gc", &[("layers/terrain/hi/6-2-33", None)]);
        let bad = terrain.key().unwrap();
        run.entries.extend([(drop, None), (terrain.clone(), None)]);
        run.step();
        let rec = run.side.driver().records().unwrap();
        assert_eq!(rec.manifest.keys().collect::<Vec<_>>(), ["layers/terrain/hi/6-2-33"]);
        assert!(rec.rejected.get(&bad).expect("refused").contains("layers/terrain/hi/6-2-33"));
        // The running depth job's entry, of an older lease, merged after: its pack is back.
        let mut job =
            entry(2, "depth", &[("layers/depth/hi/6-2-33", Some("layers/depth/hi/6-2-33.4444444444444444.pack"))]);
        job.handoff.pending.insert("layers/depth/hi/6-2-33.4444444444444444.pack".into(), "0".repeat(64));
        job.handoff.done = Some(("depth".into(), vec![("6/2/33".into(), "k".into())]));
        run.entries.push((job, None));
        run.step();
        let rec = run.side.driver().records().unwrap();
        assert_eq!(
            rec.manifest.get("layers/depth/hi/6-2-33").map(String::as_str),
            Some("layers/depth/hi/6-2-33.4444444444444444.pack"),
            "{:?}",
            rec.rejected
        );
        // Mixed versions: what this lead's gc may not remove, from a newer app, held.
        let on = |app: &'static str| move |_: &str| Some(app.to_string());
        let held = check_from(
            crate::agent::seam::for_tests(),
            &terrain,
            &Records::default(),
            "20261009-0000-bbbbbbb",
            &on("20261010-0000-ccccccc"),
        )
        .unwrap_err();
        assert!(held.starts_with(crate::pool::records::HOLD), "{held}");
    }

    #[test]
    fn jobs_an_earlier_process_left_are_handed_off() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let lease = LeaseId { epoch: 3, n: 11 };
        let dir = job_dir(&home, lease);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("work.json"),
            serde_json::to_vec(&JobKept {
                step: "unit".into(),
                targets: vec![("6/1/1".into(), "k1".into()), ("6/1/2".into(), "k2".into())],
                lease,
            })
            .unwrap(),
        )
        .unwrap();
        buildstore::handoff::write(
            &dir,
            &Handoff {
                changes: [
                    ("base/6-1-1".to_string(), Some("base/6-1-1.1111111111111111.base".to_string())),
                    ("base/6-1-2".to_string(), Some("base/6-1-2.2222222222222222.base".to_string())),
                ]
                .into(),
                ..Default::default()
            },
        )
        .unwrap();
        std::fs::write(dir.join("done.txt"), "unit 6/1/1\n").unwrap();
        // (A folder of no job: gone.)
        std::fs::create_dir_all(home.join("pool/jobs/junk")).unwrap();
        let left = left_jobs(crate::agent::seam::for_tests(), &home, "m-000000000000000a", &[]);
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
        let nas = store::nasfs::NasRoot::local_for_test(&r);
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
        let e = unit_entry("m-000000000000000a", LeaseId { epoch: 1, n: 1 }, "6/1/2", "k2");
        let r = Records::default();
        assert!(check(crate::agent::seam::for_tests(), &e, &r).is_ok());
        let mut other = e.clone();
        other.step = "pois".into();
        assert!(check(crate::agent::seam::for_tests(), &other, &r).unwrap_err().contains("done record is of unit"));
        let mut bad = e.clone();
        bad.handoff.changes.insert("base/6-1-3".into(), Some("base/6-1-4.3333333333333333.base".into()));
        assert!(check(crate::agent::seam::for_tests(), &bad, &r)
            .unwrap_err()
            .contains("isn't a content name of base/6-1-3"));
        let mut raw = e.clone();
        raw.handoff.raw.push((
            "3-1-2".into(),
            buildstore::rawpack::Pack { name: "3-1-3.0123456789abcdef.tiles".into(), bytes: 1 },
        ));
        assert!(check(crate::agent::seam::for_tests(), &raw, &r).is_err());
        let mut none = e.clone();
        none.handoff.done = Some(("unit".into(), Vec::new()));
        assert!(check(crate::agent::seam::for_tests(), &none, &r).is_err());
        // A shared step's: only its targets' files, its own uploads.
        let mut other = e.clone();
        other.handoff.changes.insert("base/6-1-9".into(), Some("base/6-1-9.3333333333333333.base".into()));
        assert!(check(crate::agent::seam::for_tests(), &other, &r)
            .unwrap_err()
            .contains("isn't one of the files of the targets it did"));
        let mut up = e.clone();
        up.handoff.pending.insert("base/6-1-9.3333333333333333.base".into(), "aa".into());
        assert!(check(crate::agent::seam::for_tests(), &up, &r).unwrap_err().contains("isn't one of its saves"));
        // Another step's (the OSM pass's stages): any name of its outputs, and none outside them
        // (the z6 tile's base pack, another step's).
        let mut pass = e;
        pass.step = "osm-pass".into();
        pass.handoff.done = None;
        pass.handoff.changes.insert(
            "sources/osm/2026-10-01/pass".into(),
            Some("sources/osm/2026-10-01/pass.4444444444444444.json".into()),
        );
        assert!(check(crate::agent::seam::for_tests(), &pass, &r)
            .unwrap_err()
            .contains("writes base/6-1-2, outside osm-pass's write-set"));
        pass.handoff.changes.remove("base/6-1-2");
        pass.handoff.pending.clear();
        assert!(check(crate::agent::seam::for_tests(), &pass, &r).is_ok());
    }

    /// An entry outside its step's write-set from a member on a newer app than the lead's is held,
    /// not refused (its app's write-sets may know the name); from the lead's app or an older one,
    /// refused; its member's app not read now, held; any other refusal stands whatever the app.
    /// Task #780 review: a redo's entry is merged from the lead merging it alone (its own job, of
    /// any epoch of its); another member's, a lead's before the owner moved the lead among them, is
    /// refused (docs/coordination.md step 8c: no term says who led an epoch). Other steps' pass.
    #[test]
    fn a_redo_entry_from_anyone_but_the_lead_is_refused() {
        let mut e = unit_entry("m-lead", LeaseId { epoch: 3, n: 1 }, "6/1/2", "");
        e.step = crate::agent::redo::STEP.into();
        assert_eq!(by_its_lead(&e, "m-lead"), Ok(()));
        e.lease.epoch = 840;
        assert_eq!(by_its_lead(&e, "m-lead"), Ok(()), "an earlier epoch's, its own");
        e.member = "m-other".into();
        assert!(by_its_lead(&e, "m-lead").unwrap_err().contains("isn't the lead"));
        let u = unit_entry("m-other", LeaseId { epoch: 3, n: 2 }, "6/1/2", "k");
        assert_eq!(by_its_lead(&u, "m-lead"), Ok(()));
    }

    #[test]
    fn an_entry_outside_from_a_newer_app_is_held() {
        let r = Records::default();
        let lead = "20261009-0000-bbbbbbb";
        let mut e = unit_entry("m-000000000000000b", LeaseId { epoch: 1, n: 1 }, "6/1/2", "k2");
        e.handoff.changes.insert("work/new-output".into(), Some("work/new-output.0123456789abcdef.json".into()));
        let on = |app: Option<&'static str>| move |_: &str| app.map(str::to_string);
        let held =
            check_from(crate::agent::seam::for_tests(), &e, &r, lead, &on(Some("20261010-0000-ccccccc"))).unwrap_err();
        assert!(
            held.starts_with(crate::pool::records::HOLD)
                && held.contains("work/new-output")
                && held.contains("20261010-0000-ccccccc"),
            "{held}"
        );
        assert!(check_from(crate::agent::seam::for_tests(), &e, &r, lead, &on(None))
            .unwrap_err()
            .starts_with(crate::pool::records::HOLD));
        for app in [lead, "20261008-0000-aaaaaaa"] {
            let why = check_from(crate::agent::seam::for_tests(), &e, &r, lead, &on(Some(app))).unwrap_err();
            assert!(
                !why.starts_with(crate::pool::records::HOLD) && why.contains("outside unit's write-set"),
                "{app}: {why}"
            );
        }
        // Another check's refusal (a content name not of its logical name): refused, newer or not.
        let mut bad = unit_entry("m-000000000000000b", LeaseId { epoch: 1, n: 2 }, "6/1/2", "k2");
        bad.handoff.changes.insert("base/6-1-2".into(), Some("base/6-1-3.3333333333333333.base".into()));
        assert!(!check_from(crate::agent::seam::for_tests(), &bad, &r, lead, &on(Some("20261010-0000-ccccccc")))
            .unwrap_err()
            .starts_with(crate::pool::records::HOLD));
        assert_eq!(
            check_from(
                crate::agent::seam::for_tests(),
                &unit_entry("m-000000000000000b", LeaseId { epoch: 1, n: 3 }, "6/1/2", "k2"),
                &r,
                lead,
                &on(None)
            ),
            Ok(())
        );
    }

    #[test]
    fn the_history_per_writer() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let ev =
            |seq: u64, t: u64| crate::coord::history::Event { seq, t, ..crate::coord::history::Event::new("start") };
        append_history(
            &store::nasfs::NasRoot::local_for_test(r),
            "m-000000000000000a",
            &[ev(1, 1_791_400_000), ev(2, 1_791_400_001)],
        )
        .unwrap();
        append_history(&store::nasfs::NasRoot::local_for_test(r), "m-000000000000000a", &[ev(3, 1_791_500_000)])
            .unwrap();
        let day = |t| journal::day(t).unwrap();
        let f = |t| {
            std::fs::read_to_string(r.join("state/coord/history").join(day(t)).join("m-000000000000000a.jsonl"))
                .unwrap()
        };
        assert_eq!(f(1_791_400_000).lines().count(), 2);
        assert_eq!(f(1_791_500_000).lines().count(), 1);
    }

    /// Moves a run's clocks on `s` seconds (its driver's wall and awake clocks: `Side::ahead`).
    fn pass(r: &mut Run, s: u64) {
        r.side.ahead += s;
    }

    /// Ticks `r` as its pool's own thread would (`TICK_S` apart: its agent's loop busy) until
    /// `done`; the other runs ticked in turn, their clocks moved alike (five minutes is a watchdog,
    /// not a measure).
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
    fn only_the_declared_leads_coordinator_answering_counts() {
        // Its answers say its epoch and member, while it has them; a member counts an answer only
        // from the declared lead's, leading (a member's broker says epoch 0).
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "a", "development");
        let client =
            crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "b");
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec!["x".into()], mem_mb: 1024, ..Default::default() };
        client.ask(&ask).ok();
        let first = client.answered().expect("answered");
        assert_eq!((first.epoch, first.member.as_deref()), (None, None), "not a pool's lead yet: it says nothing");
        c.set_member(Some("m-000000000000000a"));
        c.set_epoch(841);
        client.ask(&ask).ok();
        let a = client.answered().unwrap();
        assert_eq!((a.epoch, a.member.as_deref()), (Some(841), Some("m-000000000000000a")));
        assert_eq!(lead_answered(Some(&a), Some("m-000000000000000a")), Some(a.at));
        assert_eq!(lead_answered(Some(&a), Some("m-000000000000000c")), None, "another member's");
        assert_eq!(lead_answered(Some(&first), Some("m-000000000000000a")), None, "one that says nothing");
        assert_eq!(lead_answered(None, Some("m-000000000000000a")), None);
        assert_eq!(lead_answered(Some(&a), None), None, "no lead declared");
        let broker = crate::coord::client::Answered { epoch: Some(0), ..a.clone() };
        assert_eq!(lead_answered(Some(&broker), Some("m-000000000000000a")), None, "a broker's");
        // (Task #497) Its Mac lost the NAS: its answers say nothing of its epoch.
        c.set_member(None);
        client.ask(&ask).ok();
        let lost = client.answered().unwrap();
        assert_eq!((lost.epoch, lost.member.as_deref()), (None, None));
        assert_eq!(lead_answered(Some(&lost), Some("m-000000000000000a")), None);
    }

    #[test]
    fn a_pool_step_held_by_a_tick_is_passed_over_and_a_panicking_one_clears_in_step() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let shared: Shared = Arc::new(std::sync::Mutex::new(Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &home),
            Role::Member,
            Gates::default(),
        )));
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
    fn a_tick_is_due_while_the_agents_loop_is_quiet_and_never_during_its_step() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let home = d.path().join("a/agent");
        let mut run = Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &home),
            Role::Member,
            Gates::default(),
        );
        assert!(!run.tick_due(), "no NAS known yet: nothing to tick");
        run.cx = Cx {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: home.clone(),
            ..Default::default()
        };
        assert!(run.tick_due(), "never stepped");
        // The loop's step: none while it runs, nor for `QUIET_S` after it.
        run.in_step = true;
        assert!(!run.tick_due());
        run.step();
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

    /// Task #881: the lead's restart (an update's, at a safe point) keeps its leases. A member asks
    /// for work all through epoch 1; the lead's process ends, and the next takes the records up in
    /// epoch 2: its coordinator opens the store (task #878), every lease granted and answered in
    /// epoch 1 there, still epoch 1's, so every job out goes on renewing with it.
    #[test]
    fn the_leads_restart_keeps_its_leases() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let ha = d.path().join("a/agent");
        let mut a = Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &ha),
            Role::Member,
            Gates::default(),
        );
        let (coord, port) = crate::coord::start_for_test(&d.path().join("a/coord"), "a", "development");
        let coord = Arc::new(coord);
        let end = Instant::now() + WATCHDOG;
        while !a.step().caught_up {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(30));
        }
        a.cx = Cx {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: ha.clone(),
            coord: Some(coord.clone()),
            ..Default::default()
        };
        tick_until(&mut [&mut a], |r| r[0].gates.caught_up);
        coord.set_epoch(1);
        // The member asks without a pause, each grant a target of its own.
        let stop = Arc::new(AtomicBool::new(false));
        let (c2, s2) = (coord.clone(), stop.clone());
        let burst = std::thread::spawn(move || {
            let mut granted = Vec::new();
            let mut i = 0;
            while !s2.load(Ordering::Relaxed) {
                i += 1;
                // (Measured, so not the step's one cold run; a worker an ask.)
                let cost = crate::coord::Cost {
                    peak_mb: 100,
                    secs: 60,
                    v: crate::coord::cost_version("unit"),
                    sizes: [("threads".to_string(), 1)].into(),
                    ..Default::default()
                };
                c2.add_costs(&[(format!("6/7/{i}"), cost)]);
                let w = crate::coord::client::Client::at(
                    vec![format!("http://127.0.0.1:{port}")],
                    c2.contact.token.clone(),
                    &format!("m{i}"),
                );
                let o = crate::coord::Offer {
                    step: "unit".into(),
                    targets: vec![(format!("6/7/{i}"), "k".into(), 100)],
                    job: None,
                };
                c2.offer("p", vec![o]);
                let ask = crate::coord::Ask {
                    kind: "native".into(),
                    can: vec!["unit".into()],
                    mem_mb: 8192,
                    cores: 8,
                    app: Some("development".into()),
                    want: 1,
                    ..Default::default()
                };
                if let Ok(Some(g)) = w.ask(&ask) {
                    granted.push(g.lease);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            granted
        });
        store::testkit::eventually("the burst's first grants", || coord.leases_out() >= 3);
        // The update's safe point: the process ends, every grant answered in its store.
        stop.store(true, Ordering::Relaxed);
        let granted = burst.join().unwrap();
        coord.stop();
        drop(a);
        // The next process: its driver from the saved state takes the records up in epoch 2, and
        // its new coordinator has epoch 1's leases.
        let mut a = Run::new(
            crate::agent::seam::for_tests(),
            side(&store::nasfs::NasRoot::local_for_test(&r), &ha),
            Role::Member,
            Gates::default(),
        );
        let mut took = Vec::new();
        let end = Instant::now() + WATCHDOG;
        let out = loop {
            let out = a.step();
            took.extend(out.events.iter().filter(|e| matches!(e, Event::TookUp { .. })).cloned());
            if out.leads.is_some() {
                break out;
            }
            assert!(Instant::now() < end, "not leading again: {:?}", out.events);
            std::thread::sleep(Duration::from_millis(30));
        };
        assert_eq!(out.leads, Some(2));
        assert!(
            took.iter().any(|e| matches!(e, Event::TookUp { epoch: 2, how } if how == "its own records")),
            "{took:?}"
        );
        let (cb, _) = crate::coord::start_for_test(&d.path().join("a/coord"), "a", "development");
        took_up(&mut a, Some(&cb), &took, out.leads, "a", &[]);
        let lost: Vec<u64> = granted.iter().copied().filter(|id| cb.lease_epoch(*id) != 1).collect();
        assert!(granted.len() >= 3 && lost.is_empty(), "{} granted, not taken up: {lost:?}", granted.len());
    }
}
