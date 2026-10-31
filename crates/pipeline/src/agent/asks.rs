//! **control** (docs/agent-parts.md §4.1, task #871 step 3; its first slice task #846): the owner's
//! asks of this Mac's agent taken up at once, on a thread of their own, whatever the loop waits on.
//!
//! The asks come as files in the agent's folder (crate::control: `hold-request.json`,
//! `quit-request.json`, `cancel-requests/`, the settings' files), whoever writes them: the menu bar,
//! `scenic`, an older CLI, or the agent's HTTP endpoint (crate::coord's `/work/hold`, `/work/quit`,
//! `/work/settings`, which write the same files). A watcher on the folder (kqueue on macOS) looks
//! the moment one changes, and a sweep every `SWEEP` backs it up (an event coalesced or missed).
//!
//! What control owns, published as snapshots (crate::agent::bus) for the loop and the beat:
//! - this Mac's hold (`HoldState`, the later `at` winning, kept in `hold.json`): its own ask taken,
//!   kept and answered, the running jobs told to stop at their next safe point ("drain" in each's
//!   channel, `jobs` as the loop last published them), and the beat told, so the menu bar and the
//!   page see "pausing" within a second. The loop sets it too (another Mac's setting read on the
//!   NAS), through `Handle::set_hold`: control applies every setting, in order;
//! - the app's ask to quit;
//! - the owner's settings: the disk room target, the in-use share, the build's memory.
//!
//! Each change wakes the loop's wait (`ControlMsg`). Asks that need the jobs or the pool (a cancel,
//! a hold ask for another member) are told to **jobs**' thread (crate::agent::slots, step 5), which
//! takes them at once; the loop is woken for them while that thread doesn't run. It never touches
//! the NAS, so it can't hang on one.

use super::bus::{BeatMsg, ControlMsg, Snap};
use super::{buildmem, room, share};
use crate::control::{self, HoldAnswer, HoldState, Quit};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// The watcher's backstop: a look at least this often.
pub const SWEEP: Duration = Duration::from_secs(2);

/// The owner's settings of this Mac, as last read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// The disk room target (`scenic room`, the menu bar).
    pub room: Option<room::Target>,
    /// The in-use share (`scenic in-use`).
    pub share: Option<share::Share>,
    /// The memory given the build here (`scenic memory`, the menu bar).
    pub memory: Option<buildmem::Memory>,
}

impl Settings {
    fn read(home: &Path) -> Settings {
        Settings { room: room::target(home), share: share::read(home), memory: buildmem::read(home) }
    }
}

/// What the loop asks of control.
enum In {
    /// The folder changed, or the sweep's time came.
    Look,
    /// A hold set (another Mac's, read on the NAS): whether it stood.
    SetHold(HoldState, mpsc::SyncSender<bool>),
    Stop,
}

/// Control's own state.
pub struct Control {
    home: PathBuf,
    hold: Arc<Snap<HoldState>>,
    quit: Arc<Snap<Option<Quit>>>,
    settings: Arc<Snap<Settings>>,
    jobs: Arc<Snap<Vec<super::slots::JobRef>>>,
    beat: Sender<BeatMsg>,
    wake: Sender<ControlMsg>,
    /// **jobs**' thread (crate::agent::slots), told of a cancel or a member's hold ask; the loop
    /// woken for them while it doesn't run.
    jobs_part: Option<Sender<ControlMsg>>,
    /// The asks left as last told of (a member's hold ask, the cancels).
    left: String,
}

impl Control {
    /// One look at the folder: this Mac's own hold ask taken up, the quit ask and the settings as
    /// they are now, the loop woken for what changed and what's its.
    pub fn look(&mut self) {
        if let Some(req) = control::peek_hold_request(&self.home).filter(|r| r.member.is_none()) {
            if let Some(req) = control::take_hold_request(&self.home).filter(|r| r.member.is_none() && *r == req) {
                // (This Mac's own ask wins over any earlier setting, whatever another Mac's clock
                // put on it.)
                let st = HoldState { held: req.held.clone(), at: req.at.max(self.hold.get().at + 1) };
                let what = if st.held.is_some() { "paused" } else { "going on" };
                self.set_hold(st);
                eprintln!("agent: this Mac is {what} (taken up at once)");
                control::answer_hold(
                    &self.home,
                    &HoldAnswer { ask: req, said: format!("this Mac is {what}"), ok: true },
                );
            }
        }
        let quit = control::peek_quit(&self.home);
        if quit != *self.quit.get() {
            if let Some(q) = &quit {
                eprintln!("agent: Scenic quits ({}, {:?})", q.by, q.mode);
            }
            self.quit.set(quit);
            self.tell(ControlMsg::Quit);
        }
        let s = Settings::read(&self.home);
        let was = self.settings.get();
        if s != *was {
            said_settings(&was, &s);
            self.settings.set(s);
            self.tell(ControlMsg::Settings);
        }
        // (The jobs' asks: told once for each new state of them.)
        let member = control::peek_hold_request(&self.home).filter(|r| r.member.is_some());
        let left = format!("{member:?} {:?}", control::peek_cancels(&self.home));
        if left != self.left {
            if member.is_some() || left.contains("Cancel") {
                self.jobs_part.as_ref().unwrap_or(&self.wake).send(ControlMsg::Ask).ok();
            }
            self.left = left;
        }
    }

    /// This Mac's hold set to `st` unless a later setting stands (whether it did): kept in its
    /// folder, published, the jobs told to drain when held, the beat and the loop told.
    pub fn set_hold(&mut self, st: HoldState) -> bool {
        let was = self.hold.get();
        if st.at < was.at {
            return false;
        }
        if st == *was {
            return true;
        }
        if st.held.is_some() != was.held.is_some() {
            eprintln!("agent: {}", st.held.as_ref().map_or("this Mac goes on building".to_string(), |h| h.why()));
        }
        let kept = serde_json::to_vec(&st)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.home.join(control::HOLD), &b));
        if let Err(e) = kept {
            eprintln!("agent: keeping this Mac's pause: {e:#}");
        }
        let held = st.held.is_some();
        self.hold.set(st);
        // (Held: every job told at once; going on, the loop tells them, as it knows which other
        // drain, the memory guard's say, still stands.)
        if held {
            for c in self.jobs.get().iter().map(|j| &j.chan) {
                if let Err(e) = std::fs::write(c, b"drain") {
                    eprintln!("agent: asking {} to stop: {e}", c.display());
                }
            }
        }
        self.tell(ControlMsg::Hold);
        true
    }

    /// The beat and the loop told of a change.
    fn tell(&self, m: ControlMsg) {
        self.beat.send(BeatMsg::Changed).ok();
        self.wake.send(m).ok();
    }
}

/// A change of the owner's settings, logged.
fn said_settings(was: &Settings, now: &Settings) {
    if now.room.as_ref().map(|t| t.bytes) != was.room.as_ref().map(|t| t.bytes) {
        match &now.room {
            Some(t) => eprintln!(
                "agent: the disk room target is {} ({}): its caches freed to it, and no job starts that would cross it",
                room::size(t.bytes),
                t.by
            ),
            None => eprintln!("agent: the disk room target is off"),
        }
    }
    if now.share != was.share {
        eprintln!(
            "agent: the in-use share is {} cores kept while the owner uses this Mac ({})",
            share::cores(now.share.as_ref(), super::mac_cores()),
            now.share.as_ref().map_or("the default", |s| s.by.as_str())
        );
    }
    if now.memory.as_ref().map(|m| m.mb) != was.memory.as_ref().map(|m| m.mb) {
        match &now.memory {
            Some(m) => {
                eprintln!("agent: the build's memory is {} MB ({}), within the memory guard's limit", m.mb, m.by)
            }
            None => eprintln!("agent: the build's memory is the memory guard's limit (none set)"),
        }
    }
}

/// The loop's handle on control: in line (each `look` the loop's) until `spawn`, then on its
/// thread with the folder's watcher.
pub struct Handle {
    pub hold: Arc<Snap<HoldState>>,
    pub quit: Arc<Snap<Option<Quit>>>,
    pub settings: Arc<Snap<Settings>>,
    /// The running jobs (each one's channel, crate::control::CONTROL_ENV), as the loop last
    /// published them; shared with **jobs**' thread.
    pub jobs: Arc<Snap<Vec<super::slots::JobRef>>>,
    /// What changed, for the loop's wait (control's, and **jobs**' thread's: `waker`).
    pub woken: Receiver<ControlMsg>,
    wake: Sender<ControlMsg>,
    home: PathBuf,
    tx: Sender<In>,
    inline: Option<(Control, Receiver<In>)>,
    /// The watcher's backstop (`SWEEP`; a test's never, so only an event takes an ask up).
    sweep: Duration,
}

/// This Mac's hold set from another thread (**jobs**', taking a member's ask that names this Mac),
/// through control, in order with its own.
#[derive(Clone)]
pub struct Setter(Sender<In>);

impl Setter {
    /// `st` set unless a later setting stands; whether it was.
    pub fn set(&self, st: HoldState) -> bool {
        let (answer, answered) = mpsc::sync_channel(1);
        if self.0.send(In::SetHold(st, answer)).is_err() {
            return false;
        }
        // (Control never waits on the NAS: answered in moments.)
        answered.recv_timeout(Duration::from_secs(10)).unwrap_or(false)
    }
}

impl Handle {
    /// Control of the agent in `home`, its hold `hold` (as kept), telling `beat`.
    pub fn new(home: PathBuf, hold: Arc<Snap<HoldState>>, beat: Sender<BeatMsg>) -> Handle {
        let (wake, woken) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let c = Control {
            home: home.clone(),
            hold: hold.clone(),
            quit: Snap::new(None),
            settings: Snap::new(Settings::default()),
            jobs: Snap::new(Vec::new()),
            beat,
            wake: wake.clone(),
            jobs_part: None,
            left: String::new(),
        };
        let (quit, settings, jobs) = (c.quit.clone(), c.settings.clone(), c.jobs.clone());
        Handle { hold, quit, settings, jobs, woken, wake, home, tx, inline: Some((c, rx)), sweep: SWEEP }
    }

    /// Where another part wakes the loop's wait.
    pub fn waker(&self) -> Sender<ControlMsg> {
        self.wake.clone()
    }

    /// The hold's setter for another thread.
    pub fn setter(&self) -> Setter {
        Setter(self.tx.clone())
    }

    /// From now (before `spawn`), cancels and members' hold asks are told to `jobs` (**jobs**'
    /// thread), not the loop.
    pub fn tell_jobs(&mut self, jobs: Sender<ControlMsg>) {
        if let Some((c, _)) = &mut self.inline {
            c.jobs_part = Some(jobs);
        }
    }

    /// No sweep (before `spawn`): what's taken up was the watcher's event's, however long it took.
    #[cfg(test)]
    pub fn never_sweep(&mut self) {
        self.sweep = Duration::from_secs(365 * 86_400);
    }

    /// Its thread, and the watcher's: from now each ask is taken up as it comes.
    pub fn spawn(&mut self) {
        let Some((mut c, rx)) = self.inline.take() else { return };
        c.look();
        let started = std::thread::Builder::new().name("control".into()).spawn(move || {
            while let Ok(m) = rx.recv() {
                match m {
                    In::Look => c.look(),
                    In::SetHold(st, answer) => {
                        answer.send(c.set_hold(st)).ok();
                    }
                    In::Stop => return,
                }
            }
        });
        if let Err(e) = started {
            eprintln!("agent: control's thread: {e}");
            return;
        }
        let (home, tx, sweep) = (self.home.clone(), self.tx.clone(), self.sweep);
        let watching = std::thread::Builder::new().name("control-watch".into()).spawn(move || watch(&home, &tx, sweep));
        if let Err(e) = watching {
            eprintln!("agent: control's watcher: {e}");
        }
    }

    /// One look, in line (before `spawn`; the thread's own after).
    pub fn look(&mut self) {
        if let Some((c, rx)) = &mut self.inline {
            while rx.try_recv().is_ok() {}
            c.look();
        }
    }

    /// This Mac's hold set (another Mac's setting, read on the NAS), unless a later one stands;
    /// whether it did.
    pub fn set_hold(&mut self, st: HoldState) -> bool {
        if let Some((c, _)) = &mut self.inline {
            return c.set_hold(st);
        }
        self.setter().set(st)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.tx.send(In::Stop).ok();
    }
}

/// The folder `home` (and its cancels' folder) watched: a look asked at each change, and every
/// `sweep` whatever; ends once control is gone.
fn watch(home: &Path, tx: &Sender<In>, sweep: Duration) {
    let cancels = home.join(control::CANCEL_REQUESTS);
    // (A folder removed and made again, or swapped by a rename, leaves the old descriptors on a
    // vnode that says nothing more: watched anew once either folder isn't the one watched, the old
    // queue and descriptors closed first, so they stay three. Meanwhile, and without a kqueue, the
    // sweep.)
    #[cfg(target_os = "macos")]
    let mut kq: Option<Kqueue> = None;
    while tx.send(In::Look).is_ok() {
        #[cfg(target_os = "macos")]
        {
            if kq.as_ref().is_none_or(|k| k.stale()) {
                kq = None;
                std::fs::create_dir_all(&cancels).ok();
                kq = Kqueue::on(&[home, &cancels]);
            }
            if let Some(k) = &kq {
                k.wait(sweep);
                continue;
            }
        }
        std::thread::sleep(sweep);
    }
}

/// The folder's identity (device, inode), to tell one made again.
#[cfg(target_os = "macos")]
fn ident(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
}

/// A kqueue watching folders for their entries' changes (a file made, renamed in, removed).
#[cfg(target_os = "macos")]
struct Kqueue {
    kq: i32,
    fds: Vec<i32>,
    /// The folders watched, and each's identity when it was opened.
    dirs: Vec<(PathBuf, Option<(u64, u64)>)>,
}

#[cfg(target_os = "macos")]
impl Kqueue {
    fn on(dirs: &[&Path]) -> Option<Kqueue> {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: kqueue takes no arguments; the descriptor is closed by Drop.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return None;
        }
        let mut k = Kqueue { kq, fds: Vec::new(), dirs: Vec::new() };
        for d in dirs {
            let Ok(c) = std::ffi::CString::new(d.as_os_str().as_bytes()) else { return None };
            // SAFETY: a valid C path; O_EVTONLY opens for events only.
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_EVTONLY) };
            if fd < 0 {
                return None;
            }
            k.fds.push(fd);
            k.dirs.push((d.to_path_buf(), ident(d)));
            let ev = libc::kevent {
                ident: fd as usize,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE
                    | libc::NOTE_EXTEND
                    | libc::NOTE_ATTRIB
                    | libc::NOTE_RENAME
                    | libc::NOTE_DELETE,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: one change of our own, no events asked back.
            let r = unsafe { libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
            if r < 0 {
                return None;
            }
        }
        Some(k)
    }

    /// Whether a watched folder is gone or another now has its name.
    fn stale(&self) -> bool {
        self.dirs.iter().any(|(p, id)| id.is_none() || ident(p) != *id)
    }

    /// Until a watched folder changes, or `most` passed.
    fn wait(&self, most: Duration) {
        let ts =
            libc::timespec { tv_sec: most.as_secs() as libc::time_t, tv_nsec: most.subsec_nanos() as libc::c_long };
        // SAFETY: room for the events asked for, in a buffer we own.
        let mut out: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
        unsafe { libc::kevent(self.kq, std::ptr::null(), 0, out.as_mut_ptr(), out.len() as i32, &ts) };
    }
}

#[cfg(target_os = "macos")]
impl Drop for Kqueue {
    fn drop(&mut self) {
        // SAFETY: descriptors this owns.
        unsafe {
            for fd in &self.fds {
                libc::close(*fd);
            }
            libc::close(self.kq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Status;
    use super::*;
    use crate::control::{Hold, HoldStage};

    fn job(id: &str, slot: usize) -> super::super::JobView {
        super::super::JobView {
            id: id.into(),
            what: format!("{id}'s work"),
            started: 1_700_000_000 - 600,
            paused: None,
            pausing: None,
            tail: String::new(),
            parts: Vec::new(),
            part: None,
            progress: None,
            mem_mb: None,
            threads: None,
            waiting: None,
            slot: Some(slot),
            live: None,
        }
    }

    /// The status the menu bar and the build page print (task #846): a Mac paused alone with two
    /// jobs finishing, its verdict, the out-of-touch threshold, a stage's counts and its steps'.
    /// Pinned on both sides by tools/status/test/status-said.json (the Swift selfTest reads it;
    /// `SCENIC_BLESS=1` writes it anew).
    pub fn said_status() -> Status {
        use crate::agent::build::{Item, Line};
        let mut st = Status { host: "m1".into(), app: "2026-10-30".into(), beat: 1_700_000_000, ..Default::default() };
        st.set_jobs(vec![job("unit 6/1/2", 0), job("pack 6/1/1", 1), job("unit 6/1/3", 2)], true);
        st.checklist = vec![Line {
            id: "roads".into(),
            what: "roads".into(),
            steps: vec!["unit".into(), "pack".into()],
            done: 1234,
            total: Some(2000),
            left: Some(766),
            unit: "z6 tiles".into(),
            items: vec![
                Item {
                    step: "unit".into(),
                    noun: "z6 tile".into(),
                    done: 1234,
                    total: Some(2000),
                    left: Some(766),
                    empty: 12,
                    ..Default::default()
                },
                Item { step: "outlines".into(), noun: "run".into(), left: Some(0), ..Default::default() },
            ],
            ..Default::default()
        }
        .said()];
        let held =
            HoldState { held: Some(Hold { by: "the menu bar on m1".into(), at: 1_700_000_000 }), at: 1_700_000_000 };
        st.say(None, &held, false);
        // (Its loop stuck past the stuck threshold, as the beat says it: task #899.)
        st.loop_age_s = Some(crate::agent::beat::LOOP_STUCK.as_secs() + 100);
        st.loop_stuck_s = Some(crate::agent::beat::LOOP_STUCK.as_secs());
        st.loop_phase = Some("the pool's round".into());
        st
    }

    #[test]
    fn the_menu_bars_status_is_scenics() {
        let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/status/test/status-said.json");
        let ours = serde_json::json!({ "status": said_status(), "local": true, "now": 1_700_000_030 });
        if std::env::var_os("SCENIC_BLESS").is_some() {
            std::fs::write(&f, serde_json::to_vec_pretty(&ours).unwrap()).unwrap();
        }
        let menu: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        assert_eq!(ours, menu, "tools/status/test/status-said.json: SCENIC_BLESS=1 writes it anew");
        let s = &ours["status"];
        assert_eq!(s["verdict"]["state"], "pausing");
        assert_eq!(s["loop_stuck_s"], crate::agent::beat::LOOP_STUCK.as_secs());
        assert_eq!(s["out_of_touch_s"], crate::pool::beat::OUT_OF_TOUCH_S);
        assert_eq!(s["checklist"][0]["counts"][0], "1,234 of 2,000 z6 tiles (12 with nothing to publish)");
        assert_eq!(s["checklist"][0]["items"][1]["count"], "done");
        assert_eq!(s["checklist"][0]["items"][1]["finished"], true);
    }

    /// The watcher's kqueue knows a folder made again (removed and made), so the watcher opens it
    /// anew rather than watch a vnode that says nothing more.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_sees_a_folder_made_again() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let k = Kqueue::on(&[&a, &b]).unwrap();
        assert!(!k.stale());
        std::fs::remove_dir(&b).unwrap();
        assert!(k.stale(), "gone");
        std::fs::create_dir_all(&b).unwrap();
        assert!(k.stale(), "made again");
        let k = Kqueue::on(&[&a, &b]).unwrap();
        assert!(!k.stale() && k.fds.len() == 2);
    }

    /// Task #846, #871 step 3: this Mac's own pause ask taken up at once, by control's watcher on
    /// its thread: kept, answered, the running jobs told to drain, and the beat told, which says
    /// "pausing" with them; another member's ask left for the loop (woken for it); a resume says
    /// "resuming".
    #[test]
    fn a_hold_ask_is_taken_up_at_once() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        let hold = Snap::new(HoldState::default());
        let mut beat = super::super::beat::Handle::new(home.clone(), hold.clone());
        let mut c = Handle::new(home.clone(), hold, beat.sender());
        let chan = home.join("control");
        std::fs::write(&chan, b"run").unwrap();
        c.jobs.set(vec![super::super::slots::JobRef::on_chan(chan.clone())]);
        let mut st = Status { host: "m1".into(), beat: 1, ..Default::default() };
        st.set_jobs(vec![job("unit 6/1/2", 0)], true);
        let local = home.join("status.json");
        beat.publish(super::super::beat::LoopView {
            status: st,
            local: local.clone(),
            shared: None,
            lead: None,
            hold_at: 0,
            at: std::time::Instant::now(),
        });
        // (No sweep, no beat on its timer: whatever is taken up, said and written was the watcher's
        // event and control's telling, however long a loaded Mac took: #910.)
        beat.never_tick();
        beat.spawn();
        c.never_sweep();
        c.spawn();
        let said = || -> Status { serde_json::from_slice(&std::fs::read(&local).unwrap()).unwrap() };
        let within = |what: &str, f: &dyn Fn() -> bool| store::testkit::eventually(what, f);
        // Another member's: the loop's, woken for it.
        while c.woken.try_recv().is_ok() {}
        control::request_hold(&home, Some("m4"), Some(Hold::new("scenic pause on m1"))).unwrap();
        within("the loop woken", &|| c.woken.try_recv().is_ok_and(|m| m == ControlMsg::Ask));
        assert!(home.join(control::HOLD_REQUEST).exists());
        let r = control::request_hold(&home, None, Some(Hold::new("the menu bar on m1"))).unwrap();
        within("answered", &|| control::hold_answer(&home, &r).is_some_and(|a| a.ok));
        within("the job told", &|| std::fs::read(&chan).unwrap() == b"drain");
        within("the beat says pausing", &|| said().verdict.is_some_and(|v| v.state == "pausing"));
        assert!(!home.join(control::HOLD_REQUEST).exists());
        assert!(control::read_hold(&home).held.is_some() && c.hold.get().held.is_some());
        let h = said().hold.unwrap();
        assert_eq!((h.stage, h.finishing), (HoldStage::Pausing, vec!["unit 6/1/2".to_string()]));
        assert!(said().held.is_some() && said().beat > 1);
        // An earlier setting from the NAS loses; a later one stands.
        let at = c.hold.get().at;
        assert!(!c.set_hold(HoldState { held: None, at: at - 1 }) && c.hold.get().held.is_some());
        // Going on: resuming, until the loop's pass.
        control::request_hold(&home, None, None).unwrap();
        within("the beat says resuming", &|| {
            said().hold.is_some_and(|h| h.stage == HoldStage::Resuming) && said().held.is_none()
        });
    }

    /// Task #871 step 3: the app's quit and the owner's settings, taken up the moment their files
    /// change, the loop woken.
    #[test]
    fn a_quit_and_the_settings_are_taken_up_at_once() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        let hold = Snap::new(HoldState::default());
        let (beat, _beats) = mpsc::channel();
        let mut c = Handle::new(home.clone(), hold, beat);
        // (No sweep: each wake was the watcher's event's, however long it took: #910.)
        c.never_sweep();
        c.spawn();
        let wait = |want: ControlMsg| {
            store::testkit::wait_for(&format!("{want:?}"), || c.woken.try_recv().ok().filter(|m| *m == want))
        };
        control::request_quit(&home, control::QuitMode::Finish, "a test").unwrap();
        wait(ControlMsg::Quit);
        assert!(c.quit.get().as_ref().as_ref().is_some_and(|q| q.by == "a test"));
        room::set_target(&home, Some(50 << 30), "a test").unwrap();
        wait(ControlMsg::Settings);
        assert_eq!(c.settings.get().room.as_ref().map(|t| t.bytes), Some(50 << 30));
        control::clear_quit(&home);
        wait(ControlMsg::Quit);
        assert!(c.quit.get().is_none());
    }
}
