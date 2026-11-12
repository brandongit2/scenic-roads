//! The pool's settings as the lead keeps them (`/pool/settings`, docs/coordination.md §8), and an
//! ask to set one (`SettingAsk`), which a Mac's agent queues (crate::control) and its coordinator
//! sends (pipeline's coord::asks, which re-exports these).

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The settings the lead keeps (`/pool/settings`), once each a switch file under `state/pool/`:
/// placement by how crucial a job is (pipeline's
/// agent::placement; a boolean), the NAS's budget (crate::budget::nas's switch: its text) and the
/// NAS's load target (crate::budget::nas::Target's share, %); and, with no switch file (an older lead
/// refuses them): the batch length (task #927: the minutes of measured time a job of a shared step is
/// sized to, pipeline's coord::sched::Sched::batch_len) and the factor past which a job's
/// stillness or slowness, or a catalogue's wait, is flagged (`OVERDUE`); and two of publishing's
/// switches, booleans read by the lead's plan (agentcore's agent::plan): `HOLD_CATALOG` and
/// `NEAR_SLOPE`.
pub const PLACEMENT: &str = "placement";
pub const NAS_BUDGET: &str = "nas-budget";
pub const NAS_TARGET: &str = "nas-target";
pub const BATCH: &str = "batch-minutes";
/// How many times its usual still stretch a job's work goes without moving before the status flags
/// it stuck, how many times slower than its step's slowest tenth of jobs its progress goes on
/// before it's slow, and a catalogue due its most wait before it's stale (pipeline's agent::flags,
/// tasks #953, #986 and #990): a number in `OVERDUE_RANGE`; not set, `OVERDUE_DEFAULT`.
pub const OVERDUE: &str = "overdue-factor";
/// Publishing held for review (docs/building.md §2, Publishing): the `catalog` step writes the map
/// it would publish to the lead's disk instead (`catalog-held`), and nothing is published. Off
/// while not set.
pub const HOLD_CATALOG: &str = "hold-catalog";
/// A done region's slope and tree cover by the z6 tiles near it (agentcore's agent::publish::
/// Publishing::near). Off while not set.
pub const NEAR_SLOPE: &str = "near-slope";
pub const NAMES: [&str; 7] = [PLACEMENT, NAS_BUDGET, NAS_TARGET, BATCH, OVERDUE, HOLD_CATALOG, NEAR_SLOPE];
/// The settings that are on or off (a boolean; not set: off).
pub const SWITCHES: [&str; 3] = [PLACEMENT, HOLD_CATALOG, NEAR_SLOPE];
/// The batch lengths the lead takes (minutes): a minute to a day.
pub const BATCH_RANGE: std::ops::RangeInclusive<u64> = 1..=1440;

/// The overdue factor when none is set: the owner's "about 3x" (4 Nov).
pub const OVERDUE_DEFAULT: f64 = 3.0;
/// The factors the lead takes: past the usual time at least, and short of never.
pub const OVERDUE_RANGE: std::ops::RangeInclusive<f64> = 1.1..=100.0;

/// A setting as the lead keeps it: its value (null: cleared, its default), who set it, when (unix s).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub at: u64,
}

/// The settings (the store's `settings`), and their generation: one more at each change.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub gen: u64,
    #[serde(default)]
    pub values: BTreeMap<String, Setting>,
}

impl Settings {
    /// Setting `name`'s value, when one is set.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name).map(|s| &s.value).filter(|v| !v.is_null())
    }

    /// A switch (`SWITCHES`), as set (off while not).
    pub fn on(&self, name: &str) -> bool {
        self.get(name).and_then(Value::as_bool).unwrap_or(false)
    }

    /// Placement, as set (off while not).
    pub fn placement(&self) -> bool {
        self.on(PLACEMENT)
    }

    /// The NAS budget's switch, as set (its text, `budget::nas::parse`; not set: on, no cap).
    pub fn nas_budget(&self) -> Option<crate::budget::nas::Switch> {
        match self.get(NAS_BUDGET).and_then(Value::as_str) {
            Some(t) => crate::budget::nas::parse(t),
            None => Some(Default::default()),
        }
    }

    /// The NAS's load target, as set.
    pub fn nas_target(&self) -> Option<crate::budget::nas::Target> {
        let s = self.values.get(NAS_TARGET)?;
        let pct = s.value.as_u64()? as u32;
        Some(crate::budget::nas::Target { pct, by: s.by.clone(), at: s.at })
    }

    /// The batch length, as set (s).
    pub fn batch_s(&self) -> Option<u64> {
        self.get(BATCH).and_then(Value::as_u64).map(|m| m * 60)
    }

    /// The overdue factor, and whether it was set (else `OVERDUE_DEFAULT`).
    pub fn overdue(&self) -> (f64, bool) {
        match self.get(OVERDUE).and_then(Value::as_f64).filter(|f| OVERDUE_RANGE.contains(f)) {
            Some(f) => (f, true),
            None => (OVERDUE_DEFAULT, false),
        }
    }

    /// Takes `a` when it's no earlier than the setting it replaces: whether it changed them.
    pub fn take(&mut self, a: &SettingAsk) -> bool {
        if self.values.get(&a.name).is_some_and(|s| s.at > a.at || s.value == a.value) {
            return false;
        }
        self.values.insert(a.name.clone(), Setting { value: a.value.clone(), by: a.by.clone(), at: a.at });
        self.gen += 1;
        true
    }
}

/// A setting set (`value`), or cleared (null), by `by` at `at` (`POST /pool/settings`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingAsk {
    pub name: String,
    #[serde(default)]
    pub value: Value,
    pub by: String,
    pub at: u64,
}

impl SettingAsk {
    /// Whether the lead takes it: a setting it knows, a value of its kind.
    pub fn check(&self) -> Result<()> {
        let ok = match (self.name.as_str(), &self.value) {
            (_, Value::Null) => NAMES.contains(&self.name.as_str()),
            (n, Value::Bool(_)) => SWITCHES.contains(&n),
            (NAS_BUDGET, Value::String(t)) => t.len() <= 200,
            (NAS_TARGET, Value::Number(n)) => {
                n.as_u64().is_some_and(|p| crate::budget::nas::TARGET_RANGE.contains(&(p as u32)))
            }
            (BATCH, Value::Number(n)) => n.as_u64().is_some_and(|m| BATCH_RANGE.contains(&m)),
            (OVERDUE, Value::Number(n)) => n.as_f64().is_some_and(|f| OVERDUE_RANGE.contains(&f)),
            _ => false,
        };
        ensure!(ok, "{}: not a setting the lead takes, or not of its kind ({})", self.name, self.value);
        ensure!(self.by.len() <= 200, "who set it, in 200 bytes");
        Ok(())
    }
}
