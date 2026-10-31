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
//!
//! A member's beat goes to the lead too (`ToLead`, docs/coordination.md step 3: `POST /pool/beat`
//! with the leases it holds, its pool heartbeat and its status), by a poster thread of its own
//! that takes the newest each time it's free: a hung share holds neither it nor the lead's view of
//! this Mac. The lead's answer goes to the pool's registry (crate::coord::beats::Beats::answered).

use super::bus::{BeatMsg, Snap};
use super::{nasread, now_s, write_beat, write_replace, Status};
use crate::control::HoldState;
use crate::coord::beats::Beats;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use store::nasfs::NasPath;

/// The heartbeat's period (docs/agent-parts.md, decision 1).
pub const EVERY: Duration = Duration::from_secs(10);
/// How long after a change the beat waits for more before it writes.
pub const DEBOUNCE: Duration = Duration::from_millis(200);
/// A round not finished, or a plan running, this long is stuck: the status says it
/// (`Status::loop_stuck_s`, task #899), and the plan's own log and sample go by it
/// (crate::agent::plan::Handle::stalled).
pub const LOOP_STUCK: Duration = Duration::from_secs(300);

/// A plan running, as the beat reads it (crate::agent::plan::Handle::running): since when (s since
/// the epoch) and its phase.
pub type PlanRunning = Arc<dyn Fn() -> Option<(u64, &'static str)> + Send + Sync>;

/// What the loop last composed: its status, where it goes, the hold it said, and when.
#[derive(Clone)]
pub struct LoopView {
    pub status: Status,
    /// This Mac's copy (status.json, a helper's helper.json).
    pub local: PathBuf,
    /// The NAS's, when the NAS is up and this agent writes it (none in a dry run, or past a part
    /// change until the loop composes again).
    pub shared: Option<NasPath>,
    /// A member's beat to the lead (None leading, or the lead's contact not known yet).
    pub lead: Option<ToLead>,
    /// The hold's `at` as the loop said it: a later going on is "resuming" until the loop sees it.
    pub hold_at: u64,
    pub at: Instant,
}

/// What a member's beat sends the lead's coordinator, and where its answer goes.
#[derive(Clone)]
pub struct ToLead {
    /// The lead's coordinator's addresses and key (crate::coord::client::Client's contact).
    pub urls: Vec<String>,
    pub token: String,
    /// This Mac's worker name (the leases' holder).
    pub member: String,
    /// The lead's leases it holds, as its loop listed them at `listed`.
    pub held: BTreeSet<u64>,
    pub listed: Instant,
    /// Its pool heartbeat (`Beats::own`) and where the answer goes.
    pub beats: Arc<Beats>,
}

impl ToLead {
    /// The beat as it goes now, with `status` (its plan's parts left out: the lead's page reads
    /// the lead's), its heartbeat stamped now.
    fn body(&self, status: &Status) -> crate::coord::held::Beat {
        use crate::coord::{beats, held};
        let status = Status {
            regions: Vec::new(),
            checklist: Vec::new(),
            built: Default::default(),
            recent: Vec::new(),
            ..status.clone()
        };
        let heartbeat = self.beats.own().map(|mut h| {
            h.pool.beat = status.beat;
            h
        });
        held::Beat {
            member: self.member.clone(),
            speaks: vec![held::SPEAKS.to_string(), beats::SPEAKS.to_string()],
            held: self.held.clone(),
            held_age_s: 0,
            heartbeat,
            status: serde_json::to_value(&status).ok(),
        }
    }

    /// Posts `b` to the lead; its answer to the registry (an older lead's 404: the NAS copies go
    /// on). Whether it failed.
    fn post(&self, b: &crate::coord::held::Beat, failing: bool) -> bool {
        // (Its list's age as it goes: the loop's, stuck maybe, may lack a lease granted since,
        // which the lead then keeps. A lead that doesn't say it takes the beat may not read the
        // age: a list older than its grace isn't sent it, the lease held as away meanwhile.)
        let age = self.listed.elapsed();
        if !self.beats.lead_takes() && age >= crate::coord::held::OMIT_GRACE / 2 {
            return failing;
        }
        let b = crate::coord::held::Beat { held_age_s: age.as_secs_f64().ceil() as u64, ..b.clone() };
        let c = crate::coord::client::Client::at(self.urls.clone(), self.token.clone(), &self.member);
        match c.pool_beat(&b) {
            Ok(Some(a)) => {
                self.beats.answered(&a, now_s());
                false
            }
            Ok(None) => {
                self.beats.lead_older();
                false
            }
            Err(e) => {
                if !failing {
                    eprintln!("agent: its beat to the lead: {e:#}");
                }
                true
            }
        }
    }
}

type Mailbox = Arc<Snap<Option<(NasPath, Vec<u8>)>>>;
type LeadBox = Arc<Snap<Option<(ToLead, crate::coord::held::Beat)>>>;

/// How the shared copy is written (`write_beat`; a test's slow NAS).
pub type NasWrite = Arc<dyn Fn(&NasPath, Vec<u8>) -> anyhow::Result<()> + Send + Sync>;

/// The beat's own state.
pub struct Beat {
    home: PathBuf,
    view: Arc<Snap<Option<LoopView>>>,
    hold: Arc<Snap<HoldState>>,
    /// The shared copy's writer, once the beat runs on its thread (else written in line).
    nas: Option<(Mailbox, SyncSender<()>)>,
    /// How it writes the shared copy.
    write_nas: NasWrite,
    /// The lead's poster, once the beat runs on its thread (else posted in line).
    poster: Option<(LeadBox, SyncSender<()>)>,
    failing: bool,
    lead_failing: bool,
    /// A loop older than this is stuck (`LOOP_STUCK`; a test's shorter).
    stuck_after: Duration,
    /// The lead's plan, when one runs here: a plan running long is a stuck loop too.
    plan: Option<PlanRunning>,
    /// The timer's period (`EVERY`; a test's never, so what it says was told it).
    every: Duration,
}

impl Beat {
    /// The status as the snapshots say now, stamped now; None before the loop's first.
    pub fn compose(&self) -> Option<(Status, PathBuf, Option<NasPath>, Option<ToLead>)> {
        let v = self.view.get();
        let v = v.as_ref().as_ref()?;
        let hold = self.hold.get();
        let mut status = v.status.clone();
        let asked = crate::control::peek_hold_request(&self.home);
        status.say(asked.as_ref(), &hold, hold.held.is_none() && hold.at > v.hold_at);
        status.beat = now_s();
        // (The older of the round's view and a plan running: either stuck is the loop stuck, in
        // that part's phase.)
        let round = (v.at.elapsed().as_secs(), super::nasread::phase_now().map(str::to_string));
        let plan = self
            .plan
            .as_ref()
            .and_then(|f| f())
            .map(|(since, p)| (status.beat.saturating_sub(since), Some(format!("the plan: {p}"))));
        let (age, phase) = plan.filter(|p| p.0 > round.0).unwrap_or(round);
        let stuck = self.stuck_after.as_secs();
        status.loop_age_s = Some(age);
        status.loop_stuck_s = Some(stuck);
        status.loop_phase = phase.filter(|_| age > stuck);
        Some((status, v.local.clone(), v.shared.clone(), v.lead.clone()))
    }

    /// One beat written: here, to the lead (on its poster, or in line), and to the NAS (on its
    /// writer, or in line).
    pub fn write(&mut self) {
        let Some((status, local, shared, lead)) = self.compose() else { return };
        let Ok(body) = serde_json::to_vec_pretty(&status) else { return };
        if let Err(e) = write_replace(&local, &body) {
            eprintln!("agent: heartbeat: {}: {e:#}", local.display());
        }
        if let Some(to) = lead {
            let b = to.body(&status);
            match &self.poster {
                Some((post, wake)) => {
                    post.set(Some((to, b)));
                    wake.try_send(()).ok();
                }
                None => self.lead_failing = to.post(&b, self.lead_failing),
            }
        }
        let Some(p) = shared else { return };
        match &self.nas {
            Some((mail, wake)) => {
                mail.set(Some((p, body)));
                wake.try_send(()).ok();
            }
            None => {
                let r = (self.write_nas)(&p, body);
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
        let beat = Beat {
            home,
            view: view.clone(),
            hold,
            nas: None,
            write_nas: Arc::new(write_beat),
            poster: None,
            failing: false,
            lead_failing: false,
            stuck_after: LOOP_STUCK,
            plan: None,
            every: EVERY,
        };
        Handle { view, tx, inline: Some((beat, rx)) }
    }

    /// The lead's plan read with each beat (before `spawn`): a plan running past the threshold is
    /// said as a stuck loop, in its phase.
    pub fn watch_plan(&mut self, f: PlanRunning) {
        if let Some((b, _)) = self.inline.as_mut() {
            b.plan = Some(f);
        }
    }

    /// No timer (before `spawn`): it writes only when told a state changed.
    #[cfg(test)]
    pub fn never_tick(&mut self) {
        if let Some((b, _)) = self.inline.as_mut() {
            b.every = Duration::from_secs(365 * 86_400);
        }
    }

    /// The shared copy written by `w` (before `spawn`): a test's slow NAS.
    #[cfg(test)]
    pub fn write_nas_by(&mut self, w: NasWrite) {
        if let Some((b, _)) = self.inline.as_mut() {
            b.write_nas = w;
        }
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
        let (m, view, write_nas) = (mail.clone(), beat.view.clone(), beat.write_nas.clone());
        let nas = std::thread::Builder::new().name("beat-nas".into()).spawn(move || {
            let mut failing = false;
            while woken.recv().is_ok() {
                // (The newest body each time: one written late is never older than one before. One
                // for a path the view no longer shares, past a part change, is dropped.)
                let Some((p, body)) = m.get().as_ref().clone() else { continue };
                if view.get().as_ref().as_ref().and_then(|v| v.shared.as_ref()) != Some(&p) {
                    continue;
                }
                failing = said(write_nas(&p, body), failing);
            }
        });
        if nas.is_ok() {
            beat.nas = Some((mail, wake));
        }
        let post: LeadBox = Snap::new(None);
        let (wake, woken) = mpsc::sync_channel::<()>(1);
        let p = post.clone();
        let poster = std::thread::Builder::new().name("beat-lead".into()).spawn(move || {
            let mut failing = false;
            while woken.recv().is_ok() {
                // (The newest each time: a beat posted late is never older than one before.)
                let Some((to, b)) = p.get().as_ref().clone() else { continue };
                failing = to.post(&b, failing);
            }
        });
        if poster.is_ok() {
            beat.poster = Some((post, wake));
        }
        let spawned = std::thread::Builder::new().name("beat".into()).spawn(move || {
            let every = beat.every;
            let mut next = Instant::now() + every;
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
                    next += every;
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
            lead: None,
            hold_at: 0,
            at: Instant::now(),
        }
    }

    fn said(p: &std::path::Path) -> Status {
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }

    /// docs/coordination.md §8, step 3: the slow-NAS fake blocks every write of the shared copy
    /// until released; two members' beats reach the lead all the while, each of 60 (ten minutes'
    /// at `EVERY`; the timer off, each beat a change's, so none is the timer's doing), and each
    /// member hears the other from the lead's answers.
    #[test]
    fn beats_go_on_while_the_nas_hangs() {
        use crate::agent::pool::Heartbeat;
        // (The slow NAS: every write waits until the share answers.)
        let answers = store::testkit::Signal::new();
        let a = answers.clone();
        let hung: NasWrite = Arc::new(move |_, _| {
            a.wait("the share answers");
            Ok(())
        });
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("lead"), "lead", "");
        let at_lead = Arc::new(Beats::default());
        c.set_beats(Some(at_lead.clone()));
        let mut members: Vec<(Handle, Arc<Beats>, String)> = ["m-000000000000000a", "m-000000000000000b"]
            .into_iter()
            .map(|id| {
                let mut h = Handle::new(d.path().join(id), Snap::new(HoldState::default()));
                h.never_tick();
                h.write_nas_by(hung.clone());
                h.spawn();
                let beats = Arc::new(Beats::default());
                beats.set_own(Heartbeat {
                    pool: crate::pool::Beat { member: id.into(), host: id.into(), ..Default::default() },
                    beat_http: true,
                    ..Default::default()
                });
                (h, beats, id.to_string())
            })
            .collect();
        for i in 1..=60 {
            for (h, beats, id) in &mut members {
                std::fs::create_dir_all(d.path().join(&*id)).unwrap();
                h.publish(LoopView {
                    status: Status { host: id.clone(), beat: 1, ..Default::default() },
                    local: d.path().join(&*id).join("status.json"),
                    shared: Some(NasPath::local_for_test(&d.path().join("nas").join(&*id))),
                    lead: Some(ToLead {
                        urls: vec![format!("http://127.0.0.1:{port}")],
                        token: c.contact.token.clone(),
                        member: id.clone(),
                        held: Default::default(),
                        listed: Instant::now(),
                        beats: beats.clone(),
                    }),
                    hold_at: 0,
                    at: Instant::now(),
                });
            }
            for (_, _, id) in &members {
                store::testkit::eventually("the lead hears each beat", || {
                    at_lead.fresh(now_s()).iter().any(|x| x.beat.pool.member == *id && x.n >= i)
                });
            }
        }
        assert!(!answers.is_set(), "the NAS hung throughout");
        let (a, b) = (&members[0], &members[1]);
        assert!(a.1.get(&b.2, now_s()).is_some() && b.1.get(&a.2, now_s()).is_some());
        assert!(at_lead
            .fresh(now_s())
            .iter()
            .all(|x| x.status.as_ref().is_some_and(|s| s["host"] == x.beat.pool.member)));
        answers.set();
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

    /// A loop past the stuck threshold: the beat says the threshold, and the phase it's in.
    #[test]
    fn a_beat_says_a_stuck_loops_phase() {
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("status.json");
        let mut b = Handle::new(d.path().into(), Snap::new(HoldState::default()));
        b.publish(view(local.clone(), None));
        let s = said(&local);
        assert_eq!((s.loop_stuck_s, s.loop_phase), (Some(LOOP_STUCK.as_secs()), None));
        // (A threshold of 0 s, and a view a second old: past it.)
        b.inline.as_mut().unwrap().0.stuck_after = Duration::ZERO;
        crate::agent::nasread::phase("a test's phase");
        std::thread::sleep(Duration::from_millis(1100)); // (sleep ok: the age is whole seconds of the view's own Instant, nothing to wait on)
        b.changed();
        let s = said(&local);
        assert!(s.loop_age_s.unwrap() > 0 && s.loop_stuck_s == Some(0) && s.loop_phase.is_some(), "{s:?}");
    }

    /// A plan running long, the round going on: the loop stuck, in the plan's phase.
    #[test]
    fn a_beat_says_a_stuck_plans_phase() {
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("status.json");
        let mut b = Handle::new(d.path().into(), Snap::new(HoldState::default()));
        let since = now_s().saturating_sub(LOOP_STUCK.as_secs() + 60);
        b.watch_plan(Arc::new(move || Some((since, "the regions' counts"))));
        b.publish(view(local.clone(), None));
        let s = said(&local);
        assert!(s.loop_age_s.unwrap() > LOOP_STUCK.as_secs(), "{s:?}");
        assert_eq!(s.loop_phase.as_deref(), Some("the plan: the regions' counts"));
        // (No plan running: the round's fresh view, nothing stuck.)
        b.watch_plan(Arc::new(|| None));
        b.changed();
        let s = said(&local);
        assert_eq!((s.loop_age_s, s.loop_phase), (Some(0), None));
    }
}
