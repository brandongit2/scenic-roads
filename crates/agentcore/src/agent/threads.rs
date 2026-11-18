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

/// A job watched on the threads it runs on (`Agent::watch_threads`): since when, its CPU time and
/// wall time waited then (ns), and those threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    pub since: Instant,
    pub cpu_ns: u64,
    pub waited_ns: u64,
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
/// over the last minute, less the threads grown already this round), and its memory: whether its
/// Mac admits work at all, the memory (MB) its Mac has for jobs less its jobs' growth so far, what
/// each thread past `covers` adds to the job's (MB; 0: no figure yet), and the threads its grant's
/// memory covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    pub most: u32,
    pub free_cores: f64,
    pub mem_ok: bool,
    pub mem_mb: u64,
    pub thread_mb: u64,
    pub covers: u32,
}

/// The threads a job runs on next, and why, in words for the agent's log.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub threads: u32,
    pub why: String,
}

/// The job that did `w` decided again in `room` (the module's rule).
pub fn decide(w: &Window, room: &Room) -> Decision {
    let t = w.threads.max(1);
    let most = room.most.max(1);
    let (busy, waited) = (w.busy(), w.waited());
    let said = format!("{busy:.1} cores busy of {t}, {:.0} % of its minute waited", waited * 100.0);
    let hold = |why: String| Decision { threads: t, why };
    if t > most {
        return Decision { threads: most, why: format!("{most} threads at most on its Mac now ({said})") };
    }
    if busy >= BUSY * f64::from(t) {
        if waited >= WAITING {
            return hold(format!("every thread busy, not grown while it waits ({said})"));
        }
        let by_cores = t + room.free_cores.max(0.0).floor() as u32;
        let by_mem = if !room.mem_ok {
            t
        } else if room.thread_mb == 0 {
            t.max(room.covers)
        } else {
            let paid = t.max(room.covers);
            let more = room.mem_mb / room.thread_mb;
            paid.saturating_add(u32::try_from(more).unwrap_or(u32::MAX))
        };
        let grown = (2 * t).min(most).min(by_cores).min(by_mem);
        if grown > t {
            return Decision { threads: grown, why: format!("every thread busy ({said})") };
        }
        let short = if by_mem <= t {
            "its Mac's memory"
        } else if by_cores <= t {
            "its Mac's free cores"
        } else {
            "its Mac's cores"
        };
        return hold(format!("every thread busy, held by {short} ({said})"));
    }
    if busy < IDLE * f64::from(t) {
        let cut = (busy / BUSY).ceil() as u32 + 1;
        if cut < t {
            return Decision { threads: cut.max(1), why: format!("threads left idle ({said})") };
        }
    }
    hold(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(threads: u32, cpu_s: u64, waited_s: u64) -> Window {
        Window { threads, wall_ms: 60_000, cpu_ms: cpu_s * 1000, waited_ms: waited_s * 1000 }
    }

    /// A Mac with room: 12 cores, 8 free, memory for many threads.
    fn roomy() -> Room {
        Room { most: 12, free_cores: 8.0, mem_ok: true, mem_mb: 64_000, thread_mb: 500, covers: 2 }
    }

    /// A job keeping every thread busy is doubled; one computing on half or more of them is held;
    /// one leaving most idle is cut to what it keeps busy and a thread more.
    #[test]
    fn a_job_grows_while_its_threads_are_busy_and_shrinks_while_they_are_idle() {
        let d = decide(&w(2, 115, 0), &roomy());
        assert_eq!(d.threads, 4, "{}", d.why);
        assert!(d.why.starts_with("every thread busy"), "{}", d.why);
        assert_eq!(decide(&w(4, 3 * 60, 0), &roomy()).threads, 4, "3 of 4 busy: held");
        let d = decide(&w(8, 90, 0), &roomy());
        assert_eq!(d.threads, 3, "1.5 cores busy of 8: cut to 2 and one more: {}", d.why);
        assert!(d.why.starts_with("threads left idle"), "{}", d.why);
        // (A serial job keeps a thread to overlap its waits: never under 2 once it's cut.)
        assert_eq!(decide(&w(6, 20, 0), &roomy()).threads, 2);
        assert_eq!(decide(&w(2, 20, 0), &roomy()).threads, 2, "never cut to as many");
        // (A grow that didn't pay is cut back to what it kept busy: 4 threads, 4 busy, grown to 8;
        // still 4 busy at 8: back to 6 (4.4 busy at 0.9, and one), where it holds.)
        assert_eq!(decide(&w(4, 236, 0), &roomy()).threads, 8);
        let back = decide(&w(8, 236, 0), &roomy()).threads;
        assert_eq!(back, 6);
        assert_eq!(decide(&w(back, 236, 0), &roomy()).threads, back, "settled");
    }

    /// The busy cores are its CPU time over its wall time, waits and all: 4 threads, 40 % of the
    /// minute waiting, 2.4 cores busy, are held (not read as 4 busy over its 36 s without a wait,
    /// and doubled). A job waiting `WAITING` of its minute or more is never grown, its threads all
    /// busy or not; one whose threads sit idle is cut however much it waits (slope-deep's batches
    /// waiting on the NAS hold no idle threads).
    #[test]
    fn a_job_waiting_is_judged_on_its_cpu_over_its_minute_and_never_grown() {
        let d = decide(&w(4, 144, 24), &roomy());
        assert_eq!(d.threads, 4, "2.4 of 4 busy: {}", d.why);
        assert_eq!(decide(&w(3, 170, 10), &roomy()).threads, 6, "2.8 of 3 busy, a sixth waited: grown");
        let d = decide(&w(4, 4 * 58, 20), &roomy());
        assert_eq!(d.threads, 4, "a third waited: {}", d.why);
        assert!(d.why.contains("not grown while it waits"), "{}", d.why);
        let d = decide(&w(8, 5, 59), &roomy());
        assert_eq!(d.threads, 2, "nearly idle, waiting all its minute: cut to 2: {}", d.why);
        assert!(d.why.starts_with("threads left idle"), "{}", d.why);
        assert_eq!(decide(&w(3, 81, 40), &roomy()).threads, 3, "1.35 of 3 busy: held");
        // (Waits past its wall time, as a job's programs' summed may be: a share of 1, not more.)
        assert_eq!(decide(&Window { waited_ms: 90_000, ..w(4, 4 * 59, 0) }, &roomy()).threads, 4);
    }

    /// A grow stays within the cores its Mac has free, the memory each thread past those its grant
    /// covers adds, and the most its Mac lets a job have; none while its Mac's memory admits no
    /// work. Past the most (the owner back), it's cut to that.
    #[test]
    fn a_grow_stays_within_the_free_cores_the_memory_and_the_most() {
        let busy4 = w(4, 4 * 59, 0);
        assert_eq!(decide(&busy4, &Room { free_cores: 2.7, ..roomy() }).threads, 6, "two cores free");
        let d = decide(&busy4, &Room { free_cores: 0.4, ..roomy() });
        assert_eq!(d.threads, 4);
        assert!(d.why.contains("free cores"), "{}", d.why);
        // (Memory: its grant covers 4 threads; 1.2 GB left, 500 MB a thread past them: 2 more.)
        assert_eq!(decide(&busy4, &Room { mem_mb: 1200, covers: 4, ..roomy() }).threads, 6);
        // (Covering 6 already, its grow to 8 needs one thread's 500 MB past them: 400 MB left, 6.)
        assert_eq!(decide(&busy4, &Room { mem_mb: 400, covers: 6, ..roomy() }).threads, 6);
        let d = decide(&busy4, &Room { mem_mb: 400, ..roomy() });
        assert_eq!(d.threads, 4);
        assert!(d.why.contains("memory"), "{}", d.why);
        assert_eq!(decide(&busy4, &Room { mem_ok: false, covers: 8, ..roomy() }).threads, 4, "the brake on");
        // (No memory figure yet, a cold start's: grown within the threads its grant covers alone.)
        let d = decide(&busy4, &Room { thread_mb: 0, covers: 2, ..roomy() });
        assert_eq!(d.threads, 4, "{}", d.why);
        assert!(d.why.contains("memory"), "{}", d.why);
        assert_eq!(decide(&busy4, &Room { thread_mb: 0, covers: 6, ..roomy() }).threads, 6);
        assert_eq!(decide(&busy4, &Room { most: 5, ..roomy() }).threads, 5);
        let d = decide(&w(8, 8 * 59, 0), &Room { most: 6, ..roomy() });
        assert_eq!(d.threads, 6, "the owner back: {}", d.why);
        assert_eq!(decide(&w(8, 8 * 59, 59), &Room { most: 6, ..roomy() }).threads, 6, "waiting or not");
    }
}
