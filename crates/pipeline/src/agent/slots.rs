//! **jobs**' own thread (docs/agent-parts.md §4.5, task #871 step 5): what the running jobs need
//! at once, whatever the loop waits on (a plan, a hung share):
//!
//! - **the reaper:** each running job's end seen the moment its group's head writes its status
//!   (crate::agent::jobs::status_path), or its group empties, and the loop woken for it
//!   (`ControlMsg::Ended`), so the next job starts within a moment rather than after the loop's
//!   20-s wait;
//! - **cancels** (`scenic job cancel`, the menu bar, the page: `cancel-requests/`): taken the
//!   moment control sees them, the jobs running their target stopped at once (SIGTERM, SIGKILL
//!   after `GRACE`), then handed to the loop (`Handle::cancels`), which records each as given back,
//!   tells the lead and answers (`Agent::take_cancel`): a cancel is passed on before its job is
//!   signalled, so the loop never sees the end before the cancel;
//! - **another member's hold ask** (`scenic pause --member`, the page's): taken at once, written to
//!   that member's hold on the NAS (`take_member_hold`), under the pool's lock if free (asked again
//!   each tick while a step holds it).
//!
//! What it knows of the jobs is what the loop last published (`JobRef`s, shared with control,
//! which tells each to drain under a hold). The slots themselves, their starts, guards and
//! hand-offs are still the loop's (steps 6–7). In line (a single loop's, a dry run's, the tests')
//! the loop takes the cancels itself (`Handle::look`).

use super::bus::{ControlMsg, Snap};
use crate::control::{self, Cancel, HoldAnswer, HoldState};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the reaper looks at the jobs' ends, and an ask waiting is tried again.
pub const TICK: Duration = Duration::from_millis(100);
/// A cancelled job's group killed this long after its SIGTERM, if any of it is left.
pub const GRACE: Duration = Duration::from_secs(30);

/// A running job as the loop last published it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRef {
    /// Its control channel (crate::control::CONTROL_ENV): "drain" asks it to stop at a safe point.
    pub chan: PathBuf,
    /// Its process group (0: none, a job taken over as ended).
    pub pgid: i32,
    /// Where its group's head writes its exit status.
    pub status: PathBuf,
    pub step: Option<String>,
    pub targets: Vec<String>,
}

impl JobRef {
    /// A job known by its channel alone (a test's).
    #[cfg(test)]
    pub fn on_chan(chan: PathBuf) -> JobRef {
        JobRef { chan, pgid: 0, status: PathBuf::new(), step: None, targets: Vec::new() }
    }

    /// Whether the cancel `c` is of this job.
    pub fn hit(&self, c: &Cancel) -> bool {
        c.step.as_ref().is_none_or(|s| self.step.as_ref() == Some(s)) && self.targets.iter().any(|t| *t == c.target)
    }

    fn ended(&self) -> bool {
        self.status.exists() || (self.pgid > 0 && !crate::sys::signal_group(self.pgid, crate::sys::Signal::Probe))
    }
}

/// What the member's hold asks need of the agent: its pool (its lock taken only when free), this
/// Mac's app, the NAS's root as **nas** has it, and this Mac's hold.
#[derive(Clone)]
pub struct Members {
    pub pool: Option<super::pool::Shared>,
    pub app: String,
    pub nas: Arc<Snap<super::nas::NasView>>,
    pub hold: Arc<Snap<HoldState>>,
}

/// The thread's own state.
struct Part {
    home: PathBuf,
    jobs: Arc<Snap<Vec<JobRef>>>,
    wake: Sender<ControlMsg>,
    cancels: Sender<Cancel>,
    members: Members,
    set_hold: super::asks::Setter,
    /// Jobs seen ended (their status file and group), told once.
    told: Vec<(PathBuf, i32)>,
    /// Groups signalled to stop, killed past their time.
    stopping: BTreeMap<i32, Instant>,
}

impl Part {
    fn tick(&mut self) {
        let jobs = self.jobs.get();
        // (Told once each; forgotten once the loop no longer lists it.)
        self.told.retain(|t| jobs.iter().any(|j| (&j.status, j.pgid) == (&t.0, t.1)));
        for j in jobs.iter() {
            if !self.told.contains(&(j.status.clone(), j.pgid)) && j.ended() {
                self.told.push((j.status.clone(), j.pgid));
                self.wake.send(ControlMsg::Ended).ok();
            }
        }
        let now = Instant::now();
        self.stopping.retain(|&pgid, at| {
            if now < *at {
                return true;
            }
            if crate::sys::signal_group(pgid, crate::sys::Signal::Probe) {
                eprintln!("agent: a cancelled job's group {pgid} still runs {GRACE:?} on: killed");
                crate::sys::signal_group(pgid, crate::sys::Signal::Kill);
            }
            false
        });
        if control::peek_hold_request(&self.home).is_some_and(|r| r.member.is_some()) {
            let set = &self.set_hold;
            take_member_hold(&self.home, &self.members, &mut |st| set.set(st));
        }
    }

    /// The cancels waiting taken: each passed to the loop, then its jobs here signalled to stop.
    fn take_cancels(&mut self) {
        for ask in control::take_cancels(&self.home, super::jobs::now_s()) {
            let hit: Vec<i32> = self.jobs.get().iter().filter(|j| j.pgid > 0 && j.hit(&ask)).map(|j| j.pgid).collect();
            let target = ask.target.clone();
            self.cancels.send(ask).ok();
            self.wake.send(ControlMsg::Ask).ok();
            for pgid in hit {
                eprintln!("agent: {target} cancelled: stopping its job's group {pgid} now");
                crate::sys::signal_group(pgid, crate::sys::Signal::Cont);
                crate::sys::signal_group(pgid, crate::sys::Signal::Term);
                self.stopping.insert(pgid, Instant::now() + GRACE);
            }
        }
    }
}

/// The loop's handle on **jobs**' thread.
pub struct Handle {
    /// The running jobs as the loop last published them (shared with control).
    pub jobs: Arc<Snap<Vec<JobRef>>>,
    /// The cancels taken, for the loop to record and answer.
    cancels: Receiver<Cancel>,
    home: PathBuf,
    /// What control tells it (a cancel, a member's hold ask): a look at once.
    tx: Sender<ControlMsg>,
    stop: Arc<AtomicBool>,
    /// Until `spawn`: what the thread is made of.
    inline: Option<(Receiver<ControlMsg>, Sender<Cancel>)>,
}

impl Handle {
    pub fn new(home: PathBuf, jobs: Arc<Snap<Vec<JobRef>>>) -> Handle {
        let (tx, rx) = mpsc::channel();
        let (ctx, crx) = mpsc::channel();
        Handle { jobs, cancels: crx, home, tx, stop: Arc::new(AtomicBool::new(false)), inline: Some((rx, ctx)) }
    }

    /// Where control tells it of a cancel or a member's hold ask.
    pub fn teller(&self) -> Sender<ControlMsg> {
        self.tx.clone()
    }

    /// Its thread, waking the loop on `wake`.
    pub fn spawn(&mut self, wake: Sender<ControlMsg>, members: Members, set_hold: super::asks::Setter) {
        let Some((rx, cancels)) = self.inline.take() else { return };
        let mut p = Part {
            home: self.home.clone(),
            jobs: self.jobs.clone(),
            wake,
            cancels,
            members,
            set_hold,
            told: Vec::new(),
            stopping: BTreeMap::new(),
        };
        p.take_cancels();
        let stop = self.stop.clone();
        let started = std::thread::Builder::new().name("jobs".into()).spawn(move || loop {
            match rx.recv_timeout(TICK) {
                Ok(_) => p.take_cancels(),
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if stop.load(Ordering::SeqCst) {
                return;
            }
            p.tick();
        });
        if let Err(e) = started {
            eprintln!("agent: the jobs' thread: {e}");
        }
    }

    /// The cancels to record now: those the thread took (their jobs already told to stop), or, in
    /// line, those waiting in the folder.
    pub fn cancels(&mut self) -> Vec<Cancel> {
        if self.inline.is_some() {
            return control::take_cancels(&self.home, super::jobs::now_s());
        }
        self.cancels.try_iter().collect()
    }

    pub fn spawned(&self) -> bool {
        self.inline.is_none()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Another member's hold ask (`hold-request.json` with a member) taken and answered: that member's
/// hold written on the NAS (later than the setting there), or this Mac's own set (`set`) when the
/// name is this Mac's. Left for the next try while the pool's step holds its lock.
pub fn take_member_hold(home: &std::path::Path, m: &Members, set: &mut dyn FnMut(HoldState) -> bool) {
    // (Never waiting on the pool's lock: its step may hang on the NAS.)
    let pooled = m.pool.as_ref().map(super::pool::try_lock);
    if matches!(pooled, Some(None)) {
        return;
    }
    let Some(req) = control::take_hold_request(home).filter(|r| r.member.is_some()) else { return };
    let what = if req.held.is_some() { "paused" } else { "going on" };
    let me = pooled.as_ref().and_then(|p| p.as_ref()).map(|p| p.side.member().id.clone());
    let target: Result<Option<(String, String)>, String> = match req.member.as_deref() {
        None => Ok(None),
        Some(name) => match pooled.as_ref() {
            Some(Some(run)) => super::lead::resolve(run, name).and_then(|id| {
                if me.as_deref() == Some(id.as_str()) {
                    return Ok(None);
                }
                // (An app from before this Mac's may not know a pause of its own: refused, not
                // said done.)
                match run.side.heartbeat(&id) {
                    Some(h) if crate::pool::term::app_at_least(&h.pool.app, &m.app) => Ok(Some((id, name.to_string()))),
                    Some(_) => Err(format!(
                        "{name}'s app is too old to pause alone (older than this Mac's): update it, or pause the whole build"
                    )),
                    None => Err(format!("{name}'s heartbeat can't be read now: ask again")),
                }
            }),
            _ => Err("this Mac isn't in the pool: it can pause only itself".to_string()),
        },
    };
    drop(pooled);
    let (ok, said) = match target {
        Ok(None) => {
            // (This Mac's own ask wins over any earlier setting, whatever another Mac's clock put
            // on it.)
            let at = req.at.max(m.hold.get().at + 1);
            if set(HoldState { held: req.held.clone(), at }) {
                (true, format!("this Mac is {what}"))
            } else {
                (false, "a later setting of this Mac's pause stands".to_string())
            }
        }
        Ok(Some((id, name))) => match m.nas.get().root.clone() {
            None => (false, "the NAS isn't reachable from this Mac".to_string()),
            Some(r) => {
                // (Later than the setting there, whatever the two Macs' clocks say. Only a missing
                // file counts as none set; one that can't be read or parsed now could hide a later
                // setting: asked again, never a false ok.)
                let path = r.join(control::hold_path(&id));
                let there = match super::nasread::read_switch(&path) {
                    Ok(t) => serde_json::from_str::<HoldState>(&t).map(|h| h.at).ok(),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(0),
                    Err(_) => None,
                };
                match there {
                    None => (false, format!("{name}'s pause on the NAS can't be read now: ask again")),
                    Some(there) => {
                        let st = HoldState { held: req.held.clone(), at: req.at.max(there + 1) };
                        match super::write_hold(&path, &st) {
                            Ok(()) => (true, format!("{name} is {what} (it takes it up within a minute)")),
                            Err(e) => (false, format!("{name}'s pause on the NAS: {e:#}")),
                        }
                    }
                }
            }
        },
        Err(e) => (false, e),
    };
    eprintln!("agent: {said}");
    control::answer_hold(home, &HoldAnswer { ask: req, said, ok });
}
