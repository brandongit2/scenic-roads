//! Each Mac's agent controls its running jobs' threads live (task #1420; docs/pool.md §7.1, Threads
//! controlled live): a job's threads follow what it keeps busy now, not what its step's past runs
//! did.
//!
//! A job that takes its threads as they change (one of the steps reading its threads file,
//! jobrt::control::THREADS_ENV: as each target begins, jobrt::control::on_grant, or within the
//! second, inputs::inputs::pace) is watched on the threads it runs on (`Watch`): its CPU time (its
//! processes', the memory guard's sampler) and the wall time it waited on the NAS and the network
//! (not the local disk: a read its cache serves is computing, and a job's disk reads rise with its
//! threads; store::nasio::waited_in, which its processes add to as they go:
//! jobrt::timings). About once a minute (`EVERY`) it's decided again (`decide`), on the cores it
//! kept busy: its CPU time over its wall time, against its threads. (Not over the time it didn't
//! wait: a wait is an instant any of its threads was in a call, while the others may compute, so
//! that would count their CPU over less than the time they ran: 4 threads, 40 % of the minute
//! waiting, 2.4 cores busy, would read as all 4 busy.)
//!
//! - **Every thread busy** (`BUSY` of its threads or more) and its waits a small share of its
//!   minute (under `WAITING`): doubled, within the cores its Mac has free (its cores for jobs less
//!   those busy over the last minute, less what the jobs grown before it this round took), and the
//!   memory each thread past those its grant's memory covers adds (crate::coord::Grant::thread_mb)
//!   within what its Mac has for jobs less its jobs' growth so far; none while its Mac's memory
//!   admits no work (the brake, the floor, a give-back's cool-down), and none past those its grant
//!   covers while it has no memory figure (a cold start's: 0 MB a thread). A job waiting more is
//!   never grown: more threads would wait on the NAS with it (its pacing is the NAS budget's).
//! - **Threads left idle** (under `IDLE` of them busy): cut to the threads that keep it busy at
//!   `BUSY`, and one more to overlap its waits, however much it waits: a thread waiting on the NAS
//!   holds a core it doesn't use and adds to the NAS's load.
//! - Past the most its Mac lets a job run on now (its cores less the in-use share's while the owner
//!   uses it): cut to that, whatever it keeps busy.
//!
//! The gap between `BUSY` and `IDLE` is the hysteresis: a job cut keeps a thread past what it uses,
//! so it isn't grown again at once, and a grow that didn't pay is cut back the next minute to what
//! the job kept busy. A job decided on is watched afresh from then, and from when it runs on the
//! threads its file says (a step taking them as a target begins waits for its next target).
//!
//! - **The next thread pays while nothing waits** (task #1472): a job busy short of `BUSY` (a wave's
//!   tail, a dry queue: 0.81 a thread is never doubled) is grown one thread at a time while nothing
//!   waits for its Mac's cores (its Mac's last ask was answered with no work offered it,
//!   `Room::nothing_waits`), it waits under `WAITING`, its Mac has a core free and the memory, and
//!   the next thread pays: its step's jobs here kept more cores busy on one thread more than on
//!   these, by more than that difference's own spread (`Yields::next`: the standard error of the
//!   two means, from the windows this agent watched; no fixed share). A step not yet watched on
//!   one thread more is tried on it once these threads are measured; a thread so grown that didn't
//!   pay is cut back the next minute, and every one so grown is given back once work waits for
//!   the cores again.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// How long a job is watched on its threads before it's decided again: a minute, the span the
/// Mac's busy cores are measured over.
pub const EVERY: Duration = Duration::from_secs(60);

/// The share of its threads a job keeps busy while it computes from which it counts as keeping
/// every one busy, and is grown.
pub const BUSY: f64 = 0.9;

/// The share of its threads under which a job counts as leaving threads idle, and is cut.
pub const IDLE: f64 = 0.5;

/// The share of its minute a job waited from which it's never grown.
pub const WAITING: f64 = 0.25;

/// A job watched on the threads it runs on (`Agent::watch_threads`): since when, its CPU time, wall
/// time waited and time stopped then (ns: a freeze or pause, left out of the wall time it ran,
/// `Agent::watched_for`), and those threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    pub since: Instant,
    pub cpu_ns: u64,
    pub waited_ns: u64,
    pub stopped_ns: u64,
    pub threads: u32,
}

/// What a job did over its minute (`Watch`'s span): on `threads`, its wall time, its CPU time and
/// the wall time it waited (ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub threads: u32,
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub waited_ms: u64,
}

impl Window {
    /// Its share of the minute waited (0–1).
    pub fn waited(&self) -> f64 {
        (self.waited_ms as f64 / self.wall_ms.max(1) as f64).min(1.0)
    }

    /// The cores it kept busy: its CPU time over its wall time.
    pub fn busy(&self) -> f64 {
        self.cpu_ms as f64 / self.wall_ms.max(1) as f64
    }
}

/// What a job may grow into on its Mac: the most threads it may run on now (its Mac's cores less
/// the in-use share's while the owner uses it), the cores free (its cores for jobs less those busy
/// over the last minute, less the threads grown already this round), its memory: whether its
/// Mac admits work at all, the memory (MB) its Mac has for jobs less its jobs' growth so far, what
/// each thread past `covers` adds to the job's (MB; 0: no figure yet), and the threads its grant's
/// memory covers; and whether nothing waits for its Mac's cores (its last ask, within the minute,
/// answered with no work offered it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    pub most: u32,
    pub free_cores: f64,
    pub mem_ok: bool,
    pub mem_mb: u64,
    pub thread_mb: u64,
    pub covers: u32,
    pub nothing_waits: bool,
}

impl Room {
    /// The most threads its memory lets a job on `t` run on.
    fn by_mem(&self, t: u32) -> u32 {
        if !self.mem_ok {
            t
        } else if self.thread_mb == 0 {
            t.max(self.covers)
        } else {
            let more = self.mem_mb / self.thread_mb;
            t.max(self.covers).saturating_add(u32::try_from(more).unwrap_or(u32::MAX))
        }
    }
}

/// What one thread more (or the job's last) yields, by its step's windows here (`Yields`): not
/// measured enough to say, measured to pay or not, or (`next` alone) to be tried.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Yield {
    Unknown,
    Try,
    Pays(f64, f64),
    DoesNot(f64, f64),
}

/// The busy cores each step's jobs kept here on each count of threads, the last `KEPT` minutes'
/// windows of each, under `WAITING` waited (a waiting window measures the NAS, not the threads):
/// this agent's own record of what a thread more yields (`next`).
#[derive(Clone, Debug, Default)]
pub struct Yields {
    by: BTreeMap<String, BTreeMap<u32, VecDeque<f64>>>,
}

/// The windows `Yields` keeps of a step on a count of threads: the latest, so a step whose work
/// changed reads as it is now.
pub const KEPT: usize = 8;

impl Yields {
    /// Window `w` of a job of `step`'s, kept.
    pub fn saw(&mut self, step: &str, w: &Window) {
        if w.waited() >= WAITING || w.threads == 0 {
            return;
        }
        let v = self.by.entry(step.to_string()).or_default().entry(w.threads).or_default();
        if v.len() == KEPT {
            v.pop_front();
        }
        v.push_back(w.busy());
    }

    /// What `step`'s jobs kept busy on `t` threads: the mean of its windows, their variance's share
    /// of it (the variance over their count) and how many; None under two (no spread yet).
    fn on(&self, step: &str, t: u32) -> Option<(f64, f64)> {
        let v = self.by.get(step)?.get(&t).filter(|v| v.len() >= 2)?;
        let n = v.len() as f64;
        let mean = v.iter().sum::<f64>() / n;
        let var = v.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / (n - 1.0);
        Some((mean, var / n))
    }

    /// What the thread past `t` yields `step`'s jobs: the cores busy on `t + 1` less those on `t`,
    /// paying when past its standard error (the two means' spread together); to be tried when `t`
    /// is measured and `t + 1` not yet; unknown while `t` isn't.
    pub fn next(&self, step: &str, t: u32) -> Yield {
        let Some((a, va)) = self.on(step, t) else { return Yield::Unknown };
        let Some((b, vb)) = self.on(step, t + 1) else { return Yield::Try };
        let (y, se) = (b - a, (va + vb).sqrt());
        if y > se {
            Yield::Pays(y, se)
        } else {
            Yield::DoesNot(y, se)
        }
    }
}

/// The threads a job runs on next, how many of them the next thread's rule grew it by
/// (`grown_idle`), and why, in words for the agent's log.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub threads: u32,
    pub grown_idle: u32,
    pub why: String,
}

/// The job that did `w` decided again in `room` (the module's rule): `grown_idle` of its threads
/// grown by the next thread's rule, `next` what one more yields and `last` what its last did
/// (`Yields::next` of its threads, and of one fewer).
pub fn decide(w: &Window, room: &Room, grown_idle: u32, next: Yield, last: Yield) -> Decision {
    let t = w.threads.max(1);
    let most = room.most.max(1);
    let (busy, waited) = (w.busy(), w.waited());
    let said = format!("{busy:.1} cores busy of {t}, {:.0} % of its minute waited", waited * 100.0);
    let idle = grown_idle.min(t - 1);
    let to = |threads: u32, idle: u32, why: String| Decision { threads, grown_idle: idle, why };
    if t > most {
        return to(most, idle.saturating_sub(t - most), format!("{most} threads at most on its Mac now ({said})"));
    }
    if busy >= BUSY * f64::from(t) {
        if waited >= WAITING {
            return to(t, idle, format!("every thread busy, not grown while it waits ({said})"));
        }
        let by_cores = t + room.free_cores.max(0.0).floor() as u32;
        let by_mem = room.by_mem(t);
        let grown = (2 * t).min(most).min(by_cores).min(by_mem);
        if grown > t {
            return to(grown, idle, format!("every thread busy ({said})"));
        }
        let short = if by_mem <= t {
            "its Mac's memory"
        } else if by_cores <= t {
            "its Mac's free cores"
        } else {
            "its Mac's cores"
        };
        return to(t, idle, format!("every thread busy, held by {short} ({said})"));
    }
    if busy < IDLE * f64::from(t) {
        let cut = (busy / BUSY).ceil() as u32 + 1;
        if cut < t {
            let cut = cut.max(1);
            return to(cut, idle.saturating_sub(t - cut), format!("threads left idle ({said})"));
        }
    }
    if idle > 0 && !room.nothing_waits {
        return to(
            t - idle,
            0,
            format!(
                "its {idle} threads grown while nothing waited given back: work may wait for its Mac's cores ({said})"
            ),
        );
    }
    if let (true, Yield::DoesNot(y, se)) = (idle > 0, last) {
        return to(t - 1, idle - 1, format!("its last thread didn't pay: {y:+.2} cores busy, ±{se:.2} ({said})"));
    }
    let free = t < most && room.free_cores >= 1.0 && room.by_mem(t) > t;
    if room.nothing_waits && waited < WAITING && free {
        match next {
            Yield::Pays(y, se) => {
                return to(t + 1, idle + 1, format!("the next thread pays, {y:+.2} cores busy ±{se:.2}, and nothing waits for its Mac's cores ({said})"));
            }
            Yield::Try => {
                return to(t + 1, idle + 1, format!("a thread more tried: nothing waits for its Mac's cores, and its step isn't measured on {} here ({said})", t + 1));
            }
            Yield::Unknown | Yield::DoesNot(..) => {}
        }
    }
    to(t, idle, said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(threads: u32, cpu_s: u64, waited_s: u64) -> Window {
        Window { threads, wall_ms: 60_000, cpu_ms: cpu_s * 1000, waited_ms: waited_s * 1000 }
    }

    /// A Mac with room: 12 cores, 8 free, memory for many threads; work waiting for its cores.
    fn roomy() -> Room {
        Room {
            most: 12,
            free_cores: 8.0,
            mem_ok: true,
            mem_mb: 64_000,
            thread_mb: 500,
            covers: 2,
            nothing_waits: false,
        }
    }

    /// `decide` of a job no thread of which the next thread's rule grew, its yields unknown.
    fn dec(w: &Window, room: &Room) -> Decision {
        decide(w, room, 0, Yield::Unknown, Yield::Unknown)
    }

    /// Task #1472: `step`'s windows as `Yields` keeps them, on each (threads, [busy cores]).
    fn yields(step: &str, seen: &[(u32, &[f64])]) -> Yields {
        let mut y = Yields::default();
        for &(t, busy) in seen {
            for b in busy {
                y.saw(step, &Window { threads: t, wall_ms: 60_000, cpu_ms: (b * 60_000.0) as u64, waited_ms: 0 });
            }
        }
        y
    }

    /// Task #1472: a job at 0.81 busy a thread, never doubled, grows one thread at a time while
    /// nothing waits for its Mac's cores and the next thread pays past its own spread: tried once on
    /// one thread more when its step has no windows there, kept while it pays, cut back once it
    /// doesn't, and every thread so grown given back once work waits again. Held while work
    /// waits, while it waits on the NAS, with no core or memory free, or at the most.
    #[test]
    fn a_job_grows_a_thread_at_a_time_while_the_next_pays_and_nothing_waits() {
        let dry = Room { nothing_waits: true, ..roomy() };
        let on4 = w(4, (3.24 * 60.0) as u64, 0);
        assert_eq!(dec(&on4, &dry).threads, 4, "0.81 a thread: never doubled, and its yields unknown");
        let y = yields("terrain-deep", &[(4, &[3.2, 3.3, 3.25])]);
        let next = y.next("terrain-deep", 4);
        assert_eq!(next, Yield::Try);
        let d = decide(&on4, &dry, 0, next, y.next("terrain-deep", 3));
        assert_eq!((d.threads, d.grown_idle), (5, 1), "{}", d.why);
        assert!(d.why.contains("tried"), "{}", d.why);
        assert_eq!(decide(&on4, &roomy(), 0, next, Yield::Unknown).threads, 4, "work waits: held");
        // On 5: 0.6 cores more than on 4, past its spread: it pays, and the next is tried.
        let y = yields("terrain-deep", &[(4, &[3.2, 3.3, 3.25]), (5, &[3.8, 3.9, 3.85])]);
        let on5 = w(5, (3.85 * 60.0) as u64, 0);
        let (next, last) = (y.next("terrain-deep", 5), y.next("terrain-deep", 4));
        assert!(matches!(last, Yield::Pays(y, _) if (y - 0.6).abs() < 1e-9), "{last:?}");
        let d = decide(&on5, &dry, 1, next, last);
        assert_eq!((d.threads, d.grown_idle), (6, 2), "{}", d.why);
        // On 6 no more than on 5, within the noise: the sixth cut back.
        let y = yields("terrain-deep", &[(5, &[3.8, 3.9, 3.85]), (6, &[3.9, 3.8, 3.86])]);
        let on6 = w(6, (3.86 * 60.0) as u64, 0);
        let last = y.next("terrain-deep", 5);
        assert!(matches!(last, Yield::DoesNot(..)), "{last:?}");
        let d = decide(&on6, &dry, 2, y.next("terrain-deep", 6), last);
        assert_eq!((d.threads, d.grown_idle), (5, 1), "{}", d.why);
        assert!(d.why.contains("didn't pay"), "{}", d.why);
        // Back on 5, its next known not to pay: held there.
        let d = decide(&on5, &dry, 1, y.next("terrain-deep", 5), Yield::Pays(0.6, 0.05));
        assert_eq!((d.threads, d.grown_idle), (5, 1), "{}", d.why);
        // Work waits again: the thread so grown given back.
        let d = decide(&on5, &roomy(), 1, Yield::Pays(0.6, 0.05), Yield::Pays(0.6, 0.05));
        assert_eq!((d.threads, d.grown_idle), (4, 0), "{}", d.why);
        // Held: waiting on the NAS, no core free, no memory, at the most.
        let pays = Yield::Pays(0.6, 0.05);
        assert_eq!(decide(&w(4, 194, 20), &dry, 0, pays, Yield::Unknown).threads, 4, "a third of its minute waited");
        assert_eq!(decide(&on4, &Room { free_cores: 0.6, ..dry }, 0, pays, Yield::Unknown).threads, 4, "no core free");
        assert_eq!(
            decide(&on4, &Room { mem_mb: 100, covers: 4, ..dry }, 0, pays, Yield::Unknown).threads,
            4,
            "no memory"
        );
        assert_eq!(decide(&on4, &Room { most: 4, ..dry }, 0, pays, Yield::Unknown).threads, 4, "at the most");
    }

    /// Task #1472: what `Yields` keeps and says: the last `KEPT` windows a count of threads, none
    /// waiting `WAITING` of its minute; a count with under two windows unknown (no spread), the next
    /// to be tried once this one is known; a yield past the means' standard error pays.
    #[test]
    fn yields_are_the_windows_kept_their_difference_and_its_spread() {
        let mut y = yields("s", &[(2, &[1.0])]);
        assert_eq!(y.next("s", 2), Yield::Unknown, "one window: no spread");
        y.saw("s", &Window { threads: 2, wall_ms: 60_000, cpu_ms: 200_000, waited_ms: 30_000 });
        assert_eq!(y.next("s", 2), Yield::Unknown, "a window half waited isn't kept");
        let y = yields("s", &[(2, &[1.0, 1.2]), (3, &[1.9, 2.1])]);
        // (0.9 more, each mean's variance 0.02 over 2: a standard error of 0.14.)
        assert!(
            matches!(y.next("s", 2), Yield::Pays(d, se) if (d - 0.9).abs() < 1e-9 && (se - 0.02f64.sqrt()).abs() < 1e-9)
        );
        assert_eq!(y.next("s", 3), Yield::Try);
        assert_eq!(y.next("other", 2), Yield::Unknown);
        let y = yields("s", &[(2, &[1.0, 1.4]), (3, &[1.1, 1.5])]);
        assert!(matches!(y.next("s", 2), Yield::DoesNot(..)), "0.1 more within a spread of 0.28");
        // (The latest `KEPT` alone: a step whose work changed reads as it is now.)
        let old: Vec<f64> = (0..KEPT).map(|_| 9.0).chain((0..KEPT).map(|_| 1.0)).collect();
        let y = yields("s", &[(2, &old), (3, &[1.0, 1.0])]);
        assert!(matches!(y.next("s", 2), Yield::DoesNot(d, _) if d.abs() < 1e-9));
    }

    /// A job keeping every thread busy is doubled; one computing on half or more of them is held;
    /// one leaving most idle is cut to what it keeps busy and a thread more.
    #[test]
    fn a_job_grows_while_its_threads_are_busy_and_shrinks_while_they_are_idle() {
        let d = dec(&w(2, 115, 0), &roomy());
        assert_eq!(d.threads, 4, "{}", d.why);
        assert!(d.why.starts_with("every thread busy"), "{}", d.why);
        assert_eq!(dec(&w(4, 3 * 60, 0), &roomy()).threads, 4, "3 of 4 busy: held");
        let d = dec(&w(8, 90, 0), &roomy());
        assert_eq!(d.threads, 3, "1.5 cores busy of 8: cut to 2 and one more: {}", d.why);
        assert!(d.why.starts_with("threads left idle"), "{}", d.why);
        // (A serial job keeps a thread to overlap its waits: never under 2 once it's cut.)
        assert_eq!(dec(&w(6, 20, 0), &roomy()).threads, 2);
        assert_eq!(dec(&w(2, 20, 0), &roomy()).threads, 2, "never cut to as many");
        // (A grow that didn't pay is cut back to what it kept busy: 4 threads, 4 busy, grown to 8;
        // still 4 busy at 8: back to 6 (4.4 busy at 0.9, and one), where it holds.)
        assert_eq!(dec(&w(4, 236, 0), &roomy()).threads, 8);
        let back = dec(&w(8, 236, 0), &roomy()).threads;
        assert_eq!(back, 6);
        assert_eq!(dec(&w(back, 236, 0), &roomy()).threads, back, "settled");
    }

    /// The busy cores are its CPU time over its wall time, waits and all: 4 threads, 40 % of the
    /// minute waiting, 2.4 cores busy, are held (not read as 4 busy over its 36 s without a wait,
    /// and doubled). A job waiting `WAITING` of its minute or more is never grown, its threads all
    /// busy or not; one whose threads sit idle is cut however much it waits (slope-deep's batches
    /// waiting on the NAS hold no idle threads).
    #[test]
    fn a_job_waiting_is_judged_on_its_cpu_over_its_minute_and_never_grown() {
        let d = dec(&w(4, 144, 24), &roomy());
        assert_eq!(d.threads, 4, "2.4 of 4 busy: {}", d.why);
        assert_eq!(dec(&w(3, 170, 10), &roomy()).threads, 6, "2.8 of 3 busy, a sixth waited: grown");
        let d = dec(&w(4, 4 * 58, 20), &roomy());
        assert_eq!(d.threads, 4, "a third waited: {}", d.why);
        assert!(d.why.contains("not grown while it waits"), "{}", d.why);
        let d = dec(&w(8, 5, 59), &roomy());
        assert_eq!(d.threads, 2, "nearly idle, waiting all its minute: cut to 2: {}", d.why);
        assert!(d.why.starts_with("threads left idle"), "{}", d.why);
        assert_eq!(dec(&w(3, 81, 40), &roomy()).threads, 3, "1.35 of 3 busy: held");
        // (Waits past its wall time, as a job's programs' summed may be: a share of 1, not more.)
        assert_eq!(dec(&Window { waited_ms: 90_000, ..w(4, 4 * 59, 0) }, &roomy()).threads, 4);
    }

    /// A grow stays within the cores its Mac has free, the memory each thread past those its grant
    /// covers adds, and the most its Mac lets a job have; none while its Mac's memory admits no
    /// work. Past the most (the owner back), it's cut to that.
    #[test]
    fn a_grow_stays_within_the_free_cores_the_memory_and_the_most() {
        let busy4 = w(4, 4 * 59, 0);
        assert_eq!(dec(&busy4, &Room { free_cores: 2.7, ..roomy() }).threads, 6, "two cores free");
        let d = dec(&busy4, &Room { free_cores: 0.4, ..roomy() });
        assert_eq!(d.threads, 4);
        assert!(d.why.contains("free cores"), "{}", d.why);
        // (Memory: its grant covers 4 threads; 1.2 GB left, 500 MB a thread past them: 2 more.)
        assert_eq!(dec(&busy4, &Room { mem_mb: 1200, covers: 4, ..roomy() }).threads, 6);
        // (Covering 6 already, its grow to 8 needs one thread's 500 MB past them: 400 MB left, 6.)
        assert_eq!(dec(&busy4, &Room { mem_mb: 400, covers: 6, ..roomy() }).threads, 6);
        let d = dec(&busy4, &Room { mem_mb: 400, ..roomy() });
        assert_eq!(d.threads, 4);
        assert!(d.why.contains("memory"), "{}", d.why);
        assert_eq!(dec(&busy4, &Room { mem_ok: false, covers: 8, ..roomy() }).threads, 4, "the brake on");
        // (No memory figure yet, a cold start's: grown within the threads its grant covers alone.)
        let d = dec(&busy4, &Room { thread_mb: 0, covers: 2, ..roomy() });
        assert_eq!(d.threads, 4, "{}", d.why);
        assert!(d.why.contains("memory"), "{}", d.why);
        assert_eq!(dec(&busy4, &Room { thread_mb: 0, covers: 6, ..roomy() }).threads, 6);
        assert_eq!(dec(&busy4, &Room { most: 5, ..roomy() }).threads, 5);
        let d = dec(&w(8, 8 * 59, 0), &Room { most: 6, ..roomy() });
        assert_eq!(d.threads, 6, "the owner back: {}", d.why);
        assert_eq!(dec(&w(8, 8 * 59, 59), &Room { most: 6, ..roomy() }).threads, 6, "waiting or not");
    }
}
