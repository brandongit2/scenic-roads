//! An `ssh` to the NAS, its start bounded (`SSH_START`, `watched`): out's `verify` and the OSM
//! pass's streams (pipeline::osmpass) take it; and the job's proxy's address, which `ssh` and
//! fetch's requests go through.

use anyhow::{ensure, Context, Result};

/// How long an `ssh` to the NAS may take to start: to connect (ConnectTimeout, 20 s) and then to
/// authenticate, its key from the SSH agent, 1Password's, which waits for an approval while it's
/// locked, unbounded (ConnectTimeout doesn't cover it); a working one starts within seconds. The
/// call past it is stopped and fails `TimedOut`, as a NAS call past its deadline does
/// (pipeline::agent::nasread): the job fails and is tried again later.
pub const SSH_START: std::time::Duration = std::time::Duration::from_secs(60);

/// Runs `f` with `pid` (a child not yet waited for: the caller waits after) stopped (SIGTERM) if
/// `f` hasn't called its argument ("started") within `bound`, or returned; and whether it was.
pub fn watched<T>(pid: u32, bound: std::time::Duration, f: impl FnOnce(&dyn Fn()) -> T) -> (T, bool) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|s| {
        let dog = s.spawn(move || {
            let fire = rx.recv_timeout(bound) == Err(std::sync::mpsc::RecvTimeoutError::Timeout);
            if fire {
                // SAFETY: a plain signal to the caller's unreaped child.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }
            fire
        });
        let got = f(&|| {
            let _ = tx.send(());
        });
        drop(tx);
        (got, dog.join().unwrap_or(false))
    })
}

/// `ssh` (a command line, its host last) runs `true` within `SSH_START`: authenticated, and its
/// shared connection opened when it makes one (ControlMaster), for the calls after to take.
pub fn ssh_opened(ssh: &[&str]) -> Result<()> {
    let mut probe = store::spawn::command(ssh[0]);
    probe.args(&ssh[1..]).arg("true").stdin(std::process::Stdio::null());
    let mut child = store::spawn::spawn(&mut probe).context("ssh to the NAS")?;
    let pid = child.id();
    let ((), stalled) = watched(pid, SSH_START, |_| {
        // (Its end awaited, the child left to `wait` below: its PID stays its own till then.)
        // SAFETY: a zeroed siginfo_t, written by waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
    });
    let st = child.wait()?;
    // (One that ended well just as the bound fired stands: the signal met a finished child.)
    if stalled && !st.success() {
        return Err(ssh_stalled("its first call", SSH_START));
    }
    ensure!(st.success(), "ssh to the NAS failed: {st}");
    Ok(())
}

/// The error for an `ssh` to the NAS stopped by `watched`.
pub fn ssh_stalled(what: &str, bound: std::time::Duration) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "ssh {} ({what}) didn't start within {} s: stopped (connecting, or its SSH agent waiting for an approval: 1Password locked?)",
            store::nas::SSH,
            bound.as_secs()
        ),
    ))
}

/// The proxy's `host:port` from its URL (`http://127.0.0.1:<port>`).
pub fn proxy_addr(url: &str) -> Option<String> {
    let a = url.strip_prefix("http://")?.trim_end_matches('/');
    a.contains(':').then(|| a.to_string())
}
