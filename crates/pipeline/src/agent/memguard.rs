//! A job far over its memory (docs/pool.md §7.2): macOS swaps rather than kills, so a job holding
//! more than its Mac has leaves the Mac swapping for as long as it runs, every other job and the
//! owner's own work with it.
//!
//! - **Sampled on a thread of its own** (`Sampler`, every `EVERY`, whatever the agent's loop is
//!   waiting on: a share that stalls stalls the loop, not this): each running job's processes'
//!   physical footprints summed (crate::sys::footprint_of_group), kept per target as the most the
//!   job held while that target was under way (`Watch::current`).
//! - **Learned whatever the switch:** what a job held while a target was under way is that target's
//!   **floor** (`Floor`: crate::coord's floors), the same kind of figure as a run's own measure (a
//!   job's processes together, sampled), which takes its place.
//! - **Guarded, the switch on** (`on`, `SWITCH`): while the jobs on a Mac hold more together than its
//!   limit (`limit_mb`), by measure alone (`decide`): the job beside the largest stops at its next
//!   safe point when the largest fits alone; the largest, past the limit by itself, stops at once
//!   only while the Mac is in trouble (`trouble`: the kernel's memory pressure at warning or worse,
//!   or a GB more swap since the job began; the thread freezes it then, at once, the agent's loop
//!   stopping it), else at its next safe point too. A Mac whose memory can't be read guards nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The switch, on the NAS: `off` in it turns the guard off, `on` on; missing, the guard is as
/// `DEFAULT` says.
pub const SWITCH: &str = "state/pool/memory-guard";

/// Whether the guard stops jobs when its switch is missing (the owner's choice: on).
pub const DEFAULT: bool = true;

/// How often the jobs' memory is sampled.
pub const EVERY: Duration = Duration::from_secs(5);

/// The swap that, grown within `SWAP_WINDOW`, takes the Mac to be in trouble.
const SWAP_GROWTH: u64 = 1 << 30;
/// The window swap growth is judged over: recent, so the owner's own use growing swap hours ago
/// doesn't leave a long job in trouble for good.
pub const SWAP_WINDOW: Duration = Duration::from_secs(300);

/// The guard as the switch at `root` says: on or off; None when it can't be read now (the share not
/// answering: the caller keeps what it had).
pub fn on(root: &Path) -> Option<bool> {
    match std::fs::read_to_string(root.join(SWITCH)) {
        Ok(s) => Some(match s.trim() {
            "off" => false,
            "on" => true,
            _ => DEFAULT,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(DEFAULT),
        Err(_) => None,
    }
}

/// The most memory the jobs on a Mac of `total_mb` may hold together (MB): its memory less an eighth
/// for macOS and the owner's own work, 4 GB at least (42 GB of the M4's 48, 12 of the M1's 16); 0 for
/// a Mac whose memory isn't known (the guard then guards nothing).
pub fn limit_mb(total_mb: u64) -> u64 {
    total_mb.saturating_sub((total_mb / 8).max(4096))
}

/// What the guard does about the jobs in a Mac's slots, by what each holds now (MB; None: no job in
/// that slot, or its memory unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Nothing: the jobs fit the limit together.
    Nothing,
    /// Slot k's job stops at its next safe point: the job beside it fits the limit alone.
    Drain(usize),
    /// Slot k's job holds more than the limit by itself.
    Over(usize),
}

/// The guard's decision (`Act`), by measure alone: nothing while the jobs together fit `limit`;
/// else the largest when it alone passes it, or the others drained (each in turn: the first not
/// draining yet, `draining`) when it fits alone.
pub fn decide(held: &[Option<u64>], limit: u64, draining: &[bool]) -> Act {
    let total: u64 = held.iter().flatten().sum();
    if limit == 0 || total <= limit {
        return Act::Nothing;
    }
    let Some((big, mb)) = held.iter().enumerate().filter_map(|(k, m)| m.map(|m| (k, m))).max_by_key(|&(k, m)| (m, std::cmp::Reverse(k))) else { return Act::Nothing };
    if mb > limit {
        return Act::Over(big);
    }
    match held.iter().enumerate().find(|&(k, m)| k != big && m.is_some() && !draining.get(k).copied().unwrap_or(false)) {
        Some((k, _)) => Act::Drain(k),
        None => Act::Nothing,
    }
}

/// Whether the Mac is in trouble for its memory: the kernel's memory pressure at warning or worse
/// (`pressure`), or a GB more swap in use (`swap`) than the least in use in the last few minutes
/// (`swap_low`, over `SWAP_WINDOW`).
pub fn trouble(pressure: Option<u32>, swap: Option<u64>, swap_low: Option<u64>) -> bool {
    pressure.is_some_and(|p| p >= 2) || matches!((swap, swap_low), (Some(now), Some(low)) if now >= low.saturating_add(SWAP_GROWTH))
}

/// The target a job is on, as its costs file says (`SCENIC_COSTS`: a `started` line as each target
/// begins, a `peak_mb` line as it ends): the last begun and not ended, by its cost key
/// (crate::coord::cost_key); None when the file says none (a step that notes no costs, or between
/// targets).
pub fn current(costs: &Path) -> Option<String> {
    let s = std::fs::read_to_string(costs).ok()?;
    let mut on: Option<String> = None;
    for l in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let Some(key) = v["unit"].as_str() else { continue };
        if v.get("started").is_some() {
            on = Some(key.to_string());
        } else if v.get("peak_mb").is_some() && on.as_deref() == Some(key) {
            on = None;
        }
    }
    on
}

/// What a target takes at least (crate::coord's floors): the most a job held while it was under way
/// (MB); whether that job held it alone (one target: a batch's caches, filled over its earlier
/// targets, aren't the last's, so only a floor learned alone holds a target off a Mac); and the way
/// its step ran then (crate::coord::cost_version).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "FloorRead")]
pub struct Floor {
    pub mb: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alone: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub v: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// A floor as read: as written now, or a bare MB (a build before floors said more), learned in a
/// batch the way its step ran first.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum FloorRead {
    Mb(u64),
    Full {
        mb: u64,
        #[serde(default)]
        alone: bool,
        #[serde(default)]
        v: u32,
    },
}

impl From<FloorRead> for Floor {
    fn from(r: FloorRead) -> Floor {
        match r {
            FloorRead::Mb(mb) => Floor { mb, alone: false, v: 0 },
            FloorRead::Full { mb, alone, v } => Floor { mb, alone, v },
        }
    }
}

/// The step of a cost key (crate::coord::cost_key): a unit's is its target alone.
pub fn step_of_key(key: &str) -> &str {
    match key.split_once(' ') {
        Some((s, _)) => s,
        None => "unit",
    }
}

/// A running job as the guard watches it.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    pub pgid: i32,
    /// Its costs file (`SCENIC_COSTS`) and where it notes its targets done (`SCENIC_DONE`).
    pub costs: PathBuf,
    pub done: PathBuf,
    pub step: String,
    pub targets: Vec<String>,
    /// The cost key of a job of no targets of its own (the OSM pass: `osm-pass <date>`), else None.
    pub own_key: Option<String>,
    /// The most it held while each target was under way (MB, by cost key), and what it holds now.
    pub seen: BTreeMap<String, u64>,
    pub held: Option<u64>,
    /// Whether the guard froze it (the Mac in trouble with it past the limit alone: the agent's loop
    /// stops it).
    pub frozen: bool,
}

impl Watch {
    /// The cost key of the target it's on: the last its costs file says began and hasn't ended;
    /// else the first of its targets it hasn't noted done; else its own (`own_key`).
    pub fn current(&self) -> Option<String> {
        if let Some(k) = current(&self.costs) {
            return Some(k);
        }
        if !self.targets.is_empty() {
            let done = crate::control::read_done(&self.done, &self.step);
            return self.targets.iter().find(|t| !done.contains(*t)).map(|t| crate::coord::cost_key(&self.step, t));
        }
        self.own_key.clone()
    }

    /// The floors its run teaches (cost key → floor), from what it held while each target was under
    /// way, but for the targets its run measured itself (`measured`: a measure takes a floor's place).
    pub fn floors(&self, measured: &[String]) -> Vec<(String, Floor)> {
        let alone = self.targets.len() <= 1;
        let v = crate::coord::cost_version(&self.step);
        self.seen.iter().filter(|(k, mb)| !measured.contains(k) && **mb > 0).map(|(k, mb)| (k.clone(), Floor { mb: *mb, alone, v })).collect()
    }
}

/// What the sampler's thread and the agent's loop share.
#[derive(Default)]
struct Inner {
    watches: Vec<Option<Watch>>,
    on: bool,
    limit_mb: u64,
    /// As a test sets them: what each slot's job holds (MB), and whether the Mac is in trouble.
    held_set: Option<Vec<Option<u64>>>,
    trouble_set: Option<bool>,
    /// What the thread did last (unix seconds, in words), for the agent's loop to say.
    froze: Vec<(u64, String)>,
    /// The swap in use as each sample read it, over the last `SWAP_WINDOW`.
    swap: std::collections::VecDeque<(std::time::Instant, u64)>,
}

/// The guard's sampler: a thread sampling the watched jobs every `EVERY` (`spawn`; a test calls
/// `tick` itself), freezing one past the limit alone while the Mac is in trouble.
#[derive(Clone, Default)]
pub struct Sampler {
    inner: Arc<Mutex<Inner>>,
}

impl Sampler {
    pub fn new(slots: usize) -> Sampler {
        Sampler { inner: Arc::new(Mutex::new(Inner { watches: vec![None; slots], ..Default::default() })) }
    }

    /// Starts its thread (once a process: the agent's).
    pub fn spawn(&self) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("memory-guard".into())
            .spawn(move || loop {
                me.tick();
                std::thread::sleep(EVERY);
            })
            .ok();
    }

    /// Watches slot `k`'s job, from its start.
    pub fn watch(&self, k: usize, w: Watch) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(s) = g.watches.get_mut(k) {
                *s = Some(w);
            }
        }
    }

    /// Stops watching slot `k`'s job (it ended): what it saw.
    pub fn unwatch(&self, k: usize) -> Option<Watch> {
        self.inner.lock().ok()?.watches.get_mut(k)?.take()
    }

    /// Whether it guards (the switch, as the agent read it) and this Mac's limit (MB; 0: unknown).
    pub fn set(&self, on: bool, limit_mb: u64) {
        if let Ok(mut g) = self.inner.lock() {
            (g.on, g.limit_mb) = (on, limit_mb);
        }
    }

    /// A test's figures: what each slot's job holds (MB), and whether the Mac is in trouble.
    pub fn set_test(&self, held: Option<Vec<Option<u64>>>, trouble: Option<bool>) {
        if let Ok(mut g) = self.inner.lock() {
            (g.held_set, g.trouble_set) = (held, trouble);
        }
    }

    /// What slot `k`'s job holds now (MB), as last sampled.
    pub fn held(&self, k: usize) -> Option<u64> {
        self.inner.lock().ok()?.watches.get(k)?.as_ref()?.held
    }

    /// Whether the thread froze slot `k`'s job.
    pub fn frozen(&self, k: usize) -> bool {
        self.inner.lock().ok().and_then(|g| g.watches.get(k).and_then(|w| w.as_ref().map(|w| w.frozen))).unwrap_or(false)
    }

    /// Lets slot `k`'s job go on, if the thread froze it (the guard turned off since).
    pub fn thaw(&self, k: usize) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut).filter(|w| w.frozen) else { return false };
        w.frozen = false;
        crate::sys::signal_group(w.pgid, crate::sys::Signal::Cont)
    }

    /// Whether the Mac is in trouble now (`trouble`: its pressure, or swap grown within the last few
    /// minutes, as the samples read it).
    pub fn in_trouble(&self) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        if let Some(t) = g.trouble_set {
            return t;
        }
        let now = crate::sys::swap_used();
        if let Some(n) = now {
            g.swap.push_back((std::time::Instant::now(), n));
        }
        while g.swap.front().is_some_and(|(t, _)| t.elapsed() > SWAP_WINDOW) {
            g.swap.pop_front();
        }
        let low = g.swap.iter().map(|(_, n)| *n).min();
        drop(g);
        trouble(crate::sys::memory_pressure(), now, low)
    }

    /// What the thread did since the last call, for the agent's loop to note.
    pub fn take_froze(&self) -> Vec<(u64, String)> {
        self.inner.lock().map(|mut g| std::mem::take(&mut g.froze)).unwrap_or_default()
    }

    /// One sample: each watched job's memory, kept against its target under way; a job past the
    /// limit alone, while the Mac is in trouble, frozen at once (the agent's loop stops it).
    pub fn tick(&self) {
        let watches: Vec<(usize, i32)> = match self.inner.lock() {
            Ok(g) => g.watches.iter().enumerate().filter_map(|(k, w)| w.as_ref().map(|w| (k, w.pgid))).collect(),
            Err(_) => return,
        };
        let set = self.inner.lock().ok().and_then(|g| g.held_set.clone());
        // (Read outside the lock: the footprints and the files take time.)
        let mut got: Vec<(usize, i32, Option<u64>, Option<String>)> = Vec::new();
        // (Each sample reads the swap too: its window's least, for `in_trouble`.)
        self.in_trouble();
        for (k, pgid) in watches {
            let held = match &set {
                Some(v) => v.get(k).copied().flatten(),
                None => crate::sys::footprint_of_group(pgid).map(|b| b >> 20),
            };
            let key = self.inner.lock().ok().and_then(|g| g.watches.get(k).and_then(|w| w.clone())).and_then(|w| w.current());
            got.push((k, pgid, held, key));
        }
        let over: Vec<usize> = {
            let Ok(mut g) = self.inner.lock() else { return };
            for (k, pgid, held, key) in &got {
                // (Only the job read: another started in its slot meanwhile takes none of it.)
                let Some(w) = g.watches.get_mut(*k).and_then(Option::as_mut).filter(|w| w.pgid == *pgid) else { continue };
                w.held = *held;
                if let (Some(mb), Some(key)) = (held, key) {
                    let e = w.seen.entry(key.clone()).or_insert(0);
                    *e = (*e).max(*mb);
                }
            }
            let limit = g.limit_mb;
            if !g.on || limit == 0 {
                return;
            }
            g.watches.iter().enumerate().filter_map(|(k, w)| w.as_ref().filter(|w| !w.frozen && w.held.is_some_and(|m| m > limit)).map(|_| k)).collect()
        };
        for k in over {
            if !self.in_trouble() {
                continue;
            }
            let Ok(mut g) = self.inner.lock() else { return };
            let (limit, real) = (g.limit_mb, g.held_set.is_none());
            let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut) else { continue };
            if real {
                crate::sys::signal_group(w.pgid, crate::sys::Signal::Stop);
            }
            w.frozen = true;
            let what = format!("froze slot {k}'s job: it held {:.1} GB, past this Mac's limit of {:.1} GB, the Mac short of memory", w.held.unwrap_or(0) as f64 / 1024.0, limit as f64 / 1024.0);
            eprintln!("agent: memory guard {what}");
            g.froze.push((crate::agent::jobs::now_s(), what));
        }
    }
}

/// The guard in the status: its switch, this Mac's limit (0: its memory unknown, guarding nothing),
/// what the jobs hold now, and what it last did (when, unix seconds, and in words).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub on: bool,
    pub limit_mb: u64,
    pub held_mb: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_off: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<(u64, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_the_macs_memory_less_an_eighth() {
        assert_eq!(limit_mb(48 << 10), 42 << 10);
        assert_eq!(limit_mb(16 << 10), 12 << 10);
        assert_eq!(limit_mb(64 << 10), 56 << 10);
        assert_eq!(limit_mb(0), 0, "its memory unknown");
    }

    #[test]
    fn decisions_by_measure_alone() {
        let l = 42 << 10;
        assert_eq!(decide(&[Some(30 << 10), Some(12 << 10)], l, &[false, false]), Act::Nothing);
        assert_eq!(decide(&[None, None], l, &[false, false]), Act::Nothing);
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, false]), Act::Drain(1));
        assert_eq!(decide(&[Some(10 << 10), Some(35 << 10)], l, &[false, false]), Act::Drain(0));
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, true]), Act::Nothing);
        assert_eq!(decide(&[Some(43 << 10), Some(1 << 10)], l, &[false, true]), Act::Over(0));
        assert_eq!(decide(&[None, Some(50 << 10)], l, &[false, false]), Act::Over(1));
        assert_eq!(decide(&[Some(25 << 10), Some(25 << 10)], l, &[false, false]), Act::Drain(1));
        // A limit unknown: nothing, however much.
        assert_eq!(decide(&[Some(50 << 10)], 0, &[false]), Act::Nothing);
    }

    #[test]
    fn trouble_is_the_kernels_pressure_or_swap_grown() {
        // (Floors read as written before they said more: a bare MB.)
        let old: BTreeMap<String, Floor> = serde_json::from_str(r#"{"a": 9000, "b": {"mb": 5, "alone": true, "v": 3}}"#).unwrap();
        assert_eq!((old["a"], old["b"]), (Floor { mb: 9000, alone: false, v: 0 }, Floor { mb: 5, alone: true, v: 3 }));
        assert!(!trouble(Some(1), Some(5 << 30), Some(5 << 30)));
        assert!(trouble(Some(2), None, None) && trouble(Some(4), None, None));
        assert!(trouble(Some(1), Some(6 << 30), Some(5 << 30)));
        assert!(!trouble(Some(1), Some((5 << 30) + (512 << 20)), Some(5 << 30)));
        assert!(!trouble(None, None, None));
    }

    #[test]
    fn the_target_under_way() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("costs.jsonl");
        assert_eq!(current(&p), None);
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n").unwrap();
        assert_eq!(current(&p).as_deref(), Some("terrain 6/1/1"));
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n{\"unit\":\"terrain 6/1/1\",\"peak_mb\":900,\"secs\":3}\n").unwrap();
        assert_eq!(current(&p), None);
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n{\"unit\":\"terrain 6/1/1\",\"peak_mb\":900,\"secs\":3}\n{\"unit\":\"terrain 6/1/2\",\"started\":4}\nnot json\n").unwrap();
        assert_eq!(current(&p).as_deref(), Some("terrain 6/1/2"));
        // Its fallbacks: between targets, or a step that notes no costs, the first target not
        // noted done; a job of no targets, its own key; else none.
        let done = d.path().join("done.txt");
        let mut w = Watch { costs: d.path().join("none.jsonl"), done: done.clone(), step: "pack".into(), targets: vec!["6/1/1".into(), "6/1/2".into()], ..Default::default() };
        assert_eq!(w.current().as_deref(), Some("pack 6/1/1"));
        std::fs::write(&done, "pack 6/1/1\n").unwrap();
        assert_eq!(w.current().as_deref(), Some("pack 6/1/2"));
        std::fs::write(&done, "pack 6/1/1\npack 6/1/2\n").unwrap();
        assert_eq!(w.current(), None);
        w.targets.clear();
        w.own_key = Some("osm-pass 2026-09-28".into());
        assert_eq!(w.current().as_deref(), Some("osm-pass 2026-09-28"));
        w.own_key = None;
        assert_eq!(w.current(), None);
    }

    #[test]
    fn floors_from_what_a_run_held_but_what_it_measured() {
        let w = Watch { step: "terrain".into(), targets: vec!["6/1/1".into(), "6/1/2".into()], seen: [("terrain 6/1/1".to_string(), 9000), ("terrain 6/1/2".to_string(), 7000)].into(), ..Default::default() };
        let v = crate::coord::cost_version("terrain");
        assert_eq!(w.floors(&["terrain 6/1/2".to_string()]), [("terrain 6/1/1".to_string(), Floor { mb: 9000, alone: false, v })]);
        let w = Watch { step: "water".into(), targets: vec!["water".into()], seen: [("water water".to_string(), 33000)].into(), ..Default::default() };
        assert_eq!(w.floors(&[]), [("water water".to_string(), Floor { mb: 33000, alone: true, v: 0 })]);
        assert_eq!((step_of_key("6/1/1"), step_of_key("terrain 6/1/1")), ("unit", "terrain"));
    }

    #[test]
    fn the_sampler_keeps_the_most_held_per_target_and_freezes_only_in_trouble() {
        let d = tempfile::tempdir().unwrap();
        let s = Sampler::new(2);
        s.set(true, 12 << 10);
        s.watch(0, Watch { pgid: 0, costs: d.path().join("c.jsonl"), done: d.path().join("d.txt"), step: "pack".into(), targets: vec!["6/1/1".into()], ..Default::default() });
        s.set_test(Some(vec![Some(5000), None]), Some(false));
        s.tick();
        s.set_test(Some(vec![Some(3000), None]), Some(false));
        s.tick();
        assert_eq!(s.held(0), Some(3000));
        // Past the limit alone, the Mac not in trouble: not frozen.
        s.set_test(Some(vec![Some(13 << 10), None]), Some(false));
        s.tick();
        assert!(!s.frozen(0));
        // In trouble: frozen, said.
        s.set_test(Some(vec![Some(13 << 10), None]), Some(true));
        s.tick();
        assert!(s.frozen(0) && s.take_froze().len() == 1);
        let w = s.unwatch(0).unwrap();
        assert_eq!(w.seen.get("pack 6/1/1"), Some(&(13 << 10)));
        // A new job in the slot (another process group) starts with nothing of the last's.
        s.watch(0, Watch { pgid: 7, costs: d.path().join("c2.jsonl"), done: d.path().join("d3.txt"), step: "pack".into(), targets: vec!["6/1/3".into()], ..Default::default() });
        s.set_test(Some(vec![Some(100), None]), Some(false));
        s.tick();
        let w = s.unwatch(0).unwrap();
        assert!(!w.frozen && w.seen.get("pack 6/1/1").is_none() && w.seen.get("pack 6/1/3") == Some(&100));
        // Off, or the limit unknown: learning only.
        s.watch(1, Watch { pgid: 0, step: "pack".into(), targets: vec!["6/2/2".into()], done: d.path().join("d2.txt"), ..Default::default() });
        s.set(true, 0);
        s.set_test(Some(vec![None, Some(50 << 10)]), Some(true));
        s.tick();
        assert!(!s.frozen(1) && s.unwatch(1).unwrap().seen.get("pack 6/2/2") == Some(&(50 << 10)));
    }

    #[test]
    fn the_switch() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on(d.path()), Some(DEFAULT));
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(SWITCH), "off\n").unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::write(d.path().join(SWITCH), "on").unwrap();
        assert_eq!(on(d.path()), Some(true));
    }
}
