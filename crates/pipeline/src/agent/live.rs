//! The live store (docs/pool.md §10, Live): each Mac's own measures as series over time, kept in
//! rings on its own disk (never the NAS) and answered by its port's `POST /work/live`.
//!
//! - **Rings**: each series in three, by resolution (`TIERS`): a point a second for an hour, every
//!   10 s for a day, a minute for three days. A point is a bucket of the samples in its time: the
//!   least, the mean and the most. A sample goes into all three.
//! - **Kept on this Mac's disk**: the whole store written (create and rename) in the agent's folder
//!   (`FILE`) every `SAVE_EVERY` and as the agent stops, read back as it starts. A bucket older than
//!   its ring's span is never answered.
//! - **Sampled** on threads of the agent's own (`spawn`), never its loop: every second the load,
//!   memory free, its pressure, the swap and the jobs running; every 5 s what they hold (the memory
//!   guard's last sample); every 10 s the disk's free space. The battery, mains power and the
//!   owner's idleness as the agent's loop reads them (each round, ~20 s). The NAS's latency a second, while a build runs (`set_nas`): one stat of
//!   `catalog` through the records' readers (crate::agent::nasread::probe), never two at once.
//! - **A series missing** from an answer has no measure yet (or none in the window): readers say
//!   so, never zero.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The rings: (seconds a point, points kept): an hour by the second, a day by 10 s, three days by
/// the minute.
pub const TIERS: [(u64, usize); 3] = [(1, 3600), (10, 8640), (60, 4320)];

/// The store's file in the agent's folder.
pub const FILE: &str = "live.bin";

/// How often the store is written to the disk (and as the agent stops).
pub const SAVE_EVERY: Duration = Duration::from_secs(300);

/// The file's first bytes (its format's version).
const MAGIC: &[u8; 8] = b"SCLIVE01";

/// The series' names (units in the name; docs/pool.md §10).
pub const NAS_MS: &str = "nas_ms";
pub const LOAD: &str = "load1";
pub const MEM_FREE: &str = "mem_free_pct";
pub const PRESSURE: &str = "mem_pressure";
pub const SWAP: &str = "swap_mb";
pub const JOBS: &str = "jobs";
pub const HELD: &str = "held_mb";
pub const DISK: &str = "disk_free_gb";
pub const BATTERY: &str = "battery_pct";
pub const AC: &str = "ac";
pub const IDLE: &str = "idle_s";

/// A point: the samples in its time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Bucket {
    /// Its time, in its ring's steps since the epoch (unix seconds / step).
    slot: u64,
    n: u32,
    lo: f32,
    hi: f32,
    sum: f64,
}

/// A series' rings, each allocated with its first sample.
#[derive(Clone, Debug, Default)]
struct Series {
    rings: [Vec<Bucket>; 3],
}

impl Series {
    fn add(&mut self, t: u64, v: f64) {
        for (ring, &(step, len)) in self.rings.iter_mut().zip(TIERS.iter()) {
            if ring.is_empty() {
                ring.resize(len, Bucket::default());
            }
            let slot = t / step;
            let b = &mut ring[(slot % len as u64) as usize];
            if b.slot != slot || b.n == 0 {
                *b = Bucket { slot, n: 0, lo: f32::INFINITY, hi: f32::NEG_INFINITY, sum: 0.0 };
            }
            b.n += 1;
            b.lo = b.lo.min(v as f32);
            b.hi = b.hi.max(v as f32);
            b.sum += v;
        }
    }

    /// Ring `k`'s points from `from` to `to` (unix s, bucket times), oldest first.
    fn points(&self, k: usize, from: u64, to: u64) -> Vec<[f64; 4]> {
        let (step, len) = TIERS[k];
        let ring = &self.rings[k];
        if ring.is_empty() {
            return Vec::new();
        }
        // (No older than the ring holds.)
        let first = (from / step).max((to / step).saturating_sub(len as u64 - 1));
        (first..=to / step)
            .filter_map(|slot| {
                let b = ring[(slot % len as u64) as usize];
                (b.n > 0 && b.slot == slot)
                    .then(|| [(slot * step) as f64, round(b.lo as f64), round(b.sum / b.n as f64), round(b.hi as f64)])
            })
            .collect()
    }

    /// The newest point of the finest ring at or after `since` (unix s): its mean.
    fn latest(&self, since: u64, now: u64) -> Option<f64> {
        self.points(0, since, now).last().map(|p| p[2])
    }
}

/// Two decimals: the answers' size.
fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A reader's ask (`POST /work/live`'s body): the window back from now (s; an hour by default,
/// three days at most), or the points from `since` on (unix s, a bucket's time: the last one
/// answered again, as it may have grown since); `series`, the names wanted (all when missing).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ask {
    pub range: Option<u64>,
    pub since: Option<u64>,
    pub series: Option<Vec<String>>,
}

/// The finest ring whose span covers `range` seconds (the coarsest past three days).
pub fn tier_for(range: u64) -> usize {
    TIERS.iter().position(|&(step, len)| step * len as u64 >= range).unwrap_or(TIERS.len() - 1)
}

/// The store: every series of this Mac's, in memory, written to its file (`save`).
pub struct Store {
    series: Mutex<BTreeMap<String, Series>>,
    file: Option<PathBuf>,
    host: String,
    /// The NAS's root while a build runs (`set_nas`): its latency sampled meanwhile alone.
    nas: Mutex<Option<PathBuf>>,
    sampling: AtomicBool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("live::Store")
    }
}

impl Store {
    /// The store kept in `file` (read from it now when it's there; none: memory alone, a test's),
    /// of Mac `host`.
    pub fn new(file: Option<PathBuf>, host: &str) -> Arc<Store> {
        let series = file
            .as_deref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| match decode(&b) {
                Some(s) => Some(s),
                None => {
                    eprintln!("agent: the live store's file can't be read; starting it again");
                    None
                }
            })
            .unwrap_or_default();
        Arc::new(Store {
            series: Mutex::new(series),
            file,
            host: host.to_string(),
            nas: Mutex::new(None),
            sampling: AtomicBool::new(false),
        })
    }

    /// A sample of series `name` at `t` (unix s).
    pub fn record(&self, name: &str, t: u64, v: f64) {
        if !v.is_finite() {
            return;
        }
        if let Ok(mut s) = self.series.lock() {
            s.entry(name.to_string()).or_default().add(t, v);
        }
    }

    /// Series `name`'s newest mean by the second, when one is within `fresh_s` of `now`.
    pub fn latest(&self, name: &str, now: u64, fresh_s: u64) -> Option<f64> {
        self.series.lock().ok()?.get(name)?.latest(now.saturating_sub(fresh_s), now)
    }

    /// The NAS's latency sampled at `root` while a build runs (None: not sampled).
    pub fn set_nas(&self, root: Option<&Path>) {
        if let Ok(mut n) = self.nas.lock() {
            *n = root.map(Path::to_path_buf);
        }
    }

    /// `/work/live`'s answer at `now` (unix s): `{host, now, step, from, series: {name: [[t, lo,
    /// mean, hi], …]}}`, each series with a point in the window alone.
    pub fn answer(&self, a: &Ask, now: u64) -> serde_json::Value {
        let max = TIERS[TIERS.len() - 1].0 * TIERS[TIERS.len() - 1].1 as u64;
        let range = a.range.unwrap_or_else(|| a.since.map_or(3600, |s| now.saturating_sub(s).max(1))).min(max);
        let k = tier_for(range);
        let step = TIERS[k].0;
        let from = a.since.unwrap_or(0).max(now.saturating_sub(range));
        let out: serde_json::Map<String, serde_json::Value> = match self.series.lock() {
            Ok(s) => s
                .iter()
                .filter(|(n, _)| a.series.as_ref().is_none_or(|w| w.iter().any(|x| x == *n)))
                .filter_map(|(n, ser)| {
                    let p = ser.points(k, from, now);
                    (!p.is_empty()).then(|| (n.clone(), serde_json::json!(p)))
                })
                .collect(),
            Err(_) => Default::default(),
        };
        serde_json::json!({ "host": self.host, "now": now, "step": step, "from": from / step * step, "series": out })
    }

    /// Writes the store to its file (create and rename): none, nothing.
    pub fn save(&self) {
        let Some(f) = &self.file else { return };
        let bytes = match self.series.lock() {
            Ok(s) => encode(&s),
            Err(_) => return,
        };
        let tmp = f.with_extension("bin.tmp");
        if let Err(e) = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, f)) {
            eprintln!("agent: writing the live store: {e}");
        }
    }
}

/// The store as bytes: `MAGIC`, then per series its name (u16 length, UTF-8) and per ring its
/// buckets with samples (u32 count; each slot u64, n u32, lo f32, hi f32, sum f64), little-endian.
fn encode(s: &BTreeMap<String, Series>) -> Vec<u8> {
    let mut b = MAGIC.to_vec();
    b.extend((s.len() as u32).to_le_bytes());
    for (name, ser) in s {
        b.extend((name.len() as u16).to_le_bytes());
        b.extend(name.as_bytes());
        for ring in &ser.rings {
            let kept: Vec<&Bucket> = ring.iter().filter(|x| x.n > 0).collect();
            b.extend((kept.len() as u32).to_le_bytes());
            for x in kept {
                b.extend(x.slot.to_le_bytes());
                b.extend(x.n.to_le_bytes());
                b.extend(x.lo.to_le_bytes());
                b.extend(x.hi.to_le_bytes());
                b.extend(x.sum.to_le_bytes());
            }
        }
    }
    b
}

/// `encode`'s bytes back; None when they aren't its.
fn decode(b: &[u8]) -> Option<BTreeMap<String, Series>> {
    struct R<'a>(&'a [u8]);
    impl R<'_> {
        fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
            let (h, t) = self.0.split_at_checked(N)?;
            self.0 = t;
            h.try_into().ok()
        }
    }
    let mut r = R(b.strip_prefix(MAGIC.as_slice())?);
    let n = u32::from_le_bytes(r.take()?);
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let len = u16::from_le_bytes(r.take()?) as usize;
        let (name, rest) = r.0.split_at_checked(len)?;
        let name = std::str::from_utf8(name).ok()?.to_string();
        r.0 = rest;
        let mut ser = Series::default();
        for (ring, &(_, len)) in ser.rings.iter_mut().zip(TIERS.iter()) {
            let count = u32::from_le_bytes(r.take()?);
            if count > 0 {
                ring.resize(len, Bucket::default());
            }
            for _ in 0..count {
                let x = Bucket {
                    slot: u64::from_le_bytes(r.take()?),
                    n: u32::from_le_bytes(r.take()?),
                    lo: f32::from_le_bytes(r.take()?),
                    hi: f32::from_le_bytes(r.take()?),
                    sum: f64::from_le_bytes(r.take()?),
                };
                let i = (x.slot % len as u64) as usize;
                // (The newer of two in one place: a file from a ring of another size.)
                if ring[i].n == 0 || ring[i].slot < x.slot {
                    ring[i] = x;
                }
            }
        }
        out.insert(name, ser);
    }
    r.0.is_empty().then_some(out)
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Starts the samplers (once a process: the agent's, on a real Mac): this Mac's measures into
/// `store` (module doc), its disk's free space where `home` is, the jobs as `guard` watches them;
/// the NAS's latency on a thread of its own. The store written every `SAVE_EVERY`.
pub fn spawn(store: Arc<Store>, home: PathBuf, guard: super::memguard::Sampler) {
    if store.sampling.swap(true, Ordering::SeqCst) {
        return;
    }
    let s = store.clone();
    std::thread::Builder::new()
        .name("live-sampler".into())
        .spawn(move || {
            let mut saved = std::time::Instant::now();
            loop {
                let t = now_s();
                sample(&s, &home, &guard, t);
                if saved.elapsed() >= SAVE_EVERY {
                    s.save();
                    saved = std::time::Instant::now();
                }
                // (On the next second.)
                let ms =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_millis());
                std::thread::sleep(Duration::from_millis(u64::from(1000 - ms.min(999))));
            }
        })
        .ok();
    std::thread::Builder::new()
        .name("live-nas".into())
        .spawn(move || loop {
            let root = store.nas.lock().ok().and_then(|n| n.clone());
            if let Some(r) = root {
                let t = now_s();
                if let Some(d) = super::nasread::probe(&r.join("catalog")) {
                    store.record(NAS_MS, t, d.as_secs_f64() * 1000.0);
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        })
        .ok();
}

/// One second's samples at `t` (module doc: what, how often).
fn sample(s: &Store, home: &Path, guard: &super::memguard::Sampler, t: u64) {
    let put = |name: &str, v: Option<f64>| {
        if let Some(v) = v {
            s.record(name, t, v);
        }
    };
    put(LOAD, super::cond::load1());
    put(MEM_FREE, crate::sys::memory_free_pct().map(f64::from));
    put(PRESSURE, crate::sys::memory_pressure().map(f64::from));
    put(SWAP, crate::sys::swap_used().map(|b| (b >> 20) as f64));
    let (jobs, held) = guard.watched();
    put(JOBS, Some(jobs as f64));
    if t % 5 == 0 {
        put(HELD, if jobs == 0 { Some(0.0) } else { held.map(|h| h as f64) });
    }
    if t % 10 == 0 {
        put(DISK, super::room::disk_free(home).ok().map(|b| b as f64 / (1u64 << 30) as f64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_keeps_the_least_mean_and_most_in_every_ring() {
        let s = Store::new(None, "m4");
        let t0 = 1_000_000_000;
        for (i, v) in [3.0, 1.0, 5.0].iter().enumerate() {
            s.record(LOAD, t0 + i as u64, *v);
        }
        let by_s = s.answer(&Ask { range: Some(60), ..Default::default() }, t0 + 2);
        assert_eq!(by_s["step"], 1);
        assert_eq!(by_s["host"], "m4");
        assert_eq!(
            by_s["series"][LOAD],
            serde_json::json!([[1e9, 3.0, 3.0, 3.0], [1e9 + 1.0, 1.0, 1.0, 1.0], [1e9 + 2.0, 5.0, 5.0, 5.0]])
        );
        let by_10 = s.answer(&Ask { range: Some(86_400), ..Default::default() }, t0 + 2);
        assert_eq!(by_10["step"], 10);
        assert_eq!(by_10["series"][LOAD], serde_json::json!([[1e9, 1.0, 3.0, 5.0]]));
        let by_min = s.answer(&Ask { range: Some(3 * 86_400), ..Default::default() }, t0 + 2);
        assert_eq!(by_min["step"], 60);
        // (Past three days: the minutes' ring, three days.)
        assert_eq!(s.answer(&Ask { range: Some(30 * 86_400), ..Default::default() }, t0 + 2)["step"], 60);
    }

    #[test]
    fn since_answers_the_points_from_then_the_last_again() {
        let s = Store::new(None, "m4");
        let t0 = 2_000_000_000;
        for i in 0..10 {
            s.record(JOBS, t0 + i, i as f64);
        }
        let a = s.answer(&Ask { since: Some(t0 + 8), ..Default::default() }, t0 + 9);
        assert_eq!(a["step"], 1);
        assert_eq!(a["series"][JOBS].as_array().unwrap().len(), 2);
        // (A series with no point in the window, or none at all: missing, never zero.)
        s.record(DISK, t0, 100.0);
        let a = s.answer(&Ask { since: Some(t0 + 5), ..Default::default() }, t0 + 9);
        assert!(a["series"].get(DISK).is_none());
        assert!(a["series"].get(NAS_MS).is_none());
        let only = s.answer(&Ask { series: Some(vec![DISK.into()]), ..Default::default() }, t0 + 9);
        assert_eq!(only["series"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn a_ring_never_answers_older_than_it_holds() {
        let s = Store::new(None, "m4");
        let t0 = 3_000_000_000;
        s.record(LOAD, t0, 1.0);
        // (An hour and a second on, its place in the seconds' ring taken by a newer second.)
        s.record(LOAD, t0 + 3600, 2.0);
        let a = s.answer(&Ask { range: Some(3600), ..Default::default() }, t0 + 3600);
        assert_eq!(a["series"][LOAD], serde_json::json!([[(t0 + 3600) as f64, 2.0, 2.0, 2.0]]));
        assert_eq!(s.latest(LOAD, t0 + 3600, 30), Some(2.0));
        assert_eq!(s.latest(LOAD, t0 + 3700, 30), None);
    }

    #[test]
    fn the_store_is_written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(FILE);
        let t0 = 1_700_000_000;
        let s = Store::new(Some(f.clone()), "m1");
        for i in 0..100 {
            s.record(LOAD, t0 + i, i as f64);
            s.record(NAS_MS, t0 + i, 2.5);
        }
        s.save();
        let back = Store::new(Some(f.clone()), "m1");
        let ask = Ask { range: Some(86_400), ..Default::default() };
        assert_eq!(back.answer(&ask, t0 + 99), s.answer(&ask, t0 + 99));
        // (Not its bytes: started again, empty.)
        std::fs::write(&f, b"nonsense").unwrap();
        assert_eq!(Store::new(Some(f), "m1").answer(&ask, t0 + 99)["series"], serde_json::json!({}));
        assert!(decode(&encode(&BTreeMap::new())).unwrap().is_empty());
    }

    #[test]
    fn a_window_takes_the_finest_ring_covering_it() {
        assert_eq!(tier_for(1), 0);
        assert_eq!(tier_for(3600), 0);
        assert_eq!(tier_for(3601), 1);
        assert_eq!(tier_for(86_400), 1);
        assert_eq!(tier_for(86_401), 2);
        assert_eq!(tier_for(u64::MAX), 2);
    }
}
