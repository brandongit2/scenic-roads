//! The coordinator's state of its term on the NAS (docs/pool.md §6.2, §7.5; task #641):
//! `state/coord/term/<E>/state.json`, written by a thread of its own, never the agent's loop.
//!
//! - Woken by every change the coordinator saves on its own disk (a grant, a finish, a lapse, a
//!   given-back lease, a measure: `Keep::changed`, from `Shared::save_leases` and `save_costs`), and
//!   each `SWEEP` for what changes unsaved (a worker's failures, the pause): the state is written when
//!   its bytes differ from the last written for that term.
//! - Batched: a change waits `BATCH` for the rest of its burst, and a write starts no sooner than
//!   `GAP` after the one before: at most one file each `GAP` whatever the grants' rate (the NAS takes
//!   ~20–55 small-file creates a second; the state is one ~3 MB file).
//! - Through the agent's NAS (`nasread::Deadlined` in the agent: a write past its deadline fails and
//!   is tried again a `GAP` on).
//! - Only while its agent leads: `Coordinator::keep_state` names the term, each lead's step; the
//!   thread ends with the coordinator (`stop`, or dropped).
//!
//! What it is for: a lead gone without handing over (crashed, asleep, off the NAS, or restarted: its
//! re-assertion loads the file over the leases on its disk) is taken up from the newest term's file. A lease granted since that file was written is unknown to the new lead:
//! its worker's next renewal is refused and its job stops, its targets granted again (lost work, never
//! corrupted records: the merge's lease order, §6.5). The grant isn't held for the write: a handover
//! hands the state over whole in its settling, granting stopped (`Run::settled`).

use super::Shared;
use crate::pool::nas::Nas;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// A change waits this long for the rest of its burst.
pub const BATCH: Duration = Duration::from_secs(2);
/// A write starts at least this long after the one before (one tried, landed or not).
pub const GAP: Duration = Duration::from_secs(10);
/// Looked at this often without a change saved (a failure, the pause).
pub const SWEEP: Duration = Duration::from_secs(60);

/// The thread's pace (`BATCH`, `GAP`, `SWEEP`; a test's shorter).
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    pub batch: Duration,
    pub gap: Duration,
    pub sweep: Duration,
}

pub const PACE: Pace = Pace { batch: BATCH, gap: GAP, sweep: SWEEP };

/// What the coordinator and its writer share, under a lock of its own (taken inside the
/// coordinator's, never around it).
#[derive(Debug, Default)]
pub struct Keep {
    m: Mutex<Marks>,
    cv: Condvar,
    /// Held by the writer from its last look at the term through its write: `off` takes it, so a
    /// write already under way has landed before `off` returns and none starts after (never `m`
    /// across the write: `changed` is called under the coordinator's lock).
    w: Mutex<()>,
}

#[derive(Debug, Default)]
struct Marks {
    /// Changes saved so far.
    rev: u64,
    /// The term written for (None: not leading).
    term: Option<u64>,
    started: bool,
    stop: bool,
    /// Files written (the tests' measure of its rate).
    writes: u64,
}

impl Keep {
    /// A change saved: written within `BATCH` (or `GAP` after the last write).
    pub fn changed(&self) {
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        m.rev += 1;
        self.cv.notify_all();
    }

    /// Its coordinator no longer leads (a broker now): nothing written from here, no term's file,
    /// until a lead's step names a term again.
    pub fn off(&self) {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).term = None;
        self.cv.notify_all();
        // (A write under way lands first; the next one sees no term.)
        drop(self.w.lock().unwrap_or_else(|e| e.into_inner()));
    }

    pub fn stop(&self) {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        self.cv.notify_all();
        drop(self.w.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Files written so far.
    pub fn writes(&self) -> u64 {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).writes
    }
}

/// Names the term to write the state for, starting the writer the first time.
pub fn keep(shared: &Arc<Mutex<Shared>>, nas: Arc<dyn Nas + Send + Sync>, term: u64, pace: Pace) {
    let k = shared.lock().unwrap().keep.clone();
    let mut m = k.m.lock().unwrap_or_else(|e| e.into_inner());
    if m.term != Some(term) {
        m.term = Some(term);
        k.cv.notify_all();
    }
    if !m.started && !m.stop {
        m.started = true;
        let (weak, k2) = (Arc::downgrade(shared), k.clone());
        std::thread::Builder::new()
            .name("coord-state".into())
            .spawn(move || {
                // (A panic is said, and the next lead's step starts a writer again.)
                let k3 = k2.clone();
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(weak, k2, nas, pace))).is_err() {
                    eprintln!("coordinator: the state's writer panicked: started again at the lead's next step");
                    k3.m.lock().unwrap_or_else(|e| e.into_inner()).started = false;
                }
            })
            .map_err(|e| eprintln!("coordinator: the state's writer: {e}"))
            .ok();
    }
}

fn run(shared: Weak<Mutex<Shared>>, k: Arc<Keep>, nas: Arc<dyn Nas + Send + Sync>, pace: Pace) {
    let mut seen = 0;
    let mut written: Option<(u64, Vec<u8>)> = None;
    let mut last: Option<Instant> = None;
    loop {
        // A change, a new term, or the sweep.
        {
            let mut m = k.m.lock().unwrap_or_else(|e| e.into_inner());
            let until = Instant::now() + pace.sweep;
            while !m.stop && m.rev == seen && written.as_ref().map(|w| w.0) == m.term {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                m = k.cv.wait_timeout(m, left).unwrap_or_else(|e| e.into_inner()).0;
            }
            // The rest of the burst, and the gap since the last write.
            let wait = pace.batch.max(last.map_or(Duration::ZERO, |l| pace.gap.saturating_sub(l.elapsed())));
            let until = Instant::now() + wait;
            while !m.stop {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                m = k.cv.wait_timeout(m, left).unwrap_or_else(|e| e.into_inner()).0;
            }
            if m.stop {
                return;
            }
            seen = m.rev;
        }
        let Some(s) = shared.upgrade() else { return };
        let (term, state) = {
            let s = s.lock().unwrap();
            let term = k.m.lock().unwrap_or_else(|e| e.into_inner()).term;
            (term, super::pool_state_of(&s))
        };
        drop(s);
        let Some(term) = term else { continue };
        let Ok(b) = serde_json::to_vec(&state) else { continue };
        if written.as_ref().is_some_and(|(t, w)| *t == term && *w == b) {
            continue;
        }
        // (Not a term's any more since the snapshot: a broker now, or stopped. Held through the
        // write: `off` waits for it, so no write of this term starts or lands after `off`.)
        let _w = k.w.lock().unwrap_or_else(|e| e.into_inner());
        {
            let m = k.m.lock().unwrap_or_else(|e| e.into_inner());
            if m.stop || m.term != Some(term) {
                continue;
            }
        }
        last = Some(Instant::now());
        match nas.write_whole(&crate::agent::pool::state_path(term), &b) {
            Ok(()) => {
                written = Some((term, b));
                k.m.lock().unwrap_or_else(|e| e.into_inner()).writes += 1;
            }
            Err(e) => {
                eprintln!("coordinator: its state of term {term}: {e:#}");
                // (Tried again a `GAP` on.)
                seen = seen.wrapping_sub(1);
            }
        }
    }
}

#[cfg(all(test, not(target_os = "wasi")))]
mod tests {
    use super::*;
    use crate::coord::{client, start_for_test, Ask, Offer, PoolState};
    use crate::pool::nas::Share;

    const QUICK: Pace =
        Pace { batch: Duration::from_millis(20), gap: Duration::from_millis(200), sweep: Duration::from_secs(60) };

    fn state(root: &std::path::Path, term: u64) -> Option<PoolState> {
        let b = std::fs::read(root.join(crate::agent::pool::state_path(term))).ok()?;
        serde_json::from_slice(&b).ok()
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < end, "{what}: not in 20 s");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn ask() -> Ask {
        Ask {
            worker: "m1".into(),
            kind: "native".into(),
            can: vec!["unit".into()],
            mem_mb: 4096,
            cores: 8,
            ..Default::default()
        }
    }

    /// A member's grant is on the NAS with no step of the lead's agent after it, so a lead gone
    /// right after it (no handover) leaves it to the next: its worker's renewal is the new lead's.
    #[test]
    fn a_grant_is_on_the_nas_before_the_leads_next_step_and_the_next_lead_takes_it_up() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(&root).unwrap();
        let (c, port) = start_for_test(&d.path().join("a"), "m4", "");
        c.set_term(1);
        keep(&c.shared, Arc::new(Share::new(&root)), 1, QUICK);
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.offer("p", vec![Offer { step: "unit".into(), targets: vec![("6/1/1".into(), "k".into(), 100)], job: None }]);
        let g = w.ask(&ask()).unwrap().expect("granted");
        let key = g.lease.to_string();
        until("the grant on the NAS", || state(&root, 1).is_some_and(|s| s.leases.to_string().contains(&key)));
        // The lead gone, no word (no stop, no step): term 2's lead takes up term 1's file.
        std::mem::forget(c);
        let (b, _) = start_for_test(&d.path().join("b"), "m6", "");
        b.set_term(2);
        assert_eq!(b.load_pool_state(&state(&root, 1).unwrap()).unwrap(), 1);
        assert_eq!(b.lease_term(g.lease), 1, "the lease taken up: its worker renews it with the new lead");
    }

    /// A burst of grants is one write a `GAP` at most, the last holding them all; a stopped
    /// coordinator writes no more.
    #[test]
    fn a_burst_of_grants_is_batched() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(&root).unwrap();
        let (c, _) = start_for_test(&d.path().join("a"), "m4", "");
        c.set_term(3);
        keep(&c.shared, Arc::new(Share::new(&root)), 3, QUICK);
        let k = c.shared.lock().unwrap().keep.clone();
        until("the first write", || k.writes() == 1);
        let t0 = Instant::now();
        let mut ids = Vec::new();
        for i in 0..200 {
            ids.push(c.hold("unit", &[(format!("6/2/{i}"), "k".into())]).unwrap());
            std::thread::sleep(Duration::from_millis(5));
        }
        let took = t0.elapsed();
        until("every grant on the NAS", || {
            state(&root, 3).is_some_and(|s| ids.iter().all(|id| s.leases.to_string().contains(&id.to_string())))
        });
        let n = k.writes() - 1;
        let most = (took.as_millis() / QUICK.gap.as_millis()) as u64 + 2;
        eprintln!("200 grants in {took:?}: {n} writes (at most {most})");
        assert!(n >= 1 && n <= most, "{n} writes for 200 grants in {took:?}");
        c.stop();
        c.hold("unit", &[("6/3/1".into(), "k".into())]).unwrap();
        std::thread::sleep(QUICK.gap * 2);
        assert_eq!(k.writes(), n + 1, "stopped: no more");
    }

    /// A lead that lost its term and kept its coordinator on as a broker writes its old term's
    /// file no more (its job leases dropped would otherwise be written over the term's state).
    #[test]
    fn a_coordinator_no_longer_leading_writes_its_term_no_more() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(&root).unwrap();
        let (c, _) = start_for_test(&d.path().join("a"), "m4", "");
        c.set_term(4);
        keep(&c.shared, Arc::new(Share::new(&root)), 4, QUICK);
        let k = c.shared.lock().unwrap().keep.clone();
        let id = c.hold("unit", &[("6/4/1".into(), "k".into())]).unwrap();
        until("the lease on the NAS", || {
            state(&root, 4).is_some_and(|s| s.leases.to_string().contains(&id.to_string()))
        });
        // (Counted once `off` has returned: the file can be on the NAS before its write is counted.)
        c.become_broker();
        let (n, before) = (k.writes(), std::fs::read(root.join(crate::agent::pool::state_path(4))).unwrap());
        assert!(n >= 1);
        c.hold("unit", &[("6/4/2".into(), "k".into())]).unwrap();
        std::thread::sleep(QUICK.gap * 3);
        assert_eq!(k.writes(), n, "a broker: no more writes");
        assert_eq!(std::fs::read(root.join(crate::agent::pool::state_path(4))).unwrap(), before);
        // Leading again (a later term): that term's file.
        keep(&c.shared, Arc::new(Share::new(&root)), 6, QUICK);
        until("term 6's file", || state(&root, 6).is_some());
        c.stop();
        std::thread::sleep(QUICK.gap * 2);
        assert_eq!(k.writes(), n + 1, "stopped: no more");
    }
}
