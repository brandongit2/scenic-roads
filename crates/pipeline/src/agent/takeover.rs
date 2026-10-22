//! The agent's lock held by an agent outside this one's launcher (task #582): an orphan (its
//! launcher gone, ppid 1) or one run by a stale launcher (from before the build service was
//! registered again). Left alone, the registered launcher's agent fails "another agent is running"
//! every few seconds for ever while the stray one runs on unregistered. So an agent its launcher
//! started, finding the lock held, ends the stray one cleanly: SIGTERM to that exact process (an
//! orphan), or to the stale launcher, which passes it on to its agent and exits. Either way that
//! agent stops as it would for an update: its jobs left running and their records saved for the
//! next agent (`Agent::hand_over_jobs`), which takes them over (`adopt_jobs`). This one waits for
//! the lock (`WAIT`, past the stopping agent's own bounded waits) and goes on as the agent.
//!
//! An agent run by hand and detached (ppid 1) counts as an orphan and is ended too. An agent no
//! launcher started (run by hand) ends nothing; nor does one whose holder's launcher
//! started after its own (the newest launcher is the registered one: two launchers never take turns
//! ending each other's agents).

use crate::sys::Proc;
use std::path::Path;
use std::time::{Duration, Instant};

/// The launcher's executable (tools/launcher/launcher.c).
const LAUNCHER: &str = "scenic-launcher";

/// How long the lock is waited for once its holder is asked to stop: its loop ends within seconds,
/// then the pool's thread (a minute at most) and a caches' task (a minute at most) are waited for.
pub const WAIT: Duration = Duration::from_secs(150);

/// Whom to end for the lock (the process, and why), from this process's parent, the holder and the
/// holder's parent; None: leave it (and fail as before). An older launcher counts only when it runs
/// the same executable as this one's (the old login item's launcher and the app's never end each
/// other's agents).
pub fn verdict(me_parent: Option<&Proc>, holder: &Proc, holder_parent: Option<&Proc>) -> Option<(Proc, String)> {
    let mine = me_parent.filter(|p| p.exe == LAUNCHER)?;
    if holder.ppid == mine.pid {
        // (Its own earlier child, still ending: the launcher waits for it before starting another.)
        return None;
    }
    if holder.ppid == 1 {
        return Some((
            holder.clone(),
            format!("an orphan agent (pid {}, its launcher gone) holds the lock", holder.pid),
        ));
    }
    let theirs = holder_parent.filter(|p| p.pid == holder.ppid && p.path == mine.path)?;
    ((theirs.start_us, theirs.pid) < (mine.start_us, mine.pid)).then(|| {
        (
            theirs.clone(),
            format!("an agent (pid {}) of an older launcher (pid {}) holds the lock", holder.pid, theirs.pid),
        )
    })
}

/// The lock at `lock` held elsewhere: its holder ended when `verdict` says so, and the lock taken
/// once it lets go (within `wait`). Ok(None): nothing ended, or the lock still held after `wait`.
pub fn take_over(home: &Path, wait: Duration) -> anyhow::Result<Option<super::AgentLock>> {
    let lock = home.join("agent.lock");
    let me = crate::sys::process(std::process::id() as i32);
    let me_parent = me.and_then(|m| crate::sys::process(m.ppid));
    // (The holder: the one opener that's an agent, `scenic`; a child between its fork and exec
    // holds a copy of a descriptor for a moment, PathLock's note.)
    let Some(holder) =
        crate::sys::openers(&lock).into_iter().filter_map(crate::sys::process).find(|p| p.exe == "scenic")
    else {
        return Ok(None);
    };
    let holder_parent = crate::sys::process(holder.ppid);
    let Some((target, why)) = verdict(me_parent.as_ref(), &holder, holder_parent.as_ref()) else { return Ok(None) };
    let pid = target.pid;
    // (Read again right before the signal: a pid freed and reused meanwhile is someone else's.)
    if !same_process(&target, crate::sys::process(pid).as_ref()) {
        return Ok(None);
    }
    eprintln!("agent: {why}: asking pid {pid} to stop (SIGTERM), its jobs left for this agent");
    if !crate::sys::terminate(pid) {
        return Ok(None);
    }
    let t = Instant::now();
    while t.elapsed() < wait {
        if let Some(l) = super::AgentLock::try_take(home)? {
            eprintln!("agent: the lock taken over in {:.0} s", t.elapsed().as_secs_f64());
            return Ok(Some(l));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    eprintln!("agent: pid {pid} still holds the lock after {} s", wait.as_secs());
    Ok(None)
}

/// Whether `now` (the pid read again) is still the process `was`: its start and executable.
fn same_process(was: &Proc, now: Option<&Proc>) -> bool {
    now.is_some_and(|n| n.pid == was.pid && n.start_us == was.start_us && n.path == was.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: i32, ppid: i32, start_us: u64, exe: &str) -> Proc {
        let path = if exe == LAUNCHER { "/Applications/Scenic.app/Contents/MacOS/scenic-launcher" } else { exe };
        Proc { pid, ppid, start_us, path: path.into(), exe: exe.into() }
    }

    #[test]
    fn ends_an_orphan_or_an_older_launcher_only_from_a_launcher() {
        let mine = p(100, 1, 5_000, LAUNCHER);
        // An orphan: ended.
        let orphan = p(50, 1, 1_000, "scenic");
        assert_eq!(verdict(Some(&mine), &orphan, None).map(|v| v.0.pid), Some(50));
        // Not run by a launcher (by hand, or a test): nothing ended.
        assert_eq!(verdict(Some(&p(100, 1, 5_000, "zsh")), &orphan, None), None);
        assert_eq!(verdict(None, &orphan, None), None);
        // An older launcher's agent: that launcher ended (it passes SIGTERM on).
        let old = p(40, 1, 1_000, LAUNCHER);
        let theirs = p(41, 40, 1_100, "scenic");
        assert_eq!(verdict(Some(&mine), &theirs, Some(&old)).map(|v| v.0.pid), Some(40));
        // A newer launcher's: left (this launcher is the stale one).
        let newer = p(140, 1, 9_000, LAUNCHER);
        assert_eq!(verdict(Some(&mine), &p(141, 140, 9_100, "scenic"), Some(&newer)), None);
        // An older launcher at another path (the old login item's): left.
        let legacy = Proc { path: "/Users/x/Library/Application Support/scenic/scenic-launcher".into(), ..old.clone() };
        assert_eq!(verdict(Some(&mine), &theirs, Some(&legacy)), None);
        // Its own launcher's earlier child: left.
        assert_eq!(verdict(Some(&mine), &p(101, 100, 5_100, "scenic"), Some(&mine)), None);
        // A holder under something else (a shell): left.
        assert_eq!(verdict(Some(&mine), &p(61, 60, 1_100, "scenic"), Some(&p(60, 1, 1_000, "zsh"))), None);
    }

    #[test]
    fn a_reused_pid_is_not_the_same_process() {
        let was = p(50, 1, 1_000, "scenic");
        assert!(same_process(&was, Some(&was.clone())));
        assert!(!same_process(&was, None));
        assert!(!same_process(&was, Some(&Proc { start_us: 2_000, ..was.clone() })));
        assert!(!same_process(&was, Some(&Proc { path: "/bin/sleep".into(), ..was.clone() })));
    }

    #[test]
    fn openers_names_who_holds_a_file() {
        // (This process's own descriptor isn't listed: another's is.)
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("held");
        std::fs::write(&f, b"").unwrap();
        let mut c = store::spawn::spawn(store::spawn::command("/bin/sh").args([
            "-c",
            "exec 3<\"$0\"; sleep 30",
            f.to_str().unwrap(),
        ]))
        .unwrap();
        let t = Instant::now();
        let mut seen = false;
        while t.elapsed() < Duration::from_secs(5) && !seen {
            seen = crate::sys::openers(&f).contains(&(c.id() as i32));
            std::thread::sleep(Duration::from_millis(50));
        }
        crate::sys::terminate(c.id() as i32);
        c.wait().unwrap();
        assert!(seen);
        let me = crate::sys::process(std::process::id() as i32).unwrap();
        assert!(me.start_us > 0 && me.ppid > 0);
    }
}
