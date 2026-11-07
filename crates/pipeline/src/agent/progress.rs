//! How far the build is, as the status carries it (docs/building.md §2, docs/pipeline.md §4): the
//! checklist's lines, a line per stage (`Line`) with an item per step (`Item`), which the plan makes
//! (crate::agent::build::fold) and the agent says and marks under way; and each region's counts
//! (`RegionState`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How far each region is built: of the z6 tiles its outlines meet, how many are built as the whole
/// coverage now wants them.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RegionState {
    pub built: usize,
    pub total: usize,
}

/// One step's line of the build's checklist: its targets done of all (`total` None until an earlier
/// step makes them known), or how many are left (a worldwide step's: 1 or 0; a chain's: the targets
/// of its works to come). Neither: to come, not sized yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub step: String,
    /// The noun its targets are counted in (crate::agent::steps), filled as the lines are folded:
    /// readers without the table (the menu bar) say it so.
    #[serde(default)]
    pub noun: String,
    #[serde(default)]
    pub done: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<usize>,
    /// Of `done`, the targets that made nothing to publish (task #839: a deep tile with no fine
    /// data there; crate::agent::steps::output_named), counted apart; 0 left out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub empty: usize,
    /// Its count in words (`count`) and whether it's done, as the status serves them (`said`,
    /// task #846): what the menu bar and the build page print.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub count: String,
    #[serde(default)]
    pub finished: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// "12,345": a count as every surface says it (docs/pipeline.md §2).
fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "{done} of {total} {noun}s", and the done that made nothing to publish apart.
fn of_total(done: usize, total: usize, empty: usize, nouns: &str) -> String {
    let none = if empty > 0 { format!(" ({} with nothing to publish)", grouped(empty)) } else { String::new() };
    format!("{} of {} {nouns}{none}", grouped(done), grouped(total))
}

impl Item {
    pub(super) fn sized(step: &str, done: usize, total: Option<usize>) -> Item {
        Item { step: step.into(), done, total, left: total.map(|t| t.saturating_sub(done)), ..Default::default() }
    }

    pub(super) fn left(step: &str, left: Option<usize>) -> Item {
        Item { step: step.into(), left, ..Default::default() }
    }

    /// Whether it's sized: its targets known.
    pub fn known(&self) -> bool {
        self.left.is_some()
    }

    pub fn finished(&self) -> bool {
        self.left == Some(0)
    }

    /// Its count in words: "156 of 284 z6 tiles", "3 runs left", "done", "to come".
    pub fn count(&self, the_plan: &dyn crate::agent::seam::Plan) -> String {
        let noun = if self.noun.is_empty() { the_plan.noun(&self.step) } else { self.noun.as_str() };
        match (self.total, self.left) {
            (Some(t), _) if t > 0 => of_total(self.done, t, self.empty, &format!("{noun}s")),
            (_, Some(0)) => "done".into(),
            (_, Some(l)) => format!("{} {noun}{} left", grouped(l), if l == 1 { "" } else { "s" }),
            _ => "to come".into(),
        }
    }
}

/// One stage's line of the build's checklist (the status, the menu bar, the build page): its steps'
/// items, summed. `done` and `total` count the targets of the items sharing the line's noun
/// (`unit`: the first sized item's; the other nouns' items are said apart, `counts`), `left` every
/// item's targets left; `total` and `left` None while an item of it isn't sized yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Line {
    /// The stage's id (crate::agent::steps::STAGES).
    #[serde(default)]
    pub id: String,
    /// The stage's title.
    pub what: String,
    /// The stage's steps (a job's id starts with one: "unit 6/31/20").
    pub steps: Vec<String>,
    #[serde(default)]
    pub done: usize,
    #[serde(default)]
    pub total: Option<usize>,
    #[serde(default)]
    pub left: Option<usize>,
    /// The noun its targets are counted in, plural ("tiles").
    #[serde(default)]
    pub unit: String,
    /// Its steps' lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    /// The jobs still to do, by name, in the order they'll run (a step's runs together, its targets
    /// counted: "peaks: 178 z6 tiles"); the first is the one under way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// While it has work left and isn't this Mac's job now: why (another Mac is on it, it waits
    /// for the home network or out a failure, for the steps above). The pass's: the countries it
    /// kept as the previous pass's outlines, done or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Which of its steps a helper may do (crate::agent::steps::SHARED, `mark_shared`): "all", or
    /// the parts by their targets' noun ("candidates and peaks"); none, the lead's alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<String>,
    /// Its counts in words (`counts`) and whether it's done, as the status serves them (`said`,
    /// task #846): what the menu bar and the build page print.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counts: Vec<String>,
    #[serde(default)]
    pub finished: bool,
}

impl Line {
    /// Its counts and its items' said in its fields (`counts`, `finished`), as the status serves
    /// them: what every surface prints.
    pub fn said(mut self, the_plan: &dyn crate::agent::seam::Plan) -> Line {
        for i in self.items.iter_mut() {
            i.count = i.count(the_plan);
            i.finished = i.finished();
        }
        self.counts = self.counts(the_plan);
        self.finished = self.finished();
        self
    }

    pub fn finished(&self) -> bool {
        self.left == Some(0) || self.total.is_some_and(|t| self.done >= t && self.left.is_none())
    }

    /// `step`'s item under way (a job of it running, on any Mac): never done while it runs. Bridges
    /// the checklist being up to five minutes old (it's made at a job's end or every five minutes,
    /// before the job started). One target counted left, the line's sums with it.
    pub fn under_way(&mut self, step: &str) {
        let noun = self.unit.trim_end_matches('s').to_string();
        let Some(i) = self.items.iter_mut().find(|i| i.step == step && i.finished()) else { return };
        i.left = Some(1);
        if let Some(t) = i.total.filter(|&t| t > 0 && i.done >= t) {
            i.done = t - 1;
            if i.noun.is_empty() || i.noun == noun {
                self.done = self.done.saturating_sub(1);
            }
        }
        self.left = self.left.map(|l| l + 1);
    }

    /// Its counts in words, a noun at a time in its items' order: "398 of 398 tiles", "18 of 18
    /// z3 tiles", "2 runs left"; "done" when nothing is left, "to come" while it isn't sized. (The
    /// line's own `done`/`total` are the first noun's: what readers of the line alone show.)
    pub fn counts(&self, the_plan: &dyn crate::agent::seam::Plan) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut nouns: Vec<&str> = Vec::new();
        for i in &self.items {
            let noun = the_plan.noun(&i.step);
            if nouns.contains(&noun) {
                continue;
            }
            nouns.push(noun);
            let of: Vec<&Item> = self.items.iter().filter(|j| the_plan.noun(&j.step) == noun).collect();
            if of.iter().any(|j| j.total.is_some()) {
                if of.iter().all(|j| j.total.is_some()) {
                    let (d, t): (usize, usize) =
                        (of.iter().map(|j| j.done).sum(), of.iter().map(|j| j.total.unwrap_or(0)).sum());
                    let e: usize = of.iter().map(|j| j.empty).sum();
                    // (A noun past the line's own with nothing to do isn't said.)
                    if t > 0 || out.is_empty() {
                        out.push(of_total(d, t, e, &format!("{noun}s")));
                    }
                } else {
                    out.push(format!("{noun}s to come"));
                }
            } else if of.iter().all(|j| j.left.is_some()) {
                let l: usize = of.iter().map(|j| j.left.unwrap_or(0)).sum();
                if l > 0 {
                    out.push(format!("{l} {noun}{} left", if l == 1 { "" } else { "s" }));
                }
            } else {
                out.push(format!("{noun}s to come"));
            }
        }
        if out.is_empty() {
            out.push(if self.finished() { "done".into() } else { "to come".into() });
        }
        out
    }
}

/// The regions' steps' checklist: its items, and each stage's jobs to come (`checklist`).
pub type Checklist = (Vec<Item>, BTreeMap<&'static str, Vec<String>>);
