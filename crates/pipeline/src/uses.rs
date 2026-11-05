//! What each step keeps busy on each Mac (task #947; docs/pool.md §7.1, Threads granted): the cores
//! its jobs kept busy over their whole runs, from the build's timings (`coord/timings.jsonl`, every
//! job's record: its CPU time, wall time and threads), so a job is granted about the threads its
//! step uses there, and the cores it would hold idle go to other jobs.
//!
//! A job holds its threads from its start to its end, so what it uses of them is its CPU time over
//! its wall time (its busy cores), whatever it waited on or how its work came in bursts: slope-deep
//! kept 12-55 % of its threads busy on the M4 (0.8 cores of 3 for five hours), terrain-deep 51-67 %.
//! A Mac's runs of a step are grouped by their threads, each group's busy cores its CPU time summed
//! over its wall time summed (a long job weighs as long as it held its threads). The grant
//! (`Uses::cores`) is the most cores any group kept busy, rounded up to whole threads; where that
//! group needed every thread it had (its busy cores past one fewer) and no run had more, one thread
//! more, to learn whether threads held it: a step bounded by its threads climbs a thread a run, one
//! bounded by its waits or its serial parts settles at what it keeps busy. A step none of whose runs
//! on a Mac is in the window has none here: the cost book's rule decides (crate::needs::Book::cores).
//!
//! The runs read are those in the window the paces read (crate::paces::PACE_S, picked by a held-out
//! test of what a Mac's recent runs say of its next), ended well (a paused run isn't), with their
//! CPU time and threads said.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A run as the uses read it: when it ended (unix s), its step, its Mac, its threads, CPU and wall time.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub end: u64,
    pub step: String,
    pub mac: String,
    pub threads: u32,
    pub cpu_s: f64,
    pub wall_s: f64,
}

/// The fields of a timings record (crate::timings::RunRec) a `Run` is read from: a log's line read
/// for these alone, not its phases.
#[derive(Deserialize)]
struct Line {
    kind: String,
    #[serde(default)]
    host: String,
    start: u64,
    wall_s: f64,
    #[serde(default)]
    cpu_s: Option<f64>,
    #[serde(default)]
    threads: Option<u32>,
    ok: bool,
}

impl Run {
    fn of_line(l: Line) -> Option<Run> {
        let (cpu_s, threads) = (l.cpu_s?, l.threads.filter(|&t| t > 0)?);
        (l.ok && l.wall_s > 0.0 && cpu_s.is_finite() && l.wall_s.is_finite()).then(|| Run {
            end: l.start + l.wall_s as u64,
            step: l.kind,
            mac: crate::paces::mac_of(&l.host).to_string(),
            threads,
            cpu_s,
            wall_s: l.wall_s,
        })
    }

    /// The run a timings record says, where it reads one.
    pub fn of(r: &crate::timings::RunRec) -> Option<Run> {
        Run::of_line(Line {
            kind: r.kind.clone(),
            host: r.host.clone(),
            start: r.start,
            wall_s: r.wall_s,
            cpu_s: r.cpu_s,
            threads: r.threads,
            ok: r.ok,
        })
    }

    /// The runs of a timings log ended in the window at `now`.
    pub fn read_log(p: &std::path::Path, now: u64) -> Vec<Run> {
        let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
        s.lines()
            .filter_map(|l| serde_json::from_str::<Line>(l).ok())
            .filter_map(Run::of_line)
            .filter(|r| in_window(r, now))
            .collect()
    }
}

/// Whether `r` ended in the window at `now`.
pub fn in_window(r: &Run, now: u64) -> bool {
    r.end + crate::paces::PACE_S > now
}

/// A Mac's runs of a step at a thread count: how many, their CPU and wall time summed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Use {
    pub runs: u32,
    pub cpu_s: f64,
    pub wall_s: f64,
}

impl Use {
    /// The cores they kept busy, in tenths, to the nearest (a tenth at least).
    pub fn busy_tenths(&self) -> u32 {
        ((self.cpu_s / self.wall_s.max(f64::MIN_POSITIVE) * 10.0).round() as u32).max(1)
    }
}

/// Each step's use on each Mac: step → Mac → threads → `Use`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Uses {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, BTreeMap<String, BTreeMap<u32, Use>>>,
}

impl Uses {
    /// The uses `runs` say.
    pub fn of<'a>(runs: impl IntoIterator<Item = &'a Run>) -> Uses {
        let mut u = Uses::default();
        for r in runs {
            let e = u.steps.entry(r.step.clone()).or_default().entry(r.mac.clone()).or_default();
            let p = e.entry(r.threads).or_default();
            p.runs += 1;
            p.cpu_s += r.cpu_s;
            p.wall_s += r.wall_s;
        }
        u
    }

    /// The cores a job of `step` on `mac` is granted and counts for, by what its runs there kept busy
    /// (the module's rule): None where none of them is known.
    pub fn cores(&self, step: &str, mac: &str) -> Option<crate::needs::Cores> {
        let points = self.steps.get(step)?.get(mac)?;
        let top = *points.keys().next_back()?;
        // (The busiest group, the fewest threads between equals.)
        let (t, busy) =
            points.iter().map(|(&t, u)| (t, u.busy_tenths())).max_by_key(|&(t, b)| (b, std::cmp::Reverse(t)))?;
        let need = busy.div_ceil(10).max(1);
        let climb = need >= t && t == top;
        let grant = need + u32::from(climb);
        // (A climb counts for its thread besides: nothing measured it idle yet.)
        let busy = if climb { busy + 10 } else { busy };
        Some(crate::needs::Cores { grant, busy_tenths: Some(busy.clamp(1, grant * 10)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(step: &str, host: &str, threads: u32, cpu_s: f64, wall_s: f64) -> Run {
        Run { end: 1000, step: step.into(), mac: crate::paces::mac_of(host).into(), threads, cpu_s, wall_s }
    }

    fn cores(grant: u32, busy: u32) -> Option<crate::needs::Cores> {
        Some(crate::needs::Cores { grant, busy_tenths: Some(busy) })
    }

    /// A step keeping a share of its threads busy is granted what it keeps busy, rounded up: the M4's
    /// slope-deep at 14 threads kept 4.7 busy, so 5; at 3 threads it kept 0.8 busy for five hours,
    /// which alone grants it 1. Grouped by threads, each group's CPU over its wall summed: a long run
    /// weighs as long as it held its threads.
    #[test]
    fn a_step_low_in_use_is_granted_fewer_threads() {
        let u = Uses::of(&[run("slope-deep", "m4 #2", 14, 470.0, 100.0)]);
        assert_eq!(u.cores("slope-deep", "m4"), cores(5, 47));
        let u = Uses::of(&[run("slope-deep", "m4", 3, 0.8 * 18_000.0, 18_000.0), run("slope-deep", "m4", 3, 3.0, 1.0)]);
        // (Over the two: 14403 s of CPU over 18001 s: 0.8 busy.)
        assert_eq!(u.cores("slope-deep", "m4"), cores(1, 8));
        // Another Mac's runs say nothing of this one's; another step's neither.
        assert_eq!(u.cores("slope-deep", "m1"), None);
        assert_eq!(u.cores("terrain-deep", "m4"), None);
    }

    /// A group needing every thread it had climbs a thread, where no run had more: a step its
    /// threads held learns what more would keep busy; one that settled under its threads doesn't.
    #[test]
    fn a_step_using_every_thread_climbs_one() {
        let u = Uses::of(&[run("slope-hi", "m4", 3, 290.0, 100.0)]);
        assert_eq!(u.cores("slope-hi", "m4"), cores(4, 39));
        // Measured at 4, 2.8 busy: no busier than at 3, so 3, no climb (a run had more threads).
        let u = Uses::of(&[run("slope-hi", "m4", 3, 290.0, 100.0), run("slope-hi", "m4", 4, 280.0, 100.0)]);
        assert_eq!(u.cores("slope-hi", "m4"), cores(3, 29));
        // A one-thread step learns whether a second helps once, and settles where it showed none.
        let u = Uses::of(&[run("unit", "m4", 1, 80.0, 100.0)]);
        assert_eq!(u.cores("unit", "m4"), cores(2, 18));
        let u = Uses::of(&[run("unit", "m4", 1, 80.0, 100.0), run("unit", "m4", 2, 85.0, 100.0)]);
        assert_eq!(u.cores("unit", "m4"), cores(1, 9));
    }

    /// A timings log's runs: ended well, with CPU and threads said, in the window; a slot's worker
    /// read as its Mac.
    #[test]
    fn a_log_is_read_for_its_runs_in_the_window() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("timings.jsonl");
        let now = 10 * crate::paces::PACE_S;
        let rec = |kind: &str, host: &str, start: u64, cpu: Option<f64>, ok: bool| crate::timings::RunRec {
            v: 1,
            kind: kind.into(),
            host: host.into(),
            start,
            wall_s: 100.0,
            cpu_s: cpu,
            threads: Some(4),
            ok,
            ..Default::default()
        };
        for r in [
            rec("terrain-deep", "m4 #3", now - 200, Some(200.0), true),
            rec("terrain-deep", "m4", now - 2 * crate::paces::PACE_S, Some(400.0), true),
            rec("terrain-deep", "m4", now - 200, None, true),
            rec("terrain-deep", "m4", now - 200, Some(400.0), false),
        ] {
            crate::timings::append(&p, &r).unwrap();
        }
        let runs = Run::read_log(&p, now);
        assert_eq!(runs, vec![run_at("terrain-deep", "m4", 4, 200.0, now - 100)]);
        assert_eq!(Run::of(&rec("terrain-deep", "m4 #3", now - 200, Some(200.0), true)), Some(runs[0].clone()));
    }

    fn run_at(step: &str, mac: &str, threads: u32, cpu_s: f64, end: u64) -> Run {
        Run { end, step: step.into(), mac: mac.into(), threads, cpu_s, wall_s: 100.0 }
    }
}
