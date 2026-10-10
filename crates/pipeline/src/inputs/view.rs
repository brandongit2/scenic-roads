//! The gate in the status (docs/inputs.md §4.7): an entry per gate unit, from the records (its
//! accepted index and held report), its acceptances, and whether a check of it is planned or runs.

use super::{Finding, Index, Report};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// The flagged lines a status shows of a finding (all are in the report).
pub const LINES_SHOWN: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Its accepted version is the drop box's.
    #[default]
    Ok,
    /// A check of it is granted or running.
    Checking,
    /// A change of it is held.
    Held,
}

/// A finding as the status shows it: its first `LINES_SHOWN` lines, and how many more the report
/// has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    #[serde(flatten)]
    pub finding: Finding,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A gate unit in the status.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InputView {
    pub unit: String,
    /// Its accepted index's content name (its version), once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub state: State,
    /// When it was last checked (its report's or index's time on the NAS), once it has been.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<u64>,
    /// The changes held, and the findings holding them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Shown>,
    /// Why every change was held together (the next version checked whole).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub together: Option<String>,
    /// The ids accepted, and of them those no check raises any more (`scenic inputs unaccept
    /// --stale` removes them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    /// Why the unit can't be shown as it is (its report or index unreadable now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<String>,
}

impl InputView {
    /// Its warnings held (what Accept All accepts).
    pub fn warnings(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().map(|s| &s.finding).filter(|f| f.level == super::Level::Warning)
    }

    /// The unit in a line ("Regions: 1 warning held, 1 error").
    pub fn line(&self) -> String {
        let (e, w) = self.findings.iter().fold((0, 0), |(e, w), s| if s.finding.level == super::Level::Error { (e + 1, w) } else { (e, w + 1) });
        let n = |k: usize, one: &str| format!("{k} {one}{}", if k == 1 { "" } else { "s" });
        match self.state {
            State::Held => {
                let mut parts = Vec::new();
                if w > 0 {
                    parts.push(format!("{} held", n(w, "warning")));
                }
                if e > 0 {
                    parts.push(n(e, "error"));
                }
                format!("{}: {} ({})", self.unit, if parts.is_empty() { "held".to_string() } else { parts.join(", ") }, self.held.join(", "))
            }
            State::Checking => format!("{}: checking", self.unit),
            State::Ok => format!("{}: taken in", self.unit),
        }
    }
}

/// What the views read, kept by content name (content-named files never change).
#[derive(Default)]
pub struct Cache {
    reports: HashMap<String, Report>,
    indexes: HashMap<String, Index>,
    times: HashMap<String, u64>,
}

impl Cache {
    fn report(&mut self, root: &Path, name: &str) -> anyhow::Result<Report> {
        if let Some(r) = self.reports.get(name) {
            return Ok(r.clone());
        }
        let r = super::read_report(root, name)?;
        self.reports.insert(name.to_string(), r.clone());
        Ok(r)
    }

    fn index(&mut self, root: &Path, name: &str) -> anyhow::Result<Index> {
        if let Some(i) = self.indexes.get(name) {
            return Ok(i.clone());
        }
        let i = super::read_index(root, name)?;
        self.indexes.insert(name.to_string(), i.clone());
        Ok(i)
    }

    fn time(&mut self, root: &Path, name: &str) -> Option<u64> {
        if let Some(&t) = self.times.get(name) {
            return Some(t);
        }
        let t = std::fs::metadata(root.join(name)).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        self.times.insert(name.to_string(), t);
        Some(t)
    }
}

/// The units on the gate (`units`) and any other the records name, each as the records `manifest`
/// have it; `checking`, those with a check planned or running; `accepted`, each unit's
/// acceptances (listed here for one missing).
pub fn of(root: &Path, manifest: &BTreeMap<String, String>, units: &[&str], checking: &BTreeSet<String>, accepted: &BTreeMap<String, BTreeSet<String>>, cache: &mut Cache) -> Vec<InputView> {
    let mut all: BTreeSet<String> = units.iter().map(|u| u.to_string()).collect();
    all.extend(manifest.range("sources/inputs/".to_string()..).take_while(|(l, _)| l.starts_with("sources/inputs/")).filter_map(|(l, _)| super::unit_of(l).map(str::to_string)));
    let mut out = Vec::new();
    for unit in all {
        let mut v = InputView { unit: unit.clone(), version: manifest.get(&super::logical(&unit)).cloned(), ..Default::default() };
        let acc = match accepted.get(&unit) {
            Some(a) => a.clone(),
            None => super::acceptances(root, &unit).unwrap_or_default(),
        };
        let mut raised: BTreeSet<String> = BTreeSet::new();
        if let Some(name) = manifest.get(&super::held_logical(&unit)) {
            match cache.report(root, name) {
                Ok(r) => {
                    v.state = State::Held;
                    v.held = r.held.clone();
                    v.together = r.together.clone();
                    v.findings = r
                        .findings
                        .iter()
                        .map(|f| {
                            let mut f = f.clone();
                            let more = f.lines.len().saturating_sub(LINES_SHOWN);
                            f.lines.truncate(LINES_SHOWN);
                            Shown { finding: f, more }
                        })
                        .collect();
                    raised.extend(r.raised.iter().cloned());
                }
                Err(e) => v.unread = Some(format!("{e:#}")),
            }
            v.checked = cache.time(root, name);
        }
        if let Some(name) = v.version.clone() {
            match cache.index(root, &name) {
                Ok(i) => raised.extend(i.accepted.iter().cloned()),
                Err(e) => v.unread = Some(format!("{e:#}")),
            }
            if v.checked.is_none() {
                v.checked = cache.time(root, &name);
            }
        }
        if checking.contains(&unit) {
            v.state = State::Checking;
        }
        v.accepted = acc.iter().cloned().collect();
        // (Stale only once a check has said: a unit never checked raises nothing yet.)
        if v.checked.is_some() && v.unread.is_none() {
            v.stale = acc.iter().filter(|id| !raised.contains(*id)).cloned().collect();
        }
        out.push(v);
    }
    out
}
