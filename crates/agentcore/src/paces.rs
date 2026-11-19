//! Each Mac's time against the lead's (docs/pool.md §7.7, Each Mac's speed): from their speeds
//! (jobrt::timings::Speeds, task #1418: the instructions each Mac's performance cores retire a
//! second of their CPU, measured on the pool's clean runs), so a run's CPU on one Mac is the lead's
//! CPU seconds by the two Macs' speeds, and a prediction for a Mac is its needs at its speed
//! (crate::needs, task #1416). The cost book reads every run's CPU at the lead's speed by it, the
//! forecast (crate::agent::forecast) runs each lane at its Mac's time, and placement draws a Mac by
//! its speed. A Mac whose speed isn't measured is at the lead's.
//!
//! Beside it, each Mac's jobs' start at each step (`with_starts`, task #927): the time a job takes
//! besides its targets, from the build's history.

use crate::coord::history::Event;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The history a Mac's jobs' start is read from (s): the week the lead's history keeps.
pub const STARTS_S: u64 = crate::coord::history::KEEP_S;

/// The Mac a worker is on: a slot's worker (`<Mac> #k`) and an old second slot's
/// (`<Mac> (second job)`) its Mac's.
pub fn mac_of(worker: &str) -> &str {
    let w = worker.split(" #").next().unwrap_or(worker);
    w.strip_suffix(" (second job)").unwrap_or(w)
}

/// Each Mac's time against the lead's (`of`): its time for the same work over the lead's (2: half
/// its speed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Paces {
    /// The lead's Mac: 1. Empty: none worked out (`known`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lead: String,
    /// The lead's speed (billions of instructions a second of a performance core's CPU; 0: not
    /// measured): what a run's instructions are in the lead's CPU seconds by.
    #[serde(skip_serializing_if = "is_zero")]
    pub lead_ginsn_s: f64,
    /// Mac → its time against the lead's, the lead's speed over its own.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub macs: BTreeMap<String, f64>,
    /// Mac → step → its jobs' time past their targets' (s; `with_starts`, task #927).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub starts: BTreeMap<String, BTreeMap<String, f64>>,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// Whether event `e` is a job done well, its worker, step, targets and time said: what a job's
/// start (`Paces::with_starts`) is read from.
pub fn job(e: &Event) -> bool {
    matches!(e.kind.as_str(), "done" | "end")
        && e.ok == Some(true)
        && !e.targets.is_empty()
        && e.worker.is_some()
        && e.step.is_some()
        && e.secs.is_some_and(f64::is_finite)
}

impl Paces {
    /// Each Mac's time against `lead`'s (a worker of it, or its Mac) by their speeds `speeds`: on
    /// their performance cores, where most of a job's work runs (a Mac measured on its efficiency
    /// cores alone, by those). A Mac not measured, or a lead not measured, at the lead's.
    pub fn of(speeds: &jobrt::timings::Speeds, lead: &str) -> Paces {
        let lead = mac_of(lead);
        let speed = |m: &jobrt::timings::MacSpeed| m.p.as_ref().or(m.e.as_ref()).map(|k| k.ginsn_s).filter(|&s| s > 0.0);
        let at = speeds.macs.get(lead).and_then(speed);
        let mut out = Paces { lead: lead.to_string(), lead_ginsn_s: at.unwrap_or(0.0), ..Default::default() };
        if let Some(l) = at {
            for (m, s) in &speeds.macs {
                if let Some(s) = speed(s).filter(|_| m != lead) {
                    out.macs.insert(m.clone(), round(l / s));
                }
            }
        }
        out
    }

    /// With each Mac's jobs' start at each step (task #927): the median, over its jobs of the step
    /// done well in the last `STARTS_S` at `now` (`job`), of a job's time past its targets' times summed,
    /// each target's as its cost record `costs` says (crate::coord::Cost: its run's own time) where
    /// that run was on the job's Mac, the way the job ran (`Event::v`), and that the step runs now
    /// (crate::coord::cost_version), a job one of whose targets' record isn't so, or that doesn't say
    /// its way (an event before task #926), not counted: the time a job takes besides its targets
    /// (its records and regions read, its hand-off), which a batch of quick targets is mostly made of
    /// (slope-deep's 5 to 12 s a job on the lead's history to 5 Nov, its empty targets a second or
    /// less each). A job of an older way says nothing of it, as its runs size nothing (crate::needs):
    /// task #929 changed slope-deep's and terrain-deep's work.
    pub fn with_starts(
        mut self,
        the_plan: &dyn crate::agent::seam::Plan,
        events: &[Event],
        costs: &BTreeMap<String, crate::coord::Cost>,
        now: u64,
    ) -> Paces {
        let from = now.saturating_sub(STARTS_S);
        let mut by: BTreeMap<(&str, &str), Vec<f64>> = BTreeMap::new();
        for e in events.iter().filter(|e| e.t > from && e.t <= now && job(e)) {
            let (Some(w), Some(step), Some(secs)) = (e.worker.as_deref(), e.step.as_deref(), e.secs) else { continue };
            let mac = mac_of(w);
            let Some(v) = e.v.filter(|&v| v >= the_plan.cost_version(step)) else { continue };
            let own: Option<u64> = e
                .targets
                .iter()
                .map(|t| {
                    let c = costs.get(&crate::coord::cost_key(step, t))?;
                    let ours = c.worker.as_deref().map(mac_of) == Some(mac) && c.v == v;
                    ours.then_some(c.secs)
                })
                .sum();
            if let Some(own) = own {
                by.entry((mac, step)).or_default().push((secs - own as f64).max(0.0));
            }
        }
        self.starts.clear();
        for ((mac, step), mut v) in by {
            self.starts.entry(mac.to_string()).or_default().insert(step.to_string(), round(median(&mut v)));
        }
        self
    }

    /// `worker`'s Mac's jobs' start at `step` (`with_starts`; 0: none measured).
    pub fn start(&self, worker: &str, step: &str) -> f64 {
        self.starts.get(mac_of(worker)).and_then(|s| s.get(step)).copied().unwrap_or(0.0)
    }

    /// Whether any were worked out (a lead named).
    pub fn known(&self) -> bool {
        !self.lead.is_empty()
    }

    /// Whether `worker`'s Mac's time is measured: the lead's, or a Mac whose speed and the lead's
    /// are.
    pub fn knows(&self, worker: &str) -> bool {
        let mac = mac_of(worker);
        (self.known() && mac == self.lead) || self.macs.contains_key(mac)
    }

    /// `worker`'s Mac's time against the lead's: 1 on the lead or a Mac not measured.
    pub fn get(&self, worker: &str) -> f64 {
        self.macs.get(mac_of(worker)).copied().unwrap_or(1.0)
    }

    /// The lead's speed, where measured.
    pub fn lead_ginsn_s(&self) -> Option<f64> {
        (self.lead_ginsn_s > 0.0).then_some(self.lead_ginsn_s)
    }
}

/// The median of `v` (the mean of the middle two of an even count); 0 with none.
fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// To the thousandth: what's kept and compared is a measure, not its last bits.
fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(t: u64, worker: &str, step: &str, target: &str, secs: f64) -> Event {
        Event {
            t,
            worker: Some(worker.into()),
            step: Some(step.into()),
            targets: vec![target.into()],
            secs: Some(secs),
            ok: Some(true),
            v: Some(1),
            ..Event::new("done")
        }
    }

    #[test]
    fn a_mac_of_a_slot() {
        assert_eq!(mac_of("Mac-mini-de-Brandon #3"), "Mac-mini-de-Brandon");
        assert_eq!(mac_of("m1 (second job)"), "m1");
        assert_eq!(mac_of("m4"), "m4");
    }

    /// Each Mac's time against the lead's is the lead's speed over its own, on their performance
    /// cores; a Mac not measured, or with the lead not measured, at the lead's.
    #[test]
    fn each_macs_time_is_the_leads_speed_over_its_own() {
        let k = |g: f64| Some(jobrt::timings::KindSpeed { ginsn_s: g, steps: vec![], runs: 1, with: vec![] });
        let m = |p: Option<f64>, e: Option<f64>| jobrt::timings::MacSpeed { p: p.and_then(k), e: e.and_then(k) };
        let speeds = jobrt::timings::Speeds {
            macs: [
                ("mini".to_string(), m(Some(15.0), Some(5.0))),
                ("m1".to_string(), m(Some(7.5), Some(3.0))),
                ("m9".to_string(), m(None, Some(5.0))),
            ]
            .into(),
        };
        let p = Paces::of(&speeds, "mini #2");
        assert_eq!((p.get("mini #3"), p.get("m1 #2"), p.get("m9"), p.get("m4")), (1.0, 2.0, 3.0, 1.0));
        assert!(p.knows("mini") && p.knows("m1") && !p.knows("m4"));
        assert_eq!(p.lead_ginsn_s(), Some(15.0));
        let none = Paces::of(&jobrt::timings::Speeds::default(), "mini");
        assert!(none.known() && none.macs.is_empty() && none.lead_ginsn_s().is_none() && !none.knows("m1"));
        assert!(!Paces::default().known());
    }

    /// Task #927: a Mac's jobs' start at a step is the median of its jobs' time past their
    /// targets' own runs (each its cost record, run on that Mac the way the job ran); a job one of
    /// whose targets' record is another Mac's, or another way's, says nothing of it; nor does a job
    /// of a way older than its step's now, or one not saying its way.
    #[test]
    fn a_macs_jobs_start_is_their_time_past_their_targets() {
        let c = |worker: &str, secs: u64| crate::coord::Cost {
            secs,
            worker: Some(worker.into()),
            v: 1,
            ..Default::default()
        };
        let costs: BTreeMap<String, crate::coord::Cost> = [
            ("slope-deep 10/0/1", c("m1 #2", 1)),
            ("slope-deep 10/0/2", c("m1", 2)),
            ("slope-deep 10/0/3", c("m1", 50)),
            ("slope-deep 10/0/4", c("m4", 1)),
            ("slope-deep 10/0/5", crate::coord::Cost { v: 0, ..c("m1", 1) }),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let many = |t: u64, w: &str, ts: &[&str], secs: f64| Event {
            targets: ts.iter().map(|x| x.to_string()).collect(),
            ..job(t, w, "slope-deep", "", secs)
        };
        let ev = vec![
            many(10, "m1", &["10/0/1", "10/0/2"], 13.0),
            many(20, "m1 #3", &["10/0/3"], 58.0),
            many(30, "m1", &["10/0/2", "10/0/3"], 62.0),
            many(40, "m1", &["10/0/1", "10/0/4"], 500.0),
            Event { v: Some(2), ..many(50, "m1", &["10/0/1"], 500.0) },
            // (Task #927 review: a job of a way older than the step's now, its record of that way
            // too (slope-deep's before task #929's merge), and one that doesn't say its way.)
            Event { v: Some(0), ..many(60, "m1", &["10/0/5"], 500.0) },
            Event { v: None, ..many(70, "m1", &["10/0/2"], 500.0) },
        ];
        let p = Paces::default().with_starts(crate::agent::seam::for_tests(), &ev, &costs, 100);
        assert_eq!(p.start("m1 #2", "slope-deep"), 10.0, "of 10, 8 and 10");
        assert_eq!(p.start("m4", "slope-deep"), 0.0, "none measured");
        assert_eq!(p.start("m1", "pack"), 0.0);
        assert!(
            Paces::default()
                .with_starts(crate::agent::seam::for_tests(), &ev, &costs, 100 + STARTS_S + 100)
                .starts
                .is_empty(),
            "out of the window"
        );
    }
}
