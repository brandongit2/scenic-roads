//! The needs model's percentiles and windows (agentcore::needs, task #1416; docs/pool.md §7.7,
//! What a step needs), each a pool setting (`KNOBS`: agentcore::coord::asks, `scenic pool set
//! <name>`, the build page's Settings), read by the lead as its models are made; not set, its
//! default (`Tuning::default`). The defaults were picked by the held-out test on the lead's records
//! of 12–19 Nov (docs/pool.md §7.7, The held-out test).

use crate::settings::Knob;

/// The needs model's bounds.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tuning {
    /// A step's work per unit, and the rest of its time per unit: this percentile of its runs' of
    /// about a target's size (the median of the fastest twice this share: crowding only adds time,
    /// and one freak can't set it).
    pub work_pct: f64,
    /// A step's disk and an update drain's wait: this percentile of its runs at the target's size
    /// or more.
    pub high_pct: f64,
    /// A job's memory as the pool admits and packs it (agentcore::coord::sched): this percentile of
    /// its step's runs at the target's size or more, each at the threads the job runs on.
    pub mem_pct: f64,
    /// A thread a job grows by past its runs' threads adds at least this share (%) of their peak
    /// over their threads to its memory, whatever the fit over sizes and threads says (a peak
    /// varies with what a target holds far more than with its threads: the fit alone came out near
    /// nothing for steps whose runs say little of their threads).
    pub thread_share: f64,
    /// A step's newest runs its times (work, the rest, parallelism, the NAS rate) are learned
    /// from, and checked against their predictions over (memory and disk read every run: a peak
    /// is a run's, and only many runs show the rare large one). At most `KNOBS`' 5,000: each
    /// prediction checked is inserted in its size bin's sorted runs, so a window's check grows as
    /// its square: the cap bounds it.
    pub runs: u64,
    /// A run off its prediction by more than this many times past what its supply explains is
    /// flagged (new code, new data, a regression).
    pub off: f64,
    /// This many flagged runs in a row on one side, and the step learns from the first of them on.
    pub relearn: u64,
}

pub const WORK_PCT: f64 = 40.0;
pub const HIGH_PCT: f64 = 95.0;
/// With `THREAD_SHARE`, picked by the admission replay on the lead's records of 12–19 Nov packed
/// into each Mac's room (docs/pool.md §7.7, Memory admission): of the pairs whose jobs waited no
/// longer for room than under the scheduler it replaced, the one whose jobs took the least past
/// their memory, no step with more jobs past it than before.
pub const MEM_PCT: f64 = 98.0;
/// Picked with `MEM_PCT` by the same replay.
pub const THREAD_SHARE: f64 = 50.0;
pub const RUNS: u64 = 500;
pub const OFF: f64 = 3.0;
pub const RELEARN: u64 = 5;

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            work_pct: WORK_PCT,
            high_pct: HIGH_PCT,
            mem_pct: MEM_PCT,
            thread_share: THREAD_SHARE,
            runs: RUNS,
            off: OFF,
            relearn: RELEARN,
        }
    }
}

/// The needs model's bounds as the pool's settings: each a number in its range, null its default.
pub const KNOBS: [Knob<Tuning>; 7] = [
    Knob {
        name: "needs-work-pct",
        what: "a step's work and the rest of its time per unit: this percentile of its runs of about a target's size",
        unit: "%",
        least: 5.0,
        most: 95.0,
        get: |k| k.work_pct,
        set: |k, v| k.work_pct = v,
    },
    Knob {
        name: "needs-high-pct",
        what: "a step's disk and drain wait: this percentile of its runs at the target's size or more",
        unit: "%",
        least: 50.0,
        most: 100.0,
        get: |k| k.high_pct,
        set: |k, v| k.high_pct = v,
    },
    Knob {
        name: "needs-mem-pct",
        what: "a job's memory as the pool admits it: this percentile of its step's runs at the target's size or more, at the job's threads",
        unit: "%",
        least: 50.0,
        most: 100.0,
        get: |k| k.mem_pct,
        set: |k, v| k.mem_pct = v,
    },
    Knob {
        name: "needs-thread-share",
        what: "a thread a job grows by adds at least this share of its runs' peak over their threads to its memory",
        unit: "%",
        least: 0.0,
        most: 100.0,
        get: |k| k.thread_share,
        set: |k, v| k.thread_share = v,
    },
    Knob {
        name: "needs-runs",
        what: "a step's newest runs its times are learned from and checked over",
        unit: "runs",
        least: 20.0,
        most: 5000.0,
        get: |k| k.runs as f64,
        set: |k, v| k.runs = v as u64,
    },
    Knob {
        name: "needs-off",
        what: "a run off its prediction by this many times past what its supply explains is flagged; a rerun off its run before by this many times in memory, a cost version not raised (the cost-version flag)",
        unit: "×",
        least: 1.5,
        most: 20.0,
        get: |k| k.off,
        set: |k, v| k.off = v,
    },
    Knob {
        name: "needs-relearn",
        what: "this many flagged runs in a row on one side: the step learns from the first of them on; this many reruns off in memory on one side, and most of them, flag a cost version not raised",
        unit: "runs",
        least: 2.0,
        most: 50.0,
        get: |k| k.relearn as f64,
        set: |k, v| k.relearn = v as u64,
    },
];

impl Tuning {
    /// The tuning with each knob's value as `get` says it (None, or out of its range: its default).
    pub fn of(get: impl Fn(&str) -> Option<f64>) -> Tuning {
        let mut k = Tuning::default();
        for n in &KNOBS {
            if let Some(v) = get(n.name).filter(|v| (n.least..=n.most).contains(v)) {
                (n.set)(&mut k, v);
            }
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_knob_reads_its_setting_within_its_range() {
        assert_eq!(Tuning::of(|_| None), Tuning::default());
        let k = Tuning::of(|n| match n {
            "needs-work-pct" => Some(25.0),
            "needs-high-pct" => Some(101.0),
            "needs-mem-pct" => Some(97.0),
            "needs-thread-share" => Some(25.0),
            "needs-runs" => Some(80.0),
            _ => None,
        });
        assert_eq!((k.work_pct, k.high_pct, k.mem_pct, k.thread_share, k.runs), (25.0, HIGH_PCT, 97.0, 25.0, 80));
        for n in &KNOBS {
            let d = (n.get)(&Tuning::default());
            assert!((n.least..=n.most).contains(&d), "{}'s default {d} in its range", n.name);
            assert!(crate::settings::bound(n.name).is_some());
        }
    }
}
