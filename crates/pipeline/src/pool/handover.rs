//! Handing the lead over (docs/pool.md §6.4), as pure transitions the agent drives: what its loop
//! saw goes in (an ask, the target's heartbeat, its clock, whether it has settled), and what to do
//! comes out (settle, make the next term naming the target, take the lead back), with what its
//! heartbeat says meanwhile (`handing_to`). Times are the lead's own wall clock (§6.7).
//!
//! The lead, A, of term E:
//! - **Leading**: an ask to hand to B makes it **Offered**;
//! - **Offered**: B's heartbeat answers `ready_for: E+1` within a minute: **Settling**; else
//!   **Leading** again;
//! - **Settling**: A grants nothing new, merges what waits, cancels its duties in flight, writes the
//!   coordinator's state and saves its records; settled, it makes term E+1 naming B, with that
//!   snapshot's number: **Passed**. Not settled within a minute: **Leading** again;
//! - **Passed**: A is a member. B's heartbeat leads E+1 within two minutes: the handover is over;
//!   else A makes term E+2 naming itself ("B didn't take up") and leads it.
//!
//! B answers `ready_for` (`ready_for`) while A's heartbeat offers it the lead and it can take it;
//! whatever it answered, it takes up any term that names it. A lead's view can be old without its
//! knowing (§6.6): after a gap (`gap`) it re-asserts, making the next term naming itself
//! (crate::pool::term::claim), and steps down if that term is already made.

use super::beat::{Beat, HandingTo, Stage};
use serde::{Deserialize, Serialize};

/// How long an offer waits for the target's `ready_for` (s).
pub const OFFER_S: u64 = 60;
/// How long a lead may take to settle once the target is ready (s).
pub const SETTLE_S: u64 = 60;
/// How long the target has to lead once the term naming it is made (s).
pub const TAKE_UP_S: u64 = 120;
/// A lead whose clock moved more than this since its last loop ended (it slept, the NAS was silent,
/// a loop took over a minute), or went back as much, re-asserts before acting (s).
pub const GAP_S: u64 = 60;

/// Where a lead's handover of its term stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handover {
    /// Leading; nothing under way.
    #[default]
    Leading,
    /// Asked to hand to `to` at `since`: waiting for its `ready_for`.
    Offered { to: String, since: u64 },
    /// `to` is ready since `since`: settling before the pass.
    Settling { to: String, since: u64 },
    /// The next term, naming `to`, made at `at`: a member, waiting for `to` to lead.
    Passed { to: String, at: u64 },
}

/// What a lead's loop saw, for its handover.
#[derive(Clone, Copy, Debug, Default)]
pub struct Seen<'a> {
    /// This Mac's clock (unix seconds).
    pub now: u64,
    /// An ask to hand the lead to this member (its id) that came this loop, checked by the caller
    /// (§6.3: a live member, its app new enough, not this Mac).
    pub ask: Option<&'a str>,
    /// The heartbeat of the member it's handing to, as read this loop.
    pub target: Option<&'a Beat>,
    /// While settling: the number of the snapshot saved this loop with nothing left waiting to be
    /// merged, its duties cancelled and the coordinator's state written.
    pub settled: Option<u64>,
}

/// What the handover asks of the lead's loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    /// Nothing of the handover's: lead as usual (or, passed, wait).
    Nothing,
    /// Grant nothing new; merge what waits, cancel the duties in flight, write the coordinator's
    /// state and save the records; then say `settled`.
    Settle,
    /// Make term E+1 naming `to`, handing over snapshot `seq` (crate::pool::term::make); made,
    /// `passed`; another's, step down; refused, `abandon`.
    Pass { to: String, seq: u64 },
    /// `to` didn't lead in time: make term E+2 naming this Mac (crate::pool::term::claim) and take
    /// it up; another's made first, nothing.
    TakeBack { to: String },
    /// Over: `to` leads.
    Done,
}

/// Whether more than `s` seconds lie between `since` and `now`, either way (a clock set back as far
/// counts too).
fn past(since: u64, now: u64, s: u64) -> bool {
    now.abs_diff(since) >= s
}

impl Handover {
    /// The next step of the handover of term `term`, given what this loop saw.
    pub fn step(&mut self, term: u64, seen: &Seen) -> Do {
        let now = seen.now;
        let target = |to: &str| seen.target.filter(|b| b.member == to);
        match std::mem::take(self) {
            Handover::Leading => {
                if let Some(to) = seen.ask {
                    *self = Handover::Offered { to: to.to_string(), since: now };
                }
                Do::Nothing
            }
            Handover::Offered { to, since } => {
                if target(&to).is_some_and(|b| b.ready_for == Some(term + 1)) {
                    *self = Handover::Settling { to, since: now };
                    Do::Settle
                } else {
                    if !past(since, now, OFFER_S) {
                        *self = Handover::Offered { to, since };
                    }
                    Do::Nothing
                }
            }
            Handover::Settling { to, since } => {
                if let Some(seq) = seen.settled {
                    *self = Handover::Settling { to: to.clone(), since };
                    Do::Pass { to, seq }
                } else if past(since, now, SETTLE_S) {
                    Do::Nothing
                } else {
                    *self = Handover::Settling { to, since };
                    Do::Settle
                }
            }
            Handover::Passed { to, at } => {
                if target(&to).is_some_and(|b| b.leads.is_some_and(|l| l > term)) {
                    *self = Handover::Passed { to: to.clone(), at };
                    Do::Done
                } else if past(at, now, TAKE_UP_S) {
                    *self = Handover::Passed { to: to.clone(), at };
                    Do::TakeBack { to }
                } else {
                    *self = Handover::Passed { to, at };
                    Do::Nothing
                }
            }
        }
    }

    /// The term `Do::Pass` asked for is made, at `now`: waiting for the target to lead.
    pub fn passed(&mut self, now: u64) {
        if let Handover::Settling { to, .. } = std::mem::take(self) {
            *self = Handover::Passed { to, at: now };
        }
    }

    /// The pass can't be made (the app rule refused it, or the NAS failed): leading as before.
    pub fn abandon(&mut self) {
        if !matches!(self, Handover::Passed { .. }) {
            *self = Handover::Leading;
        }
    }

    /// Whether the lead grants jobs and does its duties: not while settling, nor once passed.
    pub fn grants(&self) -> bool {
        matches!(self, Handover::Leading | Handover::Offered { .. })
    }

    /// What this Mac's heartbeat says of the handover of term `term`.
    pub fn handing_to(&self, term: u64) -> Option<HandingTo> {
        let (to, since, stage) = match self {
            Handover::Leading => return None,
            Handover::Offered { to, since } => (to, *since, Stage::Offered),
            Handover::Settling { to, since } => (to, *since, Stage::Settling),
            Handover::Passed { to, at } => (to, *at, Stage::Passed),
        };
        Some(HandingTo { to: to.clone(), term, since, stage })
    }
}

/// The `ready_for` member `me` answers a lead's heartbeat (`lead`) with (§6.4, Ready): the term
/// after the lead's while it offers this member the lead and this member can take it (`able`: term
/// E's records and the coordinator's state loaded, its disk and app checked); none otherwise.
pub fn ready_for(lead: &Beat, me: &str, able: bool) -> Option<u64> {
    let h = lead.handing_to.as_ref()?;
    (able && h.to == me && h.stage != Stage::Passed && lead.leads == Some(h.term)).then_some(h.term + 1)
}

/// Whether a lead must re-assert before acting (§6.6): its clock moved more than `GAP_S` either
/// way between `last` (its last loop's end, or this loop's start) and `now`.
pub fn gap(last: u64, now: u64) -> bool {
    now.abs_diff(last) > GAP_S
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "m-000000000000000a";
    const B: &str = "m-000000000000000b";

    fn beat(member: &str) -> Beat {
        Beat { member: member.into(), ..Default::default() }
    }

    #[test]
    fn a_handover_goes_offered_settling_passed_then_done() {
        let mut h = Handover::Leading;
        assert_eq!(h.step(7, &Seen { now: 100, ask: Some(B), ..Default::default() }), Do::Nothing);
        assert_eq!(h, Handover::Offered { to: B.into(), since: 100 });
        assert!(h.grants());
        // The lead's heartbeat offers it; B answers.
        let lead = Beat { member: A.into(), leads: Some(7), handing_to: h.handing_to(7), ..Default::default() };
        assert_eq!(ready_for(&lead, B, true), Some(8));
        assert_eq!(ready_for(&lead, B, false), None, "B can't lead now");
        assert_eq!(ready_for(&lead, "m-000000000000000c", true), None);
        let b = Beat { ready_for: Some(8), ..beat(B) };
        assert_eq!(h.step(7, &Seen { now: 130, target: Some(&b), ..Default::default() }), Do::Settle);
        assert!(!h.grants());
        assert_eq!(h.step(7, &Seen { now: 140, target: Some(&b), ..Default::default() }), Do::Settle);
        assert_eq!(h.step(7, &Seen { now: 150, target: Some(&b), settled: Some(12), ..Default::default() }), Do::Pass { to: B.into(), seq: 12 });
        h.passed(152);
        assert_eq!(h, Handover::Passed { to: B.into(), at: 152 });
        assert_eq!(h.handing_to(7).map(|x| x.stage), Some(Stage::Passed));
        let passed = Beat { member: A.into(), handing_to: h.handing_to(7), ..Default::default() };
        assert_eq!(ready_for(&passed, B, true), None, "passed: nothing more to answer");
        assert_eq!(h.step(7, &Seen { now: 200, target: Some(&b), ..Default::default() }), Do::Nothing);
        let leading = Beat { leads: Some(8), ..beat(B) };
        assert_eq!(h.step(7, &Seen { now: 210, target: Some(&leading), ..Default::default() }), Do::Done);
    }

    #[test]
    fn a_handover_gives_up_or_takes_back_when_its_minutes_run_out() {
        // No answer within a minute: leading again.
        let mut h = Handover::Leading;
        h.step(7, &Seen { now: 100, ask: Some(B), ..Default::default() });
        let other = Beat { ready_for: Some(8), ..beat("m-000000000000000c") };
        assert_eq!(h.step(7, &Seen { now: 159, target: Some(&other), ..Default::default() }), Do::Nothing);
        assert!(matches!(h, Handover::Offered { .. }), "another's answer isn't B's");
        h.step(7, &Seen { now: 160, ..Default::default() });
        assert_eq!(h, Handover::Leading);
        // Not settled within a minute: leading again.
        h.step(7, &Seen { now: 200, ask: Some(B), ..Default::default() });
        let b = Beat { ready_for: Some(8), ..beat(B) };
        assert_eq!(h.step(7, &Seen { now: 210, target: Some(&b), ..Default::default() }), Do::Settle);
        assert_eq!(h.step(7, &Seen { now: 270, target: Some(&b), ..Default::default() }), Do::Nothing);
        assert_eq!(h, Handover::Leading);
        // A pass refused (B's app is older): leading again.
        h.step(7, &Seen { now: 300, ask: Some(B), ..Default::default() });
        h.step(7, &Seen { now: 310, target: Some(&b), ..Default::default() });
        assert!(matches!(h.step(7, &Seen { now: 320, settled: Some(3), ..Default::default() }), Do::Pass { .. }));
        h.abandon();
        assert_eq!(h, Handover::Leading);
        // Passed, and B never leads: taken back after two minutes (and asked again until made).
        h.step(7, &Seen { now: 400, ask: Some(B), ..Default::default() });
        h.step(7, &Seen { now: 410, target: Some(&b), ..Default::default() });
        h.step(7, &Seen { now: 420, settled: Some(4), ..Default::default() });
        h.passed(421);
        assert_eq!(h.step(7, &Seen { now: 540, target: Some(&b), ..Default::default() }), Do::Nothing);
        assert_eq!(h.step(7, &Seen { now: 541, ..Default::default() }), Do::TakeBack { to: B.into() });
        assert_eq!(h.step(7, &Seen { now: 560, ..Default::default() }), Do::TakeBack { to: B.into() });
        // An ask while one is under way is passed over.
        let mut busy = Handover::Offered { to: B.into(), since: 100 };
        busy.step(7, &Seen { now: 101, ask: Some("m-000000000000000c"), ..Default::default() });
        assert_eq!(busy, Handover::Offered { to: B.into(), since: 100 });
    }

    #[test]
    fn a_gap_is_a_minute_either_way() {
        assert!(!gap(1000, 1060) && gap(1000, 1061));
        assert!(!gap(1000, 940) && gap(1000, 939), "a clock set back");
    }
}
