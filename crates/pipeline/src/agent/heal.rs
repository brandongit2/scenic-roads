//! The agent's self-heal for stale SMB handles (docs/pool.md §3, docs/macs.md): when NAS calls on
//! this Mac (the agent's own, and its Rust jobs' through `crate::whole::STUCK_ENV`) keep staying
//! "Resource busy" past `crate::whole`'s retries, the agent remounts its share, which ends this
//! Mac's SMB session and with it every handle the session held (a process here killed with a file
//! open). It can't free another Mac's session: that one ends when the server drops it.
//!
//! A forced unmount fails every file open on the share, so the agent drains first: no new job is
//! taken or granted on this Mac (heartbeats and leases go on), and the remount waits for the slots
//! and the agent's own NAS threads to end, `DRAIN_MAX` at most; past it the drain is given up
//! without cutting a job.

use crate::whole::Stuck;

/// Calls stuck within this long count (seconds).
const WINDOW: u64 = 10 * 60;
/// This many stuck calls within `WINDOW` ask for a remount.
const STUCK: usize = 3;
/// At most one drain this often, at first; doubled each time the busy calls come back after a
/// remount, up to `EVERY_MAX`; back to `EVERY` after a quiet `WINDOW`.
const EVERY: u64 = 30 * 60;
const EVERY_MAX: u64 = 4 * 3600;
/// The longest a drain waits for the jobs to end.
const DRAIN_MAX: u64 = 30 * 60;

/// What the loop does now.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// Remount the share (the drain is done), then `Heal::remounted`.
    Remount,
}

#[derive(Debug, Default)]
pub struct Heal {
    /// Stuck calls within `WINDOW`, oldest first.
    stuck: Vec<Stuck>,
    /// Since when (Unix s) new work waits for a remount.
    draining: Option<u64>,
    /// The last drain's end (Unix s), and whether it ended in a remount.
    last: Option<(u64, bool)>,
    /// The interval between drains (0: `EVERY`).
    every: u64,
    /// Calls stuck again after a remount: the file.
    warn: Option<String>,
}

impl Heal {
    fn every(&self) -> u64 {
        self.every.max(EVERY)
    }

    /// Takes in `new` stuck calls and says what to do at `now`: a drain begun when `STUCK` calls
    /// since the last drain fall within `WINDOW` and the interval has passed since it; a remount once
    /// `idle` (no job, none of the agent's own NAS threads) while draining; the drain given up after
    /// `DRAIN_MAX`.
    pub fn tick(&mut self, new: Vec<Stuck>, now: u64, idle: bool) -> Act {
        self.stuck.extend(new);
        self.stuck.retain(|s| s.at + WINDOW >= now);
        let after = |s: &&Stuck| self.last.is_none_or(|(l, _)| s.at > l);
        let since: Vec<&Stuck> = self.stuck.iter().filter(after).collect();
        if let Some((l, remounted)) = self.last {
            if since.is_empty() && now >= l + WINDOW {
                // (Quiet since: healed.)
                (self.every, self.warn) = (EVERY, None);
            } else if remounted {
                if let Some(s) = since.last() {
                    self.warn = Some(s.path.clone());
                }
            }
        }
        match self.draining {
            None => {
                if since.len() >= STUCK && self.last.is_none_or(|(l, _)| now >= l + self.every()) {
                    let path = since.last().map(|s| s.path.clone()).unwrap_or_default();
                    eprintln!(
                        "agent: {} NAS calls stuck busy within {} min (the last on {path}): draining this Mac's jobs for a remount",
                        since.len(),
                        WINDOW / 60
                    );
                    if self.last.is_some_and(|(_, r)| r) {
                        // (The last remount didn't stop them: another Mac's session, most likely.)
                        self.every = (self.every() * 2).min(EVERY_MAX);
                    }
                    self.draining = Some(now);
                }
                Act::Nothing
            }
            Some(_) if idle => Act::Remount,
            Some(d) if now >= d + DRAIN_MAX => {
                eprintln!(
                    "agent: the jobs didn't end within {} min: the NAS remount is given up for now",
                    DRAIN_MAX / 60
                );
                self.draining = None;
                self.last = Some((now, false));
                Act::Nothing
            }
            Some(_) => Act::Nothing,
        }
    }

    /// The remount was done at `now`.
    pub fn remounted(&mut self, now: u64) {
        self.draining = None;
        self.last = Some((now, true));
    }

    /// Why new work waits, while it drains.
    pub fn drain_why(&self) -> Option<String> {
        self.draining.map(|_| {
            let on = self.stuck.last().map(|s| format!(" (calls stuck busy on {})", s.path)).unwrap_or_default();
            format!("draining for a NAS remount{on}: no new job starts until this Mac's jobs end")
        })
    }

    /// The status's warning while calls stay stuck after a remount.
    pub fn warning(&self) -> Option<String> {
        self.warn.as_ref().map(|p| {
            format!(
                "NAS calls stay busy after a remount (on {p}): another Mac's dead SMB session may hold it, disconnect it in DSM (docs/macs.md); remounts back off to every {} min",
                self.every() / 60
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(at: u64, p: &str) -> Stuck {
        Stuck { at, path: p.into() }
    }

    #[test]
    fn drains_then_remounts_when_idle() {
        let mut h = Heal::default();
        let t = 1_000_000;
        assert_eq!(h.tick(vec![st(t, "a"), st(t + 10, "a")], t + 20, true), Act::Nothing);
        assert!(h.drain_why().is_none(), "two aren't enough");
        assert_eq!(h.tick(vec![st(t + 30, "m")], t + 40, false), Act::Nothing);
        assert!(h.drain_why().unwrap().contains("on m"), "draining, a job running");
        assert_eq!(h.tick(vec![], t + 50, false), Act::Nothing);
        assert_eq!(h.tick(vec![], t + 60, true), Act::Remount);
        h.remounted(t + 60);
        assert!(h.drain_why().is_none() && h.warning().is_none());
        // Stuck again after it: a warning naming the file, and the next drain only after the
        // interval, which then doubles.
        let u = t + 100;
        h.tick(vec![st(u, "m"), st(u + 1, "m"), st(u + 2, "m")], u + 3, true);
        assert!(h.warning().unwrap().contains("on m") && h.drain_why().is_none());
        let v = t + 60 + EVERY;
        let new: Vec<_> = (0..3).map(|i| st(v + i, "m")).collect();
        assert_eq!(h.tick(new, v + 5, false), Act::Nothing);
        assert!(h.drain_why().is_some() && h.warning().unwrap().contains("every 60 min"));
        // A quiet window after the next remount: healed.
        h.tick(vec![], v + 6, true);
        h.remounted(v + 6);
        h.tick(vec![], v + 6 + WINDOW, true);
        assert!(h.warning().is_none() && h.every() == EVERY);
    }

    #[test]
    fn a_drain_gives_up_without_cutting_jobs() {
        let mut h = Heal::default();
        let t = 1_000_000;
        h.tick((0..3).map(|i| st(t + i, "s")).collect(), t + 5, false);
        assert!(h.drain_why().is_some());
        assert_eq!(h.tick(vec![], t + 5 + DRAIN_MAX, false), Act::Nothing);
        assert!(h.drain_why().is_none());
        // Not again before the interval.
        h.tick((0..3).map(|i| st(t + 10 + DRAIN_MAX + i, "s")).collect(), t + 20 + DRAIN_MAX, false);
        assert!(h.drain_why().is_none());
    }
}
