//! Placement by how crucial a job is, and work offered ahead (docs/pool.md §7.2, §12 phase 4's
//! eleventh batch; docs/workers.md §3), behind the switch `state/pool/placement` (`SWITCH`; off
//! while missing or `off` in it: the agent and its coordinator as they were, exactly). It acts only
//! in the pool.
//!
//! - **How crucial** (`Placement::slack`): each step's slack in the forecast, the time between its
//!   last target's forecast end and the whole build's (`slack`): the long poles (no slack) first.
//!   The lead's coordinator tries a member's ask on the offers in that order, and within a step
//!   from its target measured to take longest (crate::coord's `pick_every`).
//! - **Who takes it** (`draw`): of the members asking lately whose measured room fits a target,
//!   one drawn with a weight of its measured speed (the forecast's, against the lead's; one not
//!   measured, the lead's own), as a function of the target and the time (a minute's window,
//!   `DRAW_S`, from when the coordinator was told): the same inputs, the same draw. A member asking
//!   for a target drawn to another passes it by; a task likewise among the pages and workers that
//!   could take it, weighted by their measured pace at its kind. No rule names a Mac or a kind of
//!   job.
//! - **Offered ahead** (`AHEAD_ENV`, told to every job): a terrain job offers its next piece's
//!   subtrees as it begins a piece; each task of a terrain piece or a tree cover's row says how
//!   much of this Mac's work comes before the job needs it (`ahead`, in tasks' worth: a head start
//!   of that many times this Mac's measured time at its kind). A worker measured slower than this
//!   Mac at a kind is given such a task only while its pace (with the margin) times this Mac's time
//!   fits in the head start left (crate::coord::task's `pick`).

use std::collections::BTreeMap;
use std::path::Path;

/// The switch (`on` in it, or present; `off` in it, or missing: off, the agent as it was).
pub const SWITCH: &str = "state/pool/placement";

/// What a job is told when the switch acts: offer ahead (crate::terrain_task, crate::trees::task).
pub const AHEAD_ENV: &str = "SCENIC_OFFER_AHEAD";

/// Whether this job offers its tasks ahead (`AHEAD_ENV` set by its agent).
pub fn ahead_on() -> bool {
    std::env::var(AHEAD_ENV).is_ok_and(|v| v == "1")
}

/// How long one draw holds (s): a target drawn to a member that doesn't ask for it in that time is
/// drawn again.
pub const DRAW_S: u64 = 60;

/// Whether placement is on, as the switch at `root` says; None when it can't be read now.
pub fn on(root: &Path) -> Option<bool> {
    match super::nasread::read_to_string(&root.join(SWITCH)) {
        Ok(s) => Some(s.trim() != "off"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// What the lead's coordinator places by (`crate::coord::Coordinator::set_placement`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placement {
    /// Each step's slack in the forecast (s); a step the forecast gives no end is last.
    pub slack: BTreeMap<String, u64>,
    /// Each worker's measured speed against the lead's (a worker not in it: 1).
    pub speed: BTreeMap<String, f64>,
}

impl Placement {
    /// From the forecast: its steps' slack, and the speeds it measured (not those it guessed).
    pub fn from_forecast(f: &super::forecast::Forecast) -> Placement {
        Placement {
            slack: slack(f),
            speed: f
                .speed
                .iter()
                .filter(|(w, s)| !f.guessed.contains(w) && s.is_finite() && **s > 0.0)
                .map(|(w, s)| (w.clone(), *s))
                .collect(),
        }
    }

    /// Step `step`'s slack (u64::MAX: none known).
    pub fn slack_of(&self, step: &str) -> u64 {
        self.slack.get(step).copied().unwrap_or(u64::MAX)
    }

    /// Worker `w`'s weight in a draw: its measured speed, else the lead's (1).
    pub fn weight(&self, w: &str) -> f64 {
        self.speed.get(w).copied().unwrap_or(1.0)
    }
}

/// The workers `names` measured against the lead (`lead`) by the build's history
/// (super::forecast::speeds: the shared steps both did), those measured alone.
pub fn measured_speeds(events: &[crate::coord::history::Event], lead: &str, names: &[String]) -> BTreeMap<String, f64> {
    super::forecast::speeds(events, lead, names, 1.0)
        .into_iter()
        .filter(|(_, (_, m))| *m)
        .map(|(n, (s, _))| (n, s))
        .collect()
}

/// Each step's slack in forecast `f` (s): the build's end less the step's (0 when it ends last).
pub fn slack(f: &super::forecast::Forecast) -> BTreeMap<String, u64> {
    let Some(end) = f.done_at.or_else(|| f.steps.iter().filter_map(|s| s.done_at).max()) else {
        return BTreeMap::new();
    };
    f.steps.iter().filter_map(|s| Some((s.step.clone(), end.saturating_sub(s.done_at?)))).collect()
}

/// A stable 64-bit hash of `key` (FNV-1a, then mixed: splitmix64's finaliser), the same on every
/// Mac and every run.
pub fn hash(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// The worker drawn for `key` (a target or a task, and the draw's window) among `who` (name,
/// weight), each with a chance of its weight's share: the same `key` and `who` (in any order), the
/// same worker. None when no one has a weight above 0.
pub fn draw<'a>(key: &str, who: &'a [(String, f64)]) -> Option<&'a str> {
    let mut v: Vec<&(String, f64)> = who.iter().filter(|(_, w)| w.is_finite() && *w > 0.0).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.dedup_by(|a, b| a.0 == b.0);
    let total: f64 = v.iter().map(|(_, w)| w).sum();
    if v.is_empty() || total <= 0.0 {
        return None;
    }
    let u = (hash(key) >> 11) as f64 / (1u64 << 53) as f64 * total;
    let mut acc = 0.0;
    for (n, w) in &v {
        acc += w;
        if u < acc {
            return Some(n.as_str());
        }
    }
    v.last().map(|(n, _)| n.as_str())
}

/// A draw's key: what's drawn for, and its window (`since_s`, in `DRAW_S` steps).
pub fn key(what: &str, since_s: u64) -> String {
    format!("{what}#{}", since_s / DRAW_S)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_off_while_missing_or_off() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(SWITCH), "off\n").unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::write(d.path().join(SWITCH), "on\n").unwrap();
        assert_eq!(on(d.path()), Some(true));
    }

    #[test]
    fn draws_repeat_and_share_by_speed() {
        let who = vec![("m1".to_string(), 1.0), ("m4".to_string(), 3.0)];
        let rev: Vec<(String, f64)> = who.iter().rev().cloned().collect();
        let mut n4 = 0;
        for i in 0..4000 {
            let k = key(&format!("terrain 6/{}/{}", i % 64, i / 64), 0);
            let a = draw(&k, &who).unwrap();
            // The same inputs, in any order: the same draw.
            assert_eq!(Some(a), draw(&k, &rev));
            n4 += (a == "m4") as usize;
        }
        // Three quarters to the one three times as fast (a fixed hash: no chance in it).
        let share = n4 as f64 / 4000.0;
        assert!((share - 0.75).abs() < 0.03, "{share}");
        // A slow worker still gets its share; none with no weight; one alone, everything.
        let slow = vec![("page".to_string(), 0.1), ("m4".to_string(), 1.0)];
        let n = (0..4000).filter(|i| draw(&key(&format!("t{i}"), 0), &slow) == Some("page")).count();
        assert!((n as f64 / 4000.0 - 1.0 / 11.0).abs() < 0.02, "{n}");
        assert_eq!(draw("x", &[("a".into(), 0.0)]), None);
        assert_eq!(draw("x", &[("a".into(), 0.5)]), Some("a"));
        // A new window, another draw (no target held by one forever).
        assert_ne!(key("t", 0), key("t", DRAW_S));
        assert_eq!(key("t", 0), key("t", DRAW_S - 1));
    }

    /// A member's speed under the name it asks with, from the history's runs of a shared step by
    /// it and by the lead; one with too few runs, unmeasured (left out: weighed as the lead).
    #[test]
    fn a_members_speed_is_measured_under_its_own_name() {
        use crate::coord::history::Event;
        let run = |w: &str, secs: f64| Event {
            worker: Some(w.into()),
            step: Some("peaks".into()),
            targets: vec!["6/1/1".into()],
            secs: Some(secs),
            ok: Some(true),
            ..Event::new("done")
        };
        let mut ev: Vec<Event> = (0..3).map(|_| run("MacBook-Pro-de-Brandon", 300.0)).collect();
        ev.extend((0..3).map(|_| run("m4", 100.0)));
        ev.push(run("ipad", 50.0));
        let s = measured_speeds(&ev, "m4", &["MacBook-Pro-de-Brandon".into(), "ipad".into()]);
        assert_eq!(s.len(), 1);
        assert!((s["MacBook-Pro-de-Brandon"] - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_long_poles_have_no_slack() {
        use super::super::forecast::{Forecast, StepFc};
        let f = Forecast {
            done_at: Some(1000),
            steps: vec![
                StepFc { step: "terrain".into(), done_at: Some(1000), ..Default::default() },
                StepFc { step: "pack".into(), done_at: Some(400), ..Default::default() },
                StepFc { step: "x".into(), done_at: None, ..Default::default() },
            ],
            speed: [("m1".to_string(), 0.5), ("ipad".to_string(), 0.2)].into(),
            guessed: vec!["ipad".into()],
            ..Default::default()
        };
        let p = Placement::from_forecast(&f);
        assert_eq!((p.slack_of("terrain"), p.slack_of("pack"), p.slack_of("x")), (0, 600, u64::MAX));
        assert_eq!((p.weight("m1"), p.weight("ipad"), p.weight("m4")), (0.5, 1.0, 1.0));
    }
}
