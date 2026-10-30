//! The fields of a member's heartbeat the pool's protocol reads and writes (docs/pool.md §10): who
//! it is, when it beat, the term it leads, and where to reach it. A struct of their own, every field defaulted, to embed in the agent's heartbeat
//! (`state/pool/members/<id>.json`, written whole by its member alone; read by member id, never by
//! listing).

use super::nas::Nas;
use super::Member;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A heartbeat older than this by the reader's clock is out of touch (§10).
pub const OUT_OF_TOUCH_S: u64 = 600;
/// A member writes its heartbeat at least this often (s; crate::agent::pool).
pub const BEAT_EVERY_S: u64 = 120;
/// A heartbeat this far ahead of the reader's clock is shown as "clock wrong" (§6.7).
pub const AHEAD_S: u64 = 60;

/// The pool's fields of a heartbeat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beat {
    /// The member's id.
    pub member: String,
    /// Its host name: a label.
    pub host: String,
    /// The app it runs.
    pub app: String,
    /// When it beat: unix seconds, by its own clock.
    pub beat: u64,
    /// The term it leads, while it leads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leads: Option<u64>,
    /// Where it answers the pool's API: Tailscale's addresses, then the LAN's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    /// Whom it reached lately, and whom it has failed to reach, and why (crate::pool::reachable):
    /// the lead's coordinator and the other members' brokers. Written by the agent, not the driver.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reach: Vec<super::reachable::Reach>,
    /// Its app keeps the journal's GC fence (crate::pool::journal_gc::FENCE; 0: an app from before,
    /// which takes up from whatever records it reads): no entry is removed while one doesn't.
    pub journal: u32,
    /// Its take-ups of the term naming it refused by the journal GC's marker (docs/pool.md §7.3):
    /// since when, and why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fenced: Option<Fenced>,
    /// Its app's readers read the build's records from the term's snapshot (§6.2:
    /// crate::pool::records::Records::now), not today's files: the lead stops writing those once
    /// every member's heartbeat says so (crate::agent::pool::Run::keep_today). Left out by apps
    /// before it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reads_records: bool,
}

/// Take-ups refused by the journal GC's marker (crate::pool::journal_gc::Fenced).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fenced {
    /// The first refused, by the member's clock.
    pub since: u64,
    pub why: String,
}

/// The path of member `member`'s heartbeat.
pub fn path(member: &str) -> String {
    format!("state/pool/members/{member}.json")
}

impl Beat {
    /// Member `member`'s heartbeat, its pool's fields; None when there's none, or it can't be parsed
    /// (being replaced: read again next loop).
    /// This Mac's own heartbeat as the server has it (`Nas::read_fresh`): whether its writes land.
    pub fn read_fresh(nas: &dyn Nas, member: &str) -> Result<Option<Beat>> {
        Ok(nas.read_fresh(&path(member))?.and_then(|b| serde_json::from_slice(&b).ok()))
    }

    pub fn read(nas: &dyn Nas, member: &str) -> Result<Option<Beat>> {
        Ok(nas.read(&path(member))?.and_then(|b| serde_json::from_slice(&b).ok()))
    }

    /// Writes it whole as its member's heartbeat (the agent's own heartbeat embeds these fields
    /// and writes itself instead).
    pub fn write(&self, nas: &dyn Nas) -> Result<()> {
        nas.write_whole(&path(&self.member), &serde_json::to_vec(self)?)
    }

    /// Its member.
    pub fn member(&self) -> Member {
        Member { id: self.member.clone(), host: self.host.clone(), app: self.app.clone() }
    }

    /// Whether it's out of touch at the reader's `now` (§10): its beat over ten minutes old.
    pub fn out_of_touch(&self, now: u64) -> bool {
        now > self.beat.saturating_add(OUT_OF_TOUCH_S)
    }

    /// Whether its member's clock is wrong by the reader's `now` (§6.7): over a minute ahead.
    pub fn clock_wrong(&self, now: u64) -> bool {
        self.beat > now.saturating_add(AHEAD_S)
    }
}

#[cfg(test)]
mod tests {
    use super::super::nas::Mem;
    use super::*;

    #[test]
    fn a_heartbeat_reads_back_with_its_fields_defaulted() {
        let b: Beat =
            serde_json::from_str(r#"{"member": "m-000000000000000a", "beat": 1000, "slots": [1, 2]}"#).unwrap();
        assert_eq!((b.member.as_str(), b.beat, b.leads), ("m-000000000000000a", 1000, None));
        assert!(!serde_json::to_string(&b).unwrap().contains("leads"), "what isn't so isn't written");
        let nas = Mem::default();
        let h = Beat { leads: Some(4), ..b.clone() };
        h.write(&nas).unwrap();
        assert!(String::from_utf8(nas.read(&path("m-000000000000000a")).unwrap().unwrap())
            .unwrap()
            .contains("\"leads\":4"));
        assert_eq!(Beat::read(&nas, "m-000000000000000a").unwrap(), Some(h));
        assert_eq!(Beat::read(&nas, "m-000000000000000b").unwrap(), None);
        // Out of touch after ten minutes by the reader's clock; a clock over a minute ahead, wrong.
        assert!(!b.out_of_touch(1600) && b.out_of_touch(1601));
        assert!(!b.clock_wrong(940) && b.clock_wrong(939));
    }
}
