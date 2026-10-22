//! The Synology's own figures over SNMPv3 (docs/pool.md §7.7, task #534): the one reader of them,
//! for the NAS budget's closed loop (crate::budget::nas::Loop) and the build page's NAS figures
//! (task #517). Polled by the lead alone. The keys (`NAS_SNMP_USER`, `NAS_SNMP_AUTH`) are read
//! from the project's `inputs/keys.env` (git-ignored copies in each Mac's `data/keys.env`), handed
//! to `/usr/bin/snmpget` in a private config file (`Session`), never on its command line, and
//! never logged or printed.

use std::path::Path;
use std::time::Duration;

/// The NAS.
pub const HOST: &str = "fishandchips.local";
/// The keys' file, under the project folder.
pub const KEYS: &str = "inputs/keys.env";
/// SYNOLOGY-STORAGEIO-MIB storageIOLA: each disk's busy share now (%), disks 1 to 3.
pub const DISK_BUSY: [&str; 3] =
    [".1.3.6.1.4.1.6574.101.1.1.8.1", ".1.3.6.1.4.1.6574.101.1.1.8.2", ".1.3.6.1.4.1.6574.101.1.1.8.3"];
/// How long one poll may take before it counts as failed.
pub const TIMEOUT: Duration = Duration::from_secs(3);

/// The SNMPv3 user and its auth key (Debug shows neither).
#[derive(Clone, PartialEq, Eq)]
pub struct Keys {
    user: String,
    auth: String,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Keys(..)")
    }
}

/// Why there are no keys to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoKeys {
    /// No file, or a key missing or empty in it.
    Missing,
    /// A key is there but net-snmp's conf format can't carry its value (it splits a line at
    /// whitespace and ends it at `#`, so it would be read as another string): refused rather than
    /// quoted. Names the key only, never the value.
    Unusable(&'static str),
}

/// The keys in a `KEY=value` file's text.
pub fn parse_keys(s: &str) -> Result<Keys, NoKeys> {
    let get = |k: &'static str| -> Result<String, NoKeys> {
        let v = s
            .lines()
            .find_map(|l| {
                let (a, b) = l.trim().split_once('=')?;
                (a.trim() == k).then(|| b.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty())
            })
            .ok_or(NoKeys::Missing)?;
        if v.contains(|c: char| c.is_whitespace() || c == '#') {
            Err(NoKeys::Unusable(k))
        } else {
            Ok(v)
        }
    };
    match (get("NAS_SNMP_USER"), get("NAS_SNMP_AUTH")) {
        (Ok(user), Ok(auth)) => Ok(Keys { user, auth }),
        (Err(e @ NoKeys::Unusable(_)), _) | (_, Err(e @ NoKeys::Unusable(_))) => Err(e),
        _ => Err(NoKeys::Missing),
    }
}

/// The keys under the project folder `root`.
pub fn read_keys(root: &Path) -> Result<Keys, NoKeys> {
    parse_keys(&std::fs::read_to_string(root.join(KEYS)).map_err(|_| NoKeys::Missing)?)
}

/// How long a poll may run at all before it's killed (snmpget's own timeout doesn't cover the
/// host's mDNS lookup).
pub const KILL: Duration = Duration::from_secs(8);

/// A session's folder is this prefix and its process's id.
const PREFIX: &str = "scenic-snmp-";

/// Removes the session folders under `tmp` whose process is gone (a crashed lead leaves its conf,
/// with the auth key in it); returns how many. A folder of a live process, or of a name not ours,
/// stays.
fn clean_stale(tmp: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(tmp) else { return 0 };
    rd.flatten()
        .filter(|e| {
            let name = e.file_name();
            let pid = name.to_str().and_then(|n| n.strip_prefix(PREFIX)).and_then(|p| p.parse::<i32>().ok());
            pid.is_some_and(|p| {
                p > 0
                    && unsafe { libc::kill(p, 0) } != 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            }) && std::fs::remove_dir_all(e.path()).is_ok()
        })
        .count()
}

/// A session's keys in an `snmp.conf` of its own (0600, in a 0700 folder under the temp folder,
/// `SNMPCONFPATH`), so the auth key is never in a command line; removed when dropped. One per
/// lead's coordinator, made again only when the keys change. `~/.snmp` is never touched.
pub struct Session {
    keys: Keys,
    dir: std::path::PathBuf,
}

impl Session {
    pub fn new(keys: Keys) -> std::io::Result<Session> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        clean_stale(&std::env::temp_dir());
        let dir = std::env::temp_dir().join(format!("{PREFIX}{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let s = Session { keys, dir };
        let mut f =
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(s.dir.join("snmp.conf"))?;
        write!(
            f,
            "defSecurityName {}\ndefSecurityLevel authNoPriv\ndefAuthType SHA\ndefAuthPassphrase {}\n",
            s.keys.user, s.keys.auth
        )?;
        Ok(s)
    }

    /// Whether this session was made from `k`.
    pub fn is_for(&self, k: &Keys) -> bool {
        &self.keys == k
    }

    /// Asks the NAS for `oids` (their values in order, as `snmpget -Oqv` prints them).
    pub fn get(&self, oids: &[&str]) -> std::io::Result<Vec<String>> {
        use std::io::Read;
        let mut cmd = store::spawn::command("/usr/bin/snmpget");
        cmd.args(["-v3", "-Oqv", "-r", "0", "-t", &TIMEOUT.as_secs().to_string(), HOST])
            .args(oids)
            .env("SNMPCONFPATH", &self.dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = store::spawn::spawn(&mut cmd)?;
        let t = std::time::Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if t.elapsed() >= KILL {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::other(format!("snmpget: no answer in {} s", KILL.as_secs())));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let (mut out, mut err) = (String::new(), String::new());
        child.stdout.take().map(|mut o| o.read_to_string(&mut out));
        child.stderr.take().map(|mut e| e.read_to_string(&mut err));
        if !status.success() {
            // (Its stderr names no key; kept short.)
            return Err(std::io::Error::other(format!("snmpget: {}", err.lines().next().unwrap_or("failed"))));
        }
        let v: Vec<String> = out.lines().map(|l| l.trim().to_string()).collect();
        if v.len() != oids.len() {
            return Err(std::io::Error::other(format!("snmpget: {} answers for {} asked", v.len(), oids.len())));
        }
        Ok(v)
    }

    /// Each disk's busy share now (%).
    pub fn disk_busy(&self) -> std::io::Result<Vec<f64>> {
        parse_busy(&self.get(&DISK_BUSY)?)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn parse_busy(v: &[String]) -> std::io::Result<Vec<f64>> {
    v.iter()
        .map(|s| s.parse::<f64>().map_err(|_| std::io::Error::other(format!("snmpget: {s:?} isn't a number"))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_and_never_shown() {
        let k = parse_keys("OTHER=1\nNAS_SNMP_USER=u\nNAS_SNMP_AUTH=\"secret\"\n").unwrap();
        assert_eq!((k.user.as_str(), k.auth.as_str()), ("u", "secret"));
        assert!(!format!("{k:?}").contains("secret"));
        assert_eq!(parse_keys("NAS_SNMP_USER=u\n").unwrap_err(), NoKeys::Missing);
        assert_eq!(parse_keys("NAS_SNMP_USER=u\nNAS_SNMP_AUTH=\n").unwrap_err(), NoKeys::Missing);
    }

    /// A session's config file is private and gone when it's dropped.
    #[test]
    fn a_sessions_config_is_private_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let k = parse_keys("NAS_SNMP_USER=u\nNAS_SNMP_AUTH=secret\n").unwrap();
        let s = Session::new(k.clone()).unwrap();
        let f = s.dir.join("snmp.conf");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&s.dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(std::fs::read_to_string(&f).unwrap().contains("defAuthPassphrase secret"));
        assert!(s.is_for(&k));
        let dir = s.dir.clone();
        drop(s);
        assert!(!dir.exists());
    }

    /// Values net-snmp's conf couldn't carry are refused.
    #[test]
    fn keys_the_conf_format_cant_carry_are_refused() {
        let auth = NoKeys::Unusable("NAS_SNMP_AUTH");
        assert_eq!(parse_keys("NAS_SNMP_USER=u\nNAS_SNMP_AUTH=two words\n").unwrap_err(), auth);
        assert_eq!(parse_keys("NAS_SNMP_USER=u\nNAS_SNMP_AUTH=ab#cd\n").unwrap_err(), auth);
        assert_eq!(parse_keys("NAS_SNMP_AUTH=ab#cd\n").unwrap_err(), auth, "named with the user missing too");
        assert_eq!(parse_keys("NAS_SNMP_USER=a b\nNAS_SNMP_AUTH=x\n").unwrap_err(), NoKeys::Unusable("NAS_SNMP_USER"));
        assert!(parse_keys("NAS_SNMP_USER=u\nNAS_SNMP_AUTH='x-y_z'\n").is_ok());
    }

    /// A dead process's session folder is removed; a live one's and others' stay.
    #[test]
    fn a_crashs_session_folders_are_cleaned() {
        let tmp = std::env::temp_dir().join(format!("snmp-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mk = |n: String| {
            std::fs::create_dir_all(tmp.join(&n)).unwrap();
            tmp.join(n)
        };
        let live = mk(format!("{PREFIX}{}", std::process::id()));
        let other = mk("scenic-snmp-x".into());
        let mut c = store::spawn::spawn(&mut store::spawn::command("/usr/bin/true")).unwrap();
        let dead_pid = c.id();
        c.wait().unwrap();
        let dead = mk(format!("{PREFIX}{dead_pid}"));
        assert_eq!(clean_stale(&tmp), 1);
        assert!(live.exists() && other.exists() && !dead.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn busy_shares_parse() {
        assert_eq!(parse_busy(&["69".into(), "67".into(), "74".into()]).unwrap(), vec![69.0, 67.0, 74.0]);
        assert!(parse_busy(&["No Such Object".into()]).is_err());
    }
}
