//! The agent's part in the pool (docs/pool.md §12, phase 1's integration): what runs the pool's
//! driver (crate::pool::driver) once a loop and does what it can't do itself. The driver's contract
//! (its module's doc) says what that is; this module keeps it:
//!
//! - **One process per member** (`Side::open`): the member's id (crate::pool::member_id, in the
//!   agent's folder), its lock in the app's folder (crate::pool::MemberLock), and the driver made
//!   from the state saved last. A process the lock is held from runs no driver.
//! - **The saved state** (`Saved`) written whole to the agent's folder after every step that changed
//!   it, before anything that step said is acted on; a job's hand-off handed to a step kept (its
//!   folder) until a saved state holding it is on disk.
//! - **The listings** the driver asks for, made on a thread of their own one at a time, every one
//!   handed back once, a failed one made again a minute later; and a listing of the members'
//!   heartbeats every ten minutes the same way (the members a lead hears from: nothing lists a
//!   folder in the loop).
//! - **The messages** between members, by mailbox on the NAS (`MAIL`: `<to>/<from>.json`, written
//!   whole by its sender alone, the last `KEPT` it sent there, each numbered; read by member id,
//!   never by listing): best effort, as the driver's are (a message lost is told again or made up
//!   for). The pool's API (§9) is planned.
//! - **The heartbeat**: the driver's fields, the members it knows, written each loop they change
//!   and at least every two minutes, stamped as it's written, to `state/pool/members/<id>.json`.
//! - **The merge's checks** (`check`): what phase 1 checks of an entry.
//!
//! The switch (`mode`): `state/pool/enabled` on the NAS turns the pool on (phase 1's integration in
//! crate::agent: the agent leads or works as a member as the terms say), `state/pool/shadow` runs
//! it beside today's coordination (`Overlay`: reading the build's records, writing only under
//! `state/pool-shadow/`, acting on nothing: crate::agent::shadow), and neither leaves the agent as
//! it was.

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

/// The switch that turns the pool on, on the NAS (§12): while it's missing, the agent is as it was.
pub const ENABLED: &str = "state/pool/enabled";
/// The switch that runs the pool beside today's coordination, acting on nothing (`Overlay`).
pub const SHADOW: &str = "state/pool/shadow";
/// Where a shadow run writes, under the NAS's project folder: the pool's files as they'd be.
pub const SHADOW_ROOT: &str = "state/pool-shadow";
/// The members' mailboxes: `<to>/<from>.json`.
pub const MAIL: &str = "state/pool/mail";
/// The members' heartbeats (crate::pool::beat::path).
pub const MEMBERS: &str = "state/pool/members";
/// The messages a sender keeps in a mailbox: its last.
const KEPT: usize = 64;
/// How often the members' heartbeats are listed (off the loop), for the members it knows.
const MEMBERS_EVERY: Duration = Duration::from_secs(600);
/// A failed listing is made again after this.
const AGAIN: Duration = Duration::from_secs(60);
/// The heartbeat is written at least this often.
const BEAT_EVERY: Duration = Duration::from_secs(120);
/// A Tell the same as the last to that member is sent again only after this (the lead hears it
/// each loop otherwise; it keeps what it was told).
const TELL_AGAIN: Duration = Duration::from_secs(600);

/// How the pool runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Off: the agent as it was.
    Off,
    /// Beside today's coordination, acting on nothing.
    Shadow,
    /// On: the terms say who leads.
    On,
}

/// The pool's switch at `root` (the NAS's project folder): on when `ENABLED` is there, else shadow
/// when `SHADOW` is; None when it can't be told now (the share not answering: the caller keeps
/// what it had).
pub fn mode(root: &Path) -> Option<Mode> {
    let on = root.join(ENABLED).try_exists().ok()?;
    if on {
        return Some(Mode::On);
    }
    Some(if root.join(SHADOW).try_exists().ok()? { Mode::Shadow } else { Mode::Off })
}

/// A NAS the driver's steps and the listings' thread share.
pub type SharedNas = Arc<dyn Nas + Send + Sync>;

/// The NAS as a shadow run sees it: everything it writes, lists and reads is under `SHADOW_ROOT`,
/// but for the build's records as today's files hold them, and the Mac `state/build/writer` names,
/// read from the real folder while the shadow has none of its own (term 1's first snapshot is made
/// from them, and its lead's saves write its own copies). Nothing it does writes the real folder.
#[derive(Clone, Debug)]
pub struct Overlay {
    real: Share,
    own: Share,
}

/// What a shadow run reads from the real folder.
const READ_THROUGH: [&str; 4] = ["state/build/manifest.json", "state/build/jobs.json", "state/build/pending.json", crate::pool::term::WRITER];

impl Overlay {
    /// The shadow of the project folder `root`.
    pub fn new(root: &Path) -> Overlay {
        Overlay { real: Share::new(root), own: Share::new(&root.join(SHADOW_ROOT)) }
    }
}

impl Nas for Overlay {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        self.own.create_new(path, bytes)
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.own.write_whole(path, bytes)
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.own.read(path)? {
            Some(b) => Ok(Some(b)),
            None if READ_THROUGH.contains(&path) => self.real.read(path),
            None => Ok(None),
        }
    }

    fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.own.exists(path)? || (READ_THROUGH.contains(&path) && self.real.exists(path)?))
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.own.list(dir)
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.own.remove(path)
    }
}

/// The driver's world: a NAS and this process's clocks (the awake one, `Instant`, stops while the
/// Mac sleeps).
struct Clocked<'a> {
    nas: &'a dyn Nas,
    start: Instant,
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
        crate::agent::jobs::now_s()
    }
    fn awake(&self) -> u64 {
        self.start.elapsed().as_secs()
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
    /// Whether this is a shadow run's (§12: acting on nothing).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shadow: bool,
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
    shadow: bool,
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
    /// The listings asked for and not made yet, the one being made, those made and not handed back,
    /// and when one last failed.
    asked: VecDeque<Listing>,
    making: Option<Making>,
    made: VecDeque<Listed>,
    failed_at: Option<Instant>,
    /// The heartbeat as last written (beat left out), and when.
    beat: Option<(Heartbeat, Instant)>,
    /// It stopped for good (another process holds its lock): why.
    pub stopped: Option<String>,
}

impl Side {
    /// This Mac's member in the pool, in folder `dir` (the agent's pool folder: its saved state)
    /// with its member id in `home` (the agent's folder), its lock in `locks` (the app's folder),
    /// over `nas`; `shadow`: a shadow run. None when another process holds the member's lock (this
    /// one runs no driver).
    pub fn open(home: &Path, dir: &Path, locks: &Path, app: &str, nas: SharedNas, shadow: bool) -> Result<Option<Side>> {
        let id = crate::pool::member_id(home)?;
        let Some(lock) = MemberLock::take(locks, &id)? else { return Ok(None) };
        std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
        let me = Member { id: id.clone(), host: crate::agent::cond::host_name(), app: app.to_string() };
        let (saved, written) = load_saved(&dir.join("saved.json"));
        let mail: Mail = std::fs::read(dir.join("mail.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let known: Vec<String> = std::fs::read(dir.join("members.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut members: BTreeSet<String> = known.into_iter().filter(|m| crate::pool::is_member_id(m)).collect();
        members.insert(id);
        let driver = Driver::new(me.clone(), saved, lock);
        Ok(Some(Side { me, driver: Some(driver), nas, start: Instant::now(), dir: dir.to_path_buf(), shadow, written, folders: Vec::new(), mail, mail_written: Vec::new(), mail_dirty: BTreeSet::new(), told: BTreeMap::new(), members, members_listed: None, members_queued: false, asked: VecDeque::new(), making: None, made: VecDeque::new(), failed_at: None, beat: None, stopped: None }))
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
            self.members.insert(m.to_string());
        }
    }

    /// The listings asked for and not handed back yet (asked, being made, made).
    pub fn listings_out(&self) -> usize {
        self.asked.len() + self.making.is_some() as usize + self.made.len()
    }

    /// A new driver for this member from its saved state, as a new process would make one (the
    /// shadow's, when the agent it shadows restarted): the lock let go and taken again.
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
        let heard = Heard { msgs, asks: give.asks, entries, listed: self.made.pop_front(), settled: give.settled, able: give.able, reassert: give.reassert, members: self.members.iter().cloned().collect() };
        let io = Clocked { nas: &*self.nas, start: self.start };
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
            self.members.insert(t.member);
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
        self.members.insert(to.to_string());
        if let Msg::Tell(keys) = &m {
            if self.told.get(to).is_some_and(|(k, at)| k == keys && at.elapsed() < TELL_AGAIN) {
                return;
            }
            self.told.insert(to.to_string(), (keys.clone(), Instant::now()));
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
        let hb = Heartbeat { pool: Beat { beat: 0, ..out.beat.clone() }, members: self.members.iter().cloned().collect(), shadow: self.shadow };
        if hb.pool.member.is_empty() {
            return;
        }
        if self.beat.as_ref().is_some_and(|(b, at)| *b == hb && at.elapsed() < BEAT_EVERY) {
            return;
        }
        let mut stamped = hb.clone();
        stamped.pool.beat = crate::agent::jobs::now_s();
        match serde_json::to_vec(&stamped).map_err(anyhow::Error::from).and_then(|b| self.nas.write_whole(&crate::pool::beat::path(&self.me.id), &b)) {
            Ok(()) => self.beat = Some((hb, Instant::now())),
            Err(e) => eprintln!("pool: heartbeat: {e:#}"),
        }
    }

    /// The members' heartbeats listed every ten minutes, off the loop.
    fn list_members(&mut self) {
        if self.members_listed.is_none_or(|t| t.elapsed() >= MEMBERS_EVERY) && !self.members_queued {
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
                self.failed_at = Some(Instant::now());
            }
            (Kind::Members, Ok(names)) => {
                self.members_listed = Some(Instant::now());
                self.members_queued = false;
                let found: Vec<String> = names.iter().filter_map(|n| n.strip_suffix(".json")).filter(|m| crate::pool::is_member_id(m)).map(str::to_string).collect();
                self.members.extend(found);
                if let Ok(b) = serde_json::to_vec(&self.members) {
                    crate::whole::write(&self.dir.join("members.json"), &b).ok();
                }
            }
            (Kind::Members, Err(e)) => {
                eprintln!("pool: listing the members: {e:#}");
                self.members_queued = false;
                self.members_listed = Some(Instant::now() - MEMBERS_EVERY + AGAIN);
            }
        }
    }

    /// The next listing begun, when none is being made: the journal's asked first.
    fn next_listing(&mut self) {
        if self.making.is_some() || self.failed_at.is_some_and(|t| t.elapsed() < AGAIN) {
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

/// The merge's checks of a journal entry, phase 1's (docs/pool.md §7.3; the steps' write-sets are
/// phase 4's): its done record is its own step's and names targets; every change is to a logical
/// name, as a content name of it; each raw tiles' archive it names is one of its area.
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
            if !store::naming::parse_content_name(c).is_some_and(|n| n.logical == l) {
                return Err(format!("{c} isn't a content name of {l}"));
            }
        }
    }
    for (area, p) in &h.raw {
        if !crate::rawpack::is_area(area) || !crate::rawpack::named_for(&p.name, area) {
            return Err(format!("{} isn't an archive of {area}", p.name));
        }
    }
    Ok(())
}

/// An event in words, for the log.
pub fn said(e: &Event) -> String {
    match e {
        Event::Made { term, how } => format!("made term {term} ({how})"),
        Event::TookUp { term, how, handed } => format!("took up term {term} ({how}){}", if handed.is_some() { ", the coordinator's state handed over" } else { "" }),
        Event::SteppedDown { term, why } => format!("no longer leads term {term}: {why}"),
        Event::Handover { term, to, what } => format!("its handover of term {term} to {to}: {what}"),
        Event::Merged { applied, overtaken, refused, listed } => format!("merged {} entr{} ({overtaken} passed over, {refused} refused){}", applied.len(), if applied.len() == 1 { "y" } else { "ies" }, if *listed { ", a listing's" } else { "" }),
        Event::Waits { what, why } => format!("waits to {what}: {why}"),
        Event::Failed { what, why } => format!("couldn't {what}: {why}"),
    }
}

/// What a process of the agent is in the pool while it's on: decided as it starts, by the terms
/// (`Run::open`), and kept for its life. A process whose member takes up a term, or stops leading
/// one, restarts into its new part once its first job's slot is free (phase 1: the lead's jobs run
/// in its own process; phase 2 moves them into its slots).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// It leads: it plans, grants jobs through its coordinator, does the lead's duties.
    Lead,
    /// A member: it asks the lead for work, as a helper did.
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
}

impl Gates {
    fn of(o: &Out) -> Gates {
        Gates { term: o.term, leads: o.leads, duties: o.duties, settle: o.settle, caught_up: o.caught_up, fresh: o.fresh, listed_at: o.listed_at }
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
/// the build Mac's coordinator gave it, numbered by the clock in ms, far below).
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
    nas_drained: Option<Instant>,
    nas_empty: bool,
    /// The records (term, seq) last written to today's files; the raw tiles' archives named.
    pub today: Option<(u64, u64)>,
    pub named_raw: BTreeSet<String>,
    /// The coordinator's state per term as last written, the history's last event written, the
    /// devices last copied to the NAS.
    pub state_written: Option<(u64, Vec<u8>)>,
    pub history_seq: u64,
    pub devices_written: Option<Vec<u8>>,
    /// The terms this process led (a take-up of the next one after them keeps its coordinator's
    /// state; one after another lead's loads that lead's).
    pub led: BTreeSet<u64>,
}

impl Run {
    pub fn new(side: Side, role: Role, gates: Gates) -> Run {
        Run { side, role, gates, entries: Vec::new(), settled: None, reassert: false, restart: None, drained: BTreeSet::new(), drain_n: 0, nas_drained: None, nas_empty: false, today: None, named_raw: BTreeSet::new(), state_written: None, history_seq: 0, devices_written: None, led: BTreeSet::new() }
    }

    /// A process's first step, which says its part (`Role`): the jobs an earlier process left handed
    /// over (`left_jobs`, in the agent's folder `home`).
    pub fn start(side: Side, home: &Path) -> (Run, Out) {
        let mut r = Run::new(side, Role::Member, Gates::default());
        let me = r.side.member().id.clone();
        r.entries = left_jobs(home, &me).into_iter().map(|(e, d)| (e, Some(d))).collect();
        let out = r.step(false);
        r.role = if out.leads.is_some() { Role::Lead } else { Role::Member };
        r.restart = out.stop.as_ref().map(|why| format!("this process left the pool: {why}"));
        (r, out)
    }

    /// One step: what the agent gathered handed over, the gates kept; the role's change noted (it
    /// restarts into its new part).
    pub fn step(&mut self, able: bool) -> Out {
        let give = Give { entries: std::mem::take(&mut self.entries), settled: self.settled.take(), able, reassert: self.reassert, asks: Vec::new() };
        let out = self.side.step(give, &check);
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
        match (self.role, out.leads) {
            (Role::Lead, None) if out.stop.is_none() => {
                self.restart.get_or_insert_with(|| "it no longer leads".into());
            }
            (Role::Member, Some(e)) => {
                self.restart.get_or_insert_with(|| format!("it leads term {e}"));
            }
            _ => {}
        }
        for e in &out.events {
            eprintln!("pool: {}", said(e));
        }
        out
    }

    /// The next number for a hand-off drained from before the pool (`DRAINED` on, by the clock).
    fn next_drained(&mut self) -> journal::LeaseId {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.drain_n = (self.drain_n + 1).max(ms);
        journal::LeaseId { term: 0, n: DRAINED + self.drain_n }
    }

    /// The hand-offs from before the pool this member holds, drained into its journal entries
    /// (§12, seeding and draining): a lead's coordinator's journal on its disk (`journal`: the
    /// helpers' hand-offs it took and didn't merge), and every two minutes the NAS's hand-off
    /// folders (`nas`: an older helper's), until that's found empty; any member's outbox
    /// (`outbox`: a helper's jobs not handed back). Each is removed once a saved state holding its
    /// entry is on disk.
    pub fn drain(&mut self, journal: Option<&Path>, nas: Option<&Path>, outbox: &Path) {
        let me = self.side.member().id.clone();
        let mut found: Vec<(PathBuf, Handoff)> = Vec::new();
        if let Some(j) = journal {
            match crate::handoff::waiting_in(j) {
                Ok(hs) => found.extend(hs),
                Err(e) => eprintln!("pool: draining {}: {e:#}", j.display()),
            }
        }
        if let Some(n) = nas.filter(|_| !self.nas_empty && self.nas_drained.is_none_or(|t| t.elapsed() >= Duration::from_secs(120))) {
            self.nas_drained = Some(Instant::now());
            match crate::handoff::waiting_in(n) {
                Ok(hs) => {
                    self.nas_empty = hs.iter().all(|(p, _)| self.drained.contains(p));
                    found.extend(hs);
                }
                Err(e) => eprintln!("pool: draining {}: {e:#}", n.display()),
            }
        }
        for (p, h) in found {
            if self.drained.contains(&p) {
                continue;
            }
            let lease = self.next_drained();
            let step = h.done.as_ref().map_or_else(|| "handoff".to_string(), |d| d.0.clone());
            self.drained.insert(p.clone());
            self.entries.push((Entry { member: me.clone(), lease, step, handoff: h, at: crate::agent::jobs::now_s() }, Some(p)));
        }
        for (e, dir) in outbox_entries(outbox, &me) {
            if self.drained.insert(dir.clone()) {
                self.entries.push((e, Some(dir)));
            }
        }
    }
}

/// A term taken up (`Event::TookUp`), as the lead's coordinator takes it (§6.2, §7.5): the leases it
/// grants from now in that term; the state handed over with it (a handover's), else the term
/// before's own (a takeover's, or its own before a restart), loaded; its own host's leases from
/// before dropped (its jobs ended with the process that ran them).
pub fn took_up(run: &mut Run, coord: Option<&crate::coord::Coordinator>, out: &Out, host: &str) {
    let Some(c) = coord else { return };
    for e in &out.events {
        let Event::TookUp { term, handed, .. } = e else { continue };
        c.set_term(*term);
        // (A take-up after a term this process led keeps what it has: it's newer than the files.)
        if run.led.contains(&(term - 1)) && handed.is_none() {
            continue;
        }
        let state: Option<crate::coord::PoolState> = match handed {
            Some(v) => serde_json::from_value(v.clone()).ok(),
            None => run.side.nas().read(&state_path(term - 1)).ok().flatten().and_then(|b| serde_json::from_slice(&b).ok()),
        };
        if let Some(st) = state {
            match c.load_pool_state(&st) {
                Ok(n) => eprintln!("pool: term {term}'s coordinator took up {n} lease{} {}", if n == 1 { "" } else { "s" }, if handed.is_some() { "handed over" } else { "from the term before" }),
                Err(e) => eprintln!("pool: the coordinator's state for term {term}: {e:#}"),
            }
            for l in c.drop_workers(&[host.to_string(), crate::agent::second_worker(host)]) {
                eprintln!("pool: {}'s lease ended with the process before this one", l.what());
            }
        }
    }
    if let Some(e) = out.leads {
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
    if crate::agent::claims::SHARED.contains(&step) {
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
/// as an entry of this member under its lease from the build Mac's coordinator (term 0: before any
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
        if let Ok(Some(e)) = entry_of(&d, member, journal::LeaseId { term: 0, n: lease }, &w.step, &done, crate::agent::jobs::now_s()) {
            out.push((e, d));
        }
    }
    out
}

/// The coordinator's state of term `term` on the NAS (§6.2, §7.5).
pub fn state_path(term: u64) -> String {
    format!("state/coord/term/{term}/state.json")
}

/// The pool's copies of the workers' token and the accepted devices (§8: the same on every member).
pub const TOKEN: &str = "state/coord/token";
pub const DEVICES: &str = "state/coord/devices.json";

/// The workers' token and the accepted devices made the pool's (§12, seeding): this Mac's (the
/// build Mac's, as the pool is switched on) copied to the NAS when it has none (create-new: the
/// first lead's stay), else the NAS's copied here before its coordinator starts, so a page a lead
/// before accepted keeps working with this one.
pub fn seed(nas: &dyn Nas, coord: &Path) -> Result<()> {
    std::fs::create_dir_all(coord)?;
    for (theirs, ours) in [(TOKEN, "workers-token"), (DEVICES, "devices.json")] {
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
        std::fs::write(r.join("state/build/writer"), crate::agent::cond::host_name()).unwrap();
        r
    }

    fn side(root: &Path, home: &Path) -> Side {
        let nas: SharedNas = Arc::new(Share::new(root));
        Side::open(home, &home.join("pool"), home.parent().unwrap(), "development", nas, false).unwrap().expect("its lock")
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Steps `s` until `done` says so (each step's listings made off its loop meanwhile), at most
    /// ten times.
    fn until(s: &mut Side, give: impl Fn() -> Give, done: impl Fn(&Out) -> bool) -> Out {
        for _ in 0..40 {
            let out = s.step(give(), &any);
            if done(&out) {
                return out;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("not done in 40 steps");
    }

    fn unit_entry(member: &str, lease: LeaseId, target: &str, key: &str) -> Entry {
        let l = format!("base/{}", target.replace('/', "-"));
        let h = Handoff { changes: [(l.clone(), Some(format!("{l}.2222222222222222.base")))].into(), done: Some(("unit".into(), vec![(target.into(), key.into())])), ..Default::default() };
        Entry { member: member.into(), lease, step: "unit".into(), handoff: h, at: crate::agent::jobs::now_s() }
    }

    #[test]
    fn the_switch_is_a_file_on_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        assert_eq!(mode(&r), Some(Mode::Off));
        std::fs::create_dir_all(r.join("state/pool")).unwrap();
        std::fs::write(r.join(SHADOW), "").unwrap();
        assert_eq!(mode(&r), Some(Mode::Shadow));
        std::fs::write(r.join(ENABLED), "").unwrap();
        assert_eq!(mode(&r), Some(Mode::On), "on wins over shadow");
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
        assert!(Side::open(&home, &home.join("pool"), home.parent().unwrap(), "development", nas, false).unwrap().is_none(), "a second process of the member runs none");
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
        let mut a = Side::open(&home, &home.join("pool"), home.parent().unwrap(), "development", nas.clone(), false).unwrap().unwrap();
        a.step(Give { able: true, ..Default::default() }, &any);
        std::thread::sleep(Duration::from_millis(200));
        let out = a.step(Give { able: true, ..Default::default() }, &any);
        assert!(!out.caught_up && a.failed_at.is_some(), "the failed one isn't handed back");
        a.failed_at = Some(Instant::now() - AGAIN);
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
        // The coordinator's journal here, the NAS's hand-off folders, a helper's outbox.
        let journal = home.join("coord/journal");
        crate::handoff::write(&journal.join("m1"), &done("6/2/1", "k21")).unwrap();
        crate::handoff::write(&crate::handoff::dir(&r, "old-m1"), &done("6/2/2", "k22")).unwrap();
        let ob = home.join("outbox/1791328399374");
        std::fs::create_dir_all(&ob).unwrap();
        std::fs::write(ob.join("work.json"), serde_json::to_vec(&crate::agent::build::Work { step: "unit".into(), targets: vec![("6/2/3".into(), "k23".into())] }).unwrap()).unwrap();
        crate::handoff::write(&ob, &Handoff { changes: [("base/6-2-3".to_string(), Some("base/6-2-3.3333333333333333.base".to_string()))].into(), ..Default::default() }).unwrap();
        std::fs::write(ob.join("result.json"), serde_json::json!({ "ok": true, "done": ["unit", [["6/2/3", "k23"]]] }).to_string()).unwrap();
        // (A task's folder: left to its broker.)
        std::fs::create_dir_all(home.join("outbox/1791328399375")).unwrap();
        run.drain(Some(&journal), Some(&crate::handoff::nas_base(&r)), &home.join("outbox"));
        let leases: Vec<LeaseId> = run.entries.iter().map(|(e, _)| e.lease).collect();
        assert_eq!(leases.len(), 3, "{leases:?}");
        assert!(leases.iter().all(|l| l.term == 0));
        assert!(leases.contains(&LeaseId { term: 0, n: 1791328399374 }), "a helper's keeps its lease");
        assert_eq!(leases.iter().filter(|l| l.n >= DRAINED).count(), 2);
        // Drained once in this process, whatever the next look finds.
        run.drain(Some(&journal), Some(&crate::handoff::nas_base(&r)), &home.join("outbox"));
        assert_eq!(run.entries.len(), 3);
        // Merged by the lead's step; their sources removed once its saved state holds them.
        let out = run.step(true);
        assert_eq!(out.leads, Some(1));
        let rec = run.side.driver().records().unwrap();
        assert_eq!((rec.keys.recorded("unit", "6/2/1"), rec.keys.recorded("unit", "6/2/2"), rec.keys.recorded("unit", "6/2/3")), (Some("k21"), Some("k22"), Some("k23")));
        assert_eq!(rec.manifest.get("base/6-2-3").map(String::as_str), Some("base/6-2-3.3333333333333333.base"));
        assert!(crate::handoff::waiting_in(&journal).unwrap().is_empty() && crate::handoff::waiting(&r).unwrap().is_empty() && !ob.exists());
        assert!(home.join("outbox/1791328399375").exists());
        // Term 1's records written to today's files too, for their readers.
        assert_eq!(crate::agent::build::Keys::load(&r).recorded("unit", "6/2/2"), Some("k22"));
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
    fn the_token_and_devices_are_the_pools() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path().join("nas");
        let nas = Share::new(&r);
        let (m4, m1) = (d.path().join("m4/coord"), d.path().join("m1/coord"));
        std::fs::create_dir_all(&m4).unwrap();
        std::fs::write(m4.join("workers-token"), "t0k3n").unwrap();
        std::fs::write(m4.join("devices.json"), r#"{"accepted": []}"#).unwrap();
        // The build Mac's, copied to the NAS as the pool begins.
        seed(&nas, &m4).unwrap();
        assert_eq!(nas.read(TOKEN).unwrap().as_deref(), Some(&b"t0k3n"[..]));
        // Another lead's coordinator starts with them.
        std::fs::create_dir_all(&m1).unwrap();
        std::fs::write(m1.join("workers-token"), "other").unwrap();
        seed(&nas, &m1).unwrap();
        assert_eq!(std::fs::read_to_string(m1.join("workers-token")).unwrap(), "t0k3n");
        assert_eq!(std::fs::read_to_string(m1.join("devices.json")).unwrap(), r#"{"accepted": []}"#);
        // The first's stay.
        seed(&nas, &m1).unwrap();
        assert_eq!(nas.read(TOKEN).unwrap().as_deref(), Some(&b"t0k3n"[..]));
    }

    #[test]
    fn a_shadow_run_writes_nothing_of_the_real_folder() {
        let d = tempfile::tempdir().unwrap();
        let r = root(d.path());
        let before: Vec<(PathBuf, Vec<u8>)> = walk(&r);
        let home = d.path().join("s/agent");
        let nas: SharedNas = Arc::new(Overlay::new(&r));
        let mut s = Side::open(&home, &home.join("pool-shadow"), home.parent().unwrap(), "development", nas, true).unwrap().unwrap();
        let id = s.member().id.clone();
        let e = unit_entry(&id, LeaseId { term: 1, n: 1 }, "6/1/2", "k2");
        until(&mut s, || Give { able: true, ..Default::default() }, |o| o.caught_up);
        let out = until(&mut s, || Give { entries: vec![(e.clone(), None)], able: true, ..Default::default() }, |o| o.events.iter().any(|e| matches!(e, Event::Merged { .. })));
        assert_eq!(out.leads, Some(1));
        // Its records from the real today's files, its merge on top; written under the shadow.
        let rec = s.driver().records().unwrap();
        assert_eq!((rec.keys.recorded("unit", "6/1/1"), rec.keys.recorded("unit", "6/1/2")), (Some("k1"), Some("k2")));
        let sh = r.join(SHADOW_ROOT);
        assert!(sh.join("state/build/terms/1.json").exists() && sh.join("state/build/term/1/records.json").exists() && sh.join(crate::pool::beat::path(&id)).exists());
        let after: Vec<(PathBuf, Vec<u8>)> = walk(&r).into_iter().filter(|(p, _)| !p.starts_with(&sh)).collect();
        assert_eq!(after, before, "the real folder as it was");
        let hb: Heartbeat = serde_json::from_slice(&std::fs::read(sh.join(crate::pool::beat::path(&id))).unwrap()).unwrap();
        assert!(hb.shadow);
    }

    fn walk(d: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push((p.clone(), std::fs::read(&p).unwrap()));
            }
        }
        out.sort();
        out
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
        let mut none = e;
        none.handoff.done = Some(("unit".into(), Vec::new()));
        assert!(check(&none, &r).is_err());
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
}
