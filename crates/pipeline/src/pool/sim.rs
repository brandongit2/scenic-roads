//! The pool's simulator (docs/pool.md §13): two to four Macs running the protocol over a model of
//! the NAS's faults, on a schedule drawn from a seed, with the invariants of §4 checked at every
//! step that could break one.
//!
//! The NAS's model (`World`): create-new is atomic on the server, and the file's bytes land a step
//! later (a Mac asleep in between leaves it empty meanwhile); a whole write is its temporary file,
//! then its rename a step later (a Mac asleep in between renames when it wakes, over whatever
//! others wrote meanwhile); a rename over a file another Mac has open fails busy, and the write
//! fails after a few tries; each Mac keeps what it read, stat'ed and listed for up to `STALE`
//! seconds, so it may read an older version, miss a new file, or list a folder as it was; its own
//! writes it sees at once.
//!
//! Each Mac runs in a thread of its own, but one at a time: every NAS operation, message and look
//! at the clock waits for the Mac's turn, and after each the schedule (`World::after`) says whose
//! turn is next, puts the Mac to sleep there or not, and brings the owner's asks: hand the lead to
//! a Mac, take it over (forced at times, the lead alive), a newer app on a Mac. The Macs run the
//! protocol as the agent will (`Mac::turn`): a lead checks the next term, re-asserts after a gap,
//! merges what members told it, saves, acknowledges, and hands over; a member writes journal
//! entries for its jobs, tells the lead of them until it acknowledges them, answers an offer,
//! takes up a term that names it, and takes over when the owner asks. Once the faults stop every
//! Mac stays awake for a while; then the last term's lead must be leading, and its records must
//! name every entry ever written.
//!
//! The first draft's scheme (`Cfg::draft`: one shared records file the lead rewrites after checking
//! the term, and a journal it empties as it merges) runs on the same model, to show the model
//! finds what lost work there.

use super::beat::Beat;
use super::handover::{self, Do, Handover, Seen};
use super::journal::{self, Entry, LeaseId, Mine};
use super::nas::Nas;
use super::records::{self, Records};
use super::term::{self, Current, Term};
use super::Member;
use crate::handoff::Handoff;
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// The longest a Mac keeps what it read, stat'ed or listed (s).
const STALE: u64 = 30;
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

/// Per step of a Mac while the faults last: that it falls asleep right there.
const P_SLEEP: f64 = 0.004;
/// Per step: that the owner asks a Mac's menu to hand the lead to a Mac, asks a Mac to take over,
/// or installs a newer app on a Mac.
const P_ASK: f64 = 0.006;
const P_TAKE: f64 = 0.0012;
const P_APP: f64 = 0.0004;
/// Per step: that the turn stays with the Mac that has it.
const P_KEEP: f64 = 0.85;
/// Per loop of a Mac while the faults last: that one of its jobs ends (a journal entry); that one
/// reads the current records.
const P_JOB: f64 = 0.35;
const P_READ: f64 = 0.2;

/// What a run is.
#[derive(Clone, Copy, Debug)]
struct Cfg {
    /// The faults stop this long into the run (s); every Mac then stays awake.
    faults: u64,
    /// The run ends this long into it.
    end: u64,
    /// The first draft's scheme instead of the pool's.
    draft: bool,
}

impl Cfg {
    fn pool() -> Cfg {
        Cfg { faults: 2400, end: 3300, draft: false }
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

/// A message between Macs (the pool's HTTP API: it reaches only a Mac that's awake).
#[derive(Clone, Debug)]
enum Msg {
    /// A member tells the lead of an entry of its own.
    Tell { key: String, from: usize },
    /// The lead of `term` acknowledges an entry: applied, or refused.
    Ack { key: String, term: u64 },
    /// The owner, on this Mac's menu, asks to hand the lead to a member.
    HandTo(String),
    /// The owner asks this Mac to take the lead over (forced: without the out-of-touch check).
    TakeOver { force: bool },
}

/// A Mac as the world sees it.
struct MacW {
    id: String,
    app: u64,
    skew: i64,
    asleep_until: u64,
    /// Its next loop (it waits between loops).
    ready_at: u64,
    inbox: Vec<Msg>,
    /// What it says it believes, checked: the highest term it knows, the term it leads, the
    /// highest term it led.
    view: u64,
    leads: Option<u64>,
    led: u64,
}

/// What a Mac keeps of what it read, stat'ed and listed: the bytes (None: no file) or names, and
/// until when.
#[derive(Default)]
struct Cache {
    files: BTreeMap<String, (Option<Vec<u8>>, u64)>,
    dirs: BTreeMap<String, (Vec<String>, u64)>,
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
    turn: usize,
    done: bool,
    drained: bool,
    /// The terms made, by their maker; the term each Mac led.
    made: BTreeMap<u64, usize>,
    leaders: BTreeMap<u64, usize>,
    /// Each term's last snapshot landed: its number, and the entries it names.
    landed: BTreeMap<u64, (u64, BTreeSet<String>)>,
    /// Every entry whose bytes landed whole, and whether it's one the check refuses; those a lead
    /// acknowledged.
    written: BTreeMap<String, bool>,
    acked: BTreeSet<String>,
    wrong: Vec<String>,
    counts: Counts,
    trace: Option<Vec<String>>,
}

/// The app version of number `n`: a minute later each.
fn app(n: u64) -> String {
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

/// The lead's check of an entry: a step it doesn't know writes nothing.
fn check(e: &Entry, _: &Records) -> std::result::Result<(), String> {
    if e.step == "bogus" {
        Err("no such step".into())
    } else {
        Ok(())
    }
}

impl World {
    fn new(seed: u64, macs: usize, cfg: Cfg, tracing: bool) -> World {
        let mut rng = Rng(seed);
        let macs: Vec<MacW> = (0..macs).map(|k| MacW { id: id(k), app: 0, skew: rng.below(3) as i64 - 1, asleep_until: 0, ready_at: 0, inbox: Vec::new(), view: 0, leads: None, led: 0 }).collect();
        let mut files = BTreeMap::new();
        // Today's build Mac (mac0) and records: one unit built.
        files.insert(term::WRITER.to_string(), b"mac0\n".to_vec());
        files.insert("state/build/manifest.json".to_string(), serde_json::to_vec(&BTreeMap::from([(logical(TARGETS[0]), content(TARGETS[0], "k0"))])).unwrap());
        files.insert("state/build/jobs.json".to_string(), format!("{{\"unit\": {{\"{}\": \"k0\"}}}}", TARGETS[0]).into_bytes());
        let n = macs.len();
        let mut w = World { cfg, t: 0, rng, files, open: BTreeMap::new(), tmps: BTreeMap::new(), writes: BTreeMap::new(), caches: (0..n).map(|_| Cache::default()).collect(), macs, turn: 0, done: false, drained: false, made: BTreeMap::new(), leaders: BTreeMap::new(), landed: BTreeMap::new(), written: BTreeMap::new(), acked: BTreeSet::new(), wrong: Vec::new(), counts: BTreeMap::new(), trace: tracing.then(Vec::new) };
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
                m.asleep_until = m.asleep_until.min(t);
            }
            self.note(|| "the faults stop: every Mac awake from now".into());
        }
        if self.t >= self.cfg.end {
            self.done = true;
        }
    }

    fn faults(&mut self, me: usize) {
        if self.rng.chance(P_SLEEP) {
            let d = match self.rng.below(10) {
                0..=5 => self.rng.range(5, 90),
                6..=8 => self.rng.range(90, 900),
                _ => self.rng.range(900, 2400),
            };
            self.macs[me].asleep_until = self.t + d;
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
            self.note(|| format!("the owner asks mac{at} to take over{}", if force { " (forced)" } else { "" }));
            self.deliver(at, Msg::TakeOver { force });
        }
        if self.rng.chance(P_APP) {
            let at = self.rng.below(n) as usize;
            let newest = self.macs.iter().map(|m| m.app).max().unwrap_or(0);
            self.macs[at].app = newest + self.rng.range(1, 3);
            let v = app(self.macs[at].app);
            self.note(|| format!("mac{at} gets app {v}"));
        }
    }

    /// Whose turn is next: this Mac's again, most often, while it can run; else another's that can
    /// (awake, its loop due), at random; else time jumps to the next waking or loop.
    fn pick(&mut self, me: usize) {
        while !self.done {
            let t = self.t;
            let can = |m: &MacW| m.asleep_until <= t && m.ready_at <= t;
            if can(&self.macs[me]) && self.rng.chance(P_KEEP) {
                self.turn = me;
                return;
            }
            let ready: Vec<usize> = (0..self.macs.len()).filter(|&k| can(&self.macs[k])).collect();
            if !ready.is_empty() {
                self.turn = ready[self.rng.below(ready.len() as u64) as usize];
                return;
            }
            let next = self.macs.iter().map(|m| m.asleep_until.max(m.ready_at)).min().unwrap_or(t + 1);
            self.advance(next.max(t + 1));
        }
    }

    // The NAS.

    fn see(&mut self, me: usize, path: &str, v: Option<Vec<u8>>) {
        let until = self.t + STALE;
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
        let until = self.t + self.rng.below(STALE + 1);
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

    fn list(&mut self, me: usize, dir: &str) -> Vec<String> {
        if let Some((v, until)) = self.caches[me].dirs.get(dir) {
            if *until > self.t {
                return v.clone();
            }
        }
        let prefix = format!("{dir}/");
        let names: BTreeSet<&str> = self.files.keys().filter_map(|k| k.strip_prefix(&prefix)).filter_map(|r| r.split('/').next()).filter(|n| !n.ends_with(".tmp")).collect();
        let v: Vec<String> = names.into_iter().map(str::to_string).collect();
        let until = self.t + self.rng.below(STALE + 1);
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
        self.see(me, path, Some(Vec::new()));
        if let Some(e) = term_of(path) {
            let want = self.made.keys().max().map_or(1, |m| m + 1);
            if e != want {
                self.wrong(format!("mac{me} made term {e} when the next was {want}"));
            }
            self.made.insert(e, me);
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

    fn write_tmp(&mut self, tmp: &str, b: &[u8]) {
        self.files.insert(tmp.to_string(), b.to_vec());
        self.tmps.insert(tmp.to_string(), self.t);
    }

    fn rename(&mut self, me: usize, tmp: &str, path: &str) -> bool {
        if self.open.get(path).is_some_and(|h| h.iter().any(|(&who, &until)| who != me && until > self.t)) {
            self.count("busy renames");
            return false;
        }
        let b = self.files.remove(tmp).unwrap_or_default();
        let since = self.tmps.remove(tmp).unwrap_or(self.t);
        if let Some(&(who, at)) = self.writes.get(path) {
            if who != me && at > since {
                self.count("late renames over another's write");
                self.note(|| format!("mac{me}'s rename of {path}, written at {since}, lands over mac{who}'s write at {at}"));
            }
        }
        self.files.insert(path.to_string(), b.clone());
        self.writes.insert(path.to_string(), (me, self.t));
        self.see(me, path, Some(b.clone()));
        self.landed(me, path, &b, false);
        true
    }

    fn remove(&mut self, me: usize, path: &str) {
        if term_of(path).is_some() || records_of(path).is_some() {
            self.wrong(format!("mac{me} removed {path}"));
        }
        if let Some(key) = entry_of(path) {
            if !self.cfg.draft && !self.files.contains_key(&format!("{}/{key}.json", journal::REJECTED)) {
                self.wrong(format!("mac{me} removed entry {key} without setting it aside"));
            }
        }
        self.files.remove(path);
        self.see(me, path, None);
    }

    /// A file's bytes landed: checked by what it is.
    fn landed(&mut self, me: usize, path: &str, b: &[u8], created: bool) {
        if let Some(e) = term_of(path) {
            if !created {
                return self.wrong(format!("mac{me} wrote over term {e}'s file"));
            }
            let Ok(t) = serde_json::from_slice::<Term>(b) else { return self.wrong(format!("term {e}'s file isn't a term")) };
            let prev = e.checked_sub(1).and_then(|p| self.files.get(&term::path(p))).and_then(|b| serde_json::from_slice::<Term>(b).ok());
            if t.term != e || !self.macs.iter().any(|m| m.id == t.member) {
                return self.wrong(format!("term {e}'s file names term {} and {}", t.term, t.member));
            }
            if let Some(p) = prev.filter(|p| !term::app_at_least(&t.app, &p.app)) {
                self.wrong(format!("term {e} has app {}, older than term {}'s {}", t.app, p.term, p.app));
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
            let first = e == 1 && created;
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
            self.landed.insert(e, (r.seq, handled));
        } else if let Some(key) = entry_of(path) {
            if let Ok(en) = serde_json::from_slice::<Entry>(b) {
                self.written.insert(key, en.step == "bogus");
            }
        }
    }

    // What the Macs say they believe.

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

    fn ack(&mut self, me: usize, term: u64, done: &[(String, usize)]) {
        if self.made.contains_key(&(term + 1)) {
            self.count("acknowledgements after a later term began");
        }
        for (key, to) in done {
            self.acked.insert(key.clone());
            if !self.landed.get(&term).is_some_and(|(_, h)| h.contains(key)) {
                self.wrong(format!("mac{me} acknowledged {key} as term {term}'s before a snapshot of it named it"));
            }
            self.deliver(*to, Msg::Ack { key: key.clone(), term });
        }
    }

    /// The checks at the end of a run: one Mac leads the last term, and its records name every
    /// entry ever written.
    fn finish(&mut self) {
        let Some(&h) = self.made.keys().max() else { return self.wrong("no term was made".into()) };
        if self.cfg.draft {
            let r = self.files.get(DRAFT).and_then(|b| serde_json::from_slice::<Records>(b).ok()).unwrap_or_default();
            return self.lost(&r, "the shared records");
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
        let lost: Vec<String> = self.written.iter().filter(|(k, bogus)| if **bogus { !r.rejected.contains_key(*k) } else { !r.reflected.contains(*k) }).map(|(k, _)| k.clone()).collect();
        for k in lost {
            self.wrong(format!("entry {k} is lost: {whose} don't name it"));
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

/// A Mac's hold on the world: its NAS, its messages, its clock.
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

    fn now(&self) -> Result<u64> {
        self.op(|w, me| w.clock(me))
    }

    fn inbox(&self) -> Result<Vec<Msg>> {
        self.op(|w, me| std::mem::take(&mut w.macs[me].inbox))
    }

    fn app(&self) -> Result<String> {
        self.op(|w, me| app(w.macs[me].app))
    }

    fn send(&self, to: usize, msgs: Vec<Msg>) -> Result<()> {
        self.op(|w, _| msgs.into_iter().for_each(|m| w.deliver(to, m)))
    }

    fn ack(&self, term: u64, done: &[(String, usize)]) -> Result<()> {
        self.op(|w, me| w.ack(me, term, done))
    }

    fn view(&self, e: u64) -> Result<()> {
        self.op(|w, me| w.view(me, e))
    }

    fn leads(&self, e: Option<u64>) -> Result<()> {
        self.op(|w, me| w.leads(me, e))
    }

    /// Waits for its next loop.
    fn idle(&self) -> Result<()> {
        self.op(|w, me| {
            let j = w.rng.below(5);
            w.macs[me].ready_at = w.t + LOOP_S + j;
        })
    }
}

impl Nas for Sim {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<bool> {
        let made = self.op(|w, me| w.create(me, path))?;
        if made {
            self.op(|w, me| w.fill(me, path, bytes))?;
        }
        Ok(made)
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let tmp = format!("{path}.mac{}.tmp", self.me);
        self.op(|w, _| w.write_tmp(&tmp, bytes))?;
        for _ in 0..BUSY_TRIES {
            if self.op(|w, me| w.rename(me, &tmp, path))? {
                return Ok(());
            }
        }
        self.op(|w, _| {
            w.files.remove(&tmp);
            w.tmps.remove(&tmp);
        })?;
        bail!("rename over {path}: busy")
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.op(|w, me| w.read(me, path))
    }

    fn exists(&self, path: &str) -> Result<bool> {
        self.op(|w, me| w.fetch(me, path).is_some())
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.op(|w, me| w.list(me, dir))
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.op(|w, me| w.remove(me, path))
    }
}

/// A term this Mac leads.
struct Lead {
    term: Term,
    records: Records,
    hand: Handover,
    /// The entries members told it of and it hasn't acknowledged, by who told it.
    told: BTreeMap<String, usize>,
    /// Changes not saved yet.
    dirty: bool,
    /// Entries refused, to set aside once that's saved.
    refused: Vec<(String, String)>,
}

/// A handover this Mac passed on: waiting for its target to lead, its own records kept to take
/// the lead back with.
struct Passing {
    term: u64,
    hand: Handover,
    records: Records,
}

/// A simulated Mac, running the protocol as the agent will.
struct Mac {
    sim: Sim,
    k: usize,
    cfg: Cfg,
    rng: Rng,
    me: Member,
    cur: Current,
    lead: Option<Lead>,
    passing: Option<Passing>,
    /// Records of its own to take a term up from (a re-assertion or take-back whose take-up failed).
    spare: Option<Records>,
    /// The highest term it led.
    led: u64,
    /// It must re-assert before acting as lead (a gap, a restart).
    must: bool,
    mine: Mine,
    /// Its clock at its last loop's end; whether that loop took over a minute.
    last: u64,
    long: bool,
    jobs: u64,
}

impl Mac {
    fn new(sim: Sim, k: usize, seed: u64, cfg: Cfg) -> Mac {
        let me = Member { id: id(k), host: format!("mac{k}"), app: app(0) };
        let rng = Rng(seed ^ (k as u64 + 1).wrapping_mul(0xA076_1D64_78BD_642F));
        Mac { sim, k, cfg, rng, me, cur: Current::default(), lead: None, passing: None, spare: None, led: 0, must: false, mine: Mine::default(), last: 0, long: false, jobs: 0 }
    }

    fn run(mut self) {
        loop {
            let r = if self.cfg.draft { self.draft_turn() } else { self.turn() };
            if let Err(e) = r {
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

    /// One loop of the agent, the pool's way.
    fn turn(&mut self) -> Result<()> {
        let start = self.sim.now()?;
        let msgs = self.sim.inbox()?;
        let app = self.sim.app()?;
        let restarted = app != self.me.app;
        self.me.app = app;
        self.learn(start)?;
        // A lead: steps down once a later term exists; re-asserts after a gap or a restart.
        if self.lead.as_ref().is_some_and(|l| self.cur.term > l.term.term) {
            self.step_down("a later term exists")?;
        }
        if self.lead.is_some() && (handover::gap(self.last, start) || self.long || restarted) {
            self.must = true;
        }
        if self.lead.is_none() {
            self.must = false;
        }
        if self.must {
            self.reassert(start, if restarted { "restarted into a newer app" } else { "re-asserted after a gap" })?;
        }
        // A member: takes up a term that names it.
        if self.lead.is_none() {
            if let Some(t) = self.cur.lead.clone().filter(|t| t.member == self.me.id && t.term > self.led) {
                self.take_up(t)?;
            }
        }
        let (mut tells, mut asks, mut takeover) = (Vec::new(), Vec::new(), None);
        for m in msgs {
            match m {
                Msg::Tell { key, from } => tells.push((key, from)),
                Msg::Ack { key, term } => self.mine.acked(&key, term),
                Msg::HandTo(to) => asks.push(to),
                Msg::TakeOver { force } => takeover = Some(force),
            }
        }
        if self.lead.is_some() {
            self.lead_loop(start, tells, asks)?;
        } else if let Some(t) = self.cur.lead.as_ref().filter(|_| !asks.is_empty()) {
            // (Passed on to the lead it knows.)
            self.sim.send(mac_of(&t.member), asks.into_iter().map(Msg::HandTo).collect())?;
        }
        if self.passing.is_some() {
            self.passing_loop(start)?;
        }
        if let Some(force) = takeover {
            self.take_over(start, force)?;
        }
        self.member_loop(start)?;
        self.beat(start)?;
        let end = self.sim.now()?;
        self.long = end.saturating_sub(start) > handover::GAP_S;
        self.last = end;
        self.sim.idle()
    }

    /// Learns the current term: once from the NAS (making term 1 if it's this Mac's to make), then
    /// by checking the next.
    fn learn(&mut self, now: u64) -> Result<()> {
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
        self.sim.view(self.cur.term)
    }

    fn step_down(&mut self, why: &str) -> Result<()> {
        if let Some(l) = self.lead.take() {
            self.sim.note(|| format!("mac{} steps down from term {}: {why}", self.k, l.term.term));
            self.sim.leads(None)?;
        }
        Ok(())
    }

    /// Makes the next term naming itself and takes it up from its own records; another's made
    /// first, it steps down.
    fn reassert(&mut self, now: u64, how: &str) -> Result<()> {
        let Some(l) = self.lead.take() else { return Ok(()) };
        match term::claim(&self.sim, &self.cur, &self.me, how, now) {
            Ok(Some(t)) => {
                self.must = false;
                self.sim.leads(None)?;
                self.cur = Current { term: t.term, lead: Some(t.clone()) };
                self.sim.view(t.term)?;
                self.spare = Some(l.records);
                self.take_up(t)?;
            }
            Ok(None) => {
                self.must = false;
                self.sim.note(|| format!("mac{} re-asserting finds term {} made: steps down", self.k, self.cur.term + 1));
                self.sim.leads(None)?;
            }
            Err(e) => {
                self.lead = Some(l);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Takes up term `t`, which names this Mac.
    fn take_up(&mut self, t: Term) -> Result<()> {
        let own = self.spare.take();
        match records::take_up(&self.sim, &t, own.clone(), None, &check) {
            Ok(up) => {
                if !up.merged.applied.is_empty() {
                    self.sim.count("take-ups replaying the journal");
                }
                self.led = t.term;
                self.sim.leads(Some(t.term))?;
                if let Err(e) = term::write_hint(&self.sim, &t) {
                    if self.sim.over() {
                        return Err(e);
                    }
                }
                self.lead = Some(Lead { term: t, records: up.records, hand: Handover::Leading, told: BTreeMap::new(), dirty: false, refused: Vec::new() });
            }
            Err(e) => {
                if self.sim.over() {
                    return Err(e);
                }
                self.spare = own;
                self.sim.count("take-ups tried again");
                self.sim.note(|| format!("mac{} can't take up term {} now: {e:#}", self.k, t.term));
            }
        }
        Ok(())
    }

    /// The lead's loop: merge what members told it, save, acknowledge what's saved, hand over.
    fn lead_loop(&mut self, now: u64, tells: Vec<(String, usize)>, asks: Vec<String>) -> Result<()> {
        let sim = self.sim.clone();
        let l = self.lead.as_mut().expect("leading");
        for (k, from) in tells {
            l.told.insert(k, from);
        }
        let keys: Vec<String> = l.told.keys().filter(|k| !l.records.handles(k)).cloned().collect();
        let m = records::merge(&sim, &mut l.records, &keys, &check);
        if !m.applied.is_empty() {
            // (An entry an earlier lead acknowledged, missing from this term's records until its
            // member told this lead again: what telling every new lead of every entry is for.)
            sim.quiet(|w| {
                let again = m.applied.iter().filter(|k| w.acked.contains(*k)).count() as u64;
                *w.counts.entry("acknowledged entries merged again by a later lead").or_default() += again;
            });
        }
        if !m.applied.is_empty() || !m.refused.is_empty() {
            l.dirty = true;
            l.refused.extend(m.refused.iter().cloned());
        }
        if l.dirty {
            match l.records.save(&sim) {
                Ok(()) => {
                    l.dirty = false;
                    for (k, why) in std::mem::take(&mut l.refused) {
                        journal::set_aside(&sim, &k, &why)?;
                    }
                }
                Err(e) => {
                    if sim.over() {
                        return Err(e);
                    }
                    sim.count("saves tried again");
                }
            }
        }
        if !l.dirty {
            let done: Vec<(String, usize)> = l.told.iter().filter(|(k, _)| l.records.handles(k)).map(|(k, f)| (k.clone(), *f)).collect();
            if !done.is_empty() {
                sim.ack(l.term.term, &done)?;
                for (k, _) in &done {
                    l.told.remove(k);
                }
            }
        }
        let settled = (!l.dirty && m.waiting.is_empty()).then_some(l.records.seq);
        // An ask, checked (§6.3): a live member on an app new enough, not this Mac.
        let ask = match asks.last() {
            Some(to) if *to != self.me.id && l.hand == Handover::Leading => Beat::read(&sim, to)?.filter(|b| !b.out_of_touch(now) && term::app_at_least(&b.app, &self.me.app)).map(|_| to.clone()),
            _ => None,
        };
        let target = match &l.hand {
            Handover::Offered { to, .. } | Handover::Settling { to, .. } => Beat::read(&sim, to)?,
            _ => None,
        };
        let e = l.term.term;
        let step = l.hand.step(e, &Seen { now, ask: ask.as_deref(), target: target.as_ref(), settled });
        if let Do::Pass { to, seq } = step {
            let b = target.filter(|b| b.member == to);
            let t = b.ok_or_else(|| anyhow::anyhow!("{to}'s heartbeat isn't readable")).and_then(|b| Term::after(&self.cur, &b.member(), &format!("handed over by {}", self.me.host), now));
            let mut t = match t {
                Ok(t) => t,
                Err(err) => {
                    l.hand.abandon();
                    self.sim.note(|| format!("mac{} can't hand term {e} to {to}: {err:#}", self.k));
                    return Ok(());
                }
            };
            t.seq = Some(seq);
            match term::make(&sim, &t) {
                Ok(true) => {
                    let at = sim.now()?;
                    let mut l = self.lead.take().expect("leading");
                    l.hand.passed(at);
                    sim.leads(None)?;
                    self.passing = Some(Passing { term: e, hand: l.hand, records: l.records });
                }
                Ok(false) => self.step_down("another made the next term first")?,
                Err(err) => {
                    if sim.over() {
                        return Err(err);
                    }
                    self.lead.as_mut().expect("leading").hand.abandon();
                }
            }
        }
        Ok(())
    }

    /// A handover passed on: over once the target leads, taken back after two minutes.
    fn passing_loop(&mut self, now: u64) -> Result<()> {
        let Some(mut p) = self.passing.take() else { return Ok(()) };
        if self.cur.term > p.term + 1 {
            return Ok(());
        }
        let Handover::Passed { to, .. } = p.hand.clone() else { return Ok(()) };
        let target = Beat::read(&self.sim, &to)?;
        match p.hand.step(p.term, &Seen { now, target: target.as_ref(), ..Default::default() }) {
            Do::Done => self.sim.note(|| format!("mac{} sees {to} lead term {}", self.k, p.term + 1)),
            Do::TakeBack { to } if self.cur.term == p.term + 1 => match term::claim(&self.sim, &self.cur, &self.me, &format!("taken back: {to} didn't take up"), now) {
                Ok(Some(t)) => {
                    self.cur = Current { term: t.term, lead: Some(t.clone()) };
                    self.sim.view(t.term)?;
                    self.spare = Some(p.records);
                    self.take_up(t)?;
                }
                Ok(None) => {}
                Err(e) => {
                    self.passing = Some(p);
                    return Err(e);
                }
            },
            _ => self.passing = Some(p),
        }
        Ok(())
    }

    /// The owner's "Take it": when the lead is out of touch, or forced.
    fn take_over(&mut self, now: u64, force: bool) -> Result<()> {
        if self.lead.is_some() || self.passing.is_some() {
            return Ok(());
        }
        let gone = match &self.cur.lead {
            Some(t) if t.member == self.me.id => false,
            Some(t) => Beat::read(&self.sim, &t.member)?.is_none_or(|b| b.out_of_touch(now)),
            None => true,
        };
        if !gone && !force {
            return Ok(());
        }
        let made = if self.cur.term == 0 { term::bootstrap(&self.sim, &self.me, now, true).map(|t| t.filter(|t| t.member == self.me.id)) } else { term::claim(&self.sim, &self.cur, &self.me, &format!("taken over by {}", self.me.host), now) };
        match made {
            Ok(Some(t)) => {
                self.cur = Current { term: t.term, lead: Some(t.clone()) };
                self.sim.view(t.term)?;
                self.take_up(t)?;
            }
            Ok(None) => {}
            Err(e) => {
                if self.sim.over() {
                    return Err(e);
                }
                self.sim.note(|| format!("mac{} can't take over: {e:#}", self.k));
            }
        }
        Ok(())
    }

    /// A job's entry now and then, a job reading the records, and telling the lead.
    fn member_loop(&mut self, now: u64) -> Result<()> {
        if now < T0 + self.cfg.faults && self.rng.chance(P_JOB) {
            let e = self.job(now);
            match journal::write(&self.sim, &e) {
                Ok(key) => self.mine.wrote(&key),
                Err(err) => {
                    if self.sim.over() {
                        return Err(err);
                    }
                    self.sim.note(|| format!("mac{} can't write its entry: {err:#}", self.k));
                }
            }
        }
        if self.cur.term > 0 && self.rng.chance(P_READ) {
            if let Some(r) = Records::load(&self.sim, self.cur.term)? {
                if let Err(why) = consistent(&r) {
                    self.sim.op(|w, me| w.wrong(format!("mac{me} read term {}'s records at {}, not consistent: {why}", r.term, r.seq)))?;
                }
            }
        }
        if let Some(t) = &self.cur.lead {
            let msgs: Vec<Msg> = self.mine.to_tell(self.cur.term).into_iter().map(|key| Msg::Tell { key, from: self.k }).collect();
            if !msgs.is_empty() {
                self.sim.send(mac_of(&t.member), msgs)?;
            }
        }
        Ok(())
    }

    /// A job's hand-off: a unit built with a new key, now and then a prune, or one of a step that
    /// doesn't exist (refused).
    fn job(&mut self, now: u64) -> Entry {
        self.jobs += 1;
        let t = TARGETS[self.rng.below(TARGETS.len() as u64) as usize];
        let k = format!("k{}x{}", self.k, self.jobs);
        let (step, h) = match self.rng.below(20) {
            0 => ("bogus", Handoff { changes: [(logical(t), Some(content(t, "bogus")))].into(), done: Some(("unit".into(), vec![(t.into(), "bogus".into())])), ..Default::default() }),
            1 | 2 => ("prune", Handoff { changes: [(logical(t), None)].into(), done: Some(("prune".into(), vec![(format!("unit {t}"), String::new())])), ..Default::default() }),
            _ => ("unit", Handoff { changes: [(logical(t), Some(content(t, &k)))].into(), pending: [(content(t, &k), "sha".into())].into(), done: Some(("unit".into(), vec![(t.into(), k.clone())])), ..Default::default() }),
        };
        Entry { member: self.me.id.clone(), lease: LeaseId { term: self.cur.term, n: self.jobs * 8 + self.k as u64 }, step: step.into(), handoff: h, at: now }
    }

    /// Its heartbeat: the term it leads, a handover's state, its answer to an offer.
    fn beat(&mut self, now: u64) -> Result<()> {
        let mut ready_for = None;
        if let (None, Some(t)) = (&self.lead, &self.cur.lead) {
            if t.member != self.me.id {
                if let Some(b) = Beat::read(&self.sim, &t.member)? {
                    let able = term::app_at_least(&self.me.app, &t.app) && self.rng.chance(0.9);
                    ready_for = handover::ready_for(&b, &self.me.id, able);
                    if ready_for.is_some() {
                        // (Its records loaded read-only, as the agent will.)
                        Records::load(&self.sim, t.term)?;
                    }
                }
            }
        }
        let (leads, handing_to) = match (&self.lead, &self.passing) {
            (Some(l), _) => (Some(l.term.term), l.hand.handing_to(l.term.term)),
            (None, Some(p)) => (None, p.hand.handing_to(p.term)),
            _ => (None, None),
        };
        let b = Beat { member: self.me.id.clone(), host: self.me.host.clone(), app: self.me.app.clone(), beat: now, leads, handing_to, ready_for, addresses: Vec::new() };
        if let Err(e) = b.write(&self.sim) {
            if self.sim.over() {
                return Err(e);
            }
        }
        Ok(())
    }

    /// One loop of the first draft's scheme: terms as the pool's, but one shared records file the
    /// lead reads, merges the journal into, writes after checking the term, and the merged entries
    /// then taken off the journal.
    fn draft_turn(&mut self) -> Result<()> {
        let now = self.sim.now()?;
        let msgs = self.sim.inbox()?;
        self.learn(now)?;
        if self.lead.as_ref().is_some_and(|l| self.cur.term > l.term.term) {
            self.step_down("a later term exists")?;
        }
        if self.lead.is_none() {
            if let Some(t) = self.cur.lead.clone().filter(|t| t.member == self.me.id && t.term > self.led) {
                self.led = t.term;
                self.sim.leads(Some(t.term))?;
                self.lead = Some(Lead { term: t, records: Records::default(), hand: Handover::Leading, told: BTreeMap::new(), dirty: false, refused: Vec::new() });
            }
        }
        for m in msgs {
            if let Msg::TakeOver { force } = m {
                if self.lead.is_none() {
                    let gone = match &self.cur.lead {
                        Some(t) => Beat::read(&self.sim, &t.member)?.is_none_or(|b| b.out_of_touch(now)),
                        None => true,
                    };
                    if (gone || force) && self.cur.term > 0 {
                        if let Ok(Some(t)) = term::claim(&self.sim, &self.cur, &self.me, "taken over", now) {
                            self.cur = Current { term: t.term, lead: Some(t) };
                            self.sim.view(self.cur.term)?;
                        }
                    }
                }
            }
        }
        if let Some(e) = self.lead.as_ref().map(|l| l.term.term) {
            let mut r: Records = match self.sim.read(DRAFT)? {
                Some(b) => serde_json::from_slice(&b)?,
                None => Records::load(&self.sim, 1)?.unwrap_or_default(),
            };
            let keys = journal::list(&self.sim, None)?;
            let m = records::merge(&self.sim, &mut r, &keys, &check);
            if !m.applied.is_empty() || !m.refused.is_empty() {
                // Check the term, then write: two steps, and the write's rename may land late.
                if term::next(&self.sim, e)? {
                    self.step_down("a later term exists")?;
                } else if self.sim.write_whole(DRAFT, &serde_json::to_vec(&r)?).is_ok() {
                    for k in m.applied.iter().chain(m.refused.iter().map(|(k, _)| k)) {
                        self.sim.remove(&journal::path(k))?;
                    }
                }
            }
        }
        if now < T0 + self.cfg.faults && self.rng.chance(P_JOB) {
            let e = self.job(now);
            journal::write(&self.sim, &e).ok();
        }
        let b = Beat { member: self.me.id.clone(), host: self.me.host.clone(), app: self.me.app.clone(), beat: now, leads: self.lead.as_ref().map(|l| l.term.term), ..Default::default() };
        b.write(&self.sim).ok();
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
                if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Mac::new(sim, k, seed, cfg).run())) {
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

/// Runs every seed of `seeds` on all cores: the seeds that went wrong, with what did, and the
/// counts of what happened in all.
fn run_all(seeds: Range<u64>, cfg: Cfg) -> (Vec<(u64, Vec<String>)>, Counts) {
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
                let r = run(seed, cfg, false);
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
fn check_all(seeds: Range<u64>, cfg: Cfg) -> Counts {
    let n = seeds.end - seeds.start;
    let (bad, counts) = run_all(seeds, cfg);
    if let Some((seed, _)) = bad.first() {
        let r = run(*seed, cfg, true);
        let tail = &r.trace[r.trace.len().saturating_sub(200)..];
        let seeds: Vec<u64> = bad.iter().map(|b| b.0).take(20).collect();
        panic!("{} of {n} schedules went wrong (seeds {seeds:?}…); seed {seed}:\n{}\n\nits last events:\n{}", bad.len(), r.wrong.join("\n"), tail.join("\n"));
    }
    counts
}

#[test]
fn the_pool_keeps_its_invariants_through_thousands_of_schedules() {
    let counts = check_all(0..1500, Cfg::pool());
    eprintln!("{counts:#?}");
    // (A simulator that never got there would pass too.)
    for what in ["handed over", "taken back", "taken over", "re-asserted", "restarted", "sleeps", "busy renames", "late renames over another's write", "take-ups tried again", "saves tried again"] {
        assert!(counts.get(what).is_some_and(|&n| n >= 3), "{what}: {:?} in all the runs", counts.get(what));
    }
}

#[test]
#[ignore]
fn the_pool_keeps_its_invariants_through_a_long_run() {
    // POOL_SIM_SEEDS seeds (100,000 by default) of four hours' faults each.
    let n = std::env::var("POOL_SIM_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let counts = check_all(1_000_000..1_000_000 + n, Cfg { faults: 4 * 3600, end: 4 * 3600 + 1200, draft: false });
    eprintln!("{counts:#?}");
}

#[test]
fn the_first_drafts_shared_records_lose_an_update_under_a_delayed_rename() {
    let cfg = Cfg { draft: true, ..Cfg::pool() };
    let (bad, _) = run_all(0..300, cfg);
    let Some(&(seed, _)) = bad.iter().find(|(_, wrong)| wrong.iter().any(|w| w.contains("is lost"))) else {
        panic!("the first draft's scheme lost nothing in 300 schedules: the simulator doesn't find its lost update")
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
    let (a, b) = (run(11, Cfg::pool(), true), run(11, Cfg::pool(), true));
    assert!(a.trace.len() > 20 && a.counts["steps"] > 500, "{} events, {:?}", a.trace.len(), a.counts);
    assert_eq!(a.trace, b.trace);
    assert_eq!(a.wrong, b.wrong);
}

#[test]
#[ignore]
fn a_seeds_events() {
    // POOL_SIM_SEED's events (POOL_SIM_DRAFT=1: the first draft's), to look into one.
    let seed = std::env::var("POOL_SIM_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let cfg = Cfg { draft: std::env::var_os("POOL_SIM_DRAFT").is_some(), ..Cfg::pool() };
    let r = run(seed, cfg, true);
    eprintln!("{}\n\n{}\n\n{:#?}", r.trace.join("\n"), r.wrong.join("\n"), r.counts);
}
