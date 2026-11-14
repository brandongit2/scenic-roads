//! The status's flags (task #953; docs/pool.md §10, Flags): what the lead finds needs a look, worked
//! out once on the lead from its own status and its coordinator, and shown the same by `scenic
//! status`, the build page's alerts, the menu bar (the lead's own status, a member's lead line) and
//! tools/dev/health.sh.
//!
//! - **Idle** (`idle`): a Mac whose last ask for jobs was refused (`Status::packing`: it asked for
//!   more than it got, so a slot of it is free) while work it could take waits. Work offered and free
//!   to take (`Open`: targets not leased, of a step it may run): flagged with the refusal, which
//!   says what in its capacity held the job back (memory and its floor, cores, disk, the NAS budget,
//!   the grants, a cold start). Refused for its room (crate::coord::sched's `Refused`, task #1170),
//!   it names the first target past it (step and tile), what that needs (memory, disk) and what
//!   this Mac is short of, each by how much ("lo 3/6/3 needs 12.7 GB of memory and 35.2 GB of disk:
//!   disk short by 19.2 GB (46 GB free · 16 GB for jobs (30 GB room target))"), then how many of
//!   the other targets offered are held by jobs, done, in backoff after failing here, or free but
//!   held back otherwise; room held
//!   for that target while this Mac's jobs end is said as the hold ("room held for a job of …").
//!   Each Mac's flag says its own room, so the flags of the Macs together say what each lacks.
//!   None free, but the plan holds work of a step any Mac takes behind
//!   other work (`Status::waiting`'s entries with a step: slope-hi behind terrain-root): flagged
//!   with those waits, and the wait of the step each names where that one waits too, never just
//!   "nothing offered". Nothing left it could take: no flag. A Mac no offer reached the room rule
//!   for says so in its refusal (`Status::packing`, `nothing_words`): the work offered to other
//!   Macs, the plan's waits, else that the plan is complete; never the room left.
//! - **Stuck** (`stuck`): a leased job whose work hasn't moved, by its worker's beats, for longer
//!   than `factor` times its step's usual stillness (`Still`). Its work moved at a beat when
//!   anything of it changed since the one before (crate::coord::lease::Lease::still): its progress
//!   line, the target under way, its targets ended, its phase, its NAS bytes, files made, listed,
//!   statted or deleted (a whole file copied counts its bytes as they come, store::nasfs's
//!   `copy_counter`), its internet bytes. Its step's usual stillness is measured from its jobs that
//!   ended well (crate::coord::history::History::still_p90s): the 90th percentile of each one's
//!   longest stretch between two beats that saw its work move (`Usual`). So a job judged is held to what it
//!   is doing now, not to how long its targets took before: a target fetching for the first time,
//!   copying from a cold cache or writing to a slow NAS moves bytes all the while (6 Nov:
//!   pyramids-fill's first GSI fetches at 19-25 min, bldprep and pack while the NAS was slow,
//!   terrain-hi copying GLO-30 at 4 MB/s, all flagged by their targets' earlier times while
//!   moving), and a job that stopped is found within a few of its step's still stretches (5 Nov:
//!   terrain-hi jobs at 0 tiles and no CPU for 20 min and more). A job slow but moving isn't
//!   flagged stuck (it may be slow): there is no flag of a job's time as a whole. Beats a worker
//!   didn't send say nothing (its being out of touch is a warning of its own, `look`).
//! - **Slow** (`slow`): a leased job whose progress, by its worker's beats, went on over the last
//!   `factor` times its step's usual longest wait for progress (the window) at less than its
//!   step's usual rate divided by `factor` (`Pace`). Its rate is in the unit its progress line says
//!   (tiles, z6 tiles, packs…: crate::coord::lease::Pace), within its part under way (one line's
//!   run on one target): the most the line has said, so a line gone back (work done again) counts
//!   nothing until it passes it, and its job view's bytes never count (a retry sending its bytes
//!   again moves no line). Its step's usual rate in that unit is the 10th percentile of its jobs' that ended well
//!   (each one's progress in the unit over the time its parts in it took: `rates` on its "done" or
//!   "end" event), its usual wait the 90th percentile of their longest stretch without progress
//!   (`gap_s`), both from its jobs that had no more threads than its part under way has had at its
//!   fewest (`threads` on the events: more threads may make a job faster, never slower, so one on
//!   fewer isn't held to runs on more), each from `ENOUGH` jobs at least
//!   (crate::coord::history::Paces::usual): a step with fewer, a part younger than its window,
//!   a job whose threads aren't said, isn't judged. A window that long holds a working
//!   job's progress however bursty its line (terrain-hi says its tiles a few hundred at a time);
//!   one with none in it is at a rate of nothing. A job flagged stuck isn't flagged slow too.
//! - **Stale** (`stale`): the lead's catalogue due (`publish::PublishLine::due_since`: held back,
//!   planned or waiting out its minute) for longer than `factor` times the most the plan holds one
//!   back and waits between two (publish::CATALOGUE_CAP_S and CATALOGUE_EVERY_S), counted from the
//!   last catalogue's going out where that's later, and no catalogue job leased (that one is
//!   stuck by its own stillness): a catalogue that should have gone out and didn't (its job
//!   never started: the lead's slots full, its plan held). A catalogue merely old, nothing new to
//!   list, isn't stale.
//! - **Repeat** (`repeat`): a run of the lead's GC or backup that ended well having done nothing
//!   (its report: `Done::empty`) within `REPEATS`' time of that duty's run before it ending, while
//!   it is that recent: the duty planned again with nothing for it to do (13 Nov: GC granted 192
//!   times in 100 s after a run that removed 1,427 files, each run removing none).
//!
//! `factor` (stuck's, slow's and stale's) is the pool's setting (`overdue-factor`, `scenic pool
//! set` or the build page; crate::coord::asks::OVERDUE_DEFAULT while unset). A step none of whose jobs that ended well
//! measured its stillness (its first runs; every step as the stillness was first recorded) is held
//! to its target's time instead, which no still stretch of a working job outlasts, else to the
//! longest usual stillness of any step (`Usual`); nothing measured at all flags nothing.
//!
//! Nothing is flagged while the build is paused, nor on a Mac paused alone.

use super::{Status, Waiting};
use serde::{Deserialize, Serialize};

/// A flag: its kind ("idle", "stuck", "slow", "stale", "repeat"), the Mac it's about and what it says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    pub kind: String,
    pub mac: String,
    pub said: String,
}

impl Flag {
    /// Its line, as `scenic status` and the health check print it: "idle: …", "stuck: …", "slow: …",
    /// "stale: …", "repeat: …".
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
/// lease, step, targets (and those done, as its worker's beat last said), threads, how still it is
/// (None: no beat heard yet) and its pace (None: not judged).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub worker: String,
    pub lease: u64,
    pub step: String,
    pub targets: usize,
    pub done: Option<usize>,
    pub threads: u32,
    pub still: Option<Still>,
    pub pace: Option<Pace>,
}

/// A job's progress rate over its window (`window_s`) in its part under way's unit, and its step's
/// usual rate in that unit from its runs of `threads` threads or fewer (units a second: the
/// module's doc, Slow).
#[derive(Clone, Debug, PartialEq)]
pub struct Pace {
    pub unit: String,
    pub rate: f64,
    pub usual: f64,
    pub window_s: u64,
    pub threads: u32,
}

/// How long a job's work hasn't moved by its worker's beats (s), the usual it is held to (s, and
/// whose; None: nothing measured), and where it is, as its last beat said: the target under way
/// (empty: none, its start) and its phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Still {
    pub secs: u64,
    pub usual: Option<(u64, Usual)>,
    pub target: String,
    pub phase: Option<String>,
}

/// Whose usual a job's stillness is held to (crate::coord's `Coordinator::runs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usual {
    /// Its step's usual stillness (crate::coord::history::History::still_p90s).
    Step,
    /// Its step's not measured yet: its target's time at most as measured (its start's too before
    /// its first target), crate::coord::sched's `Sched::target_most`.
    Target,
    /// Neither: the longest usual stillness of any step.
    Pool,
}

impl Usual {
    /// What the flag calls it.
    fn said(self) -> &'static str {
        match self {
            Usual::Step => "its step's usual still stretch",
            Usual::Target => "its target's usual time (its step's stillness not measured yet)",
            Usual::Pool => "the longest usual still stretch of any step (its step not measured yet)",
        }
    }
}

/// Every flag of the lead's status `st`, at `now`.
pub fn of(
    the_plan: &dyn crate::agent::seam::Plan,
    st: &Status,
    open: &[Open],
    runs: &[Run],
    factor: f64,
    now: u64,
) -> Vec<Flag> {
    if st.pause.is_some() {
        return Vec::new();
    }
    let mut v = idle(the_plan, st, open);
    v.extend(stuck(st, runs, factor));
    // (A job stuck isn't slow too.)
    let moving: Vec<Run> =
        runs.iter().filter(|r| stuck(st, std::slice::from_ref(r), factor).is_empty()).cloned().collect();
    v.extend(slow(st, &moving, factor));
    v.extend(stale(st, runs, factor, now));
    v.extend(repeat(st, now));
    v
}

/// The lead's duties whose runs say whether they did anything (`Done::empty`), each with how soon
/// after its run before ended one that did nothing and ended is a repeat: GC's queue frees a name
/// its grace after nothing holds it, so a run within that of the last one finds what that one left;
/// the backup is planned a day after its last run that ended well (`Memory::last_ok`, the same
/// second as its `Done::ended`), so the next day's ends a day or more after it.
pub const REPEATS: [(&str, u64); 2] = [("gc", super::gcqueue::GRACE_S), ("backup", super::backup::EVERY_S)];

/// The repeat flags (the module's doc): one per duty of `REPEATS` whose newest run did nothing and
/// ended within its time of the run before's end, while that newest run is within it of `now`.
pub fn repeat(st: &Status, now: u64) -> Vec<Flag> {
    let mut out = Vec::new();
    for (step, within) in REPEATS {
        // (Its runs as the status keeps them, newest first.)
        let runs: Vec<&super::Done> =
            st.recent.iter().filter(|d| !d.cancelled && super::step_of(&d.id).as_deref() == Some(step)).collect();
        let (Some(last), Some(before)) = (runs.first(), runs.get(1)) else { continue };
        let gap = last.ended.saturating_sub(before.ended);
        let ago = now.saturating_sub(last.ended);
        if !last.empty || gap >= within || ago >= within {
            continue;
        }
        let empty = runs.iter().filter(|d| d.empty).count();
        out.push(Flag {
            kind: "repeat".into(),
            mac: st.host.clone(),
            said: format!(
                "{step} ran again and did nothing, ending {} after its run before, the last {} ago ({empty} of its last {} runs did nothing): planned again with nothing for it to do",
                if gap < 60 { format!("{gap} s") } else { span(gap) },
                span(ago),
                runs.len()
            ),
        });
    }
    out
}

/// The stale flag (the module's doc): at most one, the lead's.
pub fn stale(st: &Status, runs: &[Run], factor: f64, now: u64) -> Option<Flag> {
    let due = st.publishing.as_ref()?.due_since?;
    let since = st.catalogue.as_ref().map_or(due, |c| c.at.max(due));
    let most = super::publish::CATALOGUE_CAP_S + super::publish::CATALOGUE_EVERY_S;
    let ran = now.saturating_sub(since);
    if (ran as f64) <= most as f64 * factor
        || runs.iter().any(|r| super::poolside::PUBLISHES.contains(&r.step.as_str()))
    {
        return None;
    }
    let last =
        st.catalogue.as_ref().map_or("none yet".to_string(), |c| format!("{} ago", span(now.saturating_sub(c.at))));
    let why = st.publishing.as_ref().map_or("", |p| p.why.as_str());
    Some(Flag {
        kind: "stale".into(),
        mac: st.host.clone(),
        said: format!(
            "a map update due for {} hasn't gone out (past {factor}× the {} the plan holds one at most; the last {last}): {why}",
            span(ran),
            span(most)
        ),
    })
}

/// Whether Mac `mac` is paused alone, as its own status says (the lead's, or a member's).
fn held(st: &Status, mac: &str) -> bool {
    std::iter::once(st).chain(st.members.iter()).any(|s| s.host == mac && s.held.is_some())
}

/// The idle flags (the module's doc): one per Mac refused a job.
pub fn idle(the_plan: &dyn crate::agent::seam::Plan, st: &Status, open: &[Open]) -> Vec<Flag> {
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
            let waits = held_back(the_plan, &st.waiting, lead);
            if waits.is_empty() {
                continue;
            }
            format!("{mac} has a slot free; the work left it could take waits: {}", waits.join("; "))
        };
        out.push(Flag { kind: "idle".into(), mac, said });
    }
    out
}

/// A Mac's reason for running nothing more when no job offered reached the room rule
/// (`coord::sched::Rule::Empty`, `Rule::Away`): `away`, the coordinator's words of the work offered
/// that isn't this Mac's (`Rule::Away`'s), then the plan's waits (`Status::waiting`, the lead's):
/// work a Mac could take waiting behind other work or an input (`held_back`), named with its wait;
/// on a member, the steps left only the lead runs; and the waits of no step (the records unreadable,
/// the first OpenStreetMap pass, the disk room target), as the verdict says them ("Waiting: …").
/// Only with none of these is the plan complete, as the verdict's "Nothing to build" is. No room
/// figures: nothing was refused for room. Each list cut to its first few.
pub fn nothing_words(
    the_plan: &dyn crate::agent::seam::Plan,
    waiting: &[Waiting],
    lead: bool,
    away: Option<&str>,
) -> String {
    const SHOWN: usize = 4;
    let cut = |mut list: Vec<String>, sep: &str| {
        list.dedup();
        let more = list.len().saturating_sub(SHOWN);
        list.truncate(SHOWN);
        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
        format!("{}{more}", list.join(sep))
    };
    let mut said: Vec<String> = away.map(str::to_string).into_iter().collect();
    let waits = held_back(the_plan, waiting, lead);
    if !waits.is_empty() {
        said.push(format!("the work left waits: {}", cut(waits, "; ")));
    }
    if !lead {
        let mut only: Vec<String> = Vec::new();
        for s in waiting.iter().filter_map(|w| w.step.as_deref()) {
            if super::every::place(the_plan, s) != super::every::Place::Any && !only.iter().any(|o| o == s) {
                only.push(s.to_string());
            }
        }
        if !only.is_empty() {
            said.push(format!("work left only the lead runs: {}", cut(only, ", ")));
        }
    }
    let plain: Vec<String> =
        waiting.iter().filter(|w| w.step.is_none()).map(|w| format!("{}: {}", w.what, w.why)).collect();
    if !plain.is_empty() {
        let who = if lead { "waiting" } else { "the lead waits" };
        said.push(format!("{who}: {}", cut(plain, "; ")));
    }
    if said.is_empty() {
        "the plan is complete: no job is left to offer".to_string()
    } else {
        said.join("; ")
    }
}

/// The plan's waits of work a Mac could take (any Mac's step; the lead's own too on the lead): each
/// "<step> <why>", and where its wait names another step that waits too, that one's after it.
fn held_back(the_plan: &dyn crate::agent::seam::Plan, waiting: &[Waiting], lead: bool) -> Vec<String> {
    let takes = |s: &str| lead || super::every::place(the_plan, s) == super::every::Place::Any;
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

/// The stuck flags (the module's doc): one per job still past `factor` times its usual.
pub fn stuck(st: &Status, runs: &[Run], factor: f64) -> Vec<Flag> {
    runs.iter()
        .filter_map(|r| {
            let mac = crate::paces::mac_of(&r.worker).to_string();
            let s = r.still.as_ref()?;
            let (usual, of) = s.usual?;
            if held(st, &mac) || (s.secs as f64) <= usual as f64 * factor {
                return None;
            }
            let at = if s.target.is_empty() { "its start".to_string() } else { s.target.clone() };
            let phase = s.phase.as_ref().map_or(String::new(), |p| format!(", {p}"));
            let said = format!(
                "{} (lease {}) on {mac}: nothing moved for {} ({at}{phase}), past {factor}× {} of {}{}, {}",
                r.step,
                r.lease,
                span(s.secs),
                of.said(),
                span(usual),
                batch(r),
                threads(r)
            );
            Some(Flag { kind: "stuck".into(), mac, said })
        })
        .collect()
}

/// The slow flags (the module's doc): one per job whose progress went on over its window at less
/// than its step's usual rate divided by `factor`.
pub fn slow(st: &Status, runs: &[Run], factor: f64) -> Vec<Flag> {
    runs.iter()
        .filter_map(|r| {
            let mac = crate::paces::mac_of(&r.worker).to_string();
            let p = r.pace.as_ref()?;
            if held(st, &mac) || p.rate >= p.usual / factor {
                return None;
            }
            let at = r.still.as_ref().map_or(String::new(), |s| {
                let t = if s.target.is_empty() { "its start" } else { s.target.as_str() };
                format!(" ({t}{})", s.phase.as_ref().map_or(String::new(), |p| format!(", {p}")))
            });
            let said = format!(
                "{} (lease {}) on {mac}: {} {} a minute over the last {}{at}, under its step's usual {} a minute (the slowest tenth of its jobs at {}) ÷ {factor}{}, {}",
                r.step,
                r.lease,
                per_min(p.rate),
                p.unit,
                span(p.window_s),
                per_min(p.usual),
                if p.threads == 1 { "one thread".to_string() } else { format!("{} threads or fewer", p.threads) },
                batch(r),
                threads(r)
            );
            Some(Flag { kind: "slow".into(), mac, said })
        })
        .collect()
}

/// A rate a second as a number a minute, to three figures or so: "0", "0.042", "1.5", "37".
fn per_min(x: f64) -> String {
    let m = x * 60.0;
    if m <= 0.0 || !m.is_finite() {
        return "0".into();
    }
    let places = match m {
        m if m >= 100.0 => 0,
        m if m >= 10.0 => 1,
        m if m >= 1.0 => 2,
        m if m >= 0.1 => 3,
        _ => 4,
    };
    let s = format!("{m:.places$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// A job's batch as its flags say it: ", 2 of 8 targets done", ", 8 targets", nothing for one.
fn batch(r: &Run) -> String {
    match r.done {
        Some(d) if r.targets > 1 => format!(", {d} of {} targets done", r.targets),
        _ if r.targets > 1 => format!(", {} targets", r.targets),
        _ => String::new(),
    }
}

/// A job's threads in words: "4 threads".
fn threads(r: &Run) -> String {
    format!("{} thread{}", r.threads, if r.threads == 1 { "" } else { "s" })
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
    for h in &st.members {
        if now.saturating_sub(h.beat) > touch {
            out.push(format!("warning: {} out of touch for {}", h.host, span(now.saturating_sub(h.beat))));
        }
    }
    let unheard: std::collections::BTreeSet<&str> = st
        .capacity
        .iter()
        .map(|(n, _)| crate::paces::mac_of(n))
        .filter(|m| *m != st.host && !st.members.iter().any(|h| h.host == *m))
        .collect();
    for m in unheard {
        out.push(format!("warning: {m}: its status not heard lately (asleep, away, or its agent down)"));
    }
    for s in std::iter::once(st).chain(st.members.iter()) {
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
        let f = idle(crate::agent::seam::for_tests(), &st, &open);
        assert_eq!(f.len(), 1, "one per Mac: {f:?}");
        assert_eq!(f[0].mac, "m4");
        assert!(f[0].said.contains("12 targets it could take wait (slope-deep 12)"), "{}", f[0].said);
        assert!(f[0].line().starts_with("idle: m4 has a slot free"), "{}", f[0].line());
        // (The lead's own job is no member's.)
        let own = [Open { step: "osm-pass".into(), targets: 1, own: true }];
        assert!(
            idle(crate::agent::seam::for_tests(), &st, &own).is_empty(),
            "nothing it could take, and nothing held back"
        );
        st.packing.insert("mini".into(), "cores: 0 of 8 left, a job of osm-pass takes 2".into());
        let f = idle(crate::agent::seam::for_tests(), &st, &own);
        assert_eq!((f.len(), f[0].mac.as_str()), (1, "mini"), "{f:?}");
        assert!(f[0].said.contains("cores: 0 of 8 left"), "{}", f[0].said);
    }

    /// Task #1367: a Mac offered no job says why from the plan's waits, with no room figure: the
    /// plan complete only with no wait at all; work behind other work or an input named with its
    /// wait; steps only the lead runs said as such on a member, the lead's own on the lead; a wait
    /// of no step (the records unreadable) said, never "complete"; work offered to other Macs first.
    #[test]
    fn a_mac_offered_nothing_says_the_plan_is_complete_or_what_waits() {
        let plan = crate::agent::seam::for_tests();
        let complete = "the plan is complete: no job is left to offer";
        assert_eq!(nothing_words(plan, &[], false, None), complete);
        assert_eq!(nothing_words(plan, &[], true, None), complete);
        // (A wait of no step: the plan can't be complete while it waits.)
        let records =
            Waiting { what: "Building".into(), why: "the build's records can't be read now".into(), step: None };
        let w = nothing_words(plan, std::slice::from_ref(&records), true, None);
        assert_eq!(w, "waiting: Building: the build's records can't be read now");
        let w = nothing_words(plan, std::slice::from_ref(&records), false, None);
        assert_eq!(w, "the lead waits: Building: the build's records can't be read now");
        let waits =
            [wait("slope-hi", "wait for terrain-root"), wait("terrain-root", "waits for the lead's NAS budget")];
        assert_eq!(
            nothing_words(plan, &waits, false, None),
            "the work left waits: slope-hi: wait for terrain-root (terrain-root: waits for the lead's NAS budget); work left only the lead runs: terrain-root"
        );
        let lead = nothing_words(plan, &waits, true, None);
        assert!(lead.starts_with("the work left waits: slope-hi: wait for terrain-root ("), "{lead}");
        assert!(
            lead.contains("; terrain-root: waits for the lead's NAS budget") && !lead.contains("only the lead"),
            "{lead}"
        );
        // (Many jobs of one step waiting alike: said once; past four, counted.)
        let many: Vec<Waiting> = ["a", "a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|s| Waiting { what: (*s).into(), why: "x".into(), step: None })
            .collect();
        assert_eq!(nothing_words(plan, &many, true, None), "waiting: a: x; b: x; c: x; d: x and 2 more");
        // (Work offered, none of it this Mac's: the coordinator's words first, the waits after.)
        let away = "2 steps offered are the lead's alone: rail, pack";
        assert_eq!(nothing_words(plan, &[], false, Some(away)), away);
        let w = nothing_words(plan, &waits[..1], false, Some(away));
        assert!(w.starts_with(&format!("{away}; the work left waits: slope-hi")), "{w}");
    }

    /// Nothing free, but work of a shared step held behind another step that waits too (4 Nov:
    /// slope-hi behind terrain-root): the flag names both waits, never "nothing offered" alone.
    #[test]
    fn an_idle_mac_names_the_wait_its_work_is_held_behind() {
        let mut st = lead();
        st.packing.insert("m1".into(), "nothing offered is free to take: 3 held by jobs".into());
        st.waiting = vec![
            wait("slope-hi", "wait for terrain-root: 3 z3 tiles not current"),
            wait("terrain-root", "waits for the lead's NAS budget"),
            Waiting { what: "Building the regions".into(), why: "unreadable".into(), step: None },
        ];
        let f = idle(crate::agent::seam::for_tests(), &st, &[]);
        assert_eq!(f.len(), 1, "{f:?}");
        let s = &f[0].said;
        assert!(s.contains("slope-hi: wait for terrain-root: 3 z3 tiles not current (terrain-root: waits for the lead's NAS budget)"), "{s}");
        assert!(!s.contains("nothing offered"), "{s}");
        // (terrain-root is the lead's alone: its own wait is no member's to take, but it is named
        // as what slope-hi waits behind.)
        assert!(!s.contains("; terrain-root:"), "{s}");
        // Nothing waits at all: the Mac has nothing it could do, no flag.
        st.waiting.clear();
        assert!(idle(crate::agent::seam::for_tests(), &st, &[]).is_empty());
        // A Mac paused alone isn't flagged; nor is any while the build is paused.
        st.waiting = vec![wait("slope-hi", "wait for terrain-root")];
        st.members.push(Status { host: "m1".into(), held: Some(jobrt::control::Hold::new("t")), ..Default::default() });
        assert!(idle(crate::agent::seam::for_tests(), &st, &[]).is_empty());
        st.members.clear();
        st.pause = Some(jobrt::control::Pause::new(jobrt::control::Mode::Drain, "t"));
        assert!(of(crate::agent::seam::for_tests(), &st, &[], &[], 3.0, 0).is_empty());
    }

    fn run(step: &str, secs: u64, usual: Option<u64>) -> Run {
        Run {
            worker: "m1 #2".into(),
            lease: 9,
            step: step.into(),
            targets: 8,
            done: Some(2),
            threads: 4,
            still: Some(Still {
                secs,
                usual: usual.map(|u| (u, Usual::Step)),
                target: "6/55/26".into(),
                phase: Some("sampling glo30".into()),
            }),
            pace: None,
        }
    }

    /// A job moving, at `rate` tiles a second over its window of 30 min, against its step's usual
    /// `usual`.
    fn paced(rate: f64, usual: f64) -> Run {
        let pace = Pace { unit: "tiles".into(), rate, usual, window_s: 1800, threads: 4 };
        Run { pace: Some(pace), ..run("terrain-hi", 60, Some(240)) }
    }

    /// Task #990: a job whose progress goes on at less than its step's usual rate (the slowest tenth
    /// of its jobs) divided by the factor is slow, said in its unit a minute with where it is;
    /// within it, or not judged (no pace), not; a job stuck is flagged stuck alone.
    #[test]
    fn a_job_moving_slower_than_its_steps_slowest_tenth_by_the_factor_is_slow() {
        let st = lead();
        // (A crawl: 0.05 tiles a second, its step's slowest tenth 1.1.)
        let f = of(crate::agent::seam::for_tests(), &st, &[], &[paced(0.05, 1.1)], 3.0, 0);
        assert_eq!(
            f.iter().map(Flag::line).collect::<Vec<_>>(),
            ["slow: terrain-hi (lease 9) on m1: 3 tiles a minute over the last 30 min (6/55/26, sampling glo30), under its step's usual 66 a minute (the slowest tenth of its jobs at 4 threads or fewer) ÷ 3, 2 of 8 targets done, 4 threads"]
        );
        assert!(of(crate::agent::seam::for_tests(), &st, &[], &[paced(0.4, 1.1)], 3.0, 0).is_empty(), "within 3×");
        assert!(
            of(crate::agent::seam::for_tests(), &st, &[], &[paced(0.4, 1.1)], 2.5, 0).len() == 1,
            "a smaller factor set"
        );
        assert!(
            of(crate::agent::seam::for_tests(), &st, &[], &[paced(0.0, 0.0)], 3.0, 0).is_empty(),
            "its step's slowest tenth said none either"
        );
        assert!(
            of(crate::agent::seam::for_tests(), &st, &[], &[run("terrain-hi", 60, Some(240))], 3.0, 0).is_empty(),
            "not judged"
        );
        // (Nothing moved for 20 min against a usual 4: stuck, not slow as well.)
        let still = Run { pace: paced(0.0, 1.1).pace, ..run("terrain-hi", 20 * 60, Some(240)) };
        assert_eq!(
            of(crate::agent::seam::for_tests(), &st, &[], &[still], 3.0, 0)
                .iter()
                .map(|f| f.kind.as_str())
                .collect::<Vec<_>>(),
            ["stuck"]
        );
        assert_eq!((per_min(0.0007), per_min(0.025), per_min(10.0)), ("0.042".into(), "1.5".into(), "600".into()));
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
        let start = Run {
            still: Some(Still { secs: 900, usual: Some((60, Usual::Target)), ..Default::default() }),
            ..run("pack", 0, None)
        };
        assert_eq!(
            at(&start, 3.0)[0].said,
            "pack (lease 9) on m1: nothing moved for 15 min (its start), past 3× its target's usual time (its step's stillness not measured yet) of 1 min, 2 of 8 targets done, 4 threads"
        );
        // A Mac paused alone isn't flagged.
        let mut st = lead();
        st.members.push(Status { host: "m1".into(), held: Some(jobrt::control::Hold::new("t")), ..Default::default() });
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
            still: Some(Still { secs: 0, usual: Some((120, Usual::Step)), ..Default::default() }),
            ..run("pyramids-fill", 0, None)
        };
        assert!(of(crate::agent::seam::for_tests(), &st, &[], &[fill], 3.0, 0).is_empty());
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
                why: "a map update now: what it would list changed".into(),
                due_since: Some(since),
                ..Default::default()
            })
        };
        st.publishing = due(now - 40 * 60);
        let f = stale(&st, &[], 3.0, now).expect("40 min due");
        assert_eq!(
            f.line(),
            "stale: a map update due for 40 min hasn't gone out (past 3× the 11 min the plan holds one at most; the last 6.0 h ago): a map update now: what it would list changed"
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
            pace: None,
        };
        assert!(stale(&st, &[job], 3.0, now).is_none(), "its job is under way");
        assert!(stale(&st, &[], 3.0, now).is_some());
    }

    /// Task #1392, the lead's runs of 13 Nov as its status keeps them (newest first): GC's run that
    /// removed 1,427 files at 21:37:45 (10 s), then runs of 0.1-0.3 s a second apart, each doing
    /// nothing. Flagged as a repeat while recent; a run that did something, one an hour after the
    /// last, or one of long ago, isn't.
    #[test]
    fn an_empty_duty_run_soon_after_the_last_is_a_repeat() {
        let now = 1_794_623_970;
        let run = |id: &str, ended: u64, secs: u64, empty: bool| super::super::Done {
            job: None,
            id: id.into(),
            what: id.into(),
            ok: true,
            ended,
            secs,
            note: String::new(),
            cancelled: false,
            empty,
        };
        let mut st = Status { host: "mini".into(), ..Default::default() };
        // (The first run, then 19 empty ones, a second apart: the status keeps 20.)
        st.recent = (0..19).map(|i| run("gc", now - 2 - i, 0, true)).collect();
        st.recent.push(run("gc", now - 21 - 10, 10, false));
        let f = repeat(&st, now);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].line(),
            "repeat: gc ran again and did nothing, ending 1 s after its run before, the last 1 min ago (19 of its last 20 runs did nothing): planned again with nothing for it to do"
        );
        // (In the lead's flags with the others.)
        let plan = crate::agent::seam::for_tests();
        assert!(of(plan, &st, &[], &[], 3.0, now).iter().any(|f| f.kind == "repeat"));
        // The first run alone, which removed what it found: no flag.
        st.recent = vec![run("gc", now - 31, 10, false)];
        assert!(repeat(&st, now).is_empty());
        // An empty run past GC's grace after the one before: no repeat.
        st.recent = vec![run("gc", now, 0, true), run("gc", now - crate::agent::gcqueue::GRACE_S - 10, 10, false)];
        assert!(repeat(&st, now).is_empty());
        // One within it, flagged while it is recent; not once it is older than that.
        st.recent = vec![run("gc", now - 100, 0, true), run("gc", now - 200, 10, false)];
        assert_eq!(repeat(&st, now).len(), 1);
        assert!(repeat(&st, now + crate::agent::gcqueue::GRACE_S).is_empty());
        // The backup run again the same day, doing nothing (10 Nov: 206 runs in 2 min): a repeat.
        st.recent =
            vec![run("backup", now, 0, true), run("backup", now - 10, 8, false), run("unit 6/1/1", now, 60, false)];
        assert!(repeat(&st, now).iter().any(|f| f.said.starts_with("backup ran again")));
        // The next day's backup with nothing new, planned a day after the last ended (`last_ok`, the
        // second its `ended` says) and so ending a day after it at the soonest: none.
        st.recent = vec![run("backup", now, 7, true), run("backup", now - super::super::backup::EVERY_S, 61, false)];
        assert!(repeat(&st, now).is_empty());
    }

    /// The health check's lines: the flags, the pool's warnings, the failures since the stamp alone.
    #[test]
    fn look_says_flags_warnings_and_failures_since() {
        let now = 100_000;
        let done = |id: &str, ok: bool, ended: u64| super::super::Done {
            job: None,
            id: id.into(),
            what: format!("{id}'s work"),
            ok,
            ended,
            secs: 60,
            note: "reading\nError: the NAS went away\n".into(),
            cancelled: false,
            empty: false,
        };
        let mut st = Status { host: "mini".into(), beat: now - 5, out_of_touch_s: 300, ..Default::default() };
        st.flags.push(Flag { kind: "idle".into(), mac: "m4".into(), said: "m4 has a slot free".into() });
        st.recent = vec![done("slope-deep 10/1/1", false, now - 10), done("pack 6/1/1", false, now - 9_000)];
        st.members.push(Status {
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
