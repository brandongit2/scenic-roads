//! The owner's asks by HTTP (docs/coordination.md §8, step 5 of task #878): a Mac's pause set from
//! another (`POST /pool/hold`) and the pool-wide settings (`POST /pool/settings`), kept by the lead
//! in its store (crate::pool::store: `hold/<member>`, `settings`), each with who set it and when.
//!
//! - **The lead** takes an ask only when it's the later (`at`) of its kind; a hold for a member and
//!   the settings' generation ride in each beat's answer (crate::coord::held::Answer), so a held
//!   member hears it within one beat, and a member fetches the settings when their generation moves.
//! - **A member's agent** queues its asks (`Ask`, crate::coord::beats::Beats::ask: kept on its disk,
//!   `QUEUE`) and its beat's poster sends them, in order, once the lead answers saying it takes them
//!   (`beats::ASKS`); an ask the lead hasn't answered stays queued, sent when it returns, and one it
//!   refuses as it is (400; in process, `Refused`) leaves the queue, said for the owner
//!   (crate::agent::answered). The lead's own agent hands its queue to its coordinator in process.
//!   (The NAS's files beside, for apps before, a hold's `state/pool/holds/<member>.json` and the
//!   settings' switch files, were deleted with their rollout gate, task #1036.)

use anyhow::{ensure, Result};
use jobrt::control::{Hold, HoldState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The member's queue of asks not yet answered 200, in its agent's pool folder.
pub const QUEUE: &str = "asks.json";

// The pool's settings, their names and an ask to set one: the runtime crate's (task #916 step 3,
// docs/crates.md: control queues a Mac's asks), kept at these paths.
pub use jobrt::settings::{
    Setting, SettingAsk, Settings, BATCH, NAMES, NAS_BUDGET, NAS_TARGET, OVERDUE, OVERDUE_DEFAULT, OVERDUE_RANGE,
    PLACEMENT,
};

/// Another Mac's pause set, or lifted (`held` None), as of `at` (`POST /pool/hold`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldAsk {
    pub member: String,
    #[serde(default)]
    pub held: Option<Hold>,
    pub at: u64,
}

/// An ask the lead refuses as it is (a hold for no member id, a setting it doesn't take or not of
/// its kind: an older lead's word on a newer Mac's ask, say): why. Asked again, it gets the same, so
/// it leaves the queue, said where the owner sees it (crate::agent::answered).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// An ask queued on a member until the lead answers it 200.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Ask {
    Hold(HoldAsk),
    Setting(SettingAsk),
}

impl Ask {
    /// Its route on the lead, and its body.
    pub fn post(&self) -> (&'static str, Value) {
        match self {
            Ask::Hold(h) => ("/pool/hold", serde_json::to_value(h).unwrap_or_default()),
            Ask::Setting(s) => ("/pool/settings", serde_json::to_value(s).unwrap_or_default()),
        }
    }

    /// What it's about, the same for every ask of it (crate::agent::answered's key): a member's
    /// hold (`hold <member>`), a setting (`setting <name>`).
    pub fn key(&self) -> String {
        match self {
            Ask::Hold(h) => format!("hold {}", h.member),
            Ask::Setting(s) => format!("setting {}", s.name),
        }
    }

    /// When it was asked.
    pub fn at(&self) -> u64 {
        match self {
            Ask::Hold(h) => h.at,
            Ask::Setting(s) => s.at,
        }
    }

    /// What it asks, in words: the member held named `name` (its host, when known).
    pub fn what(&self, name: &str) -> String {
        match self {
            Ask::Hold(HoldAsk { held: Some(h), .. }) => format!("pause {name} alone (by {})", h.by),
            Ask::Hold(HoldAsk { held: None, .. }) => format!("let {name} go on"),
            Ask::Setting(s) if s.value.is_null() => format!("clear the pool's {} (by {})", s.name, s.by),
            Ask::Setting(s) => format!("set the pool's {} to {} (by {})", s.name, s.value, s.by),
        }
    }
}

/// What the lead keeps of the asks: each member's hold, the settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lead {
    pub holds: BTreeMap<String, HoldState>,
    pub settings: Settings,
}

impl Lead {
    /// As its store's documents hold them.
    pub fn of(docs: &crate::pool::store::Docs) -> Lead {
        let holds = docs
            .range("hold/".to_string()..)
            .take_while(|(k, _)| k.starts_with("hold/"))
            .filter_map(|(k, v)| Some((k["hold/".len()..].to_string(), serde_json::from_value(v.clone()).ok()?)))
            .collect();
        let settings = docs.get("settings").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        Lead { holds, settings }
    }

    /// Takes hold ask `a` when it's no earlier than the member's hold kept and changes it: the hold
    /// as it stands, and the document to keep when it changed. Times are whole seconds, so two asks
    /// in one second (a pause and a resume clicked quickly) tie: the one arriving later wins, kept a
    /// second on so the member, which takes only a later hold, takes it too. (A repeat, an ask sent
    /// again after a lost answer, changes nothing.)
    pub fn hold(&mut self, a: &HoldAsk) -> Result<(HoldState, Option<(String, Value)>)> {
        ensure!(crate::pool::is_member_id(&a.member), "{:?} isn't a member id", a.member);
        let was = self.holds.get(&a.member).cloned().unwrap_or_default();
        if a.at < was.at || (a.held == was.held && was.at > 0) {
            return Ok((was, None));
        }
        let st = HoldState { held: a.held.clone(), at: a.at.max(was.at + 1) };
        self.holds.insert(a.member.clone(), st.clone());
        let doc = serde_json::to_value(&st)?;
        Ok((st, Some((format!("hold/{}", a.member), doc))))
    }

    /// Takes setting ask `a` (`Settings::take`): the document to keep when it changed them.
    pub fn set(&mut self, a: &SettingAsk) -> Result<Option<(String, Value)>> {
        a.check()?;
        if !self.settings.take(a) {
            return Ok(None);
        }
        Ok(Some(("settings".to_string(), serde_json::to_value(&self.settings)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: &str = "m-000000000000000a";

    #[test]
    fn the_later_ask_wins_and_each_change_moves_the_generation() {
        let mut l = Lead::default();
        let held = Some(Hold { by: "the owner".into(), at: 10 });
        let (st, doc) = l.hold(&HoldAsk { member: M.into(), held: held.clone(), at: 10 }).unwrap();
        assert_eq!((st.held.clone(), doc.is_some()), (held.clone(), true));
        let (st, doc) = l.hold(&HoldAsk { member: M.into(), held: None, at: 9 }).unwrap();
        assert_eq!((st.held, doc), (held.clone(), None), "an earlier ask doesn't undo it");
        let (st, doc) = l.hold(&HoldAsk { member: M.into(), held: held.clone(), at: 10 }).unwrap();
        assert_eq!((st.at, doc), (10, None), "a repeat changes nothing");
        let (st, doc) = l.hold(&HoldAsk { member: M.into(), held: None, at: 10 }).unwrap();
        assert_eq!((st.held, st.at, doc.is_some()), (None, 11, true), "a tie: the later arrival, a second on");
        l.hold(&HoldAsk { member: M.into(), held: held.clone(), at: 12 }).unwrap();
        assert!(l.hold(&HoldAsk { member: "x".into(), held: None, at: 1 }).is_err());
        let ask = |v: Value, at| SettingAsk { name: PLACEMENT.into(), value: v, by: "t".into(), at };
        assert!(l.set(&ask(Value::Bool(true), 5)).unwrap().is_some());
        assert_eq!((l.settings.gen, l.settings.placement()), (1, true));
        assert!(l.set(&ask(Value::Bool(false), 4)).unwrap().is_none(), "earlier");
        assert!(l.set(&ask(Value::Bool(true), 6)).unwrap().is_none(), "the same");
        assert!(l.set(&ask(Value::from(3), 7)).is_err(), "not of its kind");
        assert!(l.set(&SettingAsk { name: "x".into(), value: Value::Null, by: "t".into(), at: 8 }).is_err());
        let t = SettingAsk { name: NAS_TARGET.into(), value: Value::from(70), by: "t".into(), at: 8 };
        assert!(l.set(&t).unwrap().is_some());
        assert_eq!(l.settings.nas_target().unwrap().pct, 70);
        assert!(l.set(&SettingAsk { value: Value::from(100), at: 9, ..t }).is_err(), "never 100 %");
        // (Task #927: the batch length, in minutes, a minute to a day.)
        let b = SettingAsk { name: BATCH.into(), value: Value::from(20), by: "t".into(), at: 8 };
        assert_eq!(l.settings.batch_s(), None);
        assert!(l.set(&b).unwrap().is_some());
        assert_eq!(l.settings.batch_s(), Some(1200));
        assert!(l.set(&SettingAsk { value: Value::from(0), at: 9, ..b.clone() }).is_err());
        assert!(l.set(&SettingAsk { value: Value::from("20"), at: 9, ..b }).is_err(), "a number");
        // The overdue factor (task #953): a number past 1, the default until set.
        assert_eq!(l.settings.overdue(), (OVERDUE_DEFAULT, false));
        let o = SettingAsk { name: OVERDUE.into(), value: Value::from(4.5), by: "t".into(), at: 9 };
        assert!(l.set(&o).unwrap().is_some());
        assert_eq!(l.settings.overdue(), (4.5, true));
        for bad in [Value::from(1), Value::from(0.5), Value::from(1000), Value::from("3")] {
            assert!(l.set(&SettingAsk { value: bad.clone(), at: 10, ..o.clone() }).is_err(), "{bad}");
        }
        // (Read back as the store keeps them.)
        let mut docs = crate::pool::store::Docs::new();
        docs.insert(format!("hold/{M}"), serde_json::to_value(&l.holds[M]).unwrap());
        docs.insert("settings".into(), serde_json::to_value(&l.settings).unwrap());
        assert_eq!(Lead::of(&docs), l);
    }
}
