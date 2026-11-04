//! Each Mac's pace at each step (task #926; docs/pool.md §7.7, One model per step): how long its
//! jobs of the step take a target against the lead's, from the build's history. The cost book
//! (crate::needs::Book) puts every run's time and NAS rate at the lead's pace by it before a step's
//! model is made of them, and a prediction for a Mac is the model at that Mac's pace; the forecast
//! (crate::agent::forecast, code 4) puts its costs at the lead's pace and runs each lane at its
//! Mac's pace at each step the same way.
//!
//! Measured, never set: over the jobs done well in the `PACE_S` before now, each Mac's mean log of
//! its seconds a target at a step less the lead's, the lead having run `LEAD_JOBS` of it at least.
//! A Mac's pace over every step it shares with the lead is those weighed by its jobs, and a step's
//! is drawn toward it by `PRIOR_JOBS` (`n` jobs of its own: `(n·step + PRIOR_JOBS·overall) / (n +
//! PRIOR_JOBS)`), its overall toward the lead's (1) the same way: a Mac of few jobs at a step is
//! taken at its pace elsewhere, one of few jobs at all near the lead's. A Mac the history has no
//! job of beside the lead's is not said (its caller's own fallback).
//!
//! Why the history's jobs and not the cost book's targets: the book keeps one cost a target (its
//! last run's), so two Macs' costs are of different targets, and which Mac had a step's dense ones
//! swamps their pace (on 4 Nov terrain-deep's book had the M4 at 9.3 times the mini's mean, its
//! same targets in the history at 1.4). The history's times a target, whole jobs at a time, came to
//! within 1.3 times of what the same target took on each Mac over the week (nine steps and Macs);
//! the book's within 1.7–1.8.

use crate::coord::history::Event;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The history a pace is read from (s).
pub const PACE_S: u64 = 3 * 86_400;
/// The lead's jobs of a step a pace at it needs.
pub const LEAD_JOBS: usize = 3;
/// How many jobs' weight a Mac's pace over its other steps (or the lead's, for its overall) has
/// against its own jobs at a step.
pub const PRIOR_JOBS: f64 = 3.0;

/// The Mac a worker is on: a slot's worker (`<Mac> #k`) and an old second slot's
/// (`<Mac> (second job)`) its Mac's.
pub fn mac_of(worker: &str) -> &str {
    let w = worker.split(" #").next().unwrap_or(worker);
    w.strip_suffix(" (second job)").unwrap_or(w)
}

/// Each Mac's pace against the lead's (`of`): its time a target over the lead's (2: half its speed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Paces {
    /// The lead's Mac: 1 at every step.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lead: String,
    /// Mac → step → its pace at the step.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, BTreeMap<String, f64>>,
    /// Mac → its pace over every step it and the lead ran.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub macs: BTreeMap<String, f64>,
}

impl Paces {
    /// The paces at `now` of every Mac against `lead`'s (a worker of it, or its Mac), from `events`.
    pub fn of(events: &[Event], lead: &str, now: u64) -> Paces {
        let lead = mac_of(lead);
        let from = now.saturating_sub(PACE_S);
        // ((Mac, step) → (the sum of the logs of its seconds a target, its jobs).)
        let mut per: BTreeMap<(&str, &str), (f64, usize)> = BTreeMap::new();
        for e in events.iter().filter(|e| e.t > from && e.t <= now && e.ok == Some(true)) {
            let (Some(w), Some(step), Some(secs)) = (e.worker.as_deref(), e.step.as_deref(), e.secs) else { continue };
            if !matches!(e.kind.as_str(), "done" | "end") || e.targets.is_empty() || !secs.is_finite() {
                continue;
            }
            let p = per.entry((mac_of(w), step)).or_default();
            (p.0, p.1) = (p.0 + (secs / e.targets.len() as f64).max(1.0).ln(), p.1 + 1);
        }
        let mean = |p: &(f64, usize)| p.0 / p.1 as f64;
        // (Mac → (step, its log pace there, its jobs).)
        let mut by: BTreeMap<&str, Vec<(&str, f64, usize)>> = BTreeMap::new();
        for (&(mac, step), p) in per.iter().filter(|((m, _), _)| *m != lead) {
            let Some(l) = per.get(&(lead, step)).filter(|l| l.1 >= LEAD_JOBS) else { continue };
            by.entry(mac).or_default().push((step, mean(p) - mean(l), p.1));
        }
        let mut out = Paces { lead: lead.to_string(), ..Default::default() };
        for (mac, steps) in by {
            let n: f64 = steps.iter().map(|s| s.2 as f64).sum();
            let overall = steps.iter().map(|s| s.1 * s.2 as f64).sum::<f64>() / (n + PRIOR_JOBS);
            out.macs.insert(mac.to_string(), round(overall.exp()));
            let each = steps.iter().map(|&(s, l, k)| {
                (s.to_string(), round(((k as f64 * l + PRIOR_JOBS * overall) / (k as f64 + PRIOR_JOBS)).exp()))
            });
            out.steps.insert(mac.to_string(), each.collect());
        }
        out
    }

    /// `worker`'s Mac's pace at `step`: 1 on the lead, its pace at the step, else over its steps;
    /// None when its Mac isn't measured (or there's no lead: none were worked out).
    pub fn get(&self, worker: &str, step: &str) -> Option<f64> {
        let mac = mac_of(worker);
        if self.lead.is_empty() {
            return None;
        }
        if mac == self.lead {
            return Some(1.0);
        }
        self.steps.get(mac).and_then(|s| s.get(step)).or_else(|| self.macs.get(mac)).copied()
    }

    /// A time measured on `worker` (None: not said) of `step`, put at the lead's pace (`secs` as it
    /// is where its Mac's pace isn't known).
    pub fn at_lead(&self, worker: Option<&str>, step: &str, secs: f64) -> f64 {
        secs / worker.and_then(|w| self.get(w, step)).unwrap_or(1.0)
    }
}

/// To the thousandth: what's kept and compared is a measure, not its last bits.
fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(t: u64, worker: &str, step: &str, targets: usize, secs: f64) -> Event {
        Event {
            t,
            worker: Some(worker.into()),
            step: Some(step.into()),
            targets: (0..targets).map(|k| format!("10/{k}/0")).collect(),
            secs: Some(secs),
            ok: Some(true),
            ..Event::new("done")
        }
    }

    #[test]
    fn a_mac_of_a_slot() {
        assert_eq!(mac_of("Mac-mini-de-Brandon #3"), "Mac-mini-de-Brandon");
        assert_eq!(mac_of("m1 (second job)"), "m1");
        assert_eq!(mac_of("m4"), "m4");
    }

    /// Each Mac's time a target against the lead's, a step at a time; its slots' jobs its own; a
    /// step of few jobs drawn to its pace over its steps.
    #[test]
    fn each_macs_pace_at_each_step() {
        let mut ev = Vec::new();
        for k in 0..20 {
            ev.push(job(1000 + k, "mini", "terrain-deep", 1, 100.0));
            ev.push(job(1000 + k, "mini #2", "pack", 2, 20.0));
            // (Twice the lead's time a target at terrain-deep, on its two slots.)
            ev.push(job(1000 + k, if k % 2 == 0 { "m4" } else { "m4 #2" }, "terrain-deep", 2, 400.0));
        }
        // (One pack job, at four times the lead's.)
        ev.push(job(1100, "m4", "pack", 1, 40.0));
        let p = Paces::of(&ev, "mini #2", 2000);
        assert_eq!(p.get("mini #4", "pack"), Some(1.0), "the lead's slots: the lead's");
        let td = p.get("m4", "terrain-deep").unwrap();
        assert!((td - 2.0).abs() < 0.15, "twice the lead's, drawn a little to its overall: {td}");
        let pack = p.get("m4 #2", "pack").unwrap();
        assert!(pack < 4.0 && pack > td, "one job: toward its overall, {pack}");
        let other = p.get("m4", "unit").unwrap();
        assert_eq!(other, p.macs["m4"], "a step it never ran: its overall");
        assert_eq!(p.get("m1", "pack"), None, "never measured");
        // (Before the window, or failed: nothing.)
        assert_eq!(Paces::of(&ev, "mini", 1000 + PACE_S + 2000).get("m4", "pack"), None);
        assert_eq!(p.at_lead(Some("m4"), "unit", 100.0), 100.0 / other);
        assert_eq!(p.at_lead(None, "unit", 100.0), 100.0);
    }

    /// A step the lead ran too little of says nothing of a Mac's pace at it.
    #[test]
    fn the_lead_must_have_run_the_step() {
        let mut ev = vec![job(10, "mini", "depth", 1, 10.0), job(11, "mini", "depth", 1, 10.0)];
        ev.extend((0..5).map(|k| job(20 + k, "m4", "depth", 1, 30.0)));
        let p = Paces::of(&ev, "mini", 100);
        assert_eq!(p.get("m4", "depth"), None);
        ev.push(job(12, "mini", "depth", 1, 10.0));
        let p = Paces::of(&ev, "mini", 100);
        assert!(p.get("m4", "depth").unwrap() > 2.0);
    }
}
