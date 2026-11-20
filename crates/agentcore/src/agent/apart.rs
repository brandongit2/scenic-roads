//! The NAS's answers apart from the jobs' SMB session (task #1552; docs/pool.md §7.7, The pacing):
//! a probe on an ssh of its own to the NAS (store::nas::SSH, the key the app's other ssh calls
//! take: never a password), kept open, whose shell on the NAS reads a little from its disks for
//! each line sent and answers a line (`script`). A listing on the jobs' own mount waits behind
//! their traffic on the one SMB session (on the mini, 20 Nov, in the seconds its session moved 5
//! MB/s or more: a median of 73-78 ms, 60-62 % over 30 ms, against 1.3-1.8 ms for a listing on the
//! ssh beside it, none over 30 ms), so those answers can't tell the NAS's load from the Mac's own;
//! these can. Sampled by crate::agent::live's NAS thread (`NAS_APART_MS`) as often as the pool's
//! `nas-apart-seconds` says, while a build runs.
//!
//! `Probe::ask` sends a line and waits for its answer within the deadline: the time it took, or,
//! past the deadline, the deadline (a NAS too slow to answer is the slowest answer, as
//! crate::agent::nasread::probe counts its own), the connection then closed to be opened again at
//! the next ask. A connection that won't open (ssh failing, its key's agent not answering within
//! buildstore::nasssh::SSH_START) or that ended is **down**: the ask fails with why, and none is
//! tried again for `RETRY`; meanwhile the Mac's answers are its jobs' session's, as without the
//! probe, and the lead flags it (crate::agent::flags, `nas-apart`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a probe that went down waits before opening a connection again.
pub const RETRY: Duration = Duration::from_secs(60);

/// The shell the probe runs on the NAS, for the project folder `dir` (the NAS's own path,
/// store::nas::on_nas): for each line read, a 4 KB read past the NAS's page cache (O_DIRECT) at a
/// random offset of the largest file of `sources/canopy` (`READ_FROM`, taken once), then a line
/// back, so each answer waits on the NAS's disks as a job's cold read does. A listing, as the
/// jobs' session's probe makes, comes from the NAS's cache on its own connection: 20 Nov, its
/// answers didn't follow the disks' load (a rank correlation of -0.19 over 29 half minutes,
/// against 0.70 for these reads and 0.72 for the M4's own listings). No such file, or a read
/// failing: it ends (the probe down), never answering quickly for a read not made.
pub fn script(dir: &str) -> String {
    format!(
        "cd {dir} || exit 1; f=$(ls -S {READ_FROM}/* 2>/dev/null | head -n 1); n=$(( $(stat -c %s \"$f\" 2>/dev/null || echo 0) / 4096 )); \
         [ $n -gt 0 ] || exit 2; while read l; do dd if=\"$f\" of=/dev/null bs=4096 count=1 iflag=direct skip=$(( (RANDOM * 32768 + RANDOM) % n )) 2>/dev/null || exit 3; echo; done"
    )
}

/// The project's folder whose largest file the probe reads (`script`): the canopy heights'
/// tiles, a GB or more each, which no step rewrites.
pub const READ_FROM: &str = "sources/canopy";

/// The ssh command line for the probe of the NAS's folder `dir`: no prompt (`BatchMode`), its
/// connection checked every 15 s, the shell `script` on the NAS.
pub fn ssh_command(dir: &str) -> Vec<String> {
    let quoted = format!("'{}'", script(dir).replace('\'', "'\\''"));
    ["/usr/bin/ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=20", "-o", "ServerAliveInterval=15"]
        .iter()
        .map(|s| s.to_string())
        .chain([store::nas::SSH.to_string(), format!("bash -c {quoted}")])
        .collect()
}

/// A kept connection: the child, its input, and a line from its output for each answer (closed
/// at its end).
struct Conn {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<()>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        // (Its own child: stopped and waited for.)
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The probe: its command line (`ssh_command`, a test's own shell), its connection while open,
/// why it is down and until when it isn't tried again.
pub struct Probe {
    cmd: Vec<String>,
    open: Option<Conn>,
    down: Option<String>,
    retry_at: Option<Instant>,
    start: Duration,
}

impl Probe {
    /// A probe running `cmd` (its program by its path, store::spawn), the first answer of each
    /// connection awaited for `start`.
    pub fn new(cmd: Vec<String>, start: Duration) -> Probe {
        Probe { cmd, open: None, down: None, retry_at: None, start }
    }

    /// Why it is down (None: it answered last, or hasn't been asked).
    pub fn down(&self) -> Option<&str> {
        self.down.as_deref()
    }

    /// Its command line.
    pub fn cmd(&self) -> &[String] {
        &self.cmd
    }

    /// Closes its connection (the probe turned off, the build stopped), down no more.
    pub fn close(&mut self) {
        self.open = None;
        (self.down, self.retry_at) = (None, None);
    }

    /// One answer within `deadline` (the module's doc): its time, the deadline when it took
    /// longer; Err(why) while down.
    pub fn ask(&mut self, deadline: Duration) -> Result<Duration, String> {
        if self.open.is_none() {
            if self.retry_at.is_some_and(|at| Instant::now() < at) {
                return Err(self.down.clone().unwrap_or_default());
            }
            match self.connect() {
                Ok(c) => self.open = Some(c),
                Err(why) => return Err(self.went_down(why)),
            }
        }
        let c = self.open.as_mut().expect("opened above");
        let t0 = Instant::now();
        if c.input.write_all(b"x\n").and_then(|()| c.input.flush()).is_err() {
            self.open = None;
            return Err(self.went_down("its connection ended".into()));
        }
        match c.lines.recv_timeout(deadline) {
            Ok(()) => {
                self.down = None;
                Ok(t0.elapsed())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.open = None;
                Ok(deadline)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.open = None;
                Err(self.went_down("its connection ended".into()))
            }
        }
    }

    fn went_down(&mut self, why: String) -> String {
        self.down = Some(why.clone());
        self.retry_at = Some(Instant::now() + RETRY);
        why
    }

    /// A connection opened and its first answer had within `start`.
    fn connect(&self) -> Result<Conn, String> {
        let (prog, args) = self.cmd.split_first().ok_or("no command")?;
        let mut cmd = store::spawn::command(prog);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = store::spawn::spawn(&mut cmd).map_err(|e| format!("can't start {prog}: {e}"))?;
        let (Some(input), Some(out)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("its pipes not made".into());
        };
        let (tx, lines) = mpsc::channel();
        std::thread::Builder::new()
            .name("nas-apart".into())
            .spawn(move || {
                for l in BufReader::new(out).lines() {
                    if l.is_err() || tx.send(()).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| format!("its reader not started: {e}"))?;
        let mut c = Conn { child, input, lines };
        let first = c.input.write_all(b"x\n").and_then(|()| c.input.flush());
        match first.map(|()| c.lines.recv_timeout(self.start)) {
            Ok(Ok(())) => Ok(c),
            Ok(Err(mpsc::RecvTimeoutError::Timeout)) => {
                Err(format!("no first answer within {} s", self.start.as_secs()))
            }
            _ => Err(match c.child.try_wait() {
                Ok(Some(st)) => format!("it ended at once ({st})"),
                _ => "it ended at once".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe running `script` on a folder of this Mac's (bash as the NAS's shell).
    fn local(dir: &std::path::Path) -> Probe {
        Probe::new(vec!["/bin/bash".into(), "-c".into(), script(dir.to_str().unwrap())], Duration::from_secs(10))
    }

    /// A probe whose shell answers each line.
    fn echoing() -> Probe {
        Probe::new(vec!["/bin/bash".into(), "-c".into(), "while read l; do echo; done".into()], Duration::from_secs(10))
    }

    #[test]
    fn it_answers_a_line_for_each_line_on_one_connection() {
        let mut p = echoing();
        for _ in 0..3 {
            let t = p.ask(Duration::from_secs(10)).unwrap();
            assert!(t < Duration::from_secs(5), "{t:?}");
        }
        assert_eq!(p.down(), None);
        let pid = p.open.as_ref().unwrap().child.id();
        p.ask(Duration::from_secs(10)).unwrap();
        assert_eq!(p.open.as_ref().unwrap().child.id(), pid, "kept open");
    }

    #[test]
    fn a_folder_without_the_file_to_read_is_down_never_a_quick_answer() {
        let d = tempfile::tempdir().unwrap();
        let mut p = local(d.path());
        let why = p.ask(Duration::from_secs(10)).unwrap_err();
        assert!(why.contains("ended at once"), "{why}");
    }

    #[test]
    fn a_connection_that_ends_is_down() {
        let mut p = Probe::new(
            vec!["/bin/bash".into(), "-c".into(), "read l; echo; read l; echo; read l; exit 0".into()],
            Duration::from_secs(10),
        );
        assert!(p.ask(Duration::from_secs(10)).is_ok());
        assert_eq!(p.ask(Duration::from_secs(10)).unwrap_err(), "its connection ended");
        assert!(p.down().is_some());
    }

    #[test]
    fn a_connection_that_wont_open_is_down_and_not_tried_again_at_once() {
        let mut p = local(std::path::Path::new("/no/such/folder"));
        let why = p.ask(Duration::from_secs(5)).unwrap_err();
        assert!(why.contains("ended at once"), "{why}");
        assert_eq!(p.down(), Some(why.as_str()));
        // (Within `RETRY`: no new connection, the same why.)
        let t0 = Instant::now();
        assert_eq!(p.ask(Duration::from_secs(5)).unwrap_err(), why);
        assert!(t0.elapsed() < Duration::from_millis(100));
        p.close();
        assert_eq!(p.down(), None);
    }

    #[test]
    fn an_answer_past_the_deadline_counts_as_the_deadline_and_the_next_ask_reconnects() {
        // (A shell that answers its first line, then takes 5 s over each.)
        let slow = "read l; echo; while read l; do sleep 5; echo; done";
        let mut p = Probe::new(vec!["/bin/bash".into(), "-c".into(), slow.into()], Duration::from_secs(10));
        let d = Duration::from_millis(200);
        assert_eq!(p.ask(d), Ok(d));
        assert!(p.open.is_none(), "closed past its deadline");
        assert_eq!(p.down(), None, "slow, not down");
        assert_eq!(p.ask(d), Ok(d), "opened again");
    }

    #[test]
    fn the_ssh_goes_to_the_nas_without_a_prompt() {
        let c = ssh_command("/volume1/personal/x");
        assert_eq!(c[0], "/usr/bin/ssh");
        assert!(c.iter().any(|a| a == "BatchMode=yes"));
        assert_eq!(c[c.len() - 2], store::nas::SSH);
        assert!(c[c.len() - 1].starts_with("bash -c 'cd /volume1/personal/x || exit 1;"), "{}", c[c.len() - 1]);
    }
}
