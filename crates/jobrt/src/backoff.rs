//! How long a failure keeps a target from the worker it failed on (agentcore::coord::sched's
//! `Sched::failed`; docs/pool.md §7.5, Failures back off): `start` after its first, twice as long after each
//! more, at most `ceiling`. A hand-off the lead's merge refuses again holds it as a failure does
//! (agentcore::coord's `Coordinator::merged`). The lead's agent backs off its own work's failures
//! the same way, on knobs of its own (`own_hold`: ten minutes, doubling to six hours, by default;
//! its jobs from the coordinator held by the lead's backoff besides), and keeps a job its memory
//! guard stopped from its Mac as long as the lead keeps a member's first failure (`guard_hold`).
//! All are pool settings (`KNOBS`: agentcore::coord::asks, `scenic pool set <name>`, the build
//! page's Settings), read by the lead as it packs and by its agent as it fails or stops a job; not
//! set, their defaults (`Backoff::default`: an hour, doubling to 32 hours).

use crate::settings::Knob;
use std::time::Duration;

/// The backoff's start and ceiling (hours).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Backoff {
    pub start_h: f64,
    pub ceiling_h: f64,
    /// The lead's agent's own work's: its start (minutes) and ceiling (hours).
    pub own_start_min: f64,
    pub own_ceiling_h: f64,
    /// A Mac's brake on a step (`brakes`): this many of its jobs of the step failed within
    /// `brake_minutes` hold the whole step from it, as a target's failure holds the target.
    pub brake_jobs: f64,
    pub brake_minutes: f64,
}

pub const START_H: f64 = 1.0;
pub const CEILING_H: f64 = 32.0;
pub const OWN_START_MIN: f64 = 10.0;
pub const OWN_CEILING_H: f64 = 6.0;
pub const BRAKE_JOBS: f64 = 3.0;
pub const BRAKE_MINUTES: f64 = 120.0;

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            start_h: START_H,
            ceiling_h: CEILING_H,
            own_start_min: OWN_START_MIN,
            own_ceiling_h: OWN_CEILING_H,
            brake_jobs: BRAKE_JOBS,
            brake_minutes: BRAKE_MINUTES,
        }
    }
}

/// The backoff's bounds as the pool's settings: each a number in its range, null its default. The
/// lead's start from six minutes (a hold shorter than a job's start is a retry at once) to a day;
/// its ceiling from an hour to a week, the history the lead's start recalls the holds from
/// (agentcore::coord::history::KEEP_S). The agent's own start from a minute to a day; its ceiling
/// from six minutes to a week. A step's brake from two failed jobs to a hundred, within five
/// minutes to a day.
pub const KNOBS: [Knob<Backoff>; 6] = [
    Knob {
        name: "backoff-start-hours",
        what: "a target that failed on a Mac, or whose hand-off from it was refused again, isn't offered to it again for this long, twice as long after each more; a job the lead's own memory guard stopped is kept from the lead this long too",
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
    Knob {
        name: "own-retry-start-minutes",
        what: "the lead's own work that failed there (a backup, a check, a region's step) is tried again after this long, twice as long after each more failure in a row",
        unit: "min",
        least: 1.0,
        most: 1440.0,
        get: |b| b.own_start_min,
        set: |b, v| b.own_start_min = v,
    },
    Knob {
        name: "own-retry-ceiling-hours",
        what: "the longest a failure keeps the lead's own work from being tried again",
        unit: "h",
        least: 0.1,
        most: 168.0,
        get: |b| b.own_ceiling_h,
        set: |b, v| b.own_ceiling_h = v,
    },
    Knob {
        name: "step-brake-jobs",
        what: "this many failed jobs of a step on one Mac within step-brake-minutes keep the whole step from that Mac as a target's failure keeps the target (for backoff-start-hours, twice as long each time after), whatever targets they failed on",
        unit: "jobs",
        least: 2.0,
        most: 100.0,
        get: |b| b.brake_jobs,
        set: |b, v| b.brake_jobs = v,
    },
    Knob {
        name: "step-brake-minutes",
        what: "the window step-brake-jobs failed jobs of a step on one Mac are counted in",
        unit: "min",
        least: 5.0,
        most: 1440.0,
        get: |b| b.brake_minutes,
        set: |b, v| b.brake_minutes = v,
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

    /// The lead's agent's hold of its own work after `n` failures in a row (at least one): its own
    /// start doubled `n - 1` times, at most its own ceiling.
    pub fn own_hold(&self, n: u32) -> Duration {
        let m = self.own_start_min * 2f64.powi(n.saturating_sub(1).min(60) as i32);
        Duration::from_secs_f64((m * 60.0).min(self.own_ceiling_h * 3600.0))
    }

    /// Whether a failed job at `at` (unix s) brakes its step on its Mac, `fails` the times of that
    /// Mac's failed jobs of the step before it: `brake_jobs` of them, this one among them, within
    /// `brake_minutes` (those older dropped). A brake counts afresh: `fails` emptied.
    pub fn brakes(&self, fails: &mut Vec<u64>, at: u64) -> bool {
        let window = (self.brake_minutes * 60.0) as u64;
        fails.retain(|t| at.saturating_sub(*t) < window);
        fails.push(at);
        if fails.len() as f64 >= self.brake_jobs {
            fails.clear();
            return true;
        }
        false
    }

    /// How long the lead's agent keeps a job its memory guard stopped from its Mac: as long as the
    /// lead keeps a member's job its guard stopped (`hold` of a first failure, its start).
    pub fn guard_hold(&self) -> Duration {
        self.hold(1)
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
        assert_eq!(b.guard_hold().as_secs(), 1800, "the guard's hold is the lead's start as set");
        // (The lead's own work's: ten minutes doubling to six hours, or as set.)
        assert_eq!(
            (1..=8).map(|n| d.own_hold(n).as_secs()).collect::<Vec<_>>(),
            [10, 20, 40, 80, 160, 320, 360, 360].map(|m| m * 60)
        );
        assert_eq!(d.own_hold(1000).as_secs(), 6 * 3600);
        assert_eq!(d.guard_hold().as_secs(), 3600);
        let o = Backoff::of(|n| match n {
            "own-retry-start-minutes" => Some(2.0),
            "own-retry-ceiling-hours" => Some(0.1),
            _ => None,
        });
        assert_eq!((1..=4).map(|n| o.own_hold(n).as_secs()).collect::<Vec<_>>(), [120, 240, 360, 360]);
        assert_eq!((o.start_h, o.ceiling_h), (START_H, CEILING_H), "the lead's own as they were");
        // (A step's brake: three failed jobs within two hours, by default; a brake counts afresh.)
        let mut f = Vec::new();
        assert!(!d.brakes(&mut f, 0) && !d.brakes(&mut f, 100));
        assert!(!d.brakes(&mut f, 7250), "the first out of the window");
        assert!(d.brakes(&mut f, 7280) && f.is_empty());
        let b = Backoff::of(|n| match n {
            "step-brake-jobs" => Some(2.0),
            "step-brake-minutes" => Some(10.0),
            _ => None,
        });
        let mut f = Vec::new();
        assert!(!b.brakes(&mut f, 0) && !b.brakes(&mut f, 600) && b.brakes(&mut f, 1199));
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
