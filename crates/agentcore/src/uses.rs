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
//! group needed every thread it had (its busy cores past one fewer, and half its threads at least)
//! and no run had more, one thread more, to learn whether threads held it: a step bounded by its
//! threads climbs a thread a run, one bounded by its serial parts settles at what it keeps busy.
//!
//! A cut is a trial, not a ratchet. A step whose threads each wait (slope-deep kept about 0.3 of a
//! core busy a thread on the M4, at 3 threads and at 14 alike) keeps fewer cores busy at fewer
//! threads, so its use alone would cut it run by run, never needing every thread to climb back
//! (14, 5, 2, 1 at 0.32 a thread). So where a run at the grant or fewer threads kept less than a
//! core below the busiest group, the cut stands; where it kept more than a core less, those threads
//! paid, and the step holds the fewest threads that kept within a core of the busiest. A run at
//! more threads than the cut says nothing of it (its use is its batches' as much as its threads').
//!
//! **The phases' floor** (task #948). A run's whole CPU over its whole wall hides its parallel
//! phases: pack draws a few seconds on 3-4 cores (its tiles drawn) inside half an hour of reading
//! base packs; terrain-lo's sampling keeps 3-5 threads waiting on GLO-30's reads at once (1556 s
//! of them in a 533 s run, on threads beside its main one) while its CPU says 1. Cut to its whole
//! run's use, such a step runs those phases on fewer threads, longer: terrain-lo at 1 thread took
//! 2063 s where at 3 and 6 it took 413-668. So a step is granted at least its floor (`Uses::floors`):
//! the fewest threads at which its runs, together, are predicted to take no more than `WALL_SLACK`
//! longer than they did. Its runs on every Mac count: the threads a phase keeps are the step's
//! (its readers, its batches), and a Mac whose runs were all cut low can't show them: on the lead's
//! timings to 4 Nov the M4's one terrain-lo run in the window was the 1-thread one, whose phases keep
//! a thread, so a floor of the M4's runs alone held it at 1 (task #948 review). Each top-level phase
//! of a run's main thread keeps busy its CPU over its wall, and one overlapped by phases on other
//! threads also as many threads as those phases' wall summed over the overlapped phases' (the threads
//! waiting beside it); at fewer threads than a phase keeps it takes that many over them as long
//! (`kept`). Summed over the runs, a long run weighs as long as it took, and a rare short burst
//! doesn't hold a step's threads. A job's wall time doesn't blow up to save cores: from the 6-thread
//! run, terrain-lo at 1 thread is predicted 2151 s (it took 2063). On the lead's timings (the
//! mini's, to 4 Nov) the floor holds pack at 3 on every Mac where its use cut it to 1, slope-hi at 5
//! (its use 2 on the mini, 4 on the M4 and the M1), terrain-lo on the M4 at 2 (1), and leaves
//! slope-deep, terrain-hi and terrain-deep at their use (docs/pool.md §7.1).
//!
//! A run whose targets ran on their job's grant as each began (crate::control::on_grant) says what
//! each thread count held (`timings::RunRec::held`): its use is read at each (`Run::of`).
//!
//! A step none of whose runs on a Mac is in the window has none here: the cost book's rule decides
//! (crate::needs::Book::cores).
//!
//! The runs read are those in the window the paces read (crate::paces::PACE_S, picked by a held-out
//! test of what a Mac's recent runs say of its next), ended well (a paused run isn't), with their
//! CPU time and threads said, and of the way their step runs now: a record's cost version (the
//! lead's stamp, `RunRec::cost_v`; none before the stamp, read as 0) at least the step's
//! (crate::coord::cost_version), as the cost book reads its costs. A step whose work changed
//! wholly (task #929's terrain, #952's slope-deep and slope-hi) is granted by what it does now, not
//! by the busy cores of its old way.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How much longer than they took a step's runs on a Mac may be predicted to take, together, at
/// fewer threads before those threads count as its floor (`Uses::floors`): a choice, about the
/// spread of a step's runs' times on one Mac, not a measure of a step.
pub const WALL_SLACK: f64 = 0.05;

/// A run as the uses read it: when it ended (unix s), its step, its Mac, its threads, CPU and wall
/// time (a part of it, where its threads changed: `Run::of`); and, on one part of each run, its
/// whole wall time and its main thread's top-level phases, each its wall time and the threads it
/// keeps (`phases`, the module's floor).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub end: u64,
    pub step: String,
    pub mac: String,
    pub threads: u32,
    pub cpu_s: f64,
    pub wall_s: f64,
    pub whole_s: f64,
    pub phases: Vec<(f64, f64)>,
}

/// A top-level phase as the floor reads it (crate::timings::PhaseRec's fields).
#[derive(Clone, Debug, Default, Deserialize)]
struct Phase {
    wall_s: f64,
    #[serde(default)]
    cpu_s: Option<f64>,
    #[serde(default)]
    overlapped: bool,
    #[serde(default)]
    background: bool,
}

/// A run's main-thread top-level phases, each its wall time and the threads it keeps (the module's
/// floor): its CPU over its wall, and an overlapped one also the threads its run's background
/// phases' wall summed over the overlapped phases' keeps. None where no phase says its CPU.
fn kept(phases: &[Phase]) -> Vec<(f64, f64)> {
    let main: Vec<&Phase> = phases.iter().filter(|p| !p.background && p.wall_s > 0.0).collect();
    if !main.iter().any(|p| p.cpu_s.is_some()) {
        return Vec::new();
    }
    let beside: f64 = phases.iter().filter(|p| p.background).map(|p| p.wall_s).sum();
    let over: f64 = main.iter().filter(|p| p.overlapped).map(|p| p.wall_s).sum();
    main.iter()
        .map(|p| {
            let cpu = p.cpu_s.unwrap_or(0.0) / p.wall_s;
            (p.wall_s, if p.overlapped && over > 0.0 { cpu.max(beside / over) } else { cpu })
        })
        .collect()
}

/// The most threads a floor says (past any Mac's cores: the grant is fitted to them anyway).
const MOST_FLOOR: u32 = 64;

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
    #[serde(default)]
    held: Vec<crate::timings::Held>,
    #[serde(default)]
    phases: Vec<Phase>,
    ok: bool,
    #[serde(default)]
    cost_v: Option<u32>,
}

impl Run {
    /// A record's runs: one at its threads, else one at each thread count its targets held
    /// (`Line::held`) and the rest of it at the threads it started on; each with the run's floor.
    fn of_line(the_plan: &dyn crate::agent::seam::Plan, l: Line) -> Vec<Run> {
        let (Some(cpu_s), Some(threads)) = (l.cpu_s, l.threads.filter(|&t| t > 0)) else { return Vec::new() };
        if !(l.ok && l.wall_s > 0.0 && cpu_s.is_finite() && l.wall_s.is_finite()) {
            return Vec::new();
        }
        // (Of an older way of its step: its use says nothing of the way it runs now.)
        if l.cost_v.unwrap_or(0) < the_plan.cost_version(&l.kind) {
            return Vec::new();
        }
        let part = |threads: u32, cpu_s: f64, wall_s: f64| Run {
            end: l.start + l.wall_s as u64,
            step: l.kind.clone(),
            mac: crate::paces::mac_of(&l.host).to_string(),
            threads,
            cpu_s,
            wall_s,
            ..Default::default()
        };
        let held: Vec<&crate::timings::Held> = l.held.iter().filter(|h| h.threads > 0 && h.wall_s > 0.0).collect();
        let (wall, cpu) = held.iter().fold((l.wall_s, cpu_s), |(w, c), h| (w - h.wall_s, c - h.cpu_s));
        let mut runs: Vec<Run> = held.iter().map(|h| part(h.threads, h.cpu_s, h.wall_s)).collect();
        if wall > 0.0 {
            runs.push(part(threads, cpu.max(0.0), wall));
        }
        let phases = kept(&l.phases);
        if let Some(r) = runs.first_mut().filter(|_| !phases.is_empty()) {
            (r.whole_s, r.phases) = (l.wall_s, phases);
        }
        runs
    }

    /// The runs a timings record says.
    pub fn of(the_plan: &dyn crate::agent::seam::Plan, r: &crate::timings::RunRec) -> Vec<Run> {
        let phases = r
            .phases
            .iter()
            .map(|p| Phase { wall_s: p.wall_s, cpu_s: p.cpu_s, overlapped: p.overlapped, background: p.background })
            .collect();
        Run::of_line(
            the_plan,
            Line {
                kind: r.kind.clone(),
                host: r.host.clone(),
                start: r.start,
                wall_s: r.wall_s,
                cpu_s: r.cpu_s,
                threads: r.threads,
                held: r.held.clone(),
                phases,
                ok: r.ok,
                cost_v: r.cost_v,
            },
        )
    }

    /// The runs of a timings log ended in the window at `now`.
    pub fn read_log(the_plan: &dyn crate::agent::seam::Plan, p: &std::path::Path, now: u64) -> Vec<Run> {
        let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
        s.lines()
            .filter_map(|l| serde_json::from_str::<Line>(l).ok())
            .flat_map(|l| Run::of_line(the_plan, l))
            .filter(|r| in_window(r, now))
            .collect()
    }
}

/// Whether `r` ended in the window at `now`.
pub fn in_window(r: &Run, now: u64) -> bool {
    r.end + crate::paces::PACE_S > now
}

/// A Mac's runs of a step at a thread count: their CPU and wall time summed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Use {
    pub cpu_s: f64,
    pub wall_s: f64,
}

impl Use {
    /// The cores they kept busy, in tenths, to the nearest (a tenth at least).
    pub fn busy_tenths(&self) -> u32 {
        ((self.cpu_s / self.wall_s.max(f64::MIN_POSITIVE) * 10.0).round() as u32).max(1)
    }
}

/// Each step's use on each Mac: step → Mac → threads → `Use`; and its floor (`floors`, step →
/// threads: the module's), the fewest threads at which its runs' phases on every Mac, each kept to
/// that many threads, are predicted to take their runs together no more than `WALL_SLACK` longer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Uses {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, BTreeMap<String, BTreeMap<u32, Use>>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub floors: BTreeMap<String, u32>,
}

impl Uses {
    /// The uses `runs` say.
    pub fn of<'a>(runs: impl IntoIterator<Item = &'a Run>) -> Uses {
        let runs: Vec<&Run> = runs.into_iter().collect();
        let mut u = Uses::default();
        for r in &runs {
            let e = u.steps.entry(r.step.clone()).or_default().entry(r.mac.clone()).or_default();
            let p = e.entry(r.threads).or_default();
            p.cpu_s += r.cpu_s;
            p.wall_s += r.wall_s;
        }
        // (Each step's runs with their phases said, on every Mac: their wall summed, and what fewer
        // threads would add to it, each phase keeping more than `t` taking that many over `t` as
        // long.)
        let mut by: BTreeMap<&str, Vec<&Run>> = BTreeMap::new();
        for r in runs.iter().filter(|r| !r.phases.is_empty()) {
            by.entry(r.step.as_str()).or_default().push(r);
        }
        for (step, rs) in by {
            let whole: f64 = rs.iter().map(|r| r.whole_s).sum();
            let more =
                |t: f64| -> f64 { rs.iter().flat_map(|r| &r.phases).map(|&(w, k)| w * (k / t - 1.0).max(0.0)).sum() };
            let f = (1..=MOST_FLOOR).find(|&t| more(f64::from(t)) <= whole * WALL_SLACK).unwrap_or(MOST_FLOOR);
            u.floors.insert(step.to_string(), f);
        }
        u
    }

    /// The cores a job of `step` on `mac` is granted and counts for, by what its runs there kept busy
    /// (the module's rule), never fewer threads than their phases' floor: None where none of them is
    /// known.
    pub fn cores(&self, step: &str, mac: &str) -> Option<crate::needs::Cores> {
        let c = self.by_use(step, mac)?;
        let floor = self.floors.get(step).copied().unwrap_or(0);
        Some(crate::needs::Cores { grant: c.grant.max(floor), ..c })
    }

    /// `cores` by the runs' whole use alone (`floors` aside).
    pub fn by_use(&self, step: &str, mac: &str) -> Option<crate::needs::Cores> {
        let busy: BTreeMap<u32, u32> =
            self.steps.get(step)?.get(mac)?.iter().map(|(&t, u)| (t, u.busy_tenths())).collect();
        let top = *busy.keys().next_back()?;
        // (The busiest group, the fewest threads between equals.)
        let (&t, &most) = busy.iter().max_by_key(|&(&t, &b)| (b, std::cmp::Reverse(t)))?;
        let cut = most.div_ceil(10).max(1);
        // (Every thread: past one fewer, and half of them at least: one thread kept a tenth busy
        // didn't need it.)
        let climb = t == top && most > (t - 1) * 10 && 2 * most >= t * 10;
        let grant = cut + u32::from(climb);
        // (The fewest threads that kept within a core of the busiest. Where a run at the grant or
        // fewer threads kept less, its threads paid: they hold, and the cut doesn't stand.)
        let (&held, &held_busy) = busy.iter().find(|&(_, &b)| b + 10 >= most)?;
        if held > grant && busy.keys().any(|&u| u <= grant) {
            return Some(crate::needs::Cores { grant: held, busy_tenths: Some(held_busy.clamp(1, held * 10)) });
        }
        // (A climb counts for its thread besides: nothing measured it idle yet.)
        let busy = if climb { most + 10 } else { most };
        Some(crate::needs::Cores { grant, busy_tenths: Some(busy.clamp(1, grant * 10)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(step: &str, host: &str, threads: u32, cpu_s: f64, wall_s: f64) -> Run {
        Run {
            end: 1000,
            step: step.into(),
            mac: crate::paces::mac_of(host).into(),
            threads,
            cpu_s,
            wall_s,
            ..Default::default()
        }
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
        // One thread kept a tenth busy didn't need it (the lead's backup and catalog runs, 0.1-0.4 of
        // their one thread, had climbed to 2).
        let u = Uses::of(&[run("catalog", "m4", 1, 40.0, 100.0)]);
        assert_eq!(u.cores("catalog", "m4"), cores(1, 4));
    }

    /// A cut is a trial: where the fewer threads kept more than a core less busy, the threads paid
    /// and hold; where they kept within a core, the cut stands; a run at more threads than the cut
    /// says nothing of it.
    #[test]
    fn a_cut_stands_only_where_fewer_threads_kept_as_busy() {
        // 0.32 of a core a thread, at 14 and at 5: the cut to 5 lost 2.9 cores, so 14.
        let paid = [run("slope-deep", "m4", 14, 450.0, 100.0), run("slope-deep", "m4", 5, 160.0, 100.0)];
        assert_eq!(Uses::of(&paid).cores("slope-deep", "m4"), cores(14, 45));
        // Serial half its time: 1.9 at 14, 1.3 at 2: the cut stands.
        let serial = [run("pack", "m4", 14, 190.0, 100.0), run("pack", "m4", 2, 130.0, 100.0)];
        assert_eq!(Uses::of(&serial).cores("pack", "m4"), cores(2, 19));
        // The m6's slope-hi: 0.7 at 7 threads, 2.0 at 8, 1.1 at 9 (its batches' as much as its
        // threads'): no run at the cut's 2 or fewer, so it's tried.
        let untried = [
            run("slope-hi", "m6", 7, 70.0, 100.0),
            run("slope-hi", "m6", 8, 200.0, 100.0),
            run("slope-hi", "m6", 9, 110.0, 100.0),
        ];
        assert_eq!(Uses::of(&untried).cores("slope-hi", "m6"), cores(2, 20));
    }

    /// Round after round, each job run at the threads the rule grants and its run added: a step
    /// whose threads each keep 0.32 of a core busy (a wait each) doesn't ratchet down past what
    /// its runs showed paid (by use alone it went 14, 5, 2, 1), and one that is serial half its
    /// time settles at 2.
    #[test]
    fn a_step_whose_threads_pay_doesnt_ratchet_down() {
        let rounds = |busy: &dyn Fn(u32) -> f64| {
            let mut runs = vec![run("s", "m4", 14, busy(14) * 100.0, 100.0)];
            (0..8)
                .map(|_| {
                    let t = Uses::of(&runs).cores("s", "m4").unwrap().grant.min(14);
                    runs.push(run("s", "m4", t, busy(t) * 100.0, 100.0));
                    t
                })
                .collect::<Vec<u32>>()
        };
        assert_eq!(rounds(&|t| 0.32 * f64::from(t)), [5, 14, 14, 14, 14, 14, 14, 14]);
        assert_eq!(rounds(&|t| 1.0 / (0.5 + 0.5 / f64::from(t))), [2, 2, 2, 2, 2, 2, 2, 2]);
    }

    /// A phase as recorded: its wall, CPU, and on another thread (`background`) or overlapped by one.
    fn phase(wall_s: f64, cpu_s: f64, overlapped: bool, background: bool) -> crate::timings::PhaseRec {
        crate::timings::PhaseRec { wall_s, cpu_s: Some(cpu_s), overlapped, background, n: 1, ..Default::default() }
    }

    fn timed(step: &str, threads: u32, wall_s: f64, cpu_s: f64, phases: Vec<crate::timings::PhaseRec>) -> Vec<Run> {
        Run::of(
            crate::agent::seam::for_tests(),
            &crate::timings::RunRec {
                v: 1,
                kind: step.into(),
                host: "m4".into(),
                start: 1000,
                wall_s,
                cpu_s: Some(cpu_s),
                threads: Some(threads),
                ok: true,
                phases,
                cost_v: Some(crate::coord::cost_version(step)),
                ..Default::default()
            },
        )
    }

    /// Task #948: the phases' floor. pack on the M4 (a run of 4 Nov): half an hour reading base packs
    /// at a twentieth of a core, its tiles drawn at 3.6 cores for 52 s: its whole use cuts it to 1,
    /// where its run would take 7 % longer; at 2, 2 %: 2. terrain-lo's 6-thread run (533 s): its z3
    /// tile's levels 409.5 s overlapped by 2028 s of sampling on other threads (5 at once), its CPU
    /// 1.2: at 1 thread predicted 2151 s (a 1-thread run took 2063), held at 5. Two 3-thread runs
    /// keeping 1.8 and 2.2 threads in their overlapped phases (a 668 s one's 648 s overlapped by
    /// 1180 s, 413 s's 365 s by 785 s): 2; with the 6-thread run, its 5 at once for 410 s weigh: 5.
    /// A rare short burst among long runs holds nothing.
    #[test]
    fn a_step_isnt_cut_below_what_its_parallel_phases_keep() {
        let pack = timed(
            "pack",
            14,
            1936.0,
            296.0,
            vec![
                phase(1830.6, 73.0, false, false),
                phase(51.7, 188.5, false, false),
                phase(12.5, 21.7, false, false),
                phase(41.2, 12.8, false, false),
            ],
        );
        let u = Uses::of(&pack);
        assert_eq!(u.by_use("pack", "m4").unwrap().grant, 1, "its whole use");
        assert_eq!(u.cores("pack", "m4").map(|c| c.grant), Some(2));
        let lo6 = timed(
            "terrain-lo",
            6,
            533.0,
            512.0,
            vec![
                phase(36.2, 2.0, false, false),
                phase(409.5, 508.0, true, false),
                phase(1555.9, 1308.9, true, true),
                phase(337.5, 284.3, true, true),
                phase(134.5, 282.8, true, true),
                phase(18.7, 0.7, false, false),
            ],
        );
        let more: f64 = lo6[0].phases.iter().map(|&(w, k)| w * (k - 1.0).max(0.0)).sum();
        assert_eq!((533.0 + more).round(), 2151.0, "at 1 thread, from its 6-thread run");
        assert_eq!(Uses::of(&lo6).cores("terrain-lo", "m4").map(|c| c.grant), Some(5));
        let lo3a = timed(
            "terrain-lo",
            3,
            668.0,
            355.0,
            vec![
                phase(4.7, 1.1, false, false),
                phase(647.8, 349.0, true, false),
                phase(1180.0, 650.0, true, true),
                phase(2.0, 0.5, false, false),
            ],
        );
        let lo3b = timed(
            "terrain-lo",
            3,
            413.0,
            445.0,
            vec![
                phase(20.3, 1.9, false, false),
                phase(365.2, 434.2, true, false),
                phase(785.0, 830.0, true, true),
                phase(1.2, 0.9, false, false),
            ],
        );
        let three: Vec<Run> = [lo3a.clone(), lo3b.clone()].concat();
        assert_eq!(Uses::of(&three).floors["terrain-lo"], 2);
        let all: Vec<Run> = [lo6.clone(), lo3a, lo3b].concat();
        assert_eq!(Uses::of(&all).floors["terrain-lo"], 5, "the 6-thread run's 5 at once weigh");
        // (Every Mac's runs count (review): a Mac whose one run was cut to 1 thread, its phases
        // keeping that one, isn't held there by it; the mini's 6-thread run shows what they keep.)
        let lo1 = timed("terrain-lo", 1, 2063.0, 400.0, vec![phase(2063.0, 400.0, false, false)]);
        let alone = Uses::of(&lo1);
        assert_eq!(alone.cores("terrain-lo", "m4").map(|c| c.grant), Some(1));
        let on_mini: Vec<Run> = lo6.into_iter().map(|r| Run { mac: "m6".into(), ..r }).collect();
        let both = Uses::of(lo1.iter().chain(&on_mini));
        assert_eq!(both.by_use("terrain-lo", "m4").map(|c| c.grant), Some(1));
        assert_eq!(both.cores("terrain-lo", "m4").map(|c| c.grant), Some(4));
        // A 46 s burst at 7 cores among ten half-hour runs at a twentieth of one: no floor past 1.
        let mut runs: Vec<Run> =
            (0..10).flat_map(|_| timed("pack", 2, 1800.0, 90.0, vec![phase(1800.0, 90.0, false, false)])).collect();
        runs.extend(timed("pack", 14, 46.0, 322.0, vec![phase(46.0, 322.0, false, false)]));
        assert_eq!(Uses::of(&runs).floors["pack"], 1);
    }

    /// Task #948: a run whose targets ran on their job's grant (`RunRec::held`) is read at each thread
    /// count it held, the rest at the threads it started on; its phases once.
    #[test]
    fn a_run_is_read_at_each_thread_count_it_held() {
        let rec = crate::timings::RunRec {
            v: 1,
            kind: "slope-deep".into(),
            host: "m4 #2".into(),
            start: 1000,
            wall_s: 100.0,
            cpu_s: Some(150.0),
            threads: Some(8),
            held: vec![
                crate::timings::Held { threads: 8, wall_s: 30.0, cpu_s: 30.0 },
                crate::timings::Held { threads: 2, wall_s: 60.0, cpu_s: 110.0 },
            ],
            ok: true,
            phases: vec![phase(100.0, 150.0, false, false)],
            cost_v: Some(crate::coord::cost_version("slope-deep")),
            ..Default::default()
        };
        let runs = Run::of(crate::agent::seam::for_tests(), &rec);
        let parts: Vec<(u32, f64, f64)> = runs.iter().map(|r| (r.threads, r.wall_s, r.cpu_s)).collect();
        assert_eq!(parts, [(8, 30.0, 30.0), (2, 60.0, 110.0), (8, 10.0, 10.0)]);
        assert_eq!(runs.iter().filter(|r| !r.phases.is_empty()).count(), 1);
        let u = Uses::of(&runs);
        assert_eq!(u.steps["slope-deep"]["m4"][&8], Use { cpu_s: 40.0, wall_s: 40.0 });
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
            cost_v: Some(crate::coord::cost_version(kind)),
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
        let runs = Run::read_log(crate::agent::seam::for_tests(), &p, now);
        assert_eq!(runs, vec![run_at("terrain-deep", "m4", 4, 200.0, now - 100)]);
        assert_eq!(
            Run::of(crate::agent::seam::for_tests(), &rec("terrain-deep", "m4 #3", now - 200, Some(200.0), true)),
            [runs[0].clone()]
        );
    }

    /// Task #929 review: a run of an older way of its step (its record's cost version below the
    /// step's, or none: a record from before the stamp) isn't read: slope-deep's runs before #952
    /// kept 12-55 % of their threads busy on work it no longer does. A step at cost version 0 reads
    /// a record without the stamp as before. The lead stamps what comes in.
    #[test]
    fn a_run_of_an_older_way_of_its_step_isnt_read() {
        let v = crate::coord::cost_version("slope-deep");
        assert!(v > 0);
        let rec = |kind: &str, cost_v: Option<u32>| crate::timings::RunRec {
            v: 1,
            kind: kind.into(),
            host: "m4".into(),
            start: 1000,
            wall_s: 100.0,
            cpu_s: Some(50.0),
            threads: Some(4),
            ok: true,
            cost_v,
            ..Default::default()
        };
        assert!(Run::of(crate::agent::seam::for_tests(), &rec("slope-deep", None)).is_empty());
        assert!(Run::of(crate::agent::seam::for_tests(), &rec("slope-deep", Some(v - 1))).is_empty());
        assert_eq!(Run::of(crate::agent::seam::for_tests(), &rec("slope-deep", Some(v))).len(), 1);
        assert_eq!(crate::coord::cost_version("backup"), 0);
        assert_eq!(Run::of(crate::agent::seam::for_tests(), &rec("backup", None)).len(), 1);
        // (Read back from a log alike.)
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("timings.jsonl");
        for r in [rec("slope-deep", None), rec("slope-deep", Some(v))] {
            crate::timings::append(&p, &r).unwrap();
        }
        assert_eq!(Run::read_log(crate::agent::seam::for_tests(), &p, 1100).len(), 1);
    }

    fn run_at(step: &str, mac: &str, threads: u32, cpu_s: f64, end: u64) -> Run {
        Run { end, step: step.into(), mac: mac.into(), threads, cpu_s, wall_s: 100.0, ..Default::default() }
    }
}
