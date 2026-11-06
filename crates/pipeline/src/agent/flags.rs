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
//! - **Stuck** (`stuck`): a leased job whose work hasn't moved, by its worker's beats, for longer
//!   than `factor` times its step's usual stillness (`Still`). Its work moved at a beat when
//!   anything of it changed since the one before (crate::coord::lease::Lease::still): its progress
//!   line, the target under way, its targets ended, its phase, its NAS bytes, files made, listed,
//!   statted or deleted, its internet bytes. Its step's usual stillness is measured from its jobs
//!   that ended well (crate::coord::history::History::still_p90): the 90th percentile of each one's
//!   longest stretch between two beats that saw its work move. So a job judged is held to what it
//!   is doing now, not to how long its targets took before: a target fetching for the first time,
//!   copying from a cold cache or writing to a slow NAS moves bytes all the while (6 Nov:
//!   pyramids-fill's first GSI fetches at 19-25 min, bldprep and pack while the NAS was slow,
//!   terrain-hi copying GLO-30 at 4 MB/s, all flagged by their targets' earlier times while
//!   moving), and a job that stopped is found within a few of its step's still stretches (5 Nov:
//!   terrain-hi jobs at 0 tiles and no CPU for 20 min and more). A job slow but moving isn't
//!   flagged: there is no flag of a job's time as a whole. Beats a worker didn't send say nothing
//!   (its being out of touch is a warning of its own, `look`).
//! - **Stale** (`stale`): the lead's catalogue due (`publish::PublishLine::due_since`: held back,
//!   planned or waiting out its minute) for longer than `factor` times the most the plan holds one
//!   back and waits between two (publish::CATALOGUE_CAP_S and CATALOGUE_EVERY_S), counted from the
//!   last catalogue's going out where that's later, and no catalogue job leased (that one is
//!   stuck by its own stillness): a catalogue that should have gone out and didn't (its job
//!   never started: the lead's slots full, its plan held). A catalogue merely old, nothing new to
//!   list, isn't stale.
//!
//! `factor` is the pool's setting (`overdue-factor`, `scenic pool set` or the build page;
//! crate::coord::asks::OVERDUE_DEFAULT while unset). A step none of whose jobs that ended well
//! measured its stillness flags nothing.
//!
//! Nothing is flagged while the build is paused, nor on a Mac paused alone.

use super::{Status, Waiting};
use serde::{Deserialize, Serialize};

/// A flag: its kind ("idle", "stuck", "stale"), the Mac it's about and what it says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    pub kind: String,
    pub mac: String,
    pub said: String,
}

impl Flag {
    /// Its line, as `scenic status` and the health check print it: "idle: …", "stuck: …", "stale: …".
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

/// A leased job as the lead's coordinator holds it (crate::coord's `Coordinator::runs`): its worker,
/// lease, step, targets (and those done, as its worker's beat last said), threads, and how still
/// it is (None: no beat heard yet).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub worker: String,
    pub lease: u64,
    pub step: String,
    pub targets: usize,
    pub done: Option<usize>,
    pub threads: u32,
    pub still: Option<Still>,
}

/// How long a job's work hasn't moved by its worker's beats (s), its step's usual stillness (s,
/// crate::coord::history::History::still_p90; None: not measured), and where it is, as its last
/// beat said: the target under way (empty: none, its start) and its phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Still {
    pub secs: u64,
    pub usual_s: Option<u64>,
    pub target: String,
    pub phase: Option<String>,
}

/// Every flag of the lead's status `st`, at `now`.
pub fn of(st: &Status, open: &[Open], runs: &[Run], factor: f64, now: u64) -> Vec<Flag> {
    if st.pause.is_some() {
        return Vec::new();
    }
    let mut v = idle(st, open);
    v.extend(stuck(st, runs, factor));
    v.extend(stale(st, runs, factor, now));
    v
}

/// The stale flag (the module's doc): at most one, the lead's.
pub fn stale(st: &Status, runs: &[Run], factor: f64, now: u64) -> Option<Flag> {
    let due = st.publishing.as_ref()?.due_since?;
    let since = st.catalogue.as_ref().map_or(due, |c| c.at.max(due));
    let most = super::publish::CATALOGUE_CAP_S + super::publish::CATALOGUE_EVERY_S;
    let ran = now.saturating_sub(since);
    if (ran as f64) <= most as f64 * factor || runs.iter().any(|r| super::pool::PUBLISHES.contains(&r.step.as_str())) {
        return None;
    }
    let last =
        st.catalogue.as_ref().map_or("none yet".to_string(), |c| format!("{} ago", span(now.saturating_sub(c.at))));
    let why = st.publishing.as_ref().map_or("", |p| p.why.as_str());
    Some(Flag {
        kind: "stale".into(),
        mac: st.host.clone(),
        said: format!(
            "a catalogue due for {} hasn't gone out (past {factor}× the {} the plan holds one at most; the last {last}): {why}",
            span(ran),
            span(most)
        ),
    })
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
    let takes = |s: &str| lead || super::every::place(s) == super::every::Place::Any;
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

/// The stuck flags (the module's doc): one per job still past `factor` times its step's usual.
pub fn stuck(st: &Status, runs: &[Run], factor: f64) -> Vec<Flag> {
    runs.iter()
        .filter_map(|r| {
            let mac = crate::paces::mac_of(&r.worker).to_string();
            let s = r.still.as_ref()?;
            let usual = s.usual_s?;
            if held(st, &mac) || (s.secs as f64) <= usual as f64 * factor {
                return None;
            }
            let batch = match r.done {
                Some(d) if r.targets > 1 => format!(", {d} of {} targets done", r.targets),
                _ if r.targets > 1 => format!(", {} targets", r.targets),
                _ => String::new(),
            };
            let threads = format!("{} thread{}", r.threads, if r.threads == 1 { "" } else { "s" });
            let at = if s.target.is_empty() { "its start".to_string() } else { s.target.clone() };
            let phase = s.phase.as_ref().map_or(String::new(), |p| format!(", {p}"));
            let said = format!(
                "{} (lease {}) on {mac}: nothing moved for {} ({at}{phase}), past {factor}× its step's usual still stretch of {}{batch}, {threads}",
                r.step,
                r.lease,
                span(s.secs),
                span(usual)
            );
            Some(Flag { kind: "stuck".into(), mac, said })
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

    fn run(step: &str, secs: u64, usual: Option<u64>) -> Run {
        Run {
            worker: "m1 #2".into(),
            lease: 9,
            step: step.into(),
            targets: 8,
            done: Some(2),
            threads: 4,
            still: Some(Still { secs, usual_s: usual, target: "6/55/26".into(), phase: Some("sampling glo30".into()) }),
        }
    }

    /// A job whose work stopped is stuck past the factor times its step's usual stillness, said
    /// with where it is and its batch's progress; within it, or of a step not measured, or before
    /// its first beat, not.
    #[test]
    fn a_job_still_past_the_factor_is_stuck() {
        let st = lead();
        let at = |r: &Run, f: f64| stuck(&st, std::slice::from_ref(r), f);
        // (5 Nov: terrain-hi jobs at 0 tiles and no CPU, still 20 min against a usual 4 min.)
        let f = at(&run("terrain-hi", 20 * 60, Some(240)), 3.0);
        assert_eq!(
            f.iter().map(Flag::line).collect::<Vec<_>>(),
            ["stuck: terrain-hi (lease 9) on m1: nothing moved for 20 min (6/55/26, sampling glo30), past 3× its step's usual still stretch of 4 min, 2 of 8 targets done, 4 threads"]
        );
        assert!(at(&run("terrain-hi", 3 * 240, Some(240)), 3.0).is_empty(), "at 3× exactly");
        assert!(at(&run("terrain-hi", 20 * 60, Some(240)), 6.0).is_empty(), "a larger factor set");
        assert!(at(&run("terrain-hi", 50 * 3_600, None), 3.0).is_empty(), "its step not measured");
        assert!(at(&Run { still: None, ..run("terrain-hi", 0, Some(240)) }, 3.0).is_empty(), "no beat heard");
        let start =
            Run { still: Some(Still { secs: 900, usual_s: Some(60), ..Default::default() }), ..run("pack", 0, None) };
        assert!(
            at(&start, 3.0)[0].said.contains("nothing moved for 15 min (its start), past"),
            "{:?}",
            at(&start, 3.0)
        );
        // A Mac paused alone isn't flagged.
        let mut st = lead();
        st.helpers.push(Status { host: "m1".into(), held: Some(crate::control::Hold::new("t")), ..Default::default() });
        assert!(stuck(&st, &[run("terrain-hi", 20 * 60, Some(240))], 3.0).is_empty());
    }

    /// The false flags of 6 Nov's health checks: jobs long past their targets' earlier times while
    /// their work moved (a first fetch, a cold copy, a slow NAS) are still a moment at most, so not
    /// stuck, however long their target takes; there is no flag of a job's time as a whole.
    #[test]
    fn a_slow_job_still_moving_is_not_flagged() {
        let st = lead();
        // (pyramids-fill 6/55/26 under way 24 min, its targets' earlier runs 1 min: its last beat
        // saw its fetch's bytes move, so it is still 0 s.)
        let fill = Run {
            still: Some(Still { secs: 0, usual_s: Some(120), ..Default::default() }),
            ..run("pyramids-fill", 0, None)
        };
        assert!(of(&st, &[], &[fill], 3.0, 0).is_empty());
    }

    /// A catalogue due past the factor times the plan's most hold (11 min) and not going out is
    /// stale, counted from the last catalogue where later; while a catalogue job runs, or with
    /// none due (a catalogue merely old), not.
    #[test]
    fn a_catalogue_due_too_long_is_stale() {
        let now = 100_000;
        let mut st = lead();
        st.catalogue = Some(super::super::CatalogueSeen { n: 812, at: now - 6 * 3_600 });
        assert!(stale(&st, &[], 3.0, now).is_none(), "nothing due");
        let due = |since: u64| {
            Some(super::super::publish::PublishLine {
                why: "a catalogue now: what it would list changed".into(),
                due_since: Some(since),
                ..Default::default()
            })
        };
        st.publishing = due(now - 40 * 60);
        let f = stale(&st, &[], 3.0, now).expect("40 min due");
        assert_eq!(
            f.line(),
            "stale: a catalogue due for 40 min hasn't gone out (past 3× the 11 min the plan holds one at most; the last 6.0 h ago): a catalogue now: what it would list changed"
        );
        assert!(stale(&st, &[], 3.0, now - 8 * 60).is_none(), "32 min: within 3× 11 min");
        // (A catalogue went out 10 min ago, another due since: counted from it.)
        st.catalogue = Some(super::super::CatalogueSeen { n: 813, at: now - 600 });
        assert!(stale(&st, &[], 3.0, now).is_none());
        st.catalogue = None;
        let job = Run {
            worker: "mini".into(),
            lease: 3,
            step: "catalog".into(),
            targets: 0,
            done: None,
            threads: 1,
            still: None,
        };
        assert!(stale(&st, &[job], 3.0, now).is_none(), "its job is under way");
        assert!(stale(&st, &[], 3.0, now).is_some());
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
