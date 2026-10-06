//! Handing the lead over (docs/pool.md §6.4), as pure transitions the driver (crate::pool::driver)
//! drives: what its step saw goes in (an ask, the target's heartbeat, its clock, the current term,
//! whether it has settled, whether the target took up), and what to do comes out (settle, make the
//! next term naming the target, take the lead back, step down), with what its heartbeat says
//! meanwhile (`handing_to`). Times are the lead's own wall clock (§6.7), read after what they're
//! compared with.
//!
//! The lead, A, of term E:
//! - **Leading**: an ask to hand to B makes it **Offered**: an offer, known by when it was made;
//! - **Offered**: B's heartbeat answers that offer (`ready_for`: E+1 and the offer) within a minute:
//!   **Settling**; else **Leading** again;
//! - **Settling**: A grants nothing new, merges what waits, cancels its duties in flight, writes the
//!   coordinator's state and saves its records with it; settled within a minute of B's answer, and
//!   B's answer standing, it makes term E+1 naming B, with that snapshot's number: **Passed**. Not
//!   settled within the minute: **Leading** again (a settle that ends later passes nothing: B
//!   answered an offer long gone);
//! - **Passed**: A is a member. B is known to lead E+1 within two minutes (its message, its
//!   heartbeat, or E+1's first snapshot): the handover is over; else A makes term E+2 naming itself
//!   ("B didn't take up") and leads it.
//!
//! B answers `ready_for` (`ready_for`) while A's heartbeat offers it the lead and it can take it;
//! whatever it answered, it takes up any term that names it.

use super::beat::{Beat, HandingTo, Ready, Stage};
use serde::{Deserialize, Serialize};

/// How long an offer waits for the target's answer (s).
pub const OFFER_S: u64 = 60;
/// How long a lead may take to settle once the target answered (s).
pub const SETTLE_S: u64 = 60;
/// How long the target has to lead once the term naming it is made (s).
pub const TAKE_UP_S: u64 = 120;

/// Where a lead's handover of its term stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handover {
    /// Leading; nothing under way.
    #[default]
    Leading,
    /// Asked to hand to `to`: the offer, made at `offer`, waits for its answer.
    Offered { to: String, offer: u64 },
    /// `to` answered offer `offer` at `since`: settling before the pass.
    Settling { to: String, offer: u64, since: u64 },
    /// The next term, naming `to`, made at `at`: a member, waiting for `to` to lead.
    Passed { to: String, offer: u64, at: u64 },
}

/// What a lead's step saw, for its handover.
#[derive(Clone, Copy, Debug, Default)]
pub struct Seen<'a> {
    /// This Mac's clock (unix seconds), read after the rest.
    pub now: u64,
    /// The highest term this Mac knows exists (a later one than its own, or than its pass: another
    /// Mac made it).
    pub current: u64,
    /// An ask to hand the lead to this member (its id) that came this step, checked by the caller
    /// (§6.3: a live member, its app new enough, not this Mac).
    pub ask: Option<&'a str>,
    /// The heartbeat of the member it's handing to, as read this step.
    pub target: Option<&'a Beat>,
    /// While settling: the number of the snapshot saved with nothing left waiting to be merged,
    /// its duties cancelled and the coordinator's state written with it.
    pub settled: Option<u64>,
    /// Passed: the target told this Mac it leads the term handed, or the term has a snapshot (its
    /// first, saved as it took up).
    pub taken_up: bool,
}

/// What the handover asks of the lead's step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    /// Nothing of the handover's: lead as usual (or, passed, wait).
    Nothing,
    /// Grant nothing new; merge what waits, cancel the duties in flight, write the coordinator's
    /// state and save the records with it; then say `settled`.
    Settle,
    /// Make term E+1 naming `to`, handing over snapshot `seq` (crate::pool::term::make); made,
    /// `passed`; another's, step down; refused, `abandon`.
    Pass { to: String, seq: u64 },
    /// `to` didn't lead in time: make term E+2 naming this Mac (crate::pool::term::back) and take
    /// it up; another's made first, nothing.
    TakeBack { to: String },
    /// Over: `to` leads, or a later term than the pass was made.
    Done,
    /// A later term than this lead's exists, not its own pass: stop granting, planning, merging and
    /// the duties, write nothing more, and carry on as a member (§6.6).
    StepDown,
}

/// Whether more than `s` seconds lie between `since` and `now`, either way (a clock set back as far
/// counts too).
fn past(since: u64, now: u64, s: u64) -> bool {
    now.abs_diff(since) >= s
}

impl Handover {
    /// The next step of the handover of term `term`, given what this step saw.
    pub fn step(&mut self, term: u64, seen: &Seen) -> Do {
        let now = seen.now;
        let target = |to: &str| seen.target.filter(|b| b.member == to);
        match self {
            Handover::Passed { .. } if seen.current > term + 1 => return Do::Done,
            Handover::Passed { .. } => {}
            _ if seen.current > term => return Do::StepDown,
            _ => {}
        }
        match std::mem::take(self) {
            Handover::Leading => {
                if let Some(to) = seen.ask {
                    *self = Handover::Offered { to: to.to_string(), offer: now };
                }
                Do::Nothing
            }
            Handover::Offered { to, offer } => {
                if target(&to).is_some_and(|b| b.ready_for == Some(Ready { term: term + 1, offer })) {
                    *self = Handover::Settling { to, offer, since: now };
                    Do::Settle
                } else {
                    if !past(offer, now, OFFER_S) {
                        *self = Handover::Offered { to, offer };
                    }
                    Do::Nothing
                }
            }
            Handover::Settling { to, offer, since } => {
                let standing = target(&to).is_some_and(|b| b.ready_for == Some(Ready { term: term + 1, offer }));
                if past(since, now, SETTLE_S) {
                    Do::Nothing
                } else if let (Some(seq), true) = (seen.settled, standing) {
                    *self = Handover::Settling { to: to.clone(), offer, since };
                    Do::Pass { to, seq }
                } else {
                    *self = Handover::Settling { to, offer, since };
                    Do::Settle
                }
            }
            Handover::Passed { to, offer, at } => {
                if seen.taken_up || target(&to).is_some_and(|b| b.leads.is_some_and(|l| l > term)) {
                    *self = Handover::Passed { to: to.clone(), offer, at };
                    Do::Done
                } else if past(at, now, TAKE_UP_S) {
                    *self = Handover::Passed { to: to.clone(), offer, at };
                    Do::TakeBack { to }
                } else {
                    *self = Handover::Passed { to, offer, at };
                    Do::Nothing
                }
            }
        }
    }

    /// The term `Do::Pass` asked for is made, at `now`: waiting for the target to lead.
    pub fn passed(&mut self, now: u64) {
        if let Handover::Settling { to, offer, .. } = std::mem::take(self) {
            *self = Handover::Passed { to, offer, at: now };
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
        let (to, offer, since, stage) = match self {
            Handover::Leading => return None,
            Handover::Offered { to, offer } => (to, *offer, *offer, Stage::Offered),
            Handover::Settling { to, offer, since } => (to, *offer, *since, Stage::Settling),
            Handover::Passed { to, offer, at } => (to, *offer, *at, Stage::Passed),
        };
        Some(HandingTo { to: to.clone(), term, offer, since, stage })
    }
}

/// The answer member `me` gives a lead's heartbeat (`lead`) (§6.4, Ready): the term after the
/// lead's and the offer it answers, while the lead offers this member the lead (or settles to hand
/// it over) and this member can take it (`able`: its disk, home and power, and the app rule);
/// none otherwise.
pub fn ready_for(lead: &Beat, me: &str, able: bool) -> Option<Ready> {
    let h = lead.handing_to.as_ref()?;
    (able && h.to == me && h.stage != Stage::Passed && lead.leads == Some(h.term)).then_some(Ready { term: h.term + 1, offer: h.offer })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "m-000000000000000a";
    const B: &str = "m-000000000000000b";

    fn beat(member: &str) -> Beat {
        Beat { member: member.into(), ..Default::default() }
    }

    fn answer(h: &Handover, term: u64) -> Beat {
        let lead = Beat { member: A.into(), leads: Some(term), handing_to: h.handing_to(term), ..Default::default() };
        Beat { ready_for: ready_for(&lead, B, true), ..beat(B) }
    }

    #[test]
    fn a_handover_goes_offered_settling_passed_then_done() {
        let mut h = Handover::Leading;
        assert_eq!(h.step(7, &Seen { now: 100, ask: Some(B), ..Default::default() }), Do::Nothing);
        assert_eq!(h, Handover::Offered { to: B.into(), offer: 100 });
        assert!(h.grants());
        // The lead's heartbeat offers it; B answers.
        let lead = Beat { member: A.into(), leads: Some(7), handing_to: h.handing_to(7), ..Default::default() };
        assert_eq!(ready_for(&lead, B, true), Some(Ready { term: 8, offer: 100 }));
        assert_eq!(ready_for(&lead, B, false), None, "B can't lead now");
        assert_eq!(ready_for(&lead, "m-000000000000000c", true), None);
        let b = answer(&h, 7);
        assert_eq!(h.step(7, &Seen { now: 130, target: Some(&b), ..Default::default() }), Do::Settle);
        assert!(!h.grants());
        assert_eq!(answer(&h, 7), b, "answered while it settles too");
        assert_eq!(h.step(7, &Seen { now: 140, target: Some(&b), ..Default::default() }), Do::Settle);
        assert_eq!(h.step(7, &Seen { now: 150, settled: Some(12), ..Default::default() }), Do::Settle, "B's answer not read: not yet");
        assert_eq!(h.step(7, &Seen { now: 150, target: Some(&b), settled: Some(12), ..Default::default() }), Do::Pass { to: B.into(), seq: 12 });
        h.passed(152);
        assert_eq!(h, Handover::Passed { to: B.into(), offer: 100, at: 152 });
        assert_eq!(h.handing_to(7).map(|x| x.stage), Some(Stage::Passed));
        let passed = Beat { member: A.into(), handing_to: h.handing_to(7), ..Default::default() };
        assert_eq!(ready_for(&passed, B, true), None, "passed: nothing more to answer");
        assert_eq!(h.step(7, &Seen { now: 200, target: Some(&b), ..Default::default() }), Do::Nothing);
        let leading = Beat { leads: Some(8), ..beat(B) };
        assert_eq!(h.step(7, &Seen { now: 210, target: Some(&leading), ..Default::default() }), Do::Done);
        // Or told so, or term 8's first snapshot seen, its heartbeat not read yet.
        let mut told = Handover::Passed { to: B.into(), offer: 100, at: 152 };
        assert_eq!(told.step(7, &Seen { now: 200, taken_up: true, ..Default::default() }), Do::Done);
    }

    #[test]
    fn a_handover_gives_up_or_takes_back_when_its_minutes_run_out() {
        // No answer within a minute: leading again.
        let mut h = Handover::Leading;
        h.step(7, &Seen { now: 100, ask: Some(B), ..Default::default() });
        let other = Beat { ready_for: Some(Ready { term: 8, offer: 100 }), ..beat("m-000000000000000c") };
        assert_eq!(h.step(7, &Seen { now: 159, target: Some(&other), ..Default::default() }), Do::Nothing);
        assert!(matches!(h, Handover::Offered { .. }), "another's answer isn't B's");
        h.step(7, &Seen { now: 160, ..Default::default() });
        assert_eq!(h, Handover::Leading);
        // Not settled within a minute: leading again.
        h.step(7, &Seen { now: 200, ask: Some(B), ..Default::default() });
        let b = answer(&h, 7);
        assert_eq!(h.step(7, &Seen { now: 210, target: Some(&b), ..Default::default() }), Do::Settle);
        assert_eq!(h.step(7, &Seen { now: 270, target: Some(&b), ..Default::default() }), Do::Nothing);
        assert_eq!(h, Handover::Leading);
        // A pass refused (B's app is older): leading again.
        h.step(7, &Seen { now: 300, ask: Some(B), ..Default::default() });
        let b = answer(&h, 7);
        h.step(7, &Seen { now: 310, target: Some(&b), ..Default::default() });
        assert!(matches!(h.step(7, &Seen { now: 320, target: Some(&b), settled: Some(3), ..Default::default() }), Do::Pass { .. }));
        h.abandon();
        assert_eq!(h, Handover::Leading);
        // Passed, and B never leads: taken back after two minutes (and asked again until made).
        h.step(7, &Seen { now: 400, ask: Some(B), ..Default::default() });
        let b = answer(&h, 7);
        h.step(7, &Seen { now: 410, target: Some(&b), ..Default::default() });
        h.step(7, &Seen { now: 420, target: Some(&b), settled: Some(4), ..Default::default() });
        h.passed(421);
        assert_eq!(h.step(7, &Seen { now: 540, target: Some(&b), ..Default::default() }), Do::Nothing);
        assert_eq!(h.step(7, &Seen { now: 541, ..Default::default() }), Do::TakeBack { to: B.into() });
        assert_eq!(h.step(7, &Seen { now: 560, ..Default::default() }), Do::TakeBack { to: B.into() });
        // An ask while one is under way is passed over.
        let mut busy = Handover::Offered { to: B.into(), offer: 100 };
        busy.step(7, &Seen { now: 101, ask: Some("m-000000000000000c"), ..Default::default() });
        assert_eq!(busy, Handover::Offered { to: B.into(), offer: 100 });
    }

    #[test]
    fn an_answer_is_to_its_own_offer() {
        // B answered an offer an hour ago (its heartbeat not written since, or read stale). (Review
        // M6: `ready_for` named only the term, and answered this offer too.)
        let mut h = Handover::Leading;
        h.step(7, &Seen { now: 100, ask: Some(B), ..Default::default() });
        let old = answer(&h, 7);
        h.step(7, &Seen { now: 200, ..Default::default() });
        assert_eq!(h, Handover::Leading, "unanswered in its minute");
        h.step(7, &Seen { now: 4000, ask: Some(B), ..Default::default() });
        assert_eq!(h.step(7, &Seen { now: 4001, target: Some(&old), ..Default::default() }), Do::Nothing, "an earlier offer's answer");
        let new = answer(&h, 7);
        assert_ne!(new, old);
        assert_eq!(h.step(7, &Seen { now: 4002, target: Some(&new), ..Default::default() }), Do::Settle);
    }

    #[test]
    fn a_settle_past_its_minute_passes_nothing() {
        // The lead slept 40 minutes mid-settle. (Review M6: settled, it passed to B on an answer
        // to an offer long gone.)
        let mut h = Handover::Leading;
        h.step(7, &Seen { now: 100, ask: Some(B), ..Default::default() });
        let b = answer(&h, 7);
        assert_eq!(h.step(7, &Seen { now: 101, target: Some(&b), ..Default::default() }), Do::Settle);
        let d = h.step(7, &Seen { now: 101 + 2400, target: Some(&b), settled: Some(5), ..Default::default() });
        assert_eq!((d, h), (Do::Nothing, Handover::Leading));
    }

    #[test]
    fn a_later_term_ends_a_lead_or_a_handover() {
        // Another Mac's term after this lead's: it steps down, whatever it was doing.
        let mut h = Handover::Offered { to: B.into(), offer: 100 };
        assert_eq!(h.step(7, &Seen { now: 110, current: 8, ..Default::default() }), Do::StepDown);
        // Its own pass is term 8; a term after that is another's: nothing to take back.
        let mut p = Handover::Passed { to: B.into(), offer: 90, at: 100 };
        assert_eq!(p.step(7, &Seen { now: 110, current: 8, ..Default::default() }), Do::Nothing);
        assert_eq!(p.step(7, &Seen { now: 400, current: 9, ..Default::default() }), Do::Done);
    }
}
