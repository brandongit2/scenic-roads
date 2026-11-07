//! The plan's side of the seam (docs/crates.md §6.3, 6f2): the runtime's questions (seam::Plan)
//! answered from the steps table and build.rs. The binaries hand `the_plan()` to the runtime.

use super::build;
use super::forecast::ReadTarget;
use super::jobs::Work;
use super::progress::{Item, Line};
use super::seam::{Along, Left, Plan};
use super::steps::{self, Kind, Read};
use std::collections::BTreeMap;

/// The plan, as the runtime holds it.
pub fn the_plan() -> &'static dyn Plan {
    &ThePlan
}

struct ThePlan;

impl Plan for ThePlan {
    fn step_names(&self) -> Vec<&'static str> {
        steps::TABLE.iter().map(|r| r.name).collect()
    }
    fn is_step(&self, step: &str) -> bool {
        steps::row(step).is_some()
    }
    fn shared(&self) -> &'static [&'static str] {
        &steps::SHARED
    }
    fn answered(&self, step: &str) -> bool {
        steps::ANSWERED.contains(&step)
    }
    fn alone(&self, step: &str) -> bool {
        steps::alone(step)
    }
    fn records_only(&self, step: &str) -> bool {
        steps::records_only(step)
    }
    fn fetches(&self, step: &str) -> bool {
        steps::row(step).is_some_and(|r| r.kind == Kind::Fetch)
    }
    fn cost_version(&self, step: &str) -> u32 {
        steps::row(step).map_or(0, |s| s.cost_v)
    }
    fn sources_naming(&self, step: &str, target: &str) -> Vec<&'static str> {
        let reads = steps::row(step).map_or(&[][..], |r| r.reads);
        reads
            .iter()
            .filter_map(|r| match r {
                Read::Targets { step: src, of, .. } if of(target).iter().any(|t| t == target) => Some(*src),
                _ => None,
            })
            .collect()
    }
    fn noun(&self, step: &str) -> &'static str {
        steps::noun(step)
    }
    fn said(&self, step: &str, n: usize) -> String {
        steps::said(step, n)
    }
    fn task_kinds(&self) -> Vec<&'static str> {
        steps::TASKS.iter().map(|t| t.name).collect()
    }
    fn task_disk(&self, kind: &str) -> Option<u64> {
        steps::task(kind).map(|t| t.disk)
    }
    fn is_none_key(&self, step: &str, t: &str, key: &str) -> bool {
        build::is_none_key(step, t, key)
    }
    fn output_named(&self, step: &str, target: &str, m: &BTreeMap<String, String>) -> Option<bool> {
        steps::output_named(step, target, m)
    }
    fn output_names(&self, step: &str, target: &str) -> Option<Vec<String>> {
        steps::output_names(step, target)
    }
    fn saves(&self, step: &str, target: &str, l: &str) -> bool {
        steps::saves(step, target, l)
    }
    fn outside(&self, e: &crate::pool::journal::Entry) -> Option<String> {
        steps::outside(e)
    }
    fn outside_by(
        &self,
        e: &crate::pool::journal::Entry,
        writes: &dyn Fn(&str, bool) -> Option<bool>,
    ) -> Option<String> {
        steps::outside_by(e, writes)
    }
    fn read_steps(&self, step: &str) -> Vec<(&'static str, bool)> {
        steps::read_steps(step)
    }
    fn read_targets_passing(&self, step: &str, t: &str, along: Option<&Along>) -> Vec<(ReadTarget, bool)> {
        steps::read_targets_passing(step, t, along)
    }
    fn input_readers(&self, name: &str) -> Vec<&'static str> {
        steps::input_readers(name).collect()
    }
    fn summits_waits(&self, left: &Left) -> Option<String> {
        match steps::readiness("summits", "summits", left) {
            steps::Ready::Waits(w) => Some(steps::wait_note(&w)),
            _ => None,
        }
    }
    fn table_json(&self) -> serde_json::Value {
        serde_json::json!({ "steps": steps::table(), "stages": steps::STAGES })
    }
    fn reads_sources(&self, step: &str) -> bool {
        build::reads_sources(step)
    }
    fn reads_lakefill(&self, step: &str) -> bool {
        build::reads_lakefill(step)
    }
    fn catalogue_waits_for(&self, step: &str) -> bool {
        build::CATALOGUE_GATE.contains(&step)
    }
    fn job_of(&self, w: Work) -> Work {
        build::job_of(w)
    }
    fn worldwide_steps(&self) -> &'static [&'static str] {
        &build::WORLDWIDE_STEPS
    }
    fn fold(&self, items: &[Item], next: &BTreeMap<&str, Vec<String>>, notes: &BTreeMap<&str, String>) -> Vec<Line> {
        build::fold(items, next, notes)
    }
    fn checklist_to_come(&self) -> Vec<Item> {
        build::checklist_to_come()
    }
}
