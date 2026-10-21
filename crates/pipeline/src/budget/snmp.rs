//! The Synology's own figures over SNMPv3 (docs/pool.md §7.7, task #534): the one reader of them,
//! for the NAS budget's closed loop (crate::budget::nas::Loop) and the build page's NAS figures
//! (task #517). Polled by the lead alone. The keys (`NAS_SNMP_USER`, `NAS_SNMP_AUTH`) are read
//! from the project's `inputs/keys.env` (git-ignored copies in each Mac's `data/keys.env`), passed
//! to `/usr/bin/snmpget` and never logged or printed.

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

/// The keys in a `KEY=value` file's text (None: either missing).
pub fn parse_keys(s: &str) -> Option<Keys> {
    let get = |k: &str| {
        s.lines().find_map(|l| {
            let (a, b) = l.trim().split_once('=')?;
            (a.trim() == k).then(|| b.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty())
        })
    };
    Some(Keys { user: get("NAS_SNMP_USER")?, auth: get("NAS_SNMP_AUTH")? })
}

/// The keys under the project folder `root` (None: no file, or no keys in it).
pub fn read_keys(root: &Path) -> Option<Keys> {
    parse_keys(&std::fs::read_to_string(root.join(KEYS)).ok()?)
}

/// Asks the NAS for `oids` (their values in order, as `snmpget -Oqv` prints them).
pub fn get(keys: &Keys, oids: &[&str]) -> std::io::Result<Vec<String>> {
    let mut cmd = store::spawn::command("/usr/bin/snmpget");
    cmd.args(["-v3", "-l", "authNoPriv", "-a", "SHA", "-Oqv", "-r", "0"])
        .args(["-t", &TIMEOUT.as_secs().to_string()])
        .args(["-u", &keys.user, "-A", &keys.auth, HOST])
        .args(oids);
    let out = store::spawn::output(&mut cmd)?;
    if !out.status.success() {
        // (Its stderr names no key; kept short.)
        let e = String::from_utf8_lossy(&out.stderr);
        return Err(std::io::Error::other(format!("snmpget: {}", e.lines().next().unwrap_or("failed"))));
    }
    let v: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).collect();
    if v.len() != oids.len() {
        return Err(std::io::Error::other(format!("snmpget: {} answers for {} asked", v.len(), oids.len())));
    }
    Ok(v)
}

/// Each disk's busy share now (%).
pub fn disk_busy(keys: &Keys) -> std::io::Result<Vec<f64>> {
    parse_busy(&get(keys, &DISK_BUSY)?)
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
        assert!(parse_keys("NAS_SNMP_USER=u\n").is_none());
        assert!(parse_keys("NAS_SNMP_USER=u\nNAS_SNMP_AUTH=\n").is_none());
    }

    #[test]
    fn busy_shares_parse() {
        assert_eq!(parse_busy(&["69".into(), "67".into(), "74".into()]).unwrap(), vec![69.0, 67.0, 74.0]);
        assert!(parse_busy(&["No Such Object".into()]).is_err());
    }
}
