//! The pool's NAS and network limits (docs/pool.md §7.7, task #243), on unless the switch
//! `state/pool/nas-budget` (`SWITCH`) says `off` (on while missing). Packing lets more jobs run
//! at once, and the share takes 20 to 55 small-file creates a second: the lead's coordinator keeps
//! the pool's running jobs' NAS weights together within what the NAS was measured to give
//! (crate::coord's `pack`, `pick_every` and `pick_own`: the lead's own jobs too), as the Wikimedia
//! budget keeps the pool's requests.
//!
//! - **A job's NAS weight** is the share of the NAS it takes, in thousandths: its targets' measured
//!   rate (crate::needs::Book::nas: bytes read and written a second, files made a second, from
//!   crate::costin's `bytes`, `wrote` and `creates` over its time), each over the NAS's (`Limits`),
//!   added (one combined weight; reads and writes split only if measures show they limit apart).
//!   A job's targets run one after another: its weight is its heaviest target's.
//! - **The limits** are measured, never typed: `probe` makes a few small files and one larger in a
//!   folder of its own, reads the larger back past the cache, and removes them all. The coordinator
//!   probes every `EVERY_S`, whatever the switch says, and takes the best of the last `KEEP_S`'s probes
//!   (`Probes`): a probe run while the build loads the NAS measures what's left, so the best is the
//!   nearest to the whole.
//! - **Downloads**: a job of a fetching step (crate::agent::steps::Kind::Fetch) counts one; the
//!   switch's `downloads <n>` caps them pool-wide (no cap without it).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The switch.
pub const SWITCH: &str = "state/pool/nas-budget";
/// Where the coordinator's probes are made, under the project folder (a folder per process).
pub const PROBE_DIR: &str = "state/pool/nas-probe";
/// How often the coordinator probes (s).
pub const EVERY_S: u64 = 30 * 60;
/// How long a probe counts toward the limits (s).
pub const KEEP_S: u64 = 24 * 3600;
/// The whole NAS, in weight (thousandths).
pub const WHOLE: u32 = 1000;
/// A probe's small files (each `SMALL` bytes), and its larger file's size.
pub const SMALL_FILES: usize = 12;
pub const SMALL: usize = 4096;
pub const LARGE: usize = 16 << 20;

/// The switch as its file says: None, off; else on, with its download cap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switch {
    pub downloads: Option<usize>,
}

/// The switch at `root` (None in it: off; missing, on with no download cap); None when it can't
/// be read now.
pub fn read_switch(root: &Path) -> Option<Option<Switch>> {
    match std::fs::read_to_string(root.join(SWITCH)) {
        Ok(s) => Some(parse(&s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Some(Switch::default())),
        Err(_) => None,
    }
}

/// The switch's text: `off` alone, off; else on, with its `downloads <n>` (a line not understood
/// ignored).
pub fn parse(s: &str) -> Option<Switch> {
    if s.trim() == "off" {
        return None;
    }
    let mut w = Switch::default();
    for l in s.lines() {
        if let ["downloads", n] = l.split_whitespace().collect::<Vec<_>>().as_slice() {
            w.downloads = n.parse().ok().or(w.downloads);
        }
    }
    Some(w)
}

/// What one probe measured: bytes moved a second (written and read back, together) and files made
/// a second.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Probe {
    /// When (unix s).
    pub at: u64,
    pub bytes_s: f64,
    pub creates_s: f64,
}

/// The pool's limits now: the NAS's (a probe's), and the downloads' cap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub bytes_s: f64,
    pub creates_s: f64,
    pub downloads: Option<usize>,
}

/// A target's or job's measured NAS rate (crate::needs::Book::nas).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rate {
    pub bytes_s: f64,
    pub creates_s: f64,
}

impl Limits {
    /// The weight of a job at `r` (thousandths of the NAS; rounded up, so a measured job never
    /// weighs nothing; at most the whole, `WHOLE`). 0 while no probe measured the NAS.
    pub fn weight(&self, r: Rate) -> u32 {
        let part = |x: f64, of: f64| if of > 0.0 { x / of } else { 0.0 };
        let w = (part(r.bytes_s, self.bytes_s) + part(r.creates_s, self.creates_s)) * WHOLE as f64;
        if w > 0.0 {
            w.ceil().min(WHOLE as f64) as u32
        } else {
            0
        }
    }

    /// Whether a job of weight `w` may start beside leased jobs weighing `running` together, of
    /// which the members' (all but the lead's own) weigh `members`: within the whole, or, when no
    /// member's job weighs anything, whatever the lead's own weigh (they're always about: a job
    /// that fits only beside nothing would never go).
    pub fn fits(members: u32, running: u32, w: u32) -> bool {
        w == 0 || members == 0 || running.saturating_add(w) <= WHOLE
    }

    /// Whether a job of the lead's own of weight `w` may start beside leased jobs weighing
    /// `running` together: within the whole, or beside nothing that weighs anything (a job past
    /// the whole alone).
    pub fn fits_own(running: u32, w: u32) -> bool {
        w == 0 || running == 0 || running.saturating_add(w) <= WHOLE
    }

    /// `fits_own` for the lead's own planned job, which members' small jobs would otherwise hold
    /// back for good (each lease's weight keeps `running` above zero): it goes too while no leased
    /// job weighs the whole (`heaviest`, the most one weighs), when it weighs the whole itself, or
    /// has waited `OWN_AGE` for the budget alone (`waited`) and the lead's own leased jobs (`own`,
    /// their weight together) leave it room within the whole: aged ones go past the members' jobs
    /// alone, one at a time, never piling up, and a whole one isn't held back by the lead's own
    /// stream of smaller jobs.
    pub fn fits_own_job(running: u32, own: u32, heaviest: u32, w: u32, waited: bool) -> bool {
        Self::fits_own(running, w) || (heaviest < WHOLE && (w >= WHOLE || (waited && own.saturating_add(w) <= WHOLE)))
    }
}

/// How long the lead's own planned job waits for the NAS budget alone before it goes beside
/// leased jobs none of which weighs the whole (`Limits::fits_own_job`).
pub const OWN_AGE: std::time::Duration = std::time::Duration::from_secs(600);

/// The probes kept (the last `KEEP_S`'s), and the limits they give: the best of each.
#[derive(Clone, Debug, Default)]
pub struct Probes(pub Vec<Probe>);

impl Probes {
    pub fn add(&mut self, p: Probe) {
        self.0.retain(|q| q.at + KEEP_S > p.at);
        self.0.push(p);
    }

    /// The limits (None: nothing probed yet).
    pub fn limits(&self, downloads: Option<usize>) -> Option<Limits> {
        let best = |f: fn(&Probe) -> f64| self.0.iter().map(f).fold(0.0, f64::max);
        (!self.0.is_empty()).then(|| Limits {
            bytes_s: best(|p| p.bytes_s),
            creates_s: best(|p| p.creates_s),
            downloads,
        })
    }
}

fn host_part(me: &str) -> String {
    me.replace(['/', '.', '-'], "_")
}

/// A folder of a probe's own under `root` (`PROBE_DIR`), by `me` and this process:
/// `<me>-<pid>` (`me` with no `-` in it, so one host's folders are told from another's).
pub fn probe_dir(root: &Path, me: &str) -> PathBuf {
    root.join(PROBE_DIR).join(format!("{}-{}", host_part(me), std::process::id()))
}

/// Removes `me`'s probe folders under `root` a stopped process left (a crash mid-probe): every
/// `<me>-<pid>` but this process's; other hosts' left alone. How many were removed.
pub fn clean_stale(root: &Path, me: &str) -> usize {
    let mine = probe_dir(root, me);
    let prefix = format!("{}-", host_part(me));
    let Ok(rd) = std::fs::read_dir(root.join(PROBE_DIR)) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let pid = name.strip_prefix(&prefix);
        if pid.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            && e.path() != mine
            && std::fs::remove_dir_all(e.path()).is_ok()
        {
            n += 1;
        }
    }
    n
}

/// Probes the NAS in `dir` (made, then removed with all it holds): `SMALL_FILES` files of `SMALL`
/// bytes made (create-new, each written and closed: the share's creates), then one of `LARGE`
/// written and synced and read back past the cache. Small and short: under a second or two.
pub fn probe(dir: &Path) -> std::io::Result<Probe> {
    std::fs::create_dir_all(dir)?;
    let r = probe_in(dir);
    let gone = std::fs::remove_dir_all(dir);
    let p = r?;
    gone?;
    Ok(p)
}

fn probe_in(dir: &Path) -> std::io::Result<Probe> {
    let small = vec![7u8; SMALL];
    let t = Instant::now();
    for i in 0..SMALL_FILES {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(format!("s{i}")))?;
        f.write_all(&small)?;
    }
    let creates_s = SMALL_FILES as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let large: Vec<u8> = (0..LARGE).map(|i| (i * 31 % 251) as u8).collect();
    let p = dir.join("large");
    let t = Instant::now();
    {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&p)?;
        f.write_all(&large)?;
        f.sync_all()?;
    }
    let mut f = std::fs::File::open(&p)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: plain fcntl on a descriptor we own; failure only means the read may be cached.
        unsafe {
            libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1);
        }
    }
    let mut back = Vec::with_capacity(LARGE);
    f.read_to_end(&mut back)?;
    if back.len() != LARGE {
        return Err(std::io::Error::other(format!("read back {} of {LARGE} bytes", back.len())));
    }
    let bytes_s = (2 * LARGE) as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Ok(Probe { at, bytes_s, creates_s })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_says_on_off_and_its_download_cap() {
        assert_eq!(parse("off\n"), None);
        assert_eq!(parse(""), Some(Switch::default()));
        assert_eq!(parse("on\ndownloads 2\n"), Some(Switch { downloads: Some(2) }));
    }

    /// A weight is the job's share of the NAS's bytes and creates, added, in thousandths.
    #[test]
    fn a_weight_is_a_share_of_the_nas() {
        let l = Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None };
        assert_eq!(l.weight(Rate { bytes_s: 25e6, creates_s: 0.0 }), 250);
        assert_eq!(l.weight(Rate { bytes_s: 25e6, creates_s: 4.0 }), 350);
        assert_eq!(l.weight(Rate::default()), 0);
        assert_eq!(l.weight(Rate { bytes_s: 1.0, creates_s: 0.0 }), 1, "measured: never nothing");
        assert_eq!(l.weight(Rate { bytes_s: 900e6, creates_s: 0.0 }), WHOLE, "at most the whole");
        assert!(Limits::fits(600, 600, 400) && !Limits::fits(601, 601, 400));
        assert!(Limits::fits(0, 900, 1000), "the lead's own alone: a member's job goes");
        assert!(!Limits::fits(1, 900, 1000));
        // The lead's own planned job beside members' small jobs (task #396 follow-up): a whole one
        // goes while none leased weighs the whole; a lesser one once it has waited; none beside a
        // whole one.
        assert!(!Limits::fits_own(30, 1000) && Limits::fits_own_job(30, 0, 20, 1000, false));
        assert!(!Limits::fits_own_job(30, 0, 1000, 1000, true), "beside a whole one: never");
        assert!(!Limits::fits_own_job(300, 0, 200, 800, false) && Limits::fits_own_job(300, 0, 200, 800, true));
        // (Beside an aged one of the lead's own already past the budget: within the whole with it.)
        assert!(!Limits::fits_own_job(1100, 800, 800, 800, true), "aged ones don't pile up");
        assert!(Limits::fits_own_job(1100, 800, 800, 200, true));
        assert!(Limits::fits_own_job(1100, 300, 300, 1000, false), "a whole one beside the lead's own smaller ones");
        assert!(Limits::fits(999, 999, 0));
        assert!(Limits::fits_own(600, 400) && !Limits::fits_own(601, 400));
        assert!(Limits::fits_own(0, 1000) && !Limits::fits_own(1, 1000), "the lead's own: within the whole, or alone");
    }

    /// The limits are the best of the last day's probes.
    #[test]
    fn the_limits_are_the_best_of_the_days_probes() {
        let mut p = Probes::default();
        assert!(p.limits(None).is_none());
        p.add(Probe { at: 1000, bytes_s: 80e6, creates_s: 30.0 });
        p.add(Probe { at: 2000, bytes_s: 50e6, creates_s: 45.0 });
        assert_eq!(p.limits(Some(2)), Some(Limits { bytes_s: 80e6, creates_s: 45.0, downloads: Some(2) }));
        p.add(Probe { at: 1000 + KEEP_S, bytes_s: 10e6, creates_s: 5.0 });
        assert_eq!(p.limits(None).unwrap().bytes_s, 50e6, "a day old: let go");
    }

    /// A probe measures and leaves nothing behind.
    #[test]
    fn a_probe_measures_and_removes_its_files() {
        let d = tempfile::tempdir().unwrap();
        let dir = probe_dir(d.path(), "m1.local");
        let p = probe(&dir).unwrap();
        assert!(p.bytes_s > 0.0 && p.creates_s > 0.0, "{p:?}");
        assert!(!dir.exists());
    }

    /// At start, this host's folders a crash left go; this process's and other hosts' stay.
    #[test]
    fn a_crashs_probe_folders_of_this_host_are_cleaned() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join(PROBE_DIR);
        for f in ["m4_local-1", "m4_local-22", "m1_local-3", "m4_local-x"] {
            std::fs::create_dir_all(base.join(f)).unwrap();
            std::fs::write(base.join(f).join("s0"), b"x").unwrap();
        }
        let own = probe_dir(d.path(), "m4.local");
        std::fs::create_dir_all(&own).unwrap();
        assert_eq!(clean_stale(d.path(), "m4.local"), 2);
        assert!(own.exists() && base.join("m1_local-3").exists() && base.join("m4_local-x").exists());
        assert!(!base.join("m4_local-1").exists() && !base.join("m4_local-22").exists());
    }
}
