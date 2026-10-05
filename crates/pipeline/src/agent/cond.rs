//! The build Mac's conditions (docs/plan.md §8, Interruptions): power (mains, or the battery's
//! charge), the NAS, the user at the keyboard, and sleep.

use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::{Instant, SystemTime};

/// Seconds without keyboard or mouse input after which the user counts as away.
pub const AWAY_S: u64 = 300;

/// On battery, CPU work goes on down to this charge (%), then waits for mains power.
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
