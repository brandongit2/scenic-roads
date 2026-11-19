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
    /// A step's time per unit of work, and its waits per unit: this percentile of its runs' (the
    /// median of the fastest twice this share: crowding only adds time, and one freak can't set it).
    pub work_pct: f64,
    /// A step's memory, its disk and an update drain's wait: this percentile of its runs at the
    /// target's size or more.
    pub high_pct: f64,
    /// A step's NAS bytes and metadata calls per unit of input: this percentile of its runs'.
    pub nas_pct: f64,
    /// A step's newest runs its needs are learned from.
    pub runs: u64,
    /// A run off its prediction by more than this many times past what its supply explains is
    /// flagged (new code, new data, a regression).
    pub off: f64,
    /// This many flagged runs in a row on one side, and the step learns from the first of them on.
    pub relearn: u64,
}

pub const WORK_PCT: f64 = 40.0;
pub const HIGH_PCT: f64 = 95.0;
pub const NAS_PCT: f64 = 70.0;
pub const RUNS: u64 = 500;
pub const OFF: f64 = 3.0;
pub const RELEARN: u64 = 5;

impl Default for Tuning {
    fn default() -> Self {
        Tuning { work_pct: WORK_PCT, high_pct: HIGH_PCT, nas_pct: NAS_PCT, runs: RUNS, off: OFF, relearn: RELEARN }
    }
}

/// The needs model's bounds as the pool's settings: each a number in its range, null its default.
pub const KNOBS: [Knob<Tuning>; 6] = [
    Knob {
        name: "needs-work-pct",
        what: "a step's time per unit of work and its waits: this percentile of its runs (the median of the fastest twice this share)",
        unit: "%",
        least: 5.0,
        most: 95.0,
        get: |k| k.work_pct,
        set: |k, v| k.work_pct = v,
    },
    Knob {
        name: "needs-high-pct",
        what: "a step's memory, disk and drain wait: this percentile of its runs at the target's size or more",
        unit: "%",
        least: 50.0,
        most: 100.0,
        get: |k| k.high_pct,
        set: |k, v| k.high_pct = v,
    },
    Knob {
        name: "needs-nas-pct",
        what: "a step's NAS bytes and calls per unit of input: this percentile of its runs",
        unit: "%",
        least: 10.0,
        most: 100.0,
        get: |k| k.nas_pct,
        set: |k, v| k.nas_pct = v,
    },
    Knob {
        name: "needs-runs",
        what: "a step's newest runs its needs are learned from",
        unit: "runs",
        least: 20.0,
        most: 50000.0,
        get: |k| k.runs as f64,
        set: |k, v| k.runs = v as u64,
    },
    Knob {
        name: "needs-off",
        what: "a run off its prediction by this many times past what its supply explains is flagged",
        unit: "×",
        least: 1.5,
        most: 20.0,
        get: |k| k.off,
        set: |k, v| k.off = v,
    },
    Knob {
        name: "needs-relearn",
        what: "this many flagged runs in a row on one side: the step learns from the first of them on",
        unit: "runs",
        least: 2.0,
        most: 50.0,
        get: |k| k.relearn as f64,
        set: |k, v| k.relearn = v as u64,
    },
];

/// The knob named `name`.
pub fn knob(name: &str) -> Option<&'static Knob<Tuning>> {
    KNOBS.iter().find(|k| k.name == name)
}

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
            "needs-runs" => Some(80.0),
            _ => None,
        });
        assert_eq!((k.work_pct, k.high_pct, k.runs), (25.0, HIGH_PCT, 80));
        for n in &KNOBS {
            let d = (n.get)(&Tuning::default());
            assert!((n.least..=n.most).contains(&d), "{}'s default {d} in its range", n.name);
            assert!(crate::settings::bound(n.name).is_some());
        }
    }
}
