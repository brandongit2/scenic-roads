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
//! turn is next, puts the Mac to sleep there or not, and brings the owner's asks: hand the lead to
//! a Mac, take it over (forced at times, the lead alive; on an older app, by the owner's say-so), a
//! newer app on a Mac (a restart: its memory gone but what its driver saved), or a development
//! build or a rollback (`Cfg::downgrade`); a member may leave for good (`Cfg::leave`). The Macs
//! play the agent's part around the driver (`Mac`): its jobs' hand-offs, its coordinator's state
//! (granted while the driver lets it, handed over when it settles), the listings the driver asks
//! for, off its step, its heartbeat, stamped as it's written, a job reading the records. Once the
//! faults stop every Mac stays awake for a while, and the owner takes over a term left with no
//! lead the views would show as such (its lead gone, stood down, or its term unreadable); then the
//! last term's lead must be leading, alone, its records must name every entry ever written, and no
//! term may have been made in the run's last ten minutes.
//!
//! The first draft's scheme (`Cfg::draft`: one shared records file the lead rewrites after checking
//! the term, and a journal it empties as it merges) runs on the same model, to show the model
//! finds what lost work there.

use super::beat::Beat;
use super::driver::{self, Ask, Driver, Event, Heard, Io, Listed, Out};
use super::journal::{self, Entry, LeaseId};
use super::nas::{Created, Nas};
use super::records::{self, Records};
use super::term::{self, Current, Term};
use super::Member;
use crate::handoff::Handoff;
use anyhow::{anyhow, bail, Result};
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
/// The shared records file of the first draft.
const DRAFT: &str = "state/build/records.json";
/// A development build's app number.
const DEV: u64 = u64::MAX;

/// Per step of a Mac while the faults last: that it falls asleep right there; and right after it
/// wrote a temporary file or made a file with create-new, the moments a sleep does the most (a
/// rename landing late, a file empty meanwhile).
const P_SLEEP: f64 = 0.004;
const P_SLEEP_MIDWAY: f64 = 0.05;
/// Per step: that the owner asks a Mac's menu to hand the lead to a Mac, asks a Mac to take over,
/// or installs a newer app on a Mac.
const P_ASK: f64 = 0.006;
const P_TAKE: f64 = 0.0012;
const P_APP: f64 = 0.0004;
/// With `Cfg::cuts`: per create, that its bytes don't land (the share gone between); per create
/// and whole write, that its answer is lost (done, said failed).
const P_CUT: f64 = 0.03;
const P_LOST: f64 = 0.01;
/// Per step: that the turn stays with the Mac that has it.
const P_KEEP: f64 = 0.85;
/// Per loop of a Mac while the faults last: that one of its jobs ends (a journal entry); that one
/// reads the current records; leading, that its coordinator grants a job, or that its agent is
/// about to sweep (GC) and has its driver re-assert first.
const P_JOB: f64 = 0.35;
const P_READ: f64 = 0.2;
const P_GRANT: f64 = 0.3;
const P_SWEEP: f64 = 0.01;
/// Once the faults stop: how long a term may be left with no lead before the owner, seeing "No
/// lead" in the views, takes it over (s); and how long the run's end must have seen no new term.
const NO_LEAD_S: u64 = 300;
const QUIET_S: u64 = 600;
/// At most how long a run goes on past its end while its last term's lead still merges what the
/// faults left (s): a share taking seconds an operation, a lead's minute of reads a loop merges a
/// dozen entries, and hours of faults with no lead leave hundreds.
const MERGING_S: u64 = 7200;

/// What a run is.
#[derive(Clone, Copy, Debug)]
struct Cfg {
    /// The faults stop this long into the run (s); every Mac then stays awake.
    faults: u64,
    /// The run ends this long into it, or goes on (`World::goes_on`) while its last term's lead
    /// still merges what the faults left.
    end: u64,
    /// The longest a Mac keeps what it read, stat'ed or listed (s).
    stale: u64,
    /// The first draft's scheme instead of the pool's.
    draft: bool,
    /// How long a listing of a folder takes (s), lo..=hi; (0, 0): no time.
    list_s: (u64, u64),
    /// A member leaves for good (never wakes) partway through.
    leave: bool,
    /// Earlier days in the journal (a week's), an entry each that term 1's records lack.
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
}

impl Cfg {
    /// The pool's, none of the knobs on.
    fn pool() -> Cfg {
        Cfg { faults: 2400, end: 3900, stale: 30, draft: false, list_s: (0, 0), leave: false, old_days: 0, skew: 0, downgrade: false, cuts: false, op_s: (0, 0), lose: false }
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

/// What reaches a Mac (the pool's HTTP API, and its menu: only while it's awake).
#[derive(Clone, Debug)]
enum Msg {
    /// A member's message, over the pool's API.
    Pool { from: usize, msg: driver::Msg },
    /// The owner, on this Mac's menu, asks to hand the lead to a member.
    HandTo(String),
    /// The owner asks this Mac to take the lead over.
    TakeOver { force: bool, downgrade: bool },
}

/// A Mac as the world sees it.
struct MacW {
    id: String,
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
    /// What it says it believes, checked: the highest term it knows, the term it leads, the
    /// highest term it led.
    view: u64,
    leads: Option<u64>,
    led: u64,
    /// Left for good: it never wakes.
    gone: bool,
}

/// What a Mac keeps of what it read, stat'ed and listed: the bytes (None: no file) or names, and
/// until when.
#[derive(Default)]
struct Cache {
    files: BTreeMap<String, (Option<Vec<u8>>, u64)>,
    dirs: BTreeMap<String, (Vec<String>, u64)>,
}

/// What a journal entry is to the checks: one the lead's check takes, one it refuses (a step that
/// doesn't exist), or one the lead of an odd term refuses (a check that depends on the lead).
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
    /// entries the last term's records lacked when it last went on.
    end: u64,
    behind: Option<usize>,
    /// The terms made, by their maker, and when; the term each Mac led.
    made: BTreeMap<u64, (usize, u64)>,
    leaders: BTreeMap<u64, usize>,
    /// Each term's last snapshot landed: its number, and the entries it names.
    landed: BTreeMap<u64, (u64, BTreeSet<String>)>,
    /// Every entry whose bytes landed whole, what it is, and when; those a lead acknowledged.
    written: BTreeMap<String, Kind>,
    written_at: BTreeMap<String, u64>,
    writers: BTreeMap<String, usize>,
    acked: BTreeSet<String>,
    /// The coordinator's state each settle saved with its records, by term and snapshot.
    settles: BTreeMap<(u64, u64), serde_json::Value>,
    /// After the faults: when the owner looks at the views next.
    looks: u64,
    /// The leases granted in each term (the lead grants in order: the coordinator isn't modelled).
    leases: BTreeMap<u64, u64>,
    /// When each Mac began its last listing of every day of the journal (a take-up's), and the
    /// terms whose lead was seen caught up.
    full_at: Vec<Option<u64>>,
    caught: BTreeSet<u64>,
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

fn term_of(path: &str) -> Option<u64> {
    path.strip_prefix("state/build/terms/")?.strip_suffix(".json")?.parse().ok()
}

fn records_of(path: &str) -> Option<u64> {
    path.strip_prefix("state/build/term/")?.strip_suffix("/records.json")?.parse().ok()
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

/// The lead's check of an entry: a step it doesn't know writes nothing; and one the lead of an odd
/// term refuses (a stand-in for a check by the lead's state: its leases, its app's write-sets), so
/// one a lead refused may be taken by the next.
fn check(e: &Entry, r: &Records) -> std::result::Result<(), String> {
    match e.step.as_str() {
        "bogus" => Err("no such step".into()),
        "flaky" if r.term % 2 == 1 => Err(format!("refused by term {}'s lead", r.term)),
        _ => Ok(()),
    }
}

impl World {
    fn new(seed: u64, macs: usize, cfg: Cfg, tracing: bool) -> World {
        let mut rng = Rng(seed);
        let macs: Vec<MacW> = (0..macs)
            .map(|k| {
                let skew = if cfg.skew > 0 { rng.below(2 * cfg.skew + 1) as i64 - cfg.skew as i64 } else { rng.below(3) as i64 - 1 };
                MacW { id: id(k), app: 0, skew, asleep_until: 0, slept: 0, busy_until: 0, ready_at: 0, inbox: Vec::new(), view: 0, leads: None, led: 0, gone: false }
            })
            .collect();
        let mut files = BTreeMap::new();
        // Today's build Mac (mac0) and records: one unit built.
        files.insert(term::WRITER.to_string(), b"mac0\n".to_vec());
        files.insert("state/build/manifest.json".to_string(), serde_json::to_vec(&BTreeMap::from([(logical(TARGETS[0]), content(TARGETS[0], "k0"))])).unwrap());
        files.insert("state/build/jobs.json".to_string(), format!("{{\"unit\": {{\"{}\": \"k0\"}}}}", TARGETS[0]).into_bytes());
        let mut written = BTreeMap::new();
        // A week of the journal's earlier days, an entry each that term 1's records lack.
        for d in 1..=cfg.old_days {
            let t = TARGETS[(d as usize) % TARGETS.len()];
            let k = format!("old{d}");
            let h = Handoff { changes: [(logical(t), Some(content(t, &k)))].into(), done: Some(("unit".into(), vec![(t.into(), k.clone())])), ..Default::default() };
            let e = Entry { member: id(0), lease: LeaseId { term: 0, n: d }, step: "unit".into(), handoff: h, at: T0 - d * 86_400 };
            let key = e.key().expect("a day");
            files.insert(journal::path(&key), serde_json::to_vec(&e).unwrap());
            written.insert(key, Kind::Taken);
        }
        let n = macs.len();
        let looks = cfg.faults + 180;
        let mut w = World { cfg, t: 0, rng, files, open: BTreeMap::new(), tmps: BTreeMap::new(), writes: BTreeMap::new(), caches: (0..n).map(|_| Cache::default()).collect(), macs, midway: false, turn: 0, done: false, drained: false, end: cfg.end, behind: None, made: BTreeMap::new(), leaders: BTreeMap::new(), landed: BTreeMap::new(), written, written_at: BTreeMap::new(), writers: BTreeMap::new(), acked: BTreeSet::new(), settles: BTreeMap::new(), looks, leases: BTreeMap::new(), full_at: vec![None; n], caught: BTreeSet::new(), wrong: Vec::new(), counts: BTreeMap::new(), trace: tracing.then(Vec::new) };
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
        self.count("steps");
        let dt = self.rng.below(2);
        self.advance(self.t + dt);
        if self.t < self.cfg.faults {
            self.faults(me);
        } else if self.t >= self.looks {
            self.looks = self.t + NO_LEAD_S;
            self.no_lead();
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
                }
            }
            self.note(|| "the faults stop: every Mac awake from now".into());
        }
        if self.t >= self.end && !self.goes_on() {
            self.done = true;
        }
    }

    /// At the run's end: whether it goes on ten minutes more, its last term's lead still merging
    /// what the faults left: that term's records lack entries, fewer than when it last went on
    /// (the first time, any), and it has gone on less than `MERGING_S`. (A share taking seconds an
    /// operation merges hours of entries slowly, at a minute of reads a loop. The checks at the
    /// end are the same, the terms' too: none may be made after the config's end's last ten
    /// minutes.)
    fn goes_on(&mut self) -> bool {
        let Some(&h) = self.made.keys().next_back() else { return false };
        let Some(r) = self.files.get(&records::path(h)).and_then(|b| serde_json::from_slice::<Records>(b).ok()) else { return false };
        let lack = self.written.iter().filter(|&(k, &kind)| !names(&r, k, kind)).count();
        if self.cfg.draft || lack == 0 || self.behind.is_some_and(|b| lack >= b) || self.t >= self.cfg.end + MERGING_S {
            return false;
        }
        self.behind = Some(lack);
        self.end = self.t + QUIET_S;
        self.count("ten minutes more, the lead merging");
        self.note(|| format!("the run goes on: term {h}'s records lack {lack} entries"));
        true
    }

    fn faults(&mut self, me: usize) {
        let p = if std::mem::take(&mut self.midway) { P_SLEEP_MIDWAY } else { P_SLEEP };
        if self.rng.chance(p) && !self.macs[me].gone {
            let d = match self.rng.below(10) {
                0..=5 => self.rng.range(5, 90),
                6..=8 => self.rng.range(90, 900),
                _ => self.rng.range(900, 2400),
            };
            self.macs[me].asleep_until = self.t + d;
            self.macs[me].slept += d;
            self.count("sleeps");
            self.note(|| format!("mac{me} sleeps {d} s"));
        }
        let n = self.macs.len() as u64;
        if self.rng.chance(P_ASK) {
            let (at, to) = (self.rng.below(n) as usize, self.rng.below(n) as usize);
            self.note(|| format!("the owner asks mac{at} to hand the lead to mac{to}"));
            self.deliver(at, Msg::HandTo(id(to)));
        }
        if self.rng.chance(P_TAKE) {
            let at = self.rng.below(n) as usize;
            let force = self.rng.chance(0.3);
            let downgrade = force && self.cfg.downgrade && self.rng.chance(0.5);
            self.note(|| format!("the owner asks mac{at} to take over{}{}", if force { " (forced)" } else { "" }, if downgrade { " (on any app)" } else { "" }));
            self.deliver(at, Msg::TakeOver { force, downgrade });
        }
        if self.rng.chance(P_APP) {
            let at = self.rng.below(n) as usize;
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

    /// The owner looks at the views after the faults: a term with no lead they'd show as such
    /// (its lead gone for good, stood down for its app, or the term unreadable) for five minutes
    /// is taken over from a member that can lead it (by the owner's downgrade when none's app is
    /// new enough).
    fn no_lead(&mut self) {
        let Some((&h, &(_, at))) = self.made.iter().next_back() else { return };
        if self.t < at + NO_LEAD_S || self.macs.iter().any(|m| m.leads == Some(h)) {
            return;
        }
        let named = self.files.get(&term::path(h)).and_then(|b| serde_json::from_slice::<Term>(b).ok());
        let force = match &named {
            None => true,
            Some(t) => {
                let gone = self.macs.get(mac_of(&t.member)).is_some_and(|m| m.gone);
                let stood = self.files.get(&super::beat::path(&t.member)).and_then(|b| serde_json::from_slice::<Beat>(b).ok()).is_some_and(|b| b.stood_down == Some(h));
                if !gone && !stood {
                    return;
                }
                gone
            }
        };
        // (A Mac whose app is new enough; else any, the lead that stood down too, by the owner's
        // downgrade.)
        let known = (1..=h).rev().find_map(|e| self.files.get(&term::path(e)).and_then(|b| serde_json::from_slice::<Term>(b).ok())).map(|t| t.app).unwrap_or_default();
        let can: Vec<usize> = (0..self.macs.len()).filter(|&k| !self.macs[k].gone).collect();
        let Some(&first) = can.first() else { return };
        let (at, downgrade) = match can.iter().find(|&&k| term::app_at_least(&app(self.macs[k].app), &known) && named.as_ref().is_none_or(|t| t.member != self.macs[k].id)) {
            Some(&k) => (k, false),
            None => (first, true),
        };
        self.count("the owner takes over a term with no lead");
        self.note(|| format!("the owner sees no lead of term {h}: asks mac{at} to take over{}", if downgrade { " on its app" } else { "" }));
        self.deliver(at, Msg::TakeOver { force: force || downgrade, downgrade });
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
            let next = self.macs.iter().map(|m| m.asleep_until.max(m.busy_until).max(m.ready_at)).min().unwrap_or(t + 1);
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
        let names: BTreeSet<&str> = self.files.keys().filter_map(|k| k.strip_prefix(&prefix)).filter_map(|r| r.split('/').next()).filter(|n| !n.ends_with(".tmp")).collect();
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
        if let Some(e) = term_of(path) {
            let want = self.made.keys().max().map_or(1, |m| m + 1);
            if e != want {
                self.wrong(format!("mac{me} made term {e} when the next was {want}"));
            }
            self.made.insert(e, (me, self.t));
            self.note(|| format!("mac{me} makes term {e}"));
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
        let b = self.files.remove(tmp).unwrap_or_default();
        let since = self.tmps.remove(tmp).unwrap_or(self.t);
        // (Its maker writing the bytes its create couldn't: still its create's; or the same bytes
        // again, an earlier finish's answer lost: no change.)
        let mine = self.writes.get(path).map(|w| w.0) == Some(me);
        let finishing = mine && self.files.get(path).is_some_and(|x| x.is_empty());
        if mine && term_of(path).is_some() && self.files.get(path) == Some(&b) {
            self.see(me, path, Some(b));
            return true;
        }
        if let Some(&(who, at)) = self.writes.get(path) {
            if who != me && at > since {
                self.count("late renames over another's write");
                self.note(|| format!("mac{me}'s rename of {path}, written at {since}, lands over mac{who}'s write at {at}"));
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
        if term_of(path).is_some() || records_of(path).is_some() {
            self.wrong(format!("mac{me} removed {path}"));
        }
        if let Some(key) = entry_of(path) {
            if !self.cfg.draft {
                self.wrong(format!("mac{me} removed entry {key}"));
            }
        }
        self.files.remove(path);
        self.see(me, path, None);
    }

    /// A file's bytes landed: checked by what it is. `made`: by its create (or its maker finishing
    /// it).
    fn landed(&mut self, me: usize, path: &str, b: &[u8], made: bool) {
        if let Some(e) = term_of(path) {
            if !made {
                return self.wrong(format!("mac{me} wrote over term {e}'s file"));
            }
            let Ok(t) = serde_json::from_slice::<Term>(b) else { return self.wrong(format!("term {e}'s file isn't a term")) };
            let term_at = |w: &World, e: u64| w.files.get(&term::path(e)).and_then(|b| serde_json::from_slice::<Term>(b).ok());
            if t.term != e || !self.macs.iter().any(|m| m.id == t.member) {
                return self.wrong(format!("term {e}'s file names term {} and {}", t.term, t.member));
            }
            // The app rule: against the term before; a take-back's, against the term it handed
            // over; forced past a term that can't be read, against an older (`term::force`); and
            // not on the owner's downgrade.
            let back = t.how.starts_with(term::BACK);
            let against = if back { e.checked_sub(2).and_then(|p| term_at(self, p)) } else { e.checked_sub(1).and_then(|p| term_at(self, p)) };
            if let Some(p) = against.filter(|p| !term::app_at_least(&t.app, &p.app) && !t.how.contains(term::UNREAD) && !t.how.contains(term::DOWNGRADE)) {
                self.wrong(format!("term {e} has app {}, older than term {}'s {}", t.app, p.term, p.app));
            }
            if t.how.contains(term::DOWNGRADE) {
                self.count("forced downgrades");
            }
            if t.how.contains("saved state lost") {
                self.count("re-assertions with a saved state lost");
            }
            if back && e.checked_sub(1).and_then(|p| term_at(self, p)).is_some_and(|p| !term::app_at_least(&t.app, &p.app)) {
                self.count("take-backs from a newer app");
            }
            let kind = ["handed over", "taken back", "taken over", "re-asserted", "restarted", "the build Mac"].into_iter().find(|k| t.how.starts_with(k)).unwrap_or("other");
            *self.counts.entry(kind).or_default() += 1;
            self.note(|| format!("term {e}: {} ({}{})", t.member, t.how, t.seq.map(|s| format!(", records at {s}")).unwrap_or_default()));
        } else if let Some(e) = records_of(path) {
            let r = match serde_json::from_slice::<Records>(b) {
                Ok(r) => r,
                Err(err) => return self.wrong(format!("term {e}'s records can't be parsed: {err}")),
            };
            if let Err(why) = consistent(&r) {
                self.wrong(format!("term {e}'s records at {} aren't consistent: {why}", r.seq));
            }
            let lead = self.files.get(&term::path(e)).and_then(|b| serde_json::from_slice::<Term>(b).ok()).map(|t| t.member);
            // (Term 1's first, made with create-new before term 1 is, its bytes landing when its
            // maker gets to them: before any other write, which its open file holds off.)
            let first = e == 1 && made;
            if r.term != e || (!first && lead.as_deref() != Some(self.macs[me].id.as_str())) {
                self.wrong(format!("mac{me} wrote term {e}'s records (of term {}); term {e} names {lead:?}", r.term));
            }
            let handled: BTreeSet<String> = r.reflected.iter().chain(r.rejected.keys()).cloned().collect();
            if let Some((seq, was)) = self.landed.get(&e) {
                let back = (r.seq <= *seq).then(|| format!("term {e}'s records went from {seq} to {}", r.seq));
                let lost = was.difference(&handled).next().map(|k| format!("term {e}'s records at {} lost {k}", r.seq));
                for w in back.into_iter().chain(lost) {
                    self.wrong(w);
                }
            }
            if self.made.contains_key(&(e + 1)) {
                // (A stale lead's, where no one reads: invariant 5.)
                self.count("records saved after a later term began");
            }
            self.note(|| format!("mac{me} saves term {e}'s records at {} ({} entries)", r.seq, handled.len()));
            if let Some(h) = &r.handed {
                self.settles.insert((e, r.seq), h.clone());
            }
            self.landed.insert(e, (r.seq, handled));
        } else if let Some(key) = entry_of(path) {
            if let Ok(en) = serde_json::from_slice::<Entry>(b) {
                let kind = match en.step.as_str() {
                    "bogus" => Kind::Refused,
                    "flaky" => Kind::Either,
                    _ => Kind::Taken,
                };
                self.writers.insert(key.clone(), mac_of(&en.member));
                self.written_at.entry(key.clone()).or_insert(self.t);
                self.written.insert(key, kind);
            }
        }
    }

    // What the Macs say they believe and do.

    fn view(&mut self, me: usize, e: u64) {
        if e < self.macs[me].view {
            self.wrong(format!("mac{me}'s term went back from {} to {e}", self.macs[me].view));
        }
        self.macs[me].view = e;
    }

    fn leads(&mut self, me: usize, e: Option<u64>) {
        if let Some(e) = e {
            let named = self.files.get(&term::path(e)).and_then(|b| serde_json::from_slice::<Term>(b).ok()).map(|t| t.member);
            if named.as_deref() != Some(self.macs[me].id.as_str()) {
                self.wrong(format!("mac{me} leads term {e}, which names {named:?}"));
            }
            if let Some(&other) = self.leaders.get(&e).filter(|&&o| o != me) {
                self.wrong(format!("mac{me} leads term {e}, which mac{other} led"));
            }
            if e <= self.macs[me].led {
                self.wrong(format!("mac{me} leads term {e} after term {}", self.macs[me].led));
            }
            self.leaders.insert(e, me);
            self.macs[me].led = e;
            self.count("take-ups");
            self.note(|| format!("mac{me} leads term {e}"));
        } else if let Some(was) = self.macs[me].leads {
            self.note(|| format!("mac{me} no longer leads term {was}"));
        }
        self.macs[me].leads = e;
    }

    fn ack(&mut self, me: usize, term: u64, keys: &[String]) {
        if self.made.contains_key(&(term + 1)) {
            self.count("acknowledgements after a later term began");
        }
        for key in keys {
            self.acked.insert(key.clone());
            if !self.landed.get(&term).is_some_and(|(_, h)| h.contains(key)) {
                self.wrong(format!("mac{me} acknowledged {key} as term {term}'s before a snapshot of it named it"));
            }
        }
    }

    /// A Mac took up a term: a handover's with the coordinator's state its old lead settled with,
    /// saved with the snapshot the term names (review M7: the records only).
    fn took_up(&mut self, me: usize, e: u64, how: &str, handed: Option<&serde_json::Value>) {
        let Some(t) = self.files.get(&term::path(e)).and_then(|b| serde_json::from_slice::<Term>(b).ok()) else { return };
        let Some(seq) = t.seq.filter(|_| how.starts_with("handed over")) else { return };
        match self.settles.get(&(t.from, seq)) {
            Some(v) if handed == Some(v) => self.count("coordinator states handed over"),
            v => {
                let v = v.cloned();
                self.wrong(format!("mac{me} took up term {e}, handed over at term {}'s {seq}, with the coordinator's state {handed:?}, not {v:?} as its old lead settled", t.from));
            }
        }
    }

    /// Mac `me` began a listing of every day of the journal.
    fn full_listing(&mut self, me: usize) {
        self.full_at[me] = Some(self.t);
    }

    /// Mac `me`, leading term `e`, says its records reflect the journal (review N2): its term's last
    /// snapshot names every entry written before its take-up's listing began (a listing is stale up
    /// to `Cfg::stale`).
    fn caught_up(&mut self, me: usize, e: u64) {
        if !self.caught.insert(e) {
            return;
        }
        let Some(at) = self.full_at[me] else { return self.wrong(format!("mac{me} says term {e}'s records are caught up, and listed none")) };
        let named = self.landed.get(&e).map(|(_, h)| h.clone()).unwrap_or_default();
        let missing: Vec<String> = self.written_at.iter().filter(|(k, &w)| w + self.cfg.stale + 1 < at && !named.contains(*k)).map(|(k, _)| k.clone()).collect();
        if let Some(k) = missing.first() {
            self.wrong(format!("mac{me} says term {e}'s records are caught up; they lack {k}, written before its listing ({} in all)", missing.len()));
        }
        self.count("leads caught up");
    }

    /// Entries merged: counted by what they were.
    fn merged(&mut self, applied: &[String], overtaken: usize, listed: bool) {
        let again = applied.iter().filter(|k| self.acked.contains(*k)).count() as u64;
        *self.counts.entry("acknowledged entries merged again by a later lead").or_default() += again;
        let refused = applied.iter().filter(|k| self.files.contains_key(&format!("{}/{k}.why", journal::REJECTED))).count() as u64;
        *self.counts.entry("entries another lead refused, taken").or_default() += refused;
        *self.counts.entry("entries passed over for a later lease's").or_default() += overtaken as u64;
        if listed {
            *self.counts.entry("entries found by a listing").or_default() += applied.len() as u64;
        }
    }

    /// The checks at the end of a run: one Mac leads the last term, made before its config's end's
    /// last ten minutes (a run gone on while the lead merges makes none), and its records name
    /// every entry ever written.
    fn finish(&mut self) {
        let Some((&h, &(_, at))) = self.made.iter().next_back() else { return self.wrong("no term was made".into()) };
        if self.cfg.draft {
            let r = self.files.get(DRAFT).and_then(|b| serde_json::from_slice::<Records>(b).ok()).unwrap_or_default();
            return self.lost(&r, "the shared records");
        }
        if at + QUIET_S > self.cfg.end {
            self.wrong(format!("term {h} was made at {at} s, in the run's last ten minutes (of {} s) or after: the terms don't settle", self.cfg.end));
        }
        let Some(named) = self.files.get(&term::path(h)).and_then(|b| serde_json::from_slice::<Term>(b).ok()).map(|t| t.member) else {
            return self.wrong(format!("term {h}'s file isn't whole at the end"));
        };
        for k in 0..self.macs.len() {
            let (is, leads) = (self.macs[k].id == named, self.macs[k].leads);
            if is && leads != Some(h) {
                self.wrong(format!("term {h} names mac{k}, which leads {leads:?} at the end"));
            }
            if !is && leads.is_some() {
                self.wrong(format!("mac{k} still leads term {leads:?} at the end, after term {h}"));
            }
        }
        match self.files.get(&records::path(h)).and_then(|b| serde_json::from_slice::<Records>(b).ok()) {
            Some(r) => self.lost(&r, &format!("term {h}'s last records")),
            None => self.wrong(format!("term {h} has no records at the end")),
        }
    }

    fn lost(&mut self, r: &Records, whose: &str) {
        if let Err(why) = consistent(r) {
            self.wrong(format!("{whose} aren't consistent: {why}"));
        }
        let lost: Vec<String> = self.written.iter().filter(|&(k, &kind)| !names(r, k, kind)).map(|(k, _)| k.clone()).collect();
        for k in lost {
            let gone = self.writers.get(&k).is_some_and(|&g| self.macs.get(g).is_some_and(|m| m.gone));
            self.wrong(format!("entry {k}{} is lost: {whose} don't name it", if gone { " (of a Mac gone for good)" } else { "" }));
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

    /// A job's lease in term `e`: the next granted in it.
    fn lease(&self, e: u64) -> Result<LeaseId> {
        self.op(|w, _| {
            let n = w.leases.entry(e).or_default();
            *n += 1;
            LeaseId { term: e, n: *n }
        })
    }

    fn send(&self, to: usize, m: Msg) -> Result<()> {
        self.op(|w, _| w.deliver(to, m))
    }

    fn view(&self, e: u64) -> Result<()> {
        self.op(|w, me| w.view(me, e))
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
            // A member (not leading, nor named by the newest term) leaves for good, between two of
            // its loops: its lid closed for the week.
            if w.cfg.leave && !w.macs.iter().any(|m| m.gone) && w.t > w.cfg.faults / 3 && w.t < w.cfg.faults && w.macs[me].leads.is_none() {
                let h = w.made.keys().max().copied().unwrap_or(0);
                let named = w.files.get(&term::path(h)).and_then(|b| serde_json::from_slice::<Term>(b).ok()).map(|t| t.member);
                if named.as_deref() != Some(w.macs[me].id.as_str()) && w.rng.chance(0.02) {
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
            w.read(me, path)
        })
    }

    fn exists(&self, path: &str) -> Result<bool> {
        self.op(|w, me| {
            w.slow(me);
            w.fetch(me, path).is_some()
        })
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.op(|w, me| w.list(me, dir))
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.op(|w, me| w.remove(me, path))
    }
}

impl Io for Sim {
    fn now(&self) -> u64 {
        self.op(|w, me| w.clock(me)).unwrap_or(0)
    }

    fn awake(&self) -> u64 {
        self.op(|w, me| w.awake_clock(me)).unwrap_or(0)
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
    /// The current term as its driver said last (its jobs' leases are of it).
    term: u64,
    /// The term it leads, as the world was told.
    leads: Option<u64>,
    /// The listing its driver asked for, made after its step, for its next.
    listed: Option<Listed>,
    /// Its coordinator's state (what a handover hands on): the term it's of, and the jobs granted.
    coord: (u64, u64),
    /// Settling: the coordinator's state it wrote, and whether its driver has it.
    settled: Option<serde_json::Value>,
    gave: bool,
}

impl Mac {
    fn new(sim: Sim, k: usize, seed: u64) -> Mac {
        let me = Member { id: id(k), host: format!("mac{k}"), app: app(0) };
        let rng = Rng(seed ^ (k as u64 + 1).wrapping_mul(0xA076_1D64_78BD_642F));
        let driver = Driver::new(me.clone(), driver::Saved::default());
        Mac { sim, k, rng, me, driver, jobs: 0, term: 0, leads: None, listed: None, coord: (0, 0), settled: None, gave: false }
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
                    // (Its memory and its saved state both gone: what it believed starts again.)
                    w.macs[me].view = 0;
                    w.count("saved states lost");
                }
                lost
            })? {
                saved = driver::Saved::default();
            }
            self.driver = Driver::new(self.me.clone(), saved);
            (self.listed, self.coord, self.settled, self.gave) = (None, (0, 0), None, false);
            self.sim.count("restarts");
            if self.leads.take().is_some() {
                self.sim.leads(None)?;
            }
        }
        let faulting = self.sim.faulting()?;
        let mut heard = Heard { able: self.rng.chance(0.9), listed: self.listed.take(), ..Default::default() };
        for m in self.sim.inbox()? {
            match m {
                Msg::Pool { from, msg } => heard.msgs.push((id(from), msg)),
                Msg::HandTo(to) => heard.asks.push(Ask::HandTo(to)),
                Msg::TakeOver { force, downgrade } => heard.asks.push(Ask::TakeOver { force, downgrade }),
            }
        }
        if faulting && self.rng.chance(P_JOB) {
            let lease = self.sim.lease(self.term)?;
            let now = Io::now(&self.sim);
            heard.entries.push(self.job(lease, now));
        }
        if let Some(s) = self.settled.as_ref().filter(|_| !self.gave) {
            heard.settled = Some(s.clone());
            self.gave = true;
        }
        heard.reassert = faulting && self.leads.is_some() && self.rng.chance(P_SWEEP);
        let out = self.driver.step(&self.sim, heard, &check);
        self.after(out, faulting)
    }

    /// What its driver's step came to, done.
    fn after(&mut self, out: Out, faulting: bool) -> Result<()> {
        self.term = out.term;
        self.sim.view(out.term)?;
        for ev in &out.events {
            self.event(ev)?;
        }
        if out.leads != self.leads {
            self.sim.leads(out.leads)?;
            self.leads = out.leads;
        }
        if let (Some(e), true) = (out.leads, out.caught_up) {
            self.sim.op(|w, me| w.caught_up(me, e))?;
        }
        // Its coordinator: grants while its lead may; settling, writes its state once.
        if out.leads.is_some() && out.duties && faulting && self.rng.chance(P_GRANT) {
            self.coord.1 += 1;
        }
        if out.settle {
            if self.settled.is_none() {
                self.settled = Some(serde_json::json!({ "term": self.coord.0, "grants": self.coord.1 }));
                self.gave = false;
            }
        } else {
            self.settled = None;
        }
        for (to, m) in out.send {
            if let driver::Msg::Ack { term, keys, .. } = &m {
                self.sim.op(|w, me| w.ack(me, *term, keys))?;
            }
            self.sim.send(mac_of(&to), Msg::Pool { from: self.k, msg: m })?;
        }
        if let Err(e) = self.sim.beat(out.beat) {
            if self.sim.over() {
                return Err(e);
            }
        }
        // A job reading the records.
        if out.term > 0 && self.rng.chance(P_READ) {
            if let Ok(Some(r)) = Records::newest(&self.sim, out.term) {
                if let Err(why) = consistent(&r) {
                    self.sim.op(|w, me| w.wrong(format!("mac{me} read term {}'s records at {}, not consistent: {why}", r.term, r.seq)))?;
                }
            }
        }
        // The listing its driver asked for, off its step.
        if let Some(l) = out.list {
            self.sim.count("listings");
            if l.since.is_none() {
                self.sim.op(|w, me| w.full_listing(me))?;
            }
            self.listed = journal::list(&self.sim, l.since.as_deref()).ok().map(|keys| Listed { n: l.n, keys });
        }
        self.sim.idle()
    }

    fn event(&mut self, ev: &Event) -> Result<()> {
        let k = self.k;
        match ev {
            Event::TookUp { term, how, handed } => {
                self.sim.op(|w, me| w.took_up(me, *term, how, handed.as_ref()))?;
                self.coord = match handed {
                    Some(v) => (v["term"].as_u64().unwrap_or(0), v["grants"].as_u64().unwrap_or(0)),
                    None => (*term, 0),
                };
                (self.settled, self.gave) = (None, false);
            }
            Event::Merged { applied, overtaken, listed, .. } => self.sim.quiet(|w| w.merged(applied, *overtaken, *listed)),
            Event::Waits { what: "take up", .. } => self.sim.count("take-ups tried again"),
            Event::Failed { what: "save the records", .. } => self.sim.count("saves tried again"),
            Event::SteppedDown { why, .. } if why.starts_with("can't re-assert") => self.sim.count("stood down"),
            _ => {}
        }
        self.sim.note(|| format!("mac{k}: {ev:?}"));
        Ok(())
    }

    /// A job's hand-off, of lease `lease`: a unit built with a new key, now and then a prune, one
    /// of a step that doesn't exist (refused), or one the lead of an odd term refuses.
    fn job(&mut self, lease: LeaseId, now: u64) -> Entry {
        self.jobs += 1;
        let t = TARGETS[self.rng.below(TARGETS.len() as u64) as usize];
        let k = format!("k{}x{}", self.k, self.jobs);
        let built = |k: &str| Handoff { changes: [(logical(t), Some(content(t, k)))].into(), pending: [(content(t, k), "sha".into())].into(), done: Some(("unit".into(), vec![(t.into(), k.to_string())])), ..Default::default() };
        let (step, h) = match self.rng.below(20) {
            0 => ("bogus", Handoff { changes: [(logical(t), Some(content(t, "bogus")))].into(), done: Some(("unit".into(), vec![(t.into(), "bogus".into())])), ..Default::default() }),
            1 | 2 => ("prune", Handoff { changes: [(logical(t), None)].into(), done: Some(("prune".into(), vec![(format!("unit {t}"), String::new())])), ..Default::default() }),
            3 | 4 => ("flaky", built(&k)),
            _ => ("unit", built(&k)),
        };
        Entry { member: self.me.id.clone(), lease, step: step.into(), handoff: h, at: now }
    }
}

/// A Mac running the first draft's scheme: terms as the pool's, but one shared records file the
/// lead reads, merges the journal into, writes after checking the term, and the merged entries
/// then taken off the journal.
struct Draft {
    sim: Sim,
    k: usize,
    rng: Rng,
    me: Member,
    cur: Current,
    leads: Option<u64>,
    led: u64,
    jobs: u64,
}

impl Draft {
    fn new(sim: Sim, k: usize, seed: u64) -> Draft {
        let me = Member { id: id(k), host: format!("mac{k}"), app: app(0) };
        let rng = Rng(seed ^ (k as u64 + 1).wrapping_mul(0xA076_1D64_78BD_642F));
        Draft { sim, k, rng, me, cur: Current::default(), leads: None, led: 0, jobs: 0 }
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

    fn step_down(&mut self) -> Result<()> {
        if self.leads.take().is_some() {
            self.sim.leads(None)?;
        }
        Ok(())
    }

    fn turn(&mut self) -> Result<()> {
        let now = Io::now(&self.sim);
        let msgs = self.sim.inbox()?;
        if self.cur.term == 0 {
            term::bootstrap(&self.sim, &self.me, now, false)?;
            self.cur = term::current(&self.sim)?;
        }
        if self.cur.term > 0 && self.cur.lead.is_none() {
            self.cur.lead = term::read(&self.sim, self.cur.term)?;
        }
        while self.cur.term > 0 && term::next(&self.sim, self.cur.term)? {
            let e = self.cur.term + 1;
            self.cur = Current { term: e, lead: term::read(&self.sim, e)? };
        }
        self.sim.view(self.cur.term)?;
        if self.leads.is_some_and(|e| self.cur.term > e) {
            self.step_down()?;
        }
        if self.leads.is_none() {
            if let Some(t) = self.cur.lead.clone().filter(|t| t.member == self.me.id && t.term > self.led) {
                self.led = t.term;
                self.leads = Some(t.term);
                self.sim.leads(Some(t.term))?;
            }
        }
        for m in msgs {
            if let Msg::TakeOver { force, .. } = m {
                if self.leads.is_none() && self.cur.term > 0 {
                    let gone = match &self.cur.lead {
                        Some(t) => Beat::read(&self.sim, &t.member)?.is_none_or(|b| b.out_of_touch(now)),
                        None => true,
                    };
                    if gone || force {
                        if let Ok(Some(t)) = term::claim(&self.sim, &self.cur, &self.me, "taken over", now) {
                            self.cur = Current { term: t.term, lead: Some(t) };
                            self.sim.view(self.cur.term)?;
                        }
                    }
                }
            }
        }
        if let Some(e) = self.leads {
            let mut r: Records = match self.sim.read(DRAFT)? {
                Some(b) => serde_json::from_slice(&b)?,
                None => Records::load(&self.sim, 1)?.unwrap_or_default(),
            };
            let keys = journal::list(&self.sim, None)?;
            let m = records::merge(&self.sim, &mut r, &keys, &check);
            if !m.applied.is_empty() || !m.refused.is_empty() || !m.overtaken.is_empty() {
                // Check the term, then write: two steps, and the write's rename may land late.
                if term::next(&self.sim, e)? {
                    self.step_down()?;
                } else if self.sim.write_whole(DRAFT, &serde_json::to_vec(&r)?).is_ok() {
                    for k in m.applied.iter().chain(&m.overtaken).chain(m.refused.iter().map(|(k, _)| k)) {
                        self.sim.remove(&journal::path(k))?;
                    }
                }
            }
        }
        if self.sim.faulting()? && self.rng.chance(P_JOB) {
            self.jobs += 1;
            let t = TARGETS[self.rng.below(TARGETS.len() as u64) as usize];
            let k = format!("k{}x{}", self.k, self.jobs);
            let h = Handoff { changes: [(logical(t), Some(content(t, &k)))].into(), done: Some(("unit".into(), vec![(t.into(), k.clone())])), ..Default::default() };
            let e = Entry { member: self.me.id.clone(), lease: LeaseId { term: self.cur.term, n: self.jobs * 8 + self.k as u64 }, step: "unit".into(), handoff: h, at: now };
            journal::write(&self.sim, &e).ok();
        }
        let b = Beat { member: self.me.id.clone(), host: self.me.host.clone(), app: self.me.app.clone(), leads: self.leads, ..Default::default() };
        self.sim.beat(b).ok();
        self.sim.idle()
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
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| if cfg.draft { Draft::new(sim, k, seed).run() } else { Mac::new(sim, k, seed).run() }));
                if let Err(p) = r {
                    let why = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
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
    Ran { wrong: std::mem::take(&mut w.wrong), counts: std::mem::take(&mut w.counts), trace: w.trace.take().unwrap_or_default() }
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
        panic!("{} of {n} schedules went wrong (seeds {seeds:?}…); seed {seed} ({:?}):\n{}\n\nits last events:\n{}", bad.len(), cfg(*seed), r.wrong.join("\n"), tail.join("\n"));
    }
    counts
}

/// Each kind of change of lead and fault, and what the knobs bring, that the default runs must see
/// at least three times (a simulator that never got there would pass too).
const KINDS: [&str; 27] = [
    "handed over",
    "taken back",
    "taken over",
    "re-asserted",
    "restarted",
    "sleeps",
    "busy renames",
    "late renames over another's write",
    "take-ups tried again",
    "saves tried again",
    "creates cut short",
    "lost answers",
    "listing seconds",
    "old journal days",
    "Macs gone for good",
    "stood down",
    "forced downgrades",
    "take-backs from a newer app",
    "coordinator states handed over",
    "entries found by a listing",
    "entries passed over for a later lease's",
    "entries another lead refused, taken",
    "acknowledged entries merged again by a later lead",
    "operation seconds",
    "saved states lost",
    "re-assertions with a saved state lost",
    "leads caught up",
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
    let (from, n, faults) = (var("POOL_SIM_FROM", 1_000_000), var("POOL_SIM_SEEDS", 100_000), var("POOL_SIM_MINUTES", 240) * 60);
    let started = std::time::Instant::now();
    let counts = check_all(from..from + n, |seed| Cfg { faults, end: faults + 1500, ..Cfg::mixed(seed) });
    eprintln!("{n} schedules of {} minutes' faults in {:.0} s\n{counts:#?}", faults / 60, started.elapsed().as_secs_f64());
}

#[test]
fn a_run_goes_on_only_while_its_lead_merges() {
    // (The long run's: hours of faults with no lead leave hundreds of entries, and on a share
    // taking seconds an operation the lead that takes over merges a dozen a loop, past the run's
    // end. The run goes on while its records lack fewer entries each ten minutes; a lead that
    // merges none is wrong at the end, as before.)
    let mut w = World::new(0, 4, Cfg::pool(), false);
    let (k1, k2) = ("2026-10-06/1-1".to_string(), "2026-10-06/1-2".to_string());
    w.made.insert(1, (0, 0));
    w.written.insert(k1.clone(), Kind::Taken);
    w.written.insert(k2.clone(), Kind::Taken);
    let mut r = Records { term: 1, seq: 1, ..Default::default() };
    let save = |w: &mut World, r: &Records| w.files.insert(records::path(1), serde_json::to_vec(r).unwrap());
    save(&mut w, &r);
    w.t = w.cfg.end;
    assert!(w.goes_on(), "two entries lacking: ten minutes more");
    assert!(!w.goes_on(), "none merged since: the run ends");
    r.reflected.insert(k1);
    save(&mut w, &r);
    assert!(w.goes_on(), "one merged since: ten minutes more");
    (w.t, w.behind) = (w.cfg.end + MERGING_S, Some(2));
    assert!(!w.goes_on(), "gone on two hours: the run ends");
    (w.t, w.behind) = (w.cfg.end, None);
    r.reflected.insert(k2);
    save(&mut w, &r);
    assert!(!w.goes_on(), "every entry named: the run ends");
}

#[test]
fn a_lead_keeps_leading_over_slow_listings_and_a_week_of_journal() {
    // Listings of 3 to 33 s a folder, and a week of the journal's days to list. (Review H1: a
    // take-up listed the journal in its loop, the loop ran past a minute, and the next re-asserted
    // and took up again, for ever; a handover's take-up ran past its two minutes and was taken
    // back.)
    let counts = check_all(0..300, |_| Cfg { list_s: (3, 33), old_days: 7, ..Cfg::pool() });
    for what in ["handed over", "taken over", "entries found by a listing"] {
        assert!(counts.get(what).is_some_and(|&n| n >= 3), "{what}: {:?}", counts.get(what));
    }
}

#[test]
fn a_development_build_or_a_rollback_leaves_a_lead() {
    // (Review H2: a lead restarted into an older app had its re-assertion refused, and stopped
    // there every loop; a handover to a newer app that didn't take up couldn't be taken back, nor
    // taken over from an older app.)
    let counts = check_all(0..1000, |_| Cfg { downgrade: true, leave: true, ..Cfg::pool() });
    for what in ["stood down", "forced downgrades"] {
        assert!(counts.get(what).is_some_and(|&n| n >= 1), "{what}: {:?}", counts.get(what));
    }
}

#[test]
fn a_mac_gone_for_good_loses_none_of_its_entries() {
    // A member leaves for good partway: what it never told a lead of, or told a lead no longer
    // current, is found by the lead's listings. (Review L7: only by the next take-up's.)
    let counts = check_all(0..1000, |_| Cfg { leave: true, ..Cfg::pool() });
    assert!(counts.get("Macs gone for good").is_some_and(|&n| n >= 100), "{counts:?}");
}

#[test]
fn the_first_drafts_shared_records_lose_an_update_under_a_delayed_rename() {
    // (Every read fresh: what's lost is the renames'.)
    let cfg = Cfg { draft: true, stale: 0, ..Cfg::pool() };
    let (bad, _) = run_all(0..500, |_| cfg);
    let Some(&(seed, _)) = bad.iter().find(|(_, wrong)| wrong.iter().any(|w| w.contains("is lost"))) else {
        panic!("the first draft's scheme lost nothing in 500 schedules: the simulator doesn't find its lost update")
    };
    // Seen as it happens: a lead's rename of the shared records, written before it slept, lands
    // over the next lead's, whose merged entries are off the journal by then.
    let r = run(seed, cfg, true);
    assert!(r.counts.get("late renames over another's write").is_some_and(|&n| n > 0), "seed {seed}: {:?}", r.wrong);
    let late: Vec<&String> = r.trace.iter().filter(|l| l.contains("rename of state/build/records.json")).collect();
    assert!(!late.is_empty(), "seed {seed}");
    eprintln!("the first draft, seed {seed}:\n{}\n{}", late.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n"), r.wrong.join("\n"));
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
    let knobs: [(&str, Cfg); 14] = [
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
    ];
    for (name, cfg) in knobs {
        let (bad, c) = run_all(0..n, |_| cfg);
        let get = |k: &str| c.get(k).copied().unwrap_or(0);
        eprintln!("{name}: {} of {n} wrong; handed over {}, taken back {}, taken over {}, re-asserted {}, stood down {}{}", bad.len(), get("handed over"), get("taken back"), get("taken over"), get("re-asserted"), get("stood down"), bad.first().map(|(s, w)| format!("; seed {s}: {}", w[0])).unwrap_or_default());
    }
}

#[test]
#[ignore]
fn every_seed_runs_the_same_every_time() {
    // POOL_SIM_SEEDS seeds (400), each run twice: the same events, wrongs and counts.
    let n = std::env::var("POOL_SIM_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(400);
    let differ: Vec<u64> = (0..n).filter(|&seed| {
        let (a, b) = (run(seed, Cfg::mixed(seed), true), run(seed, Cfg::mixed(seed), true));
        a.trace != b.trace || a.wrong != b.wrong || a.counts != b.counts
    }).collect();
    assert!(differ.is_empty(), "{} of {n} seeds ran differently: {differ:?}", differ.len());
}

#[test]
#[ignore]
fn a_seeds_events() {
    // POOL_SIM_SEED's events, its knobs as by default, or as POOL_SIM_KNOBS lists them (none:
    // "plain"; "draft", "list", "old", "leave", "downgrade", "cuts", "slow", "lose", "skew=<s>",
    // "stale=<s>"), its faults POOL_SIM_MINUTES long (40), to look into one.
    let seed = std::env::var("POOL_SIM_SEED").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let cfg = match std::env::var("POOL_SIM_KNOBS") {
        Ok(knobs) => knobs.split(',').fold(Cfg::pool(), |c, k| match k.split_once('=') {
            Some(("skew", v)) => Cfg { skew: v.parse().unwrap_or(0), ..c },
            Some(("stale", v)) => Cfg { stale: v.parse().unwrap_or(30), ..c },
            _ => match k {
                "draft" => Cfg { draft: true, ..c },
                "list" => Cfg { list_s: (3, 33), ..c },
                "old" => Cfg { old_days: 7, ..c },
                "leave" => Cfg { leave: true, ..c },
                "downgrade" => Cfg { downgrade: true, ..c },
                "cuts" => Cfg { cuts: true, ..c },
                "slow" => Cfg { op_s: (1, 4), ..c },
                "lose" => Cfg { lose: true, ..c },
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
