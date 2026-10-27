//! The forecasts' archive (docs/pool.md §7.9, task #621): every forecast the lead makes, kept for
//! the accuracy work (`scenic forecast-check`, crate::agent::fccheck): its schedule (each target
//! placed, by lane, from and until when) and what shaped it (each step's time a target, the lanes and
//! their speeds, each Mac's slots, room, conditions and what held it back, the NAS budget, the
//! forecast's code version and the app).
//!
//! - One file a day, `state/coord/forecasts/<day>.jsonl.zst`: a forecast an `Entry` a line, the
//!   lines appended in batches, each batch a zstd frame of its own (the frames read back as one
//!   stream). Handed over by the lead's pool step alone, written on a thread of its own
//!   (`Archiver`): the agent's loop and the pool's lock only hand it the entry (a bounded channel,
//!   never waiting: full, the entry is dropped); its appends go through `nasread::write`'s deadline.
//!   A lead that has just handed over may still append its last batch beside the new lead's, and an
//!   append over SMB isn't atomic: a reader skips a damaged frame and resumes at the next frame's
//!   magic number (`entries_of`).
//!   A batch goes once `BATCH` entries wait or the oldest has waited `FLUSH`: a few appends an hour,
//!   never a file a forecast. A batch not yet written when the agent stops is lost (`FLUSH` at most).
//! - A forecast whose schedule and inputs are those of the last one written (`Entry::key`: its
//!   times in `GRAIN`-long buckets, so two times either side of a bucket's edge differ: a repeat is
//!   sometimes written, never a changed forecast skipped; the measured NAS rates and the Macs' passing conditions left out) is not
//!   written again.
//! - The daily `gc` job (lead only) packs the days the history's window no longer holds
//!   (histarchive::cutoff) into one tar a month, `state/coord/forecasts-archive/<YYYY-MM>.tar`
//!   (members `<day>.jsonl.zst`, merged as the history's are: histarchive::merge_into_tar), then
//!   removes their files. Kept for good: never removed (23 Oct 2026, ~6,000 targets placed a
//!   forecast: ~13 KB a forecast compressed, ~4 MB a day at one forecast in 5 minutes, 19 MB at
//!   one a minute).

use super::forecast::{self, Forecast, Held, MacRoom, Placed, Record};
use super::{histarchive, nasread};
use crate::pool::journal;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const DIR: &str = "state/coord/forecasts";
pub const ARCHIVE: &str = "state/coord/forecasts-archive";
/// The record's format (`Entry::v`).
pub const FORMAT: u32 = 1;
/// A batch is appended once this many entries wait…
const BATCH: usize = 30;
/// …or the oldest has waited this long.
const FLUSH: Duration = Duration::from_secs(600);
/// Entries kept while the NAS can't be written (the oldest dropped past it, said).
const PENDING_MAX: usize = 300;
/// The times' grain in `Entry::key`: a forecast whose every time falls in the same `GRAIN`-long
/// bucket as the last one written's is the same forecast.
pub const GRAIN: u64 = 120;
/// Entries the channel holds for the thread (a forecast a minute at most: one is plenty; full while
/// the thread waits on the NAS, the entry is dropped rather than held).
const CHANNEL: usize = 4;
/// zstd's level for a batch.
const LEVEL: i32 = 9;

/// A lane of the forecast (`forecast::Machine`): a slot of a Mac (the lead's first).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub name: String,
    pub mac: String,
    pub speed: f64,
    pub measured: bool,
    /// Seconds until it's free (its job under way), and until its slot opens.
    pub busy_s: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub from_s: f64,
    /// Its job under way that no target placed is: step and targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<(String, usize)>,
    /// The targets placed on it, in time order: [step (an index into `Entry::steps`), target, gap,
    /// time] (s: its start less the end of the one before it, `Entry::at` for the first; its end
    /// less its start). Lanes run their work back to back, so the gaps are mostly 0 and the times a
    /// step's few: a third of the size compressed of unix times (`Entry::placed` reads them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<(usize, String, i64, u64)>,
}

/// A Mac as the forecast saw it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mac {
    /// Its lanes (slots) in the run.
    pub slots: usize,
    /// Its room when the scheduler had no ask of it (`Input::macs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<MacRoom>,
    /// What held it back over the last day (`Input::held`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<Held>,
    /// Its last ask's capacity (crate::coord::Capacity, a few of its fields: memory and cores free,
    /// its limits, in use, on power, battery, NAS): its conditions. None: no ask recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<serde_json::Value>,
}

/// An archived forecast: a line of a day's file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// `FORMAT`; the forecast's code version (forecast::VERSION); the app that made it.
    pub v: u32,
    pub code: u32,
    pub app: String,
    /// When it was made (unix s), and its answer: the finish and its range, or why none.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    pub measured: f64,
    pub lanes: Vec<Lane>,
    pub macs: BTreeMap<String, Mac>,
    /// Each step's time a target at the lead's pace (s).
    pub each_s: BTreeMap<String, f64>,
    /// Each step's pace against its costs the run took (`Input::step_pace`; code 2 on).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub step_pace: BTreeMap<String, f64>,
    /// The NAS budget the scheduler held to (crate::budget::nas::Limits), when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas: Option<serde_json::Value>,
    /// The steps the lanes' `jobs` name, by index.
    pub steps: Vec<String>,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// The capacity's fields kept as a Mac's conditions (`Mac::cond`).
const COND: [&str; 11] = [
    "limit_mb",
    "build_mb",
    "mem_mb",
    "cores",
    "cores_free",
    "kept_cores",
    "disk_mb",
    "in_use",
    "ac",
    "battery",
    "nas",
];

impl Entry {
    /// The entry of forecast `f`, made of `r` by app `app`.
    pub fn of(r: &Record, f: &Forecast, app: &str) -> Entry {
        let inp = &r.input;
        let lanes: Vec<Lane> = inp
            .machines
            .iter()
            .map(|m| Lane {
                name: m.name.clone(),
                mac: m.mac.clone().unwrap_or_else(|| m.name.clone()),
                speed: round(m.speed, 1000.0),
                measured: m.measured,
                busy_s: m.busy_s.round(),
                from_s: m.from_s.round(),
                running: m.running.clone(),
                jobs: Vec::new(),
            })
            .collect();
        let mut macs: BTreeMap<String, Mac> = BTreeMap::new();
        for l in &lanes {
            macs.entry(l.mac.clone()).or_default().slots += 1;
        }
        for (name, m) in macs.iter_mut() {
            m.room = inp.macs.get(name).copied();
            m.held = inp.held.get(name).cloned();
            m.cond = inp.sched.as_ref().and_then(|s| s.workers.iter().find(|w| &w.ask.worker == name)).and_then(|w| {
                let cap = serde_json::to_value(w.ask.cap.as_ref()?).ok()?;
                Some(COND.iter().filter_map(|k| Some((k.to_string(), cap.get(*k)?.clone()))).collect())
            });
        }
        let mut e = Entry {
            v: FORMAT,
            code: forecast::VERSION,
            app: app.to_string(),
            at: f.at,
            done_at: f.done_at,
            range: f.range,
            why: f.why.clone(),
            partial: f.partial,
            measured: f.measured,
            lanes,
            macs,
            each_s: r.each_s.iter().map(|(k, v)| (k.clone(), round(*v, 10.0))).collect(),
            step_pace: inp.step_pace.clone(),
            nas: inp.sched.as_ref().and_then(|s| s.nas).and_then(|l| serde_json::to_value(l).ok()),
            steps: Vec::new(),
        };
        e.set_placed(&f.placed);
        e
    }

    /// `placed` as its lanes' `jobs` (and `steps`).
    pub fn set_placed(&mut self, placed: &[Placed]) {
        let mut ix: BTreeMap<&str, usize> = BTreeMap::new();
        let mut by_lane: BTreeMap<usize, Vec<&Placed>> = BTreeMap::new();
        for p in placed {
            by_lane.entry(p.lane).or_default().push(p);
            if !ix.contains_key(p.step.as_str()) {
                ix.insert(&p.step, self.steps.len());
                self.steps.push(p.step.clone());
            }
        }
        for (k, mut ps) in by_lane {
            ps.sort_by_key(|p| (p.from, p.until));
            let Some(lane) = self.lanes.get_mut(k) else { continue };
            let mut end = self.at;
            lane.jobs = ps
                .into_iter()
                .map(|p| {
                    let j = (
                        ix[p.step.as_str()],
                        p.target.clone(),
                        p.from as i64 - end as i64,
                        p.until.saturating_sub(p.from),
                    );
                    end = p.until;
                    j
                })
                .collect();
        }
    }

    /// Every target placed (its lanes' `jobs` read back), lane by lane, each in time order.
    pub fn placed(&self) -> Vec<Placed> {
        let mut out = Vec::new();
        for (k, l) in self.lanes.iter().enumerate() {
            let mut end = self.at;
            for (s, target, gap, secs) in &l.jobs {
                let from = (end as i64 + gap).max(0) as u64;
                end = from + secs;
                out.push(Placed {
                    lane: k,
                    step: self.steps.get(*s).cloned().unwrap_or_default(),
                    target: target.clone(),
                    from,
                    until: end,
                });
            }
        }
        out
    }

    /// What makes it another forecast than the last written (`GRAIN`): its schedule and finish in
    /// unix seconds to the grain, its lanes and Macs (but their passing conditions and time left),
    /// each step's time, its code. Not its time made, nor the NAS's measured rates.
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let g = |t: u64| t / GRAIN;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let jobs: Vec<(usize, String, String, u64, u64)> =
            self.placed().into_iter().map(|p| (p.lane, p.step, p.target, g(p.from), g(p.until))).collect();
        let lanes: Vec<(&str, &str, String, bool, &Option<(String, usize)>)> = self
            .lanes
            .iter()
            .map(|l| (l.name.as_str(), l.mac.as_str(), format!("{:.2}", l.speed), l.measured, &l.running))
            .collect();
        let macs: Vec<(&str, usize, String, String)> = self
            .macs
            .iter()
            .map(|(n, m)| {
                (
                    n.as_str(),
                    m.slots,
                    serde_json::to_string(&m.room).unwrap_or_default(),
                    serde_json::to_string(&m.held).unwrap_or_default(),
                )
            })
            .collect();
        let each: Vec<(&str, String)> = self.each_s.iter().map(|(k, v)| (k.as_str(), format!("{v:.1}"))).collect();
        (self.code, self.done_at.map(|t| t / GRAIN), &self.why, jobs, lanes, macs, each).hash(&mut h);
        h.finish()
    }

    /// Its day (`journal::day`): the file it goes to.
    pub fn day(&self) -> Option<String> {
        journal::day(self.at)
    }
}

fn round(x: f64, by: f64) -> f64 {
    if x.is_finite() {
        (x * by).round() / by
    } else {
        0.0
    }
}

/// A batch as a zstd frame, its window long enough to hold the whole batch (a forecast's line is
/// ~170 KB at live scale, the next much the same).
pub fn compress(b: &[u8]) -> io::Result<Vec<u8>> {
    use std::io::Write;
    let mut z = zstd::stream::write::Encoder::new(Vec::new(), LEVEL)?;
    z.long_distance_matching(true)?;
    z.window_log(WINDOW_LOG)?;
    z.write_all(b)?;
    z.finish()
}

/// `compress`'s window: 64 MB (a reader's default limit is 128 MB).
const WINDOW_LOG: u32 = 26;

/// A day's file.
pub fn day_path(root: &store::nasfs::NasRoot, day: &str) -> store::nasfs::NasPath {
    root.join(DIR).join(format!("{day}.jsonl.zst"))
}

/// Appends `entries` (one day's) to its day's file: one zstd frame, one write, within `nasread`'s
/// deadline.
pub fn append(root: &store::nasfs::NasRoot, day: &str, entries: &[Entry]) -> Result<()> {
    let mut b = Vec::new();
    for e in entries {
        b.extend(serde_json::to_vec(e)?);
        b.push(b'\n');
    }
    let z = compress(&b)?;
    let p = day_path(root, day);
    let q = p.clone();
    nasread::write(&p, move || {
        use std::io::Write;
        q.parent().context("a day's folder")?.create_dir_all()?;
        let mut f = q.open_with(std::fs::OpenOptions::new().create(true).append(true))?;
        f.write_all(&z).with_context(|| format!("append to {}", q.display()))?;
        Ok(())
    })
}

/// The lead's archiver: a thread that takes each forecast's entry (`send`, never waiting), passes
/// over those like the last written (`Entry::key`) and appends the rest in batches (`BATCH`,
/// `FLUSH`).
pub struct Archiver {
    tx: mpsc::SyncSender<Entry>,
}

impl Archiver {
    /// Its thread started, writing under `root`.
    pub fn start(root: store::nasfs::NasRoot) -> io::Result<Archiver> {
        let (tx, rx) = mpsc::sync_channel::<Entry>(CHANNEL);
        std::thread::Builder::new()
            .name("forecast-archive".into())
            .spawn(move || Batcher::new(root.long_retry()).run(rx))?;
        Ok(Archiver { tx })
    }

    /// `e` handed to the thread, never waiting: dropped (said) while the thread is behind.
    pub fn send(&self, e: Entry) {
        if let Err(mpsc::TrySendError::Full(_)) = self.tx.try_send(e) {
            eprintln!("agent: the forecasts' archive is behind (the NAS): a forecast not archived");
        }
    }
}

/// The archiver's thread's state.
struct Batcher {
    root: store::nasfs::NasRoot,
    last: Option<u64>,
    pending: Vec<Entry>,
    since: Option<Instant>,
}

impl Batcher {
    fn new(root: store::nasfs::NasRoot) -> Batcher {
        Batcher { root, last: None, pending: Vec::new(), since: None }
    }

    fn run(mut self, rx: mpsc::Receiver<Entry>) {
        loop {
            let wait = self.since.map_or(FLUSH, |s| FLUSH.saturating_sub(s.elapsed()));
            match rx.recv_timeout(wait) {
                Ok(e) => self.take(e),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.flush();
                    return;
                }
            }
            if self.pending.len() >= BATCH || self.since.is_some_and(|s| s.elapsed() >= FLUSH) {
                self.flush();
            }
        }
    }

    /// `e` kept unless it's the last one's forecast again.
    fn take(&mut self, e: Entry) {
        let k = e.key();
        if self.last == Some(k) {
            return;
        }
        self.last = Some(k);
        self.since.get_or_insert_with(Instant::now);
        self.pending.push(e);
        if self.pending.len() > PENDING_MAX {
            let n = self.pending.len() - PENDING_MAX;
            self.pending.drain(..n);
            eprintln!("agent: the forecasts' archive dropped {n} forecast(s): the NAS isn't taking them");
        }
    }

    /// The entries waiting appended, a day's at a time; those not written kept for the next.
    fn flush(&mut self) {
        let mut by_day: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for e in std::mem::take(&mut self.pending) {
            by_day.entry(e.day().unwrap_or_default()).or_default().push(e);
        }
        for (day, es) in by_day {
            if day.is_empty() {
                continue;
            }
            if let Err(e) = append(&self.root, &day, &es) {
                eprintln!("agent: the forecasts' archive, {day}: {e:#} (kept for the next batch)");
                self.pending.extend(es);
            }
        }
        self.since = (!self.pending.is_empty()).then(Instant::now);
    }
}

/// zstd's frame magic number, little-endian on disk.
const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The entries of a file's bytes (zstd frames of lines), frame by frame: a frame that doesn't read
/// (cut short by a stopped write, or two Macs' appends crossed over SMB) is skipped, said, and
/// reading resumes at the next frame's magic number after its start.
pub fn entries_of(b: &[u8], what: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let rest = &b[at..];
        let frame = zstd::zstd_safe::find_frame_compressed_size(rest).ok().and_then(|n| {
            let mut d = zstd::stream::read::Decoder::new(&rest[..n]).ok()?.single_frame();
            d.window_log_max(WINDOW_LOG).ok()?;
            let mut v = Vec::new();
            io::Read::read_to_end(&mut d, &mut v).ok()?;
            Some((n, v))
        });
        match frame {
            Some((n, v)) => {
                for l in v.split(|c| *c == b'\n').filter(|l| !l.iter().all(u8::is_ascii_whitespace)) {
                    match serde_json::from_slice::<Entry>(l) {
                        Ok(e) => out.push(e),
                        Err(e) => eprintln!("{what}: a line not read: {e}"),
                    }
                }
                at += n.max(1);
            }
            None => {
                let next = b[at + 1..].windows(4).position(|w| w == MAGIC).map(|k| at + 1 + k);
                eprintln!(
                    "{what}: a damaged frame at byte {at} skipped{}",
                    if next.is_some() { "" } else { " (the file's end)" }
                );
                match next {
                    Some(k) => at = k,
                    None => break,
                }
            }
        }
    }
    out
}

/// The archived forecasts made from `from` until `to` (unix s): the days' files and the months'
/// tars, by time made.
pub fn read(root: &store::nasfs::NasRoot, from: u64, to: u64) -> Vec<Entry> {
    let mut out = Vec::new();
    let days: Vec<String> = (from / 86_400..=to / 86_400).filter_map(|d| journal::day(d * 86_400)).collect();
    let mut months: Vec<&str> = days.iter().map(|d| &d[..7]).collect();
    months.dedup();
    for m in months {
        let tar = root.join(ARCHIVE).join(format!("{m}.tar"));
        if let Ok(members) = tar.read().map_err(anyhow::Error::from).and_then(|t| histarchive::list_tar(&t)) {
            for (name, b) in members {
                if days.iter().any(|d| name == format!("{d}.jsonl.zst")) {
                    out.extend(entries_of(&b, &name));
                }
            }
        }
    }
    for d in &days {
        if let Ok(b) = day_path(root, d).read() {
            out.extend(entries_of(&b, d));
        }
    }
    out.retain(|e| e.at >= from && e.at < to);
    out.sort_by_key(|e| e.at);
    // (A day both packed and still there: a run stopped before its removal.)
    out.dedup_by(|a, b| a.at == b.at && a.key() == b.key());
    out
}

/// Packs the days before the history's window (histarchive::cutoff) into their months' tars and
/// removes their files (each only if its size is still the one packed). `between` runs before each
/// month (a safe point).
pub fn pack(root: &store::nasfs::NasRoot, now: u64, dry_run: bool, between: &dyn Fn()) -> Result<histarchive::Report> {
    let mut rep = histarchive::Report::default();
    let Some(cut) = histarchive::cutoff(now) else { return Ok(rep) };
    let dir = root.join(DIR);
    let listed = match histarchive::files(&dir) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(rep),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    rep.listed += 1;
    let mut months: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    for (name, size) in listed {
        let Some(day) = name.strip_suffix(".jsonl.zst") else { continue };
        if journal::is_day(day) && *day < *cut.as_str() {
            months.entry(day[..7].to_string()).or_default().push((name, size));
        }
    }
    let arch = root.join(ARCHIVE);
    if !dry_run && !months.is_empty() {
        histarchive::sweep_tmp(&arch, &mut rep);
    }
    for (month, files) in months {
        between();
        if let Err(e) = pack_month(&dir, &arch, &month, &files, dry_run, &mut rep) {
            rep.warnings.push(format!("forecasts of {month} not archived: {e:#}"));
        }
    }
    Ok(rep)
}

fn pack_month(
    dir: &store::nasfs::NasPath,
    arch: &store::nasfs::NasPath,
    month: &str,
    files: &[(String, u64)],
    dry_run: bool,
    rep: &mut histarchive::Report,
) -> Result<()> {
    let mut members = Vec::new();
    for (name, size) in files {
        let b = nasread::read(&dir.join(name)).with_context(|| format!("read {name}"))?;
        rep.stats += 1;
        anyhow::ensure!(b.len() as u64 == *size, "{name} read {} of {size} bytes", b.len());
        members.push((name.clone(), b));
    }
    if dry_run {
        return Ok(());
    }
    histarchive::merge_into_tar(&arch.join(format!("{month}.tar")), members)?;
    for (name, size) in files {
        let p = dir.join(name);
        let q = p.clone();
        let now = nasread::with(&p, move || q.metadata().map(|m| m.len()))?;
        rep.stats += 1;
        if now != *size {
            rep.warnings.push(format!("{name} grew while it was packed (kept, packed again next run)"));
            continue;
        }
        p.remove_file().with_context(|| format!("remove {name}"))?;
        rep.files_removed += 1;
        rep.days_archived += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const NOW: u64 = 1_791_600_000; // 2026-10-10

    fn entry(at: u64, until: u64) -> Entry {
        Entry {
            v: FORMAT,
            code: forecast::VERSION,
            app: "test".into(),
            at,
            lanes: vec![Lane {
                name: "a".into(),
                mac: "a".into(),
                speed: 1.0,
                measured: true,
                jobs: vec![(0, "6/1/2".into(), 0, until)],
                ..Default::default()
            }],
            steps: vec!["unit".into()],
            ..Default::default()
        }
    }

    #[test]
    fn batches_append_as_frames_and_read_back_as_one_stream() {
        let r = tempfile::tempdir().unwrap();
        let day = journal::day(NOW).unwrap();
        append(&store::nasfs::NasRoot::local_for_test(r.path()), &day, &[entry(NOW, 600), entry(NOW + 60, 900)])
            .unwrap();
        append(&store::nasfs::NasRoot::local_for_test(r.path()), &day, &[entry(NOW + 120, 1200)]).unwrap();
        let got = read(&store::nasfs::NasRoot::local_for_test(r.path()), NOW - 10, NOW + 3600);
        assert_eq!(got.iter().map(|e| e.at).collect::<Vec<_>>(), [NOW, NOW + 60, NOW + 120]);
        assert_eq!(got[2], entry(NOW + 120, 1200));
        // A frame cut short: the frames before it read.
        let p = day_path(&store::nasfs::NasRoot::local_for_test(r.path()), &day);
        let b = p.read().unwrap();
        p.write(&b[..b.len() - 5]).unwrap();
        assert_eq!(read(&store::nasfs::NasRoot::local_for_test(r.path()), NOW - 10, NOW + 3600).len(), 2);
    }

    #[test]
    fn a_damaged_frame_is_skipped_and_the_frames_after_it_read() {
        let r = tempfile::tempdir().unwrap();
        let day = journal::day(NOW).unwrap();
        append(&store::nasfs::NasRoot::local_for_test(r.path()), &day, &[entry(NOW, 600)]).unwrap();
        let p = day_path(&store::nasfs::NasRoot::local_for_test(r.path()), &day);
        let one = p.read().unwrap().len();
        append(&store::nasfs::NasRoot::local_for_test(r.path()), &day, &[entry(NOW + 60, 900)]).unwrap();
        append(&store::nasfs::NasRoot::local_for_test(r.path()), &day, &[entry(NOW + 120, 1200)]).unwrap();
        let mut b = p.read().unwrap();
        // The second frame's middle overwritten (two appends crossed): the first and third read.
        let mid = one + (b.len() - one) / 4;
        b[mid..mid + 8].copy_from_slice(&[0xAA; 8]);
        // And junk before a frame's start.
        let mut junk = vec![1, 2, 3];
        junk.extend(&b);
        p.write(&junk).unwrap();
        let got: Vec<u64> =
            read(&store::nasfs::NasRoot::local_for_test(r.path()), NOW - 10, NOW + 3600).iter().map(|e| e.at).collect();
        assert_eq!(got, [NOW, NOW + 120]);
    }

    #[test]
    fn the_same_forecast_again_is_not_kept() {
        let r = tempfile::tempdir().unwrap();
        let mut b = Batcher::new(store::nasfs::NasRoot::local_for_test(r.path().to_path_buf()));
        b.take(entry(NOW, 6000));
        // A minute on, the same target ending at the same time: the same forecast.
        b.take(entry(NOW + 60, 5940));
        assert_eq!(b.pending.len(), 1);
        b.take(entry(NOW + 120, 9000));
        assert_eq!(b.pending.len(), 2);
        b.flush();
        assert!(b.pending.is_empty() && b.since.is_none());
        assert_eq!(read(&store::nasfs::NasRoot::local_for_test(r.path()), NOW, NOW + 3600).len(), 2);
    }

    #[test]
    fn old_days_packed_by_month_kept_for_good_recent_kept_as_files() {
        let r = tempfile::tempdir().unwrap();
        let d = |back: u64| journal::day(NOW - back * 86_400).unwrap();
        for back in [20, 9, 8, 0] {
            append(&store::nasfs::NasRoot::local_for_test(r.path()), &d(back), &[entry(NOW - back * 86_400, 60)])
                .unwrap();
        }
        let rep = pack(&store::nasfs::NasRoot::local_for_test(r.path()), NOW, false, &|| {}).unwrap();
        assert_eq!(rep.days_archived, 3, "{rep:?}");
        assert!(rep.warnings.is_empty());
        assert!(day_path(&store::nasfs::NasRoot::local_for_test(r.path()), &d(0)).exists());
        assert!(!day_path(&store::nasfs::NasRoot::local_for_test(r.path()), &d(8)).exists());
        let sep = histarchive::list_tar(&std::fs::read(r.path().join(ARCHIVE).join("2026-09.tar")).unwrap()).unwrap();
        assert_eq!(sep.len(), 1);
        // All four read back, packed or not; again: nothing to do.
        assert_eq!(read(&store::nasfs::NasRoot::local_for_test(r.path()), NOW - 21 * 86_400, NOW + 60).len(), 4);
        assert_eq!(
            pack(&store::nasfs::NasRoot::local_for_test(r.path()), NOW, false, &|| {}).unwrap().days_archived,
            0
        );
        // A late append to a packed day: merged into its month's tar next run.
        append(&store::nasfs::NasRoot::local_for_test(r.path()), &d(8), &[entry(NOW - 8 * 86_400 + 60, 120)]).unwrap();
        pack(&store::nasfs::NasRoot::local_for_test(r.path()), NOW, false, &|| {}).unwrap();
        assert_eq!(read(&store::nasfs::NasRoot::local_for_test(r.path()), NOW - 8 * 86_400, NOW - 7 * 86_400).len(), 2);
    }

    #[test]
    fn lanes_jobs_read_back_as_placed_gaps_and_overlaps_kept() {
        let mut e = Entry { at: NOW, lanes: vec![Lane::default(), Lane::default()], ..Default::default() };
        let p = |lane: usize, step: &str, t: &str, from: u64, until: u64| Placed {
            lane,
            step: step.into(),
            target: t.into(),
            from: NOW + from,
            until: NOW + until,
        };
        let placed = vec![
            p(0, "unit", "a", 0, 100),
            p(0, "unit", "b", 100, 250),
            p(0, "pack", "c", 400, 410),
            p(1, "pack", "d", 50, 70),
            p(1, "pack", "e", 60, 90),
        ];
        e.set_placed(&placed);
        assert_eq!(e.steps, ["unit", "pack"]);
        assert_eq!(e.lanes[0].jobs[2], (1, "c".to_string(), 150, 10));
        assert_eq!(e.lanes[1].jobs[1], (1, "e".to_string(), -10, 30));
        assert_eq!(e.placed(), placed);
    }

    #[test]
    fn an_entry_is_made_of_the_forecast_and_its_record() {
        let inp = forecast::Input {
            now: NOW,
            before: vec![],
            regions: Default::default(),
            costs: Default::default(),
            cadence: Default::default(),
            blind: None,
            chains: vec![],
            after: vec![],
            idle: vec![],
            machines: vec![
                forecast::Machine { name: "m".into(), mac: Some("m".into()), speed: 1.0, ..Default::default() },
                forecast::Machine { name: "m #2".into(), mac: Some("m".into()), speed: 0.95123, ..Default::default() },
            ],
            running: Default::default(),
            macs: [("m".to_string(), MacRoom { mem_mb: 1000, cores: 8 })].into(),
            sched: None,
            release: vec![],
            held: Default::default(),
            pace: None,
            step_pace: Default::default(),
            paused: false,
            took: Default::default(),
        };
        let rec = Record { input: inp, each_s: [("unit".to_string(), 191.23)].into(), waiting: Default::default() };
        let f = Forecast {
            at: NOW,
            placed: vec![Placed {
                lane: 1,
                step: "unit".into(),
                target: "6/1/2".into(),
                from: NOW + 10,
                until: NOW + 200,
            }],
            ..Default::default()
        };
        let e = Entry::of(&rec, &f, "app");
        assert_eq!(e.lanes[1].jobs, [(0, "6/1/2".to_string(), 10, 190)]);
        assert_eq!(e.placed(), f.placed);
        assert_eq!(e.macs["m"].slots, 2);
        assert_eq!(e.macs["m"].room, Some(MacRoom { mem_mb: 1000, cores: 8 }));
        assert_eq!(e.lanes[1].speed, 0.951);
        assert_eq!(e.each_s["unit"], 191.2);
    }

    /// Recorded forecasts (SCENIC_FORECAST_RECORD: forecast-input.json files, ':' between, in the
    /// order made) as entries: their sizes, the ones kept (`Entry::key`), and those kept as one
    /// batch's frame; with SCENIC_FCARCHIVE_ROOT, appended under that root. For measuring at live
    /// scale (unset: nothing).
    #[test]
    fn recorded_size() {
        let Ok(ps) = std::env::var("SCENIC_FORECAST_RECORD") else { return };
        let mut b = Batcher::new(store::nasfs::NasRoot::local_for_test(PathBuf::new()));
        let (mut n, mut raw, mut alone) = (0, 0, 0);
        for p in ps.split(':') {
            let r = Record::from_bytes(&std::fs::read(p).unwrap()).unwrap();
            let e = Entry::of(&r, &r.forecast(), "recorded");
            let one = serde_json::to_vec(&e).unwrap();
            (n, raw, alone) = (n + 1, raw + one.len(), alone + compress(&one).unwrap().len());
            eprintln!(
                "{p}: at {}, jobs {}, {} B, {} B zstd",
                e.at,
                e.placed().len(),
                one.len(),
                compress(&one).unwrap().len()
            );
            b.take(e);
        }
        let mut batch = Vec::new();
        for e in &b.pending {
            batch.extend(serde_json::to_vec(e).unwrap());
            batch.push(b'\n');
        }
        eprintln!(
            "{n} forecasts, {raw} B, {alone} B compressed alone; {} kept, one batch's frame {} B",
            b.pending.len(),
            compress(&batch).unwrap().len()
        );
        if let Ok(root) = std::env::var("SCENIC_FCARCHIVE_ROOT") {
            b.root = store::nasfs::NasRoot::local_for_test(PathBuf::from(root));
            b.flush();
            assert!(b.pending.is_empty());
        }
    }
}
