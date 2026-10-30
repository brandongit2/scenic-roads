//! The owner's asks taken up at once, on a thread of the agent's own (task #846; the first slice of
//! task #871, the agent's loop split): the loop's pass can run for minutes on a slow NAS, so an
//! ask it takes up only between passes leaves the menu bar and the build page saying nothing.
//!
//! This Mac's own hold ask (crate::control::HoldRequest, no member: the menu bar's Pause This Mac,
//! `scenic pause --here`) is watched for every `EVERY`: taken, its setting kept in the agent's
//! folder and answered, the running jobs told to stop at their next safe point ("drain" in each's
//! channel), and the heartbeat written again at once with the hold's stage (`Status::say`), here
//! and on the NAS, so the lead and the page see "pausing" within seconds. The loop takes the
//! setting up at its next pass (`Agent::sync_hold`): its jobs given back as they stop, nothing new
//! started. A member's ask of another Mac needs the pool, and stays the loop's.

use super::{nasread, now_s, write_beat, write_replace, Status};
use crate::control::{self, HoldAnswer, HoldState};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use store::nasfs::NasPath;

/// How often the thread looks for an ask (a file's stat in the agent's folder).
pub const EVERY: Duration = Duration::from_millis(250);

/// The last heartbeat the loop wrote, and where: written again by the thread with a new hold.
#[derive(Clone)]
pub struct Beat {
    pub status: Status,
    pub local: PathBuf,
    pub shared: Option<NasPath>,
}

/// What the thread and the loop share.
pub struct Asks {
    home: PathBuf,
    /// This Mac's hold as last set, by either: the later setting wins (`HoldState::at`).
    pub hold: Mutex<HoldState>,
    /// The running jobs' channels (crate::control::CONTROL_ENV), as the loop last saw them.
    pub controls: Mutex<Vec<PathBuf>>,
    /// The loop's last heartbeat.
    pub beat: Mutex<Option<Beat>>,
}

impl Asks {
    pub fn new(home: PathBuf, hold: HoldState) -> Arc<Asks> {
        Arc::new(Asks { home, hold: Mutex::new(hold), controls: Mutex::default(), beat: Mutex::default() })
    }

    /// This Mac's hold as last set.
    pub fn hold(&self) -> HoldState {
        self.hold.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Sets the hold (`st`) unless a later setting stands; whether it did.
    pub fn set(&self, st: &HoldState) -> bool {
        let mut h = self.hold.lock().unwrap_or_else(|e| e.into_inner());
        if st.at < h.at {
            return false;
        }
        *h = st.clone();
        true
    }

    /// Starts the thread.
    pub fn spawn(self: &Arc<Asks>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("asks".into())
            .spawn(move || loop {
                me.round();
                std::thread::sleep(EVERY);
            })
            .ok();
    }

    /// One look: this Mac's own hold ask taken up, if one waits (true when one did).
    pub fn round(&self) -> bool {
        let Some(req) = control::peek_hold_request(&self.home).filter(|r| r.member.is_none()) else { return false };
        let Some(req) = control::take_hold_request(&self.home).filter(|r| r.member.is_none() && *r == req) else {
            return false;
        };
        // (This Mac's own ask wins over any earlier setting, whatever another Mac's clock put on
        // it.)
        let st = {
            let mut h = self.hold.lock().unwrap_or_else(|e| e.into_inner());
            *h = HoldState { held: req.held.clone(), at: req.at.max(h.at + 1) };
            h.clone()
        };
        let what = if st.held.is_some() { "paused" } else { "going on" };
        let kept = serde_json::to_vec(&st)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.home.join(control::HOLD), &b));
        if let Err(e) = kept {
            eprintln!("agent: keeping this Mac's pause: {e:#}");
        }
        eprintln!("agent: this Mac is {what} (taken up at once)");
        control::answer_hold(&self.home, &HoldAnswer { ask: req, said: format!("this Mac is {what}"), ok: true });
        // (Paused: every job told at once; going on, the loop tells them, as it knows which other
        // drain, the memory guard's say, still stands.)
        if st.held.is_some() {
            for c in self.controls.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                if let Err(e) = std::fs::write(c, b"drain") {
                    eprintln!("agent: asking {} to stop: {e}", c.display());
                }
            }
        }
        self.beat_again(&st);
        true
    }

    /// The loop's last heartbeat written again now with hold `st` (its stage, its verdict), here
    /// and on the NAS, and kept as the watchdog's beat.
    fn beat_again(&self, st: &HoldState) {
        let Some(mut b) = self.beat.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        // (Going on: resuming, until the loop's next pass starts work again.)
        b.status.say(None, st, st.held.is_none());
        b.status.beat = now_s();
        let Ok(body) = serde_json::to_vec_pretty(&b.status) else { return };
        // (Held from the local copy's write on, as the loop's own beat: an older content never
        // lands after this one.)
        let mut kept = nasread::BEATING.lock().unwrap_or_else(|e| e.into_inner());
        write_replace(&b.local, &body).ok();
        let Some(p) = b.shared.clone() else { return };
        match write_beat(&p, body) {
            Ok(()) => {
                let (mut status, own) = (b.status, b.local);
                *kept = Some(nasread::Beating {
                    at: status.beat,
                    again: Box::new(move |t| {
                        status.beat = t;
                        let b = serde_json::to_vec_pretty(&status)?;
                        write_replace(&own, &b).ok();
                        write_beat(&p, b)
                    }),
                });
            }
            Err(e) => {
                eprintln!("agent: heartbeat: {e:#}");
                *kept = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(s["out_of_touch_s"], crate::pool::beat::OUT_OF_TOUCH_S);
        assert_eq!(s["checklist"][0]["counts"][0], "1,234 of 2,000 z6 tiles (12 with nothing to publish)");
        assert_eq!(s["checklist"][0]["items"][1]["count"], "done");
        assert_eq!(s["checklist"][0]["items"][1]["finished"], true);
    }

    /// Task #846: this Mac's own pause ask taken up at once, off the loop: kept, answered, the
    /// running jobs told to drain, and the heartbeat written again saying "pausing" with them;
    /// another member's ask left for the loop; a resume beats "resuming".
    #[test]
    fn a_hold_ask_is_taken_up_at_once() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        let asks = Asks::new(home.clone(), HoldState::default());
        let control = home.join("control");
        std::fs::write(&control, b"run").unwrap();
        *asks.controls.lock().unwrap() = vec![control.clone()];
        let mut st = Status { host: "m1".into(), beat: 1, ..Default::default() };
        st.set_jobs(
            vec![super::super::JobView {
                id: "unit 6/1/2".into(),
                what: "z6 tile 6/1/2".into(),
                started: 1,
                paused: None,
                pausing: None,
                tail: String::new(),
                parts: Vec::new(),
                part: None,
                progress: None,
                mem_mb: None,
                threads: None,
                waiting: None,
                slot: Some(0),
                live: None,
            }],
            true,
        );
        let local = home.join("status.json");
        *asks.beat.lock().unwrap() = Some(Beat { status: st, local: local.clone(), shared: None });
        assert!(!asks.round());
        // Another member's: the loop's.
        control::request_hold(&home, Some("m4"), Some(Hold::new("scenic pause on m1"))).unwrap();
        assert!(!asks.round() && home.join(control::HOLD_REQUEST).exists());
        let r = control::request_hold(&home, None, Some(Hold::new("the menu bar on m1"))).unwrap();
        assert!(asks.round());
        assert!(!home.join(control::HOLD_REQUEST).exists());
        assert!(control::hold_answer(&home, &r).is_some_and(|a| a.ok));
        assert!(control::read_hold(&home).held.is_some() && asks.hold().held.is_some());
        assert_eq!(std::fs::read(&control).unwrap(), b"drain");
        let said: Status = serde_json::from_slice(&std::fs::read(&local).unwrap()).unwrap();
        let h = said.hold.unwrap();
        assert_eq!((h.stage, h.finishing), (HoldStage::Pausing, vec!["unit 6/1/2".to_string()]));
        assert_eq!(said.verdict.map(|v| v.state), Some("pausing".to_string()));
        assert!(said.held.is_some() && said.beat > 1);
        // Going on: resuming, until the loop's pass.
        control::request_hold(&home, None, None).unwrap();
        assert!(asks.round());
        let said: Status = serde_json::from_slice(&std::fs::read(&local).unwrap()).unwrap();
        assert_eq!(said.hold.map(|h| h.stage), Some(HoldStage::Resuming));
        assert!(said.held.is_none());
    }
}
