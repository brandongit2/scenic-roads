//! The pool's simulator (docs/pool.md §13): two to four Macs running the pool's driver
//! (crate::pool::driver), the code the agent will run, over a model of the NAS's faults, on a
//! schedule drawn from a seed, with the invariants of §4 checked at every step that could break
//! one, and its progress at the end.
//!
//! The NAS's model (`World`): create-new is atomic on the server, and the file's bytes land a step
//! later (a Mac asleep in between leaves it empty meanwhile), or never (`Cfg::cuts`: the share
//! gone between, the file the creator's to finish); a whole write is its temporary file, then its
//! rename a step later (a Mac asleep in between renames when it wakes, over whatever others wrote
//! meanwhile); a rename over a file another Mac has open fails busy, and the write fails after a
//! few tries; a create or a whole write that did its work may answer an error (`Cfg::cuts`: its
//! answer lost); each Mac keeps what it read, stat'ed and listed for up to `Cfg::stale` seconds, so
//! it may read an older version, miss a new file, or list a folder as it was; its own writes it
//! sees at once; a listing may take its time (`Cfg::list_s`), the Mac waiting on it, awake.
//!
//! Each Mac runs in a thread of its own, but one at a time: every NAS operation, message and look
//! at a clock waits for the Mac's turn, and after each the schedule (`World::after`) says whose
//! turn is next, puts the Mac to sleep there or not, and brings the owner's changes: a newer app on
//! a Mac (a restart: its memory gone but what its driver saved), or a development build or a
//! rollback (`Cfg::downgrade`); a member may leave for good (`Cfg::leave`). Mac 0 is the declared
//! lead (task #881): no other Mac may lead. The Macs play the agent's part around the driver
//! (`Mac`): its jobs' hand-offs, its coordinator's grants (while the driver lets it) and its store's
//! epoch (bumped at each restart, raised past a take-up's records: docs/coordination.md step 8c),
//! the listings the driver asks for, off its step, its heartbeat, stamped as it's written, a job
//! reading the records. Once the faults stop every Mac stays awake for a while; then the declared
//! lead must lead, alone, its records at the one path must name every entry ever written, and no
//! take-up may have been made in the run's last ten minutes. Every entry a lead acknowledged is in
//! every snapshot of its epoch or a later one.

mod place;

use super::beat::Beat;
use super::driver::{self, Driver, Event, Heard, Io, Listed, Out};
use super::journal::{self, Entry, LeaseId};
use super::nas::{Created, Nas};
use super::records::{self, Records};
use super::{lead, Member};
use anyhow::{anyhow, bail, Result};
use buildstore::handoff::Handoff;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// The longest a read keeps its file open (s): a rename over it meanwhile fails busy.
const OPEN: u64 = 8;
/// How many times the model's whole write tries its rename.
const BUSY_TRIES: u32 = 4;
/// A Mac's loop: every 20 s, and a little.
const LOOP_S: u64 = 20;
/// The clocks' start: half an hour before a day ends (the journal has two days).
const T0: u64 = 1_791_331_200 - 1_800;
/// The units the jobs build, again and again.
const TARGETS: [&str; 5] = ["6-1-0", "6-1-1", "6-1-2", "6-1-3", "6-1-4"];
/// A development build's app number.
const DEV: u64 = u64::MAX;

/// Per step of a Mac while the faults last: that it falls asleep right there; and right after it
/// wrote a temporary file or made a file with create-new, the moments a sleep does the most (a
/// rename landing late, a file empty meanwhile).
const P_SLEEP: f64 = 0.004;
const P_SLEEP_MIDWAY: f64 = 0.05;
/// Per step: that the owner installs a newer app on a Mac.
const P_APP: f64 = 0.0004;
/// With `Cfg::cuts`: per create, that its bytes don't land (the share gone between); per create
/// and whole write, that its answer is lost (done, said failed).
const P_CUT: f64 = 0.03;
const P_LOST: f64 = 0.01;
/// With `Cfg::truncs`: per entry's rename, that its file lands cut short for good.
const P_TRUNC: f64 = 0.05;
/// With `Cfg::fails`: per entry's rename, that its file errors on every read for good (EIO), the
/// share answering every other; and per step of a Mac while the faults last, that the share
/// answers none of its reads, stats and listings for a while (1 to 10 minutes), its writes landing.
const P_EIO: f64 = 0.02;
const P_AWAY: f64 = 0.001;
/// With `Cfg::stalls`: per operation while the faults last, that the share stalls on it (5 to 10
/// minutes, its Mac waiting, awake).
const P_STALL: f64 = 0.0001;
/// With `Cfg::lost_lists`: per listing asked for while the faults last, that its agent loses it.
const P_LOSE_LIST: f64 = 0.1;
/// Per step: that the turn stays with the Mac that has it.
const P_KEEP: f64 = 0.85;
/// Per loop of a Mac while the faults last: that one of its jobs ends (a journal entry); that one
/// reads the current records; leading, that its coordinator grants a job, or that its agent is
/// about to sweep (GC), which needs its step's read of the declaration.
const P_JOB: f64 = 0.35;
const P_READ: f64 = 0.2;
const P_GRANT: f64 = 0.3;
const P_SWEEP: f64 = 0.01;
/// How long the run's end must have seen no take-up (s).
const QUIET_S: u64 = 600;
/// At most how long a run goes on past its end while its lead still merges what the
/// faults left, or isn't caught up yet (s): a share taking seconds an operation, a lead's minute of
/// reads a loop merges a dozen entries, and hours of faults with the lead asleep leave hundreds; a listing
/// its agent lost as the faults end is asked for again two hours on, and an entry never whole that
/// only it finds is refused an hour after.
const MERGING_S: u64 = driver::OVERDUE_S + driver::UNREADABLE_S + 3600;

/// What a run is.
#[derive(Clone, Copy, Debug)]
struct Cfg {
    /// The faults stop this long into the run (s); every Mac then stays awake.
    faults: u64,
    /// The run ends this long into it, or goes on (`World::goes_on`) while its lead still merges
    /// what the faults left.
    end: u64,
    /// The longest a Mac keeps what it read, stat'ed or listed (s).
    stale: u64,
    /// How long a listing of a folder takes (s), lo..=hi; (0, 0): no time.
    list_s: (u64, u64),
    /// A member leaves for good (never wakes) partway through.
    leave: bool,
    /// Earlier days in the journal (a week's), an entry each that today's records lack.
    old_days: u64,
    /// The clocks' skew, up to this either way (s); 0: up to one second.
    skew: u64,
    /// Now and then the owner runs a development build on a Mac, or rolls its app back.
    downgrade: bool,
    /// Creates whose bytes don't land, and answers lost.
    cuts: bool,
    /// Every create, read, stat and whole write taking this long (s), lo..=hi, its Mac waiting on
    /// it, awake: the share under load; (0, 0): no time.
    op_s: (u64, u64),
    /// Now and then a restart without the state its driver saved (lost, or a copy's).
    lose: bool,
    /// An entry another Mac renamed into place isn't there to a Mac's reads for up to this long
    /// after (s; directory caching): its lead reads it not whole, and reads it again.
    lag: u64,
    /// Now and then an entry's file loses its last bytes on the share for good (a NAS losing a
    /// renamed file's last bytes): never read whole, its lead refuses it after an hour.
    truncs: bool,
    /// Reads that fail: an entry's file erroring on every read for good (its lead refuses it after
    /// an hour), and a Mac whose reads the share doesn't answer for a while.
    fails: bool,
    /// Now and then an operation the share stalls on for minutes: its step minutes long.
    stalls: bool,
    /// Now and then a listing its agent loses (its listing's thread gone), never handed back: its
    /// lead asks again once it's overdue (`driver::OVERDUE_S`).
    lost_lists: bool,
}

impl Cfg {
    /// The pool's, none of the knobs on.
    fn pool() -> Cfg {
        Cfg {
            faults: 2400,
            end: 3900,
            stale: 30,
            list_s: (0, 0),
            leave: false,
            old_days: 0,
            skew: 0,
            downgrade: false,
            cuts: false,
            op_s: (0, 0),
            lose: false,
            lag: 0,
            truncs: false,
            fails: false,
            stalls: false,
            lost_lists: false,
        }
    }

    /// The default mix: the knobs, drawn from the seed.
    fn mixed(seed: u64) -> Cfg {
        let mut r = Rng(seed ^ 0x6B6E_6F62_735F_6D69);
        let mut c = Cfg::pool();
        if r.chance(0.3) {
            c.list_s = (3, 33);
        }
        if r.chance(0.2) {
            c.old_days = 7;
        }
        c.leave = r.chance(0.25);
        if r.chance(0.25) {
            c.skew = r.range(30, 1200);
        }
        if r.chance(0.15) {
            c.stale = r.range(60, 300);
        }
        c.downgrade = r.chance(0.25);
        c.cuts = r.chance(0.5);
        if r.chance(0.2) {
            c.op_s = (1, 4);
        }
        c.lose = r.chance(0.25);
        if r.chance(0.25) {
            c.lag = r.range(30, 600);
        }
        c.truncs = r.chance(0.15);
        c.fails = r.chance(0.2);
        c.stalls = r.chance(0.15);
        c.lost_lists = r.chance(0.15);
        c
    }
}

/// splitmix64.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64) < p * (1u64 << 53) as f64
    }
}

/// What reaches a Mac (the pool's HTTP API: only while it's awake): a member's message.
#[derive(Clone, Debug)]
struct Msg {
    from: usize,
    msg: driver::Msg,
}

/// A Mac as the world sees it.
struct MacW {
    app: u64,
    skew: i64,
    asleep_until: u64,
    /// The seconds it slept (each sleep counted as it begins; cut short at the faults' end): its
    /// awake clock is the run's time less them.
    slept: u64,
    /// Waiting on a listing until then, awake.
    busy_until: u64,
    /// Its next loop (it waits between loops).
    ready_at: u64,
    inbox: Vec<Msg>,
    /// What it says it leads, checked: the epoch it leads in, the highest it led in.
    leads: Option<u64>,
    led: u64,
    /// Its store's epoch (its coordinator's: driver::Io::epoch), kept across restarts, one more at
    /// each.
    store: u64,
    /// Left for good: it never wakes.
    gone: bool,
    /// Its last sleep (from, to); when it last woke from one its driver sees as a sleep.
    sleep: (u64, u64),
    woke: u64,
    /// When its last operation ran (the end of its driver's step, to the first after it).
    op_at: u64,
    /// Its driver's last step said it leads, its records caught up.
    caught: bool,
    /// The records its driver kept on its own disk leading (driver::Io::keep): through restarts,
    /// lost with its saved state.
    local: Option<Vec<u8>>,
}

impl MacW {
    /// When it last woke, by `t`, from a sleep its driver sees as one (over `GAP_S`, its clocks
    /// apart): what its members told it meanwhile was lost.
    fn woke(&self, t: u64) -> u64 {
        let (from, to) = self.sleep;
        if to <= t && to - from > driver::GAP_S + 10 {
            self.woke.max(to)
        } else {
            self.woke
        }
    }
}

/// What a Mac keeps of what it read, stat'ed and listed: the bytes (None: no file) or names, and
/// until when.
#[derive(Default)]
struct Cache {
    files: BTreeMap<String, (Option<Vec<u8>>, u64)>,
    dirs: BTreeMap<String, (Vec<String>, u64)>,
}

/// What a journal entry is to the checks: one the lead's check takes, one it refuses (a step that
/// doesn't exist), or one the lead of an odd epoch refuses (a check that depends on the lead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Taken,
    Refused,
    Either,
}

/// Whether records name entry `k` as what it is: taken, refused, or either.
fn names(r: &Records, k: &str, kind: Kind) -> bool {
    match kind {
        Kind::Taken => r.reflected.contains(k),
        Kind::Refused => r.rejected.contains_key(k),
        Kind::Either => r.handles(k),
    }
}

/// The simulated world: the NAS's files, the Macs, the schedule, and what's checked.
struct World {
    cfg: Cfg,
    t: u64,
    rng: Rng,
    files: BTreeMap<String, Vec<u8>>,
    /// Who has each file open, until when (its creator, until its bytes land).
    open: BTreeMap<String, BTreeMap<usize, u64>>,
    /// When each temporary file was written, and every file's last writer and when.
    tmps: BTreeMap<String, u64>,
    writes: BTreeMap<String, (usize, u64)>,
    caches: Vec<Cache>,
    macs: Vec<MacW>,
    /// The step just made was a temporary file's write or a create.
    midway: bool,
    turn: usize,
    done: bool,
    drained: bool,
    /// When the run ends, gone on past its config's end while the lead merges; and how many
    /// entries the one path's records lacked when it last went on.
    end: u64,
    behind: Option<usize>,
    /// The take-ups, by epoch: by which Mac, and when; the Mac that led each epoch.
    took: BTreeMap<u64, (usize, u64)>,
    leaders: BTreeMap<u64, usize>,
    /// Each epoch's last snapshot landed at the one path: its number, and the entries it names.
    landed: BTreeMap<u64, (u64, BTreeSet<String>)>,
    /// Every entry whose bytes landed whole, what it is, and when; those a lead acknowledged, in
    /// which epoch.
    written: BTreeMap<String, Kind>,
    written_at: BTreeMap<String, u64>,
    writers: BTreeMap<String, usize>,
    acked: BTreeMap<String, u64>,
    /// The leases granted in each epoch (the lead grants in order: the coordinator isn't modelled).
    leases: BTreeMap<u64, u64>,
    /// The epochs whose lead was seen caught up, and by the listing begun when (its clock).
    caught: BTreeSet<(u64, u64)>,
    /// The entries refused as never read whole.
    unreadable: BTreeSet<String>,
    /// Entries whose files error on every read (`Cfg::fails`), and until when the share answers
    /// none of each Mac's reads.
    erroring: BTreeSet<String>,
    away: Vec<u64>,
    wrong: Vec<String>,
    counts: Counts,
    trace: Option<Vec<String>>,
}

/// The app version of number `n`: a minute later each; a development build's.
fn app(n: u64) -> String {
    if n == DEV {
        return "development".into();
    }
    format!("20261006-{:02}{:02}-{n:07x}", 12 + n / 60, n % 60)
}

fn id(k: usize) -> String {
    format!("m-{:016x}", k + 1)
}

fn mac_of(id: &str) -> usize {
    id.get(2..).and_then(|h| u64::from_str_radix(h, 16).ok()).map_or(usize::MAX, |n| n as usize - 1)
}

fn logical(t: &str) -> String {
    format!("sim/{t}")
}

fn content(t: &str, k: &str) -> String {
    format!("sim/{t}.{k}.x")
}

fn entry_of(path: &str) -> Option<String> {
    let k = path.strip_prefix("state/journal/")?.strip_suffix(".json")?;
    (!k.starts_with("rejected/")).then(|| k.to_string())
}

fn beat_of(path: &str) -> Option<&str> {
    path.strip_prefix("state/pool/members/")?.strip_suffix(".json")
}

/// Whether records are self-consistent (invariant 4): each unit's key and its file in the manifest
/// from the same entry, and nothing from a refused one.
fn consistent(r: &Records) -> std::result::Result<(), String> {
    for (t, k) in &r.keys.unit {
        if r.manifest.get(&logical(t)) != Some(&content(t, k)) {
            return Err(format!("{t}'s key is {k}, its file {:?}", r.manifest.get(&logical(t))));
        }
    }
    for (l, c) in &r.manifest {
        let Some(t) = l.strip_prefix("sim/") else { continue };
        if r.keys.unit.get(t).map(|k| content(t, k)).as_ref() != Some(c) {
            return Err(format!("{l} is {c}, its key {:?}", r.keys.unit.get(t)));
        }
        if c.contains(".bogus.") {
            return Err(format!("{l} is a refused entry's {c}"));
        }
    }
    Ok(())
}

/// The simulator's steps' outputs (crate::agent::steps::writes' stand-in): a unit job, and the
/// lead of an odd epoch's stand-in, write a unit's file, a prune removes one; another step changes
/// nothing.
fn writes(step: &str, l: &str, removal: bool) -> Option<bool> {
    match step {
        "unit" | "flaky" => Some(l.starts_with("sim/")),
        "prune" => Some(removal && l.starts_with("sim/")),
        _ => None,
    }
}

/// The lead's check of an entry: every change within its step's outputs, by the merge's own check
/// (crate::agent::steps::outside_by, refused by every lead); and one the lead of an odd epoch
/// refuses (a stand-in for a check by the lead's state: its leases), so one a lead refused may be
/// taken by the next.
fn check(e: &Entry, r: &Records) -> std::result::Result<(), String> {
    if let Some(why) = crate::agent::seam::for_tests().outside_by(e, &|l, removal| writes(&e.step, l, removal)) {
        return Err(why);
    }
    match e.step.as_str() {
        "flaky" if r.epoch % 2 == 1 => Err(format!("refused by epoch {}'s lead", r.epoch)),
        _ => Ok(()),
    }
}

impl World {
    fn new(seed: u64, macs: usize, cfg: Cfg, tracing: bool) -> World {
        let mut rng = Rng(seed);
        let macs: Vec<MacW> = (0..macs)
            .map(|_| {
                let skew = if cfg.skew > 0 {
                    rng.below(2 * cfg.skew + 1) as i64 - cfg.skew as i64
                } else {
                    rng.below(3) as i64 - 1
                };
                MacW {
                    app: 0,
                    skew,
                    asleep_until: 0,
                    slept: 0,
                    busy_until: 0,
                    ready_at: 0,
                    inbox: Vec::new(),
                    leads: None,
                    led: 0,
                    store: 0,
                    gone: false,
                    sleep: (0, 0),
                    woke: 0,
                    op_at: 0,
                    caught: false,
                    local: None,
                }
            })
            .collect();
        let mut files = BTreeMap::new();
        // Mac 0 the declared lead.
        files.insert(
            lead::LEAD.to_string(),
            serde_json::to_vec(&lead::Declared { member: id(0), host: "mac0".into(), since: T0 }).unwrap(),
        );
        // Today's records: one unit built.
        files.insert(
            "state/build/manifest.json".to_string(),
            serde_json::to_vec(&BTreeMap::from([(logical(TARGETS[0]), content(TARGETS[0], "k0"))])).unwrap(),
        );
        files.insert(
            "state/build/jobs.json".to_string(),
            format!("{{\"unit\": {{\"{}\": \"k0\"}}}}", TARGETS[0]).into_bytes(),
        );
        let mut written = BTreeMap::new();
        // A week of the journal's earlier days, an entry each that today's records lack.
        for d in 1..=cfg.old_days {
            let t = TARGETS[(d as usize) % TARGETS.len()];
            let k = format!("old{d}");
            let h = Handoff {
                changes: [(logical(t), Some(content(t, &k)))].into(),
                done: Some(("unit".into(), vec![(t.into(), k.clone())])),
                ..Default::default()
            };
            let e = Entry {
                member: id(0),
                lease: LeaseId { epoch: 0, n: d },
                step: "unit".into(),
                handoff: h,
                at: T0 - d * 86_400,
            };
            let key = e.key().expect("a day");
            files.insert(journal::path(&key), serde_json::to_vec(&e).unwrap());
            written.insert(key, Kind::Taken);
        }
        let n = macs.len();
        let mut w = World {
            cfg,
            t: 0,
            rng,
            files,
            open: BTreeMap::new(),
            tmps: BTreeMap::new(),
            writes: BTreeMap::new(),
            caches: (0..n).map(|_| Cache::default()).collect(),
            macs,
            midway: false,
            turn: 0,
            done: false,
            drained: false,
            end: cfg.end,
            behind: None,
            took: BTreeMap::new(),
            leaders: BTreeMap::new(),
            landed: BTreeMap::new(),
            written,
            written_at: BTreeMap::new(),
            writers: BTreeMap::new(),
            acked: BTreeMap::new(),
            leases: BTreeMap::new(),
            caught: BTreeSet::new(),
            unreadable: BTreeSet::new(),
            erroring: BTreeSet::new(),
            away: vec![0; n],
            wrong: Vec::new(),
            counts: BTreeMap::new(),
            trace: tracing.then(Vec::new),
        };
        if cfg.old_days > 0 {
            w.count("old journal days");
        }
        w.pick(0);
        w
    }

    fn note(&mut self, s: impl FnOnce() -> String) {
        if let Some(tr) = &mut self.trace {
            tr.push(format!("{:>5} {}", self.t, s()));
        }
    }

    fn wrong(&mut self, s: String) {
        self.note(|| format!("WRONG: {s}"));
        self.wrong.push(format!("at {} s: {s}", self.t));
    }

    fn count(&mut self, what: &'static str) {
        *self.counts.entry(what).or_default() += 1;
    }

    fn clock(&self, me: usize) -> u64 {
        (T0 as i64 + self.t as i64 + self.macs[me].skew) as u64
    }

    fn awake_clock(&self, me: usize) -> u64 {
        1_000_000 + self.t - self.macs[me].slept
    }

    fn awake(&self, k: usize) -> bool {
        self.macs[k].asleep_until <= self.t
    }

    fn deliver(&mut self, to: usize, m: Msg) {
        if to < self.macs.len() && self.awake(to) {
            self.macs[to].inbox.push(m);
        }
    }

    // The schedule.

    /// After a Mac's step: time passes, the faults' events, and whose turn is next.
    fn after(&mut self, me: usize) {
        if self.done {
            return;
        }
        self.macs[me].op_at = self.t;
        self.count("steps");
        let dt = self.rng.below(2);
        self.advance(self.t + dt);
        if self.t < self.cfg.faults {
            self.faults(me);
        }
        self.pick(me);
    }

    /// Time passes to `to`: no further than the faults' end while they last (every Mac wakes then).
    fn advance(&mut self, to: u64) {
        self.t = if self.drained { to } else { to.min(self.cfg.faults) };
        if self.t >= self.cfg.faults && !self.drained {
            self.drained = true;
            let t = self.t;
            for m in &mut self.macs {
                if !m.gone && m.asleep_until > t {
                    m.slept -= m.asleep_until - t;
                    m.asleep_until = t;
                    m.sleep.1 = t;
                }
            }
            self.note(|| "the faults stop: every Mac awake from now".into());
        }
        if self.t >= self.end && !self.goes_on() {
            self.done = true;
        }
    }

    /// At the run's end: whether it goes on ten minutes more, its lead still merging what the
    /// faults left (the one path's records lack entries, fewer than when it last went on, the
    /// first time any), or not caught up yet (an entry never whole is refused only after an hour of
    /// its reads; a listing its agent lost is asked for again two hours on); for `MERGING_S` at
    /// most. (A share taking seconds an operation merges hours of entries slowly, at a minute of
    /// reads a loop. The checks at the end are the same, the take-ups' too: none may be made after
    /// the config's end's last ten minutes.)
    fn goes_on(&mut self) -> bool {
        let Some(&h) = self.took.keys().next_back() else { return false };
        let r = self.files.get(records::ONE).and_then(|b| serde_json::from_slice::<Records>(b).ok());
        // (Its records not saved yet: nothing to go on for.)
        let lack = r.as_ref().map_or(0, |r| self.written.iter().filter(|&(k, &kind)| !names(r, k, kind)).count());
        // (Or its lead not caught up yet: an entry never whole, refused only after an hour of its
        // lead's reads; a listing its agent lost, asked for again two hours on.)
        let behind = r.is_some() && self.macs.iter().any(|m| m.leads == Some(h) && !m.gone && !m.caught);
        let merging = lack > 0 && self.behind.is_none_or(|b| lack < b);
        if !(merging || behind) || self.t >= self.cfg.end + MERGING_S {
            return false;
        }
        self.behind = Some(lack);
        self.end = self.t + QUIET_S;
        if merging {
            self.count("ten minutes more, the lead merging");
            self.note(|| format!("the run goes on: the records lack {lack} entries"));
        } else {
            self.count("ten minutes more, the lead not caught up");
            self.note(|| format!("the run goes on: epoch {h}'s lead isn't caught up"));
        }
        true
    }

    /// Mac `k`, awake, falls asleep for `d` s.
    fn sleep(&mut self, k: usize, d: u64) {
        self.macs[k].asleep_until = self.t + d;
        self.macs[k].slept += d;
        self.macs[k].woke = self.macs[k].woke(self.t);
        self.macs[k].sleep = (self.t, self.t + d);
        self.count("sleeps");
        self.note(|| format!("mac{k} sleeps {d} s"));
    }

    fn faults(&mut self, me: usize) {
        if self.cfg.fails && self.rng.chance(P_AWAY) {
            let d = self.rng.range(60, 600);
            self.away[me] = self.t + d;
            self.count("shares away from a Mac");
            self.note(|| format!("the share answers none of mac{me}'s reads for {d} s"));
        }
        let p = if std::mem::take(&mut self.midway) { P_SLEEP_MIDWAY } else { P_SLEEP };
        if self.rng.chance(p) && !self.macs[me].gone {
            let d = match self.rng.below(10) {
                0..=5 => self.rng.range(5, 90),
                6..=8 => self.rng.range(90, 900),
                _ => self.rng.range(900, 2400),
            };
            self.sleep(me, d);
        }
        if self.rng.chance(P_APP) {
            let at = self.rng.below(self.macs.len() as u64) as usize;
            let newest = self.macs.iter().map(|m| m.app).filter(|&a| a != DEV).max().unwrap_or(0);
            self.macs[at].app = if self.cfg.downgrade && self.rng.chance(0.5) {
                // A development build, or the published app rolled back.
                if self.rng.chance(0.6) {
                    DEV
                } else {
                    newest.saturating_sub(self.rng.range(1, 3))
                }
            } else {
                newest + self.rng.range(1, 3)
            };
            let v = app(self.macs[at].app);
            self.note(|| format!("mac{at} gets app {v}"));
        }
    }

    /// Whose turn is next: this Mac's again, most often, while it can run; else another's that can
    /// (awake, not waiting on the share, its loop due), at random; else time jumps to the next
    /// waking or loop.
    fn pick(&mut self, me: usize) {
        while !self.done {
            let t = self.t;
            let can = |m: &MacW| m.asleep_until <= t && m.busy_until <= t && m.ready_at <= t;
            if can(&self.macs[me]) && self.rng.chance(P_KEEP) {
                self.turn = me;
                return;
            }
            let ready: Vec<usize> = (0..self.macs.len()).filter(|&k| can(&self.macs[k])).collect();
            if !ready.is_empty() {
                self.turn = ready[self.rng.below(ready.len() as u64) as usize];
                return;
            }
            let next =
                self.macs.iter().map(|m| m.asleep_until.max(m.busy_until).max(m.ready_at)).min().unwrap_or(t + 1);
            self.advance(next.max(t + 1));
        }
    }

    // The NAS.

    fn see(&mut self, me: usize, path: &str, v: Option<Vec<u8>>) {
        let until = self.t + self.cfg.stale;
        self.caches[me].files.insert(path.to_string(), (v, until));
        if let Some((dir, _)) = path.rsplit_once('/') {
            self.caches[me].dirs.remove(dir);
        }
    }

    fn fetch(&mut self, me: usize, path: &str) -> Option<Vec<u8>> {
        if let Some((v, until)) = self.caches[me].files.get(path) {
            if *until > self.t {
                return v.clone();
            }
        }
        // (An entry another Mac renamed into place isn't there yet to this one's reads.)
        if let Some(&(who, at)) = self.writes.get(path).filter(|_| self.cfg.lag > 0 && entry_of(path).is_some()) {
            let lag = (at.wrapping_mul(2_654_435_761) ^ (me as u64).wrapping_mul(40_503) ^ path.len() as u64)
                % (self.cfg.lag + 1);
            if who != me && self.t < at + lag {
                self.count("entry reads lagging");
                self.caches[me].files.insert(path.to_string(), (None, at + lag));
                return None;
            }
        }
        let v = self.files.get(path).cloned();
        let until = self.t + self.rng.below(self.cfg.stale + 1);
        self.caches[me].files.insert(path.to_string(), (v.clone(), until));
        v
    }

    fn read(&mut self, me: usize, path: &str) -> Option<Vec<u8>> {
        let v = self.fetch(me, path);
        if v.is_some() {
            let until = self.t + self.rng.below(OPEN + 1);
            let held = self.open.entry(path.to_string()).or_default().entry(me).or_insert(0);
            *held = (*held).max(until);
        }
        v
    }

    /// An operation's time on a share under load: its Mac waits on it, awake.
    fn slow(&mut self, me: usize) {
        if self.cfg.op_s.1 > 0 {
            let d = self.rng.range(self.cfg.op_s.0, self.cfg.op_s.1);
            self.macs[me].busy_until = self.t + d;
            *self.counts.entry("operation seconds").or_default() += d;
        }
        if self.cfg.stalls && self.t < self.cfg.faults && self.rng.chance(P_STALL) {
            let d = self.rng.range(300, 600);
            self.macs[me].busy_until = self.macs[me].busy_until.max(self.t + d);
            self.count("operations stalled");
            self.note(|| format!("the share stalls mac{me}'s operation {d} s"));
        }
    }

    /// Whether the share answers Mac `me`'s read of `path` (a stat or a listing: `read` false).
    fn answers(&mut self, me: usize, path: &str, read: bool) -> Result<()> {
        if self.t < self.away[me] {
            self.count("reads the share didn't answer");
            bail!("{path}: the share doesn't answer");
        }
        if read && self.erroring.contains(path) {
            self.count("reads of an erroring entry");
            bail!("read {path}: an I/O error");
        }
        Ok(())
    }

    fn list(&mut self, me: usize, dir: &str) -> Vec<String> {
        if self.cfg.list_s.1 > 0 {
            // (The share under load: 3 to 33 s a folder, this Mac waiting on it, awake.)
            let d = self.rng.range(self.cfg.list_s.0, self.cfg.list_s.1);
            self.macs[me].busy_until = self.t + d;
            *self.counts.entry("listing seconds").or_default() += d;
        }
        if let Some((v, until)) = self.caches[me].dirs.get(dir) {
            if *until > self.t {
                return v.clone();
            }
        }
        let prefix = format!("{dir}/");
        let names: BTreeSet<&str> = self
            .files
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter_map(|r| r.split('/').next())
            .filter(|n| !n.ends_with(".tmp"))
            .collect();
        let v: Vec<String> = names.into_iter().map(str::to_string).collect();
        let until = self.t + self.rng.below(self.cfg.stale + 1);
        self.caches[me].dirs.insert(dir.to_string(), (v.clone(), until));
        v
    }

    fn create(&mut self, me: usize, path: &str) -> bool {
        if let Some(b) = self.files.get(path).cloned() {
            self.see(me, path, Some(b));
            return false;
        }
        self.files.insert(path.to_string(), Vec::new());
        self.writes.insert(path.to_string(), (me, self.t));
        self.open.entry(path.to_string()).or_default().insert(me, u64::MAX);
        self.midway = true;
        self.see(me, path, Some(Vec::new()));
        if path.contains("/terms/") || path.contains("/term/") {
            self.wrong(format!("mac{me} made {path}: no term is made since docs/coordination.md step 8c"));
        }
        true
    }

    fn fill(&mut self, me: usize, path: &str, b: &[u8]) {
        if self.files.get(path).is_none_or(|x| !x.is_empty()) || self.writes.get(path).map(|w| w.0) != Some(me) {
            self.wrong(format!("{path} changed between mac{me}'s create and its bytes"));
        }
        self.files.insert(path.to_string(), b.to_vec());
        self.writes.insert(path.to_string(), (me, self.t));
        if let Some(h) = self.open.get_mut(path) {
            h.remove(&me);
        }
        self.see(me, path, Some(b.to_vec()));
        self.landed(me, path, b, true);
    }

    /// A create whose bytes don't land: the file left empty, no longer held by its maker.
    fn cut(&mut self, me: usize, path: &str) {
        if let Some(h) = self.open.get_mut(path) {
            h.remove(&me);
        }
        self.count("creates cut short");
        self.note(|| format!("mac{me}'s create of {path}: its bytes don't land"));
    }

    fn write_tmp(&mut self, tmp: &str, b: &[u8]) {
        self.files.insert(tmp.to_string(), b.to_vec());
        self.tmps.insert(tmp.to_string(), self.t);
        self.midway = true;
    }

    fn rename(&mut self, me: usize, tmp: &str, path: &str) -> bool {
        if self.open.get(path).is_some_and(|h| h.iter().any(|(&who, &until)| who != me && until > self.t)) {
            self.count("busy renames");
            return false;
        }
        let mut b = self.files.remove(tmp).unwrap_or_default();
        let since = self.tmps.remove(tmp).unwrap_or(self.t);
        if self.cfg.truncs && entry_of(path).is_some() && self.rng.chance(P_TRUNC) {
            // (Its last bytes lost on the share, for good.)
            let cut = 1 + self.rng.below(24) as usize;
            b.truncate(b.len().saturating_sub(cut));
            self.count("entries cut short on the share");
            self.note(|| format!("{path} lands cut short"));
        }
        if self.cfg.fails && entry_of(path).is_some() && self.rng.chance(P_EIO) {
            // (Whole, and every read of it errors, for good.)
            self.erroring.insert(path.to_string());
            self.count("entries erroring on every read");
            self.note(|| format!("{path} errors on every read"));
        }
        // (Its maker writing the bytes its create couldn't: still its create's; or the same bytes
        // again, an earlier finish's answer lost: no change.)
        let mine = self.writes.get(path).map(|w| w.0) == Some(me);
        let finishing = mine && self.files.get(path).is_some_and(|x| x.is_empty());
        if let Some(&(who, at)) = self.writes.get(path) {
            if who != me && at > since {
                self.count("late renames over another's write");
                self.note(|| {
                    format!("mac{me}'s rename of {path}, written at {since}, lands over mac{who}'s write at {at}")
                });
            }
        }
        self.files.insert(path.to_string(), b.clone());
        self.writes.insert(path.to_string(), (me, self.t));
        self.see(me, path, Some(b.clone()));
        if let Some(m) = beat_of(path) {
            // A heartbeat says when its Mac beat, by its clock as it wrote it (review M6: its loop's
            // start, minutes before at times).
            let at = (T0 as i64 + since as i64 + self.macs[me].skew) as u64;
            if let Ok(bt) = serde_json::from_slice::<Beat>(&b) {
                if bt.beat + driver::GAP_S < at {
                    self.wrong(format!("{m}'s heartbeat says it beat at {}, written at {at} by its clock", bt.beat));
                }
            }
        }
        self.landed(me, path, &b, finishing);
        true
    }

    fn remove(&mut self, me: usize, path: &str) {
        if path == lead::LEAD || path == records::ONE || path == records::ONE_BRIEF {
            self.wrong(format!("mac{me} removed {path}"));
        }
        if let Some(key) = entry_of(path) {
            self.wrong(format!("mac{me} removed entry {key}"));
        }
        self.files.remove(path);
        self.see(me, path, None);
    }

    /// A file's bytes landed: checked by what it is. `made`: by its create (or its maker finishing
    /// it).
    fn landed(&mut self, me: usize, path: &str, b: &[u8], made: bool) {
        if path == lead::LEAD && !made {
            self.wrong(format!("mac{me} wrote over the lead's declaration"));
        } else if path == records::ONE {
            // The one path (docs/coordination.md steps 8b, 8c): the declared lead's snapshots alone,
            // in the epoch it leads, consistent; within an epoch never going back nor losing an
            // entry; and none of an epoch at or after one a lead acknowledged an entry in lacks it.
            let r = match serde_json::from_slice::<Records>(b) {
                Ok(r) => r,
                Err(err) => return self.wrong(format!("the one path's records can't be parsed: {err}")),
            };
            let e = r.epoch;
            if let Err(why) = consistent(&r) {
                self.wrong(format!("epoch {e}'s records at {} aren't consistent: {why}", r.seq));
            }
            if me != 0 || self.leaders.get(&e).is_some_and(|&o| o != me) || e > self.macs[me].store {
                self.wrong(format!(
                    "mac{me} wrote epoch {e}'s records (its store's epoch {}, led by {:?})",
                    self.macs[me].store,
                    self.leaders.get(&e)
                ));
            }
            let handled: BTreeSet<String> = r.reflected.iter().chain(r.rejected.keys()).cloned().collect();
            // Refused as never read whole: only what never was, as its lead's reads could see.
            for (k, why) in r.rejected.iter().filter(|(_, why)| why.starts_with("not read whole")) {
                if !self.unreadable.insert(k.clone()) {
                    continue;
                }
                self.count("entries refused, never read whole");
                if let Some(&w) = self.written_at.get(k).filter(|&&w| w + self.cfg.stale + self.cfg.lag + 600 < self.t)
                {
                    self.wrong(format!("epoch {e}'s records refuse {k} ({why}), whole on the share since {w} s"));
                }
            }
            if let Some((seq, was)) = self.landed.get(&e) {
                let back = (r.seq <= *seq).then(|| format!("epoch {e}'s records went from {seq} to {}", r.seq));
                let lost =
                    was.difference(&handled).next().map(|k| format!("epoch {e}'s records at {} lost {k}", r.seq));
                for w in back.into_iter().chain(lost) {
                    self.wrong(w);
                }
            }
            if self.landed.keys().next_back().is_some_and(|&last| last > e) {
                self.count("the one path's records landed late");
            }
            let lost: Vec<String> =
                self.acked.iter().filter(|&(k, &at)| at <= e && !handled.contains(k)).map(|(k, _)| k.clone()).collect();
            if let Some(k) = lost.first() {
                self.wrong(format!(
                    "epoch {e}'s records at {} lack {k}, acknowledged in epoch {} ({} in all)",
                    r.seq,
                    self.acked[k],
                    lost.len()
                ));
            }
            self.note(|| format!("mac{me} saves epoch {e}'s records at {} ({} entries)", r.seq, handled.len()));
            self.landed.insert(e, (r.seq, handled));
        } else if let Some(key) = entry_of(path).filter(|_| !self.erroring.contains(path)) {
            if let Ok(en) = serde_json::from_slice::<Entry>(b) {
                let kind = match en.step.as_str() {
                    _ if crate::agent::seam::for_tests()
                        .outside_by(&en, &|l, removal| writes(&en.step, l, removal))
                        .is_some() =>
                    {
                        Kind::Refused
                    }
                    "flaky" => Kind::Either,
                    _ => Kind::Taken,
                };
                if kind == Kind::Refused && !self.written.contains_key(&key) {
                    self.count("entries outside their step's outputs, refused by every lead");
                }
                self.writers.insert(key.clone(), mac_of(&en.member));
                self.written_at.entry(key.clone()).or_insert(self.t);
                self.written.insert(key, kind);
            }
        }
    }

    // What the Macs say they believe and do.

    fn leads(&mut self, me: usize, e: Option<u64>) {
        if let Some(e) = e {
            if me != 0 {
                self.wrong(format!("mac{me}, a member, leads epoch {e}: only the declared lead leads"));
            }
            if let Some(&other) = self.leaders.get(&e).filter(|&&o| o != me) {
                self.wrong(format!("mac{me} leads epoch {e}, which mac{other} led"));
            }
            if e <= self.macs[me].led {
                self.wrong(format!("mac{me} leads epoch {e} after epoch {}", self.macs[me].led));
            }
            if let Some(&last) = self.landed.keys().filter(|&&k| k != e).next_back().filter(|&&k| k >= e) {
                self.wrong(format!("mac{me} leads epoch {e}, not past the records' epoch {last}"));
            }
            self.leaders.insert(e, me);
            self.took.insert(e, (me, self.t));
            self.macs[me].led = e;
            self.count("take-ups");
            self.note(|| format!("mac{me} leads epoch {e}"));
        } else if let Some(was) = self.macs[me].leads {
            self.note(|| format!("mac{me} no longer leads epoch {was}"));
        }
        self.macs[me].leads = e;
    }

    fn ack(&mut self, me: usize, epoch: u64, keys: &[String]) {
        for key in keys {
            self.acked.entry(key.clone()).or_insert(epoch);
            if !self.landed.get(&epoch).is_some_and(|(_, h)| h.contains(key)) {
                self.wrong(format!("mac{me} acknowledged {key} as epoch {epoch}'s before a snapshot of it named it"));
            }
        }
    }

    /// Mac `me`, leading epoch `e`, says its records reflect the journal as the listing of every day
    /// it asked for at `listed` (its clock) found it (review N2; re-review F3, F4). That listing
    /// began less than a day before (`driver::RELIST_S`); after its Mac last woke from a sleep
    /// (what its members told it meanwhile was lost); and after its take-up. And its epoch's last
    /// snapshot names every entry written before it (a listing is stale up to `Cfg::stale`).
    /// Called first after its step: as of its step's last operation.
    fn caught_up(&mut self, me: usize, e: u64, listed: Option<u64>) {
        let Some(at) = listed else {
            return self.wrong(format!("mac{me} says epoch {e}'s records are caught up, and listed none"));
        };
        if !self.caught.insert((e, at)) {
            return;
        }
        self.count("leads caught up");
        let (began, said) = ((at as i64 - T0 as i64 - self.macs[me].skew).max(0) as u64, self.macs[me].op_at);
        if said.saturating_sub(began) >= driver::RELIST_S {
            self.wrong(format!("mac{me} says epoch {e}'s records are caught up by a listing {} s old", said - began));
        }
        let woke = self.macs[me].woke(said);
        if began < woke {
            self.wrong(format!("mac{me} says epoch {e}'s records are caught up by a listing begun at {began} s, before it woke at {woke} s"));
        }
        if let Some(&(_, took)) = self.took.get(&e) {
            if began + 1 < took {
                self.wrong(format!("mac{me} says epoch {e}'s records are caught up by a listing begun at {began} s, before its take-up at {took} s"));
            }
        }
        let named = self.landed.get(&e).map(|(_, h)| h.clone()).unwrap_or_default();
        let missing: Vec<String> = self
            .written_at
            .iter()
            .filter(|(k, &w)| w + self.cfg.stale + 1 < began && !named.contains(*k))
            .map(|(k, _)| k.clone())
            .collect();
        if let Some(k) = missing.first() {
            self.wrong(format!(
                "mac{me} says epoch {e}'s records are caught up; they lack {k}, written before its listing ({} in all)",
                missing.len()
            ));
        }
    }

    /// Entries merged: counted by what they were.
    fn merged(&mut self, applied: &[String], overtaken: usize, listed: bool) {
        *self.counts.entry("entries passed over for a later lease's").or_default() += overtaken as u64;
        if listed {
            *self.counts.entry("entries found by a listing").or_default() += applied.len() as u64;
        }
    }

    /// The checks at the end of a run: the declared lead (mac 0) leads, alone, caught up, its last
    /// take-up before its config's end's last ten minutes (a run gone on while the lead merges makes
    /// none), and the one path's records name every entry ever written.
    fn finish(&mut self) {
        let Some((&h, &(_, at))) = self.took.iter().next_back() else { return self.wrong("no take-up".into()) };
        if at + QUIET_S > self.cfg.end {
            self.wrong(format!(
                "epoch {h} was taken up at {at} s, in the run's last ten minutes (of {} s) or after: the take-ups don't settle",
                self.cfg.end
            ));
        }
        for k in 0..self.macs.len() {
            let leads = self.macs[k].leads;
            if k == 0 && leads != Some(h) {
                self.wrong(format!("the declared lead mac0 leads {leads:?} at the end, its last epoch {h}"));
            }
            if k == 0 && leads == Some(h) && !self.macs[k].caught {
                self.wrong(format!("the lead mac0 isn't caught up at the end"));
            }
            if k != 0 && leads.is_some() {
                self.wrong(format!("mac{k} leads {leads:?} at the end"));
            }
        }
        match self.files.get(records::ONE).and_then(|b| serde_json::from_slice::<Records>(b).ok()) {
            Some(r) => self.lost(&r, &format!("epoch {}'s last records", r.epoch)),
            None => self.wrong("no records at the one path at the end".into()),
        }
    }

    fn lost(&mut self, r: &Records, whose: &str) {
        if let Err(why) = consistent(r) {
            self.wrong(format!("{whose} aren't consistent: {why}"));
        }
        let lost: Vec<String> =
            self.written.iter().filter(|&(k, &kind)| !names(r, k, kind)).map(|(k, _)| k.clone()).collect();
        for k in lost {
            let gone = self.writers.get(&k).is_some_and(|&g| self.macs.get(g).is_some_and(|m| m.gone));
            self.wrong(format!(
                "entry {k}{} is lost: {whose} don't name it",
                if gone { " (of a Mac gone for good)" } else { "" }
            ));
        }
    }
}

/// The world, shared by the Macs' threads.
struct Shared {
    w: Mutex<World>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, World> {
        self.w.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What a Mac's step gets once the run is over: its thread ends.
const STOP: &str = "the run is over";

/// A Mac's hold on the world: its NAS, its messages, its clocks.
#[derive(Clone)]
struct Sim {
    sh: Arc<Shared>,
    me: usize,
}

impl Sim {
    /// Runs `f` on the world in this Mac's turn, then the schedule's next step.
    fn op<T>(&self, f: impl FnOnce(&mut World, usize) -> T) -> Result<T> {
        let mut w = self.sh.lock();
        while !w.done && w.turn != self.me {
            w = self.sh.cv.wait(w).unwrap_or_else(|e| e.into_inner());
        }
        if w.done {
            bail!(STOP);
        }
        let r = f(&mut w, self.me);
        w.after(self.me);
        if w.turn != self.me || w.done {
            self.sh.cv.notify_all();
        }
        Ok(r)
    }

    /// Runs `f` on the world in this Mac's turn, with no step of its own: its notes and counts, in
    /// an order the seed decides (a Mac whose step passed the turn runs on until its next, beside
    /// the Mac whose turn it is).
    fn quiet(&self, f: impl FnOnce(&mut World)) {
        let mut w = self.sh.lock();
        while !w.done && w.turn != self.me {
            w = self.sh.cv.wait(w).unwrap_or_else(|e| e.into_inner());
        }
        f(&mut w);
    }

    /// A look at one of its clocks, in this Mac's turn: no time passes, and no sleep comes between
    /// two looks (a clock is read in no time).
    fn look(&self, f: impl FnOnce(&World, usize) -> u64) -> u64 {
        let mut w = self.sh.lock();
        while !w.done && w.turn != self.me {
            w = self.sh.cv.wait(w).unwrap_or_else(|e| e.into_inner());
        }
        f(&w, self.me)
    }

    fn over(&self) -> bool {
        self.sh.lock().done
    }

    /// Notes an event, when tracing.
    fn note(&self, s: impl FnOnce() -> String) {
        self.quiet(|w| w.note(s));
    }

    fn count(&self, what: &'static str) {
        self.quiet(|w| w.count(what));
    }

    fn inbox(&self) -> Result<Vec<Msg>> {
        self.op(|w, me| std::mem::take(&mut w.macs[me].inbox))
    }

    fn app(&self) -> Result<String> {
        self.op(|w, me| app(w.macs[me].app))
    }

    fn faulting(&self) -> Result<bool> {
        self.op(|w, _| w.t < w.cfg.faults)
    }

    /// A job's lease: the next granted in the epoch the declared lead leads (the last it led while
    /// it leads none).
    fn lease(&self) -> Result<LeaseId> {
        self.op(|w, _| {
            let e = w.macs[0].leads.unwrap_or(w.macs[0].led);
            let n = w.leases.entry(e).or_default();
            *n += 1;
            LeaseId { epoch: e, n: *n }
        })
    }

    fn send(&self, to: usize, m: Msg) -> Result<()> {
        self.op(|w, _| w.deliver(to, m))
    }

    fn leads(&self, e: Option<u64>) -> Result<()> {
        self.op(|w, me| w.leads(me, e))
    }

    /// Writes its heartbeat whole, `beat` stamped by its clock as its temporary file is written.
    fn beat(&self, mut b: Beat) -> Result<()> {
        let path = super::beat::path(&b.member);
        let tmp = format!("{path}.mac{}.tmp", self.me);
        self.op(|w, me| {
            b.beat = w.clock(me);
            w.write_tmp(&tmp, &serde_json::to_vec(&b).unwrap());
        })?;
        self.rename(&tmp, &path)
    }

    fn rename(&self, tmp: &str, path: &str) -> Result<()> {
        for _ in 0..BUSY_TRIES {
            if self.op(|w, me| w.rename(me, tmp, path))? {
                return Ok(());
            }
        }
        self.op(|w, _| {
            w.files.remove(tmp);
            w.tmps.remove(tmp);
        })?;
        bail!("rename over {path}: busy")
    }

    /// Waits for its next loop.
    fn idle(&self) -> Result<()> {
        self.op(|w, me| {
            let j = w.rng.below(5);
            w.macs[me].ready_at = w.t + LOOP_S + j;
            // A member leaves for good, between two of its loops: its lid closed for the week (never
            // the declared lead: with it gone, nothing builds until it's back).
            if w.cfg.leave && me != 0 && !w.macs.iter().any(|m| m.gone) && w.t > w.cfg.faults / 3 && w.t < w.cfg.faults
            {
                if w.rng.chance(0.02) {
                    w.macs[me].gone = true;
                    w.macs[me].asleep_until = u64::MAX;
                    w.count("Macs gone for good");
                    w.note(|| format!("mac{me} leaves for good"));
                }
            }
        })
    }
}

impl Nas for Sim {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        if !self.op(|w, me| {
            w.slow(me);
            w.create(me, path)
        })? {
            return Ok(Created::There);
        }
        let (cut, lost) = self.op(|w, me| {
            if w.cfg.cuts && w.rng.chance(P_CUT) {
                w.cut(me, path);
                return (true, false);
            }
            w.fill(me, path, bytes);
            (false, w.cfg.cuts && w.rng.chance(P_LOST))
        })?;
        if cut {
            return Ok(Created::Unwritten(anyhow!("write {path}: the share went away (made, and left short)")));
        }
        if lost {
            self.count("lost answers");
            bail!("create {path}: the answer was lost");
        }
        Ok(Created::Made)
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let tmp = format!("{path}.mac{}.tmp", self.me);
        self.op(|w, me| {
            w.slow(me);
            w.write_tmp(&tmp, bytes)
        })?;
        self.rename(&tmp, path)?;
        if self.op(|w, _| w.cfg.cuts && w.rng.chance(P_LOST))? {
            self.count("lost answers");
            bail!("write {path}: the answer was lost");
        }
        Ok(())
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.op(|w, me| {
            w.slow(me);
            w.answers(me, path, true)?;
            Ok(w.read(me, path))
        })?
    }

    fn exists(&self, path: &str) -> Result<bool> {
        self.op(|w, me| {
            w.slow(me);
            w.answers(me, path, false)?;
            Ok(w.fetch(me, path).is_some())
        })?
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.op(|w, me| {
            w.answers(me, dir, false)?;
            Ok(w.list(me, dir))
        })?
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.op(|w, me| w.remove(me, path))
    }
}

impl Io for Sim {
    fn now(&self) -> u64 {
        self.look(|w, me| w.clock(me))
    }

    fn awake(&self) -> u64 {
        self.look(|w, me| w.awake_clock(me))
    }

    fn kept(&self) -> Result<Option<Vec<u8>>> {
        let (me, mut b) = (self.me, None);
        self.quiet(|w| b = w.macs[me].local.clone());
        Ok(b)
    }

    fn keep(&self, records: &[u8]) -> Result<()> {
        let me = self.me;
        self.quiet(|w| w.macs[me].local = Some(records.to_vec()));
        Ok(())
    }

    fn epoch(&self, past: u64) -> Result<u64> {
        let (me, mut e) = (self.me, 0);
        self.quiet(|w| {
            let m = &mut w.macs[me];
            m.store = m.store.max(past + 1);
            e = m.store;
        });
        Ok(e)
    }
}

/// A simulated Mac: the agent's part around the pool's driver, as the agent will play it.
struct Mac {
    sim: Sim,
    k: usize,
    rng: Rng,
    me: Member,
    driver: Driver,
    jobs: u64,
    /// The epoch it leads in, as the world was told.
    leads: Option<u64>,
    /// A listing its driver asked for, made after its step, for its next; those not made yet, the
    /// oldest first (one that failed made again after the next step).
    listed: Option<Listed>,
    listings: std::collections::VecDeque<driver::Listing>,
    /// Its coordinator's state: the epoch it grants in, and the jobs granted.
    coord: (u64, u64),
}

impl Mac {
    fn new(sim: Sim, k: usize, seed: u64) -> Mac {
        let me = Member { id: id(k), host: format!("mac{k}"), app: app(0) };
        let rng = Rng(seed ^ (k as u64 + 1).wrapping_mul(0xA076_1D64_78BD_642F));
        let driver = Driver::unlocked(me.clone(), driver::Saved::default());
        Mac { sim, k, rng, me, driver, jobs: 0, leads: None, listed: None, listings: Default::default(), coord: (0, 0) }
    }

    fn run(mut self) {
        loop {
            if let Err(e) = self.turn() {
                if self.sim.over() {
                    return;
                }
                self.sim.note(|| format!("mac{}: {e:#}", self.k));
                if self.sim.idle().is_err() {
                    return;
                }
            }
        }
    }

    /// One loop of the agent.
    fn turn(&mut self) -> Result<()> {
        // Another app installed: a restart, its memory gone but what its driver saved.
        let app = self.sim.app()?;
        if app != self.me.app {
            self.me.app = app;
            let mut saved: driver::Saved = serde_json::from_slice(&serde_json::to_vec(&self.driver.saved())?)?;
            if self.sim.op(|w, me| {
                let lost = w.cfg.lose && w.rng.chance(0.3);
                if lost {
                    // (Its memory, its saved state and the records it kept all gone; its store
                    // kept, in its own folder.)
                    w.macs[me].local = None;
                    w.count("saved states lost");
                }
                // (Its store opens again: its epoch one more.)
                w.macs[me].store += 1;
                lost
            })? {
                saved = driver::Saved::default();
            }
            self.driver = Driver::unlocked(self.me.clone(), saved);
            (self.listed, self.coord) = (None, (0, 0));
            self.listings.clear();
            self.sim.count("restarts");
            if self.leads.take().is_some() {
                self.sim.leads(None)?;
            }
        }
        let faulting = self.sim.faulting()?;
        let mut heard = Heard { listed: self.listed.take(), ..Default::default() };
        for m in self.sim.inbox()? {
            heard.msgs.push((id(m.from), m.msg));
        }
        if faulting && self.rng.chance(P_JOB) {
            let lease = self.sim.lease()?;
            let now = Io::now(&self.sim);
            heard.entries.push(self.job(lease, now));
        }
        let sweep = faulting && self.leads.is_some() && self.rng.chance(P_SWEEP);
        let out = self.driver.step(&self.sim, heard, &check);
        self.after(out, faulting, sweep)
    }

    /// What its driver's step came to, done (`sweep`: its agent would sweep, were it let).
    fn after(&mut self, out: Out, faulting: bool, sweep: bool) -> Result<()> {
        // (First: as of its step's end, a sleep since not counted.)
        let (k, caught) = (self.k, out.leads.is_some() && out.caught_up);
        self.sim.quiet(|w| w.macs[k].caught = caught);
        if let (Some(e), true) = (out.leads, out.caught_up) {
            let listed = out.listed_at;
            self.sim.op(|w, me| w.caught_up(me, e, listed))?;
        }
        for ev in &out.events {
            self.event(ev)?;
        }
        if out.leads != self.leads {
            self.sim.leads(out.leads)?;
            self.leads = out.leads;
        }
        // (A sweep needs both: its step's read of the declaration found it the lead, and its
        // records reflect the journal.)
        if sweep && out.vouched && out.caught_up {
            self.sim.count("sweeps a lead could make");
        }
        // Its coordinator: grants while its lead may.
        if out.leads.is_some() && out.duties && faulting && self.rng.chance(P_GRANT) {
            self.coord.1 += 1;
        }
        for (to, m) in out.send {
            if let driver::Msg::Ack { epoch, keys, .. } = &m {
                self.sim.op(|w, me| w.ack(me, *epoch, keys))?;
            }
            self.sim.send(mac_of(&to), Msg { from: self.k, msg: m })?;
        }
        if let Err(e) = self.sim.beat(out.beat) {
            if self.sim.over() {
                return Err(e);
            }
        }
        // A job reading the records: the one path's.
        if self.rng.chance(P_READ) {
            if let Ok(Some(r)) = records::load_one(&self.sim) {
                if let Err(why) = consistent(&r) {
                    self.sim.op(|w, me| {
                        w.wrong(format!("mac{me} read epoch {}'s records at {}, not consistent: {why}", r.epoch, r.seq))
                    })?;
                }
            }
        }
        // The listings its driver asked for, off its step, one a loop: one that fails made again
        // after the next. (Now and then, with `Cfg::lost_lists`, one its agent loses: never
        // handed back, its driver asks again once it's overdue.)
        if let Some(l) = out.list {
            self.sim.count("listings");
            if out.listed_at.is_some() && l.since.is_none() {
                self.sim.count("listings of every day again");
            }
            let mut loses = false;
            self.sim.quiet(|w| loses = w.cfg.lost_lists);
            if loses && faulting && self.rng.chance(P_LOSE_LIST) {
                self.sim.count("listings lost");
            } else {
                self.listings.push_back(l);
            }
        }
        if let Some(l) = self.listings.pop_front() {
            match journal::list(&self.sim, l.since.as_deref()) {
                Ok(keys) => self.listed = Some(Listed { n: l.n, keys }),
                Err(_) => self.listings.push_front(l),
            }
        }
        self.sim.idle()
    }

    fn event(&mut self, ev: &Event) -> Result<()> {
        let k = self.k;
        match ev {
            Event::TookUp { epoch, how } => {
                self.coord = (*epoch, 0);
                let kind = match how.as_str() {
                    "its own records" => "take-ups from its own records",
                    h if h.starts_with("the one path's") => "take-ups from the one path's records",
                    _ => "take-ups from today's files",
                };
                self.sim.count(kind);
            }
            Event::Merged { applied, overtaken, listed, .. } => {
                self.sim.quiet(|w| w.merged(applied, *overtaken, *listed))
            }
            Event::Waits { what: "take up the records", .. } => self.sim.count("take-ups tried again"),
            Event::Waits { what: "have a listing back", .. } => self.sim.count("listings overdue, asked for again"),
            Event::Failed { what: "save the records", .. } => self.sim.count("saves tried again"),
            _ => {}
        }
        self.sim.note(|| format!("mac{k}: {ev:?}"));
        Ok(())
    }

    /// A job's hand-off, of lease `lease`: a unit built with a new key, now and then a prune, one
    /// that writes outside its step's outputs too (refused by every lead), or one the lead of an
    /// odd epoch refuses.
    fn job(&mut self, lease: LeaseId, now: u64) -> Entry {
        self.jobs += 1;
        let t = TARGETS[self.rng.below(TARGETS.len() as u64) as usize];
        let k = format!("k{}x{}", self.k, self.jobs);
        let built = |k: &str| Handoff {
            changes: [(logical(t), Some(content(t, k)))].into(),
            pending: [(content(t, k), "sha".into())].into(),
            done: Some(("unit".into(), vec![(t.into(), k.to_string())])),
            ..Default::default()
        };
        let (step, h) = match self.rng.below(20) {
            0 => (
                "unit",
                Handoff {
                    changes: [
                        (logical(t), Some(content(t, "bogus"))),
                        (format!("elsewhere/{t}"), Some(format!("elsewhere/{t}.bogus.x"))),
                    ]
                    .into(),
                    done: Some(("unit".into(), vec![(t.into(), "bogus".into())])),
                    ..Default::default()
                },
            ),
            1 | 2 => (
                "prune",
                Handoff {
                    changes: [(logical(t), None)].into(),
                    done: Some(("prune".into(), vec![(format!("unit {t}"), String::new())])),
                    ..Default::default()
                },
            ),
            3 | 4 => ("flaky", built(&k)),
            _ => ("unit", built(&k)),
        };
        Entry { member: self.me.id.clone(), lease, step: step.into(), handoff: h, at: now }
    }
}

/// What a run gave: what went wrong, the counts of what happened, and its events (when traced).
struct Ran {
    wrong: Vec<String>,
    counts: Counts,
    trace: Vec<String>,
}

/// Runs seed `seed`: two to four Macs, by the seed.
fn run(seed: u64, cfg: Cfg, tracing: bool) -> Ran {
    let macs = 2 + (seed % 3) as usize;
    let sh = Arc::new(Shared { w: Mutex::new(World::new(seed, macs, cfg, tracing)), cv: Condvar::new() });
    let threads: Vec<_> = (0..macs)
        .map(|k| {
            let sh = sh.clone();
            std::thread::spawn(move || {
                let sim = Sim { sh: sh.clone(), me: k };
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Mac::new(sim, k, seed).run()));
                if let Err(p) = r {
                    let why = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    let mut w = sh.lock();
                    w.wrong(format!("mac{k} panicked: {why}"));
                    w.done = true;
                    sh.cv.notify_all();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().ok();
    }
    let mut w = sh.lock();
    w.finish();
    Ran {
        wrong: std::mem::take(&mut w.wrong),
        counts: std::mem::take(&mut w.counts),
        trace: w.trace.take().unwrap_or_default(),
    }
}

/// How many times each kind of thing happened.
type Counts = BTreeMap<&'static str, u64>;

/// Runs every seed of `seeds`, each with the run `cfg` gives it, on all cores: the seeds that went
/// wrong, with what did, and the counts of what happened in all.
fn run_all(seeds: Range<u64>, cfg: impl Fn(u64) -> Cfg + Sync) -> (Vec<(u64, Vec<String>)>, Counts) {
    let next = AtomicU64::new(seeds.start);
    let out = Mutex::new((Vec::new(), BTreeMap::new()));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let seed = next.fetch_add(1, Ordering::Relaxed);
                if seed >= seeds.end {
                    return;
                }
                let r = run(seed, cfg(seed), false);
                let mut o = out.lock().unwrap();
                if !r.wrong.is_empty() {
                    o.0.push((seed, r.wrong));
                }
                for (k, v) in r.counts {
                    *o.1.entry(k).or_insert(0) += v;
                }
            });
        }
    });
    let (mut bad, counts) = out.into_inner().unwrap();
    bad.sort();
    (bad, counts)
}

/// Runs `seeds`; the first that goes wrong is run again traced, and shown.
fn check_all(seeds: Range<u64>, cfg: impl Fn(u64) -> Cfg + Sync) -> Counts {
    let n = seeds.end - seeds.start;
    let (bad, counts) = run_all(seeds, &cfg);
    if let Some((seed, _)) = bad.first() {
        let r = run(*seed, cfg(*seed), true);
        let tail = &r.trace[r.trace.len().saturating_sub(200)..];
        let seeds: Vec<u64> = bad.iter().map(|b| b.0).take(20).collect();
        panic!(
            "{} of {n} schedules went wrong (seeds {seeds:?}…); seed {seed} ({:?}):\n{}\n\nits last events:\n{}",
            bad.len(),
            cfg(*seed),
            r.wrong.join("\n"),
            tail.join("\n")
        );
    }
    counts
}

/// Each kind of take-up and fault, and what the knobs bring, that the default runs must see at
/// least three times (a simulator that never got there would pass too).
const KINDS: [&str; 26] = [
    "entries outside their step's outputs, refused by every lead",
    "take-ups from its own records",
    "take-ups from the one path's records",
    "sleeps",
    "busy renames",
    "take-ups tried again",
    "saves tried again",
    "creates cut short",
    "lost answers",
    "listing seconds",
    "old journal days",
    "Macs gone for good",
    "entries found by a listing",
    "entries passed over for a later lease's",
    "operation seconds",
    "saved states lost",
    "leads caught up",
    "sweeps a lead could make",
    "entry reads lagging",
    "entries cut short on the share",
    "entries erroring on every read",
    "reads of an erroring entry",
    "shares away from a Mac",
    "operations stalled",
    "listings lost",
    "listings overdue, asked for again",
];

#[test]
fn the_pool_keeps_its_invariants_through_thousands_of_schedules() {
    let counts = check_all(0..2000, Cfg::mixed);
    eprintln!("{counts:#?}");
    for what in KINDS {
        assert!(counts.get(what).is_some_and(|&n| n >= 3), "{what}: {:?} in all the runs", counts.get(what));
    }
}

#[test]
#[ignore]
fn the_pool_keeps_its_invariants_through_a_long_run() {
    // POOL_SIM_SEEDS schedules (100,000 by default) from POOL_SIM_FROM (1,000,000) of
    // POOL_SIM_MINUTES' faults each (240), the knobs mixed as by default. (A run goes on past its
    // end while its last lead still merges what the faults left: `World::goes_on`.)
    let var = |v: &str, or: u64| std::env::var(v).ok().and_then(|s| s.parse().ok()).unwrap_or(or);
    let (from, n, faults) =
        (var("POOL_SIM_FROM", 1_000_000), var("POOL_SIM_SEEDS", 100_000), var("POOL_SIM_MINUTES", 240) * 60);
    let started = std::time::Instant::now();
    let counts = check_all(from..from + n, |seed| Cfg { faults, end: faults + 1500, ..Cfg::mixed(seed) });
    eprintln!(
        "{n} schedules of {} minutes' faults in {:.0} s\n{counts:#?}",
        faults / 60,
        started.elapsed().as_secs_f64()
    );
}

#[test]
fn a_run_goes_on_only_while_its_lead_merges() {
    // (The long run's: hours of faults with the lead asleep leave hundreds of entries, and on a
    // share taking seconds an operation the lead back merges a dozen a loop, past the run's end. The run goes on while its records lack fewer entries each ten minutes; a lead that
    // merges none is wrong at the end, as before.)
    let mut w = World::new(0, 4, Cfg::pool(), false);
    let (k1, k2) = ("2026-10-06/1-1".to_string(), "2026-10-06/1-2".to_string());
    w.took.insert(1, (0, 0));
    w.written.insert(k1.clone(), Kind::Taken);
    w.written.insert(k2.clone(), Kind::Taken);
    let mut r = Records { epoch: 1, seq: 1, ..Default::default() };
    let save = |w: &mut World, r: &Records| w.files.insert(records::ONE.to_string(), serde_json::to_vec(r).unwrap());
    save(&mut w, &r);
    w.t = w.cfg.end;
    assert!(w.goes_on(), "two entries lacking: ten minutes more");
    assert!(!w.goes_on(), "none merged since: the run ends");
    r.reflected.insert(k1);
    save(&mut w, &r);
    assert!(w.goes_on(), "one merged since: ten minutes more");
    (w.t, w.behind) = (w.cfg.end + MERGING_S, Some(2));
    assert!(!w.goes_on(), "gone on as long as it may: the run ends");
    (w.t, w.behind) = (w.cfg.end, None);
    r.reflected.insert(k2);
    save(&mut w, &r);
    assert!(!w.goes_on(), "every entry named: the run ends");
}

#[test]
fn a_lead_keeps_leading_over_slow_listings_and_a_week_of_journal() {
    // Listings of 3 to 33 s a folder, and a week of the journal's days to list. (Review H1: a
    // take-up listed the journal in its loop, the loop ran past a minute, and the next took up
    // again, for ever.)
    let counts = check_all(0..300, |_| Cfg { list_s: (3, 33), old_days: 7, ..Cfg::pool() });
    for what in ["take-ups from its own records", "entries found by a listing"] {
        assert!(counts.get(what).is_some_and(|&n| n >= 3), "{what}: {:?}", counts.get(what));
    }
}

#[test]
fn a_development_build_or_a_rollback_leaves_a_lead() {
    // (Review H2: a lead restarted into an older app had its take-up refused, and stopped there
    // every loop. Since #881 there's no app rule: the declared lead leads whatever app.)
    let counts = check_all(0..1000, |_| Cfg { downgrade: true, leave: true, ..Cfg::pool() });
    assert!(counts.get("restarts").is_some_and(|&n| n >= 3), "{counts:?}");
}

#[test]
fn a_lead_lists_every_day_again_daily() {
    // Runs of the faults' 40 minutes and a day after, every Mac awake: a lead's listing of every
    // day asked for again before it's a day old (driver::RELIST_S), its caught_up checked against a
    // listing that old at most.
    let counts = check_all(0..50, |_| Cfg { end: 26 * 3600, ..Cfg::pool() });
    assert!(counts.get("listings of every day again").is_some_and(|&n| n >= 3), "{counts:?}");
}

#[test]
fn cut_creates_lost_states_and_stale_reads_pair_no_versions() {
    // Seed 3090226 of the four-hour runs (re-review 2, H1, with terms): cut creates, saved states
    // lost and reads stale up to three minutes: no snapshot pairs a manifest of one version with
    // keys of another (invariant 4), and none taken up from a stale one loses an acknowledged
    // entry.
    let cfg = Cfg { faults: 14_400, end: 15_900, stale: 184, list_s: (3, 33), cuts: true, lose: true, ..Cfg::pool() };
    let r = run(3_090_226, cfg, false);
    assert!(r.wrong.is_empty(), "{:?}", r.wrong.iter().take(3).collect::<Vec<_>>());
}

#[test]
fn an_entry_never_whole_is_refused_after_an_hour() {
    // Entries cut short on the share for good, in runs of four hours' faults: refused once the
    // lead's reads have found them not whole for an hour (driver::UNREADABLE_S), and no other.
    let counts = check_all(0..200, |_| Cfg { truncs: true, faults: 14_400, end: 15_900, ..Cfg::pool() });
    assert!(counts.get("entries refused, never read whole").is_some_and(|&n| n >= 3), "{counts:?}");
}

#[test]
fn whole_entries_behind_many_not_whole_on_a_slow_share() {
    // Seed 1003691 of the four-hour runs (1051658 alike): a share taking seconds an operation, its
    // reads lagging minutes, entries cut short for good or erroring; the lead had some thirty
    // entries its reads had found not whole, and time each step to read twenty. Read in the order
    // of their leases, the whole ones of the latest leases weren't reached until those before them
    // were refused, an hour on: it merged none for ten minutes, and the run ended with them lacking.
    let cfg = Cfg {
        faults: 14_400,
        end: 15_900,
        cuts: true,
        op_s: (1, 4),
        lag: 524,
        truncs: true,
        fails: true,
        ..Cfg::pool()
    };
    let r = run(1_003_691, cfg, false);
    assert!(r.wrong.is_empty(), "{:?}", r.wrong.iter().take(3).collect::<Vec<_>>());
}

#[test]
fn a_mac_gone_for_good_loses_none_of_its_entries() {
    // A member leaves for good partway: what it never told a lead of, or told a lead no longer
    // current, is found by the lead's listings. (Review L7: only by the next take-up's.)
    let counts = check_all(0..1000, |_| Cfg { leave: true, ..Cfg::pool() });
    assert!(counts.get("Macs gone for good").is_some_and(|&n| n >= 100), "{counts:?}");
}

#[test]
fn a_seed_runs_the_same_every_time() {
    for seed in [11, 12, 13] {
        let (a, b) = (run(seed, Cfg::mixed(seed), true), run(seed, Cfg::mixed(seed), true));
        assert!(a.trace.len() > 20 && a.counts["steps"] > 500, "{} events, {:?}", a.trace.len(), a.counts);
        assert_eq!(a.trace, b.trace);
        assert_eq!(a.wrong, b.wrong);
    }
}

#[test]
#[ignore]
fn the_knobs_one_at_a_time() {
    // POOL_SIM_SEEDS schedules (1,000) with each knob alone, and how many went wrong.
    let n = std::env::var("POOL_SIM_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
    let p = Cfg::pool();
    let knobs: [(&str, Cfg); 19] = [
        ("none", p),
        ("listings of 3 to 33 s", Cfg { list_s: (3, 33), ..p }),
        ("a week of journal days", Cfg { old_days: 7, ..p }),
        ("both", Cfg { list_s: (3, 33), old_days: 7, ..p }),
        ("a Mac leaving", Cfg { leave: true, ..p }),
        ("stale 60 s", Cfg { stale: 60, ..p }),
        ("stale 120 s", Cfg { stale: 120, ..p }),
        ("stale 300 s", Cfg { stale: 300, ..p }),
        ("skew 300 s", Cfg { skew: 300, ..p }),
        ("skew 1200 s", Cfg { skew: 1200, ..p }),
        ("downgrades", Cfg { downgrade: true, ..p }),
        ("cuts and lost answers", Cfg { cuts: true, ..p }),
        ("slow operations", Cfg { op_s: (1, 4), ..p }),
        ("saved states lost", Cfg { lose: true, ..p }),
        ("entry reads lagging up to 600 s", Cfg { lag: 600, ..p }),
        ("entries cut short", Cfg { truncs: true, ..p }),
        ("reads failing", Cfg { fails: true, ..p }),
        ("operations stalled", Cfg { stalls: true, ..p }),
        ("listings lost", Cfg { lost_lists: true, ..p }),
    ];
    for (name, cfg) in knobs {
        let (bad, c) = run_all(0..n, |_| cfg);
        let get = |k: &str| c.get(k).copied().unwrap_or(0);
        eprintln!(
            "{name}: {} of {n} wrong; take-ups from its own records {}, from the one path's {}, refused never read whole {}{}",
            bad.len(),
            get("take-ups from its own records"),
            get("take-ups from the one path's records"),
            get("entries refused, never read whole"),
            bad.first().map(|(s, w)| format!("; seed {s}: {}", w[0])).unwrap_or_default()
        );
    }
}

#[test]
#[ignore]
fn every_seed_runs_the_same_every_time() {
    // POOL_SIM_SEEDS seeds (400), each run twice: the same events, wrongs and counts.
    let n = std::env::var("POOL_SIM_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(400);
    let differ: Vec<u64> = (0..n)
        .filter(|&seed| {
            let (a, b) = (run(seed, Cfg::mixed(seed), true), run(seed, Cfg::mixed(seed), true));
            a.trace != b.trace || a.wrong != b.wrong || a.counts != b.counts
        })
        .collect();
    assert!(differ.is_empty(), "{} of {n} seeds ran differently: {differ:?}", differ.len());
}

#[test]
#[ignore]
fn a_seeds_events() {
    // POOL_SIM_SEED's events, its knobs as by default, or as POOL_SIM_KNOBS lists them (none:
    // "plain"; "list", "old", "leave", "downgrade", "cuts", "slow", "lose", "truncs",
    // "fails", "stalls", "lost", "skew=<s>", "stale=<s>", "lag=<s>"), its faults POOL_SIM_MINUTES
    // long (40), to look into one.
    let seed = std::env::var("POOL_SIM_SEED").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let cfg = match std::env::var("POOL_SIM_KNOBS") {
        Ok(knobs) => knobs.split(',').fold(Cfg::pool(), |c, k| match k.split_once('=') {
            Some(("skew", v)) => Cfg { skew: v.parse().unwrap_or(0), ..c },
            Some(("stale", v)) => Cfg { stale: v.parse().unwrap_or(30), ..c },
            Some(("lag", v)) => Cfg { lag: v.parse().unwrap_or(0), ..c },
            _ => match k {
                "list" => Cfg { list_s: (3, 33), ..c },
                "old" => Cfg { old_days: 7, ..c },
                "leave" => Cfg { leave: true, ..c },
                "downgrade" => Cfg { downgrade: true, ..c },
                "cuts" => Cfg { cuts: true, ..c },
                "slow" => Cfg { op_s: (1, 4), ..c },
                "lose" => Cfg { lose: true, ..c },
                "truncs" => Cfg { truncs: true, ..c },
                "fails" => Cfg { fails: true, ..c },
                "stalls" => Cfg { stalls: true, ..c },
                "lost" => Cfg { lost_lists: true, ..c },
                _ => c,
            },
        }),
        Err(_) => Cfg::mixed(seed),
    };
    // (POOL_SIM_MINUTES: the faults' length, as the long run's.)
    let cfg = match std::env::var("POOL_SIM_MINUTES").ok().and_then(|s| s.parse::<u64>().ok()) {
        Some(m) => Cfg { faults: m * 60, end: m * 60 + 1500, ..cfg },
        None => cfg,
    };
    let r = run(seed, cfg, true);
    eprintln!("{cfg:?}\n{}\n\n{}\n\n{:#?}", r.trace.join("\n"), r.wrong.join("\n"), r.counts);
}
