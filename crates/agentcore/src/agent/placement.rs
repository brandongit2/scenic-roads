//! Placement by how crucial a job is, and work offered ahead (docs/pool.md §7.2, §12 phase 4's
//! eleventh batch; docs/coordinator.md §3), behind the pool's setting `placement`
//! (crate::coord::asks; off while not set: the agent and its coordinator as they were, exactly). It
//! acts only in the pool.
//!
//! - **How crucial** (`Placement::slack`): each step's slack in the forecast, the time between its
//!   last target's forecast end and the whole build's (`slack`): the long poles (no slack) first.
//!   The lead's coordinator tries a member's ask on the offers in that order, and within a step
//!   from its target measured to take longest (crate::coord's `pick_every`).
//! - **Who takes it** (`draw`): of the members asking lately that would take a target now (its
//!   room fits it by their last ask, with a slot free), one drawn with a weight of its Mac's speed
//!   at the target's step, the inverse of its pace there (crate::paces, task #934: the same
//!   targets' runs on it and on the lead; a Mac with no pair the lead's), times `CACHE_PREFER` where its Mac
//!   holds the step's shared cache whole (crate::needs::CACHES: no copy, no NAS read in place), as
//!   a function of the target and the time (a minute's window, `DRAW_S`, from when the coordinator
//!   was told): the same inputs, the same draw. A member asking for a target drawn to another
//!   passes it by for the next one drawn to it; its next target is one in its last one's
//!   neighbourhood (`group`) when one is drawn to it; a task likewise among the pages and workers that
//!   could take it, weighted by their measured pace at its kind. Weights only, never exclusive: no
//!   rule names a Mac or a kind of job.
//! - **Offered ahead** (jobrt::jobs::AHEAD_ENV, told to every job): a terrain job offers its next z6 tile's
//!   subtrees as it begins a z6 tile; each task of a terrain z6 tile or a tree cover's row says how
//!   much of this Mac's work comes before the job needs it (`ahead`, in tasks' worth: a head start
//!   of that many times this Mac's measured time at its kind). A worker measured slower than this
//!   Mac at a kind is given such a task only while its pace (with the margin) times this Mac's time
//!   fits in the head start left (crate::coord::task's `pick`).

use std::collections::BTreeMap;

/// How long one draw holds (s): a target drawn to a member that doesn't ask for it in that time is
/// drawn again.
pub const DRAW_S: u64 = 60;

/// What the lead's coordinator places by (`crate::coord::Coordinator::set_placement`); a draw's
/// weights are the coordinator's paces (crate::paces, `Sched::draw_weight`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    /// Each step's slack in the forecast (s); a step the forecast gives no end is last.
    pub slack: BTreeMap<String, u64>,
}

/// The weight a Mac holding a step's shared cache whole has in a draw, against one that would copy
/// it or read the NAS in place (task #552): one random 64 KB NAS read takes ~40 ms where the local
/// disk takes 0.2 ms, and a whole copy is GBs.
pub const CACHE_PREFER: f64 = 4.0;

impl Placement {
    /// From the forecast: its steps' slack.
    pub fn from_forecast(f: &super::forecast::Forecast) -> Placement {
        Placement { slack: slack(f) }
    }

    /// Step `step`'s slack (u64::MAX: none known).
    pub fn slack_of(&self, step: &str) -> u64 {
        self.slack.get(step).copied().unwrap_or(u64::MAX)
    }
}

/// A target's neighbourhood (task #552): a z6 tile's z3 tile (`6/x/y` → `3/(x/8)/(y/8)`); any
/// other target its own. An asker's next target is one in its last one's neighbourhood when it can
/// be (crate::coord::sched's `placed_start`): its jobs there read the same inputs.
pub fn group(target: &str) -> std::borrow::Cow<'_, str> {
    let mut it = target.split('/');
    match (it.next(), it.next().map(str::parse::<u32>), it.next().map(str::parse::<u32>), it.next()) {
        (Some("6"), Some(Ok(x)), Some(Ok(y)), None) => format!("3/{}/{}", x >> 3, y >> 3).into(),
        _ => target.into(),
    }
}

/// Each step's slack in forecast `f` (s): the build's end less the step's (0 when it ends last).
pub fn slack(f: &super::forecast::Forecast) -> BTreeMap<String, u64> {
    let Some(end) = f.done_at.or_else(|| f.steps.iter().filter_map(|s| s.done_at).max()) else {
        return BTreeMap::new();
    };
    f.steps.iter().filter_map(|s| Some((s.step.clone(), end.saturating_sub(s.done_at?)))).collect()
}

/// A stable 64-bit hash of `key` (FNV-1a, then mixed: splitmix64's finalizer), the same on every
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

    #[test]
    fn a_z6_tiles_group_is_its_z3_tile() {
        assert_eq!(group("6/17/22"), "3/2/2");
        assert_eq!(group("6/23/23"), "3/2/2");
        assert_eq!(group("6/24/23"), "3/3/2");
        assert_eq!(group("catalog"), "catalog");
        assert_eq!(group("8/1/1"), "8/1/1");
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
            ..Default::default()
        };
        let p = Placement::from_forecast(&f);
        assert_eq!((p.slack_of("terrain"), p.slack_of("pack"), p.slack_of("x")), (0, 600, u64::MAX));
    }
}
