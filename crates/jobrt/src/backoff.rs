//! How long a failure keeps a target from the worker it failed on (agentcore::coord::sched's
//! `Sched::failed`; docs/pool.md §7.1, Failures): `start` after its first, twice as long after each
//! more, at most `ceiling`. A hand-off the lead's merge refuses again holds it as a failure does
//! (agentcore::coord's `Coordinator::merged`). Both are pool settings (`KNOBS`:
//! agentcore::coord::asks, `scenic pool set <name>`, the build page's Settings), read by the lead
//! as it packs; not set, their defaults (`Backoff::default`: an hour, doubling to 32 hours).

use crate::settings::Knob;
use std::time::Duration;

/// The backoff's start and ceiling (hours).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Backoff {
    pub start_h: f64,
    pub ceiling_h: f64,
}

pub const START_H: f64 = 1.0;
pub const CEILING_H: f64 = 32.0;

impl Default for Backoff {
    fn default() -> Self {
        Backoff { start_h: START_H, ceiling_h: CEILING_H }
    }
}

/// The backoff's bounds as the pool's settings: each a number in its range, null its default. The
/// start from six minutes (a hold shorter than a job's start is a retry at once) to a day; the
/// ceiling from an hour to a week, the history the lead's start recalls the holds from
/// (agentcore::coord::history::KEEP_S).
pub const KNOBS: [Knob<Backoff>; 2] = [
    Knob {
        name: "backoff-start-hours",
        what: "a target that failed on a Mac, or whose hand-off from it was refused again, isn't offered to it again for this long, twice as long after each more",
        unit: "h",
        least: 0.1,
        most: 24.0,
        get: |b| b.start_h,
        set: |b, v| b.start_h = v,
    },
    Knob {
        name: "backoff-ceiling-hours",
        what: "the longest a failure or a refusal keeps a target from a Mac; a target's failures there are forgotten this long after its last",
        unit: "h",
        least: 1.0,
        most: 168.0,
        get: |b| b.ceiling_h,
        set: |b, v| b.ceiling_h = v,
    },
];

impl Backoff {
    /// The backoff with each knob's value as `get` says it (None, or out of its range: its default).
    pub fn of(get: impl Fn(&str) -> Option<f64>) -> Backoff {
        let mut b = Backoff::default();
        for k in &KNOBS {
            if let Some(v) = get(k.name).filter(|v| (k.least..=k.most).contains(v)) {
                (k.set)(&mut b, v);
            }
        }
        b
    }

    /// The hold after `n` failures in a row (at least one): `start` doubled `n - 1` times, at most
    /// the ceiling.
    pub fn hold(&self, n: u32) -> Duration {
        let h = self.start_h * 2f64.powi(n.saturating_sub(1).min(60) as i32);
        Duration::from_secs_f64(h.min(self.ceiling_h) * 3600.0)
    }

    /// How long a failure is kept: the ceiling (no hold is longer).
    pub fn ceiling(&self) -> Duration {
        Duration::from_secs_f64(self.ceiling_h * 3600.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hold_doubles_from_its_start_to_its_ceiling_as_set() {
        let h = |b: &Backoff, n| b.hold(n).as_secs();
        let d = Backoff::of(|_| None);
        assert_eq!(d, Backoff::default());
        assert_eq!((1..=7).map(|n| h(&d, n)).collect::<Vec<_>>(), [1, 2, 4, 8, 16, 32, 32].map(|x| x * 3600));
        assert_eq!(h(&d, 1000), 32 * 3600);
        let b = Backoff::of(|n| match n {
            "backoff-start-hours" => Some(0.5),
            "backoff-ceiling-hours" => Some(3.0),
            _ => None,
        });
        assert_eq!((1..=4).map(|n| h(&b, n)).collect::<Vec<_>>(), [1800, 3600, 2 * 3600, 3 * 3600]);
        assert_eq!(b.ceiling().as_secs(), 3 * 3600);
        // (Out of its range: its default.)
        let b = Backoff::of(|n| (n == "backoff-ceiling-hours").then_some(500.0));
        assert_eq!(b, Backoff::default());
        for k in &KNOBS {
            let d = (k.get)(&Backoff::default());
            assert!((k.least..=k.most).contains(&d), "{}'s default {d} in its range", k.name);
            assert!(crate::settings::bound(k.name).is_some());
        }
    }
}
