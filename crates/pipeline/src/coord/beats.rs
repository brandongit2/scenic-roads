//! The members' beats as this Mac hears them by HTTP (docs/coordination.md §8, step 3 of task
//! #878): one `POST /pool/beat` a member every 10 s carries its pool heartbeat
//! (crate::agent::pool::Heartbeat) and its agent's status besides the leases it holds
//! (crate::coord::held).
//!
//! - **On the lead** its coordinator keeps each member's last (`heard`), in memory, and answers
//!   with the others' heartbeats (`members`), its own among them: the members learn each other's
//!   brokers and the lead's from the lead, not the NAS. The lead's readers of a member's heartbeat
//!   (crate::agent::pool::Side::read_heartbeat) and of the members' statuses (the status's
//!   `helpers`) read these first.
//! - **On a member** the lead's answer fills the same registry (`answered`); its leases released
//!   wait for the agent's loop (`take_released`).
//! - **The NAS copies** (`state/pool/members/<id>.json`, `state/helpers/<host>.json`) are written
//!   only while `nas_too`: on a member, until a lead speaking `SPEAKS` says no member in touch
//!   still reads them (an older lead answers 404, or answers without `SPEAKS`: they go on); on the
//!   lead, while a member in touch heartbeats on the NAS without `beat_http` (an older app), which
//!   still reads the lead's heartbeat there. A heartbeat not on the NAS is still read there for such
//!   a member; its listing is the older apps' path, dropped one release after every member speaks
//!   `SPEAKS` (the #642 gate).
//!
//! Nothing here touches the NAS: the coordinator's handlers reach it (§2.3).

use crate::agent::pool::Heartbeat;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// What a member's beat says it speaks, and a lead's answer that it takes: its heartbeat and
/// status ride in the beat.
pub const SPEAKS: &str = "beat";

/// A member not heard from this long is forgotten (s): as the listing forgets one
/// (crate::agent::pool::FORGET_AFTER).
pub const FORGET_S: u64 = crate::agent::pool::FORGET_AFTER.as_secs();

/// One member's last beat.
#[derive(Clone, Debug)]
pub struct Heard {
    pub beat: Heartbeat,
    /// Its agent's status (crate::agent::Status, as JSON; its plan's parts left out by the member).
    pub status: Option<serde_json::Value>,
    /// When heard (this Mac's clock, unix s).
    pub at: u64,
    /// Beats heard from it since this process started.
    pub n: u64,
}

struct Inner {
    heard: BTreeMap<String, Heard>,
    own: Option<Heartbeat>,
    nas_too: bool,
    released: Vec<u64>,
}

/// The registry, shared by a member's pool side, its agent's beat and (on the lead) its coordinator.
pub struct Beats {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Beats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = self.inner.lock().unwrap();
        f.debug_struct("Beats").field("heard", &g.heard.len()).field("nas_too", &g.nas_too).finish()
    }
}

impl Default for Beats {
    fn default() -> Beats {
        Beats { inner: Mutex::new(Inner { heard: BTreeMap::new(), own: None, nas_too: true, released: Vec::new() }) }
    }
}

impl Beats {
    /// Member `h`'s beat heard at `at`, with its status.
    pub fn heard(&self, h: Heartbeat, status: Option<serde_json::Value>, at: u64) {
        let mut g = self.inner.lock().unwrap();
        let n = g.heard.get(&h.pool.member).map_or(0, |x| x.n) + 1;
        g.heard.insert(h.pool.member.clone(), Heard { beat: h, status, at, n });
    }

    /// The members heard within `FORGET_S` of `now`, the others dropped.
    pub fn fresh(&self, now: u64) -> Vec<Heard> {
        let mut g = self.inner.lock().unwrap();
        g.heard.retain(|_, x| now.saturating_sub(x.at) < FORGET_S);
        g.heard.values().cloned().collect()
    }

    /// Member `id`'s heartbeat, this Mac's own or one heard within `FORGET_S`.
    pub fn get(&self, id: &str, now: u64) -> Option<Heartbeat> {
        let g = self.inner.lock().unwrap();
        if let Some(o) = g.own.as_ref().filter(|o| o.pool.member == id) {
            return Some(o.clone());
        }
        g.heard.get(id).filter(|x| now.saturating_sub(x.at) < FORGET_S).map(|x| x.beat.clone())
    }

    /// The heartbeats a lead answers member `to` with: its own and those heard, but `to`'s.
    pub fn members(&self, now: u64, to: &str) -> Vec<Heartbeat> {
        let own = self.own();
        self.fresh(now).into_iter().map(|x| x.beat).chain(own).filter(|h| h.pool.member != to).collect()
    }

    /// This Mac's own heartbeat, as its pool side last composed it.
    pub fn set_own(&self, h: Heartbeat) {
        self.inner.lock().unwrap().own = Some(h);
    }

    pub fn own(&self) -> Option<Heartbeat> {
        self.inner.lock().unwrap().own.clone()
    }

    /// Whether this Mac's heartbeat and status go to the NAS too.
    pub fn nas_too(&self) -> bool {
        self.inner.lock().unwrap().nas_too
    }

    pub fn set_nas_too(&self, on: bool) {
        let mut g = self.inner.lock().unwrap();
        if g.nas_too != on {
            eprintln!(
                "pool: this Mac's heartbeat and status {} on the NAS",
                if on {
                    "written again (a member on an older app reads them)"
                } else {
                    "no longer written (the lead hears them by HTTP)"
                }
            );
        }
        g.nas_too = on;
    }

    /// A member's lead answered its beat at `now` (an older lead's answer, without `SPEAKS`: the
    /// NAS copies go on): the others' heartbeats as it has them, its releases for the loop.
    pub fn answered(&self, a: &super::held::Answer, now: u64) {
        let takes = a.speaks.iter().any(|w| w == SPEAKS);
        self.set_nas_too(!takes || a.nas_too);
        let mut g = self.inner.lock().unwrap();
        if takes {
            let before = std::mem::take(&mut g.heard);
            for h in &a.members {
                let n = before.get(&h.pool.member).map_or(0, |x| x.n) + 1;
                g.heard.insert(h.pool.member.clone(), Heard { beat: h.clone(), status: None, at: now, n });
            }
        }
        g.released.extend(a.released.iter().copied());
    }

    /// The leases the lead said this member doesn't hold, since last taken.
    pub fn take_released(&self) -> Vec<u64> {
        std::mem::take(&mut self.inner.lock().unwrap().released)
    }
}

/// Whether any of the members' heartbeats `beats` (as read, the NAS's for a member not heard by
/// HTTP) is an older app's in touch at `now`, but member `me`'s: the NAS copies are still read.
pub fn old_in_touch<'a>(beats: impl IntoIterator<Item = &'a Heartbeat>, me: &str, now: u64) -> bool {
    beats
        .into_iter()
        .any(|h| h.pool.member != me && !h.shadow && !h.beat_http && h.pool.beat > 0 && !h.pool.out_of_touch(now))
}

#[cfg(test)]
mod tests {
    use super::super::held::Answer;
    use super::*;

    fn hb(id: &str, at: u64, http: bool) -> Heartbeat {
        Heartbeat {
            pool: crate::pool::Beat { member: id.into(), host: format!("{id}-host"), beat: at, ..Default::default() },
            beat_http: http,
            ..Default::default()
        }
    }

    /// The lead hears two members and answers each with the other and itself; one not heard for
    /// `FORGET_S` is dropped.
    #[test]
    fn the_lead_answers_each_member_with_the_others() {
        let b = Beats::default();
        b.set_own(hb("m-lead", 100, true));
        b.heard(hb("m-a", 100, true), None, 100);
        b.heard(hb("m-b", 100, true), None, 100);
        let ids = |v: Vec<Heartbeat>| v.into_iter().map(|h| h.pool.member).collect::<Vec<_>>();
        assert_eq!(ids(b.members(110, "m-a")), ["m-b", "m-lead"]);
        assert!(b.get("m-a", 110).is_some() && b.get("m-lead", 110).is_some());
        b.heard(hb("m-b", 100 + FORGET_S, true), None, 100 + FORGET_S);
        assert_eq!(ids(b.members(100 + FORGET_S, "m-x")), ["m-b", "m-lead"], "m-a forgotten");
        assert!(b.get("m-a", 100 + FORGET_S).is_none());
    }

    /// A member's NAS copies go on until a lead speaking the beat says nobody reads them: an older
    /// lead's answer (no `SPEAKS`) keeps them; a member on an older app in touch keeps them too.
    #[test]
    fn the_nas_copies_stop_only_by_a_lead_that_takes_beats() {
        let b = Beats::default();
        assert!(b.nas_too());
        b.answered(&Answer { epoch: 1, released: vec![7], ..Default::default() }, 10);
        assert!(b.nas_too(), "a step-2 lead");
        assert_eq!(b.take_released(), [7]);
        assert!(b.take_released().is_empty());
        let a = Answer {
            epoch: 1,
            speaks: vec![SPEAKS.into()],
            nas_too: true,
            members: vec![hb("m-b", 9, true)],
            ..Default::default()
        };
        b.answered(&a, 10);
        assert!(b.nas_too() && b.get("m-b", 10).is_some());
        b.answered(&Answer { nas_too: false, ..a }, 11);
        assert!(!b.nas_too());
        // (The lead's own rule: an older app's heartbeat in touch on the NAS.)
        let (old, new) = (hb("m-o", 1000, false), hb("m-n", 1000, true));
        assert!(old_in_touch([&old, &new], "m-me", 1100));
        assert!(!old_in_touch([&new], "m-me", 1100));
        assert!(!old_in_touch([&old], "m-me", 1000 + crate::pool::beat::OUT_OF_TOUCH_S + 1), "out of touch");
    }
}
