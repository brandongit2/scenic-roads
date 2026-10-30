//! **beat** (docs/agent-parts.md §4.2, task #871 step 2): the heartbeat and status, written on a
//! steady timer of their own, whatever the loop waits on, and only here.
//!
//! The loop publishes what it last saw (`LoopView`: its status, the hold's say aside, and where it
//! goes) as a snapshot; this Mac's hold is **control**'s (crate::agent::asks). The beat composes
//! the two and writes them: every `EVERY`, and at once (`DEBOUNCE`) when told a state changed
//! (`BeatMsg::Changed`). The local copy is written here, on this Mac's disk; the shared one (the
//! NAS) by a writer thread of its own that takes the newest body each time it's free, under
//! nasread's deadline, so a hung share holds neither the timer nor the local copy; a body for a
//! path the view no longer shares (past a part change) is dropped unwritten.
//!
//! One writer of each copy, in order, each a whole document from snapshots at least as new as the
//! last: no beat lands over a newer one. The
//! status says how old the loop's part of it is (`Status::loop_age_s`): the heartbeat's time says
//! the agent is alive, that age whether its loop is slow.

use super::bus::{BeatMsg, Snap};
use super::{nasread, now_s, write_beat, write_replace, Status};
use crate::control::HoldState;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use store::nasfs::NasPath;

/// The heartbeat's period (docs/agent-parts.md, decision 1).
pub const EVERY: Duration = Duration::from_secs(10);
/// How long after a change the beat waits for more before it writes.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// What the loop last composed: its status, where it goes, the hold it said, and when.
#[derive(Clone)]
pub struct LoopView {
    pub status: Status,
    /// This Mac's copy (status.json, a helper's helper.json).
    pub local: PathBuf,
    /// The NAS's, when the NAS is up and this agent writes it (none in a dry run, or past a part
    /// change until the loop composes again).
    pub shared: Option<NasPath>,
    /// The hold's `at` as the loop said it: a later going on is "resuming" until the loop sees it.
    pub hold_at: u64,
    pub at: Instant,
}

type Mailbox = Arc<Snap<Option<(NasPath, Vec<u8>)>>>;

/// The beat's own state.
pub struct Beat {
    home: PathBuf,
    view: Arc<Snap<Option<LoopView>>>,
    hold: Arc<Snap<HoldState>>,
    /// The shared copy's writer, once the beat runs on its thread (else written in line).
    nas: Option<(Mailbox, SyncSender<()>)>,
    failing: bool,
}

impl Beat {
    /// The status as the snapshots say now, stamped now; None before the loop's first.
    pub fn compose(&self) -> Option<(Status, PathBuf, Option<NasPath>)> {
        let v = self.view.get();
        let v = v.as_ref().as_ref()?;
        let hold = self.hold.get();
        let mut status = v.status.clone();
        let asked = crate::control::peek_hold_request(&self.home);
        status.say(asked.as_ref(), &hold, hold.held.is_none() && hold.at > v.hold_at);
        status.beat = now_s();
        status.loop_age_s = Some(v.at.elapsed().as_secs());
        Some((status, v.local.clone(), v.shared.clone()))
    }

    /// One beat written: here, and to the NAS (on its writer, or in line).
    pub fn write(&mut self) {
        let Some((status, local, shared)) = self.compose() else { return };
        let Ok(body) = serde_json::to_vec_pretty(&status) else { return };
        if let Err(e) = write_replace(&local, &body) {
            eprintln!("agent: heartbeat: {}: {e:#}", local.display());
        }
        let Some(p) = shared else { return };
        match &self.nas {
            Some((mail, wake)) => {
                mail.set(Some((p, body)));
                wake.try_send(()).ok();
            }
            None => {
                let r = write_beat(&p, body);
                self.failing = said(r, self.failing);
            }
        }
    }
}

/// A shared write's outcome logged once a streak (a timeout already by nasread); whether it failed.
fn said(r: anyhow::Result<()>, failing: bool) -> bool {
    match r {
        Ok(()) => false,
        Err(e) => {
            if !failing && !nasread::timed_out(&e) {
                eprintln!("agent: heartbeat: {e:#}");
            }
            true
        }
    }
}

/// The loop's handle on the beat: in line until `spawn`, then on its thread.
pub struct Handle {
    pub view: Arc<Snap<Option<LoopView>>>,
    tx: Sender<BeatMsg>,
    inline: Option<(Beat, Receiver<BeatMsg>)>,
}

impl Handle {
    /// The beat of the agent in `home`, saying `hold`.
    pub fn new(home: PathBuf, hold: Arc<Snap<HoldState>>) -> Handle {
        let view = Snap::new(None);
        let (tx, rx) = mpsc::channel();
        let beat = Beat { home, view: view.clone(), hold, nas: None, failing: false };
        Handle { view, tx, inline: Some((beat, rx)) }
    }

    /// Where to tell it a state changed (`BeatMsg::Changed`).
    pub fn sender(&self) -> Sender<BeatMsg> {
        self.tx.clone()
    }

    /// Whether it runs on its thread.
    pub fn on(&self) -> bool {
        self.inline.is_none()
    }

    /// Its thread and its NAS writer's started: from now it beats every `EVERY`, whatever the loop
    /// does, and at once on a change. It ends when the handle goes, writing its last.
    pub fn spawn(&mut self) {
        let Some((mut beat, rx)) = self.inline.take() else { return };
        let mail: Mailbox = Snap::new(None);
        let (wake, woken) = mpsc::sync_channel::<()>(1);
        let (m, view) = (mail.clone(), beat.view.clone());
        let nas = std::thread::Builder::new().name("beat-nas".into()).spawn(move || {
            let mut failing = false;
            while woken.recv().is_ok() {
                // (The newest body each time: one written late is never older than one before. One
                // for a path the view no longer shares, past a part change, is dropped.)
                let Some((p, body)) = m.get().as_ref().clone() else { continue };
                if view.get().as_ref().as_ref().and_then(|v| v.shared.as_ref()) != Some(&p) {
                    continue;
                }
                failing = said(write_beat(&p, body), failing);
            }
        });
        if nas.is_ok() {
            beat.nas = Some((mail, wake));
        }
        let spawned = std::thread::Builder::new().name("beat".into()).spawn(move || {
            let mut next = Instant::now() + EVERY;
            loop {
                match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(BeatMsg::Changed) => {
                        std::thread::sleep(DEBOUNCE);
                        while rx.try_recv().is_ok() {}
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(BeatMsg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                        beat.write();
                        return;
                    }
                }
                beat.write();
                // (Steady: a change's write doesn't move the timer.)
                while next <= Instant::now() {
                    next += EVERY;
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("agent: the heartbeat's thread: {e}");
        }
    }

    /// The loop's newest view: said at once.
    pub fn publish(&mut self, v: LoopView) {
        self.view.set(Some(v));
        self.changed();
    }

    /// A state it says changed: written at once (in line, before the thread runs).
    pub fn changed(&mut self) {
        match &mut self.inline {
            Some((beat, rx)) => {
                while rx.try_recv().is_ok() {}
                beat.write();
            }
            None => {
                self.tx.send(BeatMsg::Changed).ok();
            }
        }
    }

    /// This Mac's part in the pool changed (docs/pool.md §7.6): no shared beat under the old part
    /// from now, until the loop composes one under the new.
    pub fn part_changed(&mut self) {
        let Some(mut v) = self.view.get().as_ref().clone() else { return };
        v.shared = None;
        self.view.set(Some(v));
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.tx.send(BeatMsg::Stop).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Hold;

    fn view(local: PathBuf, shared: Option<NasPath>) -> LoopView {
        LoopView {
            status: Status { host: "m1".into(), beat: 1, ..Default::default() },
            local,
            shared,
            hold_at: 0,
            at: Instant::now(),
        }
    }

    fn said(p: &std::path::Path) -> Status {
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }

    /// In line: each publish written here and on the NAS, saying the hold as control has it, and
    /// "resuming" for a going on the loop hasn't said yet; past a part change, the NAS's no more.
    #[test]
    fn a_beat_says_the_newest_hold_and_stops_sharing_on_a_part_change() {
        let d = tempfile::tempdir().unwrap();
        let hold = Snap::new(HoldState::default());
        let mut b = Handle::new(d.path().into(), hold.clone());
        let (local, shared) = (d.path().join("status.json"), d.path().join("nas/state/status.json"));
        b.publish(view(local.clone(), Some(NasPath::local_for_test(&shared))));
        assert!(said(&local).beat > 1 && said(&shared).beat > 1 && said(&local).loop_age_s == Some(0));
        hold.set(HoldState { held: Some(Hold::new("a test")), at: 5 });
        b.changed();
        assert!(said(&local).held.is_some() && said(&shared).held.is_some());
        hold.set(HoldState { held: None, at: 6 });
        b.changed();
        assert_eq!(said(&local).hold.map(|h| h.stage), Some(crate::control::HoldStage::Resuming));
        b.part_changed();
        std::fs::remove_file(&shared).unwrap();
        b.changed();
        assert!(local.exists() && !shared.exists());
    }
}
