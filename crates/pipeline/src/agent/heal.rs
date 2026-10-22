//! The agent's self-heal for stale SMB handles (docs/pool.md §3): when this process's NAS calls
//! keep staying "Resource busy" past `crate::whole`'s retries, the agent remounts its share, which
//! ends this Mac's SMB session and with it every handle the session held (a process here killed
//! with a file open). It can't free another Mac's session: that one ends when the server drops it
//! (the Synology and nsmb.conf settings in docs/macs.md shorten that), and meanwhile whole's
//! rename-aside keeps writes going.
//!
//! A forced unmount fails every file open on the share, so it happens only while no job of this
//! agent runs (`idle`); jobs that hold files keep them, and the remount waits for the slots to empty.

use std::time::{Duration, Instant};

/// Calls stuck within this long count.
const WINDOW: Duration = Duration::from_secs(10 * 60);
/// This many stuck calls within `WINDOW` ask for a remount.
const STUCK: usize = 3;
/// At most one remount this often.
const EVERY: Duration = Duration::from_secs(30 * 60);

/// Whether to remount now: `STUCK` calls stuck within `WINDOW`, none of them older than the last
/// remount (those it already answered), the last remount `EVERY` ago or never, and no job running.
pub fn due(stuck: &[Instant], last: Option<Instant>, idle: bool, now: Instant) -> bool {
    if !idle || last.is_some_and(|l| now.saturating_duration_since(l) < EVERY) {
        return false;
    }
    let recent =
        stuck.iter().filter(|t| now.saturating_duration_since(**t) <= WINDOW && last.is_none_or(|l| **t > l)).count();
    recent >= STUCK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_rules() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let three = [at(0), at(10), at(20)];
        assert!(due(&three, None, true, at(30)));
        assert!(!due(&three, None, false, at(30)), "a job running: waits");
        assert!(!due(&three[..2], None, true, at(30)), "two aren't enough");
        assert!(!due(&three, None, true, at(20 + 11 * 60)), "out of the window");
        // Remounted at 25: not again for half an hour, and only for calls stuck since.
        assert!(!due(&three, Some(at(25)), true, at(26)));
        assert!(!due(&three, Some(at(25)), true, at(25 + 31 * 60)));
        let later = [at(25 + 30 * 60), at(25 + 31 * 60), at(25 + 32 * 60)];
        assert!(due(&later, Some(at(25)), true, at(25 + 33 * 60)));
    }
}
