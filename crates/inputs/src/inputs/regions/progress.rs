//! How far a resolve of the regions is (task #1145): its steps in order (`Step`), each an equal share
//! of it, and within the one under way its items done of its total (regions, or cells), the items
//! under way (several at once where a step takes its regions so) by their own parts done (a
//! region's cells). The resolve counts where it works (`Progress::begin`, `one_on_drop`, `item`,
//! `Item::part_on_drop`); the check's ticker (jobrt::jobs::saying, in
//! crate::inputs::regions's `Regions::resolve`) reads the share and the line every few seconds, so
//! a check's progress goes on through the long passes (the cover check's cells, the coast's) and
//! the slow and stuck flags judge it while it works. The share never goes back.

use std::sync::Mutex;
use std::time::Instant;

/// A resolve's steps, in the order it takes them (crate::inputs::regions::resolve's `resolve_all`,
/// then crate::inputs::regions::tiles' `resolve_in` and `cover_check`). A step the folder doesn't
/// take (no region with water, no `cover`) is passed over, its share with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Plain,
    Land,
    Remainders,
    Water,
    Lakes,
    Coast,
    CoastSplits,
    CoastFills,
    Areas,
    HandOuts,
    Strays,
    Zones,
    ZoneCells,
    Cover,
    Gaps,
    Rest,
    Stored,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Step; 17] = [
        Step::Plain,
        Step::Land,
        Step::Remainders,
        Step::Water,
        Step::Lakes,
        Step::Coast,
        Step::CoastSplits,
        Step::CoastFills,
        Step::Areas,
        Step::HandOuts,
        Step::Strays,
        Step::Zones,
        Step::ZoneCells,
        Step::Cover,
        Step::Gaps,
        Step::Rest,
        Step::Stored,
    ];

    /// What it is, and what its items are.
    fn said(self) -> (&'static str, &'static str) {
        match self {
            Step::Plain => ("the regions without water", "regions"),
            Step::Land => ("the land", "regions"),
            Step::Remainders => ("the remainders", "folders"),
            Step::Water => ("the water", "regions"),
            Step::Lakes => ("the lakes", "cells"),
            Step::Coast => ("the coast", "cells"),
            Step::CoastSplits => ("the coast's splits", "cells"),
            Step::CoastFills => ("the coast's fills", "cells"),
            Step::Areas => ("each region's area", "cells"),
            Step::HandOuts => ("the hand-outs", "regions"),
            Step::Strays => ("the stray water", "regions"),
            Step::Zones => ("the 200-nmi zones", "zones"),
            Step::ZoneCells => ("the 200-nmi zones by cell", "cells"),
            Step::Cover => ("the cover check", "cells"),
            Step::Gaps => ("the gaps", "stretches"),
            Step::Rest => ("the rest", "cells"),
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
    /// The step under way (None: none begun), its items' total and how many are done, and when it
    /// began.
    step: Option<Step>,
    total: u64,
    done: u64,
    began: Option<Instant>,
    /// The named items under way (several at once where a step takes its regions so), the first
    /// begun first.
    under: Vec<Under>,
    /// The next item's key.
    next: u64,
    /// The most `share` has said.
    best: f64,
    /// Each step ended: its items done and its total as it ended, and its wall time.
    steps: Vec<Took>,
    /// Each named item done: its step, its name and its wall time.
    items: Vec<(Step, String, f64)>,
}

/// An item under way (`Progress::item`): its key, its step, its name (a region's id, a folder's
/// path), the step's items it counts as, its parts (0: not counted by parts) and how many are done,
/// and when it began.
struct Under {
    key: u64,
    step: Step,
    name: String,
    counts: u64,
    parts: u64,
    parts_done: u64,
    began: Instant,
}

/// A step's time (`Progress::steps`): its items done of its total, and its wall time, s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Took {
    pub step: Step,
    pub done: u64,
    pub total: u64,
    pub secs: f64,
}

impl Step {
    /// What it is, in words (`the cover check`).
    pub fn what(self) -> &'static str {
        self.said().0
    }
}

impl State {
    /// The step under way, as it stands.
    fn now(&self) -> Option<Took> {
        Some(Took {
            step: self.step?,
            done: self.done,
            total: self.total,
            secs: self.began.map_or(0.0, |t| t.elapsed().as_secs_f64()),
        })
    }
}

impl Progress {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Step `step` begins, with `total` items, said at once (`say`: a step shorter than the
    /// ticker's beat is seen too). A step before or at the one under way begins nothing (the share
    /// never goes back).
    pub fn begin(&self, step: Step, total: usize) {
        {
            let mut s = self.state();
            if s.step.is_some_and(|now| now >= step) {
                return;
            }
            if let Some(ended) = s.now() {
                s.steps.push(ended);
            }
            (s.step, s.total, s.done, s.began) = (Some(step), total as u64, 0, Some(Instant::now()));
            s.under.clear();
        }
        self.say();
    }

    /// An item of the step under way done, not named.
    fn one(&self) {
        let mut s = self.state();
        s.done = (s.done + 1).min(s.total);
    }

    /// An item of the step under way, not named, done as the guard drops (however its work ends:
    /// an early return, a `?`).
    pub fn one_on_drop(&self) -> Done<'_> {
        Done(self, None)
    }

    /// The step's item `name` (a region's id, a folder's path; its line says it, and its time is
    /// kept) under way, beside any others; done, its time kept, as the guard drops. Its parts:
    /// `Item::parts`, each done by `Item::part_on_drop`.
    pub fn item(&self, name: &str) -> Item<'_> {
        self.items(name, 1)
    }

    /// `item` counting as `counts` of the step's items (a run of regions taken together).
    pub fn items(&self, name: &str, counts: usize) -> Item<'_> {
        let mut s = self.state();
        let (key, step) = (s.next, s.step.unwrap_or(Step::Plain));
        s.next += 1;
        s.under.push(Under {
            key,
            step,
            name: name.to_string(),
            counts: counts as u64,
            parts: 0,
            parts_done: 0,
            began: Instant::now(),
        });
        Item(self, key)
    }

    /// A part of the item `key` done.
    fn part(&self, key: u64) {
        let mut s = self.state();
        if let Some(u) = s.under.iter_mut().find(|u| u.key == key) {
            u.parts_done = (u.parts_done + 1).min(u.parts);
        }
    }

    /// How far the resolve is, 0 to 1: its steps before the one under way, and that one's items
    /// done (those under way by their parts). Never less than it said before.
    pub fn share(&self) -> f64 {
        let mut s = self.state();
        let Some(step) = s.step else { return s.best };
        let parts: f64 = s
            .under
            .iter()
            .filter(|u| u.parts > 0)
            .map(|u| u.counts as f64 * u.parts_done as f64 / u.parts as f64)
            .sum();
        let within = if s.total > 0 { ((s.done as f64 + parts) / s.total as f64).min(1.0) } else { 1.0 };
        let share = (step.index() as f64 + within) / Step::ALL.len() as f64;
        s.best = s.best.max(share);
        s.best
    }

    /// What's under way, for the progress line: `the cover check, 5200 of 16200 cells`, the item
    /// under way longest named, by its parts, and how many more are (`the land, 12 of 300 regions;
    /// alberta, 40 of 120; 3 more`); empty before a step begins.
    pub fn line(&self) -> String {
        let s = self.state();
        let Some(step) = s.step else { return String::new() };
        let (what, items) = step.said();
        let mut l = format!("{what}, {} of {} {items}", s.done, s.total);
        let mut named = s.under.iter().filter(|u| !u.name.is_empty());
        if let Some(u) = named.next() {
            l += &format!("; {}", u.name);
            if u.parts > 0 {
                l += &format!(", {} of {}", u.parts_done, u.parts);
            }
            let more = named.count();
            if more > 0 {
                l += &format!("; {more} more");
            }
        }
        l
    }

    /// Each step begun, its items done and its total, and its wall time (the one under way: as it
    /// stands).
    pub fn steps(&self) -> Vec<Took> {
        let s = self.state();
        s.steps.iter().copied().chain(s.now()).collect()
    }

    /// The `n` named items that took longest, longest first: their step, name and wall time, s.
    pub fn slowest(&self, n: usize) -> Vec<(Step, String, f64)> {
        let mut v = self.state().items.clone();
        v.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
        v.truncate(n);
        v
    }

    /// A ticker's say for jobrt::jobs::saying: the share and the line, as the stage's under way.
    pub fn say(&self) {
        jobrt::jobs::within_doing(self.share(), &self.line());
    }
}

/// An item not named (None), or a part of one under way (its key), counted done as it drops
/// (`Progress::one_on_drop`, `Item::part_on_drop`).
pub struct Done<'a>(&'a Progress, Option<u64>);

impl Drop for Done<'_> {
    fn drop(&mut self) {
        match self.1 {
            Some(key) => self.0.part(key),
            None => self.0.one(),
        }
    }
}

/// A named item under way (`Progress::item`), done as it drops: counted, its time kept.
pub struct Item<'a>(&'a Progress, u64);

impl Item<'_> {
    /// It goes in `n` parts (`part_on_drop`).
    pub fn parts(&self, n: usize) {
        let mut s = self.0.state();
        if let Some(u) = s.under.iter_mut().find(|u| u.key == self.1) {
            (u.parts, u.parts_done) = (n as u64, 0);
        }
    }

    /// One of its parts, done as the guard drops.
    pub fn part_on_drop(&self) -> Done<'_> {
        Done(self.0, Some(self.1))
    }
}

impl Drop for Item<'_> {
    fn drop(&mut self) {
        let mut s = self.0.state();
        let Some(k) = s.under.iter().position(|u| u.key == self.1) else { return };
        let u = s.under.remove(k);
        if s.step == Some(u.step) {
            s.done = (s.done + u.counts).min(s.total);
            if !u.name.is_empty() {
                s.items.push((u.step, u.name, u.began.elapsed().as_secs_f64()));
            }
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
        let alberta = p.item("alberta");
        alberta.parts(4);
        drop(alberta.part_on_drop());
        drop(alberta.part_on_drop());
        assert!((p.share() - (1.0 + 0.25) / n).abs() < 1e-12);
        assert_eq!(p.line(), "the land, 0 of 2 regions; alberta, 2 of 4");
        drop(alberta);
        assert!((p.share() - 1.5 / n).abs() < 1e-12, "the region done: its parts no longer counted");
        assert_eq!(p.line(), "the land, 1 of 2 regions", "the item done: no longer named");
        // An item not named isn't kept.
        drop(p.item(""));
        // Steps passed over count as done; one begun again, or before, begins nothing.
        p.begin(Step::Cover, 16_200);
        let at = p.share();
        assert!((at - 13.0 / n).abs() < 1e-12);
        p.begin(Step::Land, 2);
        p.begin(Step::Cover, 10);
        assert_eq!(p.line(), "the cover check, 0 of 16200 cells");
        assert_eq!(p.share(), at);
        // More done than its total: no further than the step's end.
        for _ in 0..20_000 {
            p.one();
        }
        assert!((p.share() - 14.0 / n).abs() < 1e-12);
        p.begin(Step::Stored, 0);
        assert_eq!(p.share(), 1.0, "a step of nothing is done");
        // Each step begun kept with its count as it ended, and its time; the named items timed.
        let steps: Vec<(Step, u64, u64)> = p.steps().iter().map(|t| (t.step, t.done, t.total)).collect();
        assert_eq!(
            steps,
            [(Step::Plain, 2, 4), (Step::Land, 2, 2), (Step::Cover, 16_200, 16_200), (Step::Stored, 0, 0)]
        );
        assert!(p.steps().iter().all(|t| t.secs >= 0.0));
        let slow = p.slowest(5);
        assert_eq!(slow.iter().map(|(s, n, _)| (*s, n.as_str())).collect::<Vec<_>>(), [(Step::Land, "alberta")]);
    }

    /// Items under way at once: each counted by its own parts, the first begun named with how many
    /// more are, each timed as it ends; a run counting as several items counts them all.
    #[test]
    fn items_under_way_at_once_count_apart() {
        let p = Progress::default();
        let n = Step::ALL.len() as f64;
        p.begin(Step::Water, 4);
        let (a, b) = (p.item("iceland"), p.item("faroe-islands"));
        a.parts(2);
        b.parts(4);
        drop(b.part_on_drop());
        drop(b.part_on_drop());
        assert_eq!(p.line(), "the water, 0 of 4 regions; iceland, 0 of 2; 1 more");
        assert!((p.share() - (3.0 + 0.5 / 4.0) / n).abs() < 1e-12);
        drop(a);
        assert_eq!(p.line(), "the water, 1 of 4 regions; faroe-islands, 2 of 4");
        let run = p.items("x, y", 2);
        drop(b);
        drop(run);
        assert_eq!(p.line(), "the water, 4 of 4 regions");
        let mut slow: Vec<String> = p.slowest(5).into_iter().map(|(_, n, _)| n).collect();
        slow.sort();
        assert_eq!(slow, ["faroe-islands", "iceland", "x, y"]);
        // An item of a step ended before it is counted in neither.
        let late = p.item("late");
        p.begin(Step::Lakes, 3);
        drop(late);
        assert_eq!(p.line(), "the lakes, 0 of 3 cells");
        assert_eq!(p.slowest(5).len(), 3);
    }
}
