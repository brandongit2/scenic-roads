//! **nas**, the agent's part that owns the NAS's mount (docs/agent-parts.md §4.3, task #871 step
//! 4): finding the share's project folder, mounting it (every `MOUNT_EVERY` at most), probing it,
//! the LAN remount when home again and the self-heal's remount (crate::agent::heal), each look on a
//! worker of its own under `DEADLINE`. Past it the worker is left to the kernel (the one look
//! running for this part, no second started while it hangs) and the view says "mounting (stuck
//! since …)". The loop and the other parts read the view (`NasView`) and never wait on a mount:
//! only work that needs the NAS waits (`Needs { nas: true }`, the plan, the pool).
//!
//! On its own thread once `Handle::spawn` ran (a looping agent's), looking every `EVERY` and at
//! each ask; in line before (a single loop's, a dry run's, the tests'), each `Handle::root` one
//! look, still under the deadline.

use super::bus::Snap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use store::nasfs::NasRoot;

/// How long a look (find, mount, probe) may take before the view says it's stuck.
pub const DEADLINE: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(30) };
/// A look this often, whatever is asked.
pub const EVERY: Duration = Duration::from_secs(20);
/// A mount tried at most this often while the share isn't mounted.
const MOUNT_EVERY: Duration = Duration::from_secs(300);
/// A drained remount waits this long at most for the pool's lock (its step under way).
const LOCK_WAIT: Duration = Duration::from_secs(60);

/// The share's project folder as found (mounted first when asked): the real one, or a test's.
pub type Finder = Arc<dyn Fn(bool) -> Option<NasRoot> + Send + Sync>;

/// The NAS as this part last saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NasState {
    /// Mounted and answering.
    Up,
    /// Not mounted, or not answering.
    Away,
    /// A look (a mount, a probe) still under way past `DEADLINE`, since (Unix s).
    Stuck { since: u64 },
}

/// What **nas** publishes: the root to use (None while away or stuck), its state, and when it was
/// last looked at (Unix s).
#[derive(Clone, Debug)]
pub struct NasView {
    pub root: Option<NasRoot>,
    pub state: NasState,
    pub at: u64,
}

impl NasView {
    /// Why NAS work waits, in words, while a look is stuck.
    pub fn stuck_why(&self) -> Option<String> {
        match self.state {
            NasState::Stuck { since } => Some(format!(
                "the NAS is mounting (stuck since {}): only work on the NAS waits",
                super::nasread::clock(since)
            )),
            _ => None,
        }
    }
}

/// What the part is asked.
pub enum Req {
    /// A look now (the loop's round).
    Look,
    /// Home with the share mounted by another name (Tailscale): unmounted and mounted again by its
    /// LAN name, the loop having found nothing running.
    Lan,
    /// The self-heal drained: the share unmounted under the pool's lock (`pool`, when this Mac is
    /// in it: no tick mid-write) and mounted again at once.
    Remount(Option<super::pool::Shared>),
    Stop,
}

/// The part's own state.
struct Nas {
    find: Finder,
    probe: fn(&NasRoot) -> bool,
    /// Whether it may mount (a root found as the share's: not a test's, nor one given).
    mounts: bool,
    last_mount_try: Option<Instant>,
    /// The look under way past its deadline: its answer, and since when (Unix s).
    pending: Option<(Receiver<Option<NasRoot>>, u64)>,
    view: Arc<Snap<NasView>>,
}

impl Nas {
    /// One look: a worker's find (mounted when due) and probe, waited for `DEADLINE`; a look still
    /// stuck from before waited for again rather than a second started.
    fn look(&mut self) {
        let rx = match self.pending.take() {
            Some(p) => p,
            None => {
                let mount = self.mounts && self.last_mount_try.is_none_or(|t| t.elapsed() > MOUNT_EVERY);
                let (tx, rx) = mpsc::channel();
                let (find, probe) = (self.find.clone(), self.probe);
                let started = std::thread::Builder::new().name("nas-look".into()).spawn(move || {
                    let r = find(false).or_else(|| if mount { find(true) } else { None });
                    tx.send(r.filter(|r| probe(r)).map(|r| r.no_retry())).ok();
                });
                if let Err(e) = started {
                    eprintln!("agent: the NAS's look: {e}");
                    return;
                }
                if mount {
                    // (Counted from the try whatever it found: a share found mounted needed none.)
                    self.last_mount_try = Some(Instant::now());
                }
                (rx, super::jobs::now_s())
            }
        };
        match rx.0.recv_timeout(DEADLINE) {
            Ok(root) => {
                if root.is_some() {
                    self.last_mount_try = None;
                }
                let state = if root.is_some() { NasState::Up } else { NasState::Away };
                self.publish(NasView { root, state, at: super::jobs::now_s() });
            }
            Err(RecvTimeoutError::Timeout) => {
                let since = rx.1;
                if self.view.get().state != (NasState::Stuck { since }) {
                    eprintln!("agent: the NAS's mount or probe hasn't answered in {DEADLINE:?}: NAS work waits");
                }
                self.publish(NasView { root: None, state: NasState::Stuck { since }, at: super::jobs::now_s() });
                self.pending = Some(rx);
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.publish(NasView { root: None, state: NasState::Away, at: super::jobs::now_s() })
            }
        }
    }

    fn publish(&self, v: NasView) {
        self.view.set(v);
    }

    /// Home again, the share mounted by another name: unmounted, then looked at again (mounted by
    /// its LAN name).
    fn lan(&mut self) {
        if !self.mounts {
            return;
        }
        let Some(m) =
            store::nas::find_mount(store::nas::HOST, store::nas::SHARE).filter(|m| !store::nas::by_lan_name(m))
        else {
            return;
        };
        eprintln!("agent: home again; remounting the NAS by its LAN name (it was mounted as {})", m.from);
        if let Err(e) = store::nas::unmount(&m.point, Duration::from_secs(30)) {
            eprintln!("agent: {e:#}");
        }
        self.last_mount_try = None;
        self.look();
    }

    /// The self-heal's remount: the share unmounted under the pool's lock, then mounted again.
    fn remount(&mut self, pool: Option<super::pool::Shared>) {
        let t = Instant::now();
        let guard = loop {
            match pool.as_ref().map(super::pool::try_lock) {
                Some(None) if t.elapsed() < LOCK_WAIT => std::thread::sleep(Duration::from_secs(1)),
                Some(None) => {
                    eprintln!("agent: the pool's step held its lock for {LOCK_WAIT:?}: the remount is left for the next drain");
                    return;
                }
                Some(g) => break g,
                None => break None,
            }
        };
        if self.mounts {
            if let Some(m) = store::nas::find_mount(store::nas::HOST, store::nas::SHARE) {
                eprintln!("agent: drained; remounting {} to end this Mac's SMB session", m.point.display());
                if let Err(e) = store::nas::unmount(&m.point, Duration::from_secs(30)) {
                    eprintln!("agent: {e:#}");
                }
            }
        }
        drop(guard);
        self.last_mount_try = None;
        self.look();
    }

    fn take(&mut self, r: Req) -> bool {
        match r {
            Req::Look => self.look(),
            Req::Lan => self.lan(),
            Req::Remount(p) => self.remount(p),
            Req::Stop => return false,
        }
        true
    }
}

/// The loop's handle on **nas**.
pub struct Handle {
    pub view: Arc<Snap<NasView>>,
    tx: Sender<Req>,
    inline: Option<(Nas, Receiver<Req>)>,
}

impl Handle {
    /// The part, finding the root with `find` (mounting it when `mounts`), each found root asked
    /// whether it answers with `probe`.
    pub fn new(find: Finder, mounts: bool, probe: fn(&NasRoot) -> bool) -> Handle {
        let view = Snap::new(NasView { root: None, state: NasState::Away, at: 0 });
        let (tx, rx) = mpsc::channel();
        let nas = Nas { find, probe, mounts, last_mount_try: None, pending: None, view: view.clone() };
        Handle { view, tx, inline: Some((nas, rx)) }
    }

    /// Its thread: from now it looks every `EVERY` and at each ask, and the loop only reads.
    pub fn spawn(&mut self) {
        let Some((mut nas, rx)) = self.inline.take() else { return };
        nas.look();
        let started = std::thread::Builder::new().name("nas".into()).spawn(move || loop {
            let r = match rx.recv_timeout(EVERY) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => Req::Look,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            // (Asks that came while it looked: one look for them all.)
            let mut more = vec![r];
            more.extend(rx.try_iter());
            let mut looked = false;
            for r in more {
                if matches!(r, Req::Look) {
                    if looked {
                        continue;
                    }
                    looked = true;
                }
                if !nas.take(r) {
                    return;
                }
            }
        });
        if let Err(e) = started {
            eprintln!("agent: the NAS's thread: {e}");
        }
    }

    /// Whether its thread runs.
    pub fn spawned(&self) -> bool {
        self.inline.is_none()
    }

    /// The root to use now: the view as the part last published it (looked at once, in line,
    /// before `spawn`); the part asked to look again.
    pub fn root(&mut self) -> Option<NasRoot> {
        self.ask(Req::Look);
        self.view.get().root.clone()
    }

    /// `r` asked of the part: taken up on its thread, or at once in line.
    pub fn ask(&mut self, r: Req) {
        match &mut self.inline {
            Some((nas, _)) => {
                nas.take(r);
            }
            None => {
                self.tx.send(r).ok();
            }
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.tx.send(Req::Stop).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A finder hung until `go` is dropped, then finding `root`.
    fn hung(root: &std::path::Path, go: Receiver<()>) -> Finder {
        let (go, root) = (std::sync::Mutex::new(go), root.to_path_buf());
        Arc::new(move |_| {
            go.lock().unwrap().recv().ok();
            Some(NasRoot::local_for_test(root.clone()))
        })
    }

    /// A hung look leaves the view "stuck" past its deadline (no root: the call returned while the
    /// finder still hangs), starts no second look while it hangs, and the root comes back once it
    /// answers (the worker's answer waited for, however long a loaded Mac takes to send it: #910).
    #[test]
    fn a_hung_look_says_stuck_and_comes_back() {
        let d = tempfile::tempdir().unwrap();
        let (release, go) = mpsc::channel::<()>();
        let mut h = Handle::new(hung(d.path(), go), false, |_| true);
        assert!(h.root().is_none());
        assert!(matches!(h.view.get().state, NasState::Stuck { .. }));
        assert!(h.view.get().stuck_why().unwrap().contains("mounting (stuck since"));
        assert!(h.root().is_none(), "still stuck: the same look waited for");
        drop(release);
        let root = store::testkit::wait_for("the root back", || h.root());
        assert_eq!(root.display().to_string(), d.path().display().to_string());
        assert_eq!(h.view.get().state, NasState::Up);
    }

    /// On its thread the loop's `root` never waits on a hung look.
    #[test]
    fn the_loop_never_waits_on_the_part() {
        let d = tempfile::tempdir().unwrap();
        let (release, go) = mpsc::channel::<()>();
        let mut h = Handle::new(hung(d.path(), go), false, |_| true);
        // (Its first look, in spawn, given up past its deadline.)
        h.spawn();
        let t = Instant::now();
        for _ in 0..100 {
            assert!(h.root().is_none());
        }
        // (In line, each read would wait out the hung look's deadline: #910, not a "fast" bound.)
        store::testkit::ended_before(t, DEADLINE * 100, "a hundred reads of the view");
        drop(release);
        store::testkit::wait_for("the root back", || h.root());
    }
}
