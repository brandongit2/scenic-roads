//! The NAS's answers apart from the jobs' SMB session (task #1552; docs/pool.md §7.7, The probe
//! apart): a probe on an ssh of its own to the NAS (store::nas::SSH, the key the app's other ssh
//! calls take: never a password), kept open, whose shell on the NAS reads a little from its disks
//! for each line sent and answers a line (`script`). A listing on the jobs' own mount waits behind
//! their traffic on the one SMB session (on the mini, 20 Nov, in the seconds its session moved 5
//! MB/s or more: a median of 73-78 ms, 60-62 % over 30 ms, against 1.3-1.8 ms for a listing on the
//! ssh beside it, none over 30 ms), so those answers can't tell the NAS's load from the Mac's own;
//! these may. Sampled by crate::agent::live's NAS thread (`NAS_APART_MS`) as often as the pool's
//! `nas-apart-seconds` says, while a build runs; judged by the pacing only with the pool's
//! `nas-apart-judges` on (in shadow until then).
//!
//! Its own connection, not the ssh calls' shared one (buildstore::nasssh's ControlMaster, the OSM
//! pass's streams): a probe multiplexed on a connection moving a stream would wait behind it, as
//! the listings wait behind the jobs' session.
//!
//! `Probe::ask` sends a line and waits for its answer within the deadline: the time it took, or,
//! past the deadline, the deadline (a NAS too slow to answer is the slowest answer, as
//! crate::agent::nasread::probe counts its own). A connection that won't open (ssh failing, the
//! file not read whole, its key's agent not answering within buildstore::nasssh::SSH_START: an
//! agent waiting for an approval, as 1Password's does while locked), that ended, or whose answer
//! passed its deadline is closed and **down**: no connection is opened again for `RETRY`, doubled
//! with each such end in a row to `RETRY_MOST`, so a locked key agent is asked once a step, never
//! in a stream; an answer in time ends the run. Meanwhile its figures are missing (with
//! `nas-apart-judges` on, the Mac's answers its jobs' session's), and the lead flags it (crate::agent::flags, `nas-apart`). Its
//! connections opened and answers past their deadline are counted (`connects`, `timeouts`) for the
//! status line.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a probe that went down first waits before opening a connection again, and the most
/// its doubling makes it.
pub const RETRY: Duration = Duration::from_secs(60);
pub const RETRY_MOST: Duration = Duration::from_secs(16 * 60);

/// The shell the probe runs on the NAS, for the project folder `dir` (the NAS's own path,
/// store::nas::on_nas): for each line read, a 4 KB read past the NAS's page cache (O_DIRECT) at a
/// random offset of the largest file of `sources/canopy` (`READ_FROM`, taken once), then a line
/// back, so each answer waits on the NAS's disks as a job's cold read does. A listing, as the
/// jobs' session's probe makes, comes from the NAS's cache on its own connection: 20 Nov, its
/// answers didn't follow the disks' load (a rank correlation of -0.19 over 29 half minutes,
/// against 0.70 for these reads and 0.72 for the M4's own listings). No such file, or a read
/// failing or short of its 4,096 bytes: it ends (the probe down), never answering quickly for a
/// read not made.
pub fn script(dir: &str) -> String {
    format!(
        "cd {dir} || exit 1; f=$(ls -S {READ_FROM}/* 2>/dev/null | head -n 1); n=$(( $(stat -c %s \"$f\" 2>/dev/null || echo 0) / 4096 )); \
         [ $n -gt 0 ] || exit 2; while read l; do \
         x=$(dd if=\"$f\" bs=4096 count=1 iflag=direct skip=$(( (RANDOM * 32768 + RANDOM) % n )) 2>/dev/null | wc -c); \
         [ \"$x\" -eq 4096 ] || exit 3; echo; done"
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
/// why it is down, until when it isn't tried again and how many ends in a row, and its counts.
pub struct Probe {
    cmd: Vec<String>,
    open: Option<Conn>,
    down: Option<String>,
    retry_at: Option<Instant>,
    ends: u32,
    start: Duration,
    /// Connections opened (their first answer had), and answers past their deadline.
    pub connects: u64,
    pub timeouts: u64,
}

impl Probe {
    /// A probe running `cmd` (its program by its path, store::spawn), the first answer of each
    /// connection awaited for `start`.
    pub fn new(cmd: Vec<String>, start: Duration) -> Probe {
        Probe { cmd, open: None, down: None, retry_at: None, ends: 0, start, connects: 0, timeouts: 0 }
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
        (self.down, self.retry_at, self.ends) = (None, None, 0);
    }

    /// One answer within `deadline` (the module's doc): its time, the deadline when it took
    /// longer; Err(why) while down.
    pub fn ask(&mut self, deadline: Duration) -> Result<Duration, String> {
        if self.open.is_none() {
            if self.retry_at.is_some_and(|at| Instant::now() < at) {
                return Err(self.down.clone().unwrap_or_default());
            }
            match self.connect() {
                Ok(c) => {
                    self.connects += 1;
                    self.open = Some(c);
                }
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
                (self.down, self.ends) = (None, 0);
                Ok(t0.elapsed())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.open = None;
                self.timeouts += 1;
                self.went_down(format!("an answer past its {} ms deadline", deadline.as_millis()));
                Ok(deadline)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.open = None;
                Err(self.went_down("its connection ended".into()))
            }
        }
    }

    /// Down for `why`: not tried again for `RETRY`, doubled for each end in a row before it, to
    /// `RETRY_MOST`.
    fn went_down(&mut self, why: String) -> String {
        let wait = RETRY.saturating_mul(1u32.checked_shl(self.ends).unwrap_or(u32::MAX)).min(RETRY_MOST);
        self.ends = self.ends.saturating_add(1);
        self.down = Some(format!("{why}; tried again in {} s", wait.as_secs()));
        self.retry_at = Some(Instant::now() + wait);
        self.down.clone().unwrap_or_default()
    }

    /// A connection opened and its first answer had within `start`.
    fn connect(&self) -> Result<Conn, String> {
        let (prog, args) = self.cmd.split_first().ok_or("no command")?;
        let mut cmd = store::spawn::command(prog);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = store::spawn::spawn(&mut cmd).map_err(|e| format!("can't start {prog}: {e}"))?;
        let reap = |mut child: Child, why: String| {
            let _ = child.kill();
            let _ = child.wait();
            why
        };
        let (Some(input), Some(out)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(reap(child, "its pipes not made".into()));
        };
        let (tx, lines) = mpsc::channel();
        let reader = std::thread::Builder::new().name("nas-apart".into()).spawn(move || {
            for l in BufReader::new(out).lines() {
                if l.is_err() || tx.send(()).is_err() {
                    break;
                }
            }
        });
        if let Err(e) = reader {
            return Err(reap(child, format!("its reader not started: {e}")));
        }
        let mut c = Conn { child, input, lines };
        let first = c.input.write_all(b"x\n").and_then(|()| c.input.flush());
        match first.map(|()| c.lines.recv_timeout(self.start)) {
            Ok(Ok(())) => Ok(c),
            Ok(Err(mpsc::RecvTimeoutError::Timeout)) => Err(format!(
                "no first answer within {} s: its key's agent not answering (1Password locked, waiting for an approval?), or the NAS not",
                self.start.as_secs()
            )),
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

    /// A probe running `script` on a folder of this Mac's (bash as the NAS's shell), with GNU's
    /// `stat -c` and `dd iflag=direct` played by stand-ins in `bin` (macOS has neither): a file
    /// there of 64 KB, and a read of `bytes` bytes.
    fn local(dir: &std::path::Path, bin: &std::path::Path, bytes: usize) -> Probe {
        // (`stat -c %s <file>`: 64 KB where the file is there, as GNU's fails where it isn't.)
        std::fs::write(bin.join("stat"), "#!/bin/sh\n[ -f \"$3\" ] && echo 65536\n").unwrap();
        std::fs::write(bin.join("dd"), format!("#!/bin/sh\nhead -c {bytes} /dev/zero\n")).unwrap();
        for f in ["stat", "dd"] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("PATH={}:/usr/bin:/bin", bin.display());
        let script = script(dir.to_str().unwrap());
        Probe::new(vec!["/usr/bin/env".into(), path, "/bin/bash".into(), "-c".into(), script], Duration::from_secs(10))
    }

    /// A project folder holding a file under `READ_FROM`, and a folder for the stand-ins.
    fn project() -> (tempfile::TempDir, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(READ_FROM)).unwrap();
        std::fs::write(d.path().join(READ_FROM).join("big.tif"), vec![7u8; 64 << 10]).unwrap();
        (d, tempfile::tempdir().unwrap())
    }

    /// A probe whose shell answers each line.
    fn echoing() -> Probe {
        Probe::new(vec!["/bin/bash".into(), "-c".into(), "while read l; do echo; done".into()], Duration::from_secs(10))
    }

    #[test]
    fn it_answers_a_line_for_each_line_on_one_connection() {
        let mut p = echoing();
        for _ in 0..3 {
            p.ask(Duration::from_secs(10)).unwrap();
        }
        assert_eq!(p.down(), None);
        let pid = p.open.as_ref().unwrap().child.id();
        p.ask(Duration::from_secs(10)).unwrap();
        assert_eq!(p.open.as_ref().unwrap().child.id(), pid, "kept open");
        assert_eq!((p.connects, p.timeouts), (1, 0));
    }

    #[test]
    fn the_script_answers_each_whole_read() {
        let (d, bin) = project();
        let mut p = local(d.path(), bin.path(), 4096);
        for _ in 0..3 {
            p.ask(Duration::from_secs(10)).unwrap();
        }
        assert_eq!(p.down(), None);
    }

    #[test]
    fn a_short_read_is_down_never_a_quick_answer() {
        let (d, bin) = project();
        let mut p = local(d.path(), bin.path(), 100);
        let why = p.ask(Duration::from_secs(10)).unwrap_err();
        assert!(why.contains("ended at once") && why.contains("exit status: 3"), "{why}");
    }

    #[test]
    fn a_folder_without_the_file_to_read_is_down_never_a_quick_answer() {
        let (d, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut p = local(d.path(), bin.path(), 4096);
        let why = p.ask(Duration::from_secs(10)).unwrap_err();
        assert!(why.contains("ended at once") && why.contains("exit status: 2"), "{why}");
    }

    #[test]
    fn a_connection_that_ends_is_down() {
        // (Its first answer the connection's opening, its second the ask's; then it ends.)
        let mut p = Probe::new(
            vec!["/bin/bash".into(), "-c".into(), "read l; echo; read l; echo; read l; exit 0".into()],
            Duration::from_secs(10),
        );
        assert!(p.ask(Duration::from_secs(10)).is_ok());
        assert!(p.ask(Duration::from_secs(10)).unwrap_err().starts_with("its connection ended; tried again in 60 s"));
        assert!(p.down().is_some());
    }

    #[test]
    fn a_connection_that_wont_open_is_down_and_not_tried_again_at_once() {
        let (d, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut p = local(d.path(), bin.path(), 4096);
        let why = p.ask(Duration::from_secs(5)).unwrap_err();
        assert!(why.ends_with("tried again in 60 s"), "{why}");
        assert_eq!(p.down(), Some(why.as_str()));
        // (Within `RETRY`: no new connection, the same why, though one would open now.)
        p.cmd = echoing().cmd;
        assert_eq!(p.ask(Duration::from_secs(5)).unwrap_err(), why);
        assert!(p.open.is_none());
        assert_eq!(p.connects, 0);
        p.close();
        assert_eq!(p.down(), None);
        assert!(p.ask(Duration::from_secs(5)).is_ok(), "closed, tried again");
    }

    #[test]
    fn each_end_in_a_row_waits_twice_as_long_and_an_answer_ends_the_run() {
        let mut p = echoing();
        let waits: Vec<String> = (0..7).map(|_| p.went_down("x".into())).collect();
        assert_eq!(
            waits,
            ["60", "120", "240", "480", "960", "960", "960"].map(|s| format!("x; tried again in {s} s")),
            "doubled to RETRY_MOST"
        );
        p.retry_at = None;
        p.ask(Duration::from_secs(10)).unwrap();
        assert_eq!(p.went_down("x".into()), "x; tried again in 60 s", "an answer in time ended the run");
    }

    #[test]
    fn an_answer_past_the_deadline_counts_as_the_deadline_and_backs_off() {
        // (A shell that answers its first line, then takes 5 s over each.)
        let slow = "read l; echo; while read l; do sleep 5; echo; done";
        let mut p = Probe::new(vec!["/bin/bash".into(), "-c".into(), slow.into()], Duration::from_secs(10));
        let d = Duration::from_millis(200);
        assert_eq!(p.ask(d), Ok(d));
        assert!(p.open.is_none(), "closed past its deadline");
        assert_eq!((p.connects, p.timeouts), (1, 1));
        // (Not opened again at once: one attempt a backoff step, a locked key agent asked once.)
        let why = p.ask(d).unwrap_err();
        assert!(why.starts_with("an answer past its 200 ms deadline; tried again in 60 s"), "{why}");
        assert_eq!(p.connects, 1);
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
