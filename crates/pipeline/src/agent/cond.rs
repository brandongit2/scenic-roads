//! A Mac's conditions (docs/building.md §2, Interruptions): the NAS (and whether it answers by its LAN
//! name: at home), the user at the keyboard, sleep; and its power, which no job waits on, for the
//! status.

use serde::{Deserialize, Serialize};
use std::time::{Instant, SystemTime};

/// Seconds without keyboard or mouse input after which the user counts as away.
pub const AWAY_S: u64 = 300;

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
/// whichever Mac runs it, unplugged or loaded, at home or away, its owner at the keyboard or not (the
/// M4 unplugged at 18 % failed the pool's tests, 8 Oct).
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

    /// Whether the share is mounted by the NAS's LAN name (store::nas::lan_mounted); a fixed Mac's
    /// never (its `home` alone says).
    pub fn lan_mounted(&self) -> bool {
        match *self {
            Mac::Real => store::nas::lan_mounted(),
            Mac::Fixed { .. } => false,
        }
    }

    /// `load1`: a fixed Mac's none (its cores all free).
    pub fn load1(&self) -> Option<f64> {
        match *self {
            Mac::Real => load1(),
            Mac::Fixed { .. } => None,
        }
    }

    /// `resources`: a fixed Mac's memory its own, its disk's free space as the agent's tests set it
    /// (super::room::disk_free), no load or NAS measured.
    pub fn resources(
        &self,
        home: &std::path::Path,
        root: Option<&store::nasfs::NasRoot>,
        cache_gb: Option<f64>,
        nas_ms: Option<u64>,
    ) -> Resources {
        match *self {
            Mac::Real => resources(home, root, cache_gb, nas_ms),
            Mac::Fixed { mem_gb, mem_free_pct, .. } => Resources {
                mem_gb,
                mem_free_pct: Some(mem_free_pct),
                cores: 8,
                load1: None,
                disk_free_gb: super::room::disk_free(home)
                    .ok()
                    .map(|b| (b as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0),
                cache_gb,
                nas_ms,
                nas_free_tb: None,
                chip: None,
            },
        }
    }
}

/// Whether the Mac is at home: the NAS answered the probe by its LAN name (`probe`), or the share
/// answers and is mounted by that name (`lan_mount_answers`). The second keeps a Mac home whose probe fails for
/// its own reasons (the process refused the LAN, a resolver gone stale) while it works on the LAN:
/// it stayed "away" for hours with the share mounted by the LAN name and answering (task #569).
pub fn home(probe: bool, lan_mount_answers: bool) -> bool {
    probe || lan_mount_answers
}

/// On mains power, and the battery's charge: `pmset -g ps` names the source ("Now drawing from
/// 'AC Power'") and lists the battery ("-InternalBattery-0 (id=…)	89%; charging; …"). A Mac without
/// a battery says AC too. When pmset can't be read, mains is assumed (a broken probe shouldn't stop
/// all work).
pub fn power() -> (bool, Option<u8>) {
    match store::spawn::output(store::spawn::command("/usr/bin/pmset").args(["-g", "ps"])) {
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
    let Ok(o) = store::spawn::output(store::spawn::command("/usr/sbin/ioreg").args([
        "-c",
        "IOHIDSystem",
        "-d",
        "4",
        "-r",
        "-k",
        "HIDIdleTime",
    ])) else {
        return u64::MAX;
    };
    parse_idle(&String::from_utf8_lossy(&o.stdout)).unwrap_or(u64::MAX)
}

fn parse_idle(ioreg: &str) -> Option<u64> {
    ioreg
        .lines()
        .find_map(|l| l.split_once("\"HIDIdleTime\" = ").and_then(|(_, v)| v.trim().parse::<u64>().ok()))
        .map(|ns| ns / 1_000_000_000)
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

/// Free bytes on the volume holding `path`; with `SCENIC_TEST_DISK_EXTRA_MB` set (the two-Mac
/// test's alone, tools/check/pool-two-macs.sh `extra`: never the real agent's), that many MB more,
/// so a small disk image the test's agent home is on stands for a larger disk,
/// its fills landing in the image, never filling the Mac's own disk.
pub fn free_bytes(path: &std::path::Path) -> Option<u64> {
    store::sys::disk_free(path).ok().map(|f| f.saturating_add(test_disk_extra_mb() << 20))
}

/// `SCENIC_TEST_DISK_EXTRA_MB` (0 unset), at most 1 TB: said at the agent's start and in its
/// status, never silent.
pub fn test_disk_extra_mb() -> u64 {
    static EXTRA: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *EXTRA.get_or_init(|| {
        std::env::var("SCENIC_TEST_DISK_EXTRA_MB").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(1 << 20)
    })
}

/// `SCENIC_TEST_MEM_MB` when set: the memory (MB) the agent counts its Mac to have, for its memory
/// guard's limit and its asks (the two-Mac test's alone, tools/check/pool-two-macs.sh: a limit
/// squeezed so small jobs pass it). Said at its first read, never silent.
pub fn test_mem_mb() -> Option<u64> {
    static MEM: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *MEM.get_or_init(|| {
        let m = std::env::var("SCENIC_TEST_MEM_MB").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&m| m > 0);
        if let Some(m) = m {
            eprintln!("agent: WARNING: SCENIC_TEST_MEM_MB: this agent counts its Mac's memory as {m} MB (the two-Mac test's alone, never a real agent's)");
        }
        m
    })
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
    /// Its chip, short ("M4 Pro", "M1": `machdep.cpu.brand_string` less "Apple "): the build page's
    /// short name for the Mac in its lanes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chip: Option<String>,
}

impl Resources {
    /// The memory free now, bytes (memory_pressure's share of it; none when it can't say).
    pub fn mem_free(&self) -> Option<u64> {
        self.mem_free_pct.map(|p| (self.mem_gb * f64::from(p) / 100.0 * (1u64 << 30) as f64) as u64)
    }
}

/// The last minute's load average (None when it can't be read).
pub fn load1() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut load = [0f64; 3];
        // SAFETY: getloadavg fills at most the 3 doubles it's given.
        (unsafe { libc::getloadavg(load.as_mut_ptr(), 3) } >= 1).then_some((load[0] * 100.0).round() / 100.0)
    }
    #[cfg(not(unix))]
    None
}

/// This Mac's resources now (`Resources`): its disk's where `home` is, the NAS's at `root`; the
/// caches' and the NAS's answer as the agent last measured them.
pub fn resources(
    home: &std::path::Path,
    root: Option<&store::nasfs::NasRoot>,
    cache_gb: Option<f64>,
    nas_ms: Option<u64>,
) -> Resources {
    static MEM: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let mem_gb = *MEM.get_or_init(|| {
        let o = store::spawn::output(store::spawn::command("/usr/sbin/sysctl").args(["-n", "hw.memsize"])).ok();
        o.and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
            .map_or(0.0, |b| b as f64 / (1u64 << 30) as f64)
    });
    // ("System-wide memory free percentage: 63%")
    let mem_free_pct =
        store::spawn::output(store::spawn::command("/usr/bin/memory_pressure").arg("-Q")).ok().and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.split("free percentage:").nth(1)?.trim().trim_end_matches('%').trim().parse::<u8>().ok()
        });
    static CHIP: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let chip = CHIP
        .get_or_init(|| {
            let o = store::spawn::output(
                store::spawn::command("/usr/sbin/sysctl").args(["-n", "machdep.cpu.brand_string"]),
            )
            .ok()?;
            chip_short(&String::from_utf8_lossy(&o.stdout))
        })
        .clone();
    let load1 = load1();
    let gb = |b: u64| (b as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0;
    Resources {
        mem_gb: (mem_gb * 10.0).round() / 10.0,
        mem_free_pct,
        cores: std::thread::available_parallelism().map_or(0, |n| n.get()),
        load1,
        disk_free_gb: free_bytes(home).map(gb),
        cache_gb,
        nas_ms,
        // (The share's free space by its mount's statfs: tests/nas_access.rs.)
        nas_free_tb: root
            .and_then(|r| free_bytes(&r.unchecked_path()))
            .map(|b| (b as f64 / (1u64 << 40) as f64 * 100.0).round() / 100.0),
        chip,
    }
}

/// A chip's brand string, short: "Apple M4 Pro" → "M4 Pro"; None for none.
pub fn chip_short(brand: &str) -> Option<String> {
    let b = brand.trim();
    let b = b.strip_prefix("Apple ").unwrap_or(b).trim();
    (!b.is_empty()).then(|| b.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn home_when_the_probe_fails_but_the_lan_mount_answers() {
        use super::home;
        assert!(home(true, false), "the probe answered");
        assert!(home(false, true), "the probe failed, the LAN-name mount answers: still home");
        assert!(!home(false, false), "neither: away");
        // A fixed Mac has no mount: its home alone decides.
        assert!(!super::Mac::Fixed { ac: true, battery: None, idle_s: 0, home: false, mem_gb: 16.0, mem_free_pct: 75 }
            .lan_mounted());
    }

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
}

#[cfg(test)]
mod chip_tests {
    #[test]
    fn a_chips_short_name() {
        assert_eq!(super::chip_short("Apple M4 Pro\n").as_deref(), Some("M4 Pro"));
        assert_eq!(super::chip_short("Apple M1").as_deref(), Some("M1"));
        assert_eq!(super::chip_short("Intel(R) Core(TM) i7").as_deref(), Some("Intel(R) Core(TM) i7"));
        assert_eq!(super::chip_short("  "), None);
    }
}
