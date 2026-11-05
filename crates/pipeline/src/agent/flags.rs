//! The status's flags (task #953; docs/pool.md §10, Flags): what the lead finds needs a look, worked
//! out once on the lead from its own status and its coordinator, and shown the same by `scenic
//! status`, the build page's alerts, the menu bar (the lead's own status, a member's lead line) and
//! tools/dev/health.sh.
//!
//! - **Idle** (`idle`): a Mac whose last ask for jobs was refused (`Status::packing`: it asked for
//!   more than it got, so a slot of it is free) while work it could take waits. Work offered and free
//!   to take (`Open`: targets not leased, of a step it may run): flagged with the refusal, which
//!   says what in its capacity held the job back (memory and its floor, cores, disk, the NAS budget,
//!   the grants, a cold start). None free, but the plan holds work of a step any Mac takes behind
//!   other work (`Status::waiting`'s entries with a step: slope-hi behind terrain-root): flagged
//!   with those waits, and the wait of the step each names where that one waits too, never just
//!   "nothing offered". Nothing left it could take: no flag.
//! - **Overdue** (`overdue`): a leased job running past `factor` times its usual time on its Mac
//!   (`Run::usual_s`: what the lead's coordinator predicts its targets take that Mac, summed:
//!   crate::coord's `Coordinator::runs`), with how far its batch is. `factor` is the pool's setting
//!   (`overdue-factor`, `scenic pool set` or the build page; crate::coord::asks::OVERDUE_DEFAULT
//!   while unset). A job whose usual time isn't known (a target of its step never measured on the
//!   way it runs now) isn't flagged.
//!
//! Nothing is flagged while the build is paused, nor on a Mac paused alone.

use super::{Status, Waiting};
use serde::{Deserialize, Serialize};

/// A flag: its kind ("idle", "overdue"), the Mac it's about and what it says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    pub kind: String,
    pub mac: String,
    pub said: String,
}

impl Flag {
    /// Its line, as `scenic status` and the health check print it: "idle: …", "overdue: …".
    pub fn line(&self) -> String {
        format!("{}: {}", self.kind, self.said)
    }
}

/// Work offered and free to take: a step's targets not leased (`own`: the lead's own planned job,
/// which no member takes).
#[derive(Clone, Debug, PartialEq)]
pub struct Open {
    pub step: String,
    pub targets: usize,
    pub own: bool,
}

/// A leased job as the lead's coordinator holds it: its worker, lease, step, targets (and those
/// done, as its worker's beat last said), threads, since when (unix s) and its usual time there (s).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub worker: String,
    pub lease: u64,
    pub step: String,
    pub targets: usize,
    pub done: Option<usize>,
    pub threads: u32,
    pub since: u64,
    pub usual_s: Option<u64>,
}

/// Every flag of the lead's status `st`, at `now`.
pub fn of(st: &Status, open: &[Open], runs: &[Run], factor: f64, now: u64) -> Vec<Flag> {
    if st.pause.is_some() {
        return Vec::new();
    }
    let mut v = idle(st, open);
    v.extend(overdue(st, runs, factor, now));
    v
}

/// Whether Mac `mac` is paused alone, as its own status says (the lead's, or a member's).
fn held(st: &Status, mac: &str) -> bool {
    std::iter::once(st).chain(st.helpers.iter()).any(|s| s.host == mac && s.held.is_some())
}

/// The idle flags (the module's doc): one per Mac refused a job.
pub fn idle(st: &Status, open: &[Open]) -> Vec<Flag> {
    let mut out: Vec<Flag> = Vec::new();
    for (worker, why) in &st.packing {
        let mac = crate::paces::mac_of(worker).to_string();
        if held(st, &mac) || out.iter().any(|f| f.mac == mac) {
            continue;
        }
        let lead = mac == st.host;
        let free: Vec<&Open> = open.iter().filter(|o| o.targets > 0 && (lead || !o.own)).collect();
        let said = if !free.is_empty() {
            let n: usize = free.iter().map(|o| o.targets).sum();
            let steps: Vec<String> = free.iter().map(|o| format!("{} {}", o.step, o.targets)).collect();
            format!(
                "{mac} has a slot free while {n} target{} it could take wait{} ({}): {why}",
                if n == 1 { "" } else { "s" },
                if n == 1 { "s" } else { "" },
                steps.join(", ")
            )
        } else {
            let waits = held_back(&st.waiting, lead);
            if waits.is_empty() {
                continue;
            }
            format!("{mac} has a slot free; the work left it could take waits: {}", waits.join("; "))
        };
        out.push(Flag { kind: "idle".into(), mac, said });
    }
    out
}

/// The plan's waits of work a Mac could take (any Mac's step; the lead's own too on the lead): each
/// "<step> <why>", and where its wait names another step that waits too, that one's after it.
fn held_back(waiting: &[Waiting], lead: bool) -> Vec<String> {
    let takes = |s: &str| lead || super::steps::place(s) == super::steps::Place::Any;
    waiting
        .iter()
        .filter_map(|w| w.step.as_deref().filter(|s| takes(s)).map(|s| (s, w)))
        .map(|(s, w)| {
            let behind: Vec<String> = waiting
                .iter()
                .filter_map(|o| o.step.as_deref().filter(|&t| t != s && names(&w.why, t)).map(|t| (t, o)))
                .map(|(t, o)| format!("{t}: {}", o.why))
                .collect();
            let then = if behind.is_empty() { String::new() } else { format!(" ({})", behind.join("; ")) };
            format!("{s}: {}{then}", w.why)
        })
        .collect()
}

/// Whether `text` names step `step` as a word ("terrain-root", not within "terrain-root-x").
fn names(text: &str, step: &str) -> bool {
    text.match_indices(step).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + step.len()..].chars().next();
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_');
        !word(before) && !word(after)
    })
}

/// The overdue flags (the module's doc): one per job past `factor` times its usual time.
pub fn overdue(st: &Status, runs: &[Run], factor: f64, now: u64) -> Vec<Flag> {
    runs.iter()
        .filter_map(|r| {
            let usual = r.usual_s.filter(|&u| u > 0)?;
            let ran = now.saturating_sub(r.since);
            let mac = crate::paces::mac_of(&r.worker).to_string();
            if (ran as f64) <= usual as f64 * factor || held(st, &mac) {
                return None;
            }
            let batch = match r.done {
                Some(d) if r.targets > 1 => format!(", {d} of {} targets done", r.targets),
                _ if r.targets > 1 => format!(", {} targets", r.targets),
                _ => String::new(),
            };
            Some(Flag {
                kind: "overdue".into(),
                mac,
                said: format!(
                    "{} (lease {}) has run {} on {}, past {factor}× its usual {} there{batch}, {} thread{}",
                    r.step,
                    r.lease,
                    span(ran),
                    crate::paces::mac_of(&r.worker),
                    span(usual),
                    r.threads,
                    if r.threads == 1 { "" } else { "s" }
                ),
            })
        })
        .collect()
}

/// What needs a look in the lead's status `st` at `now`, a line each (`scenic status --flags`,
/// tools/dev/health.sh): its flags; the pool's warnings: the pool's `alert` (no lead: crate::agent::lead),
/// the lead or a member out of touch (its beat older than `Status::out_of_touch_s`, or a Mac the
/// lead holds a capacity of with no status heard), the lead's loop stuck (`Status::loop_stuck_s`);
/// and the jobs that failed since `since` (unix s), every Mac's.
pub fn look(st: &Status, alert: Option<&str>, since: u64, now: u64) -> Vec<String> {
    let mut out: Vec<String> = st.flags.iter().map(Flag::line).collect();
    let touch = if st.out_of_touch_s > 0 { st.out_of_touch_s } else { crate::pool::beat::OUT_OF_TOUCH_S };
    if let Some(a) = alert {
        out.push(format!("warning: {a}"));
    }
    if now.saturating_sub(st.beat) > touch {
        out.push(format!("warning: the lead ({}) out of touch for {}", st.host, span(now.saturating_sub(st.beat))));
    }
    if let (Some(age), Some(stuck)) = (st.loop_age_s, st.loop_stuck_s) {
        if age > stuck {
            let phase = st.loop_phase.as_deref().map(|p| format!(" in {p}")).unwrap_or_default();
            out.push(format!("warning: the lead's loop stuck for {}{phase}", span(age)));
        }
    }
    for h in &st.helpers {
        if now.saturating_sub(h.beat) > touch {
            out.push(format!("warning: {} out of touch for {}", h.host, span(now.saturating_sub(h.beat))));
        }
    }
    let unheard: std::collections::BTreeSet<&str> = st
        .capacity
        .iter()
        .map(|(n, _)| crate::paces::mac_of(n))
        .filter(|m| *m != st.host && !st.helpers.iter().any(|h| h.host == *m))
        .collect();
    for m in unheard {
        out.push(format!("warning: {m}: its status not heard lately (asleep, away, or its agent down)"));
    }
    for s in std::iter::once(st).chain(st.helpers.iter()) {
        for d in s.recent.iter().filter(|d| !d.ok && !d.cancelled && d.ended >= since) {
            let why = d.note.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            out.push(format!("failed: {}: {} ({}), {}", s.host, d.what, d.id, why));
        }
    }
    out
}

/// A span in words: "40 min", "2.5 h".
fn span(s: u64) -> String {
    if s < 5400 {
        format!("{} min", s.div_ceil(60))
    } else {
        format!("{:.1} h", s as f64 / 3600.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(step: &str, why: &str) -> Waiting {
        Waiting { what: step.into(), why: why.into(), step: Some(step.into()) }
    }

    fn lead() -> Status {
        Status { host: "mini".into(), ..Default::default() }
    }

    /// A Mac refused a job while targets it could take wait: flagged with the refusal (its
    /// capacity's reason); the lead's own planned jobs count for the lead alone.
    #[test]
    fn a_refused_mac_with_work_free_is_flagged_with_why() {
        let mut st = lead();
        st.packing.insert("m4 #2".into(), "memory: 1.2 GB for a new job (6.0 GB floor)".into());
        st.packing.insert("m4".into(), "the same Mac's first slot".into());
        let open = [Open { step: "slope-deep".into(), targets: 12, own: false }];
        let f = idle(&st, &open);
        assert_eq!(f.len(), 1, "one per Mac: {f:?}");
        assert_eq!(f[0].mac, "m4");
        assert!(f[0].said.contains("12 targets it could take wait (slope-deep 12)"), "{}", f[0].said);
        assert!(f[0].line().starts_with("idle: m4 has a slot free"), "{}", f[0].line());
        // (The lead's own job is no member's.)
        let own = [Open { step: "osm-pass".into(), targets: 1, own: true }];
        assert!(idle(&st, &own).is_empty(), "nothing it could take, and nothing held back");
        st.packing.insert("mini".into(), "cores: 0 of 8 left, a job of osm-pass takes 2".into());
        let f = idle(&st, &own);
        assert_eq!((f.len(), f[0].mac.as_str()), (1, "mini"), "{f:?}");
        assert!(f[0].said.contains("cores: 0 of 8 left"), "{}", f[0].said);
    }

    /// Nothing free, but work of a shared step held behind another step that waits too (4 Nov:
    /// slope-hi behind terrain-root): the flag names both waits, never "nothing offered" alone.
    #[test]
    fn an_idle_mac_names_the_wait_its_work_is_held_behind() {
        let mut st = lead();
        st.packing.insert("m1".into(), "nothing offered fits the room left (memory: 9 GB) or is free to take".into());
        st.waiting = vec![
            wait("slope-hi", "wait for terrain-root: 3 z3 tiles not current"),
            wait("terrain-root", "waits for the lead's NAS budget"),
            Waiting { what: "Building the regions".into(), why: "unreadable".into(), step: None },
        ];
        let f = idle(&st, &[]);
        assert_eq!(f.len(), 1, "{f:?}");
        let s = &f[0].said;
        assert!(s.contains("slope-hi: wait for terrain-root: 3 z3 tiles not current (terrain-root: waits for the lead's NAS budget)"), "{s}");
        assert!(!s.contains("nothing offered"), "{s}");
        // (terrain-root is the lead's alone: its own wait is no member's to take, but it is named
        // as what slope-hi waits behind.)
        assert!(!s.contains("; terrain-root:"), "{s}");
        // Nothing waits at all: the Mac has nothing it could do, no flag.
        st.waiting.clear();
        assert!(idle(&st, &[]).is_empty());
        // A Mac paused alone isn't flagged; nor is any while the build is paused.
        st.waiting = vec![wait("slope-hi", "wait for terrain-root")];
        st.helpers.push(Status { host: "m1".into(), held: Some(crate::control::Hold::new("t")), ..Default::default() });
        assert!(idle(&st, &[]).is_empty());
        st.helpers.clear();
        st.pause = Some(crate::control::Pause::new(crate::control::Mode::Drain, "t"));
        assert!(of(&st, &[], &[], 3.0, 0).is_empty());
    }

    /// A job past the factor times its usual time is flagged with its batch's progress; within it,
    /// or of no usual time known, not.
    #[test]
    fn a_job_past_the_factor_is_flagged() {
        let st = lead();
        let run = Run {
            worker: "m4 #2".into(),
            lease: 7,
            step: "slope-deep".into(),
            targets: 36,
            done: Some(12),
            threads: 3,
            since: 1_000,
            usual_s: Some(3_600),
        };
        assert!(overdue(&st, std::slice::from_ref(&run), 3.0, 1_000 + 3 * 3_600).is_empty(), "at 3× exactly");
        let f = overdue(&st, std::slice::from_ref(&run), 3.0, 1_000 + 5 * 3_600);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].mac, "m4");
        assert_eq!(
            f[0].line(),
            "overdue: slope-deep (lease 7) has run 5.0 h on m4, past 3× its usual 60 min there, 12 of 36 targets done, 3 threads"
        );
        assert!(overdue(&st, std::slice::from_ref(&run), 6.0, 1_000 + 5 * 3_600).is_empty(), "a larger factor set");
        let unknown = Run { usual_s: None, ..run };
        assert!(overdue(&st, &[unknown], 3.0, 1_000 + 50 * 3_600).is_empty());
    }

    /// The health check's lines: the flags, the pool's warnings, the failures since the stamp alone.
    #[test]
    fn look_says_flags_warnings_and_failures_since() {
        let now = 100_000;
        let done = |id: &str, ok: bool, ended: u64| super::super::Done {
            id: id.into(),
            what: format!("{id}'s work"),
            ok,
            ended,
            secs: 60,
            note: "reading\nError: the NAS went away\n".into(),
            cancelled: false,
        };
        let mut st = Status { host: "mini".into(), beat: now - 5, out_of_touch_s: 300, ..Default::default() };
        st.flags.push(Flag { kind: "idle".into(), mac: "m4".into(), said: "m4 has a slot free".into() });
        st.recent = vec![done("slope-deep 10/1/1", false, now - 10), done("pack 6/1/1", false, now - 9_000)];
        st.helpers.push(Status {
            host: "m4".into(),
            beat: now - 1_000,
            recent: vec![done("unit 6/2/2", true, now)],
            ..Default::default()
        });
        st.capacity.push(("m1 #2".into(), Default::default()));
        st.loop_age_s = Some(900);
        st.loop_stuck_s = Some(600);
        let l = look(&st, Some("No lead for 2 h"), now - 3_600, now);
        assert_eq!(
            l,
            vec![
                "idle: m4 has a slot free",
                "warning: No lead for 2 h",
                "warning: the lead's loop stuck for 15 min",
                "warning: m4 out of touch for 17 min",
                "warning: m1: its status not heard lately (asleep, away, or its agent down)",
                "failed: mini: slope-deep 10/1/1's work (slope-deep 10/1/1), Error: the NAS went away",
            ]
        );
    }

    #[test]
    fn a_step_is_named_as_a_word() {
        assert!(names("wait for terrain-root: 3", "terrain-root"));
        assert!(!names("wait for terrain-roots", "terrain-root"));
        assert!(!names("slope-hi-x", "slope-hi"));
    }
}
