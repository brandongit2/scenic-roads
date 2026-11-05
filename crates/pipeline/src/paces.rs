//! Each Mac's pace at each step (task #926; docs/pool.md §7.7, Each Mac's pace): how long a target
//! of the step takes it against the lead, from the build's history. The cost book
//! (crate::needs::Book) puts every run's time and NAS rate at the lead's pace by it before a step's
//! model is made of them, and a prediction for a Mac is the model at that Mac's pace; the forecast
//! (crate::agent::forecast, code 4) puts its costs at the lead's pace and runs each lane at its
//! Mac's pace at each step the same way.
//!
//! Measured on the same targets: a pair is a target of a step done well as a job of its own on a
//! Mac and on the lead, the way the step ran the same (`Event::v`) and both in the last `PACE_S`;
//! its log ratio the Mac's time over the lead's (the lead's run nearest in time). A Mac's pace at a
//! step is the median of its pairs' ratios, drawn toward its pace over its steps (the median of its
//! steps' medians, drawn toward the lead's) by `PRIOR_PAIRS` pairs' weight: `(n·median + K·prior)
//! / (n + K)`. A Mac with no pair is the lead's pace (1), everywhere.
//!
//! Why so: the same target on both takes the target's work and the job's size out (a job of one
//! target each side), so what the scheduler gave a Mac (its heavier tiles, its smaller jobs, which
//! a pace scaling its jobs' sizes would feed back into) can't move its pace. Per-target means over
//! all of a Mac's jobs did: the M4's terrain-deep read 4.49 times the mini's over a day, 1.26 over
//! a week, where its same targets took 1.39 day after day. Load from outside the pool still shows
//! (a pair's runs are what they were); the forecast's hold on each Mac (`held_back`) and its steps of
//! late (`took`) carry what held a Mac back lately.
//!
//! `PACE_S` and `PRIOR_PAIRS` were picked by a held-out test (docs/pool.md §7.7): over the
//! history of 8 Oct to 4 Nov (1,973 pairs), each day's pairs scored against the paces of the window
//! before it, the windows the lead's history holds (1 to 7 days) and weights 0 to 30: 5 days and
//! 10 pairs erred least (mean |log error| 0.834 against 0.932 with no pace; per-target means of
//! every job instead, 0.920 against 1.000).

use crate::coord::history::Event;
use det::Det;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The history a pace is read from (s).
pub const PACE_S: u64 = 5 * 86_400;
/// How many pairs' weight a Mac's pace over its steps has against its pairs at a step (and against
/// the lead's pace, for its pace over its steps, by its steps).
pub const PRIOR_PAIRS: f64 = 10.0;

/// The Mac a worker is on: a slot's worker (`<Mac> #k`) and an old second slot's
/// (`<Mac> (second job)`) its Mac's.
pub fn mac_of(worker: &str) -> &str {
    let w = worker.split(" #").next().unwrap_or(worker);
    w.strip_suffix(" (second job)").unwrap_or(w)
}

/// Each Mac's pace against the lead's (`of`): its time a target over the lead's (2: half its speed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Paces {
    /// The lead's Mac: 1 at every step. Empty: no paces worked out (`known`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lead: String,
    /// Mac → step → its pace at the step.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, BTreeMap<String, f64>>,
    /// Mac → its pace over its steps.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub macs: BTreeMap<String, f64>,
    /// Mac → step → its jobs' time past their targets' (s; `with_starts`, task #927).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub starts: BTreeMap<String, BTreeMap<String, f64>>,
}

/// A run of a target, as a job of its own done well: when, its Mac, its time, its step's way.
struct Run<'a> {
    t: u64,
    mac: &'a str,
    secs: f64,
    v: Option<u32>,
}

/// Whether event `e` is a run a pace reads: a job of one target done well (its end or done).
pub fn reads(e: &Event) -> bool {
    job(e) && e.targets.len() == 1
}

/// Whether event `e` is a job done well, its worker, step, targets and time said: what a pace
/// (`reads`) and a job's start (`Paces::with_starts`) are read from.
pub fn job(e: &Event) -> bool {
    matches!(e.kind.as_str(), "done" | "end")
        && e.ok == Some(true)
        && !e.targets.is_empty()
        && e.worker.is_some()
        && e.step.is_some()
        && e.secs.is_some_and(f64::is_finite)
}

impl Paces {
    /// The paces at `now` of every Mac against `lead`'s (a worker of it, or its Mac), from `events`.
    pub fn of(events: &[Event], lead: &str, now: u64) -> Paces {
        let lead = mac_of(lead);
        let from = now.saturating_sub(PACE_S);
        // ((step, target) → its runs.)
        let mut runs: BTreeMap<(&str, &str), Vec<Run>> = BTreeMap::new();
        for e in events.iter().filter(|e| e.t > from && e.t <= now && reads(e)) {
            let (Some(w), Some(step), Some(secs), [target]) =
                (e.worker.as_deref(), e.step.as_deref(), e.secs, &e.targets[..])
            else {
                continue;
            };
            runs.entry((step, target)).or_default().push(Run { t: e.t, mac: mac_of(w), secs: secs.max(1.0), v: e.v });
        }
        // ((Mac, step) → its pairs' log ratios.)
        let mut pairs: BTreeMap<(&str, &str), Vec<f64>> = BTreeMap::new();
        for ((step, _), rs) in &runs {
            for r in rs.iter().filter(|r| r.mac != lead) {
                let l = rs.iter().filter(|l| l.mac == lead && l.v == r.v).min_by_key(|l| l.t.abs_diff(r.t));
                if let Some(l) = l {
                    pairs.entry((r.mac, step)).or_default().push((r.secs / l.secs).dln());
                }
            }
        }
        let mut out = Paces { lead: lead.to_string(), ..Default::default() };
        let medians: Vec<((&str, &str), f64, usize)> =
            pairs.into_iter().map(|(k, mut v)| (k, median(&mut v), v.len())).collect();
        let mut macs: Vec<&str> = medians.iter().map(|m| m.0 .0).collect();
        macs.dedup();
        for mac in macs {
            let mut steps: Vec<f64> = medians.iter().filter(|m| m.0 .0 == mac).map(|m| m.1).collect();
            let k = steps.len() as f64;
            let prior = median(&mut steps) * k / (k + PRIOR_PAIRS);
            out.macs.insert(mac.to_string(), round(prior.dexp()));
            let each = medians.iter().filter(|m| m.0 .0 == mac).map(|&((_, step), m, n)| {
                let n = n as f64;
                (step.to_string(), round(((n * m + PRIOR_PAIRS * prior) / (n + PRIOR_PAIRS)).dexp()))
            });
            out.steps.insert(mac.to_string(), each.collect());
        }
        out
    }

    /// With each Mac's jobs' start at each step (task #927): the median, over its jobs of the step
    /// done well in the last `PACE_S` at `now` (`job`), of a job's time past its targets' times summed,
    /// each target's as its cost record `costs` says (crate::coord::Cost: its run's own time) where
    /// that run was on the job's Mac the way the job ran (`Event::v`, where said), a job one of whose
    /// targets' record isn't so not counted: the time a job takes besides its targets (its records
    /// and regions read, its hand-off), which a batch of quick targets is mostly made of (slope-deep's
    /// 5 to 12 s a job on the lead's history to 5 Nov, its empty targets a second or less each).
    pub fn with_starts(mut self, events: &[Event], costs: &BTreeMap<String, crate::coord::Cost>, now: u64) -> Paces {
        let from = now.saturating_sub(PACE_S);
        let mut by: BTreeMap<(&str, &str), Vec<f64>> = BTreeMap::new();
        for e in events.iter().filter(|e| e.t > from && e.t <= now && job(e)) {
            let (Some(w), Some(step), Some(secs)) = (e.worker.as_deref(), e.step.as_deref(), e.secs) else { continue };
            let mac = mac_of(w);
            let own: Option<u64> = e
                .targets
                .iter()
                .map(|t| {
                    let c = costs.get(&crate::coord::cost_key(step, t))?;
                    let ours = c.worker.as_deref().map(mac_of) == Some(mac) && e.v.is_none_or(|v| v == c.v);
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

    /// `worker`'s Mac's pace at `step`: 1 on the lead, its pace at the step, else over its steps,
    /// else (no pair of it) the lead's, 1.
    pub fn get(&self, worker: &str, step: &str) -> f64 {
        let mac = mac_of(worker);
        if mac == self.lead {
            return 1.0;
        }
        self.steps.get(mac).and_then(|s| s.get(step)).or_else(|| self.macs.get(mac)).copied().unwrap_or(1.0)
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

    /// Each Mac's pace at a step from the same targets run on it and on the lead (its slots its
    /// own), drawn toward its pace over its steps; a target run on one side alone, or a job of
    /// many, says nothing; a Mac with no pair is the lead's pace.
    #[test]
    fn each_macs_pace_from_the_same_targets() {
        let mut ev = Vec::new();
        for k in 0..30 {
            let t = format!("10/{k}/0");
            ev.push(job(1000 + k, "mini #2", "terrain-deep", &t, 100.0 + k as f64));
            ev.push(job(
                2000 + k,
                if k % 2 == 0 { "m4" } else { "m4 #2" },
                "terrain-deep",
                &t,
                2.0 * (100.0 + k as f64),
            ));
        }
        // (One pack pair at four times; a heavy pack the m4 alone ran; a job of two targets.)
        ev.push(job(1100, "mini", "pack", "6/1/1", 10.0));
        ev.push(job(1200, "m4", "pack", "6/1/1", 40.0));
        ev.push(job(1300, "m4", "pack", "6/9/9", 4000.0));
        ev.push(Event { targets: vec!["6/2/2".into(), "6/3/3".into()], ..job(1400, "m4", "pack", "", 4000.0) });
        ev.push(Event { targets: vec!["6/2/2".into(), "6/3/3".into()], ..job(1500, "mini", "pack", "", 10.0) });
        let p = Paces::of(&ev, "mini", 5000);
        assert_eq!(p.get("mini #4", "pack"), 1.0, "the lead's slots: the lead's");
        // (Its prior: the median of its steps' medians (ln 2, ln 4), drawn to 0 by its 2 steps.)
        let prior = ((2f64.dln() + 4f64.dln()) / 2.0 * 2.0 / 12.0).dexp();
        assert!((p.macs["m4"] - prior).abs() < 1e-3, "{} against {prior}", p.macs["m4"]);
        let td = p.get("m4", "terrain-deep");
        let want = ((30.0 * 2f64.dln() + 10.0 * prior.dln()) / 40.0).dexp();
        assert!((td - want).abs() < 1e-3, "{td} against {want}");
        let pack = p.get("m4 #2", "pack");
        let want = ((4f64.dln() + 10.0 * prior.dln()) / 11.0).dexp();
        assert!((pack - want).abs() < 1e-3, "one pair: near its prior, {pack} against {want}");
        assert_eq!(p.get("m4", "unit"), p.macs["m4"], "a step without a pair: its pace over its steps");
        assert_eq!(p.get("m1", "pack"), 1.0, "no pair: the lead's");
        assert!(p.known() && !Paces::default().known());
        // (Out of the window: nothing.)
        let late = Paces::of(&ev, "mini", 1000 + PACE_S + 2000);
        assert!(late.macs.is_empty() && late.get("m4", "pack") == 1.0);
    }

    /// Two runs compare only when their step ran the same way (`Event::v`): a run of before a
    /// change to the step pairs with none of after it.
    #[test]
    fn runs_of_another_way_of_the_step_dont_pair() {
        let ev = vec![
            job(10, "mini", "depth", "8/1/1", 10.0),
            Event { v: Some(2), ..job(20, "m4", "depth", "8/1/1", 100.0) },
        ];
        assert!(Paces::of(&ev, "mini", 100).macs.is_empty());
        let ev = vec![job(10, "mini", "depth", "8/1/1", 10.0), job(20, "m4", "depth", "8/1/1", 30.0)];
        assert!(Paces::of(&ev, "mini", 100).get("m4", "depth") > 1.0);
    }

    /// Task #927: a Mac's jobs' start at a step is the median of its jobs' time past their
    /// targets' own runs (each its cost record, run on that Mac the way the job ran); a job one of
    /// whose targets' record is another Mac's, or another way's, says nothing of it.
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
        ];
        let p = Paces::default().with_starts(&ev, &costs, 100);
        assert_eq!(p.start("m1 #2", "slope-deep"), 10.0, "of 10, 8 and 10");
        assert_eq!(p.start("m4", "slope-deep"), 0.0, "none measured");
        assert_eq!(p.start("m1", "pack"), 0.0);
        assert!(Paces::default().with_starts(&ev, &costs, 100 + PACE_S + 100).starts.is_empty(), "out of the window");
    }
}
