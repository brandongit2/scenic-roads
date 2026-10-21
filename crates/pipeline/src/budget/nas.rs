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
//!   added (one combined weight; reads and writes split only if measures show they limit apart),
//!   with its metadata work beside them (task #451): the folder entries it listed, the files it
//!   statted and deleted a second (crate::costin's `listed`, `stats`, `deletes`), each over the
//!   NAS's rate for it. A clean-up's walk reads and writes next to nothing yet keeps the share busy.
//!   A job's targets run one after another: its weight is its heaviest target's.
//! - **The limits** are measured, never typed: `probe` makes small files and larger ones in a
//!   folder of its own, `STREAMS` streams at once (the NAS's capacity together, task #534), reads
//!   the larger back past the cache, lists a kept folder, stats its files, deletes the small ones
//!   one by one, and removes the rest. The coordinator probes every `EVERY_S`, whatever the switch
//!   says, and takes the best of the last `KEEP_S`'s probes (`Probes`): a probe run while the build
//!   loads the NAS measures what's left, so the best is the nearest to the whole.
//! - **The closed loop** (`Loop`, task #534): every 10 s the coordinator reads the NAS's own disks'
//!   busy shares (crate::budget::snmp) and scales the limits by a gain kept under the owner's
//!   target (`Target`, `scenic nas-target`, by default `DEFAULT_TARGET_PCT`), whatever loads the
//!   NAS, the build or not.
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
/// A probe's streams at once (task #534, 21 Oct: one stream measured 5 creates/s and 25 MB/s
/// where four together made 41 creates/s and eight 57 MB/s).
pub const STREAMS: usize = 4;
/// Each stream's small files (each `SMALL` bytes), and its larger file's size.
pub const SMALL_FILES: usize = 12;
pub const SMALL: usize = 4096;
pub const LARGE: usize = 16 << 20;

/// The owner's load target for the NAS's disks, pool-wide (`TARGET_FILE`, `scenic nas-target`):
/// the busiest disk's share busy (%) the budget grants up to (`Loop`).
pub const TARGET_FILE: &str = "state/pool/nas-target.json";
/// The target while none is set: 85 % (headroom kept: #451's stall came at a NAS held busy).
pub const DEFAULT_TARGET_PCT: u32 = 85;
/// The targets that may be set (%): never 100, never so low the pool stops.
pub const TARGET_RANGE: std::ops::RangeInclusive<u32> = 30..=95;

/// The target as set: its share (%), who set it and when (unix s).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Target {
    pub pct: u32,
    pub by: String,
    pub at: u64,
}

/// The target as set under the project folder `root` (None: none set, or unreadable: the default).
pub fn read_target(root: &Path) -> Option<Target> {
    let t: Target = serde_json::from_slice(&std::fs::read(root.join(TARGET_FILE)).ok()?).ok()?;
    TARGET_RANGE.contains(&t.pct).then_some(t)
}

/// The target's share (%): the set one's, else `DEFAULT_TARGET_PCT`.
pub fn target_pct(t: Option<&Target>) -> u32 {
    t.map_or(DEFAULT_TARGET_PCT, |t| t.pct)
}

/// Sets the target under `root` (None: cleared, the default again).
pub fn set_target(root: &Path, pct: Option<u32>, by: &str) -> anyhow::Result<Option<Target>> {
    let p = root.join(TARGET_FILE);
    let Some(pct) = pct else {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => return Ok(None),
        }
    };
    anyhow::ensure!(
        TARGET_RANGE.contains(&pct),
        "the NAS target: {}–{} %, not {pct}",
        TARGET_RANGE.start(),
        TARGET_RANGE.end()
    );
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let t = Target { pct, by: by.to_string(), at: now };
    crate::whole::write(&p, &serde_json::to_vec(&t)?)?;
    Ok(Some(t))
}

/// The closed loop on the NAS's own disk load (task #534): the probed limits scaled by `gain`,
/// lowered while the busiest disk (smoothed) is over the target, raised back toward 1 once it's
/// `BAND` under it. Scaled limits make every job weigh more, the running ones too, so a NAS
/// over its target takes no new job that weighs anything until the load falls (the alone rules
/// stand: a job beside nothing that weighs anything still goes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loop {
    pub gain: f64,
    /// The busiest disk's busy share, smoothed (%; None: not read yet).
    pub load: Option<f64>,
}

/// The loop's gain's floor, its steps down and up per reading, the smoothing's weight of a new
/// reading, and the band under the target the load must fall below before the gain rises.
pub const GAIN_MIN: f64 = 0.25;
pub const GAIN_DOWN: f64 = 0.9;
pub const GAIN_UP: f64 = 1.05;
pub const SMOOTH: f64 = 0.3;
pub const BAND: f64 = 10.0;

impl Default for Loop {
    fn default() -> Self {
        Loop { gain: 1.0, load: None }
    }
}

impl Loop {
    /// One reading of the disks' busy shares (%) against the target `pct`.
    pub fn read(&mut self, busy: &[f64], pct: u32) {
        let Some(now) = busy.iter().copied().filter(|b| b.is_finite()).reduce(f64::max) else { return };
        let load = self.load.map_or(now, |l| l + SMOOTH * (now - l));
        self.load = Some(load);
        if load > pct as f64 {
            self.gain = (self.gain * GAIN_DOWN).max(GAIN_MIN);
        } else if load < pct as f64 - BAND {
            self.gain = (self.gain * GAIN_UP).min(1.0);
        }
    }

    /// No reading to be had (no keys, the NAS not answering): the probe's limits alone.
    pub fn lost(&mut self) {
        *self = Loop::default();
    }
}

impl Limits {
    /// The limits scaled by the loop's gain (the downloads' cap as it was).
    pub fn scaled(self, g: f64) -> Limits {
        Limits {
            bytes_s: self.bytes_s * g,
            creates_s: self.creates_s * g,
            listed_s: self.listed_s * g,
            stats_s: self.stats_s * g,
            deletes_s: self.deletes_s * g,
            downloads: self.downloads,
        }
    }
}

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

/// What one probe measured: bytes moved a second (written and read back, together), files made a
/// second, and the metadata operations a second: folder entries listed, files statted, files
/// deleted (0 in a probe from before them: not measured, so not weighed).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Probe {
    /// When (unix s).
    pub at: u64,
    pub bytes_s: f64,
    pub creates_s: f64,
    #[serde(default)]
    pub listed_s: f64,
    #[serde(default)]
    pub stats_s: f64,
    #[serde(default)]
    pub deletes_s: f64,
}

/// The pool's limits now: the NAS's (a probe's), and the downloads' cap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub bytes_s: f64,
    pub creates_s: f64,
    /// The metadata operations a second (0: not probed, not weighed).
    pub listed_s: f64,
    pub stats_s: f64,
    pub deletes_s: f64,
    pub downloads: Option<usize>,
}

/// A target's or job's measured NAS rate (crate::needs::Book::nas): bytes and files made, and its
/// metadata work, a second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rate {
    pub bytes_s: f64,
    pub creates_s: f64,
    pub listed_s: f64,
    pub stats_s: f64,
    pub deletes_s: f64,
}

impl Limits {
    /// The weight of a job at `r` (thousandths of the NAS; rounded up, so a measured job never
    /// weighs nothing; at most the whole, `WHOLE`). 0 while no probe measured the NAS.
    pub fn weight(&self, r: Rate) -> u32 {
        let part = |x: f64, of: f64| if of > 0.0 { x / of } else { 0.0 };
        let w = (part(r.bytes_s, self.bytes_s)
            + part(r.creates_s, self.creates_s)
            + part(r.listed_s, self.listed_s)
            + part(r.stats_s, self.stats_s)
            + part(r.deletes_s, self.deletes_s))
            * WHOLE as f64;
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

    /// `fits` for a member's job, which would starve when it weighs the whole beside two members'
    /// jobs or more: it goes too once it weighs the whole, has waited `OWN_AGE` for the budget
    /// alone (`waited`), and no leased job weighs the whole (`heaviest`, the most one weighs).
    pub fn fits_job(members: u32, running: u32, heaviest: u32, w: u32, waited: bool) -> bool {
        Self::fits(members, running, w) || (w >= WHOLE && waited && heaviest < WHOLE)
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

/// The steps that wait for a quiet NAS (task #451): never urgent, they start only beside nothing
/// that weighs anything, or within the whole by their own measured weight, never by an aged wait
/// (`fits_quiet`), never hold room for themselves, and yield at a safe point to a job held back
/// for the budget `OWN_AGE` that their going would let start (crate::coord's `nas_yield`).
pub const QUIET_STEPS: [&str; 1] = ["gc"];

/// How long a quiet step's job is held back for a quiet NAS before it goes as an aged job of the
/// lead's own (`fits_own_job`, its room held): a pool that stays busy never starves it for good.
pub const QUIET_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

impl Limits {
    /// Whether a job of a quiet step (`QUIET_STEPS`) of weight `w` may start beside leased jobs
    /// weighing `running` together: beside nothing that weighs anything, or, its weight measured
    /// (`w` > 0), within the whole with them. Unmeasured, it goes only alone.
    pub fn fits_quiet(running: u32, w: u32) -> bool {
        running == 0 || (w > 0 && running.saturating_add(w) <= WHOLE)
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
            listed_s: best(|p| p.listed_s),
            stats_s: best(|p| p.stats_s),
            deletes_s: best(|p| p.deletes_s),
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

/// Probes the NAS in `dir` (made, then removed with all it holds): in each of `STREAMS` streams at
/// once, `SMALL_FILES` files of `SMALL` bytes made (create-new, each written and closed: the
/// share's creates), then in each one of `LARGE` written and synced and read back past the cache;
/// then the kept folder listed (`meta_dir`) and its files statted, and the small files deleted one
/// by one. Short: a few seconds.
pub fn probe(dir: &Path) -> std::io::Result<Probe> {
    std::fs::create_dir_all(dir)?;
    let r = probe_in(dir);
    let gone = std::fs::remove_dir_all(dir);
    let p = r?;
    gone?;
    Ok(p)
}

fn probe_in(dir: &Path) -> std::io::Result<Probe> {
    // (Each part `STREAMS` streams at once: the NAS's capacity together, not one stream's.)
    let t = Instant::now();
    in_streams(|k| {
        let small = vec![7u8; SMALL];
        for i in 0..SMALL_FILES {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(format!("s{k}_{i}")))?;
            f.write_all(&small)?;
        }
        Ok(())
    })?;
    let creates_s = (STREAMS * SMALL_FILES) as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let t = Instant::now();
    in_streams(|k| large_round(&dir.join(format!("large{k}"))))?;
    let bytes_s = (STREAMS * 2 * LARGE) as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let (listed_s, stats_s, deletes_s) = probe_meta(dir)?;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Ok(Probe { at, bytes_s, creates_s, listed_s, stats_s, deletes_s })
}

/// Runs `f(k)` for each stream `k` of `STREAMS` at once: the first error, if any.
fn in_streams(f: impl Fn(usize) -> std::io::Result<()> + Sync) -> std::io::Result<()> {
    std::thread::scope(|s| {
        let f = &f;
        let hs: Vec<_> = (0..STREAMS).map(|k| s.spawn(move || f(k))).collect();
        hs.into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(std::io::Error::other("a probe stream panicked"))))
            .collect()
    })
}

/// One stream's larger file at `p`: `LARGE` bytes written and synced, read back past the cache.
fn large_round(p: &Path) -> std::io::Result<()> {
    let large: Vec<u8> = (0..LARGE).map(|i| (i * 31 % 251) as u8).collect();
    {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(p)?;
        f.write_all(&large)?;
        f.sync_all()?;
    }
    let mut f = std::fs::File::open(p)?;
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
    Ok(())
}

/// The metadata part of a probe in `dir` (its small files `s<k>_<i>` there): entries listed, files
/// statted and files deleted a second (one stream: a clean-up's walk is one).
fn probe_meta(dir: &Path) -> std::io::Result<(f64, f64, f64)> {
    let rate = |n: usize, t: Instant| n as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let (listed_s, stats_s) = probe_listing(&meta_dir(dir))?;
    let t = Instant::now();
    for k in 0..STREAMS {
        for i in 0..SMALL_FILES {
            std::fs::remove_file(dir.join(format!("s{k}_{i}")))?;
        }
    }
    Ok((listed_s, stats_s, rate(STREAMS * SMALL_FILES, t)))
}

/// The folder a probe in `dir` lists and stats (`<host>-meta` beside `<host>-<pid>`): kept between
/// probes, never written by the probe that measures it, so its listing and its files' attributes
/// come from the NAS, not this Mac's SMB cache of what it just wrote (task #451 review).
pub fn meta_dir(dir: &Path) -> PathBuf {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let host = name.rsplit_once('-').map_or(name.as_str(), |(h, _)| h);
    dir.with_file_name(format!("{host}-meta"))
}

/// Files in the kept folder (`meta_dir`), and how long since it was last written before it's
/// measured (the client's caches of it long gone).
pub const META_FILES: usize = 48;
pub const META_SETTLE: std::time::Duration = std::time::Duration::from_secs(300);

/// Entries listed and files statted a second in the kept folder `m`: (0, 0), not measured, when
/// it's missing (made now, for the next probe), incomplete (made again) or written within
/// `META_SETTLE`.
fn probe_listing(m: &Path) -> std::io::Result<(f64, f64)> {
    let names: Vec<String> = (0..META_FILES).map(|i| format!("m{i}")).collect();
    let settled = std::fs::metadata(m.join(&names[META_FILES - 1]))
        .and_then(|md| md.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|e| e >= META_SETTLE));
    if !settled {
        if !m.join(&names[META_FILES - 1]).exists() {
            std::fs::create_dir_all(m)?;
            for n in &names {
                std::fs::write(m.join(n), b"m")?;
            }
        }
        return Ok((0.0, 0.0));
    }
    let rate = |n: usize, t: Instant| n as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let t = Instant::now();
    let mut n = 0;
    for e in std::fs::read_dir(m)? {
        e?;
        n += 1;
    }
    let listed_s = rate(n, t);
    let t = Instant::now();
    for f in &names {
        std::fs::metadata(m.join(f))?;
    }
    Ok((listed_s, rate(META_FILES, t)))
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
        let l = Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        assert_eq!(l.weight(Rate { bytes_s: 25e6, creates_s: 0.0, ..Default::default() }), 250);
        assert_eq!(l.weight(Rate { bytes_s: 25e6, creates_s: 4.0, ..Default::default() }), 350);
        assert_eq!(l.weight(Rate::default()), 0);
        assert_eq!(l.weight(Rate { bytes_s: 1.0, creates_s: 0.0, ..Default::default() }), 1, "measured: never nothing");
        assert_eq!(l.weight(Rate { bytes_s: 900e6, creates_s: 0.0, ..Default::default() }), WHOLE, "at most the whole");
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
        // A quiet step (the clean-up): alone, or measured and within the whole; never aged past it.
        assert!(Limits::fits_quiet(0, 0) && Limits::fits_quiet(0, 1000));
        assert!(!Limits::fits_quiet(1, 0), "unmeasured: alone only");
        assert!(Limits::fits_quiet(700, 300) && !Limits::fits_quiet(701, 300));
        assert!(Limits::fits_own(600, 400) && !Limits::fits_own(601, 400));
        assert!(Limits::fits_own(0, 1000) && !Limits::fits_own(1, 1000), "the lead's own: within the whole, or alone");
    }

    /// The loop lowers the gain while the busiest disk is over the target, to its floor at most,
    /// holds it within the band, and raises it back to 1 below it; lost, the probes' limits alone.
    #[test]
    fn the_loop_keeps_the_nas_under_its_target() {
        let mut l = Loop::default();
        l.read(&[60.0, 95.0, 70.0], 85);
        assert_eq!((l.load, l.gain), (Some(95.0), GAIN_DOWN), "the busiest disk counts");
        for _ in 0..100 {
            l.read(&[99.0], 85);
        }
        assert_eq!(l.gain, GAIN_MIN, "never below its floor");
        let mut held = l;
        held.load = Some(80.0);
        held.read(&[80.0], 85);
        assert_eq!(held.gain, GAIN_MIN, "within the band: held");
        for _ in 0..200 {
            l.read(&[40.0], 85);
        }
        assert_eq!(l.gain, 1.0, "never above the probes' limits");
        l.lost();
        assert_eq!(l, Loop::default());
        let lim = Limits { bytes_s: 100e6, creates_s: 40.0, downloads: Some(2), ..Default::default() };
        let r = Rate { bytes_s: 25e6, ..Default::default() };
        assert_eq!(lim.scaled(0.5).weight(r), 500, "a lowered gain: every job weighs more");
        assert_eq!(lim.scaled(0.5).downloads, Some(2));
    }

    /// The owner's target: set, read, bounded and cleared under the project folder.
    #[test]
    fn the_target_is_set_read_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        assert_eq!(target_pct(read_target(d.path()).as_ref()), DEFAULT_TARGET_PCT);
        assert!(set_target(d.path(), Some(100), "t").is_err(), "never 100 %");
        let t = set_target(d.path(), Some(70), "t").unwrap();
        assert_eq!(read_target(d.path()), t);
        assert_eq!(target_pct(t.as_ref()), 70);
        assert_eq!(set_target(d.path(), None, "t").unwrap(), None);
        assert_eq!(read_target(d.path()), None);
        assert_eq!(set_target(d.path(), None, "t").unwrap(), None, "cleared twice: fine");
    }

    /// The limits are the best of the last day's probes.
    #[test]
    fn the_limits_are_the_best_of_the_days_probes() {
        let mut p = Probes::default();
        assert!(p.limits(None).is_none());
        p.add(Probe { at: 1000, bytes_s: 80e6, creates_s: 30.0, ..Default::default() });
        p.add(Probe { at: 2000, bytes_s: 50e6, creates_s: 45.0, ..Default::default() });
        assert_eq!(
            p.limits(Some(2)),
            Some(Limits { bytes_s: 80e6, creates_s: 45.0, downloads: Some(2), ..Default::default() })
        );
        p.add(Probe { at: 1000 + KEEP_S, bytes_s: 10e6, creates_s: 5.0, ..Default::default() });
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
        // Its listing and stats measured only in the kept folder, settled: none the first time
        // (the folder made), then once its files are old.
        assert_eq!((p.listed_s, p.stats_s), (0.0, 0.0), "{p:?}");
        assert!(p.deletes_s > 0.0);
        let m = meta_dir(&dir);
        assert!(m.join(format!("m{}", META_FILES - 1)).exists());
        let old = std::time::SystemTime::now() - META_SETTLE * 2;
        for i in 0..META_FILES {
            let f = std::fs::File::options().write(true).open(m.join(format!("m{i}"))).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
        }
        let p = probe(&dir).unwrap();
        assert!(p.listed_s > 0.0 && p.stats_s > 0.0, "{p:?}");
        assert!(!dir.exists() && m.exists());
        let probe_m = probe_dir(d.path(), "m1.local");
        assert_eq!(meta_dir(&probe_m).file_name().unwrap(), "m1_local-meta");
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
