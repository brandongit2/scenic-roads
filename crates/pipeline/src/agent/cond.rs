//! A Mac's conditions (docs/plan.md §8, Interruptions): the NAS (and whether it answers by its LAN
//! name: at home), the user at the keyboard, sleep; and its power, which no job waits on, for the
//! status and for whether it may lead (crate::agent::lead: a lead on battery or away is offered to
//! hand over).

use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::{Instant, SystemTime};

/// Seconds without keyboard or mouse input after which the user counts as away.
pub const AWAY_S: u64 = 300;

/// On battery, a Mac below this charge (%) isn't able to lead (crate::agent::pool's `able`: the
/// proactive offer, a takeover by itself). No job waits on the charge.
pub const BATTERY_MIN: u8 = 30;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conditions {
    /// On mains power.
    pub ac: bool,
    /// The battery's charge (%), when there's a battery.
    #[serde(default)]
    pub battery: Option<u8>,
    /// The NAS share mounted and answering.
    pub nas: bool,
    /// At home: the NAS answers by its LAN name (else the share, if mounted, is reached through
    /// Tailscale, slowly).
    #[serde(default = "yes")]
    pub home: bool,
    /// Seconds since the last keyboard or mouse input.
    pub idle_s: u64,
}

fn yes() -> bool {
    true
}

impl Conditions {
    pub fn user_active(&self) -> bool {
        self.idle_s < AWAY_S
    }
}

/// What an agent reads of the Mac it runs on: its power, its user's idleness, whether it's at
/// home, its memory. The real Mac's; or a test's, fixed, so the agents a test runs do the same
/// whichever Mac runs it, unplugged or loaded, at home or away, its owner at the keyboard or not (a
/// build Mac unplugged at 18 % failed the pool's tests, 8 Oct).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mac {
    Real,
    Fixed { ac: bool, battery: Option<u8>, idle_s: u64, home: bool, mem_gb: f64, mem_free_pct: u8 },
}

impl Mac {
    /// A test's Mac: on mains, at home, in use, 16 GB with three quarters free.
    pub const TEST: Mac = Mac::Fixed { ac: true, battery: None, idle_s: 0, home: true, mem_gb: 16.0, mem_free_pct: 75 };

    /// `power`.
    pub fn power(&self) -> (bool, Option<u8>) {
        match *self {
            Mac::Real => power(),
            Mac::Fixed { ac, battery, .. } => (ac, battery),
        }
    }

    /// `idle_seconds`.
    pub fn idle_seconds(&self) -> u64 {
        match *self {
            Mac::Real => idle_seconds(),
            Mac::Fixed { idle_s, .. } => idle_s,
        }
    }

    /// Whether the NAS answers by its LAN name (store::nas::at_home).
    pub fn at_home(&self) -> bool {
        match *self {
            Mac::Real => store::nas::at_home(),
            Mac::Fixed { home, .. } => home,
        }
    }

    /// `resources`: a fixed Mac's memory its own, its disk's free space as the agent's tests set it
    /// (super::room::disk_free), no load or NAS measured.
    pub fn resources(&self, home: &std::path::Path, root: Option<&std::path::Path>, cache_gb: Option<f64>, nas_ms: Option<u64>) -> Resources {
        match *self {
            Mac::Real => resources(home, root, cache_gb, nas_ms),
            Mac::Fixed { mem_gb, mem_free_pct, .. } => {
                Resources { mem_gb, mem_free_pct: Some(mem_free_pct), cores: 8, load1: None, disk_free_gb: super::room::disk_free(home).ok().map(|b| (b as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0), cache_gb, nas_ms, nas_free_tb: None }
            }
        }
    }
}

/// On mains power, and the battery's charge: `pmset -g ps` names the source ("Now drawing from
/// 'AC Power'") and lists the battery ("-InternalBattery-0 (id=…)	89%; charging; …"). A Mac without
/// a battery says AC too. When pmset can't be read, mains is assumed (a broken probe shouldn't stop
/// all work).
pub fn power() -> (bool, Option<u8>) {
    match Command::new("/usr/bin/pmset").args(["-g", "ps"]).output() {
        Ok(o) => parse_power(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => (true, None),
    }
}

fn parse_power(ps: &str) -> (bool, Option<u8>) {
    let ac = !ps.contains("'Battery Power'");
    let battery = ps.lines().filter(|l| l.contains("InternalBattery")).find_map(|l| {
        let pct = l.split('%').next()?;
        pct.rsplit(|c: char| !c.is_ascii_digit()).next()?.parse::<u8>().ok()
    });
    (ac, battery)
}

/// Seconds since the last keyboard or mouse input (IOHIDSystem's HIDIdleTime, in nanoseconds);
/// u64::MAX when it can't be read.
pub fn idle_seconds() -> u64 {
    let Ok(o) = Command::new("/usr/sbin/ioreg").args(["-c", "IOHIDSystem", "-d", "4", "-r", "-k", "HIDIdleTime"]).output() else {
        return u64::MAX;
    };
    parse_idle(&String::from_utf8_lossy(&o.stdout)).unwrap_or(u64::MAX)
}

fn parse_idle(ioreg: &str) -> Option<u64> {
    ioreg.lines().find_map(|l| l.split_once("\"HIDIdleTime\" = ").and_then(|(_, v)| v.trim().parse::<u64>().ok())).map(|ns| ns / 1_000_000_000)
}

/// Notices sleep: the wall clock runs on while the Mac sleeps, the monotonic clock (uptime) doesn't.
pub struct SleepWatch {
    wall: SystemTime,
    mono: Instant,
}

impl Default for SleepWatch {
    fn default() -> Self {
        SleepWatch { wall: SystemTime::now(), mono: Instant::now() }
    }
}

impl SleepWatch {
    /// Seconds slept since the last call (a few seconds of difference are noise: callers compare
    /// with a threshold).
    pub fn slept(&mut self) -> u64 {
        let (w, m) = (SystemTime::now(), Instant::now());
        let dw = w.duration_since(self.wall).unwrap_or_default();
        let dm = m.duration_since(self.mono);
        self.wall = w;
        self.mono = m;
        dw.saturating_sub(dm).as_secs()
    }
}

/// Free bytes on the volume holding `path`.
pub fn free_bytes(path: &std::path::Path) -> Option<u64> {
    store::sys::disk_free(path).ok()
}

/// This Mac's resources, for the status (the worker page's machines): its memory and the share of
/// it free now, its cores and load, its disk's free space and the caches it may drop to make room,
/// how long the NAS took to answer and the NAS's free space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Resources {
    pub mem_gb: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_free_pct: Option<u8>,
    pub cores: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free_gb: Option<f64>,
    /// Counted every ten minutes (a walk of the caches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_free_tb: Option<f64>,
}

impl Resources {
    /// The memory free now, bytes (memory_pressure's share of it; none when it can't say).
    pub fn mem_free(&self) -> Option<u64> {
        self.mem_free_pct.map(|p| (self.mem_gb * f64::from(p) / 100.0 * (1u64 << 30) as f64) as u64)
    }
}

/// This Mac's resources now (`Resources`): its disk's where `home` is, the NAS's at `root`; the
/// caches' and the NAS's answer as the agent last measured them.
pub fn resources(home: &std::path::Path, root: Option<&std::path::Path>, cache_gb: Option<f64>, nas_ms: Option<u64>) -> Resources {
    static MEM: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let mem_gb = *MEM.get_or_init(|| {
        let o = Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok();
        o.and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok()).map_or(0.0, |b| b as f64 / (1u64 << 30) as f64)
    });
    // ("System-wide memory free percentage: 63%")
    let mem_free_pct = Command::new("/usr/bin/memory_pressure").arg("-Q").output().ok().and_then(|o| {
        let s = String::from_utf8_lossy(&o.stdout).into_owned();
        s.split("free percentage:").nth(1)?.trim().trim_end_matches('%').trim().parse::<u8>().ok()
    });
    #[cfg(unix)]
    let load1 = {
        let mut load = [0f64; 3];
        // SAFETY: getloadavg fills at most the 3 doubles it's given.
        (unsafe { libc::getloadavg(load.as_mut_ptr(), 3) } >= 1).then_some((load[0] * 100.0).round() / 100.0)
    };
    #[cfg(not(unix))]
    let load1 = None;
    let gb = |b: u64| (b as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0;
    Resources {
        mem_gb: (mem_gb * 10.0).round() / 10.0,
        mem_free_pct,
        cores: std::thread::available_parallelism().map_or(0, |n| n.get()),
        load1,
        disk_free_gb: free_bytes(home).map(gb),
        cache_gb,
        nas_ms,
        nas_free_tb: root.and_then(free_bytes).map(|b| (b as f64 / (1u64 << 40) as f64 * 100.0).round() / 100.0),
    }
}

/// This Mac's name, read once per process (temporary files' names: crate::whole).
pub fn host() -> &'static str {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(host_name)
}

/// This Mac's name (for the heartbeat).
pub fn host_name() -> String {
    Command::new("/usr/sbin/scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_from_pmset() {
        let batt = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=35455075)\t90%; discharging; 3:59 remaining present: true\n";
        assert_eq!(parse_power(batt), (false, Some(90)));
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=35455075)\t7%; charging; 1:08 remaining present: true\n";
        assert_eq!(parse_power(ac), (true, Some(7)));
        // A Mac without a battery.
        assert_eq!(parse_power("Now drawing from 'AC Power'\n"), (true, None));
    }

    #[test]
    fn idle_from_ioreg() {
        let s = "+-o IOHIDSystem  <class IOHIDSystem>\n    {\n      \"HIDIdleTime\" = 12345678901\n    }\n";
        assert_eq!(parse_idle(s), Some(12));
        assert_eq!(parse_idle("nothing"), None);
    }

    #[test]
    fn sleep_watch_quiet() {
        let mut w = SleepWatch::default();
        assert!(w.slept() < 2);
    }
}
