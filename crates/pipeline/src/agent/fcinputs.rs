//! The forecast inputs' archive (docs/pool.md §7.9, task #933): the lead's forecast inputs
//! (`forecast::Record`, the forecast's whole input) kept for the backtest (`scenic forecast-check
//! --replay`, crate::agent::fccheck), by term and time. Its own write path: before it, the backtest
//! read the terms' `forecast-input.json`, which the lead writes only while a member reads the
//! page's files on the NAS (`Coordinator::page_on_nas`), and none has since 31 Oct 2026.
//!
//! - **What:** an input at most every `EVERY_S` (the backtest's shortest horizon,
//!   `fccheck::HORIZONS`), the first the lead's archive thread is handed after it: a replay costs
//!   seconds an input, so a minute's inputs would be more than a backtest can rerun.
//! - **Where:** `state/coord/forecast-inputs/<day>.jsonl.zst`, one file a day (UTC), written by
//!   the forecasts' archive's thread (crate::agent::fcarchive::Archiver: never the pool's lock nor
//!   a coordinator's handler). A line an input, `{term, at, record}`; the lines appended in zstd
//!   frames (`fcarchive::encoder`: long-distance matching, so an input is stored mostly as what
//!   changed since the one before it in its frame), one append a frame: once the inputs waiting
//!   hold `FRAME_RAW` of JSON (the frame's window) or the oldest has waited `FLUSH`. A frame that
//!   fails is kept and tried again after `FLUSH` (its oldest dropped, said, past `FRAME_RAW`); one
//!   waiting when the agent stops is lost (`FLUSH` at most), as `fcarchive`'s batch is.
//! - **Kept:** `KEEP_S`, the span the backtest reads by default (`fccheck::SPAN_S`): the daily
//!   `gc` job removes the days before it (`prune`).

use super::forecast::Record;
use super::{fcarchive, fccheck, histarchive, nasread};
use crate::pool::journal;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DIR: &str = "state/coord/forecast-inputs";
/// An input archived at most this often (s): the backtest's shortest horizon, so each of its
/// horizons has inputs made within it.
pub const EVERY_S: u64 = fccheck::HORIZONS[0].0;
/// A frame is appended once the inputs waiting hold this much JSON (its window: each input can
/// refer to all those before it in the frame)…
const FRAME_RAW: u64 = 1 << fcarchive::WINDOW_LOG;
/// …or the oldest has waited this long (a failed frame tried again after it too).
const FLUSH: Duration = Duration::from_secs(4 * EVERY_S);
/// The days kept (s back from now): the backtest's default span.
pub const KEEP_S: u64 = fccheck::SPAN_S;

/// A forecast's input as the lead's pool step hands it over: its term, its time (the forecast's)
/// and its bytes as `Record::bytes` made them.
pub struct Given {
    pub term: u64,
    pub at: u64,
    pub bytes: Vec<u8>,
}

/// The inputs waiting for their frame, on the forecasts' archive's thread.
pub struct Batch {
    root: store::nasfs::NasRoot,
    /// The last input taken's time.
    last: Option<u64>,
    /// Those waiting, with their JSON's length.
    pending: Vec<(Given, u64)>,
    since: Option<Instant>,
    failed: Option<Instant>,
    /// `FLUSH` (a test's shorter).
    flush_after: Duration,
}

impl Batch {
    pub fn new(root: store::nasfs::NasRoot) -> Batch {
        Batch { root, last: None, pending: Vec::new(), since: None, failed: None, flush_after: FLUSH }
    }

    /// `g` kept unless one was taken within `EVERY_S` before it (or its bytes don't read).
    pub fn take(&mut self, g: Given) {
        if self.last.is_some_and(|l| g.at < l + EVERY_S) {
            return;
        }
        let raw = match json_len(&g.bytes) {
            Ok(n) => n,
            Err(e) => return eprintln!("agent: the forecast's input at {} not archived: {e}", g.at),
        };
        self.last = Some(g.at);
        self.since.get_or_insert_with(Instant::now);
        self.pending.push((g, raw));
        if self.failed.is_some() {
            let mut n = 0;
            while self.pending.len() > 1 && self.raw() > FRAME_RAW {
                self.pending.remove(0);
                n += 1;
            }
            if n > 0 {
                eprintln!("agent: the forecast inputs' archive dropped {n} input(s): the NAS isn't taking them");
            }
        }
    }

    fn raw(&self) -> u64 {
        self.pending.iter().map(|p| p.1).sum()
    }

    /// The time until it's due (`FLUSH` with none waiting).
    pub fn wait(&self) -> Duration {
        let from = self.since.max(self.failed);
        from.map_or(self.flush_after, |t| self.flush_after.saturating_sub(t.elapsed()))
    }

    /// Whether its frame is to be written now.
    pub fn due(&self) -> bool {
        !self.pending.is_empty()
            && self.failed.is_none_or(|t| t.elapsed() >= self.flush_after)
            && (self.raw() >= FRAME_RAW || self.since.is_some_and(|t| t.elapsed() >= self.flush_after))
    }

    /// The inputs waiting appended, a frame a day's; those not written kept for the next.
    pub fn flush(&mut self) {
        let mut by_day: BTreeMap<String, Vec<(Given, u64)>> = BTreeMap::new();
        for p in std::mem::take(&mut self.pending) {
            by_day.entry(journal::day(p.0.at).unwrap_or_default()).or_default().push(p);
        }
        self.failed = None;
        for (day, ps) in by_day {
            if day.is_empty() {
                continue;
            }
            let given: Vec<&Given> = ps.iter().map(|p| &p.0).collect();
            if let Err(e) = frame(&given)
                .map_err(anyhow::Error::from)
                .and_then(|z| fcarchive::append_frame(day_path(&self.root, &day), z))
            {
                eprintln!("agent: the forecast inputs' archive, {day}: {e:#} (kept for the next frame)");
                self.pending.extend(ps);
                self.failed = Some(Instant::now());
            }
        }
        self.since = (!self.pending.is_empty()).then(Instant::now);
    }
}

/// zstd's frame magic number.
const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The length of the JSON a `Record::bytes` holds (compressed or not).
fn json_len(b: &[u8]) -> io::Result<u64> {
    if !b.starts_with(&MAGIC) {
        return Ok(b.len() as u64);
    }
    let mut n = Counter(0);
    zstd::stream::copy_decode(b, &mut n)?;
    Ok(n.0)
}

struct Counter(u64);

impl Write for Counter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0 += b.len() as u64;
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `inputs` as one frame of lines, `{"term":…,"at":…,"record":<its JSON>}` each.
pub fn frame(inputs: &[&Given]) -> io::Result<Vec<u8>> {
    let mut z = fcarchive::encoder()?;
    for g in inputs {
        write!(z, "{{\"term\":{},\"at\":{},\"record\":", g.term, g.at)?;
        if g.bytes.starts_with(&MAGIC) {
            zstd::stream::copy_decode(&g.bytes[..], &mut z)?;
        } else {
            z.write_all(&g.bytes)?;
        }
        z.write_all(b"}\n")?;
    }
    z.finish()
}

/// A day's file.
pub fn day_path(root: &store::nasfs::NasRoot, day: &str) -> store::nasfs::NasPath {
    root.join(DIR).join(format!("{day}.jsonl.zst"))
}

/// An input found for the backtest: its term, its time, and
/// where its record is read from when it's replayed (`record`), so a span's records are never all
/// in memory at once.
#[derive(Clone)]
pub struct Input {
    pub term: u64,
    pub at: u64,
    src: Src,
}

#[derive(Clone)]
enum Src {
    /// The `k`th line of a frame (as written).
    Line(Arc<Vec<u8>>, usize),
    /// A record's bytes (`Record::bytes`).
    Bytes(Arc<Vec<u8>>),
}

/// A line's term and time, its record skipped.
#[derive(Deserialize)]
struct Head {
    term: u64,
    at: u64,
}

#[derive(Deserialize)]
struct Line {
    record: Record<'static>,
}

/// A record's time, the rest skipped.
#[derive(Deserialize)]
struct RecordHead {
    input: NowHead,
}

#[derive(Deserialize)]
struct NowHead {
    now: u64,
}

impl Input {
    /// The input of a record's bytes (`Record::bytes`), its time read from it.
    pub fn of_bytes(term: u64, b: Vec<u8>) -> Result<Input> {
        let at = if b.starts_with(&MAGIC) {
            serde_json::from_slice::<RecordHead>(&zstd::decode_all(&b[..])?)?
        } else {
            serde_json::from_slice::<RecordHead>(&b)?
        }
        .input
        .now;
        Ok(Input { term, at, src: Src::Bytes(Arc::new(b)) })
    }

    /// Its record, read.
    pub fn record(&self) -> Result<Record<'static>> {
        match &self.src {
            Src::Bytes(b) => Record::from_bytes(b),
            Src::Line(frame, k) => {
                let v = fcarchive::decode(frame)?;
                let l = fcarchive::lines(&v).nth(*k).context("the frame's line")?;
                Ok(serde_json::from_slice::<Line>(l)?.record)
            }
        }
    }

    /// Whether it's read from the archive (not a term's file).
    pub fn archived(&self) -> bool {
        matches!(self.src, Src::Line(..))
    }
}

/// The archived inputs made from `from` until `to` (unix s), by time, one a time: a frame tried
/// again after its write passed `nasread`'s deadline may be there twice (the first write landed
/// after all), and the first read is kept. A line that doesn't read is skipped, said (a damaged
/// frame: `fcarchive::frames_of`).
pub fn read(root: &store::nasfs::NasRoot, from: u64, to: u64) -> Vec<Input> {
    let mut out = Vec::new();
    for d in (from / 86_400..=to / 86_400).filter_map(|d| journal::day(d * 86_400)) {
        let Ok(b) = day_path(root, &d).read() else { continue };
        fcarchive::frames_of(&b, &d, |frame, v| {
            let frame = Arc::new(frame.to_vec());
            for (k, l) in fcarchive::lines(&v).enumerate() {
                match serde_json::from_slice::<Head>(l) {
                    Ok(h) if (from..to).contains(&h.at) => {
                        out.push(Input { term: h.term, at: h.at, src: Src::Line(frame.clone(), k) })
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("{d}: an input not read: {e}"),
                }
            }
        });
    }
    out.sort_by_key(|i| i.at);
    out.dedup_by_key(|i| i.at);
    out
}

/// Removes the days before `KEEP_S` back from `now` (`dry_run`: counts them).
pub fn prune(root: &store::nasfs::NasRoot, now: u64, dry_run: bool) -> Result<histarchive::Report> {
    let mut rep = histarchive::Report::default();
    let Some(cut) = journal::day(now.saturating_sub(KEEP_S)) else { return Ok(rep) };
    let dir = root.join(DIR);
    let listed = match histarchive::files(&dir) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(rep),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    rep.listed += 1;
    for (name, _) in listed {
        let Some(day) = name.strip_suffix(".jsonl.zst") else { continue };
        if !journal::is_day(day) || *day >= *cut.as_str() {
            continue;
        }
        if !dry_run {
            let p = dir.join(&name);
            let q = p.clone();
            nasread::with(&p, move || q.remove_file()).with_context(|| format!("remove {name}"))?;
        }
        rep.files_removed += 1;
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::forecast::{Cadence, Input as FcInput, Machine};

    const NOW: u64 = 1_791_600_000; // 2026-10-10

    fn rec(now: u64, n: usize) -> Record<'static> {
        Record {
            input: FcInput {
                now,
                before: Vec::new(),
                drops: Vec::new(),
                regions: std::borrow::Cow::Owned(Vec::new()),
                costs: Default::default(),
                cadence: Cadence::default(),
                blind: None,
                chains: Vec::new(),
                after: Vec::new(),
                idle: Vec::new(),
                machines: (0..n).map(|i| Machine { name: format!("m{i}"), speed: 1.0, ..Default::default() }).collect(),
                running: BTreeMap::new(),
                macs: BTreeMap::new(),
                sched: None,
                pace: None,
                step_pace: Default::default(),
                unknown: Vec::new(),
                paused: false,
                along: Default::default(),
            },
            each_s: [("unit".to_string(), now as f64)].into(),
            waiting: BTreeMap::new(),
        }
    }

    fn given(term: u64, r: &Record) -> Given {
        Given { term, at: r.input.now, bytes: r.bytes().unwrap() }
    }

    fn root(d: &tempfile::TempDir) -> store::nasfs::NasRoot {
        store::nasfs::NasRoot::local_for_test(d.path())
    }

    #[test]
    fn inputs_are_archived_every_so_often_and_read_back_by_term_and_time() {
        let d = tempfile::tempdir().unwrap();
        let mut b = Batch::new(root(&d));
        // A forecast a minute for an hour: one input each `EVERY_S`.
        for k in 0..60 {
            b.take(given(7 + k / 30, &rec(NOW + k * 60, 3)));
        }
        assert_eq!(b.pending.len(), (3600 / EVERY_S) as usize);
        assert!(!b.due(), "neither a window's JSON nor `FLUSH` waited");
        b.flush();
        assert!(b.pending.is_empty() && b.since.is_none());
        let got = read(&root(&d), NOW, NOW + 3600);
        let ats: Vec<u64> = got.iter().map(|i| i.at).collect();
        assert_eq!(ats, (0..3600 / EVERY_S).map(|k| NOW + k * EVERY_S).collect::<Vec<_>>());
        assert_eq!(got.iter().map(|i| i.term).collect::<Vec<_>>(), [7, 7, 8, 8]);
        assert!(got.iter().all(Input::archived));
        assert_eq!(got[2].record().unwrap(), rec(NOW + 2 * EVERY_S, 3));
        // The span's edges: from inclusive, to exclusive.
        assert_eq!(read(&root(&d), NOW + 1, NOW + 2 * EVERY_S).len(), 1);
    }

    #[test]
    fn a_frame_and_its_input_compressed_or_not_read_back_the_same() {
        let d = tempfile::tempdir().unwrap();
        let day = journal::day(NOW).unwrap();
        let big = rec(NOW + 60, 30_000);
        let gs = [given(1, &rec(NOW, 2)), given(1, &big)];
        assert!(gs[1].bytes.starts_with(&MAGIC), "over forecast::ZSTD_OVER: compressed");
        for g in &gs {
            assert!(json_len(&g.bytes).unwrap() > 0);
        }
        fcarchive::append_frame(day_path(&root(&d), &day), frame(&[&gs[0], &gs[1]]).unwrap()).unwrap();
        fcarchive::append_frame(day_path(&root(&d), &day), frame(&[&given(2, &rec(NOW + 120, 1))]).unwrap()).unwrap();
        // The second frame again (a retry whose first write landed): read once.
        fcarchive::append_frame(day_path(&root(&d), &day), frame(&[&given(2, &rec(NOW + 120, 1))]).unwrap()).unwrap();
        let got = read(&root(&d), NOW, NOW + 600);
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].record().unwrap(), big);
        assert_eq!(got[2].term, 2);
        // A term file's bytes, compressed or not.
        let i = Input::of_bytes(0, big.bytes().unwrap()).unwrap();
        assert_eq!((i.at, i.archived()), (NOW + 60, false));
        assert_eq!(i.record().unwrap(), big);
        assert_eq!(Input::of_bytes(0, rec(NOW, 1).bytes().unwrap()).unwrap().at, NOW);
    }

    #[test]
    fn a_frame_that_fails_is_kept_and_tried_after_flush() {
        let d = tempfile::tempdir().unwrap();
        // The archive's folder a file: every append fails.
        std::fs::create_dir_all(d.path().join("state/coord")).unwrap();
        std::fs::write(d.path().join(DIR), b"").unwrap();
        let mut b = Batch::new(root(&d));
        b.take(given(1, &rec(NOW, 1)));
        b.flush();
        assert_eq!(b.pending.len(), 1);
        assert!(b.failed.is_some() && !b.due(), "not again before FLUSH");
        assert!(b.wait() > Duration::from_secs(EVERY_S));
        // Its folder back: written once due.
        std::fs::remove_file(d.path().join(DIR)).unwrap();
        b.flush_after = Duration::ZERO;
        assert!(b.due());
        b.flush();
        assert!(b.pending.is_empty() && b.failed.is_none());
        assert_eq!(read(&root(&d), NOW, NOW + 1).len(), 1);
    }

    #[test]
    fn days_before_the_backtest_s_span_are_removed() {
        let d = tempfile::tempdir().unwrap();
        let days = KEEP_S / 86_400;
        for back in [0, days - 1, days, days + 1, days + 5] {
            let at = NOW - back * 86_400;
            let day = journal::day(at).unwrap();
            fcarchive::append_frame(day_path(&root(&d), &day), frame(&[&given(1, &rec(at, 1))]).unwrap()).unwrap();
        }
        assert_eq!(prune(&root(&d), NOW, true).unwrap().files_removed, 2);
        assert_eq!(read(&root(&d), 0, NOW + 1).len(), 5, "a dry run removes nothing");
        assert_eq!(prune(&root(&d), NOW, false).unwrap().files_removed, 2);
        let left: Vec<u64> = read(&root(&d), 0, NOW + 1).iter().map(|i| (NOW - i.at) / 86_400).collect();
        assert_eq!(left, [days, days - 1, 0], "the day `KEEP_S` back is kept: a span from then reads it");
        assert_eq!(prune(&root(&d), NOW, false).unwrap().files_removed, 0);
    }

    /// Recorded inputs (SCENIC_FORECAST_RECORD: forecast-input.json files, ':' between, in the
    /// order made) as one frame: their JSON's size and the frame's. For measuring at live scale
    /// (unset: nothing).
    #[test]
    fn recorded_frame_size() {
        let Ok(ps) = std::env::var("SCENIC_FORECAST_RECORD") else { return };
        let gs: Vec<Given> = ps
            .split(':')
            .enumerate()
            .map(|(k, p)| {
                let bytes = std::fs::read(p).unwrap();
                let at = Input::of_bytes(0, bytes.clone()).unwrap().at;
                Given { term: k as u64, at, bytes }
            })
            .collect();
        let raw: u64 = gs.iter().map(|g| json_len(&g.bytes).unwrap()).sum();
        let given: u64 = gs.iter().map(|g| g.bytes.len() as u64).sum();
        let z = frame(&gs.iter().collect::<Vec<_>>()).unwrap();
        eprintln!("{} inputs: {raw} B of JSON, {given} B as given, one frame {} B", gs.len(), z.len());
    }
}
