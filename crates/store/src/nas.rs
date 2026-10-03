//! Finding and mounting the NAS share (plan §4, Server). The mount table is read with
//! `getfsstat(MNT_NOWAIT)`, which answers from the kernel's records without asking any
//! filesystem, so a dead mount can't block it (`statfs` on a path can). A missing share is mounted
//! with `osascript`'s `mount volume`, which takes the credentials from the Keychain, and
//! `~/Library/Preferences/nsmb.conf` makes it a soft mount, so a dead NAS fails operations instead
//! of hanging them forever.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// The NAS: its host, the share holding the project, the project folder in it, and the URL to
/// mount the share by (the password comes from the Keychain).
///
/// The share is mounted by the NAS's LAN name (Bonjour, `.local`): the bare name resolves through
/// Tailscale's DNS to its tailnet address when Tailscale runs, and the NAS's Tailscale (userspace
/// networking) is CPU-bound: SMB writes through it ran at 12 MB/s on 2026-10-03, and an SSH stream
/// at half the LAN's rate. The share is only mounted at home (the server checks the LAN name
/// first), so the LAN name always resolves then.
pub const HOST: &str = "fishandchips";
pub const LAN_HOST: &str = "fishandchips.local";
pub const SHARE: &str = "personal";
pub const PROJECT: &str = "projects/scenic-roads";
pub const SMB_URL: &str = "smb://brandontsang@fishandchips.local/personal";

/// Whether a mount reaches the NAS by its LAN name (not the bare name, which Tailscale's DNS can
/// send through the tunnel).
pub fn by_lan_name(m: &Mount) -> bool {
    matches_from(&m.from, LAN_HOST, SHARE) || m.from.to_lowercase().contains("._smb._tcp.local/")
}

/// A mounted SMB share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// Where it's mounted, e.g. `/Volumes/personal`.
    pub point: PathBuf,
    /// What's mounted, e.g. `//brandontsang@fishandchips/personal`.
    pub from: String,
}

/// The SMB mount of `share` on the host `host_hint` (matched case-insensitively, as the host
/// itself or followed by a dot: "fishandchips" matches `fishandchips.local` and
/// `FISHANDCHIPS._smb._tcp.local`). Never blocks on a dead mount.
pub fn find_mount(host_hint: &str, share: &str) -> Option<Mount> {
    smb_mounts().into_iter().find(|m| matches_from(&m.from, host_hint, share))
}

/// Every SMB mount in the kernel's table.
pub fn smb_mounts() -> Vec<Mount> {
    match mount_table() {
        Ok(t) => t.into_iter().filter(|(fstype, _, _)| fstype == "smbfs").map(|(_, from, point)| Mount { point: PathBuf::from(point), from }).collect(),
        Err(e) => {
            eprintln!("nas: can't read the mount table: {e}");
            Vec::new()
        }
    }
}

/// (filesystem type, mounted from, mounted on) for every mount, from `getfsstat(MNT_NOWAIT)`.
#[cfg(target_os = "macos")]
fn mount_table() -> io::Result<Vec<(String, String, String)>> {
    use std::mem::size_of;
    use std::os::raw::c_int;
    loop {
        // SAFETY: a null buffer asks only for the number of mounts.
        let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let cap = n as usize + 4;
        // SAFETY: `statfs` is plain old data; all zeroes is a valid value.
        let mut buf: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; cap];
        let bytes = c_int::try_from(cap * size_of::<libc::statfs>()).map_err(io::Error::other)?;
        // SAFETY: the buffer holds `cap` entries and its size in bytes is passed along.
        let got = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        if got as usize >= cap {
            continue; // mounts were added meanwhile and may not all fit: ask again
        }
        buf.truncate(got as usize);
        return Ok(buf.iter().map(|s| (c_string(&s.f_fstypename), c_string(&s.f_mntfromname), c_string(&s.f_mntonname))).collect());
    }
}

#[cfg(not(target_os = "macos"))]
fn mount_table() -> io::Result<Vec<(String, String, String)>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "getfsstat is macOS-only"))
}

#[cfg(target_os = "macos")]
fn c_string(chars: &[std::os::raw::c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Whether an smbfs `f_mntfromname` (`//[domain;][user@]host[:port]/share[/path]`, percent-encoded)
/// names `share` on a host matching `host_hint`.
fn matches_from(from: &str, host_hint: &str, share: &str) -> bool {
    let Some(rest) = from.strip_prefix("//") else { return false };
    let Some((authority, path)) = rest.split_once('/') else { return false };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => host.split(':').next().unwrap_or(host),
    };
    let host = percent_decode(host).to_lowercase();
    let hint = host_hint.to_lowercase();
    let host_ok = !hint.is_empty() && (host == hint || host.strip_prefix(&hint).is_some_and(|r| r.starts_with('.')));
    let first = path.split('/').next().unwrap_or("");
    host_ok && percent_decode(first).to_lowercase() == share.to_lowercase()
}

fn percent_decode(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Mounts `url` (e.g. `smb://brandontsang@fishandchips/personal`) with AppleScript's
/// `mount volume`, which takes the password from the Keychain. If it hasn't answered within
/// `timeout` (a dialog asking for a password, an unreachable host), it is killed and this fails.
/// An already mounted share succeeds at once. Afterwards, `find_mount` gives the mount point.
pub fn mount(url: &str, timeout: Duration) -> Result<()> {
    let mut cmd = Command::new("/usr/bin/osascript");
    cmd.arg("-e").arg(format!("mount volume \"{}\"", applescript_escape(url)));
    run_with_timeout(cmd, timeout).with_context(|| format!("mount {url}"))
}

/// `s` inside an AppleScript string literal.
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Runs `cmd` to completion within `timeout`, killing it otherwise; an error with its stderr when
/// it fails.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("can't run it")?;
    let mut stderr = child.stderr.take();
    // Read stderr on the side so a chatty child can't block on a full pipe.
    let reader = thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            let err = reader.join().unwrap_or_default();
            if status.success() {
                return Ok(());
            }
            let err = err.trim();
            bail!("{status}{}{err}", if err.is_empty() { "" } else { ": " });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("no answer within {timeout:?} (stopped it)");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Makes sure `~/Library/Preferences/nsmb.conf` has a `[HOST:SHARE]` section (upper case) with
/// `soft=yes`, so mounts of the share are soft (docs/formats.md). Existing content is kept: a
/// missing section is appended, a missing `soft` line is added to the section. Returns whether
/// the file changed. It applies to mounts made afterwards, not to one already in place.
pub fn ensure_nsmb_conf(host: &str, share: &str) -> Result<bool> {
    let home = std::env::var_os("HOME").context("HOME isn't set")?;
    ensure_nsmb_conf_in(Path::new(&home), host, share)
}

/// `ensure_nsmb_conf` for the home folder `home`.
pub fn ensure_nsmb_conf_in(home: &Path, host: &str, share: &str) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let mut path = home.join("Library/Preferences/nsmb.conf");
    // A symlinked file (dotfiles kept elsewhere) is edited where it lives, keeping the link.
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        path = fs::canonicalize(&path).with_context(|| format!("resolve {}", path.display()))?;
    }
    let section = format!("{}:{}", host.to_uppercase(), share.to_uppercase());
    let (old, mode) = match fs::read_to_string(&path) {
        Ok(s) => (s, fs::metadata(&path).ok().map(|m| m.permissions().mode() & 0o7777)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (String::new(), None),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let Some(new) = with_soft(&old, &section).with_context(|| format!("{}", path.display()))? else {
        return Ok(false);
    };
    crate::naming::replace_file(&path, new.as_bytes(), mode).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

/// `text` with `soft=yes` in section `[name]`, or None when it's there already. An error when the
/// section sets `soft` to something else (the user's choice, not ours to overwrite).
fn with_soft(text: &str, name: &str) -> Result<Option<String>> {
    let lines: Vec<&str> = text.lines().collect();
    let header = |l: &str| -> Option<String> {
        let t = l.trim();
        (t.starts_with('[') && t.ends_with(']')).then(|| t[1..t.len() - 1].trim().to_uppercase())
    };
    let mut first_header = None;
    let mut in_section = false;
    for (i, l) in lines.iter().enumerate() {
        if let Some(h) = header(l) {
            in_section = h == name;
            if in_section && first_header.is_none() {
                first_header = Some(i);
            }
            continue;
        }
        let t = l.trim();
        if !in_section || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim().eq_ignore_ascii_case("soft") {
                if v.trim().eq_ignore_ascii_case("yes") {
                    return Ok(None);
                }
                bail!("[{name}] sets soft={} (left as is; soft=yes is needed so a dead NAS can't hang the server)", v.trim());
            }
        }
    }
    let mut out = String::with_capacity(text.len() + name.len() + 16);
    match first_header {
        Some(h) => {
            for (i, l) in lines.iter().enumerate() {
                out.push_str(l);
                out.push('\n');
                if i == h {
                    out.push_str("soft=yes\n");
                }
            }
        }
        None => {
            out.push_str(text);
            if !text.is_empty() {
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(&format!("[{name}]\nsoft=yes\n"));
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_names() {
        for from in [
            "//brandontsang@fishandchips/personal",
            "//brandontsang@fishandchips.local/personal",
            "//brandontsang@FISHANDCHIPS._smb._tcp.local/Personal",
            "//fishandchips/personal",
            "//WORKGROUP;brandontsang@fishandchips:445/personal",
            "//brandontsang@fishandchips/personal/projects",
        ] {
            assert!(matches_from(from, "fishandchips", "personal"), "{from}");
            assert!(matches_from(from, "FishAndChips", "PERSONAL"), "{from}");
        }
        for from in [
            "//brandontsang@fishandchipsx/personal",
            "//brandontsang@fishandchips/personalx",
            "//brandontsang@fishandchips/other",
            "//brandontsang@other/personal",
            "fishandchips/personal",
            "//fishandchips",
        ] {
            assert!(!matches_from(from, "fishandchips", "personal"), "{from}");
        }
        assert!(matches_from("//me@nas/My%20Share", "nas", "my share"));
        // Mounted by the LAN name (or Bonjour's service name), not the bare name.
        let m = |from: &str| Mount { point: PathBuf::from("/Volumes/personal"), from: from.to_string() };
        assert!(by_lan_name(&m("//brandontsang@fishandchips.local/personal")));
        assert!(by_lan_name(&m("//brandontsang@FISHANDCHIPS._smb._tcp.local/Personal")));
        assert!(!by_lan_name(&m("//brandontsang@fishandchips/personal")));
        assert!(matches_from("//me@[fe80::1]/data", "fe80::1", "data"));
        assert!(!matches_from("//me@nas/personal", "", "personal"));
        assert_eq!(percent_decode("a%2Fb%zz%4"), "a/b%zz%4");
    }

    #[test]
    fn mount_table_reads() {
        // The root volume is always there; SMB mounts may or may not be.
        let t = mount_table().unwrap();
        assert!(t.iter().any(|(_, _, on)| on == "/"), "{t:?}");
        for m in smb_mounts() {
            assert!(m.from.starts_with("//"), "{m:?}");
        }
        assert!(find_mount("no-such-host-anywhere", "nothing").is_none());
    }

    #[test]
    fn commands_time_out_and_report_errors() {
        let t0 = Instant::now();
        let mut c = Command::new("/bin/sleep");
        c.arg("5");
        let e = run_with_timeout(c, Duration::from_millis(200)).unwrap_err();
        assert!(e.to_string().contains("no answer"), "{e:#}");
        assert!(t0.elapsed() < Duration::from_secs(2));

        let mut c = Command::new("/bin/sh");
        c.args(["-c", "echo 'execution error: no such volume (-43)' >&2; exit 1"]);
        let e = run_with_timeout(c, Duration::from_secs(5)).unwrap_err();
        assert!(e.to_string().contains("no such volume"), "{e:#}");

        run_with_timeout(Command::new("/usr/bin/true"), Duration::from_secs(5)).unwrap();
        assert_eq!(applescript_escape(r#"smb://a"b\c"#), r#"smb://a\"b\\c"#);
    }

    #[test]
    fn nsmb_conf_editing() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("Library/Preferences/nsmb.conf");

        // No file: created with the section.
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[FISHANDCHIPS:PERSONAL]\nsoft=yes\n");
        assert!(!ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());

        // Someone else's settings are kept; the section is appended.
        let theirs = "# mine\n[default]\nsigning_required=no\n[OTHER]\nsoft=no";
        fs::write(&path, theirs).unwrap();
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{theirs}\n\n[FISHANDCHIPS:PERSONAL]\nsoft=yes\n"));
        assert!(!ensure_nsmb_conf_in(home.path(), "FishAndChips", "Personal").unwrap());

        // The section without soft: the line is added under its header.
        fs::write(&path, "[default]\nstreams=yes\n\n[fishandchips:personal]\nnotify_off=yes\n").unwrap();
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[default]\nstreams=yes\n\n[fishandchips:personal]\nsoft=yes\nnotify_off=yes\n");

        // Already soft (any spacing or case): untouched.
        fs::write(&path, "[FISHANDCHIPS:PERSONAL]\n  Soft = YES\n").unwrap();
        assert!(!ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        // soft=yes in another section doesn't count.
        fs::write(&path, "[default]\nsoft=yes\n").unwrap();
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        // An explicit soft=no is the user's: an error, and the file is left alone.
        fs::write(&path, "[FISHANDCHIPS:PERSONAL]\nsoft=no\n").unwrap();
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[FISHANDCHIPS:PERSONAL]\nsoft=no\n");
        // Commented-out lines don't count.
        fs::write(&path, "[FISHANDCHIPS:PERSONAL]\n# soft=no\n").unwrap();
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[FISHANDCHIPS:PERSONAL]\nsoft=yes\n# soft=no\n");
        let left: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["nsmb.conf"], "no temporary files left behind");

        // The file's mode is kept, and a symlinked file is edited where it lives.
        use std::os::unix::fs::PermissionsExt;
        fs::write(&path, "[default]\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let real = home.path().join("dotfiles-nsmb.conf");
        fs::write(&real, "[default]\n").unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();
        assert!(ensure_nsmb_conf_in(home.path(), "fishandchips", "personal").unwrap());
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "[default]\n\n[FISHANDCHIPS:PERSONAL]\nsoft=yes\n");
    }
}
