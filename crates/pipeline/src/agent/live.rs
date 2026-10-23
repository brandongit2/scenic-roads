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
//!   memory free, its pressure, the swap and the jobs running; the cores busy (all, the efficiency
//!   cores, the performance ones: `host_processor_info`'s ticks, the efficiency cores the first
//!   processors, as many as the last perf level's; the performance ones every other); the build's CPU and memory (its jobs' processes' `proc_pid_rusage`)
//!   and the rest of the Mac's (the cores busy and the memory in use, less the build's); the bytes
//!   written to the Mac's disks (IOKit's block storage drivers' statistics) and by each job, as
//!   rates; the sampler's own CPU time. Every 10 s the disk's free space. The battery, mains power and the
//!   owner's idleness as the agent's loop reads them (each round, ~20 s; minutes apart when a round is slow). The NAS's latency every 5 s (`NAS_EVERY`), while a build runs (`set_nas`):
//!   a listing of the project's folder through the records' readers (crate::agent::nasread::probe;
//!   a stat would be answered from the SMB client's cache), never two at once.
//! - **The jobs' NAS traffic** every second (`NasRates`): each job's processes' counts summed from
//!   their files (store::nasio; the agent's folder per slot; the files of processes that ended
//!   folded into one first), as rates over the second: bytes read
//!   and written (MB/s), files opened and made, entries listed, files statted and deleted (a
//!   second). A job first seen sets its baseline (its first second not counted); one whose agent
//!   gave it no folder (from an older app) leaves the second unmeasured.
//! - **The NAS as a whole** (task #517): every second this process's NAS calls in flight and
//!   missed (crate::agent::nasread::calls); on the lead, every 10 s from its coordinator's NAS
//!   prober, the disks' busy shares (`record_disks`, the load gate's own reading) and the pool's NAS
//!   traffic, the members' summed (`record_pool`).
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

/// The most points a series in one answer (the ring's buckets grouped past it): an hour by the
/// second is 6 s a point, a page's first ask ~100 kB a Mac rather than ~1.5 MB.
pub const POINTS_MAX: u64 = 600;

/// The store's file in the agent's folder.
pub const FILE: &str = "live.bin";

/// How often the store is written to the disk (and as the agent stops).
pub const SAVE_EVERY: Duration = Duration::from_secs(300);

/// How often the NAS's latency is measured while a build runs.
pub const NAS_EVERY: Duration = Duration::from_secs(5);

/// The file's first bytes (its format's version).
const MAGIC: &[u8; 8] = b"SCLIVE01";

/// The series' names (units in the name; docs/pool.md §10).
pub const NAS_MS: &str = "nas_ms";
pub const LOAD: &str = "load1";
pub const MEM_FREE: &str = "mem_free_pct";
pub const PRESSURE: &str = "mem_pressure";
pub const SWAP: &str = "swap_mb";
pub const JOBS: &str = "jobs";
/// The cores busy (cores' worth, over the second): all, the efficiency cores, the performance ones.
pub const CORES: &str = "cores_busy";
pub const CORES_E: &str = "cores_busy_e";
pub const CORES_P: &str = "cores_busy_p";
/// The build's (its jobs') CPU and the rest of the Mac's (cores' worth), and their memory (GB).
pub const BUILD_CORES: &str = "build_cores";
pub const OTHERS_CORES: &str = "others_cores";
pub const BUILD_MEM: &str = "build_mem_gb";
pub const OTHERS_MEM: &str = "others_mem_gb";
/// The bytes written to the Mac's disks, and each job's (MB/s; `JOB_WRITE` and its slot).
pub const DISK_WRITE: &str = "disk_write_mbs";
pub const JOB_WRITE: &str = "job_write_mbs.";
/// The sampler's own CPU time a second (ms).
pub const SAMPLER: &str = "sampler_cpu_ms";
/// The bytes through the proxy, in (to the jobs) and out (MB/s; crate::budget::net).
pub const NET_IN: &str = "net_in_mbs";
pub const NET_OUT: &str = "net_out_mbs";
pub const DISK: &str = "disk_free_gb";
pub const BATTERY: &str = "battery_pct";
pub const AC: &str = "ac";
pub const IDLE: &str = "idle_s";
/// The jobs' NAS traffic (store::nasio::Count's order), per second.
pub const NAS_TRAFFIC: [&str; 7] =
    ["nas_read_mbs", "nas_files_s", "nas_write_mbs", "nas_creates_s", "nas_listed_s", "nas_stats_s", "nas_deletes_s"];
/// The pool's NAS traffic on the lead, each `NAS_TRAFFIC` series summed over the members'
/// (`POOL` then its name; crate::coord::Capacity::nas_traffic), every 10 s.
pub const POOL: &str = "pool_";
/// This process's NAS calls under their deadlines (crate::agent::nasread::calls): in flight now,
/// and missed over the second (past their deadline, or refused with every thread busy).
pub const NAS_CALLS: &str = "nas_calls";
pub const NAS_MISSED: &str = "nas_missed";
/// The NAS's disks' busy share (%), as the lead's load gate reads them every 10 s (crate::budget::snmp):
/// the busiest, and each disk's (`NAS_DISK_BUSY`, a dot, its number from 1).
pub const NAS_DISK_BUSY: &str = "nas_disk_busy_pct";

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

    /// Ring `k`'s points from `from` to `to` (unix s, bucket times), oldest first, `g` of its
    /// buckets a point (a group's from its time a multiple of `g` steps: the least, the mean of the
    /// samples, the most).
    fn points(&self, k: usize, g: u64, from: u64, to: u64) -> Vec<[f64; 4]> {
        let (step, len) = TIERS[k];
        let ring = &self.rings[k];
        if ring.is_empty() {
            return Vec::new();
        }
        // (No older than the ring holds.)
        let first = (from / step).max((to / step).saturating_sub(len as u64 - 1));
        let mut out: Vec<[f64; 4]> = Vec::new();
        let mut at: Option<(u64, Bucket)> = None;
        let mut put = |a: Option<(u64, Bucket)>| {
            if let Some((slot, b)) = a {
                out.push([(slot * step) as f64, round(b.lo as f64), round(b.sum / b.n as f64), round(b.hi as f64)]);
            }
        };
        for slot in first..=to / step {
            let b = ring[(slot % len as u64) as usize];
            if b.n == 0 || b.slot != slot {
                continue;
            }
            let start = slot / g * g;
            match &mut at {
                Some((s0, a)) if *s0 == start => {
                    a.n += b.n;
                    a.lo = a.lo.min(b.lo);
                    a.hi = a.hi.max(b.hi);
                    a.sum += b.sum;
                }
                _ => {
                    put(at.take());
                    at = Some((start, b));
                }
            }
        }
        put(at);
        out
    }

    /// The newest point of the finest ring at or after `since` (unix s): its mean.
    fn latest(&self, since: u64, now: u64) -> Option<f64> {
        self.points(0, 1, since, now).last().map(|p| p[2])
    }
}

/// Two decimals: the answers' size.
fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A reader's ask (`POST /work/live`'s body): the window back from now (s; an hour by default,
/// three days at most), or the points from `since` on (unix s, a bucket's time: the last one
/// answered again, as it may have grown since); `series`, the names wanted (all when missing);
/// `net: false` leaves the proxy's counts out (a chart polling every second needs only the series).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ask {
    pub range: Option<u64>,
    pub since: Option<u64>,
    pub series: Option<Vec<String>>,
    pub net: Option<bool>,
}

/// This Mac's size, the charts' scales (`mac` in an answer): its cores, its efficiency cores, its
/// memory (GB). Read once.
fn mac() -> &'static serde_json::Value {
    static MAC: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    MAC.get_or_init(|| {
        serde_json::json!({
            "cores": crate::inputs::pace::cores(),
            "cores_e": crate::sys::efficiency_cores(),
            "mem_gb": crate::sys::memsize().map(|b| round(b as f64 / GB)),
        })
    })
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
    /// The proxy's counts (`set_net`): answered as `net`, their rates sampled.
    net: Mutex<Option<Arc<crate::budget::net::Net>>>,
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
            net: Mutex::new(None),
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

    /// The NAS's disks' busy shares (%) read at `t` (`NAS_DISK_BUSY`): the busiest and each one's.
    pub fn record_disks(&self, t: u64, busy: &[f64]) {
        if let Some(m) = busy.iter().copied().filter(|b| b.is_finite()).reduce(f64::max) {
            self.record(NAS_DISK_BUSY, t, m);
        }
        for (i, b) in busy.iter().enumerate() {
            self.record(&format!("{NAS_DISK_BUSY}.{}", i + 1), t, *b);
        }
    }

    /// The pool's NAS traffic at `t` (`POOL`, `NAS_TRAFFIC`'s order), the members' summed.
    pub fn record_pool(&self, t: u64, traffic: &[f64; 7]) {
        for (n, v) in NAS_TRAFFIC.iter().zip(traffic) {
            self.record(&format!("{POOL}{n}"), t, *v);
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

    /// The proxy's counts, answered and sampled from here on.
    pub fn set_net(&self, net: Arc<crate::budget::net::Net>) {
        if let Ok(mut n) = self.net.lock() {
            *n = Some(net);
        }
    }

    /// Whether it has the proxy's counts.
    pub fn has_net(&self) -> bool {
        self.net.lock().is_ok_and(|n| n.is_some())
    }

    fn net(&self) -> Option<Arc<crate::budget::net::Net>> {
        self.net.lock().ok()?.clone()
    }

    /// `/work/live`'s answer at `now` (unix s): `{host, now, step, from, mac, series: {name: [[t,
    /// lo, mean, hi], …]}, net}`, each series with a point in the window alone; `mac`, its size
    /// (`mac()`); `net`, the proxy's counts (crate::budget::net::Net::view) once it has them,
    /// unless asked without.
    pub fn answer(&self, a: &Ask, now: u64) -> serde_json::Value {
        let max = TIERS[TIERS.len() - 1].0 * TIERS[TIERS.len() - 1].1 as u64;
        let range = a.range.unwrap_or_else(|| a.since.map_or(3600, |s| now.saturating_sub(s).max(1))).min(max);
        let k = tier_for(range);
        // (At most POINTS_MAX points a series: a ring's buckets grouped, the groups on multiples of
        // their span, so an ask `since` one gets the same points as the window had.)
        let g = (range / TIERS[k].0).div_ceil(POINTS_MAX).max(1);
        let step = TIERS[k].0 * g;
        let from = a.since.unwrap_or(0).max(now.saturating_sub(range)) / step * step;
        let out: serde_json::Map<String, serde_json::Value> = match self.series.lock() {
            Ok(s) => s
                .iter()
                .filter(|(n, _)| a.series.as_ref().is_none_or(|w| w.iter().any(|x| x == *n)))
                .filter_map(|(n, ser)| {
                    let p = ser.points(k, g, from, now);
                    (!p.is_empty()).then(|| (n.clone(), serde_json::json!(p)))
                })
                .collect(),
            Err(_) => Default::default(),
        };
        let mut v = serde_json::json!({ "host": self.host, "now": now, "step": step, "from": from, "mac": mac(), "series": out });
        if let Some(n) = self.net().filter(|_| a.net != Some(false)) {
            v["net"] = n.view();
        }
        v
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
            let mut rates = Rates::default();
            loop {
                let t = now_s();
                let cpu = crate::sys::thread_cpu_ns();
                rates.sample(&s, &home, &guard, t);
                s.record(SAMPLER, t, crate::sys::thread_cpu_ns().saturating_sub(cpu) as f64 / 1e6);
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
                if let Some(d) = super::nasread::probe(&r) {
                    store.record(NAS_MS, t, d.as_secs_f64() * 1000.0);
                }
            }
            std::thread::sleep(NAS_EVERY);
        })
        .ok();
}

/// The counts read last, for the rates over the second (module doc).
#[derive(Default)]
struct Rates {
    nas: NasRates,
    cores: Option<Vec<(u64, u64)>>,
    /// Per job (process group): the most of each count yet read, and when.
    jobs: BTreeMap<i32, (crate::sys::Usage, u64)>,
    disk: Option<(u64, u64)>,
    /// The proxy's bytes in and out, and when.
    net: Option<(u64, u64, u64)>,
    /// The NAS calls missed so far, as last read.
    missed: Option<u64>,
}

impl Rates {
    /// One second's samples at `t` (module doc: what, how often).
    fn sample(&mut self, s: &Store, home: &Path, guard: &super::memguard::Sampler, t: u64) {
        let put = |name: &str, v: Option<f64>| {
            if let Some(v) = v {
                s.record(name, t, v);
            }
        };
        put(LOAD, super::cond::load1());
        let free = crate::sys::memory_free_pct();
        put(MEM_FREE, free.map(f64::from));
        put(PRESSURE, crate::sys::memory_pressure().map(f64::from));
        put(SWAP, crate::sys::swap_used().map(|b| (b >> 20) as f64));
        let jobs = guard.jobs();
        put(JOBS, Some(jobs.len() as f64));
        if t % 10 == 0 {
            put(DISK, super::room::disk_free(home).ok().map(|b| b as f64 / GB));
        }
        let now = crate::sys::core_ticks();
        let busy = match (&self.cores, &now) {
            (Some(was), Some(now)) => cores_busy(was, now, crate::sys::efficiency_cores() as usize),
            _ => None,
        };
        self.cores = now;
        if let Some([all, e, p]) = busy {
            put(CORES, Some(all));
            put(CORES_E, Some(e));
            put(CORES_P, Some(p));
        }
        let used: Vec<_> = jobs.iter().map(|(k, pgid, _)| (*k, *pgid, crate::sys::usage_of_group(*pgid))).collect();
        let b = self.jobs(&used, t);
        put(BUILD_CORES, b.cores);
        if let (Some(all), Some(c)) = (busy, b.cores) {
            put(OTHERS_CORES, Some((all[0] - c).max(0.0)));
        }
        put(BUILD_MEM, b.mem.map(|m| m as f64 / GB));
        if let (Some(m), Some(total), Some(free)) = (b.mem, crate::sys::memsize(), free) {
            let used = total as f64 * f64::from(100 - free.min(100)) / 100.0;
            put(OTHERS_MEM, Some((used - m as f64).max(0.0) / GB));
        }
        for (k, v) in b.writes {
            s.record(&format!("{JOB_WRITE}{k}"), t, v);
        }
        if let Some(w) = crate::sys::disk_written() {
            if let Some((was, at)) = self.disk {
                put(DISK_WRITE, Some(w.saturating_sub(was) as f64 / MB / t.saturating_sub(at).max(1) as f64));
            }
            self.disk = Some((w, t));
        }
        self.net(s, t);
        let (calls, missed) = super::nasread::calls();
        put(NAS_CALLS, Some(calls as f64));
        if let Some(was) = self.missed {
            put(NAS_MISSED, Some(missed.saturating_sub(was) as f64));
        }
        self.missed = Some(missed);
        let counts: Vec<(i32, PathBuf)> = jobs.into_iter().map(|(_, g, d)| (g, d)).collect();
        if let Some(r) = self.nas.rates(&counts, t) {
            for (name, v) in NAS_TRAFFIC.iter().zip(r) {
                s.record(name, t, v);
            }
        }
    }

    /// The proxy's bytes in and out over the second at `t`, once the store has its counts (the
    /// first read their baseline).
    fn net(&mut self, s: &Store, t: u64) {
        let Some((i, o)) = s.net().map(|n| n.totals()) else { return };
        if let Some((wi, wo, at)) = self.net {
            let dt = t.saturating_sub(at).max(1) as f64;
            s.record(NET_IN, t, i.saturating_sub(wi) as f64 / MB / dt);
            s.record(NET_OUT, t, o.saturating_sub(wo) as f64 / MB / dt);
        }
        self.net = Some((i, o, t));
    }

    /// The jobs' use at `t`, from each one's (slot, process group, usage now): what they hold
    /// (bytes; None with jobs running and none read, zero with none), their CPU (cores' worth over
    /// the second, of the jobs read a second time; None with none such) and each one's disk writes
    /// (MB/s). A job first seen sets its baseline; each count kept at the most seen (a process that
    /// ended takes its counts with it: its last moment isn't counted).
    fn jobs(&mut self, jobs: &[(usize, i32, Option<crate::sys::Usage>)], t: u64) -> JobUse {
        let none = jobs.is_empty();
        let mut out = JobUse { mem: none.then_some(0), cores: none.then_some(0.0), writes: vec![] };
        let mut seen = BTreeMap::new();
        for (k, pgid, u) in jobs {
            let Some(u) = u else { continue };
            *out.mem.get_or_insert(0) += u.footprint;
            let mut high = *u;
            if let Some((was, at)) = self.jobs.get(pgid) {
                let dt = t.saturating_sub(*at).max(1) as f64;
                *out.cores.get_or_insert(0.0) += u.cpu_ns.saturating_sub(was.cpu_ns) as f64 / 1e9 / dt;
                out.writes.push((*k, u.written.saturating_sub(was.written) as f64 / MB / dt));
                high.cpu_ns = high.cpu_ns.max(was.cpu_ns);
                high.written = high.written.max(was.written);
            }
            seen.insert(*pgid, (high, t));
        }
        self.jobs = seen;
        out
    }
}

/// The jobs' use over a second (`Rates::jobs`).
#[derive(Debug, PartialEq)]
struct JobUse {
    mem: Option<u64>,
    cores: Option<f64>,
    writes: Vec<(usize, f64)>,
}

const GB: f64 = (1u64 << 30) as f64;
const MB: f64 = (1u64 << 20) as f64;

/// The cores busy between two reads of their ticks (`crate::sys::core_ticks`), in cores' worth:
/// all, the first `e` (the efficiency cores), the rest; None when the cores changed.
fn cores_busy(was: &[(u64, u64)], now: &[(u64, u64)], e: usize) -> Option<[f64; 3]> {
    if was.len() != now.len() || now.is_empty() {
        return None;
    }
    let mut out = [0.0; 3];
    for (i, (a, b)) in was.iter().zip(now).enumerate() {
        let all = b.1.saturating_sub(a.1);
        let f = if all == 0 { 0.0 } else { b.0.saturating_sub(a.0) as f64 / all as f64 };
        out[0] += f;
        out[if i < e { 1 } else { 2 }] += f;
    }
    Some(out)
}

/// The jobs' NAS counts, the most yet read, for their rates (module doc).
#[derive(Default)]
struct NasRates {
    /// Per job (process group): its counts and when they were read (unix s).
    last: BTreeMap<i32, (store::nasio::Counts, u64)>,
}

impl NasRates {
    /// The jobs' traffic a second at `t` (`NAS_TRAFFIC`'s order; MB for bytes), from each job's
    /// (process group, counts folder): zero with no jobs; None while one has no folder.
    fn rates(&mut self, jobs: &[(i32, PathBuf)], t: u64) -> Option<[f64; 7]> {
        let mut out = [0.0; 7];
        let mut seen = BTreeMap::new();
        let mut whole = true;
        for (pgid, dir) in jobs {
            // (Its ended programs' files folded into one: a long job's read stays small.)
            store::nasio::fold_ended(dir);
            let Some(c) = store::nasio::sum_dir(dir) else {
                whole = false;
                continue;
            };
            let mut high = c;
            if let Some((was, at)) = self.last.get(pgid) {
                let dt = t.saturating_sub(*at).max(1) as f64;
                for (i, o) in out.iter_mut().enumerate() {
                    *o += c[i].saturating_sub(was[i]) as f64 / dt;
                }
                // (The most seen: a program's counts moving to its runner's dip a moment, never
                // counted twice as they come back.)
                for (h, w) in high.iter_mut().zip(was) {
                    *h = (*h).max(*w);
                }
            }
            seen.insert(*pgid, (high, t));
        }
        self.last = seen;
        for i in [0, 2] {
            out[i] /= (1u64 << 20) as f64;
        }
        whole.then_some(out)
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
        // (A day: the 10 s ring, 15 buckets a point, POINTS_MAX; three days and past: the minutes', 8.)
        let by_10 = s.answer(&Ask { range: Some(86_400), ..Default::default() }, t0 + 2);
        assert_eq!(by_10["step"], 150);
        assert_eq!(by_10["series"][LOAD], serde_json::json!([[(t0 / 150 * 150) as f64, 1.0, 3.0, 5.0]]));
        let by_min = s.answer(&Ask { range: Some(3 * 86_400), ..Default::default() }, t0 + 2);
        assert_eq!(by_min["step"], 480);
        assert_eq!(s.answer(&Ask { range: Some(30 * 86_400), ..Default::default() }, t0 + 2)["step"], 480);
    }

    /// An hour by the second answered as POINTS_MAX points, 6 s each (the least, the samples' mean,
    /// the most), on multiples of 6 s; asked `since` the newest, the same group again, grown.
    #[test]
    fn an_answer_groups_its_buckets_to_points_max() {
        let s = Store::new(None, "m4");
        let t0 = 1_800_000_000; // (A multiple of 6.)
        for i in 0..3600 {
            s.record(LOAD, t0 + i, (i % 6) as f64);
        }
        let now = t0 + 3599;
        let a = s.answer(&Ask { range: Some(3600), ..Default::default() }, now);
        assert_eq!(a["step"], 6);
        let p = a["series"][LOAD].as_array().unwrap();
        assert_eq!(p.len() as u64, POINTS_MAX);
        assert_eq!(p[0], serde_json::json!([t0 as f64, 0.0, 2.5, 5.0]));
        s.record(LOAD, now + 1, 9.0);
        let since = p[p.len() - 1][0].as_f64().unwrap() as u64;
        let b = s.answer(&Ask { range: Some(3600), since: Some(since), ..Default::default() }, now + 1);
        assert_eq!(
            b["series"][LOAD],
            serde_json::json!([[since as f64, 0.0, 2.5, 5.0], [(now + 1) as f64, 9.0, 9.0, 9.0]])
        );
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
    fn the_jobs_nas_traffic_as_rates() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let put = |dir: &Path, pid: &str, c: [u64; 2]| {
            // (Named as a running process's, this one's: never folded.)
            let mut v = vec![0u8; store::nasio::SLOTS * 8];
            v[..8].copy_from_slice(&c[0].to_le_bytes());
            v[24..32].copy_from_slice(&c[1].to_le_bytes());
            std::fs::write(dir.join(pid), v).unwrap();
        };
        let mut r = NasRates::default();
        assert_eq!(r.rates(&[], 10), Some([0.0; 7]));
        put(&a, &format!("{}-1", std::process::id()), [4 << 20, 1]);
        // (First seen: its baseline.)
        assert_eq!(r.rates(&[(7, a.clone())], 10), Some([0.0; 7]));
        put(&a, &format!("{}-2", std::process::id()), [2 << 20, 3]);
        put(&b, &format!("{}-3", std::process::id()), [1 << 20, 0]);
        let x = r.rates(&[(7, a.clone()), (8, b.clone())], 12).unwrap();
        assert_eq!((x[0], x[3]), (1.0, 1.5));
        // (A job given no folder: the second unmeasured.)
        // (A dip as a program's counts move to its runner's, then back: nothing counted twice.)
        put(&a, &format!("{}-2", std::process::id()), [0, 0]);
        assert_eq!(r.rates(&[(7, a.clone())], 13).unwrap()[0], 0.0);
        put(&a, &format!("{}-2", std::process::id()), [2 << 20, 3]);
        assert_eq!(r.rates(&[(7, a.clone())], 14).unwrap()[0], 0.0);
        assert_eq!(r.rates(&[(7, a), (9, d.path().join("none"))], 15), None);
    }

    #[test]
    fn the_cores_busy_in_cores_worth_by_group() {
        // (Two efficiency cores, then two performance ones: half, idle, all, a quarter.)
        let was = [(0, 0), (10, 100), (0, 0), (5, 5)];
        let now = [(50, 100), (10, 200), (100, 100), (30, 105)];
        assert_eq!(cores_busy(&was, &now, 2), Some([1.75, 0.5, 1.25]));
        // (No ticks between: idle; the cores changed: none.)
        assert_eq!(cores_busy(&now, &now, 2), Some([0.0; 3]));
        assert_eq!(cores_busy(&was[..3], &now, 2), None);
        assert_eq!(cores_busy(&[], &[], 0), None);
    }

    #[test]
    fn the_jobs_cpu_memory_and_writes() {
        use crate::sys::Usage;
        let u = |cpu_s: u64, gb: u64, mb: u64| Usage {
            cpu_ns: cpu_s * 1_000_000_000,
            footprint: gb << 30,
            written: mb << 20,
        };
        let mut r = Rates::default();
        assert_eq!(r.jobs(&[], 10), JobUse { mem: Some(0), cores: Some(0.0), writes: vec![] });
        // (First seen: held now, the rates its baseline.)
        assert_eq!(
            r.jobs(&[(0, 7, Some(u(10, 2, 100)))], 10),
            JobUse { mem: Some(2 << 30), cores: None, writes: vec![] }
        );
        let j = r.jobs(&[(0, 7, Some(u(16, 3, 110))), (1, 8, Some(u(1, 1, 0)))], 12);
        assert_eq!(j, JobUse { mem: Some(4 << 30), cores: Some(3.0), writes: vec![(0, 5.0)] });
        // (A process ended, its counts gone: nothing negative, nothing counted twice.)
        let j = r.jobs(&[(0, 7, Some(u(12, 3, 105)))], 13);
        assert_eq!((j.cores, j.writes), (Some(0.0), vec![(0, 0.0)]));
        assert_eq!(r.jobs(&[(0, 7, Some(u(17, 3, 111)))], 14).cores, Some(1.0));
        // (A job unread: no measure of what it holds.)
        assert_eq!(r.jobs(&[(0, 9, None)], 15).mem, None);
    }

    /// The sampler's round on this Mac: each new series read, and its own CPU time small.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_round_reads_the_mac_and_costs_little() {
        let s = Store::new(None, "m4");
        let home = tempfile::tempdir().unwrap();
        let guard = super::super::memguard::Sampler::new(2);
        let mut r = Rates::default();
        let t0 = now_s();
        let cpu = crate::sys::thread_cpu_ns();
        for i in 0..20 {
            r.sample(&s, home.path(), &guard, t0 + i);
        }
        let ms = crate::sys::thread_cpu_ns().saturating_sub(cpu) as f64 / 1e6 / 20.0;
        eprintln!("a round: {ms:.3} ms of CPU");
        assert!(ms < 20.0, "a round took {ms} ms of CPU");
        for n in [
            NAS_CALLS,
            NAS_MISSED,
            CORES,
            CORES_E,
            CORES_P,
            BUILD_CORES,
            OTHERS_CORES,
            BUILD_MEM,
            OTHERS_MEM,
            DISK_WRITE,
        ] {
            assert!(s.latest(n, t0 + 19, 5).is_some(), "{n} not sampled");
        }
        let cores = crate::sys::core_ticks().unwrap().len() as f64;
        assert!(s.latest(CORES, t0 + 19, 5).unwrap() <= cores);
    }

    /// Task #517: the NAS's disks (the busiest and each) and the pool's traffic as series; the
    /// process's NAS calls in flight and missed sampled each second.
    #[test]
    fn the_nas_figures_as_series() {
        let s = Store::new(None, "m4");
        let t = now_s();
        s.record_disks(t, &[40.0, 72.0, f64::NAN]);
        assert_eq!(s.latest(NAS_DISK_BUSY, t, 5), Some(72.0));
        assert_eq!(s.latest(&format!("{NAS_DISK_BUSY}.2"), t, 5), Some(72.0));
        assert_eq!(s.latest(&format!("{NAS_DISK_BUSY}.3"), t, 5), None, "a disk not read: no measure");
        let mut tr = [0.0; 7];
        tr[2] = 3.5;
        s.record_pool(t, &tr);
        assert_eq!(s.latest("pool_nas_write_mbs", t, 5), Some(3.5));
        let mut r = Rates::default();
        let home = tempfile::tempdir().unwrap();
        let guard = super::super::memguard::Sampler::new(2);
        r.sample(&s, home.path(), &guard, t);
        r.sample(&s, home.path(), &guard, t + 1);
        assert!(s.latest(NAS_CALLS, t + 1, 5).is_some());
        assert!(s.latest(NAS_MISSED, t + 1, 5).is_some(), "missed over the second, from the second read");
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

    /// Task #516: the proxy's counts answered as `net`, and its bytes as rates by the second.
    #[test]
    fn the_proxys_traffic_is_answered_and_sampled_as_rates() {
        use crate::budget::net::{Net, Outcome};
        let s = Store::new(None, "m4");
        let t0 = 1_000_000_000;
        assert!(s.answer(&Ask::default(), t0).get("net").is_none());
        let n = Arc::new(Net::default());
        s.set_net(n.clone());
        let mut r = Rates::default();
        r.net(&s, t0);
        n.host("a.example").bytes_in.fetch_add(3 << 20, Ordering::Relaxed);
        n.host("b.example").bytes_out.fetch_add(1 << 20, Ordering::Relaxed);
        n.ended("a.example", (3 << 20, 0), 900, Outcome::Ok);
        r.net(&s, t0 + 2);
        let v = s.answer(&Ask { range: Some(60), ..Default::default() }, t0 + 2);
        assert_eq!(v["series"][NET_IN][0][2], 1.5);
        assert_eq!(v["series"][NET_OUT][0][2], 0.5);
        assert_eq!(v["net"]["hosts"]["a.example"]["in"], 3 << 20);
        assert_eq!(v["net"]["events"][0]["ms"], 900);
        // (Asked without: the series alone.)
        let v = s.answer(&Ask { range: Some(60), net: Some(false), ..Default::default() }, t0 + 2);
        assert!(v.get("net").is_none());
        assert_eq!(v["series"][NET_IN][0][2], 1.5);
    }

    /// Task #519: an answer says the Mac's size, the charts' scales.
    #[test]
    fn an_answer_says_the_macs_size() {
        let v = Store::new(None, "m1").answer(&Ask::default(), 1_000_000_000);
        assert!(v["mac"]["cores"].as_u64().is_some_and(|c| c >= 1));
        assert!(v["mac"]["cores_e"].as_u64().is_some_and(|e| e <= v["mac"]["cores"].as_u64().unwrap()));
    }
}
