//! The fields of a member's heartbeat the pool's protocol reads and writes (docs/pool.md §10): who
//! it is, when it beat, the term it leads, a handover's state on either side, and where to reach
//! it. A struct of their own, every field defaulted, to embed in the agent's heartbeat
//! (`state/pool/members/<id>.json`, written whole by its member alone; read by member id, never by
//! listing).

use super::nas::Nas;
use super::Member;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A heartbeat older than this by the reader's clock is out of touch (§6.5): its member may be
/// taken over from.
pub const OUT_OF_TOUCH_S: u64 = 600;
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
    /// The handover it's making, as a lead (and once it's passed, until it's over).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handing_to: Option<HandingTo>,
    /// The term it's ready to lead, answering a lead's offer (crate::pool::handover::ready_for).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_for: Option<u64>,
    /// Where it answers the pool's API: Tailscale's addresses, then the LAN's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
}

/// A handover under way, as its lead's heartbeat says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandingTo {
    /// The member it's handed to.
    pub to: String,
    /// The term handed over (the target leads the next).
    pub term: u64,
    /// Since when it's at this stage (the lead's clock).
    pub since: u64,
    /// Where it stands: offered, settling, or passed.
    pub stage: Stage,
}

/// A handover's stage, for the views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// Asked: waiting for the target's `ready_for`.
    Offered,
    /// The target is ready: the lead grants nothing new and merges what waits.
    Settling,
    /// The next term is made naming the target: waiting for it to lead.
    Passed,
}

/// The path of member `member`'s heartbeat.
pub fn path(member: &str) -> String {
    format!("state/pool/members/{member}.json")
}

impl Beat {
    /// Member `member`'s heartbeat, its pool's fields; None when there's none, or it can't be parsed
    /// (being replaced: read again next loop).
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

    /// Whether it's out of touch at the reader's `now` (§6.5): its beat over ten minutes old.
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
        let b: Beat = serde_json::from_str(r#"{"member": "m-000000000000000a", "beat": 1000, "slots": [1, 2]}"#).unwrap();
        assert_eq!((b.member.as_str(), b.beat, b.leads, b.ready_for), ("m-000000000000000a", 1000, None, None));
        assert!(!serde_json::to_string(&b).unwrap().contains("leads"), "what isn't so isn't written");
        let nas = Mem::default();
        let h = Beat { leads: Some(4), handing_to: Some(HandingTo { to: "m-000000000000000b".into(), term: 4, since: 990, stage: Stage::Offered }), ..b.clone() };
        h.write(&nas).unwrap();
        assert!(String::from_utf8(nas.read(&path("m-000000000000000a")).unwrap().unwrap()).unwrap().contains("\"stage\":\"offered\""));
        assert_eq!(Beat::read(&nas, "m-000000000000000a").unwrap(), Some(h));
        assert_eq!(Beat::read(&nas, "m-000000000000000b").unwrap(), None);
        // Out of touch after ten minutes by the reader's clock; a clock over a minute ahead, wrong.
        assert!(!b.out_of_touch(1600) && b.out_of_touch(1601));
        assert!(!b.clock_wrong(940) && b.clock_wrong(939));
    }
}
