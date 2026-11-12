//! How far a resolve of the regions is (task #1145): its steps in order (`Step`), each an equal share
//! of it, and within the one under way its items done of its total (regions, or cells), the item
//! under way by its own parts done (a region's cells). The resolve counts where it works
//! (`Progress::begin`, `one`, `item`, `part`); the check's ticker (jobrt::jobs::saying, in
//! crate::inputs::regions's `Regions::resolve`) reads the share and the line every few seconds, so
//! a check's progress goes on through the long passes (the cover check's cells, the coast's) and
//! the slow and stuck flags judge it while it works. The share never goes back.

use std::sync::Mutex;

/// A resolve's steps, in the order it takes them (crate::inputs::regions::resolve's `resolve_all`,
/// then crate::inputs::regions::tiles' `resolve_in` and `cells_pass`). A step the folder doesn't
/// take (no region with water, no `cover`) is passed over, its share with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Plain,
    Land,
    Water,
    Lakes,
    Coast,
    CoastSplits,
    CoastFills,
    Areas,
    HandOuts,
    Slivers,
    Zones,
    ZoneCells,
    Cover,
    GapLand,
    GapShares,
    Fills,
    RestInZones,
    Fit,
    Stored,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Step; 19] = [
        Step::Plain,
        Step::Land,
        Step::Water,
        Step::Lakes,
        Step::Coast,
        Step::CoastSplits,
        Step::CoastFills,
        Step::Areas,
        Step::HandOuts,
        Step::Slivers,
        Step::Zones,
        Step::ZoneCells,
        Step::Cover,
        Step::GapLand,
        Step::GapShares,
        Step::Fills,
        Step::RestInZones,
        Step::Fit,
        Step::Stored,
    ];

    /// What it is, and what its items are.
    pub fn said(self) -> (&'static str, &'static str) {
        match self {
            Step::Plain => ("the regions without water", "regions"),
            Step::Land => ("the land", "regions"),
            Step::Water => ("the water", "regions"),
            Step::Lakes => ("the lakes", "cells"),
            Step::Coast => ("the coast", "cells"),
            Step::CoastSplits => ("the coast's splits", "cells"),
            Step::CoastFills => ("the coast's fills", "cells"),
            Step::Areas => ("each region's area", "cells"),
            Step::HandOuts => ("the hand-outs", "regions"),
            Step::Slivers => ("the slivers", "regions"),
            Step::Zones => ("the 200-nmi zones", "zones"),
            Step::ZoneCells => ("the 200-nmi zones by cell", "cells"),
            Step::Cover => ("the cover check", "cells"),
            Step::GapLand => ("the gaps' land", "cells"),
            Step::GapShares => ("the gaps' shares", "stretches"),
            Step::Fills => ("the fills", "cells"),
            Step::RestInZones => ("the rest inside the zones", "cells"),
            Step::Fit => ("the fit", "cells"),
            Step::Stored => ("storing", "regions"),
        }
    }

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// A resolve's progress (the module's doc).
#[derive(Default)]
pub struct Progress(Mutex<State>);

#[derive(Default)]
struct State {
    /// The step under way (None: none begun), its items' total and how many are done.
    step: Option<Step>,
    total: u64,
    done: u64,
    /// The item under way's parts (0: not counted by parts) and how many are done.
    parts: u64,
    parts_done: u64,
    /// The most `share` has said.
    best: f64,
    /// Each step begun, with its items done and its total as it ended (the one under way: as it
    /// stands).
    #[cfg(test)]
    steps: Vec<(Step, u64, u64)>,
}

impl Progress {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Step `step` begins, with `total` items. A step before or at the one under way begins
    /// nothing (the share never goes back).
    pub fn begin(&self, step: Step, total: usize) {
        let mut s = self.state();
        if s.step.is_some_and(|now| now >= step) {
            return;
        }
        #[cfg(test)]
        if let Some(now) = s.step {
            let ended = (now, s.done, s.total);
            s.steps.push(ended);
        }
        (s.step, s.total, s.done, s.parts, s.parts_done) = (Some(step), total as u64, 0, 0, 0);
    }

    /// An item of the step under way done.
    pub fn one(&self) {
        let mut s = self.state();
        s.done = (s.done + 1).min(s.total);
        (s.parts, s.parts_done) = (0, 0);
    }

    /// An item of the step under way, done as the guard drops (however its work ends: an early
    /// return, a `?`).
    pub fn one_on_drop(&self) -> Done<'_> {
        Done(self, false)
    }

    /// A part of the item under way, done as the guard drops (`one_on_drop`'s way).
    pub fn part_on_drop(&self) -> Done<'_> {
        Done(self, true)
    }

    /// The step's next item is under way, in `parts` parts (`part`).
    pub fn item(&self, parts: usize) {
        let mut s = self.state();
        (s.parts, s.parts_done) = (parts as u64, 0);
    }

    /// A part of the item under way done.
    pub fn part(&self) {
        let mut s = self.state();
        s.parts_done = (s.parts_done + 1).min(s.parts);
    }

    /// How far the resolve is, 0 to 1: its steps before the one under way, and that one's items
    /// done (the one under way by its parts). Never less than it said before.
    pub fn share(&self) -> f64 {
        let mut s = self.state();
        let Some(step) = s.step else { return s.best };
        let item = if s.parts > 0 { s.parts_done as f64 / s.parts as f64 } else { 0.0 };
        let within = if s.total > 0 { ((s.done as f64 + item) / s.total as f64).min(1.0) } else { 1.0 };
        let share = (step.index() as f64 + within) / Step::ALL.len() as f64;
        s.best = s.best.max(share);
        s.best
    }

    /// What's under way, for the progress line: `the cover check, 5200 of 16200 cells` (empty
    /// before a step begins).
    pub fn line(&self) -> String {
        let s = self.state();
        let Some(step) = s.step else { return String::new() };
        let (what, items) = step.said();
        format!("{what}, {} of {} {items}", s.done, s.total)
    }

    /// Each step begun, its items done and its total (the tests' view).
    #[cfg(test)]
    pub fn steps(&self) -> Vec<(Step, u64, u64)> {
        let s = self.state();
        s.steps.iter().copied().chain(s.step.map(|now| (now, s.done, s.total))).collect()
    }

    /// A ticker's say for jobrt::jobs::saying: the share and the line, as the stage's under way.
    pub fn say(&self) {
        jobrt::jobs::within_doing(self.share(), &self.line());
    }
}

/// An item (or a part: true) counted done as it drops (`Progress::one_on_drop`).
pub struct Done<'a>(&'a Progress, bool);

impl Drop for Done<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.part();
        } else {
            self.0.one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The share goes on by the steps, each an equal share, and by the items and parts within the
    /// one under way; never back, a step begun again or out of order included.
    #[test]
    fn the_share_goes_on_by_steps_items_and_parts() {
        let p = Progress::default();
        let n = Step::ALL.len() as f64;
        assert_eq!((p.share(), p.line().as_str()), (0.0, ""));
        p.begin(Step::Plain, 4);
        assert_eq!(p.share(), 0.0);
        p.one();
        p.one();
        assert!((p.share() - 0.5 / n).abs() < 1e-12);
        assert_eq!(p.line(), "the regions without water, 2 of 4 regions");
        // The land: a region of 4 cells under way, 2 done.
        p.begin(Step::Land, 2);
        p.item(4);
        p.part();
        p.part();
        assert!((p.share() - (1.0 + 0.25) / n).abs() < 1e-12);
        p.one();
        assert!((p.share() - 1.5 / n).abs() < 1e-12, "the region done: its parts no longer counted");
        // Steps passed over count as done; one begun again, or before, begins nothing.
        p.begin(Step::Cover, 16_200);
        let at = p.share();
        assert!((at - 12.0 / n).abs() < 1e-12);
        p.begin(Step::Land, 2);
        p.begin(Step::Cover, 10);
        assert_eq!(p.line(), "the cover check, 0 of 16200 cells");
        assert_eq!(p.share(), at);
        // More done than its total: no further than the step's end.
        for _ in 0..20_000 {
            p.one();
        }
        assert!((p.share() - 13.0 / n).abs() < 1e-12);
        p.begin(Step::Stored, 0);
        assert_eq!(p.share(), 1.0, "a step of nothing is done");
    }
}
