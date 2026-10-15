//! Whether the members reach each other's coordinators (docs/pool.md §6.8): each member tries the
//! lead's coordinator and the other members' brokers (crate::agent::pool's `Run::tend_reach`: a
//! cheap request with a short timeout every `PROBE_S`, and the outcome of its client's own requests
//! to the lead) and says in its heartbeat (`Beat::reach`) whom it reached lately, and whom it has
//! failed to reach for `REPORT_S` or more, and why. Read back from the heartbeats:
//!
//! - a member some other member in touch reports it can't reach isn't handed the lead
//!   (`Driver::hand_to`): its coordinator would grant nobody else work;
//! - a lead reported so for `AWAY_S` or more hands the lead to a member that another member
//!   reached lately and none reports unreachable (crate::agent::lead's `away`), no sooner than
//!   `crate::agent::lead::CALM_S` after the last change of lead or the last such ask;
//! - the views say who can't reach whom, and why, with what the owner can do.
//!
//! Every time here is the reporter's own clock (its `Beat::beat` is too), so readers compare a
//! report's times with its own heartbeat's, never with theirs.

use super::beat::Beat;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A member tries the others this often (s).
pub const PROBE_S: u64 = 60;
/// A try gives up after this (s): a coordinator that's there answers in well under a second.
pub const PROBE_TIMEOUT_S: u64 = 3;
/// A member is reported unreachable once its failures span this long with no success between (s):
/// three tries at least, so a restart (its port closed for seconds) isn't.
pub const REPORT_S: u64 = 120;
/// A success or failure older than this (the reporter's clock, against its heartbeat's) says
/// nothing now (s).
pub const REACHED_S: u64 = 300;
/// A lead reported unreachable this long (s) hands the lead to a member others reach.
pub const AWAY_S: u64 = 600;

/// What a failure to reach a coordinator came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cause {
    /// The connection timed out: nothing answered at all (on a Mac at home, its firewall
    /// dropping incoming connections; or it's asleep, or off the network).
    TimedOut,
    /// The connection was refused: the Mac is there but nothing listens on the port.
    Refused,
    /// Its address doesn't resolve.
    NoName,
    /// Anything else.
    Other,
}

/// One member's report of another (`Beat::reach`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    /// The member tried.
    pub member: String,
    /// What was tried: "coordinator" (the lead's) or "broker".
    pub what: String,
    /// Reached at `at`; or not, failing since `since` and last at `at`.
    pub ok: bool,
    pub at: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Cause>,
    /// The failure as the request said it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// One target's outcomes, as a member saw them.
#[derive(Clone, Debug, Default)]
struct Seen {
    what: String,
    ok_at: Option<u64>,
    /// Failing: since, last, cause, why.
    fail: Option<(u64, u64, Cause, String)>,
}

/// A member's outcomes of its tries and requests, by target (its clock).
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    by: BTreeMap<String, Seen>,
}

impl Tracker {
    /// Target `member`'s `what` tried at `at`: reached, or not and why. Outcomes may come out of
    /// order (a request's and a try's): a success clears only failures no later than it, and a
    /// failure no later than the last success counts for nothing.
    pub fn seen(&mut self, member: &str, what: &str, r: Result<(), (Cause, String)>, at: u64) {
        let s = self.by.entry(member.to_string()).or_default();
        s.what = what.to_string();
        match r {
            Ok(()) => {
                s.ok_at = Some(s.ok_at.map_or(at, |o| o.max(at)));
                if s.fail.as_ref().is_some_and(|f| f.1 <= at) {
                    s.fail = None;
                }
            }
            Err((cause, why)) => {
                if s.ok_at.is_some_and(|o| o >= at) {
                    return;
                }
                s.fail = Some(match s.fail.take() {
                    Some((since, last, ..)) => (since.min(at), last.max(at), cause, why),
                    None => (at, at, cause, why),
                });
            }
        }
    }

    /// Forgets the targets `keep` doesn't name (no longer tried).
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.by.retain(|m, _| keep(m));
    }

    /// What its heartbeat says at `now`: whom it reached within `REACHED_S`, and whom it failed to
    /// reach across `REPORT_S` or more, its last failure within `REACHED_S` and no success since.
    pub fn report(&self, now: u64) -> Vec<Reach> {
        let mut out = Vec::new();
        for (m, s) in &self.by {
            match &s.fail {
                Some((since, last, cause, why)) if last.saturating_sub(*since) >= REPORT_S && now.saturating_sub(*last) <= REACHED_S => {
                    out.push(Reach { member: m.clone(), what: s.what.clone(), ok: false, at: *last, since: *since, cause: Some(*cause), why: why.clone() });
                }
                Some(_) => {}
                None => {
                    if let Some(at) = s.ok_at.filter(|a| now.saturating_sub(*a) <= REACHED_S) {
                        out.push(Reach { member: m.clone(), what: s.what.clone(), ok: true, at, since: 0, cause: None, why: String::new() });
                    }
                }
            }
        }
        out
    }
}

/// A failure's cause from its words (an error's chain, as anyhow prints it).
pub fn cause_of(text: &str) -> Cause {
    let t = text.to_ascii_lowercase();
    if t.contains("timed out") || t.contains("timeout") {
        Cause::TimedOut
    } else if t.contains("refused") {
        Cause::Refused
    } else if t.contains("host not found") || t.contains("resolve") || t.contains("nodename") {
        Cause::NoName
    } else {
        Cause::Other
    }
}

/// A report's failure in words, for target `host`: the cause and what's likely behind it.
pub fn words(r: &Reach, host: &str) -> String {
    match r.cause {
        Some(Cause::TimedOut) => format!("connection timed out (likely the firewall on {host}: allow Scenic in System Settings › Network › Firewall there)"),
        Some(Cause::Refused) => format!("connection refused (nothing listens on {host}'s port: its agent stopped, or is restarting)"),
        Some(Cause::NoName) => format!("{host}'s address doesn't resolve"),
        _ if r.why.is_empty() => "no answer".into(),
        _ => r.why.clone(),
    }
}

/// The reports in heartbeats `beats` (in touch at the reader's `now`, not `to`'s own) that `to`
/// can't be reached: each with its reporter's heartbeat.
pub fn unreached<'a>(beats: &'a [Beat], to: &str, now: u64) -> Vec<(&'a Beat, &'a Reach)> {
    beats.iter().filter(|b| b.member != to && b.beat > 0 && !b.out_of_touch(now)).flat_map(|b| b.reach.iter().filter(|r| r.member == to && !r.ok).map(move |r| (b, r))).collect()
}

/// Why `to` can't be handed the lead for being unreachable, in words ("unreachable from m4:
/// connection timed out (…)"); None when no member in touch reports so.
pub fn why_unreachable(beats: &[Beat], to: &str, to_host: &str, now: u64) -> Option<String> {
    let u = unreached(beats, to, now);
    let (b, r) = u.first()?;
    let host = if b.host.is_empty() { b.member.as_str() } else { b.host.as_str() };
    Some(format!("unreachable from {host}: {}", words(r, to_host)))
}

/// How long `to` has been reported unreachable (s, the longest report, each by its reporter's
/// clock); None when it isn't.
pub fn unreachable_for(beats: &[Beat], to: &str, now: u64) -> Option<u64> {
    unreached(beats, to, now).iter().map(|(b, r)| b.beat.saturating_sub(r.since)).max()
}

/// Whether a member in touch other than `to` reached it within `REACHED_S` of its heartbeat.
pub fn reached(beats: &[Beat], to: &str, now: u64) -> bool {
    beats.iter().filter(|b| b.member != to && b.beat > 0 && !b.out_of_touch(now)).any(|b| b.reach.iter().any(|r| r.member == to && r.ok && b.beat.saturating_sub(r.at) <= REACHED_S))
}

#[cfg(test)]
mod tests {
    use super::super::beat::OUT_OF_TOUCH_S;
    use super::*;

    const A: &str = "m-000000000000000a";
    const B: &str = "m-000000000000000b";
    const C: &str = "m-000000000000000c";

    fn timed_out() -> Result<(), (Cause, String)> {
        Err((Cause::TimedOut, "connection timed out".into()))
    }

    #[test]
    fn failures_are_reported_once_they_span_report_s_and_cleared_by_a_success() {
        let mut t = Tracker::default();
        t.seen(A, "coordinator", timed_out(), 1000);
        t.seen(A, "coordinator", timed_out(), 1060);
        assert!(t.report(1060).is_empty(), "a minute of failures isn't reported");
        t.seen(A, "coordinator", timed_out(), 1000 + REPORT_S);
        let r = t.report(1000 + REPORT_S);
        assert_eq!((r.len(), r[0].ok, r[0].since, r[0].cause), (1, false, 1000, Some(Cause::TimedOut)));
        assert!(t.report(1000 + REPORT_S + REACHED_S + 1).is_empty(), "a stale failure says nothing");
        // A success clears it, at once; a late failure from before it counts for nothing.
        t.seen(A, "coordinator", Ok(()), 1200);
        t.seen(A, "coordinator", timed_out(), 1190);
        let r = t.report(1200);
        assert_eq!((r.len(), r[0].ok, r[0].at), (1, true, 1200));
        // Failing again starts anew.
        t.seen(A, "coordinator", timed_out(), 1260);
        assert!(t.report(1260).is_empty());
        t.retain(|m| m != A);
        t.seen(B, "broker", Ok(()), 1300);
        assert_eq!(t.report(1300).iter().map(|r| r.member.as_str()).collect::<Vec<_>>(), [B]);
    }

    #[test]
    fn readers_go_by_the_reports_of_members_in_touch() {
        let rep = |member: &str, host: &str, beat: u64, reach: Vec<Reach>| Beat { member: member.into(), host: host.into(), beat, reach, ..Default::default() };
        let bad = Reach { member: A.into(), what: "coordinator".into(), ok: false, at: 5000, since: 4000, cause: Some(Cause::TimedOut), why: "timed out".into() };
        let good = Reach { member: C.into(), what: "broker".into(), ok: true, at: 5000, since: 0, cause: None, why: String::new() };
        let beats = vec![rep(B, "m4", 5010, vec![bad.clone(), good.clone()]), rep(A, "m1", 5010, vec![Reach { member: A.into(), ..good.clone() }])];
        assert!(why_unreachable(&beats, A, "m1", 5020).is_some_and(|w| w.starts_with("unreachable from m4: connection timed out (likely the firewall on m1")));
        assert_eq!(unreachable_for(&beats, A, 5020), Some(1010));
        assert_eq!(why_unreachable(&beats, B, "m4", 5020), None);
        assert!(reached(&beats, C, 5020) && !reached(&beats, A, 5020), "its own report of itself doesn't count");
        // Out of touch, its reports count for nothing.
        assert_eq!(why_unreachable(&beats, A, "m1", 5010 + OUT_OF_TOUCH_S + 1), None);
        assert!(!reached(&beats, C, 5010 + OUT_OF_TOUCH_S + 1));
        assert_eq!(cause_of("reach the coordinator at http://m1:8090: io: Connection refused (os error 61)"), Cause::Refused);
        assert_eq!(cause_of("timeout: connect"), Cause::TimedOut);
    }
}
